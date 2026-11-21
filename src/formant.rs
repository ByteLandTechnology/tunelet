//! LPC analysis that uses the pitch, changes to the vocal-tract shape, and
//! spectral synthesis.
//!
//! Pre-emphasis removes a simple glottal envelope from the input. Every
//! `analysis_frames` (~10 ms), the analysis makes a rate-scaled predictor from
//! a segment that is aligned to the pitch. If no pitch is available, the
//! segment is approximately 10 ms. The correlations are normalized, weighted
//! and smoothed. Then they become reflection coefficients. If width or stretch
//! is not 1, the code changes the reflection coefficients into tract areas.
//! Then it changes these areas into a synthesis model.
//!
//! Thirty model banks keep the history of the coefficients. Each audio frame
//! stores a pair of banks and a blend. Thus, a fractional voice reader can
//! find the model that applies to the audio that it reads again. The inverse
//! filter has a delay of `analysis_frames`. Resynthesis uses this delay to
//! align each source sample with the stamp of the frame that filtered it.
//! Audio and coefficients use Q14 words. All areas use one common scale that
//! the code sets at runtime. The order of the arithmetic and of the rescale
//! operations changes the output samples.

use crate::error::check_range;
use crate::filter::{Biquad, Design};
use crate::lpc;
use crate::numeric::fixed::{Q14, Q6};
use crate::numeric::{
    clamp_i32, db_to_gain, fast_sinc, history_frames, mul14, round_q14, NumericContext,
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
        check_range("resynthesis.formant.width", self.width, 0.5, 2.0)?;
        check_range("resynthesis.formant.stretch", self.stretch, 0.5, 2.0)?;
        let field = "resynthesis.formant.aspiration_mix";
        check_range(field, self.aspiration_mix, 0.0, 1.0)?;
        let field = "resynthesis.formant.aspiration_cutoff_hz";
        check_range(field, self.aspiration_cutoff_hz, 500.0, 10000.0)
    }
}

/// Returns the number of frames in each 10 ms formant analysis step at `rate`.
///
/// The analysis output has a delay equal to this count (441 frames at
/// 44.1 kHz).
pub(crate) const fn analysis_frames(rate: SampleRate) -> usize {
    (rate.as_u32() as usize + 50) / 100
}

/// Returns the LPC predictor order for `rate`, scaled from 50 at 44.1 kHz.
///
/// This order preserves the vocal-tract pole interval across sample rates.
const fn predictor_order(rate: SampleRate) -> usize {
    (50 * rate.as_u32() as usize + 22_050) / 44_100
}

/// Returns the synthesis filter capacity for a stretched model of `order`.
///
/// The capacity is 110 at 44.1 kHz and 480 at 192 kHz.
const fn max_synthesis_order(order: usize) -> usize {
    (order * 22).div_ceil(10)
}

