//! The pitch-synchronous resynthesis of voices from aligned ring histories.
//!
//! For each frame, the code first updates the pitch tracking. Then it does
//! the correction and control steps that are due. The raw input, the tracked
//! periods and the correction ratios use the same history cursor.
//!
//! The source comes from a fixed overlap buffer. If formants are enabled,
//! this buffer holds the output of the formant inverse analysis. If not,
//! it holds the raw input. Voice readers read this source at fractional
//! positions. If the shift is upward, prepared low-pass filters also
//! change the source.
//!
//! Choir renders the lead voice first. Then it makes delayed secondary voices
//! with drift from the lead history. Harmony renders the lead voice and four
//! readers with independent controls. It applies formant synthesis after the
//! voices are mixed.
//!
//! A reader jumps by one pitch period to stay in a range behind the writer.
//! The choir lead uses a crossfade across these jumps. Harmonic modulation
//! applies to the wet mix. Then the code blends in a delayed dry signal.
//!
//! Audio, reader positions and pitch ratios use Q14. Periods use Q6, harmony
//! gains use Q12 and the dry mix uses Q8. Some of these units use the same
//! storage format, but you cannot use one unit in place of another. A change
//! to one of these orders changes the rendered samples:
//! - The order of the reader updates.
//! - The order in which stages read random values.
//! - The order of the wrapping arithmetic.

mod history;

use crate::error::{check_count, check_range};
use crate::filter::{Biquad, Design};
use crate::formant::{Formant, FormantConfig, ANALYSIS_FRAMES};
use crate::harmony::{HarmonyConfig, HarmonyState, HARMONY_VOICES};
use crate::midi::MidiEvent;
use crate::numeric::fixed::{Q12, Q14, Q6, Q8};
use crate::numeric::{ring, NumericContext, HISTORY};
use crate::pitch::tracker::PitchTracker;
use crate::pitch::{PitchConfig, PitchEstimate};
use crate::scale::{CorrectionTable, NoteMask};
use crate::{Error, SampleRate};
use history::prepare_history;

/// The method that resynthesis uses to render its voices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub enum MultiVoiceMode {
    /// Voices at the pitch of the lead voice, with random drift, delays and
    /// an optional stereo pan.
    #[default]
    Choir,
    /// Voices that follow musical intervals, chords or MIDI notes.
    Harmony,
}

/// The controls for pitch-class correction, with target selection in the
/// period domain.
///
/// Correction is enabled by default, with all chromatic notes and a retune
/// time of zero. Target selection and the smoothing of the correction ratio
/// occur at each update of the pitch tracking. When a period is tracked,
/// this update usually occurs every five frames. If the active note table is
/// empty or correction is disabled, the ratio moves toward 1.0 at the same
/// rate.
///
/// Correction MIDI replaces the active note table. It does not change this
/// configuration. A change to `notes` or to `enabled` loads the configured
/// mask again. Reset releases the correction keys 0..=126. It keeps key 127
/// and the active note table. Key 127 has no effect on the note table.
///
/// At construction, the chromatic table uses 44.1 kHz periods, also for a
/// 48 kHz processor. Each of these events loads the periods for the prepared
/// rate:
/// - An initial mask that is not chromatic.
/// - A later change to the mask or to `enabled`.
/// - A correction MIDI event.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub struct CorrectionConfig {
    /// Enables the selection of correction targets. The default is `true`.
    ///
    /// If you disable correction, the ratio moves toward 1.0 with `retune_ms`
    /// at the next tracking updates. The ratio does not change to 1.0
    /// immediately.
    pub enabled: bool,
    /// The configured target pitch classes. The default is
    /// [`NoteMask::CHROMATIC`].
    ///
    /// All boolean masks are valid. MIDI can replace the active table. This
    /// replacement stays until this mask or `enabled` changes.
    pub notes: NoteMask,
    /// The half-life of the smoothing in milliseconds, 0..=400. The default
    /// is 0.
    ///
    /// If the value is zero, the ratio changes to the target at the next
    /// tracking update. A value that is not zero is a half-life. It is not a
    /// fixed time to get exactly to the target.
    pub retune_ms: f32,
}

impl Default for CorrectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            notes: NoteMask::default(),
            retune_ms: 0.0,
        }
    }
}

impl CorrectionConfig {
    /// Validates all fields, also if correction is disabled.
    ///
    /// All values of `enabled` and `notes` are valid. `retune_ms` must be
    /// finite and in its documented inclusive range. Validation does not
    /// clamp values. It does not change the state of the processor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`], with the dotted path of the field and
    /// the problem, if `retune_ms` is not valid.
    pub fn validate(self) -> Result<(), Error> {
        check_range(
            "resynthesis.correction.retune_ms",
            self.retune_ms,
            0.0,
            400.0,
        )
    }
}

