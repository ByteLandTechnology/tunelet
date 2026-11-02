//! Sample rates, block limits and the complete processor configuration.

use crate::effects::alien::AlienConfig;
use crate::effects::punch::PunchConfig;
use crate::effects::reverb::ReverbConfig;
use crate::effects::warm::WarmConfig;
use crate::error::{check_range, ConfigProblem};
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
    serde(rename_all = "camelCase"),
    tsify(large_number_types_as_bigints)
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

/// Checks the output gain range.
///
/// `ProcessorConfig` and `ControlEvent::OutputGain` use the same range.
pub(crate) fn validate_output_gain(gain: f32) -> Result<(), Error> {
    check_range("output_gain", gain, 0.0, 64.0)
}
