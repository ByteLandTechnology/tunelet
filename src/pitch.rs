//! The acquisition and tracking of pitch with fixed-point cycle-difference
//! estimates.
//!
//! Acquisition checks that repeated minima agree. Tracking refines the period
//! in a window of 40 lags. The frequency bounds limit pitch correction and
//! the feedback to the host.

mod search;
pub(crate) mod tracker;

use crate::error::{check_range, ConfigProblem};
use crate::Error;

/// The acquisition sensitivity and the frequency bounds for pitch correction
/// and host feedback.
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
    /// If an estimate is outside the bounds, pitch correction stays at unity
    /// and feedback reports the input as unvoiced.
    pub min_frequency_hz: f32,
    /// The upper frequency bound in hertz (inclusive), 40..=2000.
    ///
    /// The default is 1100. It must be strictly greater than
    /// `min_frequency_hz`. If an estimate is outside the bounds, pitch
    /// correction stays at unity. Feedback then reports the input as unvoiced,
    /// with zero frequency and zero period.
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
    /// Applies the frequency bounds to a kept estimate for correction and host
    /// reports.
    ///
    /// A tolerance of 0.6 samples keeps boundary tones stable across
    /// sub-sample tracker updates. The function clamps a valid frequency to
    /// the inclusive bounds. If the estimate is not voiced or is out of
    /// range, all fields of the result are zero.
    pub(crate) fn report(self, mut estimate: PitchEstimate) -> PitchEstimate {
        if !estimate.voiced || estimate.period_samples <= 0.0 || estimate.frequency_hz <= 0.0 {
            return PitchEstimate::default();
        }
        let rate = estimate.frequency_hz * estimate.period_samples;
        let min_period = rate / self.max_frequency_hz - 0.6;
        let max_period = rate / self.min_frequency_hz + 0.6;
        if estimate.period_samples >= min_period && estimate.period_samples <= max_period {
            estimate.frequency_hz = estimate
                .frequency_hz
                .clamp(self.min_frequency_hz, self.max_frequency_hz);
            estimate
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