/// The controls for pitch correction, pitch shift, voice arrangement and
/// formants.
///
/// The defaults render one choir voice with these settings:
/// - Chromatic correction.
/// - A pitch shift of 1.0.
/// - Disabled formants.
/// - No harmonic modulation.
/// - No dry mix.
/// - No stereo pan.
///
/// Validation applies to all controls, also when their stage or voice mode
/// is not active.
///
/// The dry path takes the audio after Punch and Warm and delays it by 1,543
/// frames. The wet voice readers have more delay, which changes with the
/// pitch. After this stage, the two paths go through Alien, reverb and the
/// final output gain.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase"),
    tsify(large_number_types_as_bigints)
)]
pub struct ResynthesisConfig {
    /// The acquisition sensitivity and the reporting bounds. The
    /// default is [`PitchConfig::default`].
    pub pitch: PitchConfig,
    /// The target notes and the retune time. The default is enabled
    /// chromatic correction with a retune time of zero.
    pub correction: CorrectionConfig,
    /// The pitch multiplier, 0.25..=4. The default is 1.
    ///
    /// A value of 0.5 lowers the pitch by one octave. A value of 2 raises the
    /// pitch by one octave. This value multiplies the correction ratio and
    /// the ratio of each voice. A value above 1 also enables the anti-alias
    /// low-pass filters.
    pub pitch_ratio: f32,
    /// The vocal-tract changes and aspiration. They are disabled by default.
    pub formant: FormantConfig,
    /// The voice arrangement. The default is [`MultiVoiceMode::Choir`].
    ///
    /// Harmony renders the lead voice and a maximum of four harmony voices.
    pub mode: MultiVoiceMode,
    /// The total number of choir voices, with the lead voice, 1..=16. The
    /// default is 1.
    ///
    /// The other voices read the delayed lead history with random pitch
    /// drift. The code adds the voices and does not divide the sum by the
    /// number of voices. Thus, more voices can increase the output level and
    /// the clipping. Harmony does not use this value.
    pub choir_voices: u8,
    /// The drift control for secondary voices, 10..=100, without a unit. The
    /// default is 50.
    ///
    /// The unit is not cents. The lead voice has no random choir drift. Thus,
    /// a choir with one voice does not get this detune.
    pub choir_detune: f32,
    /// The scale of the choir delay table, 20..=200, without a unit. The
    /// default is 60.
    ///
    /// A value of 200 uses the full fixed table of frame offsets. The unit is
    /// not milliseconds. The table offset of the lead voice is zero at all
    /// settings.
    pub choir_delay: f32,
    /// The stereo width of the choir, 0..=100%. The default is 100%.
    ///
    /// The code uses this value only if `stereo_output` enables the voice pan
    /// and the choir has more than one voice.
    pub choir_spread_percent: f32,
    /// Enables a stereo pan for each voice. The default is `false`.
    ///
    /// This value is independent of the buffer layout of the process method.
    /// A change to this value resets some audio histories and the feedback.
    /// [`crate::Processor::set_config`] gives the details.
    pub stereo_output: bool,
    /// The controls for the four harmony voices. The default is MIDI-channel
    /// allocation.
    ///
    /// Validation applies to these controls, and they can receive MIDI, also
    /// while Choir is selected.
    pub harmony: HarmonyConfig,
    /// The pan gain of the harmony voices, 0..=1. The default is 0.5.
    ///
    /// The code applies this gain only if `stereo_output` is `true`. The
    /// harmony path without pan does not use this control. It does not scale
    /// the lead voice.
    pub harmony_gain: f32,
    /// The modulation depth, 0..=24, without a unit. The default is 0 (off).
    ///
    /// The code multiplies the control by two and truncates it to an integer.
    /// Then it uses the result for a cosine modulation after the wet voices
    /// are mixed. The value is not an interval in semitones.
    pub harmonic_modulation: f32,
    /// The dry fraction at the resynthesis stage, 0..=1. The default is 0.
    ///
    /// The code truncates this value to Q8. The wet weight is 1.0 minus the
    /// Q8 dry weight. The dry path starts after Punch and Warm and has a
    /// delay of 1,543 frames. Alien, reverb and the output gain still occur
    /// after this stage.
    pub dry_mix: f32,
    /// The seed for choir drift and aspiration, which use the same stream.
    /// The default is `0x1234_abcd`.
    ///
    /// Only the low 32 bits select the sequence. If a configuration change
    /// gives a different 64-bit value, the processor starts the stream again
    /// from the low 32 bits. This is also true for a change to only the high
    /// bits. Reset keeps the current position in the stream. The enabled
    /// stages set the number of random values that the code reads.
    pub random_seed: u64,
}

impl Default for ResynthesisConfig {
    fn default() -> Self {
        Self {
            pitch: PitchConfig::default(),
            correction: CorrectionConfig::default(),
            pitch_ratio: 1.0,
            formant: FormantConfig::default(),
            mode: MultiVoiceMode::Choir,
            choir_voices: 1,
            choir_detune: 50.0,
            choir_delay: 60.0,
            choir_spread_percent: 100.0,
            stereo_output: false,
            harmony: HarmonyConfig::default(),
            harmony_gain: 0.5,
            harmonic_modulation: 0.0,
            dry_mix: 0.0,
            random_seed: 0x1234_abcd,
        }
    }
}

impl ResynthesisConfig {
    /// Validates all fields, also the settings for stages that are disabled
    /// or not active.
    ///
    /// Each floating-point control must be finite and in its documented
    /// inclusive range. `choir_voices` must be in 1..=16. Validation does not
    /// clamp values. It does not change the state of the processor.
    ///
    /// # Errors
    ///
    /// Returns the first [`Error::InvalidConfig`], with the dotted path of the
    /// field and the problem. For an array control in `harmony`, the error
    /// identifies the array field, not the index of an element.
    pub fn validate(self) -> Result<(), Error> {
        self.pitch.validate()?;
        self.correction.validate()?;
        self.formant.validate()?;
        self.harmony.validate()?;
        check_range("resynthesis.pitch_ratio", self.pitch_ratio, 0.25, 4.0)?;
        check_range("resynthesis.choir_detune", self.choir_detune, 10.0, 100.0)?;
        check_range("resynthesis.choir_delay", self.choir_delay, 20.0, 200.0)?;
        let field = "resynthesis.choir_spread_percent";
        check_range(field, self.choir_spread_percent, 0.0, 100.0)?;
        check_range("resynthesis.harmony_gain", self.harmony_gain, 0.0, 1.0)?;
        let field = "resynthesis.harmonic_modulation";
        check_range(field, self.harmonic_modulation, 0.0, 24.0)?;
        check_range("resynthesis.dry_mix", self.dry_mix, 0.0, 1.0)?;
        check_count(
            "resynthesis.choir_voices",
            i64::from(self.choir_voices),
            1,
            16,
        )
    }
}

/// The fixed choir delay offsets in frames, before the scale by
/// `choir_delay / 200`.
///
/// The offset of the lead voice is zero. The values are the same at the two
/// supported rates. Thus, the duration in time changes with the sample rate.
const SEEDS: [i32; 16] = [
    0, 0x14d1, 0x07f3, 0x115c, 0x1213, 0x1904, 0x0800, 0x2116, 0x1a53, 0x221c, 0x028d, 0x1dee,
    0x0dda, 0x01da, 0x167f, 0x0896,
];

