//! The optional JavaScript bindings for `wasm32-unknown-unknown`.
//!
//! To make the JavaScript glue, the Wasm binary and the TypeScript
//! declarations, do these steps:
//!
//! 1. Build the crate for `wasm32-unknown-unknown` with the `wasm` feature.
//! 2. Run wasm-bindgen on the result.
//!
//! wasm-pack can do the two steps in one command. The npm entry point uses
//! `loadTunelet()` to initialize these bindings in browsers and in Node.js,
//! and `createTuneletNode()` to connect an `AudioWorkletNode`.
//!
//! The exported `Tunelet` class owns a native [`Processor`], reusable 16-bit
//! PCM buffers, reusable `f32` buffers and a reusable timed MIDI buffer.
//! JavaScript gives `Int16Array` or `Float32Array` audio buffers. The class
//! copies the input before it writes the output.
//!
//! Configuration objects obey these rules:
//!
//! - Field names use camelCase.
//! - Enum values are strings.
//! - Each object must contain all required fields.
//! - The optional harmony `voices` field defaults to 4.
//!
//! [`default_config`] gives a configuration to start from. Control events use
//! `{ type, value }`, and the reset control has no `value`. MIDI messages use
//! `{ type, channel, note, velocity }`, with the fields that apply to the
//! selected variant. You can also supply timed MIDI controls in a
//! `Uint32Array` of 4-word tuples `[offset, status, data1, data2]`, or immediate
//! 3-byte MIDI messages in a `Uint8Array`. The TypeScript declarations come
//! from the same Rust types that validation uses.
//!
//! The class allocates the PCM, `f32` and MIDI buffers at construction and
//! uses them again for each block. The typed-array methods for MIDI controls
//! (`Uint32Array` block events, `applyMidi`, `applyCorrectionMidi`), feedback
//! (`feedbackInto`, `takeFeedbackInto`) and telemetry (`telemetryInto`,
//! `takeTelemetryInto`) do not allocate Wasm memory or JavaScript objects.
//! These operations can allocate:
//!
//! - The decode of `TimedEvent[]` object arrays.
//! - The conversion of configurations.
//! - `feedback`, `takeFeedback`, `telemetry` and `takeTelemetry` objects.
//! - The buffers of the formant envelopes on the first or larger call.
//! - Errors.
//!
//! If you do not need a processor any more, call the generated `free()`
//! method.

use crate::error::EventProblem;
use crate::{
    ControlEvent, Error, Feedback, MidiEvent, PrepareConfig, Processor, ProcessorConfig,
    SampleRate, Telemetry, TimedEvent,
};
use js_sys::{Float32Array, Float64Array, Int16Array, Uint32Array, Uint8Array};
use serde::{Deserialize, Serialize};
use tsify::{Ts, Tsify};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// The number of `f64` elements in a typed-array [`Feedback`] snapshot.
const FEEDBACK_LEN: usize = 8;

/// The number of `f64` elements in a typed-array [`Telemetry`] snapshot.
const TELEMETRY_LEN: usize = 45;

/// The number of `u32` words for each timed MIDI event in a `Uint32Array`.
const MIDI_EVENT_WORDS: usize = 4;

/// The status flag that routes a `Uint32Array` MIDI event to the correction
/// keyboard instead of harmony.
const CORRECTION_MIDI_FLAG: u32 = 0x100;

/// The minimum preallocated capacity for timed `Uint32Array` MIDI events.
const MIN_MIDI_CAPACITY: usize = 64;

/// The maximum initial preallocated capacity for timed `Uint32Array` MIDI
/// events.
const MAX_MIDI_CAPACITY: usize = 4096;

