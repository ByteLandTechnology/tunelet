//! The acquisition and tracking of pitch with fixed-point cycle-difference
//! estimates.
//!
//! Acquisition checks that repeated minima agree. Tracking refines the period
//! in a window of 40 lags. The reporting bounds filter only the feedback to
//! the host.

mod search;
pub(crate) mod tracker;

use crate::error::{check_range, ConfigProblem};
use crate::Error;

/// The acquisition sensitivity and the bounds for pitch reports to the host.
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
    /// The lower bound of the feedback in hertz (inclusive), 40..=2000.
    ///
    /// The default is 40. It must be strictly less than `max_frequency_hz`.
    /// These bounds do not change the tracking, the correction or the
    /// resampling. A feedback query applies the current bounds to the kept
    /// analysis. It does not process another sample.
    pub min_frequency_hz: f32,
    /// The upper bound of the feedback in hertz (inclusive), 40..=2000.
    ///
    /// The default is 1100. It must be strictly greater than
    /// `min_frequency_hz`. Results outside the bounds are reported as
    /// unvoiced, with zero frequency and zero period.
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
    /// Applies only the bounds for host reports to a kept estimate.
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
        check_range(MIN_FIELD, self.min_frequency_hz, 40.0, 2000.0)?;
        check_range(MAX_FIELD, self.max_frequency_hz, 40.0, 2000.0)?;
        check_frequency_order(self.min_frequency_hz, self.max_frequency_hz)
    }
}

/// The dotted field path of the lower pitch bound.
const MIN_FIELD: &str = "resynthesis.pitch.min_frequency_hz";
/// The dotted field path of the upper pitch bound.
const MAX_FIELD: &str = "resynthesis.pitch.max_frequency_hz";

/// Checks that `min_frequency_hz` is strictly below `max_frequency_hz`.
fn check_frequency_order(min_frequency_hz: f32, max_frequency_hz: f32) -> Result<(), Error> {
    if min_frequency_hz >= max_frequency_hz {
        return Err(Error::config(
            MIN_FIELD,
            ConfigProblem::Unordered { other: MAX_FIELD },
        ));
    }
    Ok(())
}

/// Optional changes to [`PitchConfig`].
///
/// Each `None` field keeps its active value on the processor. After a merge,
/// `min_frequency_hz` must stay strictly below `max_frequency_hz`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(default, rename_all = "camelCase")
)]
pub struct PitchConfigPatch {
    /// The normalized cycle-difference threshold for acquisition, 0.002..=0.2.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        serde(skip_serializing_if = "Option::is_none"),
        tsify(optional)
    )]
    pub threshold: Option<f32>,
    /// The lower bound of the feedback in hertz (inclusive), 40..=2000.
    ///
    /// It must stay strictly below `max_frequency_hz`.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        serde(skip_serializing_if = "Option::is_none"),
        tsify(optional)
    )]
    pub min_frequency_hz: Option<f32>,
    /// The upper bound of the feedback in hertz (inclusive), 40..=2000.
    ///
    /// It must stay strictly above `min_frequency_hz`.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        serde(skip_serializing_if = "Option::is_none"),
        tsify(optional)
    )]
    pub max_frequency_hz: Option<f32>,
}

impl PitchConfigPatch {
    /// Checks all supplied fields, and checks their order if both bounds are
    /// present.
    pub(crate) fn validate(self) -> Result<(), Error> {
        if let Some(threshold) = self.threshold {
            check_range("resynthesis.pitch.threshold", threshold, 0.002, 0.2)?;
        }
        if let Some(min_frequency_hz) = self.min_frequency_hz {
            check_range(MIN_FIELD, min_frequency_hz, 40.0, 2000.0)?;
        }
        if let Some(max_frequency_hz) = self.max_frequency_hz {
            check_range(MAX_FIELD, max_frequency_hz, 40.0, 2000.0)?;
        }
        if let (Some(min_frequency_hz), Some(max_frequency_hz)) =
            (self.min_frequency_hz, self.max_frequency_hz)
        {
            check_frequency_order(min_frequency_hz, max_frequency_hz)?;
        }
        Ok(())
    }

    /// Validates all supplied fields, checks the frequency order against
    /// `config` and writes the fields into `config`.
    pub(crate) fn apply_to(self, config: &mut PitchConfig) -> Result<(), Error> {
        if let Some(threshold) = self.threshold {
            check_range("resynthesis.pitch.threshold", threshold, 0.002, 0.2)?;
        }
        if let Some(min_frequency_hz) = self.min_frequency_hz {
            check_range(MIN_FIELD, min_frequency_hz, 40.0, 2000.0)?;
        }
        if let Some(max_frequency_hz) = self.max_frequency_hz {
            check_range(MAX_FIELD, max_frequency_hz, 40.0, 2000.0)?;
        }
        let min_frequency_hz = self.min_frequency_hz.unwrap_or(config.min_frequency_hz);
        let max_frequency_hz = self.max_frequency_hz.unwrap_or(config.max_frequency_hz);
        if self.min_frequency_hz.is_some() || self.max_frequency_hz.is_some() {
            check_frequency_order(min_frequency_hz, max_frequency_hz)?;
        }
        if let Some(threshold) = self.threshold {
            config.threshold = threshold;
        }
        config.min_frequency_hz = min_frequency_hz;
        config.max_frequency_hz = max_frequency_hz;
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
