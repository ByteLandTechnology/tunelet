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
//! The exported `Tunelet` class owns a native [`Processor`] and three reusable
//! PCM buffers. JavaScript gives the `Int16Array` input and output buffers. The
//! PCM API does not give buffer pointers or long-lived memory views to
//! JavaScript. The class copies the input before it writes the output.
//!
//! Configuration objects obey these rules:
//!
//! - Field names use camelCase.
//! - Enum values are strings.
//! - Each object must contain all of its fields.
//!
//! [`default_config`] gives a configuration to start from. Control events use
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
//! allocation. If you do not need a processor any more, call the generated
//! `free()` method.

use crate::{
    Error, Feedback, PrepareConfig, Processor, ProcessorConfig, SampleRate, Telemetry, TimedEvent,
};
use js_sys::{Float32Array, Int16Array};
use serde::{Deserialize, Serialize};
use tsify::{Ts, Tsify};
use wasm_bindgen::prelude::*;

/// The events for one call that processes audio (`TimedEvent[]` in TypeScript).
///
/// Each offset is a zero-based frame index in the current block. Each offset
/// must be in the block and must not be less than the offset before it. Events
/// with equal offsets execute in array order. An empty block accepts no
/// events. If there are no events, omit the optional argument. Then the
/// binding does not decode an array.
#[derive(Default, Serialize, Deserialize, Tsify)]
#[serde(transparent)]
pub struct BlockEvents(Vec<TimedEvent>);

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
    /// `sample_rate` must be 44100 or 48000. `max_block_frames` must be an
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
    /// - The configuration object is malformed or a field is missing.
    /// - The configuration is not valid.
    /// - The reservation of a PCM buffer fails.
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
            _ => return Err(JsError::new("sampleRate must be 44100 or 48000")),
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

    /// The nominal resynthesis delay in frames.
    ///
    /// Other signal paths have different timing. See the `latencyFrames`
    /// property for the timing of the dry path and the voices.
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
    ) -> Result<(), JsError> {
        let frames = input.length() as usize;
        Error::check_length(frames, left.length() as usize)?;
        Error::check_length(frames, right.length() as usize)?;
        self.check_frames(frames)?;
        let events = decode_events(events)?;
        input.copy_to(&mut self.input[..frames]);
        self.processor.process_planar(
            &self.input[..frames],
            &mut self.left[..frames],
            &mut self.right[..frames],
            &events.0,
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
        let events = decode_events(events)?;
        audio.copy_to(&mut self.input[..frames]);
        self.processor
            .process_mono_in_place(&mut self.input[..frames], &events.0)?;
        audio.copy_from(&self.input[..frames]);
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

/// Decodes the optional block controls before the audio state changes.
///
/// If there is no value, the result is an empty `Vec`, and no allocation
/// occurs. If the caller gives an array, the decode can allocate storage.
fn decode_events(events: Option<Ts<BlockEvents>>) -> Result<BlockEvents, JsError> {
    Ok(match events {
        Some(value) => value.to_rust()?,
        None => BlockEvents::default(),
    })
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
