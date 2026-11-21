//! LPC analysis that uses the pitch, changes to the vocal-tract shape, and
//! spectral synthesis.
//!
//! Pre-emphasis removes a simple glottal envelope from the input. Every 441
//! frames, the analysis makes a 50th-order predictor from a segment that is
//! aligned to the pitch. If no pitch is available, the segment is
//! approximately 10 ms. The correlations are normalized, weighted and
//! smoothed. Then they become reflection coefficients. If width or stretch
//! is not 1, the code changes the reflection coefficients into tract areas.
//! Then it changes these areas into a synthesis model.
//!
//! Thirty model banks keep the history of the coefficients. Each audio frame
//! stores a pair of banks and a blend. Thus, a fractional voice reader can
//! find the model that applies to the audio that it reads again. The inverse
//! filter has a delay of 441 frames. Resynthesis uses this delay to align
//! each source sample with the stamp of the frame that filtered it. Audio
//! and coefficients use Q14 words. All areas use one common scale that the
//! code sets at runtime. The order of the arithmetic and of the rescale
//! operations changes the output samples.

use crate::error::check_range;
use crate::filter::{Biquad, Design};
use crate::harmony::HARMONY_VOICES;
use crate::lpc;
use crate::numeric::fixed::{Q14, Q6};
use crate::numeric::{clamp_i32, db_to_gain, fast_sinc, mul14, round_q14, NumericContext};
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
        check_range("resynthesis.formant.width", self.width, 0.5, 2.0)?;
        check_range("resynthesis.formant.stretch", self.stretch, 0.5, 2.0)?;
        let field = "resynthesis.formant.aspiration_mix";
        check_range(field, self.aspiration_mix, 0.0, 1.0)?;
        let field = "resynthesis.formant.aspiration_cutoff_hz";
        check_range(field, self.aspiration_cutoff_hz, 500.0, 10000.0)
    }
}

/// The number of frames of input history for analysis and synthesis. The
/// value is equal to the resynthesis history length.
pub(crate) const HISTORY_FRAMES: usize = 11568;
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
/// The number of synthesis voice models: the lead voice and four harmony
/// voices.
pub(crate) const SYNTHESIS_BANKS: usize = HARMONY_VOICES + 1;

/// The model selection for one frame of input history.
///
/// Bank indices wrap independently of audio positions. `blend` is the Q14
/// weight of the current bank. The previous bank gets the rest of the weight.
#[derive(Clone, Copy, Debug)]
struct Stamp {
    /// The new model bank that was active when the related audio frame was
    /// recorded.
    current: usize,
    /// The previous bank, for the short transition between coefficients.
    previous: usize,
    /// The weight of the current bank in Q14, from zero to 16384 inclusive.
    blend: i32,
}
impl Default for Stamp {
    fn default() -> Self {
        Self {
            current: 0,
            previous: 0,
            blend: 16384,
        }
    }
}

