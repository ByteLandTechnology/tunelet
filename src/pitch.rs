//! The acquisition and tracking of pitch with fixed-point cycle-difference
//! estimates.
//!
//! Acquisition checks that repeated minima agree. Tracking refines the period
//! in a window of 40 lags. Both pitch tracking and host feedback enforce the
//! active [`VocalRange`] frequency limits.

mod search;
pub(crate) mod tracker;

use crate::error::{check_range, ConfigProblem};
use crate::Error;

/// The frequency preset for pitch acquisition, pitch tracking and host reports.
///
/// The default is [`Self::Auto`]. Presets restrict pitch detection and
/// feedback to the selected vocal register. Notes outside the active bounds
/// stay unvoiced and bypass pitch shift and harmony voices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub enum VocalRange {
    /// Custom bounds from `min_frequency_hz` and `max_frequency_hz` in
    /// [`PitchConfig`].
    Custom,
    /// The soprano register, 200..=1100 Hz.
    Soprano,
    /// The alto and tenor register, 100..=720 Hz.
    AltoTenor,
    /// The baritone and bass register, 45..=360 Hz.
    BaritoneBass,
    /// The full vocal register, 40..=1100 Hz. This is the default.
    #[default]
    Auto,
}

/// The acquisition sensitivity, vocal range preset, custom frequency limits
/// and sibilant bypass control.
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
    /// The active vocal register preset. The default is [`VocalRange::Auto`].
    ///
    /// [`VocalRange::Custom`] uses `min_frequency_hz` and `max_frequency_hz`.
    /// The other presets use fixed frequency limits. Both the pitch tracker
    /// and [`crate::Processor::feedback`] enforce the active limits.
    pub vocal_range: VocalRange,
    /// The custom lower bound in hertz (inclusive), 40..=2000.
    ///
    /// The default is 40. It must be strictly less than `max_frequency_hz`.
    /// [`VocalRange::Custom`] enforces this bound in pitch detection and in
    /// feedback reports. Validation checks this field in all vocal ranges.
    pub min_frequency_hz: f32,
    /// The custom upper bound in hertz (inclusive), 40..=2000.
    ///
    /// The default is 1100. It must be strictly greater than
    /// `min_frequency_hz`. [`VocalRange::Custom`] enforces this bound in pitch
    /// detection and in feedback reports. Results outside the active bounds
    /// are unvoiced, with zero frequency and zero period.
    pub max_frequency_hz: f32,
    /// Enables the 3.5 kHz sibilant crossover split. The default is `true`.
    ///
    /// If this field is `true`, active sibilants split into a low band and a
    /// high band. The high band bypasses pitch shift and harmony voices at
    /// unity pitch.
    pub sibilant_bypass: bool,
}

impl Default for PitchConfig {
    fn default() -> Self {
        Self {
            threshold: 0.1,
            vocal_range: VocalRange::Auto,
            min_frequency_hz: 40.0,
            max_frequency_hz: 1100.0,
            sibilant_bypass: true,
        }
    }
}

impl PitchConfig {
    /// Returns the inclusive lower and upper frequency limits in hertz for the
    /// active [`VocalRange`].
    pub(crate) fn bounds(self) -> (f32, f32) {
        match self.vocal_range {
            VocalRange::Custom => (self.min_frequency_hz, self.max_frequency_hz),
            VocalRange::Soprano => (200.0, 1100.0),
            VocalRange::AltoTenor => (100.0, 720.0),
            VocalRange::BaritoneBass => (45.0, 360.0),
            VocalRange::Auto => (40.0, 1100.0),
        }
    }

    /// Applies the active frequency bounds to a kept estimate for host reports.
    ///
    /// If the estimate is not voiced or is out of range, the result is the
    /// unvoiced value with all fields zero. The function does not change the
    /// tracker state or the correction state.
    pub(crate) fn report(self, estimate: PitchEstimate) -> PitchEstimate {
        let (min_hz, max_hz) = self.bounds();
        if estimate.voiced && estimate.frequency_hz >= min_hz && estimate.frequency_hz <= max_hz {
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
