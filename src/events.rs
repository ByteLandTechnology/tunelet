//! Controls for a block, which the processor validates before any change.

use crate::config::validate_output_gain;
use crate::effects::alien::AlienConfig;
use crate::effects::punch::{self, PunchConfig};
use crate::effects::reverb::{self, ReverbConfig};
use crate::effects::warm::{self, WarmConfig};
use crate::formant;
use crate::harmony;
use crate::midi::MidiEvent;
use crate::pitch;
use crate::resynthesis;
use crate::{Error, EventProblem, ProcessorConfig, ResynthesisConfig};

/// One scalar configuration parameter for [`ControlEvent::Parameter`].
///
/// Each variant updates one scalar field in [`ProcessorConfig`] and reports
/// the same dotted field name and inclusive range as its stage structure. All
/// `f32` values must be finite.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase", tag = "type", content = "value")
)]
pub enum ParameterControl {
    /// Sets [`PunchConfig::gain_db`] (`punch.gain_db`), -96..=12 dB.
    PunchGainDb(f32),
    /// Sets [`PunchConfig::impact`] (`punch.impact`), 0..=1.
    PunchImpact(f32),
    /// Sets [`PunchConfig::ceiling_db`] (`punch.ceiling_db`), -96..=0 dBFS.
    PunchCeilingDb(f32),
    /// Sets [`PunchConfig::attack_ms`] (`punch.attack_ms`), 0.01..=5000 ms.
    PunchAttackMs(f32),
    /// Sets [`WarmConfig::drive_db`] (`warm.drive_db`), 0..=6 dB.
    WarmDriveDb(f32),
    /// Sets [`crate::PitchConfig::threshold`] (`resynthesis.pitch.threshold`),
    /// 0.002..=0.2.
    PitchThreshold(f32),
    /// Sets [`crate::CorrectionConfig::retune_ms`]
    /// (`resynthesis.correction.retune_ms`), 0..=400 ms.
    RetuneMs(f32),
    /// Sets [`crate::CorrectionConfig::humanize`]
    /// (`resynthesis.correction.humanize`), 0..=1.
    Humanize(f32),
    /// Sets [`crate::CorrectionConfig::flex_tune`]
    /// (`resynthesis.correction.flex_tune`), 0..=1.
    FlexTune(f32),
    /// Sets [`ResynthesisConfig::pitch_ratio`] (`resynthesis.pitch_ratio`),
    /// 0.25..=4.
    PitchRatio(f32),
    /// Sets [`crate::FormantConfig::width`] (`resynthesis.formant.width`),
    /// 0.5..=2.
    FormantWidth(f32),
    /// Sets [`crate::FormantConfig::stretch`] (`resynthesis.formant.stretch`),
    /// 0.5..=2.
    FormantStretch(f32),
    /// Sets [`crate::FormantConfig::aspiration_mix`]
    /// (`resynthesis.formant.aspiration_mix`), 0..=1.
    FormantAspirationMix(f32),
    /// Sets [`crate::FormantConfig::aspiration_cutoff_hz`]
    /// (`resynthesis.formant.aspiration_cutoff_hz`), 500..=10000 Hz.
    FormantAspirationCutoffHz(f32),
    /// Sets [`ResynthesisConfig::choir_voices`] (`resynthesis.choir_voices`),
    /// 1..=16.
    ChoirVoices(u8),
    /// Sets [`ResynthesisConfig::choir_detune`] (`resynthesis.choir_detune`),
    /// 10..=100.
    ChoirDetune(f32),
    /// Sets [`ResynthesisConfig::choir_delay`] (`resynthesis.choir_delay`),
    /// 20..=200.
    ChoirDelay(f32),
    /// Sets [`ResynthesisConfig::choir_spread_percent`]
    /// (`resynthesis.choir_spread_percent`), 0..=100%.
    ChoirSpreadPercent(f32),
    /// Sets [`crate::HarmonyConfig::register_percent`]
    /// (`resynthesis.harmony.register_percent`), 0..=100%.
    HarmonyRegisterPercent(f32),
    /// Sets [`crate::HarmonyConfig::voicing_spread_percent`]
    /// (`resynthesis.harmony.voicing_spread_percent`), 0..=100%.
    HarmonyVoicingSpreadPercent(f32),
    /// Sets [`crate::HarmonyConfig::portamento_ms`]
    /// (`resynthesis.harmony.portamento_ms`), 0..=10000 ms.
    HarmonyPortamentoMs(f32),
    /// Sets [`crate::HarmonyConfig::voices`] (`resynthesis.harmony.voices`),
    /// 0..=4.
    HarmonyVoices(u8),
    /// Sets [`ResynthesisConfig::harmony_gain`] (`resynthesis.harmony_gain`),
    /// 0..=1.
    HarmonyGain(f32),
    /// Sets [`ResynthesisConfig::harmonic_modulation`]
    /// (`resynthesis.harmonic_modulation`), 0..=24.
    HarmonicModulation(f32),
    /// Sets [`ResynthesisConfig::dry_mix`] (`resynthesis.dry_mix`), 0..=1.
    DryMix(f32),
    /// Sets [`ReverbConfig::mix`] (`reverb.mix`), 0..=1.
    ReverbMix(f32),
    /// Sets [`ReverbConfig::diffusion`] (`reverb.diffusion`), 0..=1.
    ReverbDiffusion(f32),
    /// Sets [`ReverbConfig::decay`] (`reverb.decay`), 0..=1.
    ReverbDecay(f32),
    /// Sets [`ReverbConfig::brightness_hz`] (`reverb.brightness_hz`),
    /// 80..=20000 Hz.
    ReverbBrightnessHz(f32),
}

