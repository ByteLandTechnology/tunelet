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
//! - Each object must contain all required fields.
//! - The optional harmony `voices` field defaults to 4.
//!
//! [`default_config`] gives a configuration to start from. Control events use
//! `{ type, value }`, and the reset control has no `value`. Scalar parameter
//! events nest `{ type, value }` inside `{ type: "parameter", value }`. MIDI
//! messages use `{ type, channel, note, velocity }`, with the fields that
//! apply to the selected variant. The TypeScript declarations come from the
//! same Rust types that validation uses.
//!
//! The class allocates the PCM storage at construction and uses it again for
//! each block. It also keeps an event buffer and uses that capacity again
//! across calls. These operations can allocate:
//!
//! - Event decode when a call gives more events than before, or when an event
//!   converts a stage configuration.
//! - The conversion of configurations.
//! - Feedback and telemetry objects.
//! - The buffers of the formant envelopes.
//! - Errors.
//!
//! The JavaScript glue can also make temporary views when it copies data. This
//! generic binding does not guarantee an AudioWorklet callback that is free of
//! allocation. If you do not need a processor any more, call the generated
//! `free()` method.

use crate::events::ValidatedEvents;
use crate::{
    ControlEvent, Error, Feedback, MidiEvent, ParameterControl, PrepareConfig, Processor,
    ProcessorConfig, SampleRate, Telemetry, TimedEvent,
};
use js_sys::{Array, Float32Array, Int16Array, Reflect};
use serde::{Deserialize, Serialize};
use tsify::{serde_wasm_bindgen, Ts, Tsify};
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
    /// The reusable buffer for block events. It becomes longer when a call
    /// gives more events, and it does not become shorter.
    events: Vec<TimedEvent>,
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
            events: Vec::new(),
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
    /// The dry path also has a delay of 1,593 frames. Resampled voices, choir
    /// delays and effects have their own timing. Thus, this value does not
    /// align all paths.
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
        self.processor.check_block(frames)?;
        let events = match events {
            Some(events) => decode_events(&mut self.events, events, frames)?,
            None => &[],
        };
        input.copy_to(&mut self.input[..frames]);
        self.processor.process_planar(
            &self.input[..frames],
            &mut self.left[..frames],
            &mut self.right[..frames],
            events,
        )?;
        left.copy_from(&self.left[..frames]);
        right.copy_from(&self.right[..frames]);
        Ok(())
    }

    /// Replaces the audio in a mono `Int16Array` with the stereo downmix.
    ///
    /// The timing, the events and the drain cancellation are the same as for
    /// `process`. Peak feedback and telemetry measure the mono signal after the
    /// downmix, as in the native processor.
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
        self.processor.check_block(frames)?;
        let events = match events {
            Some(events) => decode_events(&mut self.events, events, frames)?,
            None => &[],
        };
        audio.copy_to(&mut self.input[..frames]);
        self.processor
            .process_mono_in_place(&mut self.input[..frames], events)?;
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
    pub fn drain(&mut self, left: &Int16Array, right: &Int16Array) -> Result<usize, JsError> {
        let frames = left.length() as usize;
        Error::check_length(frames, right.length() as usize)?;
        self.processor.check_block(frames)?;
        let count = self
            .processor
            .drain_planar(&mut self.left[..frames], &mut self.right[..frames])?;
        left.copy_from(&self.left[..frames]);
        right.copy_from(&self.right[..frames]);
        Ok(count)
    }

    /// Renders a mono tail block and returns its frame count.
    ///
    /// The method fills the unused output with zeros. The downmix and the peak
    /// measurements are the same as for `processMono`. The return values, empty
    /// arrays and completion are the same as for `drain`.
    ///
    /// # Errors
    ///
    /// If the block is larger than `maxBlockFrames`, this method throws before
    /// the output or the state changes.
    #[wasm_bindgen(js_name = drainMono)]
    pub fn drain_mono(&mut self, audio: &Int16Array) -> Result<usize, JsError> {
        let frames = audio.length() as usize;
        self.processor.check_block(frames)?;
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

    /// Returns the feedback and clears only the peak window.
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

/// One cached `ParameterControl` discriminant and its constructor.
struct ParameterKey<T> {
    name: JsValue,
    field: &'static str,
    make: fn(T) -> ParameterControl,
}

impl<T> ParameterKey<T> {
    /// Allocates one cached discriminant key and stores its constructor.
    fn new(name: &'static str, field: &'static str, make: fn(T) -> ParameterControl) -> Self {
        Self {
            name: JsValue::from_str(name),
            field,
            make,
        }
    }
}

/// The cached property keys and variant names for JavaScript events.
struct EventKeys {
    offset: JsValue,
    event: JsValue,
    kind: JsValue,
    value: JsValue,
    channel: JsValue,
    note: JsValue,
    velocity: JsValue,
    midi: JsValue,
    correction_midi: JsValue,
    output_gain: JsValue,
    parameter: JsValue,
    reset: JsValue,
    configure: JsValue,
    resynthesis: JsValue,
    punch: JsValue,
    warm: JsValue,
    alien: JsValue,
    reverb: JsValue,
    note_on: JsValue,
    note_off: JsValue,
    all_notes_off: JsValue,
    f32_parameters: [ParameterKey<f32>; 25],
    u8_parameters: [ParameterKey<u8>; 2],
}

impl EventKeys {
    /// Allocates the property keys and variant names one time for this thread.
    fn new() -> Self {
        Self {
            offset: JsValue::from_str("offset"),
            event: JsValue::from_str("event"),
            kind: JsValue::from_str("type"),
            value: JsValue::from_str("value"),
            channel: JsValue::from_str("channel"),
            note: JsValue::from_str("note"),
            velocity: JsValue::from_str("velocity"),
            midi: JsValue::from_str("midi"),
            correction_midi: JsValue::from_str("correctionMidi"),
            output_gain: JsValue::from_str("outputGain"),
            parameter: JsValue::from_str("parameter"),
            reset: JsValue::from_str("reset"),
            configure: JsValue::from_str("configure"),
            resynthesis: JsValue::from_str("resynthesis"),
            punch: JsValue::from_str("punch"),
            warm: JsValue::from_str("warm"),
            alien: JsValue::from_str("alien"),
            reverb: JsValue::from_str("reverb"),
            note_on: JsValue::from_str("noteOn"),
            note_off: JsValue::from_str("noteOff"),
            all_notes_off: JsValue::from_str("allNotesOff"),
            f32_parameters: [
                ParameterKey::new(
                    "punchGainDb",
                    "punchGainDb value",
                    ParameterControl::PunchGainDb,
                ),
                ParameterKey::new(
                    "punchImpact",
                    "punchImpact value",
                    ParameterControl::PunchImpact,
                ),
                ParameterKey::new(
                    "punchCeilingDb",
                    "punchCeilingDb value",
                    ParameterControl::PunchCeilingDb,
                ),
                ParameterKey::new(
                    "punchAttackMs",
                    "punchAttackMs value",
                    ParameterControl::PunchAttackMs,
                ),
                ParameterKey::new(
                    "warmDriveDb",
                    "warmDriveDb value",
                    ParameterControl::WarmDriveDb,
                ),
                ParameterKey::new(
                    "pitchThreshold",
                    "pitchThreshold value",
                    ParameterControl::PitchThreshold,
                ),
                ParameterKey::new("retuneMs", "retuneMs value", ParameterControl::RetuneMs),
                ParameterKey::new(
                    "pitchRatio",
                    "pitchRatio value",
                    ParameterControl::PitchRatio,
                ),
                ParameterKey::new(
                    "formantWidth",
                    "formantWidth value",
                    ParameterControl::FormantWidth,
                ),
                ParameterKey::new(
                    "formantStretch",
                    "formantStretch value",
                    ParameterControl::FormantStretch,
                ),
                ParameterKey::new(
                    "formantAspirationMix",
                    "formantAspirationMix value",
                    ParameterControl::FormantAspirationMix,
                ),
                ParameterKey::new(
                    "formantAspirationCutoffHz",
                    "formantAspirationCutoffHz value",
                    ParameterControl::FormantAspirationCutoffHz,
                ),
                ParameterKey::new(
                    "choirDetune",
                    "choirDetune value",
                    ParameterControl::ChoirDetune,
                ),
                ParameterKey::new(
                    "choirDelay",
                    "choirDelay value",
                    ParameterControl::ChoirDelay,
                ),
                ParameterKey::new(
                    "choirSpreadPercent",
                    "choirSpreadPercent value",
                    ParameterControl::ChoirSpreadPercent,
                ),
                ParameterKey::new(
                    "harmonyRegisterPercent",
                    "harmonyRegisterPercent value",
                    ParameterControl::HarmonyRegisterPercent,
                ),
                ParameterKey::new(
                    "harmonyVoicingSpreadPercent",
                    "harmonyVoicingSpreadPercent value",
                    ParameterControl::HarmonyVoicingSpreadPercent,
                ),
                ParameterKey::new(
                    "harmonyPortamentoMs",
                    "harmonyPortamentoMs value",
                    ParameterControl::HarmonyPortamentoMs,
                ),
                ParameterKey::new(
                    "harmonyGain",
                    "harmonyGain value",
                    ParameterControl::HarmonyGain,
                ),
                ParameterKey::new(
                    "harmonicModulation",
                    "harmonicModulation value",
                    ParameterControl::HarmonicModulation,
                ),
                ParameterKey::new("dryMix", "dryMix value", ParameterControl::DryMix),
                ParameterKey::new("reverbMix", "reverbMix value", ParameterControl::ReverbMix),
                ParameterKey::new(
                    "reverbDiffusion",
                    "reverbDiffusion value",
                    ParameterControl::ReverbDiffusion,
                ),
                ParameterKey::new(
                    "reverbDecay",
                    "reverbDecay value",
                    ParameterControl::ReverbDecay,
                ),
                ParameterKey::new(
                    "reverbBrightnessHz",
                    "reverbBrightnessHz value",
                    ParameterControl::ReverbBrightnessHz,
                ),
            ],
            u8_parameters: [
                ParameterKey::new(
                    "choirVoices",
                    "choirVoices value",
                    ParameterControl::ChoirVoices,
                ),
                ParameterKey::new(
                    "harmonyVoices",
                    "harmonyVoices value",
                    ParameterControl::HarmonyVoices,
                ),
            ],
        }
    }
}

thread_local! {
    static EVENT_KEYS: EventKeys = EventKeys::new();
}

/// Decodes and validates the block controls before the audio or the state
/// changes.
///
/// The function clears `buffer` and uses its capacity again. Only a call with
/// more events than the current capacity reserves more space.
fn decode_events(
    buffer: &mut Vec<TimedEvent>,
    events: Ts<BlockEvents>,
    frames: usize,
) -> Result<&[TimedEvent], JsError> {
    let value = JsValue::from(events);
    let Some(array) = value.dyn_ref::<Array>() else {
        return Err(JsError::new("events must be an Array"));
    };
    buffer.clear();
    let count = array.length();
    if count > 0 {
        buffer.try_reserve(count as usize)?;
        EVENT_KEYS.with(|keys| -> Result<(), JsError> {
            for index in 0..count {
                buffer.push(decode_timed_event(&array.get(index), keys)?);
            }
            Ok(())
        })?;
    }
    ValidatedEvents::new(buffer, frames)?;
    Ok(buffer)
}

/// Decodes one JavaScript object into a `TimedEvent`.
fn decode_timed_event(item: &JsValue, keys: &EventKeys) -> Result<TimedEvent, JsError> {
    let offset = decode_offset(&get_property(item, &keys.offset)?)?;
    let event = decode_control_event(&get_property(item, &keys.event)?, keys)?;
    Ok(TimedEvent { offset, event })
}

/// Decodes one `ControlEvent` from its cached discriminant.
fn decode_control_event(event: &JsValue, keys: &EventKeys) -> Result<ControlEvent, JsError> {
    let kind = get_property(event, &keys.kind)?;
    if kind == keys.midi {
        let value = get_property(event, &keys.value)?;
        Ok(ControlEvent::Midi(decode_midi_event(&value, keys)?))
    } else if kind == keys.correction_midi {
        let value = get_property(event, &keys.value)?;
        Ok(ControlEvent::CorrectionMidi(decode_midi_event(
            &value, keys,
        )?))
    } else if kind == keys.output_gain {
        let value = get_property(event, &keys.value)?;
        Ok(ControlEvent::OutputGain(decode_f32(
            &value,
            "outputGain value",
        )?))
    } else if kind == keys.parameter {
        let value = get_property(event, &keys.value)?;
        Ok(ControlEvent::Parameter(decode_parameter_control(
            &value, keys,
        )?))
    } else if kind == keys.reset {
        let value = get_property(event, &keys.value)?;
        if !value.is_undefined() && !value.is_null() {
            return Err(JsError::new("reset event must not have a value"));
        }
        Ok(ControlEvent::Reset)
    } else if kind == keys.configure {
        let value = get_property(event, &keys.value)?;
        Ok(ControlEvent::Configure(serde_wasm_bindgen::from_value(
            value,
        )?))
    } else if kind == keys.resynthesis {
        let value = get_property(event, &keys.value)?;
        Ok(ControlEvent::Resynthesis(serde_wasm_bindgen::from_value(
            value,
        )?))
    } else if kind == keys.punch {
        let value = get_property(event, &keys.value)?;
        Ok(ControlEvent::Punch(serde_wasm_bindgen::from_value(value)?))
    } else if kind == keys.warm {
        let value = get_property(event, &keys.value)?;
        Ok(ControlEvent::Warm(serde_wasm_bindgen::from_value(value)?))
    } else if kind == keys.alien {
        let value = get_property(event, &keys.value)?;
        Ok(ControlEvent::Alien(serde_wasm_bindgen::from_value(value)?))
    } else if kind == keys.reverb {
        let value = get_property(event, &keys.value)?;
        Ok(ControlEvent::Reverb(serde_wasm_bindgen::from_value(value)?))
    } else {
        Err(JsError::new("event type is absent or unknown"))
    }
}

/// Decodes one `MidiEvent` object from its cached discriminant.
fn decode_midi_event(value: &JsValue, keys: &EventKeys) -> Result<MidiEvent, JsError> {
    let kind = get_property(value, &keys.kind)?;
    if kind == keys.note_on {
        let channel = decode_u8(&get_property(value, &keys.channel)?, "MIDI channel")?;
        let note = decode_u8(&get_property(value, &keys.note)?, "MIDI note")?;
        let velocity = decode_u8(&get_property(value, &keys.velocity)?, "MIDI velocity")?;
        Ok(MidiEvent::NoteOn {
            channel,
            note,
            velocity,
        })
    } else if kind == keys.note_off {
        let channel = decode_u8(&get_property(value, &keys.channel)?, "MIDI channel")?;
        let note = decode_u8(&get_property(value, &keys.note)?, "MIDI note")?;
        Ok(MidiEvent::NoteOff { channel, note })
    } else if kind == keys.all_notes_off {
        Ok(MidiEvent::AllNotesOff)
    } else {
        Err(JsError::new("MIDI event type is absent or unknown"))
    }
}

/// Decodes one `ParameterControl` object from its cached discriminant.
fn decode_parameter_control(
    value: &JsValue,
    keys: &EventKeys,
) -> Result<ParameterControl, JsError> {
    let kind = get_property(value, &keys.kind)?;
    for entry in &keys.f32_parameters {
        if kind == entry.name {
            let raw = get_property(value, &keys.value)?;
            return Ok((entry.make)(decode_f32(&raw, entry.field)?));
        }
    }
    for entry in &keys.u8_parameters {
        if kind == entry.name {
            let raw = get_property(value, &keys.value)?;
            return Ok((entry.make)(decode_u8(&raw, entry.field)?));
        }
    }
    Err(JsError::new("parameter event type is absent or unknown"))
}

/// Reads a property from a JavaScript object and converts any exception.
fn get_property(target: &JsValue, key: &JsValue) -> Result<JsValue, JsError> {
    Reflect::get(target, key).map_err(|err| match err.dyn_ref::<js_sys::Error>() {
        Some(error) => JsError::new(&String::from(error.message())),
        None => JsError::new("event must be an object"),
    })
}

/// Converts a JavaScript number into a valid wasm32 frame offset.
fn decode_offset(value: &JsValue) -> Result<usize, JsError> {
    let Some(number) = value.as_f64() else {
        return Err(JsError::new(
            "event offset must be an integer in 0..=4294967295",
        ));
    };
    if !(0.0..=f64::from(u32::MAX)).contains(&number) || number.fract() != 0.0 {
        return Err(JsError::new(
            "event offset must be an integer in 0..=4294967295",
        ));
    }
    Ok(number as usize)
}

/// Converts a JavaScript number into an `f32` scalar.
fn decode_f32(value: &JsValue, field: &str) -> Result<f32, JsError> {
    let Some(number) = value.as_f64() else {
        return Err(JsError::new(&format!("{field} must be a number")));
    };
    Ok(number as f32)
}

/// Converts a JavaScript number into a `u8` integer.
fn decode_u8(value: &JsValue, field: &str) -> Result<u8, JsError> {
    let Some(number) = value.as_f64() else {
        return Err(JsError::new(&format!(
            "{field} must be an integer in 0..=255"
        )));
    };
    if !(0.0..=255.0).contains(&number) || number.fract() != 0.0 {
        return Err(JsError::new(&format!(
            "{field} must be an integer in 0..=255"
        )));
    }
    Ok(number as u8)
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