/// The length of the analysis overlap buffer. The dry path has the same
/// delay.
const OVERLAP_FRAMES: usize = 1543;
/// The offset from the history position of a reader to its position for
/// formant synthesis.
const FORMANT_OFFSET_FRAMES: i32 = (OVERLAP_FRAMES - ANALYSIS_FRAMES) as i32;
/// The number of frames that a new reader stays behind the write position.
const READER_LAG_FRAMES: i32 = 50;
/// The nominal latency of resynthesis. Each reader moves with the pitch and
/// the delay.
///
/// The dry path uses only `OVERLAP_FRAMES`.
pub(crate) const LATENCY_FRAMES: usize = OVERLAP_FRAMES + READER_LAG_FRAMES as usize;
/// The counter value at which a control step runs. Usually, a step runs
/// every 101 eligible frames.
///
/// The Choir counter advances while the pitch is tracked. The Harmony
/// counter advances each frame, and MIDI can make the step due.
const CONTROL_DUE: i32 = 100;
/// The largest choir drift for each step, as a Q14 ratio offset
/// (approximately 0.2).
const DRIFT_LIMIT: Q14 = Q14::from_bits(3276);
/// The step for each frame of the crossfade after a reader jump.
const FADE_STEP: Q14 = Q14::from_bits(0x333);

/// Returns the reader position at the start of the history frame `frame`.
#[inline]
fn position_of(frame: i32) -> Q14 {
    Q14::from_bits(frame.wrapping_shl(14))
}

/// The resynthesis stage.
///
/// The units are as follows:
/// - Audio is Q14 at all points.
/// - Reader positions are history frames in Q14.
/// - Periods are Q6 samples.
/// - Ratios, pans and drift are Q14.
/// - Harmony gains are Q12.
pub(crate) struct Resynthesis {
    /// The sample rate, which does not change. The code uses it to convert
    /// periods, filters and times.
    rate: SampleRate,
    /// The current resynthesis controls. The public API validates them.
    config: ResynthesisConfig,
    /// The pitch estimator at the full rate, with decimated acquisition.
    pitch: PitchTracker,
    /// The Q14 input history after Punch and Warm, before formant analysis.
    raw: Box<[Q14; HISTORY]>,
    /// The current frame. The raw, period, ratio and rendered histories use
    /// the same frame.
    write: usize,
    /// The 128 Q14 samples of one cosine cycle, for harmonic modulation.
    cosine: [Q14; 128],
    /// The preallocated inverse and forward spectral models and the
    /// synthesis history.
    formant: Formant,
    /// The target and gain controller for the four harmony voices. The code
    /// also keeps it while Choir is selected.
    harmony: HarmonyState,
    /// The active correction targets and an independent MIDI keyboard for
    /// correction.
    scale: CorrectionTable,
    /// The delayed source audio after the two prepared shift filters. If
    /// formants are enabled, the source is inverse-filtered.
    filtered: Box<[Q14; HISTORY]>,
    /// The history of the rendered choir lead. All secondary choir voices
    /// read this history.
    lead: Box<[Q14; HISTORY]>,
    /// The Q6 tracked periods, aligned to the audio. The lead voice can
    /// replace a delayed entry with its resampled output period. This aligns
    /// the secondary voices.
    periods: Box<[Q6; HISTORY]>,
    /// The last usable Q6 tracker periods. The code keeps them through
    /// unvoiced gaps.
    previous_periods: Box<[Q6; HISTORY]>,
    /// The Q14 ratios of the correction only, aligned to each recorded input
    /// frame.
    ratios: Box<[Q14; HISTORY]>,
    /// The fixed memory for the analysis overlap. It gives the source a delay
    /// of 1,543 frames.
    overlap: [Q14; OVERLAP_FRAMES],
    /// The current slot in the overlap ring.
    overlap_index: usize,
    /// The configured integer frame offset of each voice. Harmony uses
    /// offsets of zero.
    delays: [i32; 16],
    /// The Q14 fractional reader positions. When the writer wraps, the code
    /// moves them back by HISTORY.
    positions: [Q14; 16],
    /// The Q14 random choir drift offsets of the secondary voices.
    drift: [Q14; 16],
    /// The Q14 multipliers, each equal to 1.0 plus the drift. An audio reset
    /// sets them to 1.0, but it keeps the drift state.
    drift_ratio: [Q14; 16],
    /// The Q14 left and right gains of the choir voices. The code reads them
    /// only if the stereo pan is enabled.
    pan: [[Q14; 2]; 16],
    /// The Q14 pan gains of the harmony voices. They include the configured
    /// harmony level.
    harmony_pan: [[Q14; 2]; HARMONY_VOICES],
    /// The last Q14 target ratios from the harmony controller.
    harmony_ratios: [Q14; HARMONY_VOICES],
    /// The last Q12 gain envelopes from the harmony controller. An audio
    /// reset keeps them.
    harmony_gains: [Q12; HARMONY_VOICES],
    /// The effective Q14 ratios of the lead and the four harmony readers from
    /// the last frame. In Choir mode, only the lead slot changes. Only the
    /// telemetry reads them.
    voice_ratios: [Q14; HARMONY_VOICES + 1],
    /// The signed history frame that the formant model used for the last
    /// frame. The audio path sets it, and only the formant envelope reads it.
    formant_position: i32,
    /// Two cascaded anti-alias filter sections. If the shift is not upward,
    /// they have coefficients of 1.0 and zero.
    post: [Biquad; 2],
    /// The current smoothed Q14 correction ratio. It does not include the
    /// global shift or the voice ratios.
    correction: Q14,
    /// The Q14 half-life coefficient for correction updates. These updates
    /// usually occur every five frames.
    correction_decay: Q14,
    /// If `true`, the current correction target and octave are selected.
    scale_initialized: bool,
    /// The index of the active target. It is valid only if the correction
    /// table is initialized and not empty.
    scale_index: usize,
    /// The signed power-of-two exponent that normalizes the input period
    /// into the range of the correction table.
    octave: i32,
    /// The Q14 global pitch multiplier.
    shift: Q14,
    /// The plain integer modulation depth. It is the public control times
    /// two, truncated.
    modulation: i32,
    /// The Q14 phase in the cosine cycle. An audio reset keeps it.
    modulation_phase: Q14,
    /// The Q8 weight of the delayed raw audio. The wet weight is 1.0 minus
    /// this weight.
    dry_mix: Q8,
    /// The previous position of the lead reader. The crossfade after a jump
    /// follows this position.
    old_position: Q14,
    /// The Q14 weight of the new lead reader. After a jump, it increases
    /// toward 1.0.
    fade: Q14,
    /// The Q6 period that the code selected last for a voice. The modulation
    /// uses it.
    current_output_period: Q6,
    /// The control counter for Choir and Harmony. At the value CONTROL_DUE,
    /// the next eligible step runs.
    control_counter: i32,
    /// The Q14 smoothing coefficient for the random drift of secondary choir
    /// voices.
    drift_decay: Q14,
    /// The Q14 weight of new drift, from the choir detune and the drift
    /// decay.
    drift_amount: Q14,
    /// The previous Q14 synthesized output for each channel. The glottal
    /// envelope uses it.
    previous_output: [Q14; 2],
}

