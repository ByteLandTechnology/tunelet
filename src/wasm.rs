//! The optional JavaScript bindings for `wasm32-unknown-unknown`.
//!
//! To make the JavaScript glue, the Wasm binary and the TypeScript
//! declarations, do these steps:
//!
//! 1. Build the crate for `wasm32-unknown-unknown` with the `wasm` feature.
//! 2. Run wasm-bindgen on the result.
//!
//! wasm-pack can do the two steps in one command. The npm entry point uses
//! `loadTunelet()` to initialize these bindings in browsers and in Node.js.
//!
//! The exported `Tunelet` class owns a native `Processor` and three reusable
//! PCM buffers. JavaScript gives the `Int16Array` input and output buffers. The
//! PCM API does not give buffer pointers or long-lived memory views to
//! JavaScript. The class copies the input before it writes the output.
//!
//! Configuration objects obey these rules:
//!
//! - Field names use camelCase.
//! - Enum values are strings.
//! - Each object must contain all required fields.
//! - The optional harmony `voices` field defaults to 4.
//!
//! `defaultConfig` gives a configuration to start from. Control events use
//! `{ type, value }`, and the reset control has no `value`. MIDI messages use
//! `{ type, channel, note, velocity }`, with the fields that apply to the
//! selected variant. The TypeScript declarations come from the same Rust types
//! that validation uses.
//!
//! The class allocates the PCM storage at construction and uses it again for
//! each block. These operations can allocate:
//!
//! - The decode of events.
//! - The conversion of configurations.
//! - Feedback and telemetry objects.
//! - The buffers of the formant envelopes.
//! - Errors.
//!
//! The JavaScript glue can also make temporary views when it copies data. This
//! generic binding does not guarantee an AudioWorklet callback that is free of
//! allocation. If you do not need a processor any more, call `free()`.

use crate::{
    BufferName, ConfigField, ConfigProblem, Error, EventProblem, Feedback, MidiProblem,
    PrepareConfig, Processor, ProcessorConfig, SampleRate, Telemetry, TimedEvent,
};
use js_sys::{Float32Array, Int16Array};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use tsify::{Ts, Tsify};
use wasm_bindgen::prelude::*;

/// The events for one call that processes audio (`TimedEvent[]` in TypeScript).
///
/// Each offset is a zero-based frame index in the current block. Each offset
/// must be in the block and must not be less than the offset before it. Events
/// with equal offsets execute in array order. An empty block accepts no
/// events. If there are no events, omit the optional argument. Then the
/// binding does not decode an array.
#[derive(Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BlockEvents(Vec<TimedEvent>);

const _: () = {
    #[wasm_bindgen]
    extern "C" {
        #[derive(Clone)]
        #[wasm_bindgen(typescript_type = "TimedEvent[]")]
        pub type JsTimedEvents;
    }

    impl Tsify for BlockEvents {
        type JsType = JsTimedEvents;
        const DECL: &'static str = "";
    }
};

/// The specific kind of configuration failure in `ErrorDetails`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub enum ConfigProblemKind {
    /// The floating-point value is outside `min..=max`.
    OutOfRange,
    /// The integer value is outside `min..=max`.
    IntegerOutOfRange,
    /// The number is finite, but it has a fractional part where an integer is required.
    NotInteger,
    /// The number is `NaN` or infinite.
    NotFinite,
    /// The value is not strictly less than `other`.
    Unordered,
}

/// The specific kind of timed-event failure in `ErrorDetails`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub enum EventProblemKind {
    /// The zero-based frame offset is outside `0..frames`.
    OffsetOutsideBlock,
    /// The zero-based frame offset is less than `previous`.
    OffsetDecreasing,
    /// The configuration payload of the event is not valid.
    Config,
    /// The MIDI payload of the event is not valid.
    Midi,
}

/// The specific kind of MIDI failure in `ErrorDetails`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub enum MidiProblemKind {
    /// The status byte at index `0` is below `0x80`.
    StatusByte,
    /// A data byte at `byteIndex` is more than `127`.
    DataByte,
    /// The zero-based MIDI channel is more than `15`.
    Channel,
    /// The MIDI note number is more than `127`.
    Note,
    /// The MIDI velocity is more than `127`.
    Velocity,
}

/// The public TypeScript input type that rejected a malformed JavaScript value.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Tsify)]
pub enum InputTarget {
    /// A `ProcessorConfig` argument.
    #[serde(rename = "ProcessorConfig")]
    ProcessorConfig,
    /// A `TimedEvent[]` argument.
    #[serde(rename = "TimedEvent[]")]
    TimedEvents,
}

