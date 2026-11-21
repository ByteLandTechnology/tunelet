//! Controls for a block, which the processor validates before any change.

use crate::config::validate_output_gain;
use crate::effects::alien::AlienConfig;
use crate::effects::punch::PunchConfig;
use crate::effects::reverb::{ReverbConfig, ReverbMode};
use crate::effects::warm::WarmConfig;
use crate::error::check_range;
use crate::formant::FormantConfig;
use crate::harmony::{
    ChordDegree, ChordName, HarmonyConfig, HarmonyScale, HarmonySource, Inversion, HARMONY_VOICES,
};
use crate::midi::MidiEvent;
use crate::pitch::PitchConfig;
use crate::resynthesis::{CorrectionConfig, MultiVoiceMode};
use crate::scale::{NoteMask, PitchClass};
use crate::{Error, EventProblem, ProcessorConfig, ResynthesisConfig};

/// A single parameter change inside [`ProcessorConfig`].
///
/// Use [`ControlEvent::Parameter`] to change one control at a frame offset and
/// keep all other active controls. Range and order checks match
/// [`ProcessorConfig::validate`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase", tag = "type", content = "value")
)]
pub enum ParameterControl {
    /// Enables or disables Punch (`punch.enabled`).
    PunchEnabled(bool),
    /// Sets the Punch input gain in decibels (`punch.gain_db`), -96..=12.
    PunchGainDb(f32),
    /// Sets the Punch transient emphasis (`punch.impact`), 0..=1.
    PunchImpact(f32),
    /// Sets the Punch output ceiling in dBFS (`punch.ceiling_db`), -96..=0.
    PunchCeilingDb(f32),
    /// Sets the Punch attack half-life in milliseconds (`punch.attack_ms`),
    /// 0.01..=5000.
    PunchAttackMs(f32),
    /// Enables or disables Warm (`warm.enabled`).
    WarmEnabled(bool),
    /// Sets the Warm drive in decibels (`warm.drive_db`), 0..=6.
    WarmDriveDb(f32),
    /// Sets the pitch acquisition threshold (`resynthesis.pitch.threshold`),
    /// 0.002..=0.2.
    PitchThreshold(f32),
    /// Sets the lower pitch report bound in hertz
    /// (`resynthesis.pitch.min_frequency_hz`), 40..=2000.
    ///
    /// Validation requires a value strictly below `max_frequency_hz`.
    PitchMinFrequencyHz(f32),
    /// Sets the upper pitch report bound in hertz
    /// (`resynthesis.pitch.max_frequency_hz`), 40..=2000.
    ///
    /// Validation requires a value strictly above `min_frequency_hz`.
    PitchMaxFrequencyHz(f32),
    /// Enables or disables pitch correction
    /// (`resynthesis.correction.enabled`).
    CorrectionEnabled(bool),
    /// Sets the correction pitch-class mask (`resynthesis.correction.notes`).
    CorrectionNotes(NoteMask),
    /// Sets the correction smoothing half-life in milliseconds
    /// (`resynthesis.correction.retune_ms`), 0..=400.
    CorrectionRetuneMs(f32),
    /// Sets the global pitch multiplier (`resynthesis.pitch_ratio`),
    /// 0.25..=4.
    PitchRatio(f32),
    /// Enables or disables formant analysis and synthesis
    /// (`resynthesis.formant.enabled`).
    FormantEnabled(bool),
    /// Sets the vocal-tract width factor (`resynthesis.formant.width`),
    /// 0.5..=2.
    FormantWidth(f32),
    /// Sets the vocal-tract length factor (`resynthesis.formant.stretch`),
    /// 0.5..=2.
    FormantStretch(f32),
    /// Sets the aspiration level (`resynthesis.formant.aspiration_mix`),
    /// 0..=1.
    FormantAspirationMix(f32),
    /// Sets the aspiration high-pass cutoff in hertz
    /// (`resynthesis.formant.aspiration_cutoff_hz`), 500..=10000.
    FormantAspirationCutoffHz(f32),
    /// Sets the voice arrangement (`resynthesis.mode`).
    ResynthesisMode(MultiVoiceMode),
    /// Sets the total choir voice count (`resynthesis.choir_voices`), 1..=16.
    ChoirVoices(u8),
    /// Sets the secondary choir drift control (`resynthesis.choir_detune`),
    /// 10..=100.
    ChoirDetune(f32),
    /// Sets the choir delay scale (`resynthesis.choir_delay`), 20..=200.
    ChoirDelay(f32),
    /// Sets the choir stereo width percentage
    /// (`resynthesis.choir_spread_percent`), 0..=100.
    ChoirSpreadPercent(f32),
    /// Enables or disables a stereo pan for each voice
    /// (`resynthesis.stereo_output`).
    StereoOutput(bool),
    /// Sets the harmony pitch source (`resynthesis.harmony.source`).
    HarmonySource(HarmonySource),
    /// Sets the harmony tonic key (`resynthesis.harmony.key`).
    HarmonyKey(PitchClass),
    /// Sets the harmony scale (`resynthesis.harmony.scale`).
    HarmonyScale(HarmonyScale),
    /// Sets the four chromatic semitone offsets
    /// (`resynthesis.harmony.chromatic_intervals`), each in -24..=24.
    HarmonyChromaticIntervals(
        #[cfg_attr(
            all(feature = "wasm", target_arch = "wasm32"),
            tsify(type = "[number, number, number, number]")
        )]
        [i8; HARMONY_VOICES],
    ),
    /// Sets the four diatonic scale-step offsets
    /// (`resynthesis.harmony.diatonic_intervals`), each in -14..=14.
    HarmonyDiatonicIntervals(
        #[cfg_attr(
            all(feature = "wasm", target_arch = "wasm32"),
            tsify(type = "[number, number, number, number]")
        )]
        [i8; HARMONY_VOICES],
    ),
    /// Sets the named harmony chord (`resynthesis.harmony.chord`).
    HarmonyChord(ChordName),
    /// Sets the scale-degree harmony chord (`resynthesis.harmony.degree`).
    HarmonyDegree(ChordDegree),
    /// Sets the harmony chord inversion (`resynthesis.harmony.inversion`).
    HarmonyInversion(Inversion),
    /// Sets the harmony chord register percentage
    /// (`resynthesis.harmony.register_percent`), 0..=100.
    HarmonyRegisterPercent(f32),
    /// Sets the harmony voicing spread percentage
    /// (`resynthesis.harmony.voicing_spread_percent`), 0..=100.
    HarmonyVoicingSpreadPercent(f32),
    /// Sets the harmony portamento half-life in milliseconds
    /// (`resynthesis.harmony.portamento_ms`), 0..=10000.
    HarmonyPortamentoMs(f32),
    /// Sets the interval harmony voice count (`resynthesis.harmony.voices`),
    /// 0..=4.
    HarmonyVoices(u8),
    /// Sets the linear gain of the four harmony voices
    /// (`resynthesis.harmony_gain`), 0..=1.
    HarmonyGain(f32),
    /// Sets the harmonic modulation depth (`resynthesis.harmonic_modulation`),
    /// 0..=24.
    HarmonicModulation(f32),
    /// Sets the resynthesis dry fraction (`resynthesis.dry_mix`), 0..=1.
    DryMix(f32),
    /// Sets the 32-bit random seed (`resynthesis.random_seed`).
    RandomSeed(u32),
    /// Enables or disables Alien (`alien.enabled`).
    AlienEnabled(bool),
    /// Sets the Alien loop length in frames (`alien.loop_frames`), 1..=96000.
    AlienLoopFrames(u32),
    /// Enables or disables reverb (`reverb.enabled`).
    ReverbEnabled(bool),
    /// Sets the reverb network (`reverb.mode`).
    ReverbMode(ReverbMode),
    /// Sets the reverb wet fraction (`reverb.mix`), 0..=1.
    ReverbMix(f32),
    /// Sets the reverb diffusion control (`reverb.diffusion`), 0..=1.
    ReverbDiffusion(f32),
    /// Sets the reverb feedback control (`reverb.decay`), 0..=1.
    ReverbDecay(f32),
    /// Sets the reverb low-pass cutoff in hertz (`reverb.brightness_hz`),
    /// 80..=20000.
    ReverbBrightnessHz(f32),
    /// Sets the final linear gain (`output_gain`), 0..=64.
    OutputGain(f32),
}

