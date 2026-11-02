//! The processor: audio blocks, timed controls, host feedback and the tail.

use crate::effects::alien::{self, Alien};
use crate::effects::punch::{Punch, PunchConfig};
use crate::effects::reverb::{self, Reverb};
use crate::effects::warm::Warm;
use crate::events::{ValidatedEvent, ValidatedEvents};
use crate::numeric::rng::RandomStream;
use crate::numeric::NumericContext;
use crate::resynthesis::{self, Resynthesis};
use crate::{ControlEvent, Error, PrepareConfig, ProcessorConfig, TimedEvent};

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
    /// The largest absolute clipped stereo PCM sample in the feedback window.
    peak: i32,
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
    /// Punch uses its default cached drive until the first window of 100
    /// frames with Punch enabled. The initial note table of correction has the
    /// rate behavior that [`crate::CorrectionConfig`] describes.
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
        // Drive uses the default gain until the first 100-frame peak window.
        let mut punch = Punch::new(rate, PunchConfig::default());
        punch.set_config(config.punch);
        Ok(Self {
            preparation,
            config,
            punch,
            warm: Warm::new(rate, config.warm),
            resynthesis: Box::new(Resynthesis::new(rate, config.resynthesis)),
            alien: Alien::new(config.alien),
            reverb: Reverb::new(rate, config.reverb),
            numeric: NumericContext::new(config.resynthesis.random_seed as u32),
            peak: 0,
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
        self.process_into(input, events, |index, frame| output[index] = frame)
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
        self.process_into(input, events, |index, frame| {
            left[index] = frame[0];
            right[index] = frame[1];
        })
    }

    /// Replaces mono PCM with the stereo result after the downmix.
    ///
    /// The method reads each input sample before it writes over that location.
    /// The downmix uses `(i32::from(left) + i32::from(right)) >> 1`, which
    /// rounds toward negative infinity. Panning and stereo effects run before
    /// this downmix. Peak feedback measures the stereo samples. Events and
    /// drain cancellation are the same as for [`process`](Self::process).
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
        let mut events = self.validate(audio, events)?;
        self.drain_remaining = None;
        for (index, sample) in audio.iter_mut().enumerate() {
            while let Some(event) = events.next_at(index) {
                self.apply(event);
            }
            let output = self.next(*sample);
            *sample = ((output[0] as i32 + output[1] as i32) >> 1) as i16;
        }
        Ok(())
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
        let count = self.drain_into(output.len(), |index, frame| output[index] = frame)?;
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
        let count = self.drain_into(left.len(), |index, frame| {
            left[index] = frame[0];
            right[index] = frame[1];
        })?;
        left[count..].fill(0);
        right[count..].fill(0);
        Ok(count)
    }

    /// Renders the tail as mono PCM and returns the number of rendered frames.
    ///
    /// The downmix is the same as in
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
        let count = self.drain_into(output.len(), |index, frame| {
            output[index] = ((i32::from(frame[0]) + i32::from(frame[1])) >> 1) as i16;
        })?;
        output[count..].fill(0);
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

    /// Returns the nominal resynthesis latency: 1,593 frames at both rates.
    ///
    /// The dry path has a delay of 1,543 frames. Resampled voices, choir delays
    /// and effects have their own timing. Thus, this value does not align all
    /// paths.
    pub fn latency_frames(&self) -> usize {
        resynthesis::LATENCY_FRAMES
    }

    /// Returns the pitch, stereo peak and frame count, and does not clear them.
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

    /// Validates all configuration fields and applies them.
    ///
    /// The validation also examines the fields of disabled stages. A stage
    /// that does not change keeps its state. A change of `stereo_output`
    /// resets the resynthesis audio state, Alien, the reverb delays, the peak
    /// and the reported ratio. It also releases the harmony MIDI notes. It
    /// keeps Punch, Warm, the correction keys, the random stream, the reverb
    /// low-pass memories and the frame count. A change of `random_seed` starts
    /// the random stream again.
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
        if self.config.resynthesis.stereo_output != config.resynthesis.stereo_output {
            // Resynthesis resets its histories if its stereo setting changes.
            self.alien.reset();
            self.reverb.reset();
            self.peak = 0;
            self.last_resample_ratio = 0.0;
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
    /// MIDI-chord mode, the chord voices. It keeps these items:
    ///
    /// - the configuration
    /// - the random stream and the arctangent lookup position of Punch
    /// - the reverb low-pass memories and the reverb write position
    /// - correction key 127 and the active correction note table
    /// - the last usable period of the pitch tracker, which fills unvoiced
    ///   gaps
    /// - the harmony gain envelopes and the last voice ratios of the harmony
    ///   controller
    /// - the prepared chord targets of the named and degree chord sources
    /// - the choir drift
    /// - the control-step counter of Choir and Harmony
    /// - the phase of the harmonic modulation.
    ///
    /// When the method clears the held keys, it does not load the configured
    /// scale again. For a fully new state, make a new processor.
    pub fn reset(&mut self) {
        self.punch.reset();
        self.warm.reset();
        self.resynthesis.reset();
        self.alien.reset();
        self.reverb.reset();
        self.peak = 0;
        self.frames = 0;
        self.last_resample_ratio = 0.0;
        self.drain_remaining = None;
    }

    /// Checks the block size, then checks all events without a change.
    ///
    /// The returned cursor is the only source of validated control tokens.
    fn validate<'a>(
        &self,
        input: &[i16],
        events: &'a [TimedEvent],
    ) -> Result<ValidatedEvents<'a>, Error> {
        self.check_block(input.len())?;
        ValidatedEvents::new(events, input.len())
    }

    /// Enforces the prepared block limit for each call.
    ///
    /// Zero frames are valid.
    fn check_block(&self, frames: usize) -> Result<(), Error> {
        let max = self.preparation.max_block_frames.get();
        if frames > max {
            return Err(Error::BlockTooLarge { frames, max });
        }
        Ok(())
    }

    /// Applies a validated control immediately before its frame is rendered.
    ///
    /// A partial configuration event replaces one stage in the active
    /// configuration. MIDI and reset events act directly. A reset does not
    /// discard later events in the block.
    fn apply(&mut self, event: ValidatedEvent) {
        let mut config = self.config;
        match event.into_inner() {
            ControlEvent::Configure(value) => config = value,
            ControlEvent::Resynthesis(value) => config.resynthesis = value,
            ControlEvent::Punch(value) => config.punch = value,
            ControlEvent::Warm(value) => config.warm = value,
            ControlEvent::Alien(value) => config.alien = value,
            ControlEvent::Reverb(value) => config.reverb = value,
            ControlEvent::OutputGain(value) => config.output_gain = value,
            ControlEvent::Midi(event) => {
                self.resynthesis.apply_midi(event);
                return;
            }
            ControlEvent::CorrectionMidi(event) => {
                self.resynthesis.apply_correction_midi(event);
                return;
            }
            ControlEvent::Reset => {
                self.reset();
                return;
            }
        }
        self.apply_config(config);
    }

    /// Renders one frame through the full chain and updates stereo feedback.
    ///
    /// Resynthesis produces Q14 audio. Output gain is Q8. The multiplication
    /// wraps before the shift. PCM conversion clips to the symmetric 16-bit
    /// range. This order and the peak measurement before the downmix are part
    /// of the output.
    fn next(&mut self, mut input: i16) -> [i16; 2] {
        if self.config.punch.enabled {
            input = self.punch.process_sample(input, &mut self.numeric);
        }
        if self.config.warm.enabled {
            input = self.warm.process_sample(input);
        }
        let voice = self.resynthesis.process_sample(input, &mut self.numeric);
        self.last_resample_ratio = self.resynthesis.resample_ratio();
        let alien = self.alien.process_sample(voice);
        let reverb = self.reverb.process_sample(alien);
        let gain = (self.config.output_gain as f64 * 256.0) as i32;
        let result = reverb.map(|sample| {
            (sample.to_bits().wrapping_mul(gain) >> 8)
                .wrapping_shl(1)
                .clamp(-32767, 32767) as i16
        });
        self.peak = self
            .peak
            .max((result[0] as i32).abs())
            .max((result[1] as i32).abs());
        self.frames = self.frames.saturating_add(1);
        result
    }

    /// Checks the block, applies all controls at each offset and writes each
    /// rendered frame.
    ///
    /// A callback for the buffer layout writes the frames.
    fn process_into(
        &mut self,
        input: &[i16],
        events: &[TimedEvent],
        mut write: impl FnMut(usize, [i16; 2]),
    ) -> Result<(), Error> {
        let mut events = self.validate(input, events)?;
        self.drain_remaining = None;
        for (index, sample) in input.iter().enumerate() {
            while let Some(event) = events.next_at(index) {
                self.apply(event);
            }
            write(index, self.next(*sample));
        }
        Ok(())
    }

    /// Renders only the budget that is left. The caller writes zeros in the
    /// unused frames.
    ///
    /// The method mutes aspiration temporarily, but does not change the
    /// settings that the host sees. It restores the configuration before
    /// completion resets the state. `Some(0)` records completion, so later
    /// drain calls continue to return zero.
    fn drain_into(
        &mut self,
        frames: usize,
        mut write: impl FnMut(usize, [i16; 2]),
    ) -> Result<usize, Error> {
        self.check_block(frames)?;
        let count = self.drain_remaining.unwrap_or(0).min(frames);
        let configured = self.config.resynthesis;
        if configured.formant.aspiration_mix != 0.0 {
            let mut silent = configured;
            silent.formant.aspiration_mix = 0.0;
            // Zero aspiration is valid in all validated configurations.
            self.resynthesis.apply_config(silent);
        }
        for index in 0..count {
            write(index, self.next(0));
        }
        if configured.formant.aspiration_mix != 0.0 {
            self.resynthesis.apply_config(configured);
        }
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

/// A snapshot of the pitch analysis and the output level that you can copy.
///
/// Get it with [`Processor::feedback`] or [`Processor::take_feedback`]. The
/// pitch fields describe the tracked input and the correction target. The
/// peak measures the final clipped stereo output. `Default` gives a snapshot
/// with all zeros and no target. It does not read the nominal latency of a
/// prepared processor.
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
    /// a change of stereo output.
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
    /// The largest absolute clipped stereo sample, divided by 32768.
    ///
    /// The peak window starts at construction, at a reset, at a change of
    /// stereo output or at a call to [`Processor::take_feedback`]. The value
    /// is in 0..=32767/32768. The mono downmix occurs after this measurement.
    /// [`Processor::feedback`] does not clear the value.
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