/// Machine-readable diagnostic fields on `TuneletError.details`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum ErrorDetails {
    /// The constructor `sampleRate` is not `44100` or `48000`.
    UnsupportedSampleRate {
        /// The rejected sample rate in hertz.
        hz: f64,
    },
    /// A frequency argument is not finite or is outside `0 < hz <= maxHz`.
    InvalidFrequency {
        /// The rejected frequency in hertz.
        hz: f64,
        /// The inclusive upper limit, half of `sampleRate` in hertz.
        #[serde(rename = "maxHz")]
        max_hz: f64,
    },
    /// A constructor or `ProcessorConfig` control is not valid.
    InvalidConfig {
        /// The dotted `camelCase` path of the rejected field.
        field: ConfigField,
        /// The zero-based voice index in `0..=3` for a harmony interval array.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        voice: Option<usize>,
        /// The kind of validation failure.
        problem: ConfigProblemKind,
        /// The rejected numeric value.
        value: f64,
        /// The inclusive lower bound, when `problem` checks bounds.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        min: Option<f64>,
        /// The inclusive upper bound, when `problem` checks bounds.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        max: Option<f64>,
        /// The related field whose value `value` must stay below.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        other: Option<ConfigField>,
        /// The current value of `other`.
        #[serde(rename = "otherValue", skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        other_value: Option<f64>,
    },
    /// An output buffer length is not equal to the input or the other channel.
    BufferLength {
        /// The output buffer whose length is not valid.
        buffer: BufferName,
        /// The required frame or point count.
        expected: usize,
        /// The actual length of `buffer`.
        actual: usize,
    },
    /// A block has more frames than `maxBlockFrames`.
    BlockTooLarge {
        /// The frame count of the rejected block.
        frames: usize,
        /// The prepared `maxBlockFrames` limit.
        max: usize,
    },
    /// A timed event in `TimedEvent[]` failed the block preflight check.
    InvalidEvent {
        /// The zero-based index of the rejected event in `events`.
        index: usize,
        /// The zero-based frame offset of the rejected event.
        offset: usize,
        /// The kind of event failure.
        problem: EventProblemKind,
        /// The frame count of the current block, for `offsetOutsideBlock`.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        frames: Option<usize>,
        /// The frame offset of the previous event, for `offsetDecreasing`.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        previous: Option<usize>,
        /// The rejected `camelCase` field path, for `config` failures.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        field: Option<ConfigField>,
        /// The zero-based voice index in `0..=3`, for harmony array failures.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        voice: Option<usize>,
        /// The configuration failure kind, for `config` failures.
        #[serde(rename = "configProblem", skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        config_problem: Option<ConfigProblemKind>,
        /// The MIDI failure kind, for `midi` failures.
        #[serde(rename = "midiProblem", skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        midi_problem: Option<MidiProblemKind>,
        /// The byte index in a three-byte MIDI message, when applicable.
        #[serde(rename = "byteIndex", skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        byte_index: Option<usize>,
        /// The rejected configuration or MIDI value, when applicable.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        value: Option<f64>,
        /// The inclusive lower bound of the rejected field or MIDI value.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        min: Option<f64>,
        /// The inclusive upper bound of the rejected field or MIDI value.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        max: Option<f64>,
        /// The related field for an `unordered` configuration failure.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        other: Option<ConfigField>,
        /// The value of `other` for an `unordered` configuration failure.
        #[serde(rename = "otherValue", skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        other_value: Option<f64>,
    },
    /// A MIDI message or `MidiEvent` field is out of range.
    InvalidMidi {
        /// The kind of MIDI failure.
        problem: MidiProblemKind,
        /// The byte index (`0`, `1` or `2`) for raw three-byte MIDI messages.
        #[serde(rename = "byteIndex", skip_serializing_if = "Option::is_none")]
        #[tsify(optional)]
        byte_index: Option<usize>,
        /// The rejected byte, channel, note or velocity value.
        value: u8,
        /// The inclusive lower bound for the byte or field.
        min: u8,
        /// The inclusive upper bound for the byte or field.
        max: u8,
    },
    /// A JavaScript argument failed conversion into a public SDK type.
    InvalidInput {
        /// The target TypeScript type that rejected the input.
        target: InputTarget,
    },
    /// WebAssembly failed to reserve a required internal buffer.
    AllocationFailed,
    /// WebAssembly failed to convert a configuration or snapshot object.
    SerializationFailed,
    /// A call ran before `loadTunelet()` or `initSync()` initialized WebAssembly.
    NotInitialized,
    /// A method or getter ran on a `Tunelet` instance after `free()`.
    Disposed,
    /// An initialization call gave a custom Wasm source that did not match.
    SourceMismatch,
}

/// Returns a complete, independent default configuration for JavaScript.
///
/// Change its fields and give it to `Tunelet` or to `setConfig`. The
/// `randomSeed` field is a JavaScript `bigint`, so it keeps all 64 bits. After
/// the binding decodes them, floating-point controls have the precision of the
/// Rust `f32` type.
///
/// # Errors
///
/// If the conversion to a JavaScript object fails, this function throws a
/// structured JavaScript `Error`.
#[wasm_bindgen(js_name = defaultConfig)]
pub fn default_config() -> Result<Ts<ProcessorConfig>, JsValue> {
    encode_ts(&ProcessorConfig::default())
}