impl ParameterControl {
    /// Writes this parameter value into `config`.
    pub(crate) fn apply(self, config: &mut ProcessorConfig) {
        match self {
            Self::PunchEnabled(value) => config.punch.enabled = value,
            Self::PunchGainDb(value) => config.punch.gain_db = value,
            Self::PunchImpact(value) => config.punch.impact = value,
            Self::PunchCeilingDb(value) => config.punch.ceiling_db = value,
            Self::PunchAttackMs(value) => config.punch.attack_ms = value,
            Self::WarmEnabled(value) => config.warm.enabled = value,
            Self::WarmDriveDb(value) => config.warm.drive_db = value,
            Self::PitchThreshold(value) => config.resynthesis.pitch.threshold = value,
            Self::PitchMinFrequencyHz(value) => {
                config.resynthesis.pitch.min_frequency_hz = value;
            }
            Self::PitchMaxFrequencyHz(value) => {
                config.resynthesis.pitch.max_frequency_hz = value;
            }
            Self::CorrectionEnabled(value) => config.resynthesis.correction.enabled = value,
            Self::CorrectionNotes(value) => config.resynthesis.correction.notes = value,
            Self::CorrectionRetuneMs(value) => config.resynthesis.correction.retune_ms = value,
            Self::PitchRatio(value) => config.resynthesis.pitch_ratio = value,
            Self::FormantEnabled(value) => config.resynthesis.formant.enabled = value,
            Self::FormantWidth(value) => config.resynthesis.formant.width = value,
            Self::FormantStretch(value) => config.resynthesis.formant.stretch = value,
            Self::FormantAspirationMix(value) => {
                config.resynthesis.formant.aspiration_mix = value;
            }
            Self::FormantAspirationCutoffHz(value) => {
                config.resynthesis.formant.aspiration_cutoff_hz = value;
            }
            Self::ResynthesisMode(value) => config.resynthesis.mode = value,
            Self::ChoirVoices(value) => config.resynthesis.choir_voices = value,
            Self::ChoirDetune(value) => config.resynthesis.choir_detune = value,
            Self::ChoirDelay(value) => config.resynthesis.choir_delay = value,
            Self::ChoirSpreadPercent(value) => {
                config.resynthesis.choir_spread_percent = value;
            }
            Self::StereoOutput(value) => config.resynthesis.stereo_output = value,
            Self::HarmonySource(value) => config.resynthesis.harmony.source = value,
            Self::HarmonyKey(value) => config.resynthesis.harmony.key = value,
            Self::HarmonyScale(value) => config.resynthesis.harmony.scale = value,
            Self::HarmonyChromaticIntervals(value) => {
                config.resynthesis.harmony.chromatic_intervals = value;
            }
            Self::HarmonyDiatonicIntervals(value) => {
                config.resynthesis.harmony.diatonic_intervals = value;
            }
            Self::HarmonyChord(value) => config.resynthesis.harmony.chord = value,
            Self::HarmonyDegree(value) => config.resynthesis.harmony.degree = value,
            Self::HarmonyInversion(value) => config.resynthesis.harmony.inversion = value,
            Self::HarmonyRegisterPercent(value) => {
                config.resynthesis.harmony.register_percent = value;
            }
            Self::HarmonyVoicingSpreadPercent(value) => {
                config.resynthesis.harmony.voicing_spread_percent = value;
            }
            Self::HarmonyPortamentoMs(value) => {
                config.resynthesis.harmony.portamento_ms = value;
            }
            Self::HarmonyVoices(value) => config.resynthesis.harmony.voices = value,
            Self::HarmonyGain(value) => config.resynthesis.harmony_gain = value,
            Self::HarmonicModulation(value) => {
                config.resynthesis.harmonic_modulation = value;
            }
            Self::DryMix(value) => config.resynthesis.dry_mix = value,
            Self::RandomSeed(value) => config.resynthesis.random_seed = value,
            Self::AlienEnabled(value) => config.alien.enabled = value,
            Self::AlienLoopFrames(value) => config.alien.loop_frames = value,
            Self::ReverbEnabled(value) => config.reverb.enabled = value,
            Self::ReverbMode(value) => config.reverb.mode = value,
            Self::ReverbMix(value) => config.reverb.mix = value,
            Self::ReverbDiffusion(value) => config.reverb.diffusion = value,
            Self::ReverbDecay(value) => config.reverb.decay = value,
            Self::ReverbBrightnessHz(value) => config.reverb.brightness_hz = value,
            Self::OutputGain(value) => config.output_gain = value,
        }
    }

