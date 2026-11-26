//! LPC analysis that uses the pitch, changes to the vocal-tract shape, and
//! spectral synthesis.
//!
//! Pre-emphasis removes a simple glottal envelope from the input. Every 441
//! frames, the analysis makes a 50th-order predictor from a segment that is
//! aligned to the pitch. If no pitch is available, the segment is
//! approximately 10 ms. The correlations are normalized, weighted and
//! smoothed. Then they become reflection coefficients. For each voice bank
//! whose width or stretch is not 1, the code changes the reflection
//! coefficients into tract areas. Then it changes these areas into a
//! synthesis model for that voice.
//!
//! Thirty model banks keep the history of the coefficients for the lead voice
//! and for the four harmony voices. Each audio frame stores a pair of banks
//! and a blend. Thus, a fractional voice reader can find the model that
//! applies to the audio that it reads again. The inverse filter has a delay
//! of 441 frames. Resynthesis uses this delay to align each source sample
//! with the stamp of the frame that filtered it. Audio and coefficients use
//! Q14 words. All areas use one common scale that the code sets at runtime.
//! The order of the arithmetic and of the rescale operations changes the
//! output samples.

use crate::error::check_range;
use crate::filter::{Biquad, Design};
use crate::harmony::HARMONY_VOICES;
use crate::lpc;
use crate::numeric::fixed::{Q14, Q6};
use crate::numeric::{
    clamp_i32, db_to_gain, fast_sinc, mul14, ring, round_q14, NumericContext, HISTORY,
};
use crate::{Error, SampleRate};

/// The controls for vocal-tract changes and synthesized aspiration during
/// resynthesis.
///
/// This stage is disabled by default. If it is enabled, it does these steps:
/// 1. It estimates an LPC model.
/// 2. It removes the spectral envelope of the input.
/// 3. It applies a changed model to the resampled audio.
///
/// If width and stretch are 1, the model dimensions do not change. But the
/// fixed-point analysis and synthesis path does not give a bit-identical
/// copy of the input. Aspiration occurs only if the formant path is enabled.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub struct FormantConfig {
    /// Enables formant analysis and synthesis. The default is `false`.
    pub enabled: bool,
    /// The vocal-tract width factor, 0.5..=2. The default is 1.
    ///
    /// The square of the width scales the estimated tract areas. The width
    /// also scales the amplitude of the synthesized output.
    pub width: f32,
    /// The vocal-tract length factor, 0.5..=2. The default is 1.
    ///
    /// It controls the interpolation of the area model and thus the filter
    /// order.
    pub stretch: f32,
    /// The aspiration level, 0..=1, without a unit. The default is 0.
    ///
    /// If the value is 0.01 or less, the stage makes no noise. Active
    /// aspiration follows the signal amplitude. It uses the random stream of
    /// the processor, together with other stages. During each drain call,
    /// the processor sets the aspiration to zero. The configuration that the
    /// host sees does not change.
    pub aspiration_mix: f32,
    /// The high-pass cutoff for aspiration in hertz, 500..=10000. The default
    /// is 4000.
    ///
    /// The stage limits the cutoff to 0.48 × sample rate and truncates it to
    /// whole hertz. A change to this value updates the coefficients. The
    /// filter history does not change.
    pub aspiration_cutoff_hz: f32,
}
impl Default for FormantConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            width: 1.0,
            stretch: 1.0,
            aspiration_mix: 0.0,
            aspiration_cutoff_hz: 4000.0,
        }
    }
}
impl FormantConfig {
    /// Validates all fields, also if the formant stage is disabled.
    ///
    /// Each floating-point control must be finite and in its documented
    /// inclusive range. Validation does not clamp values. It does not change
    /// the state of the processor.
    ///
    /// # Errors
    ///
    /// Returns the first [`Error::InvalidConfig`], with the dotted path of the
    /// field and the problem.
    pub fn validate(self) -> Result<(), Error> {
        validate_width(self.width)?;
        validate_stretch(self.stretch)?;
        validate_aspiration_mix(self.aspiration_mix)?;
        validate_aspiration_cutoff_hz(self.aspiration_cutoff_hz)
    }
}

/// Checks the range of the vocal-tract width.
pub(crate) fn validate_width(width: f32) -> Result<(), Error> {
    check_range("resynthesis.formant.width", width, 0.5, 2.0)
}

/// Checks the range of the vocal-tract length.
pub(crate) fn validate_stretch(stretch: f32) -> Result<(), Error> {
    check_range("resynthesis.formant.stretch", stretch, 0.5, 2.0)
}

/// Checks the range of the aspiration mix.
pub(crate) fn validate_aspiration_mix(aspiration_mix: f32) -> Result<(), Error> {
    check_range(
        "resynthesis.formant.aspiration_mix",
        aspiration_mix,
        0.0,
        1.0,
    )
}

/// Checks the range of the aspiration high-pass cutoff.
pub(crate) fn validate_aspiration_cutoff_hz(aspiration_cutoff_hz: f32) -> Result<(), Error> {
    check_range(
        "resynthesis.formant.aspiration_cutoff_hz",
        aspiration_cutoff_hz,
        500.0,
        10000.0,
    )
}

/// The number of frames in each analysis step. The analysis output has a
/// delay of this number of frames.
pub(crate) const ANALYSIS_FRAMES: usize = 441;
/// The order of the input LPC predictor and of the synthesis model without
/// changes.
const ORDER: usize = 50;
/// The capacity for stretched models and synthesis feedback. A valid
/// stretch uses a maximum of 99.
const MAX_ORDER: usize = 110;
/// The number of coefficient banks in the ring. The stamps of the audio
/// history refer to these banks.
const BANKS: usize = 30;
/// The number of synthesis voice slots: the lead voice and the four harmony
/// voices.
const VOICE_BANKS: usize = HARMONY_VOICES + 1;