/// A stateful PCM processor that JavaScript uses as the `Tunelet` class.
///
/// The input is signed 16-bit mono PCM. The output is planar stereo or an
/// in-place mono downmix. The quantization, latency and configuration behavior
/// are the same as in the native processor. Buffer lengths count frames, not
/// bytes.
///
/// These rules apply to the buffers:
///
/// - Separate stereo output arrays must not overlap.
/// - The input can overlap an output, because the class copies the input
///   first.
/// - All buffers must stay attached until each call is complete.
///
/// If a call that processes audio fails, it throws a JavaScript `Error` before
/// the audio state or the output changes. The class rejects malformed
/// configuration objects and malformed event objects. It also rejects values
/// that are out of range. The class decodes objects inside the methods that
/// can fail. Thus, if a conversion fails, Rust releases the temporary values
/// as usual.
#[wasm_bindgen(js_name = Tunelet)]
pub struct WasmProcessor {
    /// The DSP state, with its preparation limits, which do not change.
    processor: Processor,
    /// The reusable PCM buffer for input or in-place mono audio, with a length
    /// equal to the prepared block limit.
    input: Vec<i16>,
    /// The reusable PCM buffer for the left channel, which the class copies to
    /// the caller only after a successful call.
    left: Vec<i16>,
    /// The reusable PCM buffer for the right channel, which the class copies to
    /// the caller only after a successful call.
    right: Vec<i16>,
    /// The reusable buffers for the two formant envelopes. They become longer
    /// when a call needs more points, and they do not become shorter.
    envelope_input: Vec<f32>,
    envelope_output: Vec<f32>,
}