/// The events for one call that processes audio (`TimedEvent[] | Uint32Array`
/// in TypeScript).
///
/// Each offset is a zero-based frame index in the current block. Each offset
/// must be in the block and must not be less than the offset before it. Events
/// with equal offsets execute in array order. An empty block accepts no
/// events.
///
/// For zero-allocation MIDI controls, supply a `Uint32Array` whose length is a
/// multiple of 4. Each group of four words is `[offset, status, data1, data2]`,
/// where `status` is in `0x80..=0xff` for harmony MIDI or `0x180..=0x1ff`
/// (`status | 0x100`) for correction MIDI, and `data1` and `data2` are in
/// `0..=127`. If there are no events, omit the optional argument.
#[derive(Default, Serialize, Deserialize, Tsify)]
#[serde(transparent)]
pub struct BlockEvents(#[tsify(type = "TimedEvent[] | Uint32Array")] Vec<TimedEvent>);

/// Returns a complete, independent default configuration for JavaScript.
///
/// Change its fields and give it to `Tunelet` or to `setConfig`. The
/// `randomSeed` field is a JavaScript `bigint`, so it keeps all 64 bits. After
/// the binding decodes them, floating-point controls have the precision of the
/// Rust `f32` type.
///
/// # Errors
///
/// If the conversion to a JavaScript object fails, this function returns a
/// JavaScript `Error`.
#[wasm_bindgen(js_name = defaultConfig)]
pub fn default_config() -> Result<Ts<ProcessorConfig>, JsError> {
    Ok(ProcessorConfig::default().into_ts()?)
}

/// A stateful audio processor that JavaScript uses as the `Tunelet` class.
///
/// The input is mono signed 16-bit PCM (`Int16Array`) or normalized mono audio
/// (`Float32Array`). The output is planar stereo or an in-place mono downmix.
/// The quantization, latency and configuration behavior are the same as in the
/// native processor. Buffer lengths count frames, not bytes.
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
/// that are out of range.
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
    /// The reusable `f32` buffer for input or in-place mono audio.
    float_input: Vec<f32>,
    /// The reusable `f32` buffer for the left channel.
    float_left: Vec<f32>,
    /// The reusable `f32` buffer for the right channel.
    float_right: Vec<f32>,
    /// The preallocated event storage for `Uint32Array` MIDI blocks.
    midi_events: Vec<TimedEvent>,
    /// The reusable buffers for the two formant envelopes. They become longer
    /// when a call needs more points, and they do not become shorter.
    envelope_input: Vec<f32>,
    envelope_output: Vec<f32>,
}