    /// Checks the range of this parameter without a processor context.
    ///
    /// A block preflight also checks frequency-bound order against the active
    /// configuration.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] if the value is not finite or is out
    /// of range.
    pub fn validate(self) -> Result<(), Error> {
        match self {
            Self::PitchMinFrequencyHz(value) => {
                check_range("resynthesis.pitch.min_frequency_hz", value, 40.0, 2000.0)
            }
            Self::PitchMaxFrequencyHz(value) => {
                check_range("resynthesis.pitch.max_frequency_hz", value, 40.0, 2000.0)
            }
            other => {
                let mut config = ProcessorConfig::default();
                other.apply(&mut config);
                config.validate()
            }
        }
    }
}

/// A configuration or MIDI control that applies before a frame in the block.
///
/// Give controls to a process call as [`TimedEvent`] values. The call borrows
/// the slice only for that call. There is no heap allocation and no deferred
/// queue. A stage or sub-stage event replaces all settings of that section.
/// [`Self::Parameter`] changes one field and keeps all other active settings.
/// Configuration changes have the same effects on state as
/// [`crate::Processor::set_config`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase", tag = "type", content = "value")
)]
pub enum ControlEvent {
    /// This event replaces the full configuration.
    Configure(ProcessorConfig),
    /// This event replaces the resynthesis configuration.
    Resynthesis(ResynthesisConfig),
    /// This event replaces the pitch configuration.
    Pitch(PitchConfig),
    /// This event replaces the pitch-correction configuration.
    Correction(CorrectionConfig),
    /// This event replaces the formant configuration.
    Formant(FormantConfig),
    /// This event replaces the harmony configuration.
    Harmony(HarmonyConfig),
    /// This event replaces the Punch configuration.
    Punch(PunchConfig),
    /// This event replaces the Warm configuration.
    Warm(WarmConfig),
    /// This event replaces the Alien configuration.
    Alien(AlienConfig),
    /// This event replaces the reverb configuration.
    Reverb(ReverbConfig),
    /// This event sets the final linear gain, 0..=64.
    ///
    /// The quantization and the mute behavior are the same as for
    /// [`ProcessorConfig::output_gain`]. The value must be finite.
    OutputGain(f32),
    /// This event changes one configuration parameter and keeps all other
    /// active settings.
    Parameter(ParameterControl),
    /// This event sends a note control to the harmony source that
    /// [`crate::HarmonyConfig`] selects.
    ///
    /// MIDI-channel mode uses channels 0..=3. Omni mode and MIDI-chord mode
    /// combine channels.
    Midi(MidiEvent),
    /// This event changes the correction keyboard independently of harmony MIDI.
    ///
    /// The held notes 0..=126 become pitch classes. Channels and nonzero
    /// velocities do not change the selected classes. Key 127 is stored, but
    /// the classes do not include it. Each event replaces the active
    /// correction table. If a later change sets new configured notes, or
    /// enables or disables correction, the configured mask loads again.
    CorrectionMidi(MidiEvent),
    /// This event resets immediately before the event frame, with the semantics of
    /// [`crate::Processor::reset`].
    ///
    /// Later events in the same block still run. The processor continues to
    /// render from this frame.
    Reset,
}

