//! Controls for a block, which the processor validates before any change.

use crate::config::validate_output_gain;
use crate::effects::alien::AlienConfig;
use crate::effects::punch::PunchConfig;
use crate::effects::reverb::ReverbConfig;
use crate::effects::warm::WarmConfig;
use crate::midi::MidiEvent;
use crate::{Error, EventProblem, ProcessorConfig, ResynthesisConfig};

/// A configuration or MIDI control that applies before a frame in the block.
///
/// Give controls to a process call as [`TimedEvent`] values. The call borrows
/// the slice only for that call. There is no heap allocation and no deferred
/// queue. A stage event replaces all settings of that stage. To change one
/// field, start from the current configuration of the processor.
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
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] for an invalid configuration or gain.
    /// Returns [`Error::InvalidMidi`] for invalid MIDI fields. Reset is always
    /// valid. The process call that contains the event checks the offset and
    /// the order.
    pub fn validate(self) -> Result<(), Error> {
        match self {
            Self::Configure(config) => config.validate(),
            Self::Resynthesis(config) => config.validate(),
            Self::Punch(config) => config.validate(),
            Self::Warm(config) => config.validate(),
            Self::Alien(config) => config.validate(),
            Self::Reverb(config) => config.validate(),
            Self::OutputGain(gain) => validate_output_gain(gain),
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
    /// Checks the offsets, the order and the payloads, in slice order.
    ///
    /// If an event fails, returns an error with the zero-based index of the
    /// first invalid event. The offset bounds have priority over the order.
    /// The order has priority over the payload validation. The function gives
    /// no tokens unless the full slice passes.
    pub(crate) fn new(events: &'a [TimedEvent], frames: usize) -> Result<Self, Error> {
        let mut previous = 0;
        for (index, event) in events.iter().enumerate() {
            let problem = if event.offset >= frames {
                Some(EventProblem::OffsetOutsideBlock)
            } else if event.offset < previous {
                Some(EventProblem::OffsetDecreasing)
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
