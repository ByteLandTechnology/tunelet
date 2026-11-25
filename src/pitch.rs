//! The acquisition and tracking of pitch with fixed-point cycle-difference
//! estimates.
//!
//! Acquisition checks that repeated minima agree. Tracking refines the period
//! in a window of 40 lags. The frequency bounds gate host feedback, the
//! `tracking` flag and pitch correction.

mod search;
pub(crate) mod tracker;

use crate::error::{check_range, ConfigProblem};
use crate::numeric::fixed::Q6;
use crate::{Error, SampleRate};

/// The acquisition sensitivity and the frequency bounds for pitch analysis
/// and correction.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub struct PitchConfig {
    /// The normalized cycle-difference threshold for acquisition, 0.002..=0.2.
    ///
    /// The default is 0.1. Larger values accept less exact cycle agreement.
    /// This value controls acquisition. It is not an output gain or an
    /// amplitude gate.
    pub threshold: f32,
    /// The lower frequency bound in hertz (inclusive), 40..=2000.
    ///
    /// The default is 40. It must be strictly less than `max_frequency_hz`.
    /// If a pitch is outside the bounds, it reports as unvoiced, sets
    /// [`crate::Telemetry::tracking`] to `false` and moves the correction
    /// ratio toward 1.0. Feedback and telemetry queries apply the current
    /// bounds to the kept analysis.
    pub min_frequency_hz: f32,
    /// The upper frequency bound in hertz (inclusive), 40..=2000.
    ///
    /// The default is 1100. It must be strictly greater than
    /// `min_frequency_hz`. If a result is outside the bounds, it reports as
    /// unvoiced with zero frequency and zero period, and it bypasses target
    /// selection.
    pub max_frequency_hz: f32,
}

impl Default for PitchConfig {
    fn default() -> Self {
        Self {
            threshold: 0.1,
            min_frequency_hz: 40.0,
            max_frequency_hz: 1100.0,
        }
    }
}

impl PitchConfig {
    /// Returns `true` if a positive Q6 period is inside the configured
    /// frequency bounds at `rate`.
    ///
    /// A tolerance of `1/256` of the boundary period (at least `1/8` of a
    /// sample) absorbs the parabolic quantization at the boundaries.
    pub(crate) fn in_bounds(self, rate: SampleRate, period: Q6) -> bool {
        let bits = period.to_bits();
        if bits <= 0 {
            return false;
        }
        let scale = f64::from(rate.as_u32()) * 64.0;
        let min_exact = (scale / f64::from(self.max_frequency_hz)).floor() as i32;
        let max_exact = (scale / f64::from(self.min_frequency_hz)).ceil() as i32;
        let min_period = min_exact - (min_exact >> 8).max(8);
        let max_period = max_exact + (max_exact >> 8).max(8);
        bits >= min_period && bits <= max_period
    }

    /// Converts a kept Q6 period to a host estimate and applies the frequency
    /// bounds.
    ///
    /// If the period is not positive or is out of range, the result is the
    /// unvoiced value with all fields zero.
    pub(crate) fn report(self, rate: SampleRate, period: Q6) -> PitchEstimate {
        if self.in_bounds(rate, period) {
            let period_samples = (period.to_bits() as f64 / 64.0) as f32;
            let frequency_hz =
                (rate.hz() / period_samples).clamp(self.min_frequency_hz, self.max_frequency_hz);
            PitchEstimate {
                period_samples,
                frequency_hz,
                voiced: true,
            }
        } else {
            PitchEstimate::default()
        }
    }

    /// Checks the threshold, then each frequency range, then the order of the
    /// frequencies.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] for a value that is not finite or is
    /// out of range. If the bounds are equal or reversed, the error has the
    /// field `resynthesis.pitch.min_frequency_hz` and the problem
    /// [`ConfigProblem::Unordered`]. The `other` value of that problem is
    /// `resynthesis.pitch.max_frequency_hz`.
    pub fn validate(self) -> Result<(), Error> {
        check_range("resynthesis.pitch.threshold", self.threshold, 0.002, 0.2)?;
        let (min, max) = (
            "resynthesis.pitch.min_frequency_hz",
            "resynthesis.pitch.max_frequency_hz",
        );
        check_range(min, self.min_frequency_hz, 40.0, 2000.0)?;
        check_range(max, self.max_frequency_hz, 40.0, 2000.0)?;
        if self.min_frequency_hz >= self.max_frequency_hz {
            return Err(Error::config(min, ConfigProblem::Unordered { other: max }));
        }
        Ok(())
    }
}

/// A pitch estimate as a period in samples and a frequency in hertz.
///
/// The kept analysis and the host report use this type. The host gets it
/// after the reporting bounds apply. Unvoiced values are zero.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PitchEstimate {
    /// The pitch period in samples. It is zero when the input is unvoiced.
    pub period_samples: f32,
    /// The frequency in hertz. It is zero when the input is unvoiced.
    pub frequency_hz: f32,
    /// `true` if the estimate contains a pitch.
    pub voiced: bool,
}

impl PitchEstimate {
    /// If the estimate is voiced, returns the pitch as a fractional MIDI note.
    ///
    /// If the estimate is unvoiced or its frequency is not positive, the
    /// result is `None`.
    pub(crate) fn midi_note(self) -> Option<f32> {
        if self.voiced && self.frequency_hz > 0.0 {
            Some(69.0 + 12.0 * (self.frequency_hz / 440.0).log2())
        } else {
            None
        }
    }
}