impl ControlEvent {
    /// Checks the configuration or MIDI ranges of the event, and does not
    /// apply the event.
    ///
    /// A process call also checks the offset, the slice order and cross-field
    /// configuration invariants against the active settings.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] for an invalid configuration,
    /// parameter or gain. Returns [`Error::InvalidMidi`] for invalid MIDI
    /// fields. Reset is always valid.
    pub fn validate(self) -> Result<(), Error> {
        match self {
            Self::Configure(config) => config.validate(),
            Self::Resynthesis(config) => config.validate(),
            Self::Pitch(config) => config.validate(),
            Self::Correction(config) => config.validate(),
            Self::Formant(config) => config.validate(),
            Self::Harmony(config) => config.validate(),
            Self::Punch(config) => config.validate(),
            Self::Warm(config) => config.validate(),
            Self::Alien(config) => config.validate(),
            Self::Reverb(config) => config.validate(),
            Self::OutputGain(gain) => validate_output_gain(gain),
            Self::Parameter(parameter) => parameter.validate(),
            Self::Midi(event) | Self::CorrectionMidi(event) => event.validate(),
            Self::Reset => Ok(()),
        }
    }

    /// Writes a configuration change into `config` and returns `true`, or
    /// returns `false` for MIDI and reset events.
    pub(crate) fn apply_to_config(self, config: &mut ProcessorConfig) -> bool {
        match self {
            Self::Configure(value) => *config = value,
            Self::Resynthesis(value) => config.resynthesis = value,
            Self::Pitch(value) => config.resynthesis.pitch = value,
            Self::Correction(value) => config.resynthesis.correction = value,
            Self::Formant(value) => config.resynthesis.formant = value,
            Self::Harmony(value) => config.resynthesis.harmony = value,
            Self::Punch(value) => config.punch = value,
            Self::Warm(value) => config.warm = value,
            Self::Alien(value) => config.alien = value,
            Self::Reverb(value) => config.reverb = value,
            Self::OutputGain(value) => config.output_gain = value,
            Self::Parameter(parameter) => parameter.apply(config),
            Self::Midi(_) | Self::CorrectionMidi(_) | Self::Reset => return false,
        }
        true
    }
}