impl Resynthesis {
    /// Creates the state from a configuration that the public API validated.
    pub(crate) fn new(rate: SampleRate, config: ResynthesisConfig) -> Self {
        let mut result = Self {
            rate,
            config,
            pitch: PitchTracker::new(rate, config.pitch),
            raw: prepare_history(Q14::ZERO),
            write: 0,
            cosine: {
                let mut phase = 0.0_f64;
                std::array::from_fn(|_| {
                    let value = Q14::from_f64(phase.cos());
                    phase += std::f64::consts::PI / 64.0;
                    value
                })
            },
            formant: Formant::new(rate, config.formant),
            harmony: HarmonyState::new(rate, config.harmony),
            scale: {
                // The initial chromatic table uses 44.1 kHz periods at the two
                // rates. If the notes load again, the periods for the
                // configured rate become active.
                let mut scale = CorrectionTable::new(SampleRate::Hz44100, NoteMask::default());
                scale.set_sample_rate(rate);
                if config.correction.notes != NoteMask::default() {
                    scale.set_notes(config.correction.notes);
                }
                scale
            },
            filtered: prepare_history(Q14::ZERO),
            lead: prepare_history(Q14::ZERO),
            periods: prepare_history(Q6::ZERO),
            previous_periods: prepare_history(Q6::ZERO),
            ratios: prepare_history(Q14::ONE),
            overlap: [Q14::ZERO; OVERLAP_FRAMES],
            overlap_index: 0,
            delays: [0; 16],
            positions: [Q14::ZERO; 16],
            drift: [Q14::ZERO; 16],
            drift_ratio: [Q14::ONE; 16],
            pan: [[Q14::ONE; 2]; 16],
            harmony_pan: [[Q14::ZERO; 2]; HARMONY_VOICES],
            harmony_ratios: [Q14::ONE; HARMONY_VOICES],
            harmony_gains: [Q12::ZERO; HARMONY_VOICES],
            voice_ratios: [Q14::ONE; HARMONY_VOICES + 1],
            formant_position: 0,
            post: [Biquad::new(), Biquad::new()],
            correction: Q14::ONE,
            correction_decay: Q14::ZERO,
            scale_initialized: false,
            scale_index: 0,
            octave: 0,
            shift: Q14::ONE,
            modulation: 0,
            modulation_phase: Q14::ZERO,
            dry_mix: Q8::ZERO,
            old_position: position_of(-READER_LAG_FRAMES),
            fade: Q14::ONE,
            current_output_period: Q6::ZERO,
            control_counter: CONTROL_DUE,
            drift_decay: Q14::ZERO,
            drift_amount: Q14::ZERO,
            previous_output: [Q14::ZERO; 2],
        };
        result.configure_coefficients();
        result.configure_shift_filters();
        result.reset();
        result
    }

    /// Quantizes the controls and calculates the decay coefficients and the
    /// voice pan gains again.
    ///
    /// The coefficients are the correction decay, the drift decay and the
    /// drift amount. The histories and the controller phase do not change.
    /// The harmony level is part of the stereo pan gains. Thus, the path
    /// without pan never applies that control.
    fn configure_coefficients(&mut self) {
        self.shift = Q14::from_f64(self.config.pitch_ratio as f64);
        self.modulation = (self.config.harmonic_modulation as f64 / 8.0 * 16.0) as i32;
        self.dry_mix = Q8::from_f64((self.config.dry_mix * 100.0) as f64 / 100.0);
        self.correction_decay = if self.config.correction.retune_ms == 0.0 {
            Q14::ZERO
        } else {
            let t = self.config.correction.retune_ms / 1000.0;
            Q14::from_f64(0.5_f64.powf(1.0 / ((self.rate.hz() / 5.0) * t) as f64))
        };
        let base = (self.rate.hz() / 100.0) as f64 * 0.2;
        let decay = (-std::f64::consts::LN_2 / base).exp();
        self.drift_decay = Q14::from_f64(decay);
        self.drift_amount = Q14::from_f64((1.0 - decay) * 0.008 * self.config.choir_detune as f64);
        for voice in 0..self.config.choir_voices as usize {
            if self.config.choir_voices > 1 {
                let normalized =
                    (voice as f64 + voice as f64) / (self.config.choir_voices as f64 - 1.0) - 1.0;
                let angle = 0.785398163565
                    - (self.config.choir_spread_percent as f64 * 0.785398163565 * 0.01)
                        * normalized;
                self.pan[voice] = [Q14::from_f64(angle.cos()), Q14::from_f64(angle.sin())];
            } else {
                self.pan[voice] = [Q14::ONE; 2];
            }
        }
        for (voice, fraction) in [0.3333333333333333, 0.6666666666666666, 0.0, 1.0]
            .into_iter()
            .enumerate()
        {
            let angle: f64 = 1.57079632713 * fraction;
            let gain = self.config.harmony_gain as f64;
            self.harmony_pan[voice] = [
                Q14::from_f64(gain * angle.cos()),
                Q14::from_f64(gain * angle.sin()),
            ];
        }
    }

    /// Prepares two low-pass sections if the global shift is upward. If not,
    /// sets coefficients of 1.0 and zero.
    ///
    /// A change of the coefficients keeps the filter histories. The code
    /// truncates the numerator of the cutoff before it divides by the pitch
    /// ratio.
    fn configure_shift_filters(&mut self) {
        for filter in &mut self.post {
            if self.config.pitch_ratio > 1.0 {
                let numerator = (self.rate.hz() as f64 * 0.49) as i32;
                let cutoff = numerator as f32 / self.config.pitch_ratio;
                filter.set_design(Design::low_pass(self.rate.hz(), cutoff, 0.707));
            } else {
                filter.bypass();
            }
        }
    }

