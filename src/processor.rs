//! The processor: audio blocks, timed controls, host feedback and the tail.

use crate::effects::alien::{self, Alien};
use crate::effects::punch::Punch;
use crate::effects::reverb::{self, Reverb};
use crate::effects::warm::Warm;
use crate::events::{ValidatedEvent, ValidatedEvents};
use crate::numeric::rng::RandomStream;
use crate::numeric::NumericContext;
use crate::resynthesis::{self, Resynthesis};
use crate::{
    ControlActivity, ControlEvent, Error, ParameterControl, PrepareConfig, ProcessorConfig,
    TimedEvent,
};

/// Selects the output channel layout for the downmix and the peak meters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputLayout {
    /// Keeps the left and right channels and measures stereo peaks.
    Stereo,
    /// Downmixes the two channels and measures mono peaks.
    Mono,
}

/// The formant state and query parameters for [`Processor::formant_envelope`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(all(feature = "wasm", target_arch = "wasm32")),
    expect(dead_code, reason = "only WasmProcessor reads this cache key")
)]
pub(crate) struct EnvelopeKey {
    /// The active flag, the bank epoch, the stamp fields and the Q14 width.
    stamp: (bool, u64, u8, u8, u16, i32),
    /// The IEEE 754 bits of the validated upper frequency in hertz.
    max_frequency_bits: u32,
    /// The validated number of frequency bins in each slice.
    points: usize,
}

/// A prepared vocal processor with state, mono input and stereo synthesis.
///
/// Construction fixes the sample rate and the block limit, and
/// allocates all histories. Use the process method that matches the buffer
/// layout of the host. Each input frame produces one output frame.
/// [`crate::ResynthesisConfig::stereo_output`] selects the voice panning
/// separately.
///
/// The audio, configuration, reset and drain calls need exclusive access to
/// this instance. Each processor owns its histories and numeric state, and
/// there is no global audio state. These prepared operations do not allocate,
/// do not lock and do no I/O. Public operations that can fail validate before
/// they change state or output. Refer to the [crate documentation](crate) for
/// the signal chain and the lifecycle.
pub struct Processor {
    /// The fixed rate and the block limit for each call.
    preparation: PrepareConfig,
    /// The settings that the host sees.
    ///
    /// The temporary aspiration mute of a drain is not stored here.
    config: ProcessorConfig,
    /// The input dynamics stage, which advances only while it is enabled.
    punch: Punch,
    /// The input saturation stage, which advances only while it is enabled.
    warm: Warm,
    /// The pitch tracking and voice histories, in a box to keep the handle
    /// small.
    resynthesis: Box<Resynthesis>,
    /// The reverse-loop history, which records also while its wet output is
    /// disabled.
    alien: Alien,
    /// The delay network and the damping filters.
    ///
    /// In bypass, their histories do not change.
    reverb: Reverb,
    /// The random sequence of this instance and the cached arctangent position.
    numeric: NumericContext,
    /// The largest absolute clipped output PCM sample in the feedback window.
    peak: i32,
    /// The largest absolute PCM input sample in the telemetry window, before
    /// Punch.
    input_peak: i32,
    /// The largest absolute clipped PCM sample of each output channel in the
    /// telemetry window.
    output_peaks: [i32; 2],
    /// The last ratio from correction only.
    ///
    /// It is zero before the first `process` call, after a reset or after a
    /// change of stereo output.
    last_resample_ratio: f32,
    /// The count of rendered frames since reset, with drain frames included.
    ///
    /// The count saturates at the maximum value.
    frames: u64,
    /// The number of frames of silent input that are left for the drain.
    ///
    /// `None` means that the drain is not active. `Some(0)` means that the
    /// drain is complete.
    drain_remaining: Option<usize>,
}

impl std::fmt::Debug for Processor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Processor")
            .field("prepare", &self.preparation)
            .field("config", &self.config)
            .field("processed_frames", &self.frames)
            .finish_non_exhaustive()
    }
}

impl Processor {
    /// Validates the settings, allocates histories and makes a new processor.
    ///
    /// Each processor is independent. `preparation` fixes the rate and the
    /// block limit for each call. After construction, the audio, control,
    /// feedback, reset and drain calls do not allocate, do not lock and do no
    /// I/O. Audio histories, pitch reports and meters start clear. The random
    /// stream starts from the low 32 bits of `random_seed`.
    ///
    /// Punch sets its initial drive from `config.punch.gain_db`. The initial
    /// note table of correction has the rate behavior that
    /// [`crate::CorrectionConfig`] describes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] if a stage field or the output gain is
    /// not valid. This check occurs before the allocation of the histories.
    /// The types of `preparation` already make sure that the block limit is
    /// not zero and that the sample rate is supported.
    pub fn new(preparation: PrepareConfig, config: ProcessorConfig) -> Result<Self, Error> {
        config.validate()?;
        let rate = preparation.sample_rate;
        Ok(Self {
            preparation,
            config,
            punch: Punch::new(rate, config.punch),
            warm: Warm::new(rate, config.warm),
            resynthesis: Box::new(Resynthesis::new(rate, config.resynthesis)),
            alien: Alien::new(config.alien),
            reverb: Reverb::new(rate, config.reverb),
            numeric: NumericContext::new(config.resynthesis.random_seed as u32),
            peak: 0,
            input_peak: 0,
            output_peaks: [0; 2],
            last_resample_ratio: 0.0,
            frames: 0,
            drain_remaining: None,
        })
    }

    /// Processes mono PCM into `[left, right]` stereo frames.
    ///
    /// The output must have the same number of frames as the input. Both must
    /// fit in [`PrepareConfig::max_block_frames`]. If the call succeeds, it
    /// writes all output frames. The layout is stereo also when voice panning
    /// is off. For interleaved PCM, `as_chunks_mut::<2>()` can show a buffer
    /// of even length as pairs.
    ///
    /// Each event applies before its frame. Offsets must be in the block and
    /// must not decrease. Events with equal offsets execute in slice order. An
    /// empty block accepts no events. A successful call cancels a drain, also
    /// if the call is empty.
    ///
    /// # Errors
    ///
    /// The call checks [`Error::BufferLength`] first and
    /// [`Error::BlockTooLarge`] second. Then it checks [`Error::InvalidEvent`]
    /// for the first invalid event. The validation of the full event slice
    /// finishes before the state or the output changes.
    pub fn process(
        &mut self,
        input: &[i16],
        output: &mut [[i16; 2]],
        events: &[TimedEvent],
    ) -> Result<(), Error> {
        Error::check_length(input.len(), output.len())?;
        self.process_into(input.len(), events, |processor, index| {
            output[index] = processor.next(input[index], OutputLayout::Stereo);
        })
    }