/// The formant stage. It holds preallocated analysis memory, the model
/// histories and the synthesis state for five voice banks.
#[derive(Debug)]
pub(crate) struct Formant {
    /// The validated formant controls. The resynthesis caller decides if this
    /// stage runs.
    config: FormantConfig,
    /// The sample rate for analysis windows, glottal decay and the aspiration
    /// filter.
    rate: SampleRate,
    /// The ring of pre-emphasized Q14 audio words. It is aligned with the
    /// resynthesis history.
    input: Vec<i32>,
    /// The preallocated segment memory for period folding and
    /// autocorrelation.
    temporary: Vec<i32>,
    /// The Q14 coefficient banks for the inverse filter. Only the first ORDER
    /// coefficients are used.
    inverse: Vec<[i32; MAX_ORDER]>,
    /// The Q14 coefficient banks for the five forward synthesis filters.
    /// `orders` holds the active lengths.
    models: Vec<[[i32; MAX_ORDER]; SYNTHESIS_BANKS]>,
    /// The valid synthesis order for each voice bank and ring slot. Zero means
    /// that there is no feedback model.
    orders: [[usize; BANKS]; SYNTHESIS_BANKS],
    /// One record of a bank pair and a blend for each frame of input history.
    stamps: Vec<Stamp>,
    /// The index of the current coefficient bank.
    current: usize,
    /// The bank before `current`. The stage keeps it while the coefficients
    /// blend.
    previous: usize,
    /// The current Q14 bank weight. Each input frame of the analysis moves it
    /// toward 1.0.
    blend: i32,
    /// A counter that increases from a negative value to zero. At zero, the
    /// stage tries to make a model.
    design_counter: i32,
    /// The separate Q14 feedback histories of the synthesis, for the lead
    /// voice and the four harmony voices. Choir uses slot zero.
    history: [[i32; MAX_ORDER]; SYNTHESIS_BANKS],
    /// The next slot in the synthesis history. All voice banks use the same
    /// slot.
    history_index: usize,
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
    /// The Q14 first-order coefficient for pre-emphasis and de-emphasis.
    glottal: i32,
    /// The Q14 width control for each voice bank. The stage also uses it as
    /// the output gain of the forward filter.
    width: [i32; SYNTHESIS_BANKS],
    /// The Q14 square of each width, for the scale of the tract areas.
    width_squared: [i32; SYNTHESIS_BANKS],
    /// The Q14 stretch control for each voice bank. The stage uses it to
    /// decide if a change to the areas is necessary.
    stretch: [i32; SYNTHESIS_BANKS],
    /// The number of active coefficients in each stretched model, not more
    /// than MAX_ORDER.
    stretch_count: [usize; SYNTHESIS_BANKS],
    /// The Q7 positions that map each stretched model into the area model with
    /// 50 sections.
    stretch_map: [[i32; MAX_ORDER]; SYNTHESIS_BANKS],
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
    /// The wrapping count of model designs that the stage tried. Only the
    /// telemetry reads it.
    generation: u32,
}