impl ParameterControl {
    /// Checks the finiteness and the inclusive range of the scalar parameter,
    /// and does not apply it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] with the dotted field path and the
    /// range or finiteness problem of the related stage field.
    pub fn validate(self) -> Result<(), Error> {
        match self {
            Self::PunchGainDb(value) => punch::validate_gain_db(value),
            Self::PunchImpact(value) => punch::validate_impact(value),
            Self::PunchCeilingDb(value) => punch::validate_ceiling_db(value),
            Self::PunchAttackMs(value) => punch::validate_attack_ms(value),
            Self::WarmDriveDb(value) => warm::validate_drive_db(value),
            Self::PitchThreshold(value) => pitch::validate_threshold(value),
            Self::RetuneMs(value) => resynthesis::validate_retune_ms(value),
            Self::Humanize(value) => resynthesis::validate_humanize(value),
            Self::FlexTune(value) => resynthesis::validate_flex_tune(value),
            Self::PitchRatio(value) => resynthesis::validate_pitch_ratio(value),
            Self::FormantWidth(value) => formant::validate_width(value),
            Self::FormantStretch(value) => formant::validate_stretch(value),
            Self::FormantAspirationMix(value) => formant::validate_aspiration_mix(value),
            Self::FormantAspirationCutoffHz(value) => formant::validate_aspiration_cutoff_hz(value),
            Self::ChoirVoices(value) => resynthesis::validate_choir_voices(value),
            Self::ChoirDetune(value) => resynthesis::validate_choir_detune(value),
            Self::ChoirDelay(value) => resynthesis::validate_choir_delay(value),
            Self::ChoirSpreadPercent(value) => resynthesis::validate_choir_spread_percent(value),
            Self::HarmonyRegisterPercent(value) => harmony::validate_register_percent(value),
            Self::HarmonyVoicingSpreadPercent(value) => {
                harmony::validate_voicing_spread_percent(value)
            }
            Self::HarmonyPortamentoMs(value) => harmony::validate_portamento_ms(value),
            Self::HarmonyVoices(value) => harmony::validate_voices(value),
            Self::HarmonyGain(value) => resynthesis::validate_harmony_gain(value),
            Self::HarmonicModulation(value) => resynthesis::validate_harmonic_modulation(value),
            Self::DryMix(value) => resynthesis::validate_dry_mix(value),
            Self::ReverbMix(value) => reverb::validate_mix(value),
            Self::ReverbDiffusion(value) => reverb::validate_diffusion(value),
            Self::ReverbDecay(value) => reverb::validate_decay(value),
            Self::ReverbBrightnessHz(value) => reverb::validate_brightness_hz(value),
        }
    }