    /// Processes mono PCM into separate left and right output slices.
    ///
    /// The three slices must have equal frame counts. The audio, the event
    /// order, the behavior for empty blocks and the drain cancellation are the
    /// same as for [`process`](Self::process). If the call succeeds, it writes
    /// all output samples.
    ///
    /// # Errors
    ///
    /// The call checks [`Error::BufferLength`] for the left channel, then for
    /// the right channel. Next, it checks [`Error::BlockTooLarge`], then
    /// [`Error::InvalidEvent`]. If an error occurs, the two output buffers and
    /// all processor state do not change.
    pub fn process_planar(
        &mut self,
        input: &[i16],
        left: &mut [i16],
        right: &mut [i16],
        events: &[TimedEvent],
    ) -> Result<(), Error> {
        Error::check_length(input.len(), left.len())?;
        Error::check_length(input.len(), right.len())?;
        self.process_into(input.len(), events, |processor, index| {
            let frame = processor.next(input[index], OutputLayout::Stereo);
            left[index] = frame[0];
            right[index] = frame[1];
        })
    }

    /// Replaces mono PCM with the stereo result after the downmix.
    ///
    /// The method reads each input sample before it writes over that location.
    /// The downmix uses `(i32::from(left) + i32::from(right)) >> 1`, which
    /// rounds toward negative infinity. Panning and stereo effects run before
    /// this downmix. Peak feedback and output telemetry measure the mono
    /// samples after this downmix. Events and drain cancellation are the same
    /// as for [`process`](Self::process).
    ///
    /// # Errors
    ///
    /// Returns [`Error::BlockTooLarge`] for a slice that is too large. Returns
    /// [`Error::InvalidEvent`] for the first invalid event. All checks finish
    /// before the audio slice or the processor state changes.
    pub fn process_mono_in_place(
        &mut self,
        audio: &mut [i16],
        events: &[TimedEvent],
    ) -> Result<(), Error> {
        self.process_into(audio.len(), events, |processor, index| {
            let output = processor.next(audio[index], OutputLayout::Mono);
            audio[index] = output[0];
        })
    }

    /// Processes mono `f32` audio into `[left, right]` stereo frames.
    ///
    /// For each input sample, `NaN` becomes `0`, and other values clamp to
    /// `-1.0..=1.0`, multiply by `32767.0` and round to `-32767..=32767`. Each
    /// clipped stereo `i16` sample then divides by `32768.0`, so each output
    /// value is in `-32767.0 / 32768.0..=32767.0 / 32768.0`.
    ///
    /// The frame counts, the block limit, the events and the drain cancellation
    /// obey the same rules as in [`process`](Self::process). If the call
    /// succeeds, it writes all output frames.
    ///
    /// # Errors
    ///
    /// The call checks [`Error::BufferLength`] first and
    /// [`Error::BlockTooLarge`] second. Then it checks [`Error::InvalidEvent`]
    /// for the first invalid event. The validation of the full event slice
    /// finishes before the state or the output changes.
    pub fn process_f32(
        &mut self,
        input: &[f32],
        output: &mut [[f32; 2]],
        events: &[TimedEvent],
    ) -> Result<(), Error> {
        Error::check_length(input.len(), output.len())?;
        self.process_into(input.len(), events, |processor, index| {
            output[index] = processor
                .next(pcm_from_f32(input[index]), OutputLayout::Stereo)
                .map(pcm_to_f32);
        })
    }

    /// Processes mono `f32` audio into separate left and right `f32` slices.
    ///
    /// The three slices must have equal frame counts and fit in
    /// [`PrepareConfig::max_block_frames`]. Input quantization, the output
    /// range `-32767.0 / 32768.0..=32767.0 / 32768.0`, events and drain
    /// cancellation are the same as in [`process_f32`](Self::process_f32). If
    /// the call succeeds, it writes all output samples.
    ///
    /// # Errors
    ///
    /// The call checks [`Error::BufferLength`] for the left channel, then for
    /// the right channel. Next, it checks [`Error::BlockTooLarge`], then
    /// [`Error::InvalidEvent`]. If an error occurs, the two output buffers and
    /// all processor state do not change.
    pub fn process_planar_f32(
        &mut self,
        input: &[f32],
        left: &mut [f32],
        right: &mut [f32],
        events: &[TimedEvent],
    ) -> Result<(), Error> {
        Error::check_length(input.len(), left.len())?;
        Error::check_length(input.len(), right.len())?;
        self.process_into(input.len(), events, |processor, index| {
            let frame = processor.next(pcm_from_f32(input[index]), OutputLayout::Stereo);
            left[index] = pcm_to_f32(frame[0]);
            right[index] = pcm_to_f32(frame[1]);
        })
    }

    /// Replaces mono `f32` audio with the stereo result after the downmix.
    ///
    /// The method quantizes each input sample before it writes over that
    /// location, as in [`process_f32`](Self::process_f32). It divides the `i16`
    /// downmix of [`process_mono_in_place`](Self::process_mono_in_place) by
    /// `32768.0`, so each output sample is in
    /// `-32767.0 / 32768.0..=32767.0 / 32768.0`. Peak feedback and output
    /// telemetry measure the mono samples after this downmix. Events and drain
    /// cancellation are the same as for [`process`](Self::process).
    ///
    /// # Errors
    ///
    /// Returns [`Error::BlockTooLarge`] for a slice that is too large. Returns
    /// [`Error::InvalidEvent`] for the first invalid event. All checks finish
    /// before the audio slice or the processor state changes.
    pub fn process_mono_in_place_f32(
        &mut self,
        audio: &mut [f32],
        events: &[TimedEvent],
    ) -> Result<(), Error> {
        self.process_into(audio.len(), events, |processor, index| {
            let output = processor.next(pcm_from_f32(audio[index]), OutputLayout::Mono);
            audio[index] = pcm_to_f32(output[0]);
        })
    }