    /// Scales the fixed choir frame offsets and puts all readers behind the
    /// writer.
    ///
    /// Harmony uses offsets of zero. This function moves the readers. It does
    /// not clear the audio histories, the drift or the overlap buffer.
    fn create_delays(&mut self) {
        let parameter = if self.config.mode == MultiVoiceMode::Choir {
            self.config.choir_delay
        } else {
            0.0
        };
        for (voice, seed) in SEEDS.iter().enumerate() {
            self.delays[voice] = (*seed as f64 * (parameter as f64 * 0.005)) as i32;
            self.positions[voice] =
                position_of(self.write as i32 - READER_LAG_FRAMES - self.delays[voice]);
        }
    }

    /// Applies a configuration that passed the full validation before.
    pub(crate) fn apply_config(&mut self, config: ResynthesisConfig) {
        let old = self.config;
        self.config = config;
        self.pitch.set_config(config.pitch);
        self.formant.set_config(config.formant);
        self.harmony.apply_config(config.harmony);
        if old.correction.notes != config.correction.notes
            || old.correction.enabled != config.correction.enabled
        {
            self.scale.set_notes(config.correction.notes);
            self.scale_initialized = false;
        }
        if old.pitch_ratio != config.pitch_ratio {
            self.configure_shift_filters();
        }
        if old.mode != config.mode || old.choir_delay != config.choir_delay {
            self.create_delays();
        }
        if old.choir_voices < config.choir_voices {
            for voice in old.choir_voices as usize..config.choir_voices as usize {
                self.positions[voice] = self.positions[0];
            }
        }
        self.configure_coefficients();
        if old.stereo_output != config.stereo_output {
            // A stereo change resets the audio state. The correction keyboard
            // keeps its held keys.
            self.reset_audio();
        }
    }

    /// Releases the correction keys 0..=126 and resets the audio state.
    ///
    /// The active correction table, key 127 and the items that
    /// `reset_audio` keeps do not change.
    pub(crate) fn reset(&mut self) {
        self.scale.reset();
        self.reset_audio();
    }

    /// Clears the audio and period histories, the tracker, the formant
    /// synthesis and the filter history. Then initializes the readers and
    /// the correction again.
    ///
    /// These items do not change:
    /// - The correction keyboard.
    /// - The active note table.
    /// - The random state that the caller holds.
    /// - The choir drift.
    /// - The modulation phase.
    /// - The control counter.
    /// - The harmony release envelopes and the last harmony gains.
    fn reset_audio(&mut self) {
        self.pitch.reset();
        self.raw.fill(Q14::ZERO);
        self.write = 0;
        self.formant.reset();
        self.harmony.reset();
        self.filtered.fill(Q14::ZERO);
        self.lead.fill(Q14::ZERO);
        self.periods.fill(Q6::ZERO);
        self.previous_periods.fill(Q6::ZERO);
        self.ratios.fill(Q14::ONE);
        self.overlap.fill(Q14::ZERO);
        self.overlap_index = 0;
        self.previous_output = [Q14::ZERO; 2];
        for filter in &mut self.post {
            filter.reset();
        }
        self.create_delays();
        self.drift_ratio.fill(Q14::ONE);
        self.old_position = self.positions[0];
        self.fade = Q14::ONE;
        self.harmony_ratios.fill(Q14::ONE);
        self.voice_ratios.fill(Q14::ONE);
        self.formant_position = 0;
        self.current_output_period = Q6::ZERO;
        self.correction = Q14::ONE;
        self.scale_initialized = false;
    }

    /// Applies a MIDI event that the API or the block code checked before.
    pub(crate) fn apply_midi(&mut self, event: MidiEvent) {
        if self.harmony.apply_validated_midi(event) {
            self.control_counter = CONTROL_DUE;
        }
    }

    /// Applies a correction MIDI event that the API or the block code checked
    /// before.
    pub(crate) fn apply_correction_midi(&mut self, event: MidiEvent) {
        self.scale.apply_validated_midi(event);
        self.scale_initialized = false;
    }

