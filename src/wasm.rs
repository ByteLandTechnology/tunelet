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
//! - `config` and [`default_config`] return complete objects.
//! - The constructor and `updateConfig` accept `DeepPartial<ProcessorConfig>`.
//! - Fixed-size arrays replace the full array.
//!
//! In the constructor and in `setConfig`, omitted or `undefined` fields use
//! their defaults. In `updateConfig`, omitted or `undefined` fields keep the
//! active values from `config`. A `null` field or a wrong field type throws a
//! JavaScript `Error` before the state changes.
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
    AlienConfig, ChordDegree, ChordName, CorrectionConfig, Error, Feedback, FormantConfig,
    HarmonyConfig, HarmonyScale, HarmonySource, Inversion, MultiVoiceMode, NoteMask, PitchClass,
    PitchConfig, PrepareConfig, Processor, ProcessorConfig, PunchConfig, ResynthesisConfig,
    ReverbConfig, ReverbMode, SampleRate, Telemetry, TimedEvent, WarmConfig, HARMONY_VOICES,
};
use js_sys::{
    Array, ArrayBuffer, Boolean, Date, Float32Array, Int16Array, Number, Reflect, RegExp, Symbol,
};
use serde::{de::DeserializeOwned, Deserialize, Deserializer, Serialize};
use tsify::{Ts, Tsify};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(typescript_custom_section)]
const DEEP_PARTIAL_DECL: &'static str = r#"/**
 * Makes each object property in `T` optional at all levels.
 *
 * Functions and fixed-size arrays or tuples stay complete.
 */
export type DeepPartial<T> = T extends (...args: never[]) => unknown
    ? T
    : T extends readonly unknown[]
        ? T
        : T extends object
            ? { [K in keyof T]?: DeepPartial<T[K]> }
            : T;
"#;