/// The model selection for one frame of input history.
///
/// Bank indices wrap independently of audio positions. `blend` is the Q14
/// weight of the current bank. The previous bank gets the rest of the weight.
#[derive(Clone, Copy, Debug)]
struct Stamp {
    /// The new model bank that was active when the related audio frame was
    /// recorded.
    current: u8,
    /// The previous bank, for the short transition between coefficients.
    previous: u8,
    /// The weight of the current bank in Q14, from zero to 16384 inclusive.
    blend: u16,
}
/// Keeps each stamp at four bytes in the history ring.
const _: () = assert!(size_of::<Stamp>() == 4);
impl Default for Stamp {
    fn default() -> Self {
        Self {
            current: 0,
            previous: 0,
            blend: 16384,
        }
    }
}
impl Stamp {
    /// Borrows a prefix of `current` from `banks` without a copy.
    ///
    /// If `allow_previous` is `true` and `blend` is less than 16384, the
    /// function returns `None`.
    #[inline]
    fn steady_bank<const LEN: usize>(
        self,
        banks: &[[i32; LEN]],
        allow_previous: bool,
        order: usize,
    ) -> Option<&[i32]> {
        if allow_previous && self.blend < 16384 {
            None
        } else {
            Some(&banks[usize::from(self.current)][..order])
        }
    }
    /// Blends `previous` and `current` from `banks` into `output`.
    ///
    /// Returns `output` with the saturated Q14 sum at each tap.
    fn blend_banks<'a, const LEN: usize>(
        self,
        banks: &[[i32; LEN]],
        output: &'a mut [i32],
    ) -> &'a [i32] {
        let current = usize::from(self.current);
        let previous = usize::from(self.previous);
        let blend = i32::from(self.blend);
        for (i, value) in output.iter_mut().enumerate() {
            *value = mul14(16384 - blend, banks[previous][i])
                .saturating_add(mul14(blend, banks[current][i]));
        }
        output
    }
    /// Borrows the active synthesis bank from `models` without a copy.
    ///
    /// If `blend` is less than 16384 and the previous bank has a nonzero
    /// order, the function returns `None`.
    #[inline]
    fn synthesis_bank<'a>(
        self,
        models: &'a [[i32; MAX_ORDER]],
        orders: &[usize; BANKS],
    ) -> Option<&'a [i32]> {
        let current = usize::from(self.current);
        let previous = usize::from(self.previous);
        self.steady_bank(models, orders[previous] != 0, orders[current])
    }
    /// Blends the two synthesis banks into `output` and returns the active
    /// prefix.
    fn blend_synthesis<'a>(
        self,
        models: &[[i32; MAX_ORDER]],
        orders: &[usize; BANKS],
        output: &'a mut [i32; MAX_ORDER],
    ) -> &'a [i32] {
        let order = orders[usize::from(self.current)].max(orders[usize::from(self.previous)]);
        self.blend_banks(models, &mut output[..order])
    }
}

/// The formant stage. It holds preallocated analysis memory, the model
/// histories and the synthesis state for five voices.
#[derive(Debug)]
pub(crate) struct Formant {
    /// The validated formant controls. The resynthesis caller decides if this
    /// stage runs.
    config: FormantConfig,
    /// The validated vocal-tract width multipliers of the four harmony voices.
    voice_width: [f32; HARMONY_VOICES],
    /// The validated vocal-tract length multipliers of the four harmony
    /// voices.
    voice_stretch: [f32; HARMONY_VOICES],
    /// The sample rate for analysis windows, glottal decay and the aspiration
    /// filter.
    rate: SampleRate,
    /// The reverse-order ring of pre-emphasized Q14 audio words, with a tail
    /// copy of `ORDER` entries.
    input: Vec<i32>,
    /// The preallocated segment memory for period folding and
    /// autocorrelation.
    temporary: Vec<i32>,
    /// The Q14 coefficient banks for the inverse filter.
    inverse: Vec<[i32; ORDER]>,
    /// The Q14 coefficient banks for the five forward synthesis voices.
    /// `orders` holds the active lengths.
    models: Vec<[[i32; MAX_ORDER]; BANKS]>,
    /// The valid synthesis order for each voice slot and bank. Zero means
    /// that there is no feedback model.
    orders: [[usize; BANKS]; VOICE_BANKS],
    /// One record of a bank pair and a blend for each frame of input history.
    stamps: Vec<Stamp>,
    /// The active pair of coefficient banks and the Q14 weight of the blend.
    /// Each input frame moves the weight toward 1.0.
    stamp: Stamp,
    /// A counter that increases from a negative value to zero. At zero, the
    /// stage tries to make a model.
    design_counter: i32,
    /// The separate Q14 feedback rings for the lead voice and the four
    /// harmony voices. Each ring holds two copies of `MAX_ORDER` entries.
    history: [[i32; MAX_ORDER * 2]; VOICE_BANKS],
    /// The start slot in the synthesis history of each voice.
    history_index: [usize; VOICE_BANKS],
    /// Marks each voice history that holds nonzero synthesis feedback.
    history_dirty: [bool; VOICE_BANKS],
    /// The two previous normalized autocorrelation vectors for the moving
    /// average.
    correlation_history: [[i32; 111]; 2],
    /// The wrapped sum of the stored correlation vectors, each divided by two.
    correlation_sum: [i32; 111],
    /// The slot in the correlation history that the next vector replaces.
    smooth_index: usize,
    /// The number of valid correlation vectors, from zero to two.
    smooth_filled: usize,
    /// If `true`, the next call to `smooth` clears the state of the
    /// correlation average.
    smooth_reset: bool,
    /// If `true`, the stage changed state since construction or the last reset.
    dirty: bool,
    /// The Q14 first-order coefficient for pre-emphasis and de-emphasis.
    glottal: i32,
    /// The Q14 width control of each voice. The stage also uses it as the
    /// output gain of that forward filter.
    width: [i32; VOICE_BANKS],
    /// The Q14 square of the width of each voice, for the scale of the tract
    /// areas.
    width_squared: [i32; VOICE_BANKS],
    /// The Q14 stretch control of each voice. The stage uses it to decide if
    /// a change to the areas is necessary.
    stretch: [i32; VOICE_BANKS],
    /// The number of active coefficients in each stretched model, not more
    /// than MAX_ORDER.
    stretch_count: [usize; VOICE_BANKS],
    /// The Q7 positions that map the stretched sections of each voice into
    /// the area model with 50 sections.
    stretch_map: [[i32; MAX_ORDER]; VOICE_BANKS],
    /// The original tract areas, with the runtime scale that the LPC
    /// conversion returns.
    areas: [i32; MAX_ORDER],
    /// The interpolated areas, scaled by the width. They use the same binary
    /// scale as the original areas.
    stretched_areas: [i32; MAX_ORDER],
    /// The high-pass filter for the random aspiration that the signal
    /// modulates.
    noise_filter: Biquad,
    /// The Q14 wet mix of the aspiration, from the configured level.
    noise_gain: i32,
    /// The Q14 compensation, from the aspiration cutoff divided by the
    /// sample rate.
    noise_amplitude: i32,
    /// If `true`, the stage mutes aspiration temporarily for a drain.
    aspiration_muted: bool,
    /// The wrapping count of model designs that the stage tried. Only the
    /// telemetry reads it.
    generation: u32,
    /// The count of bank advances from model designs and resets.
    bank_epoch: u64,
}

