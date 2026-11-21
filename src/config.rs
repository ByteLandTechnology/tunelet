//! Sample rates, block limits and the complete processor configuration.

use crate::effects::alien::{AlienConfig, AlienPatch};
use crate::effects::punch::{PunchConfig, PunchPatch};
use crate::effects::reverb::{ReverbConfig, ReverbPatch};
use crate::effects::warm::{WarmConfig, WarmPatch};
use crate::error::{check_range, ConfigProblem};
use crate::resynthesis::ResynthesisPatch;
use crate::Error;
use crate::ResynthesisConfig;
use std::num::NonZeroUsize;

/// The sample rate of the input and the output.
///
/// The default is 44.1 kHz. The prepared delay and filter layouts support only
/// the listed rates. To convert an integer device rate, use
/// [`SampleRate::try_from`]. The rate of a processor is fixed at construction.
/// To change the rate, you must make a new processor.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum SampleRate {
    /// 44.1 kHz.
    #[default]
    Hz44100,
    /// 48 kHz.
    Hz48000,
}

impl SampleRate {
    /// Returns the rate in hertz.
    pub const fn as_u32(self) -> u32 {
        match self {
            Self::Hz44100 => 44_100,
            Self::Hz48000 => 48_000,
        }
    }

    /// Returns the rate in hertz, as `f32`.
    pub fn hz(self) -> f32 {
        self.as_u32() as f32
    }
}

impl TryFrom<u32> for SampleRate {
    type Error = Error;

    /// Converts a device sample rate in hertz.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedSampleRate`] with `hz`, unless the value is
    /// exactly 44,100 or 48,000. The function does not resample and does not
    /// select the nearest rate.
    fn try_from(hz: u32) -> Result<Self, Self::Error> {
        match hz {
            44_100 => Ok(Self::Hz44100),
            48_000 => Ok(Self::Hz48000),
            _ => Err(Error::UnsupportedSampleRate { hz }),
        }
    }
}

/// The fixed limits of a processor: the sample rate and the maximum number
/// of frames for each call.
///
/// The default is 44.1 kHz, with a limit of 1,024 frames for each block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PrepareConfig {
    /// The input and output rate.
    ///
    /// The default is [`SampleRate::Hz44100`].
    pub sample_rate: SampleRate,
    /// The maximum number of frames in one process call or drain call.
    ///
    /// The default is 1,024. The limit must not be zero, but a block can be
    /// empty. The limit does not change the delay-line capacities of the
    /// algorithm. It also does not require full blocks.
    pub max_block_frames: NonZeroUsize,
}

impl Default for PrepareConfig {
    fn default() -> Self {
        Self {
            sample_rate: SampleRate::Hz44100,
            max_block_frames: NonZeroUsize::new(1024).expect("1024 is nonzero"),
        }
    }
}

impl PrepareConfig {
    /// Returns new processor limits and does not allocate audio storage.
    ///
    /// `max_block_frames` counts mono input samples or stereo output frames.
    /// It does not count the total samples in the two output channels. It is
    /// a limit for each call, and smaller blocks and empty blocks are also
    /// valid. The value does not have to be a power of two.
    /// [`crate::Processor::new`] allocates the processor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] with the field
    /// `prepare.max_block_frames` if the maximum is zero. The field type is
    /// [`NonZeroUsize`]. Thus, this invariant also holds if you edit the field
    /// after construction.
    pub fn new(sample_rate: SampleRate, max_block_frames: usize) -> Result<Self, Error> {
        let max_block_frames = NonZeroUsize::new(max_block_frames).ok_or_else(|| {
            Error::config(
                "prepare.max_block_frames",
                ConfigProblem::OutOfRange {
                    min: 1.0,
                    max: usize::MAX as f64,
                },
            )
        })?;
        Ok(Self {
            sample_rate,
            max_block_frames,
        })
    }

    /// Returns a copy with `sample_rate` set to the given value.
    pub const fn with_sample_rate(mut self, sample_rate: SampleRate) -> Self {
        self.sample_rate = sample_rate;
        self
    }