    /// Starts a tail with the current [`tail_frames`](Self::tail_frames)
    /// budget and returns that budget.
    ///
    /// Call this method one time after the last input block. Another call
    /// starts the full budget again, but does not clear histories. A
    /// successful process call, a successful configuration call or a reset
    /// cancels the drain.
    ///
    /// The drain uses silent input and mutes aspiration temporarily. Each
    /// drain call returns a frame count and fills the unused output with zeros.
    /// The call that uses all of the budget does a [`reset`](Self::reset),
    /// also if it returns frames. An empty buffer uses no budget. The budget
    /// can include silence. The reset at the end of the budget stops the
    /// fixed-point output that is left in the histories that it clears.
    pub fn begin_drain(&mut self) -> usize {
        let count = self.tail_frames();
        self.drain_remaining = Some(count);
        count
    }

    /// Renders the next stereo tail frames and returns their count.
    ///
    /// Call [`begin_drain`](Self::begin_drain) first. Before a drain starts or
    /// after it finishes, this method returns zero and clears the output. The
    /// method always fills unused frames with zeros. An empty buffer does not
    /// advance the drain.
    ///
    /// # Errors
    ///
    /// If the output is larger than the prepared block limit, the method
    /// returns [`Error::BlockTooLarge`] and does not change state or output.
    pub fn drain(&mut self, output: &mut [[i16; 2]]) -> Result<usize, Error> {
        let count = self.drain_into(output.len(), OutputLayout::Stereo, |index, frame| {
            output[index] = frame
        })?;
        output[count..].fill([0; 2]);
        Ok(count)
    }

    /// Renders the tail into separate output channels of equal size.
    ///
    /// The return values, the zeros in unused samples, empty buffers and
    /// completion are the same as for [`drain`](Self::drain). A final call can
    /// return frames and also do the reset in that call.
    ///
    /// # Errors
    ///
    /// The method checks [`Error::BufferLength`] before
    /// [`Error::BlockTooLarge`]. If either error occurs, the two channels and
    /// the drain state do not change.
    pub fn drain_planar(&mut self, left: &mut [i16], right: &mut [i16]) -> Result<usize, Error> {
        Error::check_length(left.len(), right.len())?;
        let count = self.drain_into(left.len(), OutputLayout::Stereo, |index, frame| {
            left[index] = frame[0];
            right[index] = frame[1];
        })?;
        left[count..].fill(0);
        right[count..].fill(0);
        Ok(count)
    }

    /// Renders the tail as mono PCM and returns the number of rendered frames.
    ///
    /// The downmix and the peak measurements are the same as in
    /// [`process_mono_in_place`](Self::process_mono_in_place). The method
    /// fills unused samples with zeros. If the drain is not active or is
    /// complete, the method returns zero. An empty output uses no budget. At
    /// completion, the method does a reset, as
    /// [`begin_drain`](Self::begin_drain) describes.
    ///
    /// # Errors
    ///
    /// If the output is larger than the prepared block limit, the method
    /// returns [`Error::BlockTooLarge`] before it changes output or state.
    pub fn drain_mono(&mut self, output: &mut [i16]) -> Result<usize, Error> {
        let count = self.drain_into(output.len(), OutputLayout::Mono, |index, frame| {
            output[index] = frame[0];
        })?;
        output[count..].fill(0);
        Ok(count)
    }

    /// Renders the next stereo tail frames as `f32` pairs and returns their
    /// count.
    ///
    /// Call [`begin_drain`](Self::begin_drain) first. Each rendered sample is
    /// in `-32767.0 / 32768.0..=32767.0 / 32768.0`, and unused frames get
    /// `0.0`. Return values, empty buffers and completion are the same as in
    /// [`drain`](Self::drain).
    ///
    /// # Errors
    ///
    /// If the output is larger than the prepared block limit, the method
    /// returns [`Error::BlockTooLarge`] and does not change state or output.
    pub fn drain_f32(&mut self, output: &mut [[f32; 2]]) -> Result<usize, Error> {
        let count = self.drain_into(output.len(), OutputLayout::Stereo, |index, frame| {
            output[index] = frame.map(pcm_to_f32);
        })?;
        output[count..].fill([0.0; 2]);
        Ok(count)
    }