#[wasm_bindgen]
extern "C" {
    /// A partial processor configuration object from JavaScript.
    #[wasm_bindgen(typescript_type = "DeepPartial<ProcessorConfig>")]
    pub type PartialProcessorConfig;
}

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
    /// processor uses the default configuration. Omitted or `undefined` fields
    /// in `config` also use their defaults. The constructor validates the
    /// settings before it allocates the processor or the PCM buffers. In
    /// TypeScript, use `new Tunelet(sampleRate, maxBlockFrames, config)`.
    ///
    /// # Errors
    ///
    /// The constructor throws if one of these conditions occurs:
    ///
    /// - The rate is not 44100 or 48000.
    /// - The block limit is not an integer or is out of range.
    /// - The configuration object is malformed or a field is `null`.
    /// - The configuration is not valid.
    /// - The reservation of a PCM buffer fails.
    ///
    /// The available Wasm memory must also have sufficient space for the DSP
    /// histories.
    #[wasm_bindgen(constructor)]
    pub fn new(
        sample_rate: f64,
        max_block_frames: f64,
        config: Option<PartialProcessorConfig>,
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
        let mut resolved = ProcessorConfig::default();
        if let Some(patch) = config {
            merge_processor_config(&mut resolved, &patch)?;
        }
        let processor = Processor::new(preparation, resolved)?;
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
    /// A change to the object has no effect until `setConfig`, `updateConfig`
    /// or a timed event applies it.
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
    /// Omitted or `undefined` fields use their defaults. State retention and
    /// drain cancellation follow the native processor. The conversion ignores
    /// additional properties of the JavaScript object.
    ///
    /// # Errors
    ///
    /// The method throws if an object is not valid or a field is `null`.
    /// After an error, all processor state and any active drain stay the same.
    #[wasm_bindgen(js_name = setConfig)]
    pub fn set_config(&mut self, config: Ts<ProcessorConfig>) -> Result<(), JsError> {
        let mut next = ProcessorConfig::default();
        merge_processor_config(&mut next, &config.js_value())?;
        self.processor.set_config(next)?;
        Ok(())
    }

    /// Combines the defined fields of `patch` with the active configuration,
    /// validates all fields and applies the result.
    ///
    /// Omitted or `undefined` fields keep their active values from `config`.
    /// Fixed-size arrays replace the full array. State retention and drain
    /// cancellation follow the native processor. The conversion ignores
    /// additional properties of the JavaScript object.
    ///
    /// # Errors
    ///
    /// If `patch` is not an object, a field is `null` or a value is not valid,
    /// this method throws. After an error, all processor state and any active
    /// drain stay the same.
    #[wasm_bindgen(js_name = updateConfig)]
    pub fn update_config(&mut self, patch: PartialProcessorConfig) -> Result<(), JsError> {
        let mut next = self.processor.config();
        merge_processor_config(&mut next, &patch)?;
        self.processor.set_config(next)?;
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

/// Converts a JavaScript error from property access to a `JsError`.
fn js_error(error: JsValue) -> JsError {
    if let Some(err) = error.dyn_ref::<js_sys::Error>() {
        JsError::new(&String::from(err.message()))
    } else if let Some(text) = error.as_string() {
        JsError::new(&text)
    } else {
        JsError::new("failed to read configuration property")
    }
}

/// Rejects `null`, primitives, functions, arrays and non-record objects.
fn ensure_config_object(value: &JsValue) -> Result<(), JsError> {
    if value.is_null()
        || !value.is_object()
        || value.is_function()
        || Array::is_array(value)
        || ArrayBuffer::is_view(value)
        || value.is_instance_of::<ArrayBuffer>()
        || value.is_instance_of::<Date>()
        || value.is_instance_of::<RegExp>()
        || value.is_instance_of::<Number>()
        || value.is_instance_of::<Boolean>()
        || Reflect::has(value, &Symbol::iterator()).unwrap_or(true)
    {
        return Err(JsError::new("expected a configuration object"));
    }
    Ok(())
}

/// Reads one property and treats an omitted or `undefined` value as `None`.
fn get_property(object: &JsValue, key: &str) -> Result<Option<JsValue>, JsError> {
    let value = Reflect::get(object, &JsValue::from_str(key)).map_err(js_error)?;
    if value.is_undefined() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

/// If the property is defined, decodes one leaf field.
fn merge_field<T: DeserializeOwned>(
    object: &JsValue,
    key: &str,
    target: &mut T,
) -> Result<(), JsError> {
    if let Some(value) = get_property(object, key)? {
        *target = tsify::serde_wasm_bindgen::from_value(value)?;
    }
    Ok(())
}

/// If the property is defined, applies `merge` to a nested stage object.
fn merge_nested(
    object: &JsValue,
    key: &str,
    merge: impl FnOnce(&JsValue) -> Result<(), JsError>,
) -> Result<(), JsError> {
    if let Some(value) = get_property(object, key)? {
        ensure_config_object(&value)?;
        merge(&value)?;
    }
    Ok(())
}

/// Copies defined fields from a JavaScript patch onto `config`.
fn merge_processor_config(config: &mut ProcessorConfig, patch: &JsValue) -> Result<(), JsError> {
    ensure_config_object(patch)?;
    merge_nested(patch, "punch", |sub| {
        merge_punch_config(&mut config.punch, sub)
    })?;
    merge_nested(patch, "warm", |sub| {
        merge_warm_config(&mut config.warm, sub)
    })?;
    merge_nested(patch, "resynthesis", |sub| {
        merge_resynthesis_config(&mut config.resynthesis, sub)
    })?;
    merge_nested(patch, "alien", |sub| {
        merge_alien_config(&mut config.alien, sub)
    })?;
    merge_nested(patch, "reverb", |sub| {
        merge_reverb_config(&mut config.reverb, sub)
    })?;
    merge_field(patch, "outputGain", &mut config.output_gain)
}

fn merge_punch_config(config: &mut PunchConfig, patch: &JsValue) -> Result<(), JsError> {
    merge_field(patch, "enabled", &mut config.enabled)?;
    merge_field(patch, "gainDb", &mut config.gain_db)?;
    merge_field(patch, "impact", &mut config.impact)?;
    merge_field(patch, "ceilingDb", &mut config.ceiling_db)?;
    merge_field(patch, "attackMs", &mut config.attack_ms)
}

fn merge_warm_config(config: &mut WarmConfig, patch: &JsValue) -> Result<(), JsError> {
    merge_field(patch, "enabled", &mut config.enabled)?;
    merge_field(patch, "driveDb", &mut config.drive_db)
}

fn merge_alien_config(config: &mut AlienConfig, patch: &JsValue) -> Result<(), JsError> {
    merge_field(patch, "enabled", &mut config.enabled)?;
    merge_field(patch, "loopFrames", &mut config.loop_frames)
}

fn merge_reverb_config(config: &mut ReverbConfig, patch: &JsValue) -> Result<(), JsError> {
    merge_field(patch, "enabled", &mut config.enabled)?;
    merge_field(patch, "mode", &mut config.mode)?;
    merge_field(patch, "mix", &mut config.mix)?;
    merge_field(patch, "diffusion", &mut config.diffusion)?;
    merge_field(patch, "decay", &mut config.decay)?;
    merge_field(patch, "brightnessHz", &mut config.brightness_hz)
}

fn merge_pitch_config(config: &mut PitchConfig, patch: &JsValue) -> Result<(), JsError> {
    merge_field(patch, "threshold", &mut config.threshold)?;
    merge_field(patch, "minFrequencyHz", &mut config.min_frequency_hz)?;
    merge_field(patch, "maxFrequencyHz", &mut config.max_frequency_hz)
}

fn merge_correction_config(config: &mut CorrectionConfig, patch: &JsValue) -> Result<(), JsError> {
    merge_field(patch, "enabled", &mut config.enabled)?;
    merge_field(patch, "notes", &mut config.notes)?;
    merge_field(patch, "retuneMs", &mut config.retune_ms)
}

fn merge_formant_config(config: &mut FormantConfig, patch: &JsValue) -> Result<(), JsError> {
    merge_field(patch, "enabled", &mut config.enabled)?;
    merge_field(patch, "width", &mut config.width)?;
    merge_field(patch, "stretch", &mut config.stretch)?;
    merge_field(patch, "aspirationMix", &mut config.aspiration_mix)?;
    merge_field(
        patch,
        "aspirationCutoffHz",
        &mut config.aspiration_cutoff_hz,
    )
}

fn merge_harmony_config(config: &mut HarmonyConfig, patch: &JsValue) -> Result<(), JsError> {
    merge_field(patch, "source", &mut config.source)?;
    merge_field(patch, "key", &mut config.key)?;
    merge_field(patch, "scale", &mut config.scale)?;
    merge_field(patch, "chromaticIntervals", &mut config.chromatic_intervals)?;
    merge_field(patch, "diatonicIntervals", &mut config.diatonic_intervals)?;
    merge_field(patch, "chord", &mut config.chord)?;
    merge_field(patch, "degree", &mut config.degree)?;
    merge_field(patch, "inversion", &mut config.inversion)?;
    merge_field(patch, "registerPercent", &mut config.register_percent)?;
    merge_field(
        patch,
        "voicingSpreadPercent",
        &mut config.voicing_spread_percent,
    )?;
    merge_field(patch, "portamentoMs", &mut config.portamento_ms)?;
    merge_field(patch, "voices", &mut config.voices)
}

fn merge_resynthesis_config(
    config: &mut ResynthesisConfig,
    patch: &JsValue,
) -> Result<(), JsError> {
    merge_nested(patch, "pitch", |sub| {
        merge_pitch_config(&mut config.pitch, sub)
    })?;
    merge_nested(patch, "correction", |sub| {
        merge_correction_config(&mut config.correction, sub)
    })?;
    merge_field(patch, "pitchRatio", &mut config.pitch_ratio)?;
    merge_nested(patch, "formant", |sub| {
        merge_formant_config(&mut config.formant, sub)
    })?;
    merge_field(patch, "mode", &mut config.mode)?;
    merge_field(patch, "choirVoices", &mut config.choir_voices)?;
    merge_field(patch, "choirDetune", &mut config.choir_detune)?;
    merge_field(patch, "choirDelay", &mut config.choir_delay)?;
    merge_field(
        patch,
        "choirSpreadPercent",
        &mut config.choir_spread_percent,
    )?;
    merge_field(patch, "stereoOutput", &mut config.stereo_output)?;
    merge_nested(patch, "harmony", |sub| {
        merge_harmony_config(&mut config.harmony, sub)
    })?;
    merge_field(patch, "harmonyGain", &mut config.harmony_gain)?;
    merge_field(patch, "harmonicModulation", &mut config.harmonic_modulation)?;
    merge_field(patch, "dryMix", &mut config.dry_mix)?;
    merge_field(patch, "randomSeed", &mut config.random_seed)
}

// Remote serde definitions keep `serde(default)` out of the `Tsify` derive so
// `ProcessorConfig` and its stage types stay complete in `pkg/tunelet.d.ts`.

#[derive(Deserialize)]
#[serde(
    default = "ProcessorConfig::default",
    remote = "ProcessorConfig",
    rename_all = "camelCase"
)]
struct ProcessorConfigDef {
    punch: PunchConfig,
    warm: WarmConfig,
    resynthesis: ResynthesisConfig,
    alien: AlienConfig,
    reverb: ReverbConfig,
    output_gain: f32,
}

impl<'de> Deserialize<'de> for ProcessorConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        ProcessorConfigDef::deserialize(deserializer)
    }
}