    /// Validates the scalar parameter and writes it into `config`.
    pub(crate) fn apply_to(self, config: &mut ProcessorConfig) -> Result<(), Error> {
        self.validate()?;
        match self {
            Self::PunchGainDb(value) => config.punch.gain_db = value,
            Self::PunchImpact(value) => config.punch.impact = value,
            Self::PunchCeilingDb(value) => config.punch.ceiling_db = value,
            Self::PunchAttackMs(value) => config.punch.attack_ms = value,
            Self::WarmDriveDb(value) => config.warm.drive_db = value,
            Self::PitchThreshold(value) => config.resynthesis.pitch.threshold = value,
            Self::RetuneMs(value) => config.resynthesis.correction.retune_ms = value,
            Self::Humanize(value) => config.resynthesis.correction.humanize = value,
            Self::FlexTune(value) => config.resynthesis.correction.flex_tune = value,
            Self::PitchRatio(value) => config.resynthesis.pitch_ratio = value,
            Self::FormantWidth(value) => config.resynthesis.formant.width = value,
            Self::FormantStretch(value) => config.resynthesis.formant.stretch = value,
            Self::FormantAspirationMix(value) => config.resynthesis.formant.aspiration_mix = value,
            Self::FormantAspirationCutoffHz(value) => {
                config.resynthesis.formant.aspiration_cutoff_hz = value;
            }
            Self::ChoirVoices(value) => config.resynthesis.choir_voices = value,
            Self::ChoirDetune(value) => config.resynthesis.choir_detune = value,
            Self::ChoirDelay(value) => config.resynthesis.choir_delay = value,
            Self::ChoirSpreadPercent(value) => config.resynthesis.choir_spread_percent = value,
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
            Self::ReverbMix(value) => config.reverb.mix = value,
            Self::ReverbDiffusion(value) => config.reverb.diffusion = value,
            Self::ReverbDecay(value) => config.reverb.decay = value,
            Self::ReverbBrightnessHz(value) => config.reverb.brightness_hz = value,
        }
        Ok(())
    }
}

