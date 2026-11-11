//! A crate for vocal pitch correction, choir, harmony, formant control and effects.
//!
//! [`Processor`] changes mono signed 16-bit PCM into the same number of stereo
//! or mono frames. The sample rate is 44.1 kHz or 48 kHz. The signal chain is
//! Punch → Warm → pitch tracking and resynthesis → Alien → reverb → output
//! gain. The default configuration enables chromatic pitch correction and one
//! choir voice. The optional effects and the formant stage are disabled.
//!
//! # Preparing and configuring
//!
//! [`PrepareConfig`] sets the sample rate. It also sets the maximum number of
//! frames for one process call or one drain call. To make a
//! [`ProcessorConfig`], start from its default value and change its public
//! stage fields. Then give it to [`Processor::new`]. All configuration ranges
//! are inclusive, unless the documentation tells you differently.
//! Floating-point controls must be finite.
//!
//! Validation also examines disabled stages and the controls of voice modes
//! that are not active.
//!
//! A configuration value is a copy. If you change it, the processor does not
//! change until [`Processor::set_config`] or a configuration event applies it.
//! You can change settings between blocks, or at a frame offset with a
//! [`TimedEvent`]. To change the sample rate or the block limit, you
//! must make a new processor.
//!
//! # Audio and timed controls
//!
//! There are three process methods:
//!
//! - [`Processor::process`] writes `[left, right]` frames.
//! - [`Processor::process_planar`] writes separate channels.
//! - [`Processor::process_mono_in_place`] replaces the input with the stereo
//!   downmix.
//!
//! These layouts do not depend on [`ResynthesisConfig::stereo_output`], which
//! controls voice panning. The output is clipped to -32767..=32767.
//!
//! An event applies before its selected frame. Offsets must be in the current
//! block and must not decrease. Events with equal offsets execute in slice
//! order. The processor validates the full block before it changes the output
//! or its state. Thus, an invalid last event also rejects the full call. An
//! empty block accepts no events.
//!
//! [`ControlEvent::Midi`] controls harmony. [`ControlEvent::CorrectionMidi`]
//! selects the correction pitch classes independently. The processor does not
//! keep MIDI controls in a queue after the current call.
//!
//! # Feedback, latency and lifecycle
//!
//! [`Processor::feedback`] gives the pitch, the correction ratio, the
//! accumulated stereo peak and the frame count.
//! [`Processor::take_feedback`] also clears the peak window. The pitch
//! reporting bounds change only the reports. They do not change the detection
//! or the audio signal. [`Processor::latency_frames`] gives a nominal
//! resynthesis delay. Voice delays, pitch changes and effects have their own
//! timing.
//!
//! [`Processor::telemetry`] gives data for meters and displays: the ratio and
//! the gain of each voice, the input and output peaks, the pitch clarity and
//! the state of the effects. [`Processor::take_telemetry`] also clears the
//! peak windows of the telemetry. [`Processor::formant_envelope`] writes the
//! LPC envelopes before and after the formant controls. These calls do not
//! change the audio.
//!
//! After the last input block, call [`Processor::begin_drain`] one time. Then
//! render drain blocks until a call returns zero. Each call fills the unused
//! output with zeros. The drain uses a finite configured budget and mutes
//! aspiration. It is not a silence detector.
//!
//! The call that uses all of the budget does the same reset as
//! [`Processor::reset`]. This occurs also if that call returns audio. A
//! successful process call, a successful configuration call or a reset
//! cancels the drain.
//!
//! [`Processor::reset`] keeps the configuration and some controller state, for
//! example the random stream. Its documentation lists the state that it keeps.
//! If you need a fully new instance, make a new processor.
//!
//! # Execution model
//!
//! Construction allocates the audio storage. After construction, the calls for
//! audio, controls, feedback, reset and drain do not allocate memory. They do
//! not acquire locks and do no I/O. Each processor owns its histories and its
//! random stream. Fixed-point arithmetic uses explicit wrapping and
//! quantization. If you change the order of the arithmetic, the samples can
//! change.
//!
//! The default build uses only safe Rust and has no dependencies.
//!
//! # WebAssembly
//!
//! To export JavaScript bindings and generated TypeScript definitions, enable
//! the optional `wasm` feature for `wasm32-unknown-unknown`. The binding owns
//! PCM buffers that it uses again. Configuration conversion, event conversion,
//! feedback objects and errors cause more allocation at the JavaScript
//! boundary.
//! Refer to the README for how to build the shared package for browsers and
//! Node.js.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]
#![warn(unreachable_pub)]

mod config;
mod effects;
mod error;
mod events;
mod filter;
mod formant;
mod harmony;
mod lpc;
mod midi;
mod numeric;
mod pitch;
mod processor;
mod resynthesis;
mod scale;

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub mod wasm;

pub use config::{PrepareConfig, ProcessorConfig, SampleRate};
pub use effects::alien::AlienConfig;
pub use effects::punch::PunchConfig;
pub use effects::reverb::{ReverbConfig, ReverbMode};
pub use effects::warm::WarmConfig;
pub use error::{ConfigProblem, Error, EventProblem};
pub use events::{ControlEvent, TimedEvent};
pub use formant::FormantConfig;
pub use harmony::{
    ChordDegree, ChordName, HarmonyConfig, HarmonyScale, HarmonySource, Inversion, HARMONY_VOICES,
};
pub use midi::MidiEvent;
pub use pitch::PitchConfig;
pub use processor::{Feedback, Processor, Telemetry};
pub use resynthesis::{CorrectionConfig, MultiVoiceMode, ResynthesisConfig};
pub use scale::{CorrectionScale, NoteMask, PitchClass};
