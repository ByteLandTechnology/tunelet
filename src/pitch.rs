//! The acquisition and tracking of pitch with fixed-point cycle-difference
//! estimates.
//!
//! Acquisition checks that repeated minima agree. Tracking refines the period
//! in a window of 40 lags. The frequency bounds filter the feedback, the
//! tracking state and the pitch correction.

mod search;
pub(crate) mod tracker;

use crate::error::{check_range, ConfigProblem};
use crate::numeric::fixed::Q6;
use crate::{Error, SampleRate};

/// The acquisition sensitivity and the frequency bounds for pitch tracking and
/// correction.
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
    /// The lower bound of tracked pitch in hertz (inclusive), 40..=2000.
    ///
    /// The default is 40. It must be strictly less than `max_frequency_hz`.
    /// Estimates outside the inclusive bounds are reported as unvoiced and
    /// untracked, and they bypass pitch correction. A feedback query applies
    /// the current bounds to the kept analysis and does not process another
    /// sample.
    pub min_frequency_hz: f32,
    /// The upper bound of tracked pitch in hertz (inclusive), 40..=2000.
    ///
    /// The default is 1100. It must be strictly greater than
    /// `min_frequency_hz`. Estimates outside the inclusive bounds are reported
    /// as unvoiced and untracked, with zero frequency and zero period, and
    /// they bypass pitch correction.
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
    /// Returns `true` if `period` gives a frequency inside the inclusive
    /// bounds at `rate`.
    pub(crate) fn contains_period(self, rate: SampleRate, period: Q6) -> bool {
        if period <= Q6::ZERO {
            return false;
        }
        let period_samples = (period.to_bits() as f64 / 64.0) as f32;
        let frequency_hz = rate.hz() / period_samples;
        frequency_hz >= self.min_frequency_hz && frequency_hz <= self.max_frequency_hz
    }

    /// Applies the frequency bounds for host reports to a kept estimate.
    ///
    /// If the estimate is not voiced or is out of range, the result is the
    /// unvoiced value with all fields zero. The function does not change the
    /// tracker state or the correction state.
    pub(crate) fn report(self, estimate: PitchEstimate) -> PitchEstimate {
        if estimate.voiced
            && estimate.frequency_hz >= self.min_frequency_hz
            && estimate.frequency_hz <= self.max_frequency_hz
        {
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
