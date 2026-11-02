//! Asymmetric saturation with gain compensation and a high-pass output filter.

use crate::error::check_range;
use crate::filter::{Biquad, Design};
use crate::numeric::db_to_gain;
use crate::numeric::fixed::{Fixed, Q12, Q14, Q8};
use crate::{Error, SampleRate};

/// The update interval of the peak follower, in frames.
const WINDOW_FRAMES: usize = 100;

/// Asymmetric input saturation, followed by a 50 Hz high-pass filter.
///
/// Warm operates after Punch and before pitch detection. Its adaptive
/// normalization updates every 100 enabled frames. When Warm is disabled,
/// the envelope and filter histories do not change. When Warm is enabled
/// again, they continue from their last state, unless the processor was
/// reset.
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
        check_range("warm.drive_db", self.drive_db, 0.0, 6.0)
    }
}

/// An adaptive asymmetric saturator with one persistent high-pass output
/// filter.
#[derive(Debug)]
pub(crate) struct Warm {
    /// The 50 Hz one-pole high-pass output filter.
    filter: Biquad,
    /// The kept Q14 peak envelope from completed windows.
    peak: Q14,
    /// The largest absolute Q14 sample in the current 100-frame window.
    window_peak: Q14,
    /// The number of enabled samples in this window.
    counter: usize,
    /// The Q8 normalization gain. The stage divides by it again after the
    /// nonlinear curve.
    compression: Q8,
    /// The Q14 envelope coefficient for a 200 ms half-life at the window
    /// cadence.
    smooth: Q14,
    /// The linear saturation drive in Q12.
    drive: Q12,
    /// The Q14 compensation from the configured drive.
    output_gain: Q14,
    /// The Q14 threshold of -30 dB. If the peak envelope is not above it, the
    /// normalization uses a fixed headroom.
    input_threshold: Q14,
    /// The Q8 normalization gain of +30 dB for input below the threshold.
    output_headroom: Q8,
}
impl Warm {
    /// Prepares the fixed high-pass filter and the initial normalization
    /// state.
    pub(crate) fn new(rate: SampleRate, config: WarmConfig) -> Self {
        let mut filter = Biquad::new();
        filter.set_design(Design::high_pass_one_pole(rate.hz(), 50.0));
        let mut result = Self {
            filter,
            peak: Q14::default(),
            window_peak: Q14::default(),
            counter: 0,
            compression: Q8::ONE,
            smooth: Q14::from_f64(0.5_f64.powf(1.0 / (rate.hz() as f64 * 0.2 / 100.0))),
            drive: Q12::default(),
            output_gain: Q14::default(),
            input_threshold: Q14::from_f64(db_to_gain(-30.0)),
            output_headroom: Q8::from_f64(db_to_gain(30.0)),
        };
        result.set_config(config);
        result
    }
    /// Updates the drive and the output compensation, and keeps the envelope
    /// and filter state.
    pub(crate) fn set_config(&mut self, config: WarmConfig) {
        self.drive = Q12::from_f64(db_to_gain(config.drive_db as f64));
        self.output_gain = Q14::from_f64(db_to_gain(-(config.drive_db as f64) * 0.35));
    }
    /// Clears the filter memories, the peak windows and the normalization,
    /// and keeps the coefficients.
    pub(crate) fn reset(&mut self) {
        self.filter.reset();
        self.peak = Q14::default();
        self.window_peak = Q14::default();
        self.counter = 0;
        self.compression = Q8::ONE;
    }
    /// Processes one PCM sample through the saturation and the high-pass
    /// filter.
    ///
    /// The steps are:
    ///
    /// 1. Normalize the sample.
    /// 2. Shape the positive and negative halves differently.
    /// 3. Remove the normalization.
    /// 4. Apply the high-pass filter and clip the result.
    ///
    /// The window update uses the current sample for the branch with an
    /// immediate attack. It uses the window peak for the smoothed branch.
    /// These values are not interchangeable. This difference is intentional.
    pub(crate) fn process_sample(&mut self, input: i16) -> i16 {
        let mut sample = Q14::from_bits(i32::from(input) >> 1);
        self.window_peak = self.window_peak.max(sample.abs());
        self.counter += 1;
        if self.counter == WINDOW_FRAMES {
            self.counter = 0;
            self.peak = if sample.abs() > self.peak {
                sample.abs()
            } else {
                self.window_peak + self.smooth.mul(self.peak - self.window_peak)
            };
            self.compression = if self.peak > self.input_threshold {
                Fixed::<22>::ONE.div(self.peak)
            } else {
                self.output_headroom
            };
            self.window_peak = Q14::default();
        }
        sample = sample
            .mul::<8, 14>(self.compression)
            .clamp(-Q14::ONE, Q14::ONE);
        if sample > Q14::default() {
            let knee: Q12 = (self.drive - Q12::ONE).mul(sample);
            sample = (self.drive - knee).mul(sample);
        } else {
            sample = self.drive.mul::<14, 14>(sample).max(-Q14::ONE);
        }
        sample = sample.div(self.compression);
        let output: Q14 = self.output_gain.mul::<14, 14>(sample) << 1;
        self.filter
            .process_sample(output)
            .to_bits()
            .clamp(-32767, 32767) as i16
    }
}