/// A configuration or MIDI control that applies before a frame in the block.
///
/// Give controls to a process call as [`TimedEvent`] values. The call borrows
/// the slice only for that call. There is no heap allocation and no deferred
/// queue. Use [`Self::Parameter`] to change one scalar field. A stage event
/// replaces all settings of that stage. Configuration changes have the same
/// effects on state as [`crate::Processor::set_config`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
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
    /// This event updates one scalar parameter without a copy of the full stage
    /// structure.
    ///
    /// Validation, quantization, state retention and drain cancellation are
    /// identical to an equivalent stage event.
    Parameter(ParameterControl),
    /// This event sends a note control to the harmony source that
    /// [`crate::HarmonyConfig`] selects.
    ///
    /// MIDI-channel mode uses channels 0..=3. Omni mode and MIDI-chord mode
    /// combine channels.
    Midi(MidiEvent),
    /// This event changes the correction keyboard independently of harmony MIDI.
    ///
    /// While at least one key in 0..=127 is held, the held keys become the
    /// active pitch classes. Channels and nonzero velocities do not change the
    /// selected classes. When all keys release, or on All Notes Off, the
    /// configured mask loads again. If a later change sets new configured
    /// notes, or enables or disables correction, the configured mask also
    /// loads again.
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
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] for an invalid configuration, scalar
    /// parameter or gain. Returns [`Error::InvalidMidi`] for invalid MIDI
    /// fields. Reset is always valid. The process call that contains the event
    /// checks the offset and the order.
    pub fn validate(self) -> Result<(), Error> {
        match self {
            Self::Configure(config) => config.validate(),
            Self::Resynthesis(config) => config.validate(),
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

/// A MIDI message of three bytes that applies on a selected bus before the
/// zero-based input frame `offset`.
///
/// Give packed MIDI controls to [`crate::Processor::process_with_midi`],
/// [`crate::Processor::process_planar_with_midi`] or
/// [`crate::Processor::process_mono_in_place_with_midi`]. Offsets obey the same
/// bounds and order rules as [`TimedEvent`].
///
/// `bus` selects the destination: `0` ([`Self::MIDI_BUS`]) sends to
/// [`ControlEvent::Midi`], and `1` ([`Self::CORRECTION_MIDI_BUS`]) sends to
/// [`ControlEvent::CorrectionMidi`]. [`MidiEvent::from_bytes`] decodes `bytes`.
/// If `MidiEvent::from_bytes` returns `Ok(None)`, the processor ignores that
/// message after the offset checks succeed.
///
/// In a packed 32-bit message word, bits 24..=31 hold `bus` and bits 16..=23
/// hold `bytes[0]`. Bits 8..=15 hold `bytes[1]` and bits 0..=7 hold
/// `bytes[2]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackedMidiEvent {
    /// The zero-based frame offset in the block, `0..frames`.
    pub offset: usize,
    /// The target MIDI bus, `0..=1`: `0` for harmony MIDI or `1` for
    /// correction MIDI.
    pub bus: u8,
    /// The MIDI message of three bytes `[status, data1, data2]`, where
    /// `status` is in `0x80..=0xff` and `data1` and `data2` are in `0..=127`.
    pub bytes: [u8; 3],
}

impl PackedMidiEvent {
    /// The bus selector `0` for [`ControlEvent::Midi`].
    pub const MIDI_BUS: u8 = 0;

    /// The bus selector `1` for [`ControlEvent::CorrectionMidi`].
    pub const CORRECTION_MIDI_BUS: u8 = 1;

    /// Unpacks a frame `offset` and a 32-bit `packed_message` word into an
    /// event record.
    ///
    /// Bits 24..=31 set `bus` (`0..=1` for a valid bus). Bits 16..=23 set
    /// `bytes[0]` (`0x80..=0xff` for a valid status byte). Bits 8..=15 set
    /// `bytes[1]` and bits 0..=7 set `bytes[2]` (`0..=127` for valid data
    /// bytes). This constructor does not validate the unpacked fields.
    pub const fn from_packed(offset: usize, packed_message: u32) -> Self {
        let [bus, status, data1, data2] = packed_message.to_be_bytes();
        Self {
            offset,
            bus,
            bytes: [status, data1, data2],
        }
    }

    /// Packs `bus` and `bytes` into one 32-bit message word.
    ///
    /// Bits 24..=31 hold `bus`, bits 16..=23 hold `bytes[0]`, bits 8..=15 hold
    /// `bytes[1]` and bits 0..=7 hold `bytes[2]`.
    pub const fn packed_message(self) -> u32 {
        u32::from_be_bytes([self.bus, self.bytes[0], self.bytes[1], self.bytes[2]])
    }

    /// Checks `bus` and decodes `bytes` into an optional MIDI control event.
    pub(crate) fn decode_payload(self) -> Result<Option<ControlEvent>, EventProblem> {
        let is_correction = match self.bus {
            Self::MIDI_BUS => false,
            Self::CORRECTION_MIDI_BUS => true,
            bus => return Err(EventProblem::MidiBus { bus }),
        };
        let Some(midi) = MidiEvent::from_bytes(self.bytes).map_err(|_| EventProblem::Midi)? else {
            return Ok(None);
        };
        Ok(Some(if is_correction {
            ControlEvent::CorrectionMidi(midi)
        } else {
            ControlEvent::Midi(midi)
        }))
    }
}

/// A copyable packed MIDI entry for native slices and Wasm word pairs.
pub(crate) trait PackedMidiRecord: Copy {
    /// Converts this entry into a [`PackedMidiEvent`].
    fn into_event(self) -> PackedMidiEvent;
}

impl PackedMidiRecord for PackedMidiEvent {
    fn into_event(self) -> PackedMidiEvent {
        self
    }
}

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
impl PackedMidiRecord for [u32; 2] {
    fn into_event(self) -> PackedMidiEvent {
        PackedMidiEvent::from_packed(self[0] as usize, self[1])
    }
}

/// A packed MIDI slice that is fully validated before the processor uses an
/// entry.
pub(crate) struct ValidatedPackedMidi<'a, T> {
    /// The borrowed remainder of the checked slice, used in frame order.
    remaining: &'a [T],
}