#[derive(Deserialize)]
#[serde(
    default = "PunchConfig::default",
    remote = "PunchConfig",
    rename_all = "camelCase"
)]
struct PunchConfigDef {
    enabled: bool,
    gain_db: f32,
    impact: f32,
    ceiling_db: f32,
    attack_ms: f32,
}

impl<'de> Deserialize<'de> for PunchConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        PunchConfigDef::deserialize(deserializer)
    }
}

#[derive(Deserialize)]
#[serde(
    default = "WarmConfig::default",
    remote = "WarmConfig",
    rename_all = "camelCase"
)]
struct WarmConfigDef {
    enabled: bool,
    drive_db: f32,
}

impl<'de> Deserialize<'de> for WarmConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        WarmConfigDef::deserialize(deserializer)
    }
}

#[derive(Deserialize)]
#[serde(
    default = "AlienConfig::default",
    remote = "AlienConfig",
    rename_all = "camelCase"
)]
struct AlienConfigDef {
    enabled: bool,
    loop_frames: u32,
}

impl<'de> Deserialize<'de> for AlienConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        AlienConfigDef::deserialize(deserializer)
    }
}

#[derive(Deserialize)]
#[serde(
    default = "ReverbConfig::default",
    remote = "ReverbConfig",
    rename_all = "camelCase"
)]
struct ReverbConfigDef {
    enabled: bool,
    mode: ReverbMode,
    mix: f32,
    diffusion: f32,
    decay: f32,
    brightness_hz: f32,
}