    /// Renders the tail into separate `f32` output channels of equal size.
    ///
    /// Each rendered sample is in `-32767.0 / 32768.0..=32767.0 / 32768.0`,
    /// and unused samples get `0.0`. The return values, empty buffers and
    /// completion are the same as in [`drain_planar`](Self::drain_planar).
    ///
    /// # Errors
    ///
    /// The method checks [`Error::BufferLength`] before
    /// [`Error::BlockTooLarge`]. If either error occurs, the two channels and
    /// the drain state do not change.
    pub fn drain_planar_f32(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<usize, Error> {
        Error::check_length(left.len(), right.len())?;
        let count = self.drain_into(left.len(), OutputLayout::Stereo, |index, frame| {
            left[index] = pcm_to_f32(frame[0]);
            right[index] = pcm_to_f32(frame[1]);
        })?;
        left[count..].fill(0.0);
        right[count..].fill(0.0);
        Ok(count)
    }

    /// Renders the tail as mono `f32` audio and returns the number of rendered
    /// frames.
    ///
    /// The downmix, the peak measurements and the `1 / 32768` output scale are
    /// the same as in
    /// [`process_mono_in_place_f32`](Self::process_mono_in_place_f32). Unused
    /// samples get `0.0`. The return values, empty buffers and completion are
    /// the same as in [`drain_mono`](Self::drain_mono).
    ///
    /// # Errors
    ///
    /// If the output is larger than the prepared block limit, the method
    /// returns [`Error::BlockTooLarge`] before it changes output or state.
    pub fn drain_mono_f32(&mut self, output: &mut [f32]) -> Result<usize, Error> {
        let count = self.drain_into(output.len(), OutputLayout::Mono, |index, frame| {
            output[index] = pcm_to_f32(frame[0]);
        })?;
        output[count..].fill(0.0);
        Ok(count)
    }

    /// Returns the finite tail budget for the current configuration.
    ///
    /// This value is a configured limit. It is not the number of drain frames
    /// that are left, and it is not a measured decay time. It can include
    /// silence. It can also stop filter output that continues for a long
    /// time.
    pub fn tail_frames(&self) -> usize {
        self.resynthesis
            .tail_frames()
            .saturating_add(alien::tail_frames(self.config.alien))
            .saturating_add(reverb::tail_frames(
                self.preparation.sample_rate,
                self.config.reverb,
            ))
    }

    /// Returns the fixed sample rate and block limit.
    pub fn prepare_config(&self) -> PrepareConfig {
        self.preparation
    }

    /// Returns a copy of the current settings and does not change state.
    ///
    /// If you edit the copy, nothing changes until
    /// [`set_config`](Self::set_config) or a configuration event applies it.
    /// The temporary aspiration mute of a drain does not change the settings
    /// that this method returns.
    pub fn config(&self) -> ProcessorConfig {
        self.config
    }

    /// Returns which choir, harmony and reverb controls change the audio in
    /// the current configuration.
    ///
    /// A `false` flag means that the related control makes no sample change in
    /// the active voice mode, harmony source or reverb network. This method
    /// does not allocate memory and does not change state.
    pub fn control_activity(&self) -> ControlActivity {
        self.config.control_activity()
    }

    /// Returns the nominal resynthesis latency: 1,593 frames in
    /// [`crate::LatencyMode::Standard`] or 50 frames in
    /// [`crate::LatencyMode::LowLatency`] at both rates.
    ///
    /// The dry path also uses this delay. Resampled voices, choir delays and
    /// effects have their own timing. Thus, this value does not align all
    /// paths.
    pub fn latency_frames(&self) -> usize {
        resynthesis::latency_frames(self.config.resynthesis.latency_mode)
    }

    /// Returns the pitch, output peak and frame count, and does not clear them.
    ///
    /// To start a new peak window, use [`take_feedback`](Self::take_feedback).
    pub fn feedback(&self) -> Feedback {
        let estimate = self.resynthesis.estimate();
        Feedback {
            resample_ratio: self.last_resample_ratio,
            period_samples: estimate.period_samples,
            frequency_hz: estimate.frequency_hz,
            target_midi_note: self.resynthesis.target_note(),
            voiced: estimate.voiced,
            peak_level: self.peak as f32 / 32_768.0,
            processed_frames: self.frames,
            latency_frames: self.latency_frames(),
        }
    }

    /// Returns the current feedback and clears only the peak window.
    pub fn take_feedback(&mut self) -> Feedback {
        let result = self.feedback();
        self.peak = 0;
        result
    }

    /// Returns the pitch, the state of the voices, the levels and the effects,
    /// and does not clear the peak windows.
    ///
    /// The telemetry is for meters and displays. It does not change the
    /// audio. To start new peak windows, use
    /// [`take_telemetry`](Self::take_telemetry). The peak windows of the
    /// telemetry are independent of the peak window of [`Feedback`].
    pub fn telemetry(&self) -> Telemetry {
        let estimate = self.resynthesis.estimate();
        let mut voice_ratios = [0.0; 16];
        let mut voice_gains = [0.0; 16];
        let voice_count = self.resynthesis.voices(&mut voice_ratios, &mut voice_gains);
        let (alien_loop, alien_phase) = self.alien.loop_state();
        Telemetry {
            processed_frames: self.frames,
            input_peak: self.input_peak as f32 / 32_768.0,
            output_peak: self.output_peaks.map(|peak| peak as f32 / 32_768.0),
            reverb_wet_peak: self.reverb.wet_peak(),
            resample_ratio: self.last_resample_ratio,
            period_samples: estimate.period_samples,
            frequency_hz: estimate.frequency_hz,
            target_midi_note: self.resynthesis.target_note(),
            voiced: estimate.voiced,
            clarity: self.resynthesis.clarity(),
            tracking: self.resynthesis.is_tracking(),
            voice_count,
            voice_ratios,
            voice_gains,
            punch_gain: if self.config.punch.enabled {
                self.punch.effective_gain()
            } else {
                1.0
            },
            warm_gain: if self.config.warm.enabled {
                self.warm.normalization_gain()
            } else {
                1.0
            },
            alien_loop_frames: alien_loop as u32,
            alien_phase_frames: alien_phase as u32,
            formant_generation: self.resynthesis.formant_generation(),
        }
    }

    /// Returns the current telemetry and clears only its three peak windows.
    ///
    /// The peak window of [`Feedback`] and all audio histories do not change.
    pub fn take_telemetry(&mut self) -> Telemetry {
        let result = self.telemetry();
        self.clear_telemetry_peaks();
        result
    }

    /// Starts new windows for the input peak, the output peaks and the wet
    /// peak of the reverb.
    fn clear_telemetry_peaks(&mut self) {
        self.input_peak = 0;
        self.output_peaks = [0; 2];
        self.reverb.clear_wet_peak();
    }

    /// Writes the formant envelopes of the model that the audio used for the
    /// last frame.
    ///
    /// `input` gets the vocal-tract envelope that the LPC analysis found.
    /// `output` gets the envelope after the width and stretch controls. Point
    /// `k` of `n` is at the frequency `max_frequency_hz * k / (n - 1)`. The
    /// values are linear magnitudes. They have no absolute level, because the
    /// analysis normalizes the signal energy. Scale them for a display.
    ///
    /// `max_frequency_hz` must be finite, more than zero and not more than
    /// half of the sample rate. The two slices must have the same length.
    ///
    /// Returns `Ok(true)` if the envelopes are valid. Returns `Ok(false)` and
    /// fills the two slices with zeros in these cases:
    ///
    /// - [`crate::FormantConfig::enabled`] is `false`.
    /// - The current model has no synthesis filter. This occurs before the
    ///   first analysis and after an analysis that fails.
    ///
    /// The method does not allocate memory and does not change the audio.
    ///
    /// # Errors
    ///
    /// The method checks [`Error::BufferLength`] first, then
    /// [`Error::InvalidFrequency`]. On error, the two slices do not change.
    pub fn formant_envelope(
        &self,
        max_frequency_hz: f32,
        input: &mut [f32],
        output: &mut [f32],
    ) -> Result<bool, Error> {
        self.check_envelope(max_frequency_hz, input.len(), output.len())?;
        Ok(self
            .resynthesis
            .formant_envelope(max_frequency_hz, input, output))
    }

    /// Checks the slice lengths and the upper frequency for the formant
    /// envelopes.
    fn check_envelope(
        &self,
        max_frequency_hz: f32,
        input_len: usize,
        output_len: usize,
    ) -> Result<(), Error> {
        Error::check_length(input_len, output_len)?;
        let nyquist = self.preparation.sample_rate.hz() / 2.0;
        if !max_frequency_hz.is_finite() || max_frequency_hz <= 0.0 || max_frequency_hz > nyquist {
            return Err(Error::InvalidFrequency {
                hz: max_frequency_hz,
                max_hz: nyquist,
            });
        }
        Ok(())
    }

    /// Validates the arguments and returns the cache key for the current
    /// formant envelope.
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        expect(dead_code, reason = "only WasmProcessor calls this helper")
    )]
    pub(crate) fn envelope_key(
        &self,
        max_frequency_hz: f32,
        points: usize,
        output_len: usize,
    ) -> Result<EnvelopeKey, Error> {
        self.check_envelope(max_frequency_hz, points, output_len)?;
        Ok(EnvelopeKey {
            stamp: self.resynthesis.envelope_stamp(),
            max_frequency_bits: max_frequency_hz.to_bits(),
            points,
        })
    }

    /// Validates all configuration fields and applies them.
    ///
    /// The validation also examines the fields of disabled stages. A stage
    /// that does not change keeps its state. A change of `stereo_output` or
    /// `latency_mode` resets the resynthesis audio state, Alien, the reverb
    /// delays, the peak and the reported ratio. It also releases the harmony
    /// MIDI notes. It keeps Punch, Warm, the correction keys, the random
    /// stream, the reverb low-pass memories and the frame count. A change of
    /// `random_seed` starts the random stream again.
    ///
    /// Each successful call cancels a drain, also for an identical
    /// configuration. A change of reverb mode keeps the delay contents. If you
    /// disable Punch, Warm or reverb, the history of that stage stops. Alien
    /// continues to record. A change in either half of the `u64` seed starts
    /// the stream again, but the new sequence uses only the low 32 bits.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] if a field is not valid. The method
    /// returns this error before it changes any state, also an active drain.
    pub fn set_config(&mut self, config: ProcessorConfig) -> Result<(), Error> {
        config.validate()?;
        self.apply_config(config);
        Ok(())
    }

    /// Applies a configuration that the public setter or the preflight checked.
    fn apply_config(&mut self, config: ProcessorConfig) {
        if self.config.punch != config.punch {
            self.punch.set_config(config.punch);
        }
        if self.config.warm != config.warm {
            self.warm.set_config(config.warm);
        }
        if self.config.resynthesis != config.resynthesis {
            self.resynthesis.apply_config(config.resynthesis);
        }
        if self.config.resynthesis.stereo_output != config.resynthesis.stereo_output
            || self.config.resynthesis.latency_mode != config.resynthesis.latency_mode
        {
            // Resynthesis resets its histories if its stereo or latency mode
            // changes.
            self.reset_post_resynthesis();
        }
        if self.config.resynthesis.random_seed != config.resynthesis.random_seed {
            self.numeric.random = RandomStream::new(config.resynthesis.random_seed as u32);
        }
        if self.config.alien != config.alien {
            self.alien.set_config(config.alien);
        }
        if self.config.reverb != config.reverb {
            self.reverb.set_config(config.reverb);
        }
        self.config = config;
        self.drain_remaining = None;
    }

    /// Clears the audio histories, held notes, pitch feedback, peak and frame
    /// count.
    ///
    /// This method cancels an active drain. It also clears the correction
    /// ratio, the Punch envelopes, the Warm filters, the Alien recording and
    /// the reverb delays. It releases the harmony MIDI notes and, in
    /// MIDI-chord mode, the chord voices. It releases correction keys 0..=127
    /// and loads the configured correction notes again. It keeps these items:
    ///
    /// - the configuration
    /// - the random stream and the arctangent lookup position of Punch
    /// - the reverb low-pass memories and the reverb write position
    /// - the last usable period of the pitch tracker, which fills unvoiced
    ///   gaps
    /// - the harmony gain envelopes and the last voice ratios of the harmony
    ///   controller
    /// - the prepared chord targets of the named and degree chord sources
    /// - the choir drift
    /// - the control-step counter of Choir and Harmony
    /// - the phase of the harmonic modulation.
    ///
    /// For a fully new state, make a new processor.
    pub fn reset(&mut self) {
        self.punch.reset();
        self.warm.reset();
        self.resynthesis.reset();
        self.reset_post_resynthesis();
        self.frames = 0;
        self.drain_remaining = None;
    }

    /// Clears the stages, the peak windows and the ratio after resynthesis.
    ///
    /// A full reset, a stereo change or a latency-mode change clears this
    /// state. The frame count does not change here.
    fn reset_post_resynthesis(&mut self) {
        self.alien.reset();
        self.reverb.reset();
        self.peak = 0;
        self.clear_telemetry_peaks();
        self.last_resample_ratio = 0.0;
    }

    /// Checks the block size, then checks all events without a change.
    ///
    /// The returned cursor is the only source of validated control tokens.
    fn validate<'a>(
        &self,
        frames: usize,
        events: &'a [TimedEvent],
    ) -> Result<ValidatedEvents<'a>, Error> {
        self.check_block(frames)?;
        ValidatedEvents::new(events, frames, self.config)
    }

    /// Enforces the prepared block limit for each call.
    ///
    /// Zero frames are valid.
    pub(crate) fn check_block(&self, frames: usize) -> Result<(), Error> {
        let max = self.preparation.max_block_frames.get();
        if frames > max {
            return Err(Error::BlockTooLarge { frames, max });
        }
        Ok(())
    }

    /// Applies a validated control immediately before its frame is rendered.
    ///
    /// A stage event replaces one stage in the active configuration. A scalar
    /// parameter event updates one field and its quantized state. MIDI and
    /// reset events act directly. A reset does not discard later events in the
    /// block.
    fn apply(&mut self, event: ValidatedEvent) {
        match event.into_inner() {
            ControlEvent::Configure(value) => self.apply_config(value),
            ControlEvent::Resynthesis(value) => {
                let mut config = self.config;
                config.resynthesis = value;
                self.apply_config(config);
            }
            ControlEvent::Punch(value) => {
                let mut config = self.config;
                config.punch = value;
                self.apply_config(config);
            }
            ControlEvent::Warm(value) => {
                let mut config = self.config;
                config.warm = value;
                self.apply_config(config);
            }
            ControlEvent::Alien(value) => {
                let mut config = self.config;
                config.alien = value;
                self.apply_config(config);
            }
            ControlEvent::Reverb(value) => {
                let mut config = self.config;
                config.reverb = value;
                self.apply_config(config);
            }
            ControlEvent::OutputGain(value) => {
                self.config.output_gain = value;
                self.drain_remaining = None;
            }
            ControlEvent::Parameter(parameter) => self.apply_parameter(parameter),
            ControlEvent::Midi(event) => self.resynthesis.apply_midi(event),
            ControlEvent::CorrectionMidi(event) => self.resynthesis.apply_correction_midi(event),
            ControlEvent::Reset => self.reset(),
        }
    }

    /// Updates one validated scalar parameter, cancels an active drain and
    /// calls only the setter for that field.
    fn apply_parameter(&mut self, parameter: ParameterControl) {
        match parameter {
            ParameterControl::PunchGainDb(value) => {
                self.config.punch.gain_db = value;
                self.punch.set_gain_db(value);
            }
            ParameterControl::PunchImpact(value) => {
                self.config.punch.impact = value;
                self.punch.set_impact(value);
            }
            ParameterControl::PunchCeilingDb(value) => {
                self.config.punch.ceiling_db = value;
                self.punch.set_ceiling_db(value);
            }
            ParameterControl::PunchAttackMs(value) => {
                self.config.punch.attack_ms = value;
                self.punch.set_attack_ms(value);
            }
            ParameterControl::WarmDriveDb(value) => {
                self.config.warm.drive_db = value;
                self.warm.set_drive_db(value);
            }
            ParameterControl::PitchThreshold(value) => {
                self.config.resynthesis.pitch.threshold = value;
                self.resynthesis.set_pitch_threshold(value);
            }
            ParameterControl::RetuneMs(value) => {
                self.config.resynthesis.correction.retune_ms = value;
                self.resynthesis.set_retune_ms(value);
            }
            ParameterControl::Humanize(value) => {
                self.config.resynthesis.correction.humanize = value;
                self.resynthesis.set_humanize(value);
            }
            ParameterControl::FlexTune(value) => {
                self.config.resynthesis.correction.flex_tune = value;
                self.resynthesis.set_flex_tune(value);
            }
            ParameterControl::PitchRatio(value) => {
                self.config.resynthesis.pitch_ratio = value;
                self.resynthesis.set_pitch_ratio(value);
            }
            ParameterControl::FormantWidth(value) => {
                self.config.resynthesis.formant.width = value;
                self.resynthesis.set_formant_width(value);
            }
            ParameterControl::FormantStretch(value) => {
                self.config.resynthesis.formant.stretch = value;
                self.resynthesis.set_formant_stretch(value);
            }
            ParameterControl::FormantAspirationMix(value) => {
                self.config.resynthesis.formant.aspiration_mix = value;
                self.resynthesis.set_formant_aspiration_mix(value);
            }
            ParameterControl::FormantAspirationCutoffHz(value) => {
                self.config.resynthesis.formant.aspiration_cutoff_hz = value;
                self.resynthesis.set_formant_aspiration_cutoff_hz(value);
            }
            ParameterControl::ChoirVoices(value) => {
                self.config.resynthesis.choir_voices = value;
                self.resynthesis.set_choir_voices(value);
            }
            ParameterControl::ChoirDetune(value) => {
                self.config.resynthesis.choir_detune = value;
                self.resynthesis.set_choir_detune(value);
            }
            ParameterControl::ChoirDelay(value) => {
                self.config.resynthesis.choir_delay = value;
                self.resynthesis.set_choir_delay(value);
            }
            ParameterControl::ChoirSpreadPercent(value) => {
                self.config.resynthesis.choir_spread_percent = value;
                self.resynthesis.set_choir_spread_percent(value);
            }
            ParameterControl::HarmonyRegisterPercent(value) => {
                self.config.resynthesis.harmony.register_percent = value;
                self.resynthesis.set_harmony_register_percent(value);
            }
            ParameterControl::HarmonyVoicingSpreadPercent(value) => {
                self.config.resynthesis.harmony.voicing_spread_percent = value;
                self.resynthesis.set_harmony_voicing_spread_percent(value);
            }
            ParameterControl::HarmonyPortamentoMs(value) => {
                self.config.resynthesis.harmony.portamento_ms = value;
                self.resynthesis.set_harmony_portamento_ms(value);
            }
            ParameterControl::HarmonyVoices(value) => {
                self.config.resynthesis.harmony.voices = value;
                self.resynthesis.set_harmony_voices(value);
            }
            ParameterControl::HarmonyGain(value) => {
                self.config.resynthesis.harmony_gain = value;
                self.resynthesis.set_harmony_gain(value);
            }
            ParameterControl::HarmonicModulation(value) => {
                self.config.resynthesis.harmonic_modulation = value;
                self.resynthesis.set_harmonic_modulation(value);
            }
            ParameterControl::DryMix(value) => {
                self.config.resynthesis.dry_mix = value;
                self.resynthesis.set_dry_mix(value);
            }
            ParameterControl::ReverbMix(value) => {
                self.config.reverb.mix = value;
                self.reverb.set_mix(value);
            }
            ParameterControl::ReverbDiffusion(value) => {
                self.config.reverb.diffusion = value;
                self.reverb.set_diffusion(value);
            }
            ParameterControl::ReverbDecay(value) => {
                self.config.reverb.decay = value;
                self.reverb.set_decay(value);
            }
            ParameterControl::ReverbBrightnessHz(value) => {
                self.config.reverb.brightness_hz = value;
                self.reverb.set_brightness_hz(value);
            }
        }
        self.drain_remaining = None;
    }

    /// Renders one frame through the full chain and updates the peak meters.
    ///
    /// Resynthesis produces Q14 audio. Output gain is Q8. The gain product
    /// uses `i64` arithmetic before the symmetric `-32767..=32767` PCM clamp.
    /// In the mono layout, the downmix occurs before the peak measurement.
    fn next(&mut self, mut input: i16, layout: OutputLayout) -> [i16; 2] {
        self.input_peak = self.input_peak.max((input as i32).abs());
        if self.config.punch.enabled {
            input = self.punch.process_sample(input, &mut self.numeric);
        }
        if self.config.warm.enabled {
            input = self.warm.process_sample(input);
        }
        let voice = self.resynthesis.process_sample(input, &mut self.numeric);
        self.last_resample_ratio = self.resynthesis.resample_ratio();
        let alien = self.alien.process_sample(voice);
        let reverb = self.reverb.process_sample(alien, layout);
        let gain = (self.config.output_gain as f64 * 256.0) as i64;
        let mut result = reverb.map(|sample| {
            (((i64::from(sample.to_bits()) * gain) >> 8) << 1).clamp(-32767, 32767) as i16
        });
        if matches!(layout, OutputLayout::Mono) {
            let mono = downmix(result);
            result = [mono, mono];
        }
        self.peak = self
            .peak
            .max((result[0] as i32).abs())
            .max((result[1] as i32).abs());
        for (peak, sample) in self.output_peaks.iter_mut().zip(result) {
            *peak = (*peak).max((sample as i32).abs());
        }
        self.frames = self.frames.saturating_add(1);
        result
    }

    /// Checks the block, applies all controls at each offset and renders each
    /// frame.
    ///
    /// A callback reads the input sample and writes the output for its buffer
    /// layout.
    fn process_into(
        &mut self,
        frames: usize,
        events: &[TimedEvent],
        mut render: impl FnMut(&mut Self, usize),
    ) -> Result<(), Error> {
        let mut events = self.validate(frames, events)?;
        self.drain_remaining = None;
        for index in 0..frames {
            while let Some(event) = events.next_at(index) {
                self.apply(event);
            }
            render(self, index);
        }
        Ok(())
    }

    /// Renders only the budget that is left. The caller writes zeros in the
    /// unused frames.
    ///
    /// The method mutes aspiration temporarily, but does not change the
    /// settings that the host sees. It clears the mute before completion
    /// resets the state. `Some(0)` records completion, so later drain calls
    /// continue to return zero.
    fn drain_into(
        &mut self,
        frames: usize,
        layout: OutputLayout,
        mut write: impl FnMut(usize, [i16; 2]),
    ) -> Result<usize, Error> {
        self.check_block(frames)?;
        let count = self.drain_remaining.unwrap_or(0).min(frames);
        self.resynthesis.set_aspiration_muted(true);
        for index in 0..count {
            write(index, self.next(0, layout));
        }
        self.resynthesis.set_aspiration_muted(false);
        if let Some(remaining) = self.drain_remaining {
            let remaining = remaining - count;
            if remaining == 0 {
                self.reset();
            }
            self.drain_remaining = Some(remaining);
        }
        Ok(count)
    }
}