#[wasm_bindgen(js_class = Tunelet)]
impl WasmProcessor {
    /// Prepares a processor and its reusable PCM buffers.
    ///
    /// `sampleRate` must be 44100 or 48000. `maxBlockFrames` must be an
    /// integer in 1..=4294967295. It is a limit for each call, and smaller
    /// blocks and empty blocks are also valid. If you omit `config`, the
    /// processor uses the default configuration. The constructor validates
    /// the settings before it allocates the processor or the PCM buffers. In
    /// TypeScript, use `new Tunelet(sampleRate, maxBlockFrames, config)`.
    ///
    /// # Errors
    ///
    /// The constructor throws if one of these conditions occurs:
    ///
    /// - The rate is not 44100 or 48000.
    /// - The block limit is not an integer or is out of range.
    /// - The configuration object is malformed or a field is absent.
    /// - The configuration is not valid.
    /// - The reservation of a PCM buffer fails.
    ///
    /// The available Wasm memory must also have sufficient space for the DSP
    /// histories.
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(js_name = sampleRate)] sample_rate: f64,
        #[wasm_bindgen(js_name = maxBlockFrames)] max_block_frames: f64,
        config: Option<Ts<ProcessorConfig>>,
    ) -> Result<WasmProcessor, JsValue> {
        // Accept JavaScript numbers without the u32 coercion of wasm-bindgen.
        // That coercion silently truncates fractions and wraps large values.
        let rate = match sample_rate {
            44100.0 => SampleRate::Hz44100,
            48000.0 => SampleRate::Hz48000,
            _ => {
                return Err(js_error(
                    &format!("sampleRate ({sample_rate} Hz) must be 44100 or 48000"),
                    &ErrorDetails::UnsupportedSampleRate { hz: sample_rate },
                ));
            }
        };
        let max_block_frames = validate_max_block_frames(max_block_frames)?;
        let preparation = PrepareConfig::new(rate, max_block_frames).map_err(domain_error)?;
        let config = match config {
            Some(value) => decode_config(&value)?,
            None => ProcessorConfig::default(),
        };
        let processor = Processor::new(preparation, config).map_err(domain_error)?;
        Ok(Self {
            processor,
            input: pcm_buffer(max_block_frames)?,
            left: pcm_buffer(max_block_frames)?,
            right: pcm_buffer(max_block_frames)?,
            envelope_input: Vec::new(),
            envelope_output: Vec::new(),
        })
    }

    /// The sample rate of the input and the output in hertz.
    ///
    /// The constructor sets this value, and the value does not change.
    #[wasm_bindgen(getter, js_name = sampleRate)]
    pub fn sample_rate(&self) -> u32 {
        self.processor.prepare_config().sample_rate.as_u32()
    }

    /// The maximum frame count for each call that processes or drains audio.
    ///
    /// The constructor sets this value, and the value does not change.
    #[wasm_bindgen(getter, js_name = maxBlockFrames)]
    pub fn max_block_frames(&self) -> usize {
        self.input.len()
    }

    /// The nominal resynthesis latency: 1,593 frames at both sample rates.
    ///
    /// The dry path has a delay of 1,543 frames. Resampled voices, choir delays
    /// and effects have their own timing. Thus, this value does not align all
    /// paths.
    #[wasm_bindgen(getter, js_name = latencyFrames)]
    pub fn latency_frames(&self) -> usize {
        self.processor.latency_frames()
    }

    /// Returns a copy of the active configuration as a new JavaScript object.
    ///
    /// A change to the object has no effect until `setConfig` or a timed event
    /// applies it.
    ///
    /// # Errors
    ///
    /// If the conversion to a JavaScript object fails, this method throws.
    pub fn config(&self) -> Result<Ts<ProcessorConfig>, JsValue> {
        encode_ts(&self.processor.config())
    }

    /// Decodes and validates a configuration, then replaces the complete
    /// configuration with it.
    ///
    /// State retention and drain cancellation follow the native processor.
    /// The conversion ignores additional properties of the JavaScript object.
    ///
    /// # Errors
    ///
    /// The method throws if an object is not valid or a field is absent.
    /// After an error, all processor state and any active drain stay the same.
    #[wasm_bindgen(js_name = setConfig)]
    pub fn set_config(&mut self, config: Ts<ProcessorConfig>) -> Result<(), JsValue> {
        let decoded = decode_config(&config)?;
        self.processor.set_config(decoded).map_err(domain_error)
    }

    /// Processes mono input into left and right `Int16Array` buffers of equal
    /// length.
    ///
    /// If the call is successful, it writes every output sample. Optional
    /// events apply before their frame, and their offsets obey the rules of
    /// `TimedEvent[]`. An empty block must have no events. A successful call
    /// cancels a drain, also for an empty block. The call uses the existing
    /// JavaScript and Rust PCM buffers again.
    ///
    /// # Errors
    ///
    /// The method throws if the channel lengths are not equal. It also throws
    /// if the block is larger than `maxBlockFrames` or an event is not valid.
    /// The call checks the lengths and the block size before it decodes and
    /// validates the events. If an error occurs, the output buffers and the DSP
    /// state do not change.
    pub fn process(
        &mut self,
        input: &Int16Array,
        left: &Int16Array,
        right: &Int16Array,
        events: Option<Ts<BlockEvents>>,
    ) -> Result<(), JsValue> {
        let frames = input.length() as usize;
        Error::check_length(BufferName::Left, frames, left.length() as usize)
            .map_err(domain_error)?;
        Error::check_length(BufferName::Right, frames, right.length() as usize)
            .map_err(domain_error)?;
        self.processor.check_block(frames).map_err(domain_error)?;
        let events = decode_events(events)?;
        input.copy_to(&mut self.input[..frames]);
        self.processor
            .process_planar(
                &self.input[..frames],
                &mut self.left[..frames],
                &mut self.right[..frames],
                &events.0,
            )
            .map_err(domain_error)?;
        left.copy_from(&self.left[..frames]);
        right.copy_from(&self.right[..frames]);
        Ok(())
    }

    /// Replaces the audio in a mono `Int16Array` with the stereo downmix.
    ///
    /// The timing, the events and the drain cancellation are the same as for
    /// `process`. Peak feedback measures the stereo output before the downmix,
    /// as in the native processor.
    ///
    /// # Errors
    ///
    /// The method throws if the block is too large or an event is not valid.
    /// It throws before the audio of the caller or the DSP state changes.
    #[wasm_bindgen(js_name = processMono)]
    pub fn process_mono(
        &mut self,
        audio: &Int16Array,
        events: Option<Ts<BlockEvents>>,
    ) -> Result<(), JsValue> {
        let frames = audio.length() as usize;
        self.processor.check_block(frames).map_err(domain_error)?;
        let events = decode_events(events)?;
        audio.copy_to(&mut self.input[..frames]);
        self.processor
            .process_mono_in_place(&mut self.input[..frames], &events.0)
            .map_err(domain_error)?;
        audio.copy_from(&self.input[..frames]);
        Ok(())
    }

    /// Starts a finite tail budget and returns its frame count as a number.
    ///
    /// Call this method one time after the final input block. Another call
    /// starts the full budget again, but it does not clear the histories. Refer
    /// to `drain` for completion.
    #[wasm_bindgen(js_name = beginDrain)]
    pub fn begin_drain(&mut self) -> usize {
        self.processor.begin_drain()
    }

    /// Returns the tail budget of the current configuration, not the remaining
    /// drain count.
    #[wasm_bindgen(js_name = tailFrames)]
    pub fn tail_frames(&self) -> usize {
        self.processor.tail_frames()
    }

    /// Renders stereo tail frames into `Int16Array` buffers of equal length.
    ///
    /// The method returns the number of rendered frames and fills the unused
    /// output with zeros. If the drain is not active or is complete, the method
    /// returns zero. Empty arrays do not use the budget. Completion resets the
    /// histories and the feedback in the final drain call, also when that call
    /// returns audio.
    ///
    /// # Errors
    ///
    /// The method throws if the channel lengths are not equal or if a block is
    /// larger than `maxBlockFrames`. After an error, both output arrays and the
    /// drain state do not change.
    pub fn drain(&mut self, left: &Int16Array, right: &Int16Array) -> Result<usize, JsValue> {
        let frames = left.length() as usize;
        Error::check_length(BufferName::Right, frames, right.length() as usize)
            .map_err(domain_error)?;
        self.processor.check_block(frames).map_err(domain_error)?;
        let count = self
            .processor
            .drain_planar(&mut self.left[..frames], &mut self.right[..frames])
            .map_err(domain_error)?;
        left.copy_from(&self.left[..frames]);
        right.copy_from(&self.right[..frames]);
        Ok(count)
    }

    /// Renders a mono tail block and returns its frame count.
    ///
    /// The method fills the unused output with zeros. The downmix and the peak
    /// feedback are the same as for `processMono`. The return values, empty
    /// arrays and completion are the same as for `drain`.
    ///
    /// # Errors
    ///
    /// If the block is larger than `maxBlockFrames`, this method throws before
    /// the output or the state changes.
    #[wasm_bindgen(js_name = drainMono)]
    pub fn drain_mono(&mut self, audio: &Int16Array) -> Result<usize, JsValue> {
        let frames = audio.length() as usize;
        self.processor.check_block(frames).map_err(domain_error)?;
        let count = self
            .processor
            .drain_mono(&mut self.input[..frames])
            .map_err(domain_error)?;
        audio.copy_from(&self.input[..frames]);
        Ok(count)
    }

    /// Returns a new feedback object and does not clear the peak window.
    ///
    /// `processedFrames` is a `bigint`, so its 64-bit count stays exact.
    /// `latencyFrames` is a number. See `Feedback` for each measurement.
    ///
    /// `targetMidiNote` is `undefined` in these cases, which are also the
    /// cases where `voiced` is `false`:
    ///
    /// - The kept pitch estimate is unvoiced. This includes the time before
    ///   the first detection and after a reset or a drain completion.
    /// - The estimated frequency is outside the inclusive reporting bounds
    ///   `minFrequencyHz` and `maxFrequencyHz` of the pitch configuration.
    ///
    /// # Errors
    ///
    /// If the conversion to a JavaScript object fails, this method throws.
    pub fn feedback(&self) -> Result<Ts<Feedback>, JsValue> {
        encode_ts(&self.processor.feedback())
    }

    /// Returns the feedback and clears only the stereo peak window.
    ///
    /// The other feedback fields and all audio histories do not change.
    ///
    /// # Errors
    ///
    /// If the conversion to a JavaScript object fails, this method throws. The
    /// method clears the peak only after it successfully makes a feedback
    /// object.
    #[wasm_bindgen(js_name = takeFeedback)]
    pub fn take_feedback(&mut self) -> Result<Ts<Feedback>, JsValue> {
        let feedback = encode_ts(&self.processor.feedback())?;
        self.processor.take_feedback();
        Ok(feedback)
    }

    /// Returns a new telemetry object and does not clear its peak windows.
    ///
    /// The telemetry is for meters and displays. It gives the ratio and the
    /// gain of each voice, the input and output peaks, the pitch clarity and
    /// the state of the effects. `processedFrames` is a `bigint`. See
    /// `Telemetry` for each measurement.
    ///
    /// # Errors
    ///
    /// If the conversion to a JavaScript object fails, this method throws.
    pub fn telemetry(&self) -> Result<Ts<Telemetry>, JsValue> {
        encode_ts(&self.processor.telemetry())
    }

    /// Returns the telemetry and clears only its three peak windows.
    ///
    /// The peak window of the feedback and all audio histories do not change.
    ///
    /// # Errors
    ///
    /// If the conversion to a JavaScript object fails, this method throws. The
    /// method clears the peaks only after it successfully makes a telemetry
    /// object.
    #[wasm_bindgen(js_name = takeTelemetry)]
    pub fn take_telemetry(&mut self) -> Result<Ts<Telemetry>, JsValue> {
        let telemetry = encode_ts(&self.processor.telemetry())?;
        self.processor.take_telemetry();
        Ok(telemetry)
    }

    /// Writes the formant envelopes of the current model into two
    /// `Float32Array` buffers of equal length.
    ///
    /// `input` gets the vocal-tract envelope that the LPC analysis found.
    /// `output` gets the envelope after the width and stretch controls. Point
    /// `k` of `n` is at the frequency `maxFrequencyHz * k / (n - 1)`. The
    /// values are linear magnitudes with no absolute level. Scale them for a
    /// display.
    ///
    /// `maxFrequencyHz` must be finite, more than zero and not more than half
    /// of the sample rate.
    ///
    /// The method returns `true` if the envelopes are valid. It returns
    /// `false` and fills the two arrays with zeros if the formant stage is
    /// disabled or if the current model has no synthesis filter. The audio
    /// does not change.
    ///
    /// The class keeps a buffer for the points. Only the first call and a
    /// call with more points than before reserve memory.
    ///
    /// # Errors
    ///
    /// The method throws if the array lengths are not equal, if the frequency
    /// is not valid or if the reservation of the buffer fails. The arrays
    /// then do not change.
    #[wasm_bindgen(js_name = formantEnvelope)]
    pub fn formant_envelope(
        &mut self,
        #[wasm_bindgen(js_name = maxFrequencyHz)] max_frequency_hz: f32,
        input: &Float32Array,
        output: &Float32Array,
    ) -> Result<bool, JsValue> {
        let points = input.length() as usize;
        // Check before the buffers become longer for a call that cannot succeed.
        Error::check_length(BufferName::Output, points, output.length() as usize)
            .map_err(domain_error)?;
        let analysis = envelope_buffer(&mut self.envelope_input, points)?;
        let synthesis = envelope_buffer(&mut self.envelope_output, points)?;
        let valid = self
            .processor
            .formant_envelope(max_frequency_hz, analysis, synthesis)
            .map_err(domain_error)?;
        input.copy_from(analysis);
        output.copy_from(synthesis);
        Ok(valid)
    }

    /// Resets the audio histories and feedback, as the native processor does.
    ///
    /// The configuration, the random stream and some controller and filter
    /// state do not change. This method cancels any active drain. For a fully
    /// new instance, construct a new `Tunelet`.
    pub fn reset(&mut self) {
        self.processor.reset();
    }
}