    /// Returns the finite number of frames for a drain of resynthesis.
    ///
    /// The number is the sum of these items:
    /// - Two full histories.
    /// - Two seconds, if formants are enabled.
    /// - One second, if the global shift is upward.
    ///
    /// This is a configured limit for the truncation. It does not find
    /// actual silence.
    pub(crate) fn tail_frames(&self) -> usize {
        2 * HISTORY
            + if self.config.formant.enabled {
                self.rate.as_u32() as usize * 2
            } else {
                0
            }
            + if self.config.pitch_ratio > 1.0 {
                self.rate.as_u32() as usize
            } else {
                0
            }
    }
    /// Converts the latest period to samples and hertz and applies the
    /// current report limits.
    ///
    /// The analysis does not advance, and the correction does not change.
    pub(crate) fn estimate(&self) -> PitchEstimate {
        let period = (self.periods[self.write].to_bits() as f64 / 64.0) as f32;
        self.config.pitch.report(PitchEstimate {
            period_samples: period,
            frequency_hz: if period > 0.0 {
                self.rate.hz() / period
            } else {
                0.0
            },
            voiced: period > 0.0,
        })
    }
    /// Returns the current Q14 correction factor as f32. The result does not
    /// include other ratios.
    pub(crate) fn resample_ratio(&self) -> f32 {
        (self.correction.to_bits() as f64 / 16384.0) as f32
    }
    /// Returns the selected correction pitch class in the octave that is
    /// nearest to the reported input.
    ///
    /// If correction is disabled, the active table is empty or no target is
    /// selected, returns the fractional MIDI note of the input. If the input
    /// is not reported as voiced, returns `None`.
    pub(crate) fn target_note(&self) -> Option<f32> {
        let midi = self.estimate().midi_note()?;
        if self.config.correction.enabled && self.scale_initialized && self.scale.count != 0 {
            let class = self.scale.notes[self.scale_index] as f32;
            Some(class + ((midi - class) / 12.0).round() * 12.0)
        } else {
            Some(midi)
        }
    }
    /// Returns how periodic the tracked input is, from 0 to 1. Refer to
    /// `PitchTracker::clarity`.
    pub(crate) fn clarity(&self) -> f32 {
        self.pitch.clarity()
    }
    /// Returns `true` if the pitch tracker follows a period, and `false`
    /// during acquisition.
    pub(crate) fn is_tracking(&self) -> bool {
        !self.pitch.is_detecting()
    }
    /// Writes the effective ratio and the linear gain of each voice from the
    /// last frame, and returns the number of voices.
    ///
    /// Slot zero is the lead. In Harmony mode, slots 1 to 4 are the harmony
    /// voices, and the gain is the envelope of the harmony controller. In
    /// Choir mode, the slots after the lead are the secondary voices with a
    /// gain of 1. Each ratio includes the correction, the global shift and
    /// the ratio of the voice. The entries after the count do not change.
    pub(crate) fn voices(&self, ratios: &mut [f32; 16], gains: &mut [f32; 16]) -> u8 {
        let lead = (self.voice_ratios[0].to_bits() as f64 / 16384.0) as f32;
        ratios[0] = lead;
        gains[0] = 1.0;
        if self.config.mode == MultiVoiceMode::Choir {
            let count = self.config.choir_voices as usize;
            for voice in 1..count {
                ratios[voice] =
                    (lead as f64 * self.drift_ratio[voice].to_bits() as f64 / 16384.0) as f32;
                gains[voice] = 1.0;
            }
            count as u8
        } else {
            for voice in 0..HARMONY_VOICES {
                ratios[voice + 1] =
                    (self.voice_ratios[voice + 1].to_bits() as f64 / 16384.0) as f32;
                gains[voice + 1] = (self.harmony_gains[voice].to_bits() as f64 / 4096.0) as f32;
            }
            (HARMONY_VOICES + 1) as u8
        }
    }
    /// Returns the wrapping count of formant model designs.
    pub(crate) fn formant_generation(&self) -> u32 {
        self.formant.generation()
    }
    /// Writes the formant envelopes of the model that the audio used for the
    /// last frame. Refer to `Formant::envelope`.
    ///
    /// Returns `false` and fills the slices with zeros if the formant stage
    /// is disabled or has no model. The slices must have the same length.
    pub(crate) fn formant_envelope(
        &self,
        max_frequency_hz: f32,
        input: &mut [f32],
        output: &mut [f32],
    ) -> bool {
        if !self.config.formant.enabled {
            input.fill(0.0);
            output.fill(0.0);
            return false;
        }
        self.formant
            .envelope(self.formant_position, max_frequency_hz, input, output)
    }
    /// Selects a target in the period domain and smooths the Q14 correction
    /// ratio.
    ///
    /// The Q6 input period must be a usable tracking result. The target
    /// search moves through the midpoints, also across octave limits. The
    /// global shift is part of the normalization.
    ///
    /// If correction is disabled or the active table is empty, the ratio moves
    /// toward 1.0. If the target selection starts again and the shifted
    /// period is not positive, the ratio becomes 1.0 immediately. A
    /// multiplication by fixed-point 1.0 still wraps. Do not remove it.
    fn update_correction(&mut self, period: Q6) {
        let mut target_ratio = Q14::ONE;
        if self.config.correction.enabled && self.scale.count != 0 {
            let count = self.scale.count;
            // The correction table holds periods normalized to one octave.
            // They use the same units as the shifted target period below.
            let table_period = |index: usize| Q6::from_bits(self.scale.midpoints[index]);
            let mut target: Q6;
            if !self.scale_initialized {
                self.scale_initialized = true;
                self.scale_index = 0;
                self.octave = 0;
                target = self.shift.mul(period);
                if target <= Q6::ZERO {
                    self.correction = Q14::ONE;
                    return;
                }
                while table_period(1) > target {
                    self.octave += 1;
                    target = target << 1;
                    if self.octave >= 30 || target <= Q6::ZERO {
                        break;
                    }
                }
            }
            target = self.shift.mul(if self.octave >= 0 {
                period << self.octave as u32
            } else {
                period >> (-self.octave) as u32
            });
            // The comparisons multiply by 1.0 to keep the wrap of the product.
            while table_period(self.scale_index + 1).mul::<14, 6>(Q14::ONE) < target {
                self.scale_index += 1;
                if self.scale_index == count {
                    self.scale_index = 0;
                    target = target >> 1;
                    self.octave -= 1;
                }
            }
            while target.mul::<14, 6>(Q14::ONE) < table_period(self.scale_index) {
                if self.scale_index == 0 {
                    self.scale_index = count - 1;
                    target = target << 1;
                    self.octave += 1;
                } else {
                    self.scale_index -= 1;
                }
            }
            let denominator = Q6::from_bits(self.scale.periods[self.scale_index]);
            if denominator != Q6::ZERO {
                target_ratio = target.div(denominator);
            }
        }
        self.correction = self.correction_decay.mul(self.correction - target_ratio) + target_ratio;
    }

    /// Selects the period and the correction.
    ///
    /// If the delayed audio is voiced, the function uses it. If not, it uses
    /// the current audio. If that is also unvoiced, it uses the last usable
    /// delayed period. The indices must be less than HISTORY.
    fn period_for(&self, delayed: usize, current: usize) -> (Q6, Q14) {
        if self.periods[delayed] != Q6::ZERO {
            (self.periods[delayed], self.ratios[delayed])
        } else if self.periods[current] != Q6::ZERO {
            (self.periods[current], self.ratios[current])
        } else {
            (self.previous_periods[delayed], self.ratios[delayed])
        }
    }

    /// Advances one Q14 reader by its ratio and keeps it near the writer.
    ///
    /// If the reader gets to one of the two limits, it jumps by one Q6 pitch
    /// period. `offset` is an integer voice delay. `voice` must identify an
    /// allocated reader slot. If `fade` is `true` and the reader jumps, the
    /// function saves the old position and starts the crossfade again. Only
    /// the choir lead sets `fade`.
    fn move_reader(
        &mut self,
        voice: usize,
        ratio: Q14,
        period: Q6,
        offset: i32,
        fade: bool,
    ) -> Q14 {
        let mut position = self.positions[voice] + ratio;
        let write = self.write as i32;
        let upper = position_of(write - 40 - offset);
        let span = (period.to_bits() >> 6).wrapping_mul(12).wrapping_div(10);
        let lower = position_of(
            write
                .wrapping_sub(span)
                .wrapping_sub(40)
                .wrapping_sub(offset),
        );
        if position >= upper {
            if fade {
                self.old_position = position;
                self.fade = Q14::ZERO;
            }
            position = position - period.rescale();
        } else if position <= lower {
            if fade {
                self.old_position = position;
                self.fade = Q14::ZERO;
            }
            position = position + period.rescale();
        }
        self.positions[voice] = position;
        position
    }