impl<'a, T: PackedMidiRecord> ValidatedPackedMidi<'a, T> {
    /// Checks the offsets, the order, the bus and the MIDI bytes in slice
    /// order.
    pub(crate) fn new(events: &'a [T], frames: usize) -> Result<Self, Error> {
        let mut previous = 0;
        for (index, item) in events.iter().copied().enumerate() {
            let event = item.into_event();
            let problem = if event.offset >= frames {
                Some(EventProblem::OffsetOutsideBlock)
            } else if event.offset < previous {
                Some(EventProblem::OffsetDecreasing)
            } else {
                event.decode_payload().err()
            };
            if let Some(problem) = problem {
                return Err(Error::InvalidEvent { index, problem });
            }
            previous = event.offset;
        }
        Ok(Self { remaining: events })
    }

    /// Consumes the next supported MIDI event at `frame`, or returns `None` if
    /// no event is due.
    pub(crate) fn next_at(&mut self, frame: usize) -> Option<ValidatedEvent> {
        while let Some((first, remaining)) = self.remaining.split_first() {
            let event = first.into_event();
            if event.offset != frame {
                return None;
            }
            self.remaining = remaining;
            if let Ok(Some(control)) = event.decode_payload() {
                return Some(ValidatedEvent(control));
            }
        }
        None
    }
}

/// An event slice that is fully validated before the processor uses an entry.
pub(crate) struct ValidatedEvents<'a> {
    /// The borrowed remainder of the checked event slice, used in frame order.
    remaining: &'a [TimedEvent],
}

/// A proof that an event passed validation.
///
/// Only `ValidatedEvents` and `ValidatedPackedMidi` can make this proof.
pub(crate) struct ValidatedEvent(ControlEvent);

impl ValidatedEvent {
    /// Consumes the validation token and returns the copied control payload.
    pub(crate) fn into_inner(self) -> ControlEvent {
        self.0
    }
}

impl<'a> ValidatedEvents<'a> {
    /// Checks the offsets, the order and the payloads, in slice order.
    ///
    /// If an event fails, returns an error with the zero-based index of the
    /// first invalid event. The offset bounds have priority over the order.
    /// The order has priority over the payload validation. The function gives
    /// no tokens unless the full slice passes.
    pub(crate) fn new(
        events: &'a [TimedEvent],
        frames: usize,
        mut config: ProcessorConfig,
    ) -> Result<Self, Error> {
        let mut previous = 0;
        for (index, event) in events.iter().enumerate() {
            let problem = if event.offset >= frames {
                Some(EventProblem::OffsetOutsideBlock)
            } else if event.offset < previous {
                Some(EventProblem::OffsetDecreasing)
            } else {
                match validate_in_sequence(event.event, &mut config) {
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

/// Validates one event and advances the stack configuration copy.
fn validate_in_sequence(event: ControlEvent, config: &mut ProcessorConfig) -> Result<(), Error> {
    match event {
        ControlEvent::Configure(value) => {
            value.validate()?;
            *config = value;
            Ok(())
        }
        ControlEvent::Resynthesis(value) => {
            value.validate()?;
            config.resynthesis = value;
            Ok(())
        }
        ControlEvent::Punch(value) => {
            value.validate()?;
            config.punch = value;
            Ok(())
        }
        ControlEvent::Warm(value) => {
            value.validate()?;
            config.warm = value;
            Ok(())
        }
        ControlEvent::Alien(value) => {
            value.validate()?;
            config.alien = value;
            Ok(())
        }
        ControlEvent::Reverb(value) => {
            value.validate()?;
            config.reverb = value;
            Ok(())
        }
        ControlEvent::OutputGain(gain) => {
            validate_output_gain(gain)?;
            config.output_gain = gain;
            Ok(())
        }
        ControlEvent::Parameter(parameter) => parameter.apply_to(config),
        ControlEvent::Midi(midi) | ControlEvent::CorrectionMidi(midi) => midi.validate(),
        ControlEvent::Reset => Ok(()),
    }
}