/// Validates the JavaScript `maxBlockFrames` number before `usize` conversion.
fn validate_max_block_frames(value: f64) -> Result<usize, JsValue> {
    let field = ConfigField::PrepareMaxBlockFrames;
    let max = f64::from(u32::MAX);
    if !value.is_finite() {
        return Err(js_error(
            &format!(
                "invalid config: {} ({value}) must be finite",
                field.camel_case()
            ),
            &ErrorDetails::InvalidConfig {
                field,
                voice: None,
                problem: ConfigProblemKind::NotFinite,
                value,
                min: Some(1.0),
                max: Some(max),
                other: None,
                other_value: None,
            },
        ));
    }
    if value.fract() != 0.0 {
        return Err(js_error(
            &format!(
                "invalid config: {} ({value}) must be an integer in 1..={}",
                field.camel_case(),
                u32::MAX
            ),
            &ErrorDetails::InvalidConfig {
                field,
                voice: None,
                problem: ConfigProblemKind::NotInteger,
                value,
                min: Some(1.0),
                max: Some(max),
                other: None,
                other_value: None,
            },
        ));
    }
    if !(1.0..=max).contains(&value) {
        return Err(js_error(
            &format!(
                "invalid config: {} ({value}) must be in 1..={}",
                field.camel_case(),
                u32::MAX
            ),
            &ErrorDetails::InvalidConfig {
                field,
                voice: None,
                problem: ConfigProblemKind::IntegerOutOfRange,
                value,
                min: Some(1.0),
                max: Some(max),
                other: None,
                other_value: None,
            },
        ));
    }
    Ok(value as usize)
}