/// A control that applies immediately before the zero-based input frame
/// `offset`.
///
/// Offsets must be less than the current block length. They must not decrease
/// in the event slice. Events with equal offsets execute in slice order. Thus,
/// a later change at the same frame sees the earlier changes. An empty block
/// accepts no events.
///
/// Offsets are local to this call, not a total count of processed frames.
/// This timing tells when the state changes. Smoothing, the analysis interval
/// and the signal latency also control when the audible result occurs.
///
/// The processor validates each event before it starts to process the block.
/// If one event is not valid, the processor rejects the full block. This
/// includes the valid events that are earlier in the slice.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub struct TimedEvent {
    /// The frame offset in the block.
    pub offset: usize,
    /// The control change.
    pub event: ControlEvent,
}

/// An event slice that is fully validated before the processor uses an entry.
pub(crate) struct ValidatedEvents<'a> {
    /// The borrowed remainder of the checked event slice, used in frame order.
    remaining: &'a [TimedEvent],
}

/// A proof that an event passed validation.
///
/// Only `ValidatedEvents` can make this proof.
pub(crate) struct ValidatedEvent(ControlEvent);

impl ValidatedEvent {
    /// Consumes the validation token and returns the copied control payload.
    pub(crate) fn into_inner(self) -> ControlEvent {
        self.0
    }
}

impl<'a> ValidatedEvents<'a> {
    /// Checks the offsets, the order and the payloads on a stack copy of
    /// `config`, in slice order.
    ///
    /// If an event fails, returns an error with the zero-based index of the
    /// first invalid event. The offset bounds have priority over the order.
    /// The order has priority over the payload validation. The function gives
    /// no tokens unless the full slice passes.
    pub(crate) fn new(
        mut config: ProcessorConfig,
        events: &'a [TimedEvent],
        frames: usize,
    ) -> Result<Self, Error> {
        let mut previous = 0;
        for (index, event) in events.iter().enumerate() {
            let problem = if event.offset >= frames {
                Some(EventProblem::OffsetOutsideBlock)
            } else if event.offset < previous {
                Some(EventProblem::OffsetDecreasing)
            } else if event.event.apply_to_config(&mut config) {
                match config.validate() {
                    Ok(()) => None,
                    Err(Error::InvalidConfig { field, problem }) => {
                        Some(EventProblem::Config { field, problem })
                    }
                    Err(_) => Some(EventProblem::Midi),
                }
            } else {
                match event.event.validate() {
                    Ok(()) => None,
                    Err(Error::InvalidConfig { field, problem }) => {
                        Some(EventProblem::Config { field, problem })
                    }
                    Err(_) => Some(EventProblem::Midi),
                }
            };
            if let Some(problem) = problem {
                return Err(Error::InvalidEvent { index, problem });
            }
            previous = event.offset;
        }
        Ok(Self { remaining: events })
    }

    /// Consumes one event at `frame`, or returns `None` if no event is due.
    ///
    /// The caller visits all frames in increasing order. At each frame, it
    /// calls this method again until it returns `None`. This keeps the slice
    /// order of equal offsets. If the caller skips the frame of an event, the
    /// cursor cannot move forward.
    pub(crate) fn next_at(&mut self, frame: usize) -> Option<ValidatedEvent> {
        let (event, remaining) = self.remaining.split_first()?;
        if event.offset != frame {
            return None;
        }
        self.remaining = remaining;
        Some(ValidatedEvent(event.event))
    }
}