#[wasm_bindgen(js_class = Tunelet)]
impl WasmProcessor {
    /// Prepares a processor and its reusable audio and MIDI buffers.
    ///
    /// `sample_rate` must be 44100, 48000, 88200, 96000, 176400 or 192000.
    /// `max_block_frames` must be an integer in 1..=4294967295. It is a limit
    /// for each call, and smaller blocks and empty blocks are also valid. If
    /// you omit `config`, the processor uses the default configuration. The
    /// constructor validates the settings before it allocates the processor or
    /// the buffers. In TypeScript, use
    /// `new Tunelet(sampleRate, maxBlockFrames, config)`.
    ///
    /// # Errors
    ///
    /// The constructor throws if one of these conditions occurs:
    ///
    /// - The rate is not 44100, 48000, 88200, 96000, 176400 or 192000.
    /// - The block limit is not an integer or is out of range.
    /// - The configuration object is malformed or a field is missing.
    /// - The configuration is not valid.
    /// - The reservation of a buffer fails.
    ///
    /// The available Wasm memory must also have sufficient space for the DSP
    /// histories.
    #[wasm_bindgen(constructor)]
    pub fn new(
        sample_rate: f64,
        max_block_frames: f64,
        config: Option<Ts<ProcessorConfig>>,
    ) -> Result<WasmProcessor, JsError> {
        // Accept JavaScript numbers without the u32 coercion of wasm-bindgen.
        // That coercion silently truncates fractions and wraps large values.
        let rate = match sample_rate {
            44100.0 => SampleRate::Hz44100,
            48000.0 => SampleRate::Hz48000,
            88200.0 => SampleRate::Hz88200,
            96000.0 => SampleRate::Hz96000,
            176400.0 => SampleRate::Hz176400,
            192000.0 => SampleRate::Hz192000,
            _ => {
                return Err(JsError::new(
                    "sampleRate must be 44100, 48000, 88200, 96000, 176400 or 192000",
                ))
            }
        };
        if !(1.0..=f64::from(u32::MAX)).contains(&max_block_frames)
            || max_block_frames.fract() != 0.0
        {
            return Err(JsError::new(
                "maxBlockFrames must be an integer in 1..=4294967295",
            ));
        }
        let max_block_frames = max_block_frames as usize;
        let preparation = PrepareConfig::new(rate, max_block_frames)?;
        let config = match config {
            Some(value) => value.to_rust()?,
            None => ProcessorConfig::default(),
        };
        let processor = Processor::new(preparation, config)?;
        let midi_capacity = max_block_frames.clamp(MIN_MIDI_CAPACITY, MAX_MIDI_CAPACITY);
        let mut midi_events = Vec::new();
        midi_events.try_reserve_exact(midi_capacity)?;
        Ok(Self {
            processor,
            input: pcm_buffer(max_block_frames)?,
            left: pcm_buffer(max_block_frames)?,
            right: pcm_buffer(max_block_frames)?,
            float_input: float_buffer(max_block_frames)?,
            float_left: float_buffer(max_block_frames)?,
            float_right: float_buffer(max_block_frames)?,
            midi_events,
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

    /// The nominal delay in host frames, with the Q14 resampler filter delay
    /// included at 88.2, 96, 176.4 and 192 kHz.
    ///
    /// Other signal paths have different timing.
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
    pub fn config(&self) -> Result<Ts<ProcessorConfig>, JsError> {
        Ok(self.processor.config().into_ts()?)
    }

    /// Decodes and validates a configuration, then replaces the complete
    /// configuration with it.
    ///
    /// State retention and drain cancellation follow the native processor.
    /// The conversion ignores additional properties of the JavaScript object.
    ///
    /// # Errors
    ///
    /// The method throws if an object is not valid or a field is missing.
    /// After an error, all processor state and any active drain stay the same.
    #[wasm_bindgen(js_name = setConfig)]
    pub fn set_config(&mut self, config: Ts<ProcessorConfig>) -> Result<(), JsError> {
        self.processor.set_config(config.to_rust()?)?;
        Ok(())
    }

    /// Decodes a 3-byte MIDI message from a `Uint8Array` and applies it to
    /// harmony immediately.
    ///
    /// `bytes` must have length 3 (`[status, data1, data2]`, with `status` in
    /// `0x80..=0xff` and `data1` and `data2` in `0..=127`). Returns `true` if
    /// the message is Note On, Note Off or controller 123 (All Notes Off).
    /// Returns `false` for other valid 3-byte MIDI messages. This method does
    /// not allocate Wasm memory or JavaScript objects.
    ///
    /// # Errors
    ///
    /// Throws if `bytes.length` is not 3 or if the MIDI bytes are out of range,
    /// before the processor state changes.
    #[wasm_bindgen(js_name = applyMidi)]
    pub fn apply_midi(&mut self, bytes: &Uint8Array) -> Result<bool, JsError> {
        let Some(event) = decode_midi_bytes(bytes)? else {
            return Ok(false);
        };
        self.processor.apply_midi(event)?;
        Ok(true)
    }

    /// Decodes a 3-byte MIDI message from a `Uint8Array` and applies it to the
    /// correction keyboard immediately.
    ///
    /// Byte rules, return values and zero-allocation behavior are the same as
    /// for `applyMidi`.
    ///
    /// # Errors
    ///
    /// Throws if `bytes.length` is not 3 or if the MIDI bytes are out of range,
    /// before the processor state changes.
    #[wasm_bindgen(js_name = applyCorrectionMidi)]
    pub fn apply_correction_midi(&mut self, bytes: &Uint8Array) -> Result<bool, JsError> {
        let Some(event) = decode_midi_bytes(bytes)? else {
            return Ok(false);
        };
        self.processor.apply_correction_midi(event)?;
        Ok(true)
    }

    /// Processes mono input into left and right `Int16Array` buffers of equal
    /// length.
    ///
    /// If the call is successful, it writes every output sample. Optional
    /// events apply before their frame, and their offsets obey the rules of
    /// `BlockEvents` (`TimedEvent[]` or `Uint32Array`). An empty block must
    /// have no events. A successful call cancels a drain, also for an empty
    /// block. The call uses the existing JavaScript and Rust PCM buffers again.
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
    ) -> Result<(), JsError> {
        let frames = input.length() as usize;
        Error::check_length(frames, left.length() as usize)?;
        Error::check_length(frames, right.length() as usize)?;
        self.check_frames(frames)?;
        let events = decode_events(&mut self.midi_events, frames, events)?;
        input.copy_to(&mut self.input[..frames]);
        self.processor.process_planar(
            &self.input[..frames],
            &mut self.left[..frames],
            &mut self.right[..frames],
            events.as_slice(),
        )?;
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
    ) -> Result<(), JsError> {
        let frames = audio.length() as usize;
        self.check_frames(frames)?;
        let events = decode_events(&mut self.midi_events, frames, events)?;
        audio.copy_to(&mut self.input[..frames]);
        self.processor
            .process_mono_in_place(&mut self.input[..frames], events.as_slice())?;
        audio.copy_from(&self.input[..frames]);
        Ok(())
    }

    /// Processes mono `Float32Array` input in `-1.0..=1.0` into left and right
    /// `Float32Array` buffers of equal length.
    ///
    /// Optional events accept `TimedEvent[]` or a packed `Uint32Array` of
    /// `[offset, status, data1, data2]` MIDI tuples. Channel length checks,
    /// block limits, event timing and drain cancellation are the same as for
    /// `process`.
    ///
    /// # Errors
    ///
    /// Throws if the channel lengths are not equal, if the block is larger
    /// than `maxBlockFrames` or if an event is not valid. On error, the output
    /// arrays and the DSP state do not change.
    #[wasm_bindgen(js_name = processFloat32)]
    pub fn process_f32(
        &mut self,
        input: &Float32Array,
        left: &Float32Array,
        right: &Float32Array,
        events: Option<Ts<BlockEvents>>,
    ) -> Result<(), JsError> {
        let frames = input.length() as usize;
        Error::check_length(frames, left.length() as usize)?;
        Error::check_length(frames, right.length() as usize)?;
        self.check_frames(frames)?;
        let events = decode_events(&mut self.midi_events, frames, events)?;
        input.copy_to(&mut self.float_input[..frames]);
        self.processor.process_planar_f32(
            &self.float_input[..frames],
            &mut self.float_left[..frames],
            &mut self.float_right[..frames],
            events.as_slice(),
        )?;
        left.copy_from(&self.float_left[..frames]);
        right.copy_from(&self.float_right[..frames]);
        Ok(())
    }

    /// Replaces the audio in a mono `Float32Array` with the stereo downmix.
    ///
    /// Input quantization, output scaling, stereo peak measurement, events and
    /// drain cancellation follow `processFloat32` and `processMono`.
    ///
    /// # Errors
    ///
    /// Throws if the block is larger than `maxBlockFrames` or an event is not
    /// valid, before the array or the DSP state changes.
    #[wasm_bindgen(js_name = processMonoFloat32)]
    pub fn process_mono_f32(
        &mut self,
        audio: &Float32Array,
        events: Option<Ts<BlockEvents>>,
    ) -> Result<(), JsError> {
        let frames = audio.length() as usize;
        self.check_frames(frames)?;
        let events = decode_events(&mut self.midi_events, frames, events)?;
        audio.copy_to(&mut self.float_input[..frames]);
        self.processor
            .process_mono_in_place_f32(&mut self.float_input[..frames], events.as_slice())?;
        audio.copy_from(&self.float_input[..frames]);
        Ok(())
    }

    /// Starts a finite tail budget and returns its frame count as a number.
    ///
    /// Call this method one time after the final input block. Another call
    /// starts the full budget again, but it does not clear the histories. See
    /// `beginDrain` for completion.
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
    pub fn drain(&mut self, left: &Int16Array, right: &Int16Array) -> Result<usize, JsError> {
        let frames = left.length() as usize;
        Error::check_length(frames, right.length() as usize)?;
        self.check_frames(frames)?;
        let count = self
            .processor
            .drain_planar(&mut self.left[..frames], &mut self.right[..frames])?;
        left.copy_from(&self.left[..frames]);
        right.copy_from(&self.right[..frames]);
        Ok(count)
    }

    /// Renders a mono tail block and returns its frame count.
    ///
    /// The method fills the unused output with zeros. The downmix, completion
    /// and feedback behavior are the same as for `drainMono`.
    ///
    /// # Errors
    ///
    /// If the block is larger than `maxBlockFrames`, this method throws before
    /// the output or the state changes.
    #[wasm_bindgen(js_name = drainMono)]
    pub fn drain_mono(&mut self, audio: &Int16Array) -> Result<usize, JsError> {
        let frames = audio.length() as usize;
        self.check_frames(frames)?;
        let count = self.processor.drain_mono(&mut self.input[..frames])?;
        audio.copy_from(&self.input[..frames]);
        Ok(count)
    }

    /// Renders stereo tail frames into `Float32Array` buffers of equal length.
    ///
    /// Output scaling, zero fill and completion are the same as for `drain`.
    ///
    /// # Errors
    ///
    /// Throws if the channel lengths are not equal or if the block is larger
    /// than `maxBlockFrames`, before the arrays or the drain state change.
    #[wasm_bindgen(js_name = drainFloat32)]
    pub fn drain_f32(
        &mut self,
        left: &Float32Array,
        right: &Float32Array,
    ) -> Result<usize, JsError> {
        let frames = left.length() as usize;
        Error::check_length(frames, right.length() as usize)?;
        self.check_frames(frames)?;
        let count = self.processor.drain_planar_f32(
            &mut self.float_left[..frames],
            &mut self.float_right[..frames],
        )?;
        left.copy_from(&self.float_left[..frames]);
        right.copy_from(&self.float_right[..frames]);
        Ok(count)
    }

    /// Renders a mono tail block into a `Float32Array` and returns its frame
    /// count.
    ///
    /// Output scaling, zero fill and completion follow `drainMono`.
    ///
    /// # Errors
    ///
    /// Throws if the block is larger than `maxBlockFrames` before the output
    /// or the state changes.
    #[wasm_bindgen(js_name = drainMonoFloat32)]
    pub fn drain_mono_f32(&mut self, audio: &Float32Array) -> Result<usize, JsError> {
        let frames = audio.length() as usize;
        self.check_frames(frames)?;
        let count = self
            .processor
            .drain_mono_f32(&mut self.float_input[..frames])?;
        audio.copy_from(&self.float_input[..frames]);
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
    pub fn feedback(&self) -> Result<Ts<Feedback>, JsError> {
        Ok(self.processor.feedback().into_ts()?)
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
    pub fn take_feedback(&mut self) -> Result<Ts<Feedback>, JsError> {
        let feedback = self.processor.feedback().into_ts()?;
        self.processor.take_feedback();
        Ok(feedback)
    }

    /// Writes the feedback snapshot into a `Float64Array` of length 8 without
    /// allocation, and does not clear the peak window.
    ///
    /// The indices are `0`: `resampleRatio`, `1`: `periodSamples`,
    /// `2`: `frequencyHz`, `3`: `targetMidiNote` (`NaN` if `voiced` is `0`),
    /// `4`: `voiced` (`1` or `0`), `5`: `peakLevel`, `6`: `processedFrames`
    /// and `7`: `latencyFrames`.
    ///
    /// # Errors
    ///
    /// Throws if `output.length` is not 8, before `output` changes.
    #[wasm_bindgen(js_name = feedbackInto)]
    pub fn feedback_into(&self, output: &Float64Array) -> Result<(), JsError> {
        Error::check_length(FEEDBACK_LEN, output.length() as usize)?;
        write_feedback(&self.processor.feedback(), output);
        Ok(())
    }

    /// Writes the feedback snapshot into a `Float64Array` of length 8 without
    /// allocation, and clears only the stereo peak window.
    ///
    /// # Errors
    ///
    /// Throws if `output.length` is not 8, before `output` or the peak window
    /// changes.
    #[wasm_bindgen(js_name = takeFeedbackInto)]
    pub fn take_feedback_into(&mut self, output: &Float64Array) -> Result<(), JsError> {
        Error::check_length(FEEDBACK_LEN, output.length() as usize)?;
        write_feedback(&self.processor.take_feedback(), output);
        Ok(())
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
    pub fn telemetry(&self) -> Result<Ts<Telemetry>, JsError> {
        Ok(self.processor.telemetry().into_ts()?)
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
    pub fn take_telemetry(&mut self) -> Result<Ts<Telemetry>, JsError> {
        let telemetry = self.processor.telemetry().into_ts()?;
        self.processor.take_telemetry();
        Ok(telemetry)
    }

    /// Writes the telemetry snapshot into a `Float64Array` of length 45 without
    /// allocation, and does not clear its peak windows.
    ///
    /// The indices are `0`: `processedFrames`, `1`: `inputPeak`,
    /// `2..=3`: `outputPeak`, `4`: `reverbWetPeak`, `5`: `clarity`,
    /// `6`: `tracking` (`1` or `0`), `7`: `voiceCount`,
    /// `8..=23`: `voiceRatios`, `24..=39`: `voiceGains`, `40`: `punchGain`,
    /// `41`: `warmGain`, `42`: `alienLoopFrames`, `43`: `alienPhaseFrames` and
    /// `44`: `formantGeneration`.
    ///
    /// # Errors
    ///
    /// Throws if `output.length` is not 45, before `output` changes.
    #[wasm_bindgen(js_name = telemetryInto)]
    pub fn telemetry_into(&self, output: &Float64Array) -> Result<(), JsError> {
        Error::check_length(TELEMETRY_LEN, output.length() as usize)?;
        write_telemetry(&self.processor.telemetry(), output);
        Ok(())
    }

    /// Writes the telemetry snapshot into a `Float64Array` of length 45 without
    /// allocation, and clears only its three peak windows.
    ///
    /// # Errors
    ///
    /// Throws if `output.length` is not 45, before `output` or the peak windows
    /// change.
    #[wasm_bindgen(js_name = takeTelemetryInto)]
    pub fn take_telemetry_into(&mut self, output: &Float64Array) -> Result<(), JsError> {
        Error::check_length(TELEMETRY_LEN, output.length() as usize)?;
        write_telemetry(&self.processor.take_telemetry(), output);
        Ok(())
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
        max_frequency_hz: f32,
        input: &Float32Array,
        output: &Float32Array,
    ) -> Result<bool, JsError> {
        let points = input.length() as usize;
        // Check before the buffers become longer for a call that cannot succeed.
        Error::check_length(points, output.length() as usize)?;
        let analysis = envelope_buffer(&mut self.envelope_input, points)?;
        let synthesis = envelope_buffer(&mut self.envelope_output, points)?;
        let valid = self
            .processor
            .formant_envelope(max_frequency_hz, analysis, synthesis)?;
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

impl WasmProcessor {
    /// Checks the block limit before a method uses a prefix of the buffers.
    fn check_frames(&self, frames: usize) -> Result<(), Error> {
        let max = self.input.len();
        if frames > max {
            Err(Error::BlockTooLarge { frames, max })
        } else {
            Ok(())
        }
    }
}

/// A borrowed slice of MIDI controls or an owned vector of block events.
enum DecodedEvents<'a> {
    /// An empty slice or events decoded into the preallocated MIDI buffer.
    Borrowed(&'a [TimedEvent]),
    /// Events decoded from a JavaScript `TimedEvent[]` array.
    Owned(Vec<TimedEvent>),
}

impl DecodedEvents<'_> {
    /// Returns the validated or decoded slice for the native processor.
    fn as_slice(&self) -> &[TimedEvent] {
        match self {
            Self::Borrowed(slice) => slice,
            Self::Owned(events) => events.as_slice(),
        }
    }
}

/// Decodes the optional block controls before the audio state changes.
///
/// If there is no value, no allocation occurs. If the caller supplies a
/// `Uint32Array`, the function decodes the 4-word MIDI tuples into `scratch`
/// without allocation. If the caller supplies `TimedEvent[]`, serde decodes
/// the array.
fn decode_events<'a>(
    scratch: &'a mut Vec<TimedEvent>,
    frames: usize,
    events: Option<Ts<BlockEvents>>,
) -> Result<DecodedEvents<'a>, JsError> {
    let Some(value) = events else {
        return Ok(DecodedEvents::Borrowed(&[]));
    };
    let js = value.js_value();
    if let Some(words) = js.dyn_ref::<Uint32Array>() {
        decode_midi_words(scratch, frames, words)?;
        return Ok(DecodedEvents::Borrowed(scratch.as_slice()));
    }
    Ok(DecodedEvents::Owned(value.to_rust()?.0))
}

/// Decodes and validates packed `[offset, status, data1, data2]` MIDI tuples.
fn decode_midi_words(
    scratch: &mut Vec<TimedEvent>,
    frames: usize,
    words: &Uint32Array,
) -> Result<(), JsError> {
    scratch.clear();
    let length = words.length() as usize;
    if !length.is_multiple_of(MIDI_EVENT_WORDS) {
        return Err(Error::InvalidMidi.into());
    }
    let count = length / MIDI_EVENT_WORDS;
    if scratch.capacity() < count {
        scratch.try_reserve_exact(count - scratch.capacity())?;
    }
    let mut previous = 0usize;
    for index in 0..count {
        let base = (index * MIDI_EVENT_WORDS) as u32;
        let offset = words.get_index(base) as usize;
        if offset >= frames {
            return Err(Error::InvalidEvent {
                index,
                problem: EventProblem::OffsetOutsideBlock,
            }
            .into());
        }
        if offset < previous {
            return Err(Error::InvalidEvent {
                index,
                problem: EventProblem::OffsetDecreasing,
            }
            .into());
        }
        let raw_status = words.get_index(base + 1);
        let data1 = words.get_index(base + 2);
        let data2 = words.get_index(base + 3);
        let correction = (raw_status & CORRECTION_MIDI_FLAG) != 0;
        let status = raw_status & !CORRECTION_MIDI_FLAG;
        if !(0x80..=0xff).contains(&status) || data1 > 127 || data2 > 127 {
            return Err(Error::InvalidEvent {
                index,
                problem: EventProblem::Midi,
            }
            .into());
        }
        let parsed =
            MidiEvent::from_bytes([status as u8, data1 as u8, data2 as u8]).map_err(|_| {
                Error::InvalidEvent {
                    index,
                    problem: EventProblem::Midi,
                }
            })?;
        if let Some(midi) = parsed {
            let event = if correction {
                ControlEvent::CorrectionMidi(midi)
            } else {
                ControlEvent::Midi(midi)
            };
            scratch.push(TimedEvent { offset, event });
        }
        previous = offset;
    }
    Ok(())
}

/// Decodes one 3-byte MIDI message from a `Uint8Array` without allocation.
fn decode_midi_bytes(bytes: &Uint8Array) -> Result<Option<MidiEvent>, Error> {
    Error::check_length(3, bytes.length() as usize)?;
    MidiEvent::from_bytes([bytes.get_index(0), bytes.get_index(1), bytes.get_index(2)])
}

/// Writes all 8 [`Feedback`] fields into a validated `Float64Array`.
fn write_feedback(feedback: &Feedback, output: &Float64Array) {
    output.set_index(0, f64::from(feedback.resample_ratio));
    output.set_index(1, f64::from(feedback.period_samples));
    output.set_index(2, f64::from(feedback.frequency_hz));
    output.set_index(3, feedback.target_midi_note.map_or(f64::NAN, f64::from));
    output.set_index(4, if feedback.voiced { 1.0 } else { 0.0 });
    output.set_index(5, f64::from(feedback.peak_level));
    output.set_index(6, feedback.processed_frames as f64);
    output.set_index(7, feedback.latency_frames as f64);
}

/// Writes all 45 [`Telemetry`] fields into a validated `Float64Array`.
fn write_telemetry(telemetry: &Telemetry, output: &Float64Array) {
    output.set_index(0, telemetry.processed_frames as f64);
    output.set_index(1, f64::from(telemetry.input_peak));
    output.set_index(2, f64::from(telemetry.output_peak[0]));
    output.set_index(3, f64::from(telemetry.output_peak[1]));
    output.set_index(4, f64::from(telemetry.reverb_wet_peak));
    output.set_index(5, f64::from(telemetry.clarity));
    output.set_index(6, if telemetry.tracking { 1.0 } else { 0.0 });
    output.set_index(7, f64::from(telemetry.voice_count));
    for (index, &ratio) in telemetry.voice_ratios.iter().enumerate() {
        output.set_index(8 + index as u32, f64::from(ratio));
    }
    for (index, &gain) in telemetry.voice_gains.iter().enumerate() {
        output.set_index(24 + index as u32, f64::from(gain));
    }
    output.set_index(40, f64::from(telemetry.punch_gain));
    output.set_index(41, f64::from(telemetry.warm_gain));
    output.set_index(42, f64::from(telemetry.alien_loop_frames));
    output.set_index(43, f64::from(telemetry.alien_phase_frames));
    output.set_index(44, f64::from(telemetry.formant_generation));
}

/// Returns the first `points` entries of a reusable envelope buffer. If the
/// buffer is shorter, it becomes `points` long first.
///
/// If the allocation fails, this function returns a JavaScript error. It does
/// not panic.
fn envelope_buffer(buffer: &mut Vec<f32>, points: usize) -> Result<&mut [f32], JsError> {
    if buffer.len() < points {
        buffer.try_reserve_exact(points - buffer.len())?;
        buffer.resize(points, 0.0);
    }
    Ok(&mut buffer[..points])
}

/// Allocates PCM storage that contains zeros.
///
/// If the capacity overflows or the allocation fails, this function returns a
/// JavaScript error. It does not panic.
fn pcm_buffer(frames: usize) -> Result<Vec<i16>, JsError> {
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(frames)?;
    buffer.resize(frames, 0);
    Ok(buffer)
}

/// Allocates `f32` audio storage that contains zeros.
///
/// If the capacity overflows or the allocation fails, this function returns a
/// JavaScript error. It does not panic.
fn float_buffer(frames: usize) -> Result<Vec<f32>, JsError> {
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(frames)?;
    buffer.resize(frames, 0.0);
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