    /// Renders the choir lead before a secondary voice reads its history.
    ///
    /// The function does these steps:
    /// 1. It selects the period and the ratio.
    /// 2. It moves the reader and applies the crossfade.
    /// 3. If formants are enabled, it applies them and the glottal envelope.
    /// 4. It stores the Q14 result in the lead history.
    ///
    /// Returns the lead sample and its effective ratio, for the modulation
    /// state that follows.
    fn render_lead(&mut self, delayed: usize, numeric: &mut NumericContext) -> (Q14, Q14) {
        let write = self.write;
        let delay = ring(delayed as i32 - self.delays[0]);
        let (period, mut ratio) = self.period_for(delay, write);
        self.current_output_period = period;
        ratio = ratio.mul::<14, 14>(self.drift_ratio[0]).mul(self.shift);
        if period == Q6::ZERO {
            ratio = Q14::ONE;
        }
        if ratio != Q14::ZERO {
            self.periods[delay] = period.div(ratio);
        }
        self.old_position = self.old_position + ratio;
        let position = self.move_reader(0, ratio, period, self.delays[0], true);
        let (mut value, center) = history::interpolation(&self.filtered, position);
        if self.fade < Q14::ONE {
            let (old, _) = history::interpolation(&self.filtered, self.old_position);
            value = value.mul(self.fade) + old.mul(Q14::ONE - self.fade);
            self.fade = (self.fade + FADE_STEP).min(Q14::ONE);
        }
        if self.config.formant.enabled {
            self.formant_position = center - FORMANT_OFFSET_FRAMES;
            value = Q14::from_bits(self.formant.forward(
                value.to_bits(),
                self.formant_position,
                numeric,
            ));
            value = Q14::from_bits(
                self.formant
                    .model_glottal(value.to_bits(), self.previous_output[0].to_bits()),
            );
            self.previous_output[0] = value;
        }
        self.lead[write] = value;
        (value, ratio)
    }