/// Multiplies `coefficients` with `samples` in tap order and returns the
/// exact `i64` sum.
#[inline]
fn tap_sum(samples: &[i32], coefficients: &[i32]) -> i64 {
    samples
        .iter()
        .zip(coefficients)
        .map(|(&sample, &coefficient)| i64::from(sample) * i64::from(coefficient))
        .sum()
}

impl Formant {
    /// Creates the stage, allocates the memory for audio, models and
    /// intermediate values, and prepares the controls for the sample rate.
    ///
    /// At the supported rates, the capacity of the temporary segment holds
    /// the longest analysis segment and the predictor padding. Later calls
    /// use this memory again.
    pub(crate) fn new(
        rate: SampleRate,
        config: FormantConfig,
        voice_width: [f32; HARMONY_VOICES],
        voice_stretch: [f32; HARMONY_VOICES],
    ) -> Self {
        let d1 = -std::f64::consts::LN_2 / 0.9_f64.ln();
        let d2 = rate.hz() as f64 * d1 / 44100.0;
        let mut result = Self {
            config,
            voice_width,
            voice_stretch,
            rate,
            input: vec![0; HISTORY + ORDER],
            temporary: vec![0_i32; (2 * (rate.as_u32() as usize / 40) + MAX_ORDER + 256).max(2865)],
            inverse: vec![[0; ORDER]; BANKS],
            models: vec![[[0; MAX_ORDER]; BANKS]; VOICE_BANKS],
            orders: [[0; BANKS]; VOICE_BANKS],
            stamps: vec![Stamp::default(); HISTORY],
            stamp: Stamp {
                current: 1,
                previous: 0,
                blend: 0,
            },
            design_counter: 0,
            history: [[0; MAX_ORDER * 2]; VOICE_BANKS],
            history_index: [0; VOICE_BANKS],
            history_dirty: [false; VOICE_BANKS],
            correlation_history: [[0; 111]; 2],
            correlation_sum: [0; 111],
            smooth_index: 0,
            smooth_filled: 0,
            smooth_reset: true,
            dirty: false,
            glottal: (0.5_f64.powf(1.0 / d2) * 16384.0) as i32,
            width: [16384; VOICE_BANKS],
            width_squared: [16384; VOICE_BANKS],
            stretch: [16384; VOICE_BANKS],
            stretch_count: [ORDER; VOICE_BANKS],
            stretch_map: [[0; MAX_ORDER]; VOICE_BANKS],
            areas: [0; MAX_ORDER],
            stretched_areas: [0; MAX_ORDER],
            noise_filter: Biquad::new(),
            noise_gain: 0_i32,
            noise_amplitude: 0_i32,
            aspiration_muted: false,
            generation: 0,
            bank_epoch: 0,
        };
        result.configure_all();
        result
    }
    /// Sets the temporary aspiration mute for a drain without a change to the
    /// configuration.
    pub(crate) fn set_aspiration_muted(&mut self, muted: bool) {
        self.aspiration_muted = muted;
    }
    /// Quantizes the vocal-tract width and its square for one voice slot.
    fn configure_slot_width(&mut self, slot: usize, width: f64) {
        self.width[slot] = (width * 16384.0) as i32;
        self.width_squared[slot] = (width * width * 16384.0) as i32;
    }
    /// Quantizes the stretch factor and builds the Q7 area map for one voice
    /// slot.
    ///
    /// A valid stretch gives a model with 25 to 99 entries. Thus, the last
    /// point of each map and the denominators of the interpolation are always
    /// defined.
    fn configure_slot_stretch(&mut self, slot: usize, stretch: f64) {
        self.stretch[slot] = (stretch * 16384.0) as i32;
        let count = (((ORDER - 1) as f64 * stretch + 1.001) as usize).min(MAX_ORDER);
        self.stretch_count[slot] = count;
        for (i, entry) in self.stretch_map[slot][..count - 1].iter_mut().enumerate() {
            let fraction = i as f64 / (count - 1) as f64;
            *entry = ((ORDER - 1) as f64 * fraction * 128.0) as i32;
        }
        self.stretch_map[slot][count - 1] = ((ORDER - 1) * 128) as i32;
    }
    /// Quantizes the vocal-tract width and its square for all five voice slots
    /// unconditionally.
    fn configure_width(&mut self) {
        let lead_width = f64::from(self.config.width);
        self.configure_slot_width(0, lead_width);
        for voice in 0..HARMONY_VOICES {
            let width = (lead_width * f64::from(self.voice_width[voice])).clamp(0.5, 2.0);
            self.configure_slot_width(voice + 1, width);
        }
    }
    /// Quantizes the stretch factor and prepares the Q7 interpolation maps for
    /// all five voice slots unconditionally.
    fn configure_stretch(&mut self) {
        let lead_stretch = f64::from(self.config.stretch);
        self.configure_slot_stretch(0, lead_stretch);
        for voice in 0..HARMONY_VOICES {
            let stretch = (lead_stretch * f64::from(self.voice_stretch[voice])).clamp(0.5, 2.0);
            self.configure_slot_stretch(voice + 1, stretch);
        }
    }
    /// Quantizes the aspiration wet mix unconditionally.
    fn configure_aspiration_mix(&mut self) {
        self.noise_gain = (self.config.aspiration_mix as f64 * 16384.0) as i32;
    }
    /// Updates the compensation and high-pass filter for aspiration
    /// unconditionally.
    fn configure_aspiration_cutoff_hz(&mut self) {
        let frequency = (self.config.aspiration_cutoff_hz as i32)
            .min((self.rate.hz() as f64 * 0.48) as i32)
            .max(500);
        self.noise_amplitude = (frequency as f64 * 0.8 / self.rate.hz() as f64 * 16384.0) as i32;
        self.noise_filter
            .set_design(Design::high_pass(self.rate.hz(), frequency as f32, 0.707));
    }
    /// Initializes all configurable controls and filter coefficients
    /// unconditionally.
    fn configure_all(&mut self) {
        self.configure_width();
        self.configure_stretch();
        self.configure_aspiration_mix();
        self.configure_aspiration_cutoff_hz();
    }
    /// Updates the quantized controls, the Q7 area maps and the aspiration
    /// filter for the fields that changed.
    ///
    /// The histories do not change.
    pub(crate) fn set_config(
        &mut self,
        config: FormantConfig,
        voice_width: [f32; HARMONY_VOICES],
        voice_stretch: [f32; HARMONY_VOICES],
    ) {
        let old = self.config;
        let old_voice_width = self.voice_width;
        let old_voice_stretch = self.voice_stretch;
        self.config = config;
        self.voice_width = voice_width;
        self.voice_stretch = voice_stretch;
        if old.width != config.width || old_voice_width != voice_width {
            self.configure_width();
        }
        if old.stretch != config.stretch || old_voice_stretch != voice_stretch {
            self.configure_stretch();
        }
        if old.aspiration_mix != config.aspiration_mix {
            self.configure_aspiration_mix();
        }
        if old.aspiration_cutoff_hz != config.aspiration_cutoff_hz {
            self.configure_aspiration_cutoff_hz();
        }
    }
    /// Stores the vocal-tract width. If the value changed, quantizes all five
    /// voice slots again.
    pub(crate) fn set_width(&mut self, width: f32) {
        let changed = self.config.width != width;
        self.config.width = width;
        if changed {
            self.configure_width();
        }
    }
    /// Stores the vocal-tract stretch. If the value changed, prepares the area
    /// maps for all five voice slots again.
    pub(crate) fn set_stretch(&mut self, stretch: f32) {
        let changed = self.config.stretch != stretch;
        self.config.stretch = stretch;
        if changed {
            self.configure_stretch();
        }
    }
    /// Stores the aspiration mix. If the value changed, quantizes it again.
    pub(crate) fn set_aspiration_mix(&mut self, aspiration_mix: f32) {
        let changed = self.config.aspiration_mix != aspiration_mix;
        self.config.aspiration_mix = aspiration_mix;
        if changed {
            self.configure_aspiration_mix();
        }
    }
    /// Stores the aspiration cutoff. If the value changed, updates the
    /// high-pass filter.
    pub(crate) fn set_aspiration_cutoff_hz(&mut self, aspiration_cutoff_hz: f32) {
        let changed = self.config.aspiration_cutoff_hz != aspiration_cutoff_hz;
        self.config.aspiration_cutoff_hz = aspiration_cutoff_hz;
        if changed {
            self.configure_aspiration_cutoff_hz();
        }
    }
    /// Clears the audio, the stamps, the synthesis histories and the history
    /// of the aspiration filter.
    ///
    /// The function also selects and clears the initial bank pair, and starts
    /// the correlation smoothing again. The other banks stay allocated. The
    /// stage clears each bank when it uses that bank again.
    pub(crate) fn reset(&mut self) {
        self.aspiration_muted = false;
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.input.fill(0_i32);

        self.stamps.fill(Stamp::default());
        self.history = [[0; MAX_ORDER * 2]; VOICE_BANKS];
        self.history_index = [0; VOICE_BANKS];
        self.history_dirty = [false; VOICE_BANKS];
        self.stamp.current = (BANKS - 1) as u8;
        self.next_filter();
        self.next_filter();
        self.design_counter = 0;
        self.noise_filter.reset();
        self.smooth_reset = true;
    }
    /// Starts a new model and a new correlation window when pitch acquisition
    /// locks.
    ///
    /// The audio and the model banks do not change. Thus, the coefficients
    /// can blend into the new model.
    pub(crate) fn detection_mode_off(&mut self) {
        self.smooth_reset = true;
        self.design_counter = 0;
    }
    /// Applies pre-emphasis to a Q14 sample word.
    ///
    /// The function subtracts the product of the previous sample and the
    /// glottal coefficient. It uses the common signed rounding rule.
    #[inline]
    pub(crate) fn remove_glottal(&self, sample: i32, previous: i32) -> i32 {
        sample.saturating_sub(round_q14(i64::from(previous) * i64::from(self.glottal)))
    }
    /// Puts the glottal envelope back from a Q14 excitation and the previous
    /// output.
    ///
    /// The caller keeps the history of previous outputs, separately for each
    /// voice slot.
    #[inline]
    pub(crate) fn model_glottal(&self, sample: i32, previous: i32) -> i32 {
        sample.saturating_add(round_q14(i64::from(previous) * i64::from(self.glottal)))
    }
    /// Stores the pre-emphasized input, tries a model design if one is due,
    /// and returns the inverse-filtered sample.
    ///
    /// The returned sample has a delay of ANALYSIS_FRAMES. `write` and the
    /// period history must refer to the same full-size ring of audio frames.
    /// `output_period_q6` is the kept period that sets the analysis segments.
    pub(crate) fn process_analysis(
        &mut self,
        sample: i32,
        previous: i32,
        periods: &[Q6],
        write: usize,
        output_period_q6: i32,
    ) -> i32 {
        self.dirty = true;
        let filtered = self.remove_glottal(sample, previous);
        let reverse_write = HISTORY - 1 - write;
        self.input[reverse_write] = filtered;
        if reverse_write < ORDER {
            self.input[reverse_write + HISTORY] = filtered;
        }

        self.design(periods, write, output_period_q6);
        self.inverse(write)
    }
    /// Moves to the next bank and keeps the old bank for the blend.
    ///
    /// The function clears the inverse coefficients, the forward coefficients
    /// and the active order of the new bank.
    fn next_filter(&mut self) {
        self.bank_epoch = self.bank_epoch.wrapping_add(1);
        self.stamp.previous = self.stamp.current;
        self.stamp.current += 1;
        if usize::from(self.stamp.current) == BANKS {
            self.stamp.current = 0;
        }
        self.stamp.blend = 0;
        let current = usize::from(self.stamp.current);
        self.inverse[current].fill(0_i32);
        for slot in 0..VOICE_BANKS {
            self.models[slot][current].fill(0_i32);
            self.orders[slot][current] = 0;
        }
    }
    /// Calculates the average of a maximum of two autocorrelation vectors. It
    /// does not allocate memory.
    ///
    /// The slice must fit in the histories of 111 entries. The function
    /// divides each input by two before it adds the input to the sum. The
    /// compensation for the first window keeps that order of division.
    fn smooth(&mut self, values: &mut [i32]) {
        if self.smooth_reset {
            self.smooth_index = 0;
            self.smooth_filled = 0;
            self.smooth_reset = false;
            self.correlation_sum[..values.len()].fill(0_i32);
        }
        for (i, value) in values.iter_mut().enumerate() {
            if self.smooth_filled == 2 {
                self.correlation_sum[i] = self.correlation_sum[i]
                    .wrapping_sub(self.correlation_history[self.smooth_index][i]);
            }
            *value = value.wrapping_div(2);
            self.correlation_history[self.smooth_index][i] = *value;
            self.correlation_sum[i] = self.correlation_sum[i].wrapping_add(*value);
        }
        self.smooth_filled = (self.smooth_filled + 1).min(2);
        for (i, value) in values.iter_mut().enumerate() {
            *value = if self.smooth_filled < 2 {
                self.correlation_sum[i]
                    .wrapping_div(self.smooth_filled as i32)
                    .wrapping_mul(2)
            } else {
                self.correlation_sum[i]
            };
        }
        self.smooth_index = (self.smooth_index + 1) % 2;
    }
    /// Tries a model that uses the pitch each ANALYSIS_FRAMES, and stamps
    /// each frame.
    ///
    /// If a pitch is available, the segment folds adjacent cycles and clamps
    /// the period to `rate / 40`. If not, the segment is a window of
    /// approximately 10 ms. Before the Durbin analysis, the function does
    /// these steps on the correlations:
    /// 1. It rescales them during the sum.
    /// 2. It applies sinc weights.
    /// 3. It calculates their average.
    ///
    /// If the Durbin analysis succeeds, width and stretch select the model:
    /// - If the two controls are 1, the model is a copy of the inverse
    ///   coefficients.
    /// - If not, the area conversion makes the model. If the conversion
    ///   fails, the new synthesis order stays at zero.
    ///
    /// If the Durbin analysis fails, smoothing starts again. The selected
    /// bank keeps the inverse coefficients that the analysis wrote.
    fn design(&mut self, periods: &[Q6], position: usize, output_period_q6: i32) {
        let max_period = (self.rate.as_u32() as usize / 40).max(1);
        let period = (output_period_q6.wrapping_shr(6).max(0) as usize).min(max_period);
        let design_now = self.design_counter == 0;
        self.design_counter += 1;
        let one = 16384;
        if design_now {
            self.design_counter -= ANALYSIS_FRAMES as i32;
            let count;
            if period == 0 {
                count = (self.rate.as_u32() as usize + 50) / 100;
                for i in 0..count {
                    let index = ring((position as i32).wrapping_sub(i as i32));
                    self.temporary[i] = self.input[HISTORY - 1 - index];
                }
                self.temporary[count..count + ORDER].fill(0_i32);
            } else {
                count = if periods[position] == Q6::ZERO {
                    period * 2
                } else {
                    period
                };
                let extra = (period / 10).max(1);
                if periods[position] == Q6::ZERO {
                    for i in 0..count + extra {
                        let index = ring((position as i32).wrapping_sub(i as i32));
                        self.temporary[i] = self.input[HISTORY - 1 - index];
                    }
                } else {
                    for i in 0..count + extra {
                        let index = ring((position as i32).wrapping_sub(i as i32));
                        let mut accumulator = (index as i32) << 6;
                        let mut sum = 0_i32;
                        let mut reads = 0;
                        for _ in 0..2 {
                            let index = ring(accumulator.wrapping_add(32).wrapping_shr(6));
                            sum = sum.wrapping_add(self.input[HISTORY - 1 - index]);
                            reads += 1;
                            if periods[index] == Q6::ZERO {
                                break;
                            }
                            accumulator = accumulator.wrapping_sub(periods[index].to_bits());
                        }
                        self.temporary[i] = sum.wrapping_div(reads);
                    }
                }
                let mut blend = 16384;
                let step = 16384 / extra as i32;
                for i in 0..extra {
                    self.temporary[i] = mul14(16384 - blend, self.temporary[i])
                        .wrapping_add(mul14(blend, self.temporary[i + count]));
                    blend -= step;
                }
                for i in count..count + ORDER {
                    self.temporary[i] = self.temporary[i - count];
                }
            }
            let mut previous = self.temporary[count - 1];
            for i in 0..count + ORDER {
                let current = self.temporary[i];
                self.temporary[i] = self.remove_glottal(current, previous);
                previous = current;
            }
            // One common binary scale must apply to all lags. If a new product
            // needs one more shift, rescale all completed lags immediately.
            // Do not add the products first, and do not reduce the lanes in a
            // different order. These changes move the points where the shifts
            // occur, and thus change the model.
            let mut correlations = [0_i32; 111];
            let mut shift = 0;
            for lag in 0..=ORDER {
                let mut correlation = 0_i32;
                let (left_chunks, left_tail) = self.temporary[..count].as_chunks::<4>();
                let (right_chunks, right_tail) = self.temporary[lag..lag + count].as_chunks::<4>();
                let mut accumulate = |product: i32| {
                    correlation = correlation.wrapping_add(product.wrapping_shr(shift));
                    if correlation.wrapping_abs() > 0x4000000 {
                        shift += 1;
                        for prior in &mut correlations[..lag] {
                            *prior = prior.wrapping_shr(1);
                        }
                        correlation = correlation.wrapping_shr(1);
                    }
                };
                for (left, right) in left_chunks.iter().zip(right_chunks) {
                    let products = [
                        left[0].wrapping_mul(right[0]),
                        left[1].wrapping_mul(right[1]),
                        left[2].wrapping_mul(right[2]),
                        left[3].wrapping_mul(right[3]),
                    ];
                    for product in products {
                        accumulate(product);
                    }
                }
                for (&left, &right) in left_tail.iter().zip(right_tail) {
                    accumulate(left.wrapping_mul(right));
                }
                correlations[lag] = correlation;
            }
            let mut normalization = 0;
            while correlations[0] > one {
                correlations[0] = correlations[0].wrapping_shr(1);
                normalization += 1;
            }
            for value in &mut correlations[1..=ORDER] {
                *value = value.wrapping_shr(normalization);
            }
            let scale = (8192 / if period != 0 { period } else { count })
                .min((2.0 / ORDER as f64 * 16384.0) as usize) as i32;
            let mut accumulator = 0;
            for value in &mut correlations[1..=ORDER] {
                accumulator += scale;
                let sinc = fast_sinc(accumulator);
                *value = mul14(*value, mul14(sinc, sinc));
            }
            self.smooth(&mut correlations[..=ORDER]);
            // Select a cleared bank before the solve. If the solve fails, some
            // inverse coefficients can stay in the bank. But the synthesis
            // order stays zero until all necessary model changes complete.
            self.next_filter();
            let current = usize::from(self.stamp.current);
            self.generation = self.generation.wrapping_add(1);
            let design_gain = ((db_to_gain(-60.0) + 1.0) * 16384.0) as i32;
            correlations[0] = mul14(correlations[0], design_gain).wrapping_add(200);
            let mut parcor = [0; MAX_ORDER];
            let mut scratch = [0; 112];

            let valid = lpc::durbin(
                &correlations,
                &mut self.inverse[current],
                &mut parcor,
                &mut scratch,
            );

            if valid {
                let needs_areas = (0..VOICE_BANKS)
                    .any(|slot| self.width[slot] != one || self.stretch[slot] != one);
                let area_scale = if needs_areas {
                    lpc::parcor_to_areas(&parcor[..ORDER], &mut self.areas)
                } else {
                    None
                };
                let mut voice_parcor = [0; MAX_ORDER];
                for slot in 0..VOICE_BANKS {
                    if self.width[slot] == one && self.stretch[slot] == one {
                        self.models[slot][current][..ORDER].copy_from_slice(&self.inverse[current]);
                        self.orders[slot][current] = ORDER;
                    } else if let Some(prev) = (0..slot).find(|&prev| {
                        let count = self.stretch_count[slot];
                        self.width_squared[prev] == self.width_squared[slot]
                            && self.stretch[prev] == self.stretch[slot]
                            && self.stretch_count[prev] == count
                            && self.stretch_map[prev][..count] == self.stretch_map[slot][..count]
                    }) {
                        let order = self.orders[prev][current];
                        let (head, tail) = self.models.split_at_mut(slot);
                        tail[0][current][..order].copy_from_slice(&head[prev][current][..order]);
                        self.orders[slot][current] = order;
                    } else if let Some(scale) = area_scale {
                        let count = self.stretch_count[slot];
                        for i in 0..count {
                            let map = self.stretch_map[slot][i];
                            let index = map.wrapping_shr(7).min((ORDER - 2) as i32) as usize;
                            let fraction = map.wrapping_sub((index as i32) << 7).wrapping_shl(7);
                            self.stretched_areas[i] = mul14(
                                mul14(one.wrapping_sub(fraction), self.areas[index])
                                    .wrapping_add(mul14(fraction, self.areas[index + 1])),
                                self.width_squared[slot],
                            );
                            if self.stretched_areas[i] == 0_i32 {
                                self.stretched_areas[i] = 1;
                            }
                        }
                        if lpc::areas_to_parcor(
                            &self.stretched_areas[..count],
                            &mut voice_parcor,
                            scale,
                        ) {
                            lpc::parcor_to_pef(
                                &voice_parcor[..count],
                                &mut self.models[slot][current],
                                &mut scratch,
                            );
                            self.orders[slot][current] = count;
                        }
                    }
                }
            } else {
                self.smooth_reset = true;
            }
        }
        if self.stamp.blend < 16384 {
            self.stamp.blend = (self.stamp.blend + 1638).min(16384);
        }
        self.stamps[position] = self.stamp;
    }