    /// Returns a copy with `max_block_frames` set to the given nonzero limit.
    pub const fn with_max_block_frames(mut self, max_block_frames: NonZeroUsize) -> Self {
        self.max_block_frames = max_block_frames;
        self
    }
}

/// The settings for the complete signal chain, in the order of the stages.
///
/// The default enables chromatic pitch correction, one choir voice and unity
/// output gain. Punch, Warm, formants, Alien and reverb are disabled. This
/// configuration does active pitch processing. It is not a bypass with zero
/// latency.
///
/// Change a copy, then apply it with [`crate::Processor::set_config`] or
/// [`crate::ControlEvent::Configure`]. The validation examines all fields
/// before a change has an effect. This includes the fields of disabled stages.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub struct ProcessorConfig {
    /// The input dynamics and saturation stage, which applies first.
    ///
    /// It is disabled by default.
    pub punch: PunchConfig,
    /// The asymmetric saturation stage after Punch.
    ///
    /// It is disabled by default.
    pub warm: WarmConfig,
    /// The stage for pitch tracking, correction, voice resynthesis and
    /// formant control.
    ///
    /// The default is [`ResynthesisConfig::default`].
    pub resynthesis: ResynthesisConfig,
    /// The reverse-loop effect after resynthesis.
    ///
    /// It is disabled by default.
    pub alien: AlienConfig,
    /// The echo or reverberation stage after Alien.
    ///
    /// It is disabled by default.
    pub reverb: ReverbConfig,
    /// The linear gain after all effects, 0..=64.
    ///
    /// The default is 1 (unity). Zero mutes the output, but the internal
    /// processing continues. The gain is quantized in steps of 1/256 before
    /// the fixed-point multiplication and the final PCM clipping. Thus, a
    /// positive value below 1/256 also mutes the output.
    pub output_gain: f32,
}

impl Default for ProcessorConfig {
    /// Returns chromatic correction with zero retune time, one choir voice,
    /// disabled effects, disabled formants and unity gain.
    fn default() -> Self {
        Self {
            punch: PunchConfig::default(),
            warm: WarmConfig::default(),
            resynthesis: ResynthesisConfig::default(),
            alien: AlienConfig::default(),
            reverb: ReverbConfig::default(),
            output_gain: 1.0,
        }
    }
}

impl ProcessorConfig {
    /// Returns a copy with `punch` set to the given configuration.
    pub const fn with_punch(mut self, punch: PunchConfig) -> Self {
        self.punch = punch;
        self
    }

    /// Returns a copy with `warm` set to the given configuration.
    pub const fn with_warm(mut self, warm: WarmConfig) -> Self {
        self.warm = warm;
        self
    }

    /// Returns a copy with `resynthesis` set to the given configuration.
    pub const fn with_resynthesis(mut self, resynthesis: ResynthesisConfig) -> Self {
        self.resynthesis = resynthesis;
        self
    }

    /// Returns a copy with `alien` set to the given configuration.
    pub const fn with_alien(mut self, alien: AlienConfig) -> Self {
        self.alien = alien;
        self
    }

    /// Returns a copy with `reverb` set to the given configuration.
    pub const fn with_reverb(mut self, reverb: ReverbConfig) -> Self {
        self.reverb = reverb;
        self
    }

    /// Returns a copy with `output_gain` set to the given value, 0..=64.
    pub const fn with_output_gain(mut self, output_gain: f32) -> Self {
        self.output_gain = output_gain;
        self
    }

    /// Merges each present stage patch and gain field of `patch` into this
    /// configuration.
    ///
    /// Fields that are `None` do not change. This method does not validate the
    /// result.
    pub fn merge_patch(&mut self, patch: ProcessorPatch) {
        if let Some(punch) = patch.punch {
            self.punch.merge_patch(punch);
        }
        if let Some(warm) = patch.warm {
            self.warm.merge_patch(warm);
        }
        if let Some(resynthesis) = patch.resynthesis {
            self.resynthesis.merge_patch(resynthesis);
        }
        if let Some(alien) = patch.alien {
            self.alien.merge_patch(alien);
        }
        if let Some(reverb) = patch.reverb {
            self.reverb.merge_patch(reverb);
        }
        if let Some(output_gain) = patch.output_gain {
            self.output_gain = output_gain;
        }
    }

