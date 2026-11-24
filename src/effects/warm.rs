//! Asymmetric saturation with gain compensation and a high-pass output filter.

use crate::effects::{SlidingPeak, WINDOW_FRAMES};
use crate::error::check_range;
use crate::filter::{Biquad, Design};
use crate::numeric::fixed::{Fixed, Q12, Q14, Q20, Q28};
use crate::numeric::{db_to_gain, envelope_decay};
use crate::{Error, SampleRate};

/// Asymmetric input saturation, followed by a 50 Hz high-pass filter.
///
/// Warm operates after Punch and before pitch detection. Its adaptive
/// normalization tracks a 100-frame lookahead peak window on each enabled
/// frame. When Warm is disabled, the envelope and filter histories do not
/// change. When Warm is enabled again, they continue from their last state,
/// unless the processor was reset.
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
    /// The 100-frame lookahead peak window.
    window: SlidingPeak,
    /// The Q28 peak envelope.
    peak: Q28,
    /// The lookahead frames left in the current upward peak ramp.
    ramp_remaining: usize,
    /// The Q20 normalization gain. The stage divides by it again after the
    /// nonlinear curve.
    compression: Q20,
    /// The Q28 envelope coefficient for a 200 ms half-life on each frame.
    smooth: Q28,
    /// The linear saturation drive in Q12.
    drive: Q12,
    /// The Q14 compensation from the configured drive.
    output_gain: Q14,
    /// The Q28 threshold of -30 dB. If the peak envelope is not above it, the
    /// normalization uses a fixed headroom.
    input_threshold: Q28,
    /// The Q20 normalization gain of +30 dB for input below the threshold.
    output_headroom: Q20,
}
impl Warm {
    /// Prepares the fixed high-pass filter and the initial normalization
    /// state.
    pub(crate) fn new(rate: SampleRate, config: WarmConfig) -> Self {
        let mut filter = Biquad::new();
        filter.set_design(Design::high_pass_one_pole(rate.hz(), 50.0));
        let mut result = Self {
            filter,
            window: SlidingPeak::new(),
            peak: Q28::ZERO,
            ramp_remaining: 0,
            compression: Q20::ONE,
            smooth: Q28::from_f64(envelope_decay(200.0, rate.hz() as f64, 1)),
            drive: Q12::default(),
            output_gain: Q14::default(),
            input_threshold: Q28::from_f64(db_to_gain(-30.0)),
            output_headroom: Q20::from_f64(db_to_gain(30.0)),
        };
        result.set_config(config);
        result
    }
    /// Returns the normalization gain as f32. The stage state does not change.
    pub(crate) fn normalization_gain(&self) -> f32 {
        (self.compression.to_bits() as f64 / f64::from(1u32 << 20)) as f32
    }
    /// Updates the drive and the output compensation, and keeps the envelope
    /// and filter state.
    pub(crate) fn set_config(&mut self, config: WarmConfig) {
        self.drive = Q12::from_f64(db_to_gain(config.drive_db as f64));
        self.output_gain = Q14::from_f64(db_to_gain(-(config.drive_db as f64) * 0.35));
    }
    /// Clears the filter memories, the peak window and the normalization, and
    /// keeps the coefficients.
    pub(crate) fn reset(&mut self) {
        self.filter.reset();
        self.window.reset();
        self.peak = Q28::ZERO;
        self.ramp_remaining = 0;
        self.compression = Q20::ONE;
    }
    /// Updates the lookahead peak envelope and processes one delayed PCM
    /// sample through the saturation and the high-pass filter.
    ///
    /// The steps are:
    ///
    /// 1. Update the 100-frame lookahead peak and move the envelope toward it.
    /// 2. Normalize the delayed sample.
    /// 3. Shape the positive and negative halves differently.
    /// 4. Remove the normalization.
    /// 5. Apply the high-pass filter and clip the result.
    pub(crate) fn process_sample(&mut self, input: i16, lookahead: i16) -> i16 {
        let ahead = Q14::from_bits(i32::from(lookahead) >> 1)
            .abs()
            .rescale::<28>();
        let target = self.window.push(ahead);
        if target > self.peak {
            if self.ramp_remaining == 0 {
                self.ramp_remaining = WINDOW_FRAMES;
            }
            let remaining = self.ramp_remaining as i32;
            let step = (target.to_bits() - self.peak.to_bits()) / remaining;
            self.peak = if self.ramp_remaining == 1 {
                target
            } else {
                Q28::from_bits(self.peak.to_bits() + step)
            };
            self.ramp_remaining -= 1;
        } else {
            self.ramp_remaining = 0;
            self.peak = target + self.smooth.mul(self.peak - target);
        }
        self.compression = if self.peak > self.input_threshold {
            Fixed::<24>::ONE.div(self.peak)
        } else {
            self.output_headroom
        };
        let mut sample = Q14::from_bits(i32::from(input) >> 1);
        sample = sample
            .mul::<20, 14>(self.compression)
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
