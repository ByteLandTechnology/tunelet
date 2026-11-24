//! The acquisition and tracking of pitch with fixed-point cycle-difference
//! estimates.
//!
//! Acquisition checks that repeated minima agree inside the active vocal
//! range. Tracking refines the period in a window of 40 lags and checks cycle
//! clarity. The active vocal range filters acquisition, tracking and feedback
//! to the host.

mod search;
pub(crate) mod tracker;

use crate::error::{check_range, ConfigProblem};
use crate::numeric::fixed::Q6;
use crate::{Error, SampleRate};

/// A vocal range preset or custom bounds for pitch detection and reports.
///
/// The default is [`Self::Custom`], which uses `min_frequency_hz` and
/// `max_frequency_hz` from [`PitchConfig`]. Each other variant sets fixed
/// inclusive bounds for pitch acquisition, tracking and feedback reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub enum VocalRange {
    /// Custom bounds from `min_frequency_hz` and `max_frequency_hz` in
    /// [`PitchConfig`].
    #[default]
    Custom,
    /// The soprano range, 240..=1100 Hz.
    Soprano,
    /// The alto and tenor range, 120..=600 Hz.
    AltoTenor,
    /// The baritone and bass range, 45..=360 Hz.
    BaritoneBass,
}

/// The vocal range, acquisition sensitivity and bounds for pitch detection and
/// host reports.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub struct PitchConfig {
    /// The vocal range preset, or [`VocalRange::Custom`] for custom bounds.
    ///
    /// The default is [`VocalRange::Custom`]. If you select a named preset,
    /// its frequency bounds override `min_frequency_hz` and `max_frequency_hz`
    /// for detection and reports. Validation still checks those two fields.
    pub vocal_range: VocalRange,
    /// The normalized cycle-difference threshold for acquisition, 0.002..=0.2.
    ///
    /// The default is 0.1. Larger values accept less exact cycle agreement.
    /// This value controls acquisition. It is not an output gain or an
    /// amplitude gate.
    pub threshold: f32,
    /// The lower bound for [`VocalRange::Custom`] in hertz (inclusive),
    /// 40..=2000.
    ///
    /// The default is 40. It must be strictly less than `max_frequency_hz`,
    /// also when a named preset in `vocal_range` overrides it. Pitches below
    /// the active range do not acquire or track, and feedback reports them as
    /// unvoiced.
    pub min_frequency_hz: f32,
    /// The upper bound for [`VocalRange::Custom`] in hertz (inclusive),
    /// 40..=2000.
    ///
    /// The default is 1100. It must be strictly greater than
    /// `min_frequency_hz`, also when a named preset in `vocal_range` overrides
    /// it. Pitches above the active range do not acquire or track, and
    /// feedback reports them as unvoiced.
    pub max_frequency_hz: f32,
}

impl Default for PitchConfig {
    fn default() -> Self {
        Self {
            vocal_range: VocalRange::Custom,
            threshold: 0.1,
            min_frequency_hz: 40.0,
            max_frequency_hz: 1100.0,
        }
    }
}

impl PitchConfig {
    /// Returns the active inclusive frequency bounds `(min_hz, max_hz)`.
    pub(crate) fn frequency_bounds(self) -> (f32, f32) {
        match self.vocal_range {
            VocalRange::Custom => (self.min_frequency_hz, self.max_frequency_hz),
            VocalRange::Soprano => (240.0, 1100.0),
            VocalRange::AltoTenor => (120.0, 600.0),
            VocalRange::BaritoneBass => (45.0, 360.0),
        }
    }

    /// Converts the active frequency bounds into inclusive Q6 period limits
    /// `(min_period, max_period)` at `rate`.
    pub(crate) fn period_bounds(self, rate: SampleRate) -> (Q6, Q6) {
        let (min_hz, max_hz) = self.frequency_bounds();
        let scale = f64::from(rate.as_u32()) * 64.0;
        let raw_min = (scale / f64::from(max_hz)).floor() as i32;
        let raw_max = (scale / f64::from(min_hz)).ceil() as i32;
        let min_slop = (raw_min >> 10).max(8);
        let max_slop = (raw_max >> 10).max(8);
        (
            Q6::from_bits((raw_min - min_slop).max(1)),
            Q6::from_bits(raw_max.saturating_add(max_slop)),
        )
    }

    /// Applies the active Q6 period limits at `rate` to a kept estimate.
    ///
    /// If the estimate is not voiced or its period is outside the active
    /// range, the result is the unvoiced value with all fields zero. The
    /// function does not change the tracker state or the correction state.
    pub(crate) fn report(
        self,
        rate: SampleRate,
        period: Q6,
        estimate: PitchEstimate,
    ) -> PitchEstimate {
        let (min_period, max_period) = self.period_bounds(rate);
        if estimate.voiced && period >= min_period && period <= max_period {
            estimate
        } else {
            PitchEstimate::default()
        }
    }

    /// Checks the threshold, then each frequency range, then the order of the
    /// frequencies.
    ///
    /// Validation checks `min_frequency_hz` and `max_frequency_hz` also when
    /// `vocal_range` selects a named preset.
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