/// Mixes a stereo frame to mono and rounds toward negative infinity.
fn downmix(frame: [i16; 2]) -> i16 {
    ((i32::from(frame[0]) + i32::from(frame[1])) >> 1) as i16
}

/// Quantizes one `f32` sample to symmetric `-32767..=32767` PCM.
fn pcm_from_f32(sample: f32) -> i16 {
    if sample.is_nan() {
        return 0;
    }
    (f64::from(sample.clamp(-1.0, 1.0)) * 32_767.0).round() as i16
}

/// Divides one 16-bit PCM sample by `32768.0`.
fn pcm_to_f32(sample: i16) -> f32 {
    f32::from(sample) / 32_768.0
}

/// A snapshot of the pitch analysis and the output level that you can copy.
///
/// Get it with [`Processor::feedback`] or [`Processor::take_feedback`]. The
/// pitch fields describe the tracked input and the correction target. The
/// peak measures the final clipped output of each call. `Default` gives a
/// snapshot with all zeros and no target. It does not read the nominal
/// latency of a prepared processor.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase"),
    tsify(large_number_types_as_bigints)
)]
pub struct Feedback {
    /// The pitch-correction ratio from the last processed frame, where 1 is
    /// unity.
    ///
    /// It does not include the configured pitch shift or the ratios of
    /// individual voices. It is zero before processing, after a reset or after
    /// a change of stereo output or latency mode.
    pub resample_ratio: f32,
    /// The reported input pitch period in samples, with fractional samples.
    ///
    /// It is zero if the input is unvoiced or outside the reporting bounds of
    /// [`crate::PitchConfig`].
    pub period_samples: f32,
    /// The reported input pitch frequency in hertz.
    ///
    /// The value comes from the sample rate and the current tracked period.
    /// It is zero if the estimate is not reported.
    pub frequency_hz: f32,
    /// The correction target or the input pitch, as a MIDI note number.
    ///
    /// The value is the selected correction pitch class if all of these
    /// conditions are true:
    ///
    /// - Correction is enabled.
    /// - The active note table is not empty.
    /// - A tracking update selected a target after the last table change,
    ///   reset or tracking restart.
    ///
    /// The pitch class is in the octave nearest to the reported input. In
    /// other cases, the value is the input pitch as a fractional MIDI note. It
    /// is not the final pitch of shifted output or harmony output.
    ///
    /// The value is `None` if [`voiced`](Self::voiced) is `false`, and only
    /// then. This occurs if the input is unvoiced or if its frequency is
    /// outside the current reporting bounds. A target table that has no
    /// selected target does not give `None`.
    pub target_midi_note: Option<f32>,
    /// This field is `true` if the current pitch estimate is positive and
    /// inside the current reporting bounds.
    ///
    /// This is not an amplitude gate or a confidence score. A tracking result
    /// can stay between detector updates.
    pub voiced: bool,
    /// The largest absolute clipped output sample, divided by 32768.
    ///
    /// The peak window starts at construction, at a reset, at a change of
    /// stereo output or latency mode, or at a call to
    /// [`Processor::take_feedback`]. The value is in 0..=32767/32768. Mono
    /// calls measure the sample after the downmix. [`Processor::feedback`] does
    /// not clear the value.
    pub peak_level: f32,
    /// The number of frames processed since construction or the last reset,
    /// with drain frames included.
    ///
    /// The count saturates at `u64::MAX`. Drain completion resets it in the
    /// last drain call.
    pub processed_frames: u64,
    /// The nominal resynthesis latency, as [`Processor::latency_frames`] gives.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        serde(serialize_with = "crate::wasm::serialize_frame_count"),
        tsify(type = "number")
    )]
    pub latency_frames: usize,
}