/// Converts a Rust `f32` value to a JavaScript `f64` number without binary
/// conversion artifacts on decimal literals.
fn f32_to_f64(value: f32) -> f64 {
    if value.is_finite() {
        value.to_string().parse::<f64>().unwrap_or(f64::from(value))
    } else {
        f64::from(value)
    }
}

/// Unpacked `camelCase` text and `ErrorDetails` fields for a `ConfigProblem`.
struct UnpackedConfigProblem {
    reason: String,
    kind: ConfigProblemKind,
    value: f64,
    min: Option<f64>,
    max: Option<f64>,
    other: Option<ConfigField>,
    other_value: Option<f64>,
}

/// Unpacks a `ConfigProblem` into `camelCase` message text and `ErrorDetails`
/// components.
fn unpack_config_problem(problem: ConfigProblem) -> UnpackedConfigProblem {
    match problem {
        ConfigProblem::OutOfRange { value, min, max } => UnpackedConfigProblem {
            reason: format!("({value}) must be in {min}..={max}"),
            kind: ConfigProblemKind::OutOfRange,
            value: f32_to_f64(value),
            min: Some(f32_to_f64(min)),
            max: Some(f32_to_f64(max)),
            other: None,
            other_value: None,
        },
        ConfigProblem::IntegerOutOfRange { value, min, max } => UnpackedConfigProblem {
            reason: format!("({value}) must be in {min}..={max}"),
            kind: ConfigProblemKind::IntegerOutOfRange,
            value: value as f64,
            min: Some(min as f64),
            max: Some(max as f64),
            other: None,
            other_value: None,
        },
        ConfigProblem::NotFinite { value } => UnpackedConfigProblem {
            reason: format!("({value}) must be finite"),
            kind: ConfigProblemKind::NotFinite,
            value: f32_to_f64(value),
            min: None,
            max: None,
            other: None,
            other_value: None,
        },
        ConfigProblem::Unordered {
            value,
            other,
            other_value,
        } => UnpackedConfigProblem {
            reason: format!(
                "({value}) must be below {} ({other_value})",
                other.camel_case()
            ),
            kind: ConfigProblemKind::Unordered,
            value: f32_to_f64(value),
            min: None,
            max: None,
            other: Some(other),
            other_value: Some(f32_to_f64(other_value)),
        },
    }
}

/// Unpacked `ErrorDetails` fields for a `MidiProblem`.
struct UnpackedMidiProblem {
    kind: MidiProblemKind,
    byte_index: Option<usize>,
    value: u8,
    min: u8,
    max: u8,
}