    /// Applies the blended 50th-order prediction-error filter to the delayed
    /// input.
    ///
    /// The full sum of Q28 products accumulates in `i64` before one signed
    /// shift and an `i32` clamp. The function blends the coefficients for the
    /// current transition. Then it multiplies them with the previous samples.
    fn inverse(&self, position: usize) -> i32 {
        let read = if position < ANALYSIS_FRAMES {
            position + HISTORY - ANALYSIS_FRAMES
        } else {
            position - ANALYSIS_FRAMES
        };
        let mut blended;
        let coefficients = if let Some(slice) = self.stamp.steady_bank(&self.inverse, true, ORDER) {
            slice
        } else {
            blended = [0_i32; ORDER];
            self.stamp.blend_banks(&self.inverse, &mut blended)
        };
        let reverse_read = HISTORY - 1 - read;
        let sum = tap_sum(
            &self.input[reverse_read + 1..reverse_read + 1 + ORDER],
            coefficients,
        );
        self.input[reverse_read].saturating_sub(round_q14(sum))
    }
    /// Blends random aspiration into a Q14 sample word. The signal modulates
    /// the aspiration, and a high-pass filter changes it.
    ///
    /// If the stage mutes aspiration or the level is 0.01 or less, the
    /// function returns immediately. The random stream and the noise filter
    /// then do not advance. If not, the function reads one random value.
    fn add_noise(&mut self, sample: i32, context: &mut NumericContext) -> i32 {
        if self.aspiration_muted || self.config.aspiration_mix <= 0.01 {
            return sample;
        }
        let random = context.random.next_u31() >> 17;
        let noise = mul14(random - 8192, sample);
        let filtered = self
            .noise_filter
            .process_sample(Q14::from_bits(noise))
            .to_bits();
        let noise = clamp_i32(i64::from(mul14(self.noise_amplitude, filtered)) << 6);
        mul14(sample, 16384_i32.saturating_sub(self.noise_gain))
            .saturating_add(mul14(noise, self.noise_gain))
    }
    /// Returns the wrapping count of model designs that the stage tried.
    pub(crate) fn generation(&self) -> u32 {
        self.generation
    }
    /// Returns the bank epoch, the stamp at a signed history frame and the Q14
    /// lead width.
    pub(crate) fn envelope_stamp(&self, position: i32) -> (u64, u8, u8, u16, i32) {
        let stamp = self.stamps[ring(position)];
        (
            self.bank_epoch,
            stamp.current,
            stamp.previous,
            stamp.blend,
            self.width[0],
        )
    }
    /// Returns the analysis coefficients of the stamp at a signed history
    /// frame.
    ///
    /// If `stamp.blend` is 16384, the function borrows the current inverse
    /// bank. If not, the function blends the two banks into `output`. Negative
    /// positions wrap in the ring.
    fn analysis_coefficients<'a>(
        &'a self,
        position: i32,
        output: &'a mut [i32; ORDER],
    ) -> &'a [i32] {
        let stamp = self.stamps[ring(position)];
        if let Some(slice) = stamp.steady_bank(&self.inverse, true, ORDER) {
            slice
        } else {
            stamp.blend_banks(&self.inverse, output)
        }
    }
    /// Writes the magnitude response of the two models of the stamp at a
    /// signed history frame. This function is for telemetry only.
    ///
    /// `input` gets the vocal-tract envelope from the analysis. `output` gets
    /// the envelope of the synthesis model, which includes the width gain.
    /// The two responses include the glottal de-emphasis. Point `k` of `n` is
    /// at the frequency `max_frequency_hz * k / (n - 1)`. The values are
    /// linear and have no absolute level, because the analysis normalizes the
    /// signal energy.
    ///
    /// Returns `false` and fills the slices with zeros if the stamp has no
    /// synthesis model. The slices must have the same length. The function
    /// does not allocate memory and does not change the stage.
    pub(crate) fn envelope(
        &self,
        position: i32,
        max_frequency_hz: f32,
        input: &mut [f32],
        output: &mut [f32],
    ) -> bool {
        let stamp = self.stamps[ring(position)];
        let mut blended;
        let synthesis = if let Some(slice) = stamp.synthesis_bank(&self.models[0], &self.orders[0])
        {
            slice
        } else {
            blended = [0; MAX_ORDER];
            stamp.blend_synthesis(&self.models[0], &self.orders[0], &mut blended)
        };
        if synthesis.is_empty() {
            input.fill(0.0);
            output.fill(0.0);
            return false;
        }
        let points = input.len();
        if points == 0 {
            return true;
        }
        let order = synthesis.len();
        let mut analysis_blended = [0; ORDER];
        let analysis = self.analysis_coefficients(position, &mut analysis_blended);
        let step = if points > 1 {
            std::f64::consts::PI * 2.0 * max_frequency_hz as f64
                / self.rate.hz() as f64
                / (points - 1) as f64
        } else {
            0.0
        };
        const SCALE: f64 = 1.0 / 16384.0;
        let glottal = self.glottal as f64 * SCALE;
        let width = self.width[0] as f64 * SCALE;
        let mut analysis_taps = [0.0; ORDER];
        for (tap, &coefficient) in analysis_taps.iter_mut().zip(analysis) {
            *tap = coefficient as f64 * SCALE;
        }
        let mut synthesis_taps = [0.0; MAX_ORDER];
        for (tap, &coefficient) in synthesis_taps[..order].iter_mut().zip(synthesis) {
            *tap = coefficient as f64 * SCALE;
        }
        let shared = ORDER.min(order);
        let (step_sine, step_cosine) = step.sin_cos();
        let (mut bin_cosine, mut bin_sine) = (1.0, 0.0);
        // Refresh the angle from sin_cos every 64 bins to bound recurrence drift.
        let mut next_angle = |k: usize| {
            if k.is_multiple_of(64) {
                let (sine, cosine) = (step * k as f64).sin_cos();
                bin_cosine = cosine;
                bin_sine = sine;
            } else {
                (bin_cosine, bin_sine) = (
                    bin_cosine * step_cosine - bin_sine * step_sine,
                    bin_sine * step_cosine + bin_cosine * step_sine,
                );
            }
            (bin_cosine, bin_sine)
        };
        // |1 - sum(c[i] * z^-(i+1))| at z = e^(j * omega).
        let (input_chunks, input_tail) = input.as_chunks_mut::<4>();
        let (output_chunks, output_tail) = output.as_chunks_mut::<4>();
        for (chunk_index, (in_chunk, out_chunk)) in
            input_chunks.iter_mut().zip(output_chunks).enumerate()
        {
            let base = chunk_index * 4;
            let mut cosine = [0.0; 4];
            let mut sine = [0.0; 4];
            for (lane, (cos, sin)) in cosine.iter_mut().zip(&mut sine).enumerate() {
                (*cos, *sin) = next_angle(base + lane);
            }
            let mut c = cosine;
            let mut s = sine;
            let mut analysis_real = [1.0; 4];
            let mut analysis_imag = [0.0; 4];
            let mut synthesis_real = [1.0; 4];
            let mut synthesis_imag = [0.0; 4];
            for (&analysis_tap, &synthesis_tap) in analysis_taps[..shared]
                .iter()
                .zip(&synthesis_taps[..shared])
            {
                for lane in 0..4 {
                    analysis_real[lane] -= analysis_tap * c[lane];
                    analysis_imag[lane] += analysis_tap * s[lane];
                    synthesis_real[lane] -= synthesis_tap * c[lane];
                    synthesis_imag[lane] += synthesis_tap * s[lane];
                    (c[lane], s[lane]) = (
                        c[lane] * cosine[lane] - s[lane] * sine[lane],
                        s[lane] * cosine[lane] + c[lane] * sine[lane],
                    );
                }
            }
            for &analysis_tap in &analysis_taps[shared..ORDER] {
                for lane in 0..4 {
                    analysis_real[lane] -= analysis_tap * c[lane];
                    analysis_imag[lane] += analysis_tap * s[lane];
                    (c[lane], s[lane]) = (
                        c[lane] * cosine[lane] - s[lane] * sine[lane],
                        s[lane] * cosine[lane] + c[lane] * sine[lane],
                    );
                }
            }
            for &synthesis_tap in &synthesis_taps[shared..order] {
                for lane in 0..4 {
                    synthesis_real[lane] -= synthesis_tap * c[lane];
                    synthesis_imag[lane] += synthesis_tap * s[lane];
                    (c[lane], s[lane]) = (
                        c[lane] * cosine[lane] - s[lane] * sine[lane],
                        s[lane] * cosine[lane] + c[lane] * sine[lane],
                    );
                }
            }
            for lane in 0..4 {
                let emphasis = ((1.0 - glottal * cosine[lane]).powi(2)
                    + (glottal * sine[lane]).powi(2))
                .sqrt()
                .max(1e-6);
                let analysis_mag = (analysis_real[lane] * analysis_real[lane]
                    + analysis_imag[lane] * analysis_imag[lane])
                    .sqrt()
                    .max(1e-6);
                let synthesis_mag = (synthesis_real[lane] * synthesis_real[lane]
                    + synthesis_imag[lane] * synthesis_imag[lane])
                    .sqrt()
                    .max(1e-6);
                in_chunk[lane] = (1.0 / (analysis_mag * emphasis)) as f32;
                out_chunk[lane] = (width / (synthesis_mag * emphasis)) as f32;
            }
        }
        let tail_base = input_chunks.len() * 4;
        for (offset, (in_bin, out_bin)) in input_tail.iter_mut().zip(output_tail).enumerate() {
            let (cosine, sine) = next_angle(tail_base + offset);
            let (mut c, mut s) = (cosine, sine);
            let (mut analysis_real, mut analysis_imag) = (1.0, 0.0);
            let (mut synthesis_real, mut synthesis_imag) = (1.0, 0.0);
            for (&analysis_tap, &synthesis_tap) in analysis_taps[..shared]
                .iter()
                .zip(&synthesis_taps[..shared])
            {
                analysis_real -= analysis_tap * c;
                analysis_imag += analysis_tap * s;
                synthesis_real -= synthesis_tap * c;
                synthesis_imag += synthesis_tap * s;
                (c, s) = (c * cosine - s * sine, s * cosine + c * sine);
            }
            for &analysis_tap in &analysis_taps[shared..ORDER] {
                analysis_real -= analysis_tap * c;
                analysis_imag += analysis_tap * s;
                (c, s) = (c * cosine - s * sine, s * cosine + c * sine);
            }
            for &synthesis_tap in &synthesis_taps[shared..order] {
                synthesis_real -= synthesis_tap * c;
                synthesis_imag += synthesis_tap * s;
                (c, s) = (c * cosine - s * sine, s * cosine + c * sine);
            }
            let emphasis = ((1.0 - glottal * cosine).powi(2) + (glottal * sine).powi(2))
                .sqrt()
                .max(1e-6);
            let analysis_mag = (analysis_real * analysis_real + analysis_imag * analysis_imag)
                .sqrt()
                .max(1e-6);
            let synthesis_mag = (synthesis_real * synthesis_real + synthesis_imag * synthesis_imag)
                .sqrt()
                .max(1e-6);
            *in_bin = (1.0 / (analysis_mag * emphasis)) as f32;
            *out_bin = (width / (synthesis_mag * emphasis)) as f32;
        }
        true
    }
    /// Applies the stamped synthesis model and the width gain of `slot` to
    /// `sample`, and updates the feedback history of `slot`.
    #[inline]
    fn synthesize_slot(&mut self, slot: usize, sample: i32, position: i32) -> i32 {
        self.history_dirty[slot] = true;
        let stamp = self.stamps[ring(position)];
        let mut blended;
        let coefficients =
            if let Some(slice) = stamp.synthesis_bank(&self.models[slot], &self.orders[slot]) {
                slice
            } else {
                blended = [0; MAX_ORDER];
                stamp.blend_synthesis(&self.models[slot], &self.orders[slot], &mut blended)
            };
        let index = self.history_index[slot];
        let sum = tap_sum(&self.history[slot][index..], coefficients);
        let sample = sample.saturating_add(round_q14(sum));
        let next = if index == 0 { MAX_ORDER - 1 } else { index - 1 };
        self.history[slot][next] = sample;
        self.history[slot][next + MAX_ORDER] = sample;
        self.history_index[slot] = next;
        round_q14(i64::from(sample) * i64::from(self.width[slot]))
    }
    /// Applies the optional aspiration and the stamped synthesis model to one
    /// Q14 excitation, then scales the result by the lead width.
    ///
    /// The function uses the feedback history of slot zero. `position` is an
    /// integer frame of the input history. Resynthesis aligns this frame.
    pub(crate) fn forward(
        &mut self,
        sample: i32,
        position: i32,
        context: &mut NumericContext,
    ) -> i32 {
        self.dirty = true;
        let sample = self.add_noise(sample, context);
        self.synthesize_slot(0, sample, position)
    }
    /// Reads at most one random value for the current Harmony frame and
    /// advances the aspiration filter one time.
    ///
    /// If the stage mutes aspiration or the level is 0.01 or less, the
    /// function returns `None`. Then the random stream and the noise filter do
    /// not advance.
    pub(crate) fn prepare_noise(&mut self, context: &mut NumericContext) -> Option<i32> {
        if self.aspiration_muted || self.config.aspiration_mix <= 0.01 {
            return None;
        }
        self.dirty = true;
        let random = (context.random.next_u31() >> 17) - 8192;
        Some(
            self.noise_filter
                .process_sample(Q14::from_bits(random))
                .to_bits(),
        )
    }
    /// Clears the synthesis feedback history of an inactive or muted voice
    /// slot.
    pub(crate) fn bypass_voice(&mut self, slot: usize) {
        if self.history_dirty[slot] {
            self.history[slot].fill(0_i32);
            self.history_index[slot] = 0;
            self.history_dirty[slot] = false;
        }
    }
    /// Synthesizes one Harmony voice slot with its stamped model, its own
    /// feedback history and the shared aspiration sample of the frame.
    pub(crate) fn forward_voice(
        &mut self,
        slot: usize,
        sample: i32,
        position: i32,
        frame_noise: Option<i32>,
    ) -> i32 {
        self.dirty = true;
        let sample = if let Some(filtered_random) = frame_noise {
            let modulated = mul14(filtered_random, sample);
            let noise = clamp_i32(i64::from(mul14(self.noise_amplitude, modulated)) << 6);
            mul14(sample, 16384_i32.saturating_sub(self.noise_gain))
                .saturating_add(mul14(noise, self.noise_gain))
        } else {
            sample
        };
        self.synthesize_slot(slot, sample, position)
    }
}