/// A snapshot of the pitch, the voices, the levels and the effect state that
/// you can copy.
///
/// Get it with [`Processor::telemetry`] or [`Processor::take_telemetry`]. The
/// telemetry is for meters and displays. No audio calculation reads it.
/// `Default` gives a snapshot with all zeros and no target.
///
/// The three peak fields measure a window. The window starts at
/// construction, at a reset, at a change of stereo output or latency mode, or
/// at a call to [`Processor::take_telemetry`]. This window is independent of
/// the peak window of [`Feedback`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase"),
    tsify(large_number_types_as_bigints)
)]
pub struct Telemetry {
    /// The number of frames processed since construction or the last reset,
    /// as [`Feedback::processed_frames`] gives.
    pub processed_frames: u64,
    /// The largest absolute input sample in the window, divided by 32768.
    ///
    /// The measurement is before Punch and Warm. The value is in 0..=1.
    pub input_peak: f32,
    /// The largest absolute clipped sample of the left and the right output
    /// in the window, divided by 32768.
    ///
    /// The measurement is after the output gain. In mono calls, both entries
    /// measure the mono sample after the downmix. Each value is in
    /// 0..=32767/32768.
    pub output_peak: [f32; 2],
    /// The largest absolute wet contribution of the reverb in the window,
    /// where 1 is full scale.
    ///
    /// The contribution is the wet signal times the mix, before the output
    /// gain. Stereo calls measure the larger of the two channels. Mono calls
    /// measure the wet signal after the downmix. While the reverb is disabled,
    /// the window gets no new values.
    pub reverb_wet_peak: f32,
    /// The pitch-correction ratio from the last processed frame, where 1 is
    /// unity, as [`Feedback::resample_ratio`] gives.
    ///
    /// It does not include the configured pitch shift or the ratios of
    /// individual voices. It is zero before the first frame, after a reset or
    /// after a change of stereo output or latency mode.
    pub resample_ratio: f32,
    /// The reported input pitch period in samples, with fractional samples, as
    /// [`Feedback::period_samples`] gives.
    ///
    /// If [`voiced`](Self::voiced) is `false`, this value is zero. The bounds
    /// of [`crate::PitchConfig`] apply to this report.
    pub period_samples: f32,
    /// The reported input pitch frequency in hertz, as
    /// [`Feedback::frequency_hz`] gives.
    ///
    /// The value comes from the sample rate and
    /// [`period_samples`](Self::period_samples). If [`voiced`](Self::voiced)
    /// is `false`, this value is zero.
    pub frequency_hz: f32,
    /// The correction target or the input pitch, as a MIDI note number, as
    /// [`Feedback::target_midi_note`] gives.
    ///
    /// If correction selects a note from a non-empty table, the value is that
    /// pitch class in the nearest octave. In other cases, the value is the
    /// fractional MIDI note of the input. If [`voiced`](Self::voiced) is
    /// `false`, and only then, this value is `None`.
    pub target_midi_note: Option<f32>,
    /// This field is `true` if the current pitch estimate is positive and
    /// inside the bounds of [`crate::PitchConfig`].
    ///
    /// This is not an amplitude gate or a confidence score. A result can stay
    /// between detector updates.
    pub voiced: bool,
    /// How periodic the tracked input is, in 0..=1.
    ///
    /// The value is 1 for a signal that repeats exactly at the tracked
    /// period. It is 0 for a signal with no relation between cycles and
    /// during pitch acquisition. Use it as a confidence score for
    /// [`voiced`](Self::voiced). The tracker updates the related sums on each
    /// frame.
    pub clarity: f32,
    /// This field is `true` if the pitch tracker follows a period. It is
    /// `false` during pitch acquisition.
    pub tracking: bool,
    /// The number of valid entries in [`voice_ratios`](Self::voice_ratios)
    /// and [`voice_gains`](Self::voice_gains).
    ///
    /// The value is 5 in Harmony mode. In Choir mode, it is
    /// [`crate::ResynthesisConfig::choir_voices`], in 1..=16.
    pub voice_count: u8,
    /// The pitch ratio of each voice on the last frame, where 1 is the input
    /// pitch.
    ///
    /// Entry zero is the lead voice. In Harmony mode, entries 1 to 4 are the
    /// harmony voices. In Choir mode, the entries after the lead are the
    /// secondary voices. Each ratio includes the correction, the configured
    /// pitch shift and the ratio of the voice. The entries after
    /// [`voice_count`](Self::voice_count) are zero. In JavaScript, this field
    /// is a `Float32Array` of length `voiceCount`.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        serde(deserialize_with = "crate::wasm::deserialize_voice_array"),
        tsify(type = "Float32Array")
    )]
    pub voice_ratios: [f32; 16],
    /// The linear gain of each voice, in the order of
    /// [`voice_ratios`](Self::voice_ratios).
    ///
    /// The lead and the choir voices have a gain of 1. The gain of a harmony
    /// voice includes the MIDI velocity gain, in 0..=3.98095703125. MIDI
    /// velocity 127 gives the maximum, approximately +12 dB. The gain is zero
    /// for a voice that does not sound. It does not include the configured
    /// harmony level or the pan. The harmony controller updates it every 101
    /// frames. In JavaScript, this field is a `Float32Array` of length
    /// `voiceCount`.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        serde(deserialize_with = "crate::wasm::deserialize_voice_array"),
        tsify(type = "Float32Array")
    )]
    pub voice_gains: [f32; 16],
    /// The effective linear gain of Punch at the current envelope level.
    ///
    /// The value reflects the smoothed drive boost from the two input
    /// envelopes, the configured input gain and any attenuation from the
    /// output ceiling. Punch updates it on each enabled frame. The value is 1
    /// while Punch is disabled.
    pub punch_gain: f32,
    /// The linear gain that Warm applies before its saturation curve and
    /// removes after it.
    ///
    /// The value tracks the inverse of the input peak envelope, with +30 dB
    /// for input below -30 dB. Warm reduces it immediately on attacks, smooths
    /// it during release and updates it on each enabled frame. The value is 1
    /// while Warm is disabled.
    pub warm_gain: f32,
    /// The length of the Alien loop in frames, as
    /// [`crate::AlienConfig::loop_frames`] gives.
    pub alien_loop_frames: u32,
    /// The current frame in the Alien loop. It is less than
    /// [`alien_loop_frames`](Self::alien_loop_frames).
    ///
    /// The reverse reader is two times this number of frames behind the
    /// recording position. The value advances also while Alien is disabled.
    pub alien_phase_frames: u32,
    /// The wrapping count of formant analyses since construction.
    ///
    /// While the formant stage is enabled, the count increases every 441
    /// frames in standard mode and every 128 frames in low-latency mode.
    /// Compare two values to find if [`Processor::formant_envelope`] can give
    /// a new result.
    pub formant_generation: u32,
}

impl Telemetry {
    /// Returns the pitch ratios of the active voices, from `0` to
    /// [`voice_count`](Self::voice_count).
    ///
    /// If `voice_count` is more than 16, this method limits the slice length
    /// to 16.
    pub fn active_voice_ratios(&self) -> &[f32] {
        let count = usize::from(self.voice_count).min(self.voice_ratios.len());
        &self.voice_ratios[..count]
    }

    /// Returns the linear gains of the active voices, from `0` to
    /// [`voice_count`](Self::voice_count).
    ///
    /// If `voice_count` is more than 16, this method limits the slice length
    /// to 16.
    pub fn active_voice_gains(&self) -> &[f32] {
        let count = usize::from(self.voice_count).min(self.voice_gains.len());
        &self.voice_gains[..count]
    }
}