    /// Renders one mono PCM input frame into stereo Q14 samples.
    ///
    /// The order of calls follows the module pipeline:
    /// 1. Analysis and control.
    /// 2. The delayed source.
    /// 3. The voice readers.
    /// 4. Formant synthesis.
    /// 5. Modulation.
    /// 6. The dry and wet mix.
    ///
    /// When the writer wraps, the code changes the coordinates of the active
    /// readers. Their relative positions do not change. MIDI can make a
    /// harmony control step occur. The choir control advances only if
    /// tracking was active before this frame.
    pub(crate) fn process_sample(&mut self, input: i16, numeric: &mut NumericContext) -> [Q14; 2] {
        let sample = Q14::from_bits(i32::from(input) >> 1);
        self.write += 1;
        if self.write == HISTORY {
            self.write = 0;
        }
        self.raw[self.write] = sample;
        let was_tracking = !self.pitch.is_detecting();
        let step = self.pitch.process_sample(sample.to_bits());
        let write = self.write;
        // Readers have signed fractional coordinates. When the writer wraps,
        // move the origin of these coordinates with it. Do not apply a modulo
        // to each fractional position. This keeps the distances that the jump
        // detection uses.
        if write == 0 {
            let voices = if self.config.mode == MultiVoiceMode::Choir {
                self.config.choir_voices as usize
            } else {
                5
            };
            for voice in 0..voices {
                self.positions[voice] = self.positions[voice] - position_of(HISTORY as i32);
            }
            self.old_position = self.old_position - position_of(HISTORY as i32);
        }
        if step.restarted {
            self.correction = Q14::ONE;
            self.scale_initialized = false;
        }
        if step.acquired {
            self.formant.detection_mode_off();
        }
        if step.tracking_update {
            self.update_correction(step.period);
        }
        // Use the state from before this sample. If acquisition occurs on
        // this frame, the choir drift counter starts on the next frame. Read
        // random values in voice order, so that the stream that aspiration
        // also uses stays aligned.
        if was_tracking && self.config.mode == MultiVoiceMode::Choir {
            let counter = self.control_counter;
            self.control_counter += 1;
            if counter == CONTROL_DUE {
                self.control_counter = 0;
                for voice in 1..self.config.choir_voices as usize {
                    let random = Q14::from_bits((numeric.random.next_u31() >> 17) - 8192);
                    self.drift[voice] = (self.drift_decay.mul(self.drift[voice])
                        + self.drift_amount.mul(random))
                    .clamp(-DRIFT_LIMIT, DRIFT_LIMIT);
                    self.drift_ratio[voice] = self.drift[voice] + Q14::ONE;
                }
                self.drift_ratio[0] = Q14::ONE;
            }
        }
        self.periods[write] = step.period;
        self.previous_periods[write] = step.previous_period;
        self.ratios[write] = self.correction;
        self.overlap_index += 1;
        if self.overlap_index == OVERLAP_FRAMES {
            self.overlap_index = 0;
        }
        self.overlap[self.overlap_index] = if self.config.formant.enabled {
            let previous = self.raw[ring(write as i32 - 1)];
            Q14::from_bits(self.formant.process_analysis(
                sample.to_bits(),
                previous.to_bits(),
                self.periods.as_slice(),
                write,
                step.previous_period.to_bits(),
            ))
        } else {
            self.raw[ring(write as i32 - ANALYSIS_FRAMES as i32)]
        };
        // The inverse analysis adds a delay of ANALYSIS_FRAMES. Read ahead by
        // the same number of frames in the overlap ring. Then the total delay
        // of the source is OVERLAP_FRAMES in the two formant modes.
        let overlap_read = (self.overlap_index + ANALYSIS_FRAMES) % OVERLAP_FRAMES;
        let mut filtered = self.overlap[overlap_read];
        for filter in &mut self.post {
            filtered = filter.process_sample(filtered);
        }
        self.filtered[write] = filtered;
        let delayed = ring(write as i32 - OVERLAP_FRAMES as i32);
        let dry = self.raw[delayed];
        let mut mix = [Q14::ZERO; 2];
        let mut final_ratio = Q14::ONE;
        if self.config.mode == MultiVoiceMode::Choir {
            let voice_count = self.config.choir_voices as usize;
            // Secondary voices read the lead history written by voice zero.
            {
                let (value, ratio) = self.render_lead(delayed, numeric);
                final_ratio = ratio;
                self.voice_ratios[0] = ratio;
                for (channel, sum) in mix.iter_mut().enumerate() {
                    *sum = *sum
                        + if self.config.stereo_output {
                            value.mul(self.pan[0][channel])
                        } else {
                            value
                        };
                }
            }
            {
                let mut voice = 1;
                while voice + 4 <= voice_count {
                    let mut positions = [Q14::ZERO; 4];
                    // After the lead update, set the reader positions in
                    // voice order.
                    for (lane, position) in positions.iter_mut().enumerate() {
                        let current_voice = voice + lane;
                        let delay = ring(delayed as i32 - self.delays[current_voice]);
                        let (period, _) = self.period_for(delay, write);
                        self.current_output_period = period;
                        let ratio = self.drift_ratio[current_voice];
                        final_ratio = ratio;
                        *position = self.move_reader(
                            current_voice,
                            ratio,
                            period,
                            self.delays[current_voice],
                            false,
                        );
                    }
                    let values =
                        positions.map(|position| history::interpolation(&self.lead, position).0);
                    // Keep the voice and channel order of each wrapping sum.
                    for (lane, value) in values.into_iter().enumerate() {
                        let current_voice = voice + lane;
                        for (channel, sum) in mix.iter_mut().enumerate() {
                            *sum = *sum
                                + if self.config.stereo_output {
                                    value.mul(self.pan[current_voice][channel])
                                } else {
                                    value
                                };
                        }
                    }
                    voice += 4;
                }
                // The zero to three secondary readers that are left use
                // scalar code.
                for voice in voice..voice_count {
                    let delay = ring(delayed as i32 - self.delays[voice]);
                    let (period, _) = self.period_for(delay, write);
                    self.current_output_period = period;
                    let ratio = self.drift_ratio[voice];
                    final_ratio = ratio;
                    let position =
                        self.move_reader(voice, ratio, period, self.delays[voice], false);
                    let (value, _) = history::interpolation(&self.lead, position);
                    for (channel, sum) in mix.iter_mut().enumerate() {
                        *sum = *sum
                            + if self.config.stereo_output {
                                value.mul(self.pan[voice][channel])
                            } else {
                                value
                            };
                    }
                }
            }
            // The multiplication by 1.0 keeps the wrap of the product.
            for sum in &mut mix {
                *sum = Q14::ONE.mul(*sum);
            }
        } else {
            let (period, ratio) = self.period_for(delayed, write);
            self.current_output_period = period;
            let counter = self.control_counter;
            self.control_counter += 1;
            if counter == CONTROL_DUE {
                self.control_counter = 0;
                for voice in 0..HARMONY_VOICES {
                    let corrected_period = if ratio == Q14::ZERO {
                        Q6::ZERO
                    } else {
                        period.div(ratio)
                    };
                    let (ratio, gain) = self.harmony.advance_voice(corrected_period, voice);
                    self.harmony_ratios[voice] = ratio;
                    self.harmony_gains[voice] = gain;
                }
            }
            for slot in 0..5 {
                let mut current: Q14 = if slot == 0 {
                    ratio
                } else {
                    self.harmony_ratios[slot - 1].mul(ratio)
                };
                current = self.shift.mul(current);
                if period == Q6::ZERO {
                    current = Q14::ONE;
                }
                final_ratio = current;
                self.voice_ratios[slot] = current;
                let position = self.move_reader(slot, current, period, 0, false);
                let (mut value, _) = history::interpolation(&self.filtered, position);
                if slot > 0 {
                    value = value.mul(self.harmony_gains[slot - 1]);
                }
                for (channel, sum) in mix.iter_mut().enumerate() {
                    *sum = *sum
                        + if self.config.stereo_output && slot > 0 {
                            value.mul(self.harmony_pan[slot - 1][channel])
                        } else {
                            value
                        };
                }
            }
            if self.config.formant.enabled {
                self.formant_position = (self.positions[0].to_bits() >> 14) - FORMANT_OFFSET_FRAMES;
                mix = self
                    .formant
                    .forward_stereo(mix.map(Q14::to_bits), self.formant_position, numeric)
                    .map(Q14::from_bits);
                for (channel, value) in mix.iter_mut().enumerate() {
                    *value =
                        Q14::from_bits(self.formant.model_glottal(
                            value.to_bits(),
                            self.previous_output[channel].to_bits(),
                        ));
                    self.previous_output[channel] = *value;
                }
            }
        }
        if self.modulation != 0 {
            if self.current_output_period != Q6::ZERO {
                // The phase advances by depth x 4 x ratio / period, in Q14
                // cycles.
                let step = self
                    .modulation
                    .wrapping_mul(4)
                    .wrapping_mul(final_ratio.to_bits())
                    .wrapping_div(self.current_output_period.to_bits());
                self.modulation_phase = self.modulation_phase + Q14::from_bits(step);
                if self.modulation_phase > Q14::ONE {
                    self.modulation_phase = self.modulation_phase - Q14::ONE;
                }
            } else {
                self.modulation_phase =
                    Q14::from_bits((self.modulation_phase.to_bits() as f64 * 0.999) as i32);
            }
            let scaled = self.modulation_phase.to_bits().wrapping_mul(128);
            let center = scaled.wrapping_add(8192) >> 14;
            let frac = Q14::from_bits(scaled.wrapping_sub(center.wrapping_shl(14)));
            let index = center.rem_euclid(128) as usize;
            let left = self.cosine[(index + 127) % 128];
            let middle = self.cosine[index];
            let right = self.cosine[(index + 1) % 128];
            let cosine = middle
                + ((((left + right) >> 1) - middle).mul::<14, 14>(frac) + ((right - left) >> 1))
                    .mul(frac);
            for sum in &mut mix {
                *sum = sum.mul(cosine);
            }
        }
        // The dry branch is the delayed raw input. It bypasses the shift
        // filters, the voice readers, the formant stages and the harmonic
        // modulation. Keep the two products in Q22. Rescale their wrapping sum
        // to Q14 one time only.
        let wet = Q8::ONE - self.dry_mix;
        for sum in &mut mix {
            *sum = (dry.mul::<8, 22>(self.dry_mix) + sum.mul::<8, 22>(wet)).rescale();
        }
        mix
    }
}