/// Unpacks a `MidiProblem` into its kind, optional byte index, value and bounds.
const fn unpack_midi_problem(problem: MidiProblem) -> UnpackedMidiProblem {
    match problem {
        MidiProblem::StatusByte { byte } => UnpackedMidiProblem {
            kind: MidiProblemKind::StatusByte,
            byte_index: Some(0),
            value: byte,
            min: 0x80,
            max: 0xff,
        },
        MidiProblem::DataByte { index, byte } => UnpackedMidiProblem {
            kind: MidiProblemKind::DataByte,
            byte_index: Some(index),
            value: byte,
            min: 0,
            max: 127,
        },
        MidiProblem::Channel { channel } => UnpackedMidiProblem {
            kind: MidiProblemKind::Channel,
            byte_index: None,
            value: channel,
            min: 0,
            max: 15,
        },
        MidiProblem::Note { note } => UnpackedMidiProblem {
            kind: MidiProblemKind::Note,
            byte_index: None,
            value: note,
            min: 0,
            max: 127,
        },
        MidiProblem::Velocity { velocity } => UnpackedMidiProblem {
            kind: MidiProblemKind::Velocity,
            byte_index: None,
            value: velocity,
            min: 0,
            max: 127,
        },
    }
}

/// Formats a `ConfigField` path with an optional `[voice]` suffix in `camelCase`.
fn format_camel_target(field: ConfigField, voice: Option<usize>) -> String {
    let mut target = String::from(field.camel_case());
    if let Some(index) = voice {
        let _ = write!(target, "[{index}]");
    }
    target
}

/// Converts a native `Error` into a JavaScript `Error` with `camelCase` text
/// and `ErrorDetails`.
fn domain_error(error: Error) -> JsValue {
    match error {
        Error::UnsupportedSampleRate { hz } => js_error(
            &format!("sampleRate ({hz} Hz) must be 44100 or 48000"),
            &ErrorDetails::UnsupportedSampleRate { hz: f64::from(hz) },
        ),
        Error::InvalidFrequency { hz, max_hz } => js_error(
            &format!("maxFrequencyHz ({hz} Hz) must be finite and in 0 < hz <= {max_hz}"),
            &ErrorDetails::InvalidFrequency {
                hz: f32_to_f64(hz),
                max_hz: f32_to_f64(max_hz),
            },
        ),
        Error::InvalidConfig {
            field,
            voice,
            problem,
        } => {
            let target = format_camel_target(field, voice);
            let unpacked = unpack_config_problem(problem);
            js_error(
                &format!("invalid config: {target} {}", unpacked.reason),
                &ErrorDetails::InvalidConfig {
                    field,
                    voice,
                    problem: unpacked.kind,
                    value: unpacked.value,
                    min: unpacked.min,
                    max: unpacked.max,
                    other: unpacked.other,
                    other_value: unpacked.other_value,
                },
            )
        }
        Error::BufferLength {
            buffer,
            expected,
            actual,
        } => js_error(
            &format!("{buffer} buffer length is {actual}, expected {expected}"),
            &ErrorDetails::BufferLength {
                buffer,
                expected,
                actual,
            },
        ),
        Error::BlockTooLarge { frames, max } => js_error(
            &format!("block of {frames} frames exceeds maxBlockFrames ({max})"),
            &ErrorDetails::BlockTooLarge { frames, max },
        ),
        Error::InvalidEvent {
            index,
            offset,
            problem,
        } => match problem {
            EventProblem::OffsetOutsideBlock { offset, frames } => js_error(
                &format!("event {index}: offset {offset} is outside the block of {frames} frames"),
                &ErrorDetails::InvalidEvent {
                    index,
                    offset,
                    problem: EventProblemKind::OffsetOutsideBlock,
                    frames: Some(frames),
                    previous: None,
                    field: None,
                    voice: None,
                    config_problem: None,
                    midi_problem: None,
                    byte_index: None,
                    value: None,
                    min: None,
                    max: None,
                    other: None,
                    other_value: None,
                },
            ),
            EventProblem::OffsetDecreasing { offset, previous } => js_error(
                &format!("event {index}: offset {offset} is below previous offset {previous}"),
                &ErrorDetails::InvalidEvent {
                    index,
                    offset,
                    problem: EventProblemKind::OffsetDecreasing,
                    frames: None,
                    previous: Some(previous),
                    field: None,
                    voice: None,
                    config_problem: None,
                    midi_problem: None,
                    byte_index: None,
                    value: None,
                    min: None,
                    max: None,
                    other: None,
                    other_value: None,
                },
            ),
            EventProblem::Config {
                field,
                voice,
                problem,
            } => {
                let target = format_camel_target(field, voice);
                let unpacked = unpack_config_problem(problem);
                js_error(
                    &format!(
                        "event {index} at offset {offset}: {target} {}",
                        unpacked.reason
                    ),
                    &ErrorDetails::InvalidEvent {
                        index,
                        offset,
                        problem: EventProblemKind::Config,
                        frames: None,
                        previous: None,
                        field: Some(field),
                        voice,
                        config_problem: Some(unpacked.kind),
                        midi_problem: None,
                        byte_index: None,
                        value: Some(unpacked.value),
                        min: unpacked.min,
                        max: unpacked.max,
                        other: unpacked.other,
                        other_value: unpacked.other_value,
                    },
                )
            }
            EventProblem::Midi { problem } => {
                let unpacked = unpack_midi_problem(problem);
                js_error(
                    &format!("event {index} at offset {offset}: {problem}"),
                    &ErrorDetails::InvalidEvent {
                        index,
                        offset,
                        problem: EventProblemKind::Midi,
                        frames: None,
                        previous: None,
                        field: None,
                        voice: None,
                        config_problem: None,
                        midi_problem: Some(unpacked.kind),
                        byte_index: unpacked.byte_index,
                        value: Some(f64::from(unpacked.value)),
                        min: Some(f64::from(unpacked.min)),
                        max: Some(f64::from(unpacked.max)),
                        other: None,
                        other_value: None,
                    },
                )
            }
        },
        Error::InvalidMidi { problem } => {
            let unpacked = unpack_midi_problem(problem);
            js_error(
                &problem.to_string(),
                &ErrorDetails::InvalidMidi {
                    problem: unpacked.kind,
                    byte_index: unpacked.byte_index,
                    value: unpacked.value,
                    min: unpacked.min,
                    max: unpacked.max,
                },
            )
        }
    }
}

