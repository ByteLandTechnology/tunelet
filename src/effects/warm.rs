//! Asymmetric saturation with gain compensation and a high-pass output filter.

use crate::error::check_range;
use crate::filter::{Biquad, Design};
use crate::numeric::fixed::{Fixed, Q14, Q22, Q24, Q28, Q30};
use crate::numeric::{db_to_gain, envelope_decay};
use crate::{Error, SampleRate};

/// Asymmetric input saturation, followed by a 50 Hz high-pass filter.
///
/// Warm operates after Punch and before pitch detection. Its adaptive
/// normalization updates on each enabled frame with an immediate attack and a
/// 200 ms release half-life. When Warm is disabled, the envelope and filter
/// histories do not change. When Warm is enabled again, they continue from
/// their last state, unless the processor was reset.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub struct WarmConfig {
    /// Enables the saturation stage. The default is `false`.
    pub enabled: bool,
    /// The drive in decibels, 0..=6. The default is 3 dB.
    /// The output gain changes by -0.35 dB for each dB of drive. At 0 dB, the
    /// envelope normalization and the output high-pass filter still operate.
    pub drive_db: f32,
}
impl Default for WarmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            drive_db: 3.0,
        }
    }
}
impl WarmConfig {
    /// Validates all fields, also when Warm is disabled.
    ///
    /// `drive_db` must be finite and in its documented inclusive range.
    /// Validation does not clamp values. It does not change the processor
    /// state.
    ///
    /// # Errors
    ///
    /// Returns the first [`Error::InvalidConfig`] with its dotted field path
    /// and the problem.
    pub fn validate(self) -> Result<(), Error> {
        validate_drive_db(self.drive_db)
    }
}

/// Checks the range of the drive of Warm.
pub(crate) fn validate_drive_db(drive_db: f32) -> Result<(), Error> {
    check_range("warm.drive_db", drive_db, 0.0, 6.0)
}

/// An adaptive asymmetric saturator with one persistent high-pass output
/// filter.
#[derive(Debug)]
pub(crate) struct Warm {
    /// The validated controls that set the saturation drive.
    config: WarmConfig,
    /// The 50 Hz one-pole high-pass output filter.
    filter: Biquad,
    /// The Q28 peak envelope of the input.
    peak: Q28,
    /// The Q22 normalization gain. The stage divides by it again after the
    /// nonlinear curve.
    compression: Q22,
    /// The Q30 release coefficient for a 200 ms half-life on each frame.
    smooth: Q30,
    /// The linear saturation drive in Q24.
    drive: Q24,
    /// The Q28 compensation from the configured drive.
    output_gain: Q28,
    /// The Q28 threshold of -30 dB. If the peak envelope is not above it, the
    /// normalization targets a fixed headroom.
    input_threshold: Q28,
    /// The Q22 normalization gain of +30 dB for input below the threshold.
    output_headroom: Q22,
}
impl Warm {
    /// Prepares the fixed high-pass filter and the initial normalization
    /// state.
    pub(crate) fn new(rate: SampleRate, config: WarmConfig) -> Self {
        let mut filter = Biquad::new();
        filter.set_design(Design::high_pass_one_pole(rate.hz(), 50.0));
        let mut result = Self {
            config,
            filter,
            peak: Q28::ZERO,
            compression: Q22::ONE,
            smooth: Q30::from_f64(envelope_decay(200.0, rate.hz() as f64, 1)),
            drive: Q24::ZERO,
            output_gain: Q28::ZERO,
            input_threshold: Q28::from_f64(db_to_gain(-30.0)),
            output_headroom: Q22::from_f64(db_to_gain(30.0)),
        };
        result.configure_drive_db();
        result
    }
    /// Returns the Q22 normalization gain as f32. The stage state does not
    /// change.
    pub(crate) fn normalization_gain(&self) -> f32 {
        (self.compression.to_bits() as f64 / f64::from(1u32 << 22)) as f32
    }
    /// Quantizes the drive and its output compensation unconditionally.
    fn configure_drive_db(&mut self) {
        self.drive = Q24::from_f64(db_to_gain(self.config.drive_db as f64));
        self.output_gain = Q28::from_f64(db_to_gain(-(self.config.drive_db as f64) * 0.35));
    }
    /// Stores the configuration. If the drive changed, updates the drive and
    /// the output compensation, and keeps the envelope and filter state.
    pub(crate) fn set_config(&mut self, config: WarmConfig) {
        let old = self.config;
        self.config = config;
        if old.drive_db != config.drive_db {
            self.configure_drive_db();
        }
    }
    /// Stores the drive. If the value changed, updates the quantized drive and
    /// its output compensation.
    pub(crate) fn set_drive_db(&mut self, drive_db: f32) {
        let changed = self.config.drive_db != drive_db;
        self.config.drive_db = drive_db;
        if changed {
            self.configure_drive_db();
        }
    }
    /// Clears the filter memories, the peak envelope and the normalization,
    /// and keeps the coefficients.
    pub(crate) fn reset(&mut self) {
        self.filter.reset();
        self.peak = Q28::ZERO;
        self.compression = Q22::ONE;
    }
    /// Processes one PCM sample through the saturation and the high-pass
    /// filter.
    ///
    /// The steps are:
    ///
    /// 1. Update the peak envelope and the normalization gain.
    /// 2. Normalize the sample.
    /// 3. Shape the positive and negative halves differently.
    /// 4. Remove the normalization.
    /// 5. Apply the high-pass filter and clip the result.
    ///
    /// If an attack increases the peak, the gain decreases immediately.
    /// During release, the gain moves smoothly toward the target.
    pub(crate) fn process_sample(&mut self, input: i16) -> i16 {
        let mut sample: Q28 = Fixed::<15>::from_bits(i32::from(input)).rescale();
        let level = sample.abs();
        self.peak = if level > self.peak {
            level
        } else {
            self.smooth.mul(self.peak)
        };
        let target = if self.peak > self.input_threshold {
            Q28::ONE.div::<28, 22>(self.peak).min(self.output_headroom)
        } else {
            self.output_headroom
        };
        self.compression = if target <= self.compression {
            target
        } else {
            target - self.smooth.mul(target - self.compression)
        };
        sample = sample
            .mul::<22, 28>(self.compression)
            .clamp(-Q28::ONE, Q28::ONE);
        if sample > Q28::ZERO {
            let knee: Q24 = (self.drive - Q24::ONE).mul(sample);
            sample = (self.drive - knee).mul(sample);
        } else {
            sample = self.drive.mul::<28, 28>(sample).max(-Q28::ONE);
        }
        sample = sample.div(self.compression.max(Q22::ONE));
        let output = Q14::from_bits(self.output_gain.mul::<28, 15>(sample).to_bits());
        self.filter
            .process_sample(output)
            .to_bits()
            .clamp(-32767, 32767) as i16
    }
}