/// The stack buffer ceiling for synthesis orders up to 192 kHz (`480`).
const MAX_SYNTHESIS_ORDER: usize = 480;
/// The number of coefficient banks in the ring. The stamps of the audio
/// history refer to these banks.
const BANKS: usize = 30;

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
/// history and the synthesis state for two channels.
#[derive(Debug)]
pub(crate) struct Formant {
    /// The validated formant controls. The resynthesis caller decides if this
    /// stage runs.
    config: FormantConfig,
    /// The sample rate for analysis windows, glottal decay and the aspiration
    /// filter.
    rate: SampleRate,
    /// The number of frames of input history, aligned with resynthesis.
    history_frames: usize,
    /// The number of frames in each analysis step.
    analysis_frames: usize,
    /// The order of the input LPC predictor for the prepared sample rate.
    order: usize,
    /// The capacity for stretched models at the prepared sample rate.
    max_order: usize,
    /// The Q14 increment of `blend` per input frame.
    blend_step: i32,
    /// The ring of pre-emphasized Q14 audio words. It is aligned with the
    /// resynthesis history.
    input: Box<[i32]>,
    /// The preallocated segment memory for period sums and autocorrelation.
    temporary: Box<[i32]>,
    /// The flat Q14 coefficient banks (`BANKS * max_order`) for the inverse
    /// filter. Only the first `order` entries of each bank are used.
    inverse: Box<[i32]>,
    /// The flat Q14 coefficient banks (`BANKS * max_order`) for the forward
    /// filter. `orders` holds the active lengths.
    models: Box<[i32]>,
    /// The valid synthesis order for each bank. Zero means that there is no
    /// feedback model.
    orders: [usize; BANKS],
    /// One record of a bank pair and a blend for each frame of input history.
    stamps: Box<[Stamp]>,
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
    /// The separate Q14 feedback histories of the synthesis, for the left and
    /// right channels. Mono synthesis uses channel zero.
    history: [Box<[i32]>; 2],
    /// The next slot in the synthesis history. The two channels use the same
    /// slot.
    history_index: usize,
    /// The two previous normalized autocorrelation vectors for the average.
    correlation_history: [Box<[i32]>; 2],
    /// The wrapped sum of the stored correlation vectors, each divided by two.
    correlation_sum: Box<[i32]>,
    /// Preallocated autocorrelation vector for the current design step.
    correlations: Box<[i32]>,
    /// Preallocated reflection coefficient buffer for Durbin and area steps.
    parcor: Box<[i32]>,
    /// Preallocated recursion scratch buffer for Durbin and filter conversion.
    scratch: Box<[i32]>,
    /// The slot in the correlation history that the next vector replaces.
    smooth_index: usize,
    /// The number of valid correlation vectors, from zero to two.
    smooth_filled: usize,
    /// If `true`, the next call to `smooth` clears the state of the
    /// correlation average.
    smooth_reset: bool,
    /// The Q14 first-order coefficient for pre-emphasis and de-emphasis.
    glottal: i32,
    /// The Q14 width control. The stage also uses it as the output gain of
    /// the forward filter.
    width: i32,
    /// The Q14 square of the width, for the scale of the tract areas.
    width_squared: i32,
    /// The Q14 stretch control. The stage uses it to decide if a change to
    /// the areas is necessary.
    stretch: i32,
    /// The number of active coefficients in the stretched model, not more
    /// than `max_order`.
    stretch_count: usize,
    /// The Q7 positions that map the stretched sections into the area model.
    stretch_map: Box<[i32]>,
    /// The original tract areas, with the runtime scale that the LPC
    /// conversion returns.
    areas: Box<[i32]>,
    /// The interpolated areas, scaled by the width. They use the same binary
    /// scale as the original areas.
    stretched_areas: Box<[i32]>,
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

impl Formant {
    /// Creates the stage, allocates the memory for audio, models and
    /// intermediate values, and prepares the controls for the sample rate.
    ///
    /// At the supported rates, the capacity of the temporary segment holds
    /// the longest analysis segment and the predictor padding. Later calls
    /// use this memory again.
    pub(crate) fn new(rate: SampleRate, config: FormantConfig) -> Self {
        let hist_frames = history_frames(rate);
        let ana_frames = analysis_frames(rate);
        let order = predictor_order(rate);
        let max_order = max_synthesis_order(order);
        let blend_step = ((1638 * 44_100) / rate.as_u32() as usize).max(1) as i32;
        let d1 = -std::f64::consts::LN_2 / 0.9_f64.ln();
        let d2 = rate.hz() as f64 * d1 / 44100.0;
        let mut result = Self {
            config,
            rate,
            history_frames: hist_frames,
            analysis_frames: ana_frames,
            order,
            max_order,
            blend_step,
            input: vec![0; hist_frames].into_boxed_slice(),
            temporary: vec![0_i32; (hist_frames + max_order + 256).max(2865)].into_boxed_slice(),
            inverse: vec![0; BANKS * max_order].into_boxed_slice(),
            models: vec![0; BANKS * max_order].into_boxed_slice(),
            orders: [0; BANKS],
            stamps: vec![Stamp::default(); hist_frames].into_boxed_slice(),
            current: BANKS - 1,
            previous: 0,
            blend: 0,
            design_counter: 0,
            history: [
                vec![0; max_order].into_boxed_slice(),
                vec![0; max_order].into_boxed_slice(),
            ],
            history_index: 0,
            correlation_history: [
                vec![0; order + 1].into_boxed_slice(),
                vec![0; order + 1].into_boxed_slice(),
            ],
            correlation_sum: vec![0; order + 1].into_boxed_slice(),
            correlations: vec![0; order + 1].into_boxed_slice(),
            parcor: vec![0; max_order].into_boxed_slice(),
            scratch: vec![0; max_order + 2].into_boxed_slice(),
            smooth_index: 0,
            smooth_filled: 0,
            smooth_reset: true,
            glottal: (0.5_f64.powf(1.0 / d2) * 16384.0) as i32,
            width: 16384,
            width_squared: 16384,
            stretch: 16384,
            stretch_count: order,
            stretch_map: vec![0; max_order].into_boxed_slice(),
            areas: vec![0; max_order].into_boxed_slice(),
            stretched_areas: vec![0; max_order].into_boxed_slice(),
            noise_filter: Biquad::new(),
            noise_gain: 0_i32,
            noise_amplitude: 0_i32,
            generation: 0,
        };
        result.set_config(config);
        result.reset();
        result
    }
    /// Quantizes the validated controls, prepares the Q7 map for the area
    /// interpolation and updates the coefficients of the aspiration filter.
    ///
    /// The histories do not change. A valid stretch in `0.5..=2.0` gives at
    /// least five entries. Thus, the last point of the map and the
    /// denominators of the interpolation are always defined.
    pub(crate) fn set_config(&mut self, config: FormantConfig) {
        let order = self.order;
        self.config = config;
        self.width = (config.width as f64 * 16384.0) as i32;
        self.width_squared = (config.width as f64 * config.width as f64 * 16384.0) as i32;
        self.stretch = (config.stretch as f64 * 16384.0) as i32;
        self.stretch_count =
            (((order - 1) as f64 * config.stretch as f64 + 1.001) as usize).min(self.max_order);
        for i in 0..self.stretch_count - 1 {
            let fraction = i as f64 / (self.stretch_count - 1) as f64;
            self.stretch_map[i] = ((order - 1) as f64 * fraction * 128.0) as i32;
        }
        self.stretch_map[self.stretch_count - 1] = ((order - 1) * 128) as i32;
        self.noise_gain = (config.aspiration_mix as f64 * 16384.0) as i32;
        let frequency = (config.aspiration_cutoff_hz as i32)
            .min((self.rate.hz() as f64 * 0.48) as i32)
            .max(500);
        self.noise_amplitude = (frequency as f64 * 0.8 / self.rate.hz() as f64 * 16384.0) as i32;
        self.noise_filter
            .set_design(Design::high_pass(self.rate.hz(), frequency as f32, 0.707));
    }
    /// Clears the audio, the stamps, the synthesis history and the history of
    /// the aspiration filter.
    ///
    /// The function also selects and clears the initial bank pair, and starts
    /// the correlation average again. The other banks stay allocated. The
    /// stage clears each bank when it uses that bank again.
    pub(crate) fn reset(&mut self) {
        self.input.fill(0_i32);

        self.stamps.fill(Stamp::default());
        self.history[0].fill(0);
        self.history[1].fill(0);
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
        let max_order = self.max_order;
        self.previous = self.current;
        self.current += 1;
        if self.current == BANKS {
            self.current = 0;
        }
        self.blend = 0;
        let start = self.current * max_order;
        let end = start + max_order;
        self.inverse[start..end].fill(0_i32);

        self.models[start..end].fill(0_i32);
        self.orders[self.current] = 0;
    }
    /// Calculates the average of a maximum of two autocorrelation vectors. It
    /// does not allocate memory.
    ///
    /// `self.correlations` has `order + 1` entries. The function divides each
    /// entry by two before it adds the entry to the sum. The compensation for
    /// the first window keeps that order of division.
    fn smooth(&mut self) {
        let len = self.correlations.len();
        if self.smooth_reset {
            self.smooth_index = 0;
            self.smooth_filled = 0;
            self.smooth_reset = false;
            self.correlation_sum[..len].fill(0_i32);
        }
        for (i, value) in self.correlations.iter_mut().enumerate() {
            if self.smooth_filled == 2 {
                self.correlation_sum[i] = self.correlation_sum[i]
                    .wrapping_sub(self.correlation_history[self.smooth_index][i]);
            }
            *value = value.wrapping_div(2);
            self.correlation_history[self.smooth_index][i] = *value;
            self.correlation_sum[i] = self.correlation_sum[i].wrapping_add(*value);
        }
        self.smooth_filled = (self.smooth_filled + 1).min(2);
        for (i, value) in self.correlations.iter_mut().enumerate() {
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
    /// Tries a model that uses the pitch each `analysis_frames`, and stamps
    /// each frame.
    ///
    /// If a pitch is available, the segment averages adjacent cycles. If not,
    /// the segment is a window of approximately 10 ms. Before the Durbin
    /// analysis, the function does these steps on the correlations:
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
    /// If the Durbin analysis fails, the correlation average starts again. The
    /// selected bank keeps the inverse coefficients that the analysis wrote.
    fn design(&mut self, periods: &[Q6], position: usize, output_period_q6: i32) {
        let hist_frames = self.history_frames;
        let order = self.order;
        let max_order = self.max_order;
        let period = output_period_q6.wrapping_shr(6).max(0) as usize;
        let design_now = self.design_counter == 0;
        self.design_counter += 1;
        let one = 16384;
        if design_now {
            self.design_counter -= self.analysis_frames as i32;
            let count;
            if period == 0 {
                count = (self.rate.as_u32() as usize + 50) / 100;
                for i in 0..count {
                    self.temporary[i] = self.input[(position + hist_frames - i) % hist_frames];
                }
                self.temporary[count..count + order].fill(0_i32);
            } else {
                count = if periods[position] == Q6::ZERO {
                    period * 2
                } else {
                    period
                };
                let extra = (period / 10).max(1);
                if periods[position] == Q6::ZERO {
                    for i in 0..count + extra {
                        self.temporary[i] = self.input[(position + hist_frames - i) % hist_frames];
                    }
                } else {
                    for i in 0..count + extra {
                        let index = (position + hist_frames - i) % hist_frames;
                        let mut accumulator = (index as i32) << 6;
                        let mut sum = 0_i32;
                        let mut reads = 0;
                        for _ in 0..2 {
                            let index = accumulator
                                .wrapping_add(32)
                                .wrapping_shr(6)
                                .rem_euclid(hist_frames as i32)
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
                for i in count..count + order {
                    self.temporary[i] = self.temporary[i - count];
                }
            }
            let mut previous = self.temporary[count - 1];
            for sample in &mut self.temporary[..count + order] {
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
            self.correlations.fill(0_i32);
            let mut shift = 0;
            for lag in 0..=order {
                let mut correlation = 0_i32;
                let (left_chunks, left_tail) = self.temporary[..count].as_chunks::<4>();
                let (right_chunks, right_tail) = self.temporary[lag..lag + count].as_chunks::<4>();
                let prior_correlations = &mut self.correlations[..lag];
                let mut accumulate = |product: i32| {
                    correlation = correlation.wrapping_add(product.wrapping_shr(shift));
                    if correlation.wrapping_abs() > 0x4000000 {
                        shift += 1;
                        for prior in &mut *prior_correlations {
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
                self.correlations[lag] = correlation;
            }
            let mut normalization = 0;
            while self.correlations[0] > one {
                self.correlations[0] = self.correlations[0].wrapping_shr(1);
                normalization += 1;
            }
            for value in &mut self.correlations[1..=order] {
                *value = value.wrapping_shr(normalization);
            }
            let scale = (8192 / if period != 0 { period } else { count })
                .min((2.0 / order as f64 * 16384.0) as usize) as i32;
            let mut accumulator = 0;
            for value in &mut self.correlations[1..=order] {
                accumulator += scale;
                let sinc = fast_sinc(accumulator);
                *value = mul14(*value, mul14(sinc, sinc));
            }
            self.smooth();
            // Select a cleared bank before the solve. If the solve fails, some
            // inverse coefficients can stay in the bank. But the synthesis
            // order stays zero until all necessary model changes complete.
            self.next_filter();
            self.generation = self.generation.wrapping_add(1);
            let design_gain = ((db_to_gain(-60.0) + 1.0) * 16384.0) as i32;
            self.correlations[0] = mul14(self.correlations[0], design_gain).wrapping_add(200);
            self.parcor.fill(0);
            self.scratch.fill(0);
            let bank_start = self.current * max_order;

            let valid = lpc::durbin(
                &self.correlations,
                &mut self.inverse[bank_start..bank_start + order],
                &mut self.parcor,
                &mut self.scratch,
            );

            if valid {
                if self.width != one || self.stretch != one {
                    if let Some(scale) =
                        lpc::parcor_to_areas(&self.parcor[..order], &mut self.areas)
                    {
                        for i in 0..self.stretch_count {
                            let map = self.stretch_map[i];
                            let index = map.wrapping_shr(7).min((order - 2) as i32) as usize;
                            let fraction = map.wrapping_sub((index as i32) << 7).wrapping_shl(7);
                            self.stretched_areas[i] = mul14(
                                mul14(one.wrapping_sub(fraction), self.areas[index])
                                    .wrapping_add(mul14(fraction, self.areas[index + 1])),
                                self.width_squared,
                            );
                            if self.stretched_areas[i] == 0_i32 {
                                self.stretched_areas[i] = 1;
                            }
                        }
                        if lpc::areas_to_parcor(
                            &self.stretched_areas[..self.stretch_count],
                            &mut self.parcor,
                            scale,
                        ) {
                            lpc::parcor_to_pef(
                                &self.parcor[..self.stretch_count],
                                &mut self.models[bank_start..bank_start + max_order],
                                &mut self.scratch,
                                14,
                            );
                            self.orders[self.current] = self.stretch_count;
                        }
                    }
                } else {
                    self.models[bank_start..bank_start + order]
                        .copy_from_slice(&self.inverse[bank_start..bank_start + order]);
                    self.orders[self.current] = order;
                }
            } else {
                self.smooth_reset = true;
            }
        }
        if self.blend < 16384 {
            self.blend = (self.blend + self.blend_step).min(16384);
        }
        self.stamps[position] = Stamp {
            current: self.current,
            previous: self.previous,
            blend: self.blend,
        };
    }

    /// Applies the blended prediction-error filter to the delayed input.
    ///
    /// The full sum of Q28 products accumulates in `i64` before one signed
    /// shift and an `i32` clamp. The function blends the coefficients for the
    /// current transition. Then it multiplies them with the previous samples.
    fn inverse(&self, position: usize) -> i32 {
        let hist_frames = self.history_frames;
        let ana_frames = self.analysis_frames;
        let max_order = self.max_order;
        let read = if position < ana_frames {
            position + hist_frames - ana_frames
        } else {
            position - ana_frames
        };

        let previous_base = self.previous * max_order;
        let current_base = self.current * max_order;
        let mut cursor = read;
        let mut sum = 0_i64;
        for i in 0..self.order {
            let coefficient = if self.blend < 16384 {
                mul14(16384 - self.blend, self.inverse[previous_base + i])
                    .saturating_add(mul14(self.blend, self.inverse[current_base + i]))
            } else {
                self.inverse[current_base + i]
            };
            cursor = if cursor == 0 {
                hist_frames - 1
            } else {
                cursor - 1
            };
            sum = sum.saturating_add(i64::from(self.input[cursor]) * i64::from(coefficient));
        }
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
    /// Finds and blends the synthesis model of the stamp at a signed history
    /// frame.
    ///
    /// Returns the length of the active prefix that the function writes to
    /// `output`. The entries after the prefix are not specified, and you must
    /// not use them. Negative positions wrap in the ring.
    fn coefficients(&self, position: i32, output: &mut [i32; MAX_SYNTHESIS_ORDER]) -> usize {
        let stamp = self.stamps[position.rem_euclid(self.history_frames as i32) as usize];
        let max_order = self.max_order;
        let previous_base = stamp.previous * max_order;
        let current_base = stamp.current * max_order;
        let mut order = self.orders[stamp.current];
        if stamp.blend < 16384 && self.orders[stamp.previous] != 0 {
            order = order.max(self.orders[stamp.previous]);
            for (i, value) in output[..order].iter_mut().enumerate() {
                *value = mul14(16384 - stamp.blend, self.models[previous_base + i])
                    .saturating_add(mul14(stamp.blend, self.models[current_base + i]));
            }
        } else {
            output[..order].copy_from_slice(&self.models[current_base..current_base + order]);
        }
        order
    }
    /// Returns the wrapping count of model designs that the stage tried.
    pub(crate) fn generation(&self) -> u32 {
        self.generation
    }
    /// Finds and blends the analysis coefficients of the stamp at a signed
    /// history frame, in the same way as [`Self::coefficients`].
    ///
    /// The function writes `self.order` entries to the start of `output`.
    fn analysis_coefficients(&self, position: i32, output: &mut [i32; MAX_SYNTHESIS_ORDER]) {
        let stamp = self.stamps[position.rem_euclid(self.history_frames as i32) as usize];
        let max_order = self.max_order;
        let previous_base = stamp.previous * max_order;
        let current_base = stamp.current * max_order;
        let order = self.order;
        if stamp.blend < 16384 {
            for (i, value) in output[..order].iter_mut().enumerate() {
                *value = mul14(16384 - stamp.blend, self.inverse[previous_base + i])
                    .saturating_add(mul14(stamp.blend, self.inverse[current_base + i]));
            }
        } else {
            output[..order].copy_from_slice(&self.inverse[current_base..current_base + order]);
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
        let mut synthesis = [0; MAX_SYNTHESIS_ORDER];
        let order = self.coefficients(position, &mut synthesis);
        if order == 0 {
            input.fill(0.0);
            output.fill(0.0);
            return false;
        }
        let mut analysis = [0; MAX_SYNTHESIS_ORDER];
        self.analysis_coefficients(position, &mut analysis);
        let analysis_order = self.order;
        let points = input.len();
        let step = if points > 1 {
            std::f64::consts::PI * 2.0 * max_frequency_hz as f64
                / self.rate.hz() as f64
                / (points - 1) as f64
        } else {
            0.0
        };
        let glottal = self.glottal as f64 / 16384.0;
        let width = self.width as f64 / 16384.0;
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
            *input =
                (1.0 / (magnitude(&analysis[..analysis_order], cosine, sine) * emphasis)) as f32;
            *output = (width / (magnitude(&synthesis[..order], cosine, sine) * emphasis)) as f32;
        }
        true
    }
    /// Applies the optional aspiration and the stamped synthesis model to one
    /// Q14 excitation, then scales the result by the width.
    ///
    /// The function uses the feedback history of channel zero. `position` is
    /// an integer frame of the input history. Resynthesis aligns this frame.
    pub(crate) fn forward(
        &mut self,
        sample: i32,
        position: i32,
        context: &mut NumericContext,
    ) -> i32 {
        let sample = self.add_noise(sample, context);
        let mut coefficients = [0; MAX_SYNTHESIS_ORDER];
        let order = self.coefficients(position, &mut coefficients);
        let max_order = self.max_order;
        let mut sum = 0_i64;
        let mut cursor = self.history_index;
        for &coefficient in &coefficients[..order] {
            cursor = if cursor == 0 {
                max_order - 1
            } else {
                cursor - 1
            };
            sum = sum.saturating_add(i64::from(self.history[0][cursor]) * i64::from(coefficient));
        }
        let sample = sample.saturating_add(round_q14(sum));
        self.history[0][self.history_index] = sample;
        self.history_index += 1;
        if self.history_index == max_order {
            self.history_index = 0;
        }
        round_q14(i64::from(sample) * i64::from(self.width))
    }
    /// Synthesizes two Q14 channels with one stamped model for the two
    /// channels. Each channel has its own feedback history.
    ///
    /// The function applies the aspiration mix one time to the stereo
    /// midpoint. Then it adds the change of the midpoint to the two channels.
    /// If aspiration is active, the function reads one random value for each
    /// frame. The history cursor is the same for the two channels. It
    /// advances only after the two channels.
    pub(crate) fn forward_stereo(
        &mut self,
        mut samples: [i32; 2],
        position: i32,
        context: &mut NumericContext,
    ) -> [i32; 2] {
        let mid = ((i64::from(samples[0]) + i64::from(samples[1])) >> 1) as i32;
        let noise = self.add_noise(mid, context).saturating_sub(mid);
        for sample in &mut samples {
            *sample = sample.saturating_add(noise);
        }
        let mut coefficients = [0; MAX_SYNTHESIS_ORDER];
        let order = self.coefficients(position, &mut coefficients);
        let max_order = self.max_order;
        for (channel, sample) in samples.iter_mut().enumerate() {
            let mut sum = 0_i64;
            for (i, &coefficient) in coefficients[..order].iter().enumerate() {
                sum = sum.saturating_add(
                    i64::from(
                        self.history[channel][(self.history_index + max_order - i - 1) % max_order],
                    ) * i64::from(coefficient),
                );
            }
            *sample = sample.saturating_add(round_q14(sum));
            self.history[channel][self.history_index] = *sample;
            *sample = round_q14(i64::from(*sample) * i64::from(self.width));
        }
        self.history_index = (self.history_index + 1) % max_order;
        samples
    }
}