impl<'de> Deserialize<'de> for ReverbConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        ReverbConfigDef::deserialize(deserializer)
    }
}

#[derive(Deserialize)]
#[serde(
    default = "PitchConfig::default",
    remote = "PitchConfig",
    rename_all = "camelCase"
)]
struct PitchConfigDef {
    threshold: f32,
    min_frequency_hz: f32,
    max_frequency_hz: f32,
}

impl<'de> Deserialize<'de> for PitchConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        PitchConfigDef::deserialize(deserializer)
    }
}

#[derive(Deserialize)]
#[serde(
    default = "CorrectionConfig::default",
    remote = "CorrectionConfig",
    rename_all = "camelCase"
)]
struct CorrectionConfigDef {
    enabled: bool,
    notes: NoteMask,
    retune_ms: f32,
}

impl<'de> Deserialize<'de> for CorrectionConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        CorrectionConfigDef::deserialize(deserializer)
    }
}

#[derive(Deserialize)]
#[serde(
    default = "FormantConfig::default",
    remote = "FormantConfig",
    rename_all = "camelCase"
)]
struct FormantConfigDef {
    enabled: bool,
    width: f32,
    stretch: f32,
    aspiration_mix: f32,
    aspiration_cutoff_hz: f32,
}

impl<'de> Deserialize<'de> for FormantConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        FormantConfigDef::deserialize(deserializer)
    }
}

#[derive(Deserialize)]
#[serde(
    default = "HarmonyConfig::default",
    remote = "HarmonyConfig",
    rename_all = "camelCase"
)]
struct HarmonyConfigDef {
    source: HarmonySource,
    key: PitchClass,
    scale: HarmonyScale,
    chromatic_intervals: [i8; HARMONY_VOICES],
    diatonic_intervals: [i8; HARMONY_VOICES],
    chord: ChordName,
    degree: ChordDegree,
    inversion: Inversion,
    register_percent: f32,
    voicing_spread_percent: f32,
    portamento_ms: f32,
    voices: u8,
}

impl<'de> Deserialize<'de> for HarmonyConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        HarmonyConfigDef::deserialize(deserializer)
    }
}

#[derive(Deserialize)]
#[serde(
    default = "ResynthesisConfig::default",
    remote = "ResynthesisConfig",
    rename_all = "camelCase"
)]
struct ResynthesisConfigDef {
    pitch: PitchConfig,
    correction: CorrectionConfig,
    pitch_ratio: f32,
    formant: FormantConfig,
    mode: MultiVoiceMode,
    choir_voices: u8,
    choir_detune: f32,
    choir_delay: f32,
    choir_spread_percent: f32,
    stereo_output: bool,
    harmony: HarmonyConfig,
    harmony_gain: f32,
    harmonic_modulation: f32,
    dry_mix: f32,
    random_seed: u64,
}

impl<'de> Deserialize<'de> for ResynthesisConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        ResynthesisConfigDef::deserialize(deserializer)
    }
}