    /// Checks all stages and the final gain, and does not change state.
    ///
    /// The validation uses the signal order: Punch, Warm, resynthesis, Alien,
    /// reverb, then output gain. It includes disabled stages and the controls
    /// of voice modes that are not active.
    ///
    /// # Errors
    ///
    /// Returns the first [`Error::InvalidConfig`]. The error names the field
    /// and its range, finiteness or order problem. The function does not clamp
    /// or normalize values.
    pub fn validate(self) -> Result<(), Error> {
        self.punch.validate()?;
        self.warm.validate()?;
        self.resynthesis.validate()?;
        self.alien.validate()?;
        self.reverb.validate()?;
        validate_output_gain(self.output_gain)
    }
}

impl From<ProcessorPatch> for ProcessorConfig {
    fn from(patch: ProcessorPatch) -> Self {
        let mut config = Self::default();
        config.merge_patch(patch);
        config
    }
}

/// Partial changes for [`ProcessorConfig`].
///
/// Give this structure to [`crate::Processor::new`],
/// [`crate::Processor::update_config`] or [`crate::ControlEvent::Patch`]. Only
/// fields and sub-patches that are `Some` overwrite the target settings in
/// [`ProcessorConfig::merge_patch`]. All fields are `None` by default.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(default, rename_all = "camelCase")
)]
pub struct ProcessorPatch {
    /// Partial changes for the Punch input stage.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        serde(skip_serializing_if = "Option::is_none"),
        tsify(optional)
    )]
    pub punch: Option<PunchPatch>,
    /// Partial changes for the Warm saturation stage.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        serde(skip_serializing_if = "Option::is_none"),
        tsify(optional)
    )]
    pub warm: Option<WarmPatch>,
    /// Partial changes for pitch tracking, correction, voices and formants.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        serde(skip_serializing_if = "Option::is_none"),
        tsify(optional)
    )]
    pub resynthesis: Option<ResynthesisPatch>,
    /// Partial changes for the Alien reverse-loop stage.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        serde(skip_serializing_if = "Option::is_none"),
        tsify(optional)
    )]
    pub alien: Option<AlienPatch>,
    /// Partial changes for the reverb stage.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        serde(skip_serializing_if = "Option::is_none"),
        tsify(optional)
    )]
    pub reverb: Option<ReverbPatch>,
    /// The linear gain after all effects, 0..=64.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        serde(skip_serializing_if = "Option::is_none"),
        tsify(optional)
    )]
    pub output_gain: Option<f32>,
}

impl ProcessorPatch {
    /// Returns an empty patch with all fields set to `None`.
    pub const fn new() -> Self {
        Self {
            punch: None,
            warm: None,
            resynthesis: None,
            alien: None,
            reverb: None,
            output_gain: None,
        }
    }

    /// Sets `punch` on this patch.
    pub const fn with_punch(mut self, punch: PunchPatch) -> Self {
        self.punch = Some(punch);
        self
    }

    /// Sets `warm` on this patch.
    pub const fn with_warm(mut self, warm: WarmPatch) -> Self {
        self.warm = Some(warm);
        self
    }

    /// Sets `resynthesis` on this patch.
    pub const fn with_resynthesis(mut self, resynthesis: ResynthesisPatch) -> Self {
        self.resynthesis = Some(resynthesis);
        self
    }

    /// Sets `alien` on this patch.
    pub const fn with_alien(mut self, alien: AlienPatch) -> Self {
        self.alien = Some(alien);
        self
    }

    /// Sets `reverb` on this patch.
    pub const fn with_reverb(mut self, reverb: ReverbPatch) -> Self {
        self.reverb = Some(reverb);
        self
    }

    /// Sets `output_gain` on this patch, 0..=64.
    pub const fn with_output_gain(mut self, output_gain: f32) -> Self {
        self.output_gain = Some(output_gain);
        self
    }
}

/// Checks the output gain range.
///
/// `ProcessorConfig` and `ControlEvent::OutputGain` use the same range.
pub(crate) fn validate_output_gain(gain: f32) -> Result<(), Error> {
    check_range("output_gain", gain, 0.0, 64.0)
}