/// Multiplies `coefficients` with the samples before `index` in `ring`.
///
/// If the span crosses index zero, the function splits `ring` into two
/// slices. The function adds each `i64` product with saturation in tap order.
#[inline]
fn ring_sum(ring: &[i32], index: usize, coefficients: &[i32]) -> i64 {
    let (head, tail) = ring.split_at(index);
    let (first, second) = coefficients.split_at(coefficients.len().min(head.len()));
    let mut sum = 0_i64;
    for (&sample, &coefficient) in head.iter().rev().zip(first) {
        sum = sum.saturating_add(i64::from(sample) * i64::from(coefficient));
    }
    for (&sample, &coefficient) in tail.iter().rev().zip(second) {
        sum = sum.saturating_add(i64::from(sample) * i64::from(coefficient));
    }
    sum
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
            rate,
            input: vec![0; HISTORY_FRAMES],
            temporary: vec![0_i32; (2 * (rate.as_u32() as usize / 40) + MAX_ORDER + 256).max(2865)],
            inverse: vec![[0; MAX_ORDER]; BANKS],
            models: vec![[[0; MAX_ORDER]; SYNTHESIS_BANKS]; BANKS],
            orders: [[0; BANKS]; SYNTHESIS_BANKS],
            stamps: vec![Stamp::default(); HISTORY_FRAMES],
            current: BANKS - 1,
            previous: 0,
            blend: 0,
            design_counter: 0,
            history: [[0; MAX_ORDER]; SYNTHESIS_BANKS],
            history_index: 0,
            correlation_history: [[0; 111]; 2],
            correlation_sum: [0; 111],
            smooth_index: 0,
            smooth_filled: 0,
            smooth_reset: true,
            glottal: (0.5_f64.powf(1.0 / d2) * 16384.0) as i32,
            width: [16384; SYNTHESIS_BANKS],
            width_squared: [16384; SYNTHESIS_BANKS],
            stretch: [16384; SYNTHESIS_BANKS],
            stretch_count: [ORDER; SYNTHESIS_BANKS],
            stretch_map: [[0; MAX_ORDER]; SYNTHESIS_BANKS],
            areas: [0; MAX_ORDER],
            stretched_areas: [0; MAX_ORDER],
            noise_filter: Biquad::new(),
            noise_gain: 0_i32,
            noise_amplitude: 0_i32,
            generation: 0,
        };
        result.set_config(config, voice_width, voice_stretch);
        result.reset();
        result
    }
    /// Quantizes the validated controls, prepares the Q7 maps for the area
    /// interpolation and updates the coefficients of the aspiration filter.
    ///
    /// The histories do not change. A valid stretch gives a model with 25 to
    /// 99 entries. Thus, the last point of each map and the denominators of
    /// the interpolation are always defined.
    pub(crate) fn set_config(
        &mut self,
        config: FormantConfig,
        voice_width: [f32; HARMONY_VOICES],
        voice_stretch: [f32; HARMONY_VOICES],
    ) {
        self.config = config;
        self.prepare_bank(0, config.width, config.stretch);
        for voice in 0..HARMONY_VOICES {
            self.prepare_bank(voice + 1, voice_width[voice], voice_stretch[voice]);
        }
        self.noise_gain = (config.aspiration_mix as f64 * 16384.0) as i32;
        let frequency = (config.aspiration_cutoff_hz as i32)
            .min((self.rate.hz() as f64 * 0.48) as i32)
            .max(500);
        self.noise_amplitude = (frequency as f64 * 0.8 / self.rate.hz() as f64 * 16384.0) as i32;
        self.noise_filter
            .set_design(Design::high_pass(self.rate.hz(), frequency as f32, 0.707));
    }
    /// Quantizes the width and stretch controls for one synthesis bank.
    fn prepare_bank(&mut self, bank: usize, width: f32, stretch: f32) {
        self.width[bank] = (width as f64 * 16384.0) as i32;
        self.width_squared[bank] = (width as f64 * width as f64 * 16384.0) as i32;
        self.stretch[bank] = (stretch as f64 * 16384.0) as i32;
        let count = (((ORDER - 1) as f64 * stretch as f64 + 1.001) as usize).min(MAX_ORDER);
        self.stretch_count[bank] = count;
        for i in 0..count - 1 {
            let fraction = i as f64 / (count - 1) as f64;
            self.stretch_map[bank][i] = ((ORDER - 1) as f64 * fraction * 128.0) as i32;
        }
        self.stretch_map[bank][count - 1] = ((ORDER - 1) * 128) as i32;
    }
    /// Clears the synthesis feedback of the four harmony banks.
    pub(crate) fn clear_harmony_history(&mut self) {
        for slot in 1..SYNTHESIS_BANKS {
            self.history[slot].fill(0_i32);
        }
    }
    /// Clears the audio, the stamps, the synthesis history and the history of
    /// the aspiration filter.
    ///
    /// The function also selects and clears the initial bank pair, and starts
    /// the correlation smoothing again. The other banks stay allocated. The
    /// stage clears each bank when it uses that bank again.
    pub(crate) fn reset(&mut self) {
        self.input.fill(0_i32);

        self.stamps.fill(Stamp::default());
        self.history = [[0; MAX_ORDER]; SYNTHESIS_BANKS];
        self.history_index = 0;
        self.current = BANKS - 1;
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
    /// channel.
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
        self.input[write] = self.remove_glottal(sample, previous);

        self.design(periods, write, output_period_q6);
        self.inverse(write)
    }
    /// Moves to the next bank and keeps the old bank for the blend.
    ///
    /// The function clears the inverse coefficients, the forward coefficients
    /// and the active order of the new bank.
    fn next_filter(&mut self) {
        self.previous = self.current;
        self.current += 1;
        if self.current == BANKS {
            self.current = 0;
        }
        self.blend = 0;
        self.inverse[self.current].fill(0_i32);

        for bank in 0..SYNTHESIS_BANKS {
            self.models[self.current][bank].fill(0_i32);
            self.orders[bank][self.current] = 0;
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
                    let index = (position as i32)
                        .wrapping_sub(i as i32)
                        .rem_euclid(HISTORY_FRAMES as i32) as usize;
                    self.temporary[i] = self.input[index];
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
                        let index = (position as i32)
                            .wrapping_sub(i as i32)
                            .rem_euclid(HISTORY_FRAMES as i32)
                            as usize;
                        self.temporary[i] = self.input[index];
                    }
                } else {
                    for i in 0..count + extra {
                        let index = (position as i32)
                            .wrapping_sub(i as i32)
                            .rem_euclid(HISTORY_FRAMES as i32)
                            as usize;
                        let mut accumulator = (index as i32) << 6;
                        let mut sum = 0_i32;
                        let mut reads = 0;
                        for _ in 0..2 {
                            let index = accumulator
                                .wrapping_add(32)
                                .wrapping_shr(6)
                                .rem_euclid(HISTORY_FRAMES as i32)
                                as usize;
                            sum = sum.wrapping_add(self.input[index]);
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
            for sample in &mut self.temporary[..count + ORDER] {
                let current = *sample;
                *sample = current
                    .saturating_sub(round_q14(i64::from(previous) * i64::from(self.glottal)));
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
            self.generation = self.generation.wrapping_add(1);
            let design_gain = ((db_to_gain(-60.0) + 1.0) * 16384.0) as i32;
            correlations[0] = mul14(correlations[0], design_gain).wrapping_add(200);
            let mut parcor = [0; MAX_ORDER];
            let mut scratch = [0; 112];

            let valid = lpc::durbin(
                &correlations,
                &mut self.inverse[self.current][..ORDER],
                &mut parcor,
                &mut scratch,
            );

            if valid {
                let needs_areas = (0..SYNTHESIS_BANKS)
                    .any(|bank| self.width[bank] != one || self.stretch[bank] != one);
                let area_scale = if needs_areas {
                    lpc::parcor_to_areas(&parcor[..ORDER], &mut self.areas)
                } else {
                    None
                };
                for bank in 0..SYNTHESIS_BANKS {
                    if self.width[bank] != one || self.stretch[bank] != one {
                        if let Some(scale) = area_scale {
                            let count = self.stretch_count[bank];
                            for i in 0..count {
                                let map = self.stretch_map[bank][i];
                                let index = map.wrapping_shr(7).min((ORDER - 2) as i32) as usize;
                                let fraction =
                                    map.wrapping_sub((index as i32) << 7).wrapping_shl(7);
                                self.stretched_areas[i] = mul14(
                                    mul14(one.wrapping_sub(fraction), self.areas[index])
                                        .wrapping_add(mul14(fraction, self.areas[index + 1])),
                                    self.width_squared[bank],
                                );
                                if self.stretched_areas[i] == 0_i32 {
                                    self.stretched_areas[i] = 1;
                                }
                            }
                            if lpc::areas_to_parcor(
                                &self.stretched_areas[..count],
                                &mut parcor,
                                scale,
                            ) {
                                lpc::parcor_to_pef(
                                    &parcor[..count],
                                    &mut self.models[self.current][bank],
                                    &mut scratch,
                                    14,
                                );
                                self.orders[bank][self.current] = count;
                            }
                        }
                    } else {
                        self.models[self.current][bank][..ORDER]
                            .copy_from_slice(&self.inverse[self.current][..ORDER]);
                        self.orders[bank][self.current] = ORDER;
                    }
                }
            } else {
                self.smooth_reset = true;
            }
        }
        if self.blend < 16384 {
            self.blend = (self.blend + 1638).min(16384);
        }
        self.stamps[position] = Stamp {
            current: self.current,
            previous: self.previous,
            blend: self.blend,
        };
    }

    /// Applies the blended 50th-order prediction-error filter to the delayed
    /// input.
    ///
    /// The full sum of Q28 products accumulates in `i64` before one signed
    /// shift and an `i32` clamp. The function blends the coefficients for the
    /// current transition. Then it multiplies them with the previous samples.
    fn inverse(&self, position: usize) -> i32 {
        let read = if position < ANALYSIS_FRAMES {
            position + HISTORY_FRAMES - ANALYSIS_FRAMES
        } else {
            position - ANALYSIS_FRAMES
        };
        let mut blended;
        let coefficients = if self.blend < 16384 {
            blended = [0_i32; ORDER];
            for (i, value) in blended.iter_mut().enumerate() {
                *value = mul14(16384 - self.blend, self.inverse[self.previous][i])
                    .saturating_add(mul14(self.blend, self.inverse[self.current][i]));
            }
            &blended[..]
        } else {
            &self.inverse[self.current][..ORDER]
        };
        let sum = ring_sum(&self.input, read, coefficients);
        self.input[read].saturating_sub(round_q14(sum))
    }
    /// Blends random aspiration into a Q14 sample word. The signal modulates
    /// the aspiration, and a high-pass filter changes it.
    ///
    /// If the level is 0.01 or less, the function returns immediately. The
    /// random stream and the noise filter then do not advance. If not, the
    /// function reads one random value.
    fn add_noise(&mut self, sample: i32, context: &mut NumericContext) -> i32 {
        if self.config.aspiration_mix <= 0.01 {
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
    /// Finds the synthesis model of `bank` at a signed history frame.
    ///
    /// If `stamp.blend` is 16384 or the previous bank has order zero, the
    /// function borrows the current bank. If not, the function returns `None`.
    /// Negative positions wrap in the ring.
    #[inline]
    fn coefficients(&self, bank: usize, position: i32) -> Option<&[i32]> {
        let stamp = self.stamps[position.rem_euclid(HISTORY_FRAMES as i32) as usize];
        if stamp.blend < 16384 && self.orders[bank][stamp.previous] != 0 {
            None
        } else {
            Some(&self.models[stamp.current][bank][..self.orders[bank][stamp.current]])
        }
    }
    /// Blends the two synthesis models of `bank` at a signed history frame
    /// into `output`.
    ///
    /// Returns the active prefix of `output`. Negative positions wrap in the
    /// ring.
    fn blend_coefficients<'a>(
        &self,
        bank: usize,
        position: i32,
        output: &'a mut [i32; MAX_ORDER],
    ) -> &'a [i32] {
        let stamp = self.stamps[position.rem_euclid(HISTORY_FRAMES as i32) as usize];
        let order = self.orders[bank][stamp.current].max(self.orders[bank][stamp.previous]);
        for (i, value) in output[..order].iter_mut().enumerate() {
            *value = mul14(16384 - stamp.blend, self.models[stamp.previous][bank][i])
                .saturating_add(mul14(stamp.blend, self.models[stamp.current][bank][i]));
        }
        &output[..order]
    }
    /// Returns the wrapping count of model designs that the stage tried.
    pub(crate) fn generation(&self) -> u32 {
        self.generation
    }
    /// Finds and blends the analysis coefficients of the stamp at a signed
    /// history frame, in the same way as [`Self::coefficients`].
    ///
    /// The function writes ORDER entries to the start of `output`.
    fn analysis_coefficients(&self, position: i32, output: &mut [i32; MAX_ORDER]) {
        let stamp = self.stamps[position.rem_euclid(HISTORY_FRAMES as i32) as usize];
        if stamp.blend < 16384 {
            for (i, value) in output[..ORDER].iter_mut().enumerate() {
                *value = mul14(16384 - stamp.blend, self.inverse[stamp.previous][i])
                    .saturating_add(mul14(stamp.blend, self.inverse[stamp.current][i]));
            }
        } else {
            output[..ORDER].copy_from_slice(&self.inverse[stamp.current][..ORDER]);
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
        let mut blended;
        let synthesis = if let Some(slice) = self.coefficients(0, position) {
            slice
        } else {
            blended = [0; MAX_ORDER];
            self.blend_coefficients(0, position, &mut blended)
        };
        if synthesis.is_empty() {
            input.fill(0.0);
            output.fill(0.0);
            return false;
        }
        let mut analysis = [0; MAX_ORDER];
        self.analysis_coefficients(position, &mut analysis);
        let points = input.len();
        let step = if points > 1 {
            std::f64::consts::PI * 2.0 * max_frequency_hz as f64
                / self.rate.hz() as f64
                / (points - 1) as f64
        } else {
            0.0
        };
        let glottal = self.glottal as f64 / 16384.0;
        let width = self.width[0] as f64 / 16384.0;
        // |1 - sum(c[i] * z^-(i+1))| at z = e^(j * omega).
        let magnitude = |coefficients: &[i32], cosine: f64, sine: f64| {
            let (mut real, mut imaginary) = (1.0, 0.0);
            let (mut c, mut s) = (cosine, sine);
            for &coefficient in coefficients {
                let value = coefficient as f64 / 16384.0;
                real -= value * c;
                imaginary += value * s;
                (c, s) = (c * cosine - s * sine, s * cosine + c * sine);
            }
            (real * real + imaginary * imaginary).sqrt().max(1e-6)
        };
        for (k, (input, output)) in input.iter_mut().zip(output.iter_mut()).enumerate() {
            let omega = step * k as f64;
            let (sine, cosine) = omega.sin_cos();
            let emphasis = ((1.0 - glottal * cosine).powi(2) + (glottal * sine).powi(2))
                .sqrt()
                .max(1e-6);
            *input = (1.0 / (magnitude(&analysis[..ORDER], cosine, sine) * emphasis)) as f32;
            *output = (width / (magnitude(synthesis, cosine, sine) * emphasis)) as f32;
        }
        true
    }
    /// Applies the optional aspiration and the lead synthesis model to one
    /// Q14 excitation, then scales the result by the lead width.
    ///
    /// The function uses the feedback history of bank zero. `position` is
    /// an integer frame of the input history. Resynthesis aligns this frame.
    pub(crate) fn forward(
        &mut self,
        sample: i32,
        position: i32,
        context: &mut NumericContext,
    ) -> i32 {
        let sample = self.add_noise(sample, context);
        let mut blended;
        let coefficients = if let Some(slice) = self.coefficients(0, position) {
            slice
        } else {
            blended = [0; MAX_ORDER];
            self.blend_coefficients(0, position, &mut blended)
        };
        let sum = ring_sum(&self.history[0], self.history_index, coefficients);
        let sample = sample.saturating_add(round_q14(sum));
        self.history[0][self.history_index] = sample;
        self.history_index += 1;
        if self.history_index == MAX_ORDER {
            self.history_index = 0;
        }
        round_q14(i64::from(sample) * i64::from(self.width[0]))
    }
    /// Synthesizes the lead voice and four harmony voices with their separate
    /// stamped models and feedback histories.
    ///
    /// If aspiration is active, the function reads one random value for the
    /// frame and filters it one time. Each voice modulates that filtered noise
    /// with its own Q14 excitation. The history cursor is the same for all
    /// five banks and advances after the five voices.
    pub(crate) fn forward_voices(
        &mut self,
        mut samples: [i32; SYNTHESIS_BANKS],
        positions: [i32; SYNTHESIS_BANKS],
        context: &mut NumericContext,
    ) -> [i32; SYNTHESIS_BANKS] {
        let shaped_noise = if self.config.aspiration_mix <= 0.01 {
            None
        } else {
            let random = (context.random.next_u31() >> 17) - 8192;
            let filtered = self
                .noise_filter
                .process_sample(Q14::from_bits(random))
                .to_bits();
            Some(mul14(self.noise_amplitude, filtered))
        };
        for (bank, sample) in samples.iter_mut().enumerate() {
            if let Some(shaped) = shaped_noise {
                let noise = clamp_i32(i64::from(mul14(shaped, *sample)) << 6);
                *sample = mul14(*sample, 16384_i32.saturating_sub(self.noise_gain))
                    .saturating_add(mul14(noise, self.noise_gain));
            }
            let mut blended;
            let coefficients = if let Some(slice) = self.coefficients(bank, positions[bank]) {
                slice
            } else {
                blended = [0; MAX_ORDER];
                self.blend_coefficients(bank, positions[bank], &mut blended)
            };
            let sum = ring_sum(&self.history[bank], self.history_index, coefficients);
            *sample = sample.saturating_add(round_q14(sum));
            self.history[bank][self.history_index] = *sample;
            *sample = round_q14(i64::from(*sample) * i64::from(self.width[bank]));
        }
        self.history_index += 1;
        if self.history_index == MAX_ORDER {
            self.history_index = 0;
        }
        samples
    }
}