/// Removes any Rust module path prefixes from a serde error string.
fn sanitize_serde_message(raw: &str) -> String {
    let mut text = raw.replace("tunelet::wasm::BlockEvents", "TimedEvent[]");
    text = text.replace("BlockEvents", "TimedEvent[]");
    while let Some(start) = text.find("tunelet::") {
        let rest = &text[start + "tunelet::".len()..];
        let mut offset = 0;
        while let Some(sep) = rest[offset..].find("::") {
            let segment = &rest[offset..offset + sep];
            if segment
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
            {
                offset += sep + 2;
            } else {
                break;
            }
        }
        text.replace_range(start..start + "tunelet::".len() + offset, "");
    }
    text
}

/// Builds a JavaScript `Error` with an attached `details` property.
fn js_error(message: &str, details: &ErrorDetails) -> JsValue {
    let error = js_sys::Error::new(message);
    if let Ok(details_js) = details.into_js() {
        let _ = js_sys::Reflect::set(error.as_ref(), &JsValue::from_str("details"), &details_js);
    }
    error.into()
}

/// Converts a Rust `Tsify` value into a JavaScript object or throws a
/// structured error.
fn encode_ts<T: Tsify + Serialize>(value: &T) -> Result<Ts<T>, JsValue> {
    value.into_ts().map_err(|err| {
        js_error(
            &sanitize_serde_message(&err.to_string()),
            &ErrorDetails::SerializationFailed,
        )
    })
}

/// Decodes a `ProcessorConfig` from JavaScript without Rust path leaks.
fn decode_config(config: &Ts<ProcessorConfig>) -> Result<ProcessorConfig, JsValue> {
    ProcessorConfig::from_js(config.js_value()).map_err(|err| {
        let reason = sanitize_serde_message(&err.to_string());
        js_error(
            &format!("invalid ProcessorConfig: {reason}"),
            &ErrorDetails::InvalidInput {
                target: InputTarget::ProcessorConfig,
            },
        )
    })
}

/// Decodes the optional block controls before the audio state changes.
///
/// If there is no value, the result is an empty `Vec`, and no allocation
/// occurs. If the caller gives an array, the decode can allocate storage.
fn decode_events(events: Option<Ts<BlockEvents>>) -> Result<BlockEvents, JsValue> {
    match events {
        Some(value) => BlockEvents::from_js(value.js_value()).map_err(|err| {
            let reason = sanitize_serde_message(&err.to_string());
            js_error(
                &format!("invalid TimedEvent[]: {reason}"),
                &ErrorDetails::InvalidInput {
                    target: InputTarget::TimedEvents,
                },
            )
        }),
        None => Ok(BlockEvents::default()),
    }
}

/// Returns the first `points` entries of a reusable envelope buffer. If the
/// buffer is shorter, it becomes `points` long first.
///
/// If the allocation fails, this function returns a JavaScript error. It does
/// not panic.
fn envelope_buffer(buffer: &mut Vec<f32>, points: usize) -> Result<&mut [f32], JsValue> {
    if buffer.len() < points {
        buffer
            .try_reserve_exact(points - buffer.len())
            .map_err(|_| {
                js_error(
                    "failed to reserve formant envelope buffer",
                    &ErrorDetails::AllocationFailed,
                )
            })?;
        buffer.resize(points, 0.0);
    }
    Ok(&mut buffer[..points])
}

/// Allocates PCM storage that contains zeros.
///
/// If the capacity overflows or the allocation fails, this function returns a
/// JavaScript error. It does not panic.
fn pcm_buffer(frames: usize) -> Result<Vec<i16>, JsValue> {
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(frames).map_err(|_| {
        js_error(
            "failed to reserve PCM buffer",
            &ErrorDetails::AllocationFailed,
        )
    })?;
    buffer.resize(frames, 0);
    Ok(buffer)
}

/// Serializes the bounded Wasm frame count as a JavaScript number.
///
/// The serializer of the feedback structure keeps its other counter, a `u64`,
/// as a `bigint`. This module compiles only for wasm32, where every `usize`
/// fits in a `u32`.
pub(crate) fn serialize_frame_count<S: serde::Serializer>(
    frames: &usize,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_u32(*frames as u32)
}
