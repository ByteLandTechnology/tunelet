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
//! - The order of the fixed-point arithmetic.

pub(crate) mod history;

use crate::error::{check_count, check_range};
use crate::filter::{Biquad, Design};
use crate::formant::{Formant, FormantConfig, ANALYSIS_FRAMES};
use crate::harmony::{HarmonyConfig, HarmonyState, HARMONY_VOICES};
use crate::midi::MidiEvent;
use crate::numeric::fixed::{Q12, Q14, Q6, Q8};
use crate::numeric::{clamp_i32, quadratic, ring, NumericContext, HISTORY};
use crate::pitch::tracker::PitchTracker;
use crate::pitch::{PitchConfig, PitchEstimate};
use crate::scale::{CorrectionMidiMode, CorrectionTable, NoteMask};
use crate::{Error, SampleRate};
use history::{clear_dirty_ring, prepare_history, record_history};

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
/// Correction is enabled by default, with all chromatic notes, a retune time
/// of zero, zero humanize, zero flex-tune and momentary MIDI mode. Target
/// selection and the smoothing of the correction ratio occur at each update of
/// the pitch tracking. When a period is tracked, this update usually occurs
/// every five frames. If the active note table is empty or correction is
/// disabled, the ratio moves toward 1.0 with `retune_ms`.
///
/// Correction MIDI replaces the active note table. It does not change this
/// configuration. In [`CorrectionMidiMode::Momentary`], when all keys in
/// 0..=127 release, the configured `notes` load again. In
/// [`CorrectionMidiMode::Latch`], a chord stays active until
/// [`MidiEvent::AllNotesOff`], a reset or a scale change. A change to `notes`
/// or to `enabled` also loads the configured mask again. Reset releases the
/// correction keys 0..=127 and clears any latched chord.
///
/// Construction loads the configured `notes` mask with the periods for the
/// prepared sample rate. Later changes to `notes`, to `enabled` or from
/// correction MIDI also use the periods for the prepared sample rate.
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
    /// All boolean masks are valid. Correction MIDI can replace the active
    /// table. In [`CorrectionMidiMode::Momentary`], when all keys release,
    /// this mask loads again. In [`CorrectionMidiMode::Latch`], All Notes Off,
    /// a reset or a change to this mask or `enabled` loads it again.
    pub notes: NoteMask,
    /// The half-life of the smoothing in milliseconds, 0..=400. The default
    /// is 0.
    ///
    /// If the value is zero, the ratio changes to the target at the next
    /// tracking update. A value that is not zero is a half-life. It is not a
    /// fixed time to get exactly to the target.
    pub retune_ms: f32,
    /// The relaxation on sustained notes without a unit, 0..=1. The default
    /// is 0.
    ///
    /// While the tracked pitch stays in one target bin, the half-life
    /// increases smoothly toward `retune_ms + (400 - retune_ms) * humanize`
    /// milliseconds. If the target bin, the note table or `pitch_ratio`
    /// changes, or if the pitch tracker starts again, the half-life returns to
    /// `retune_ms`. Zero disables this relaxation. In JavaScript, the field is
    /// optional and an absent or `undefined` value gives 0.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        serde(default, deserialize_with = "deserialize_zero_f32"),
        tsify(optional, type = "number | undefined")
    )]
    pub humanize: f32,
    /// The outer soft-zone width of each target bin without a unit, 0..=1.
    /// The default is 0.
    ///
    /// Pitches in the inner `1 - flex_tune` fraction of the half-bin around a
    /// target period get the full pull toward the target. In the outer
    /// `flex_tune` fraction, the pull decreases to zero at the midpoint
    /// boundary. Zero applies full pull across the whole bin, and 1 softens
    /// pull from the target outward. In JavaScript, the field is optional and
    /// an absent or `undefined` value gives 0.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        serde(default, deserialize_with = "deserialize_zero_f32"),
        tsify(optional, type = "number | undefined")
    )]
    pub flex_tune: f32,
    /// How [`crate::ControlEvent::CorrectionMidi`] keeps or releases notes.
    ///
    /// The default is [`CorrectionMidiMode::Momentary`]. In JavaScript, the
    /// field is optional and an absent or `undefined` value gives the default.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        serde(default, deserialize_with = "crate::harmony::deserialize_default"),
        tsify(optional, type = "CorrectionMidiMode | undefined")
    )]
    pub midi_mode: CorrectionMidiMode,
}

/// Decodes an optional correction control and maps an absent value to 0.
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
fn deserialize_zero_f32<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<f32, D::Error> {
    use serde::Deserialize;
    Ok(Option::<f32>::deserialize(deserializer)?.unwrap_or(0.0))
}

impl Default for CorrectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            notes: NoteMask::default(),
            retune_ms: 0.0,
            humanize: 0.0,
            flex_tune: 0.0,
            midi_mode: CorrectionMidiMode::default(),
        }
    }
}

impl CorrectionConfig {
    /// Validates all fields, also if correction is disabled.
    ///
    /// All values of `enabled`, `notes` and `midi_mode` are valid. `retune_ms`,
    /// `humanize` and `flex_tune` must be finite and in their documented
    /// inclusive ranges. Validation does not clamp values. It does not change
    /// the state of the processor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`], with the dotted path of the field and
    /// the problem, if a field is not valid.
    pub fn validate(self) -> Result<(), Error> {
        validate_retune_ms(self.retune_ms)?;
        validate_humanize(self.humanize)?;
        validate_flex_tune(self.flex_tune)
    }
}

/// Checks the range of the retune half-life.
pub(crate) fn validate_retune_ms(retune_ms: f32) -> Result<(), Error> {
    check_range("resynthesis.correction.retune_ms", retune_ms, 0.0, 400.0)
}

/// Checks the range of the relaxation on sustained notes.
pub(crate) fn validate_humanize(humanize: f32) -> Result<(), Error> {
    check_range("resynthesis.correction.humanize", humanize, 0.0, 1.0)
}

/// Checks the range of the outer soft-zone width of each target bin.
pub(crate) fn validate_flex_tune(flex_tune: f32) -> Result<(), Error> {
    check_range("resynthesis.correction.flex_tune", flex_tune, 0.0, 1.0)
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
/// The dry path takes the audio after Punch and Warm and delays it by 1,593
/// frames. At unity pitch and zero choir delay, the wet voices have the same
/// nominal delay. Resampled voices and choir delays use different delays.
/// After this stage, the two paths go through Alien, reverb and the final
/// output gain.
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
    /// The linear gain of the four harmony voices, 0..=1. The default is 0.5.
    ///
    /// If `stereo_output` is `true`, the stereo pan gains include this gain.
    /// If `stereo_output` is `false`, this gain scales each harmony voice
    /// directly. It does not scale the lead voice.
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
    /// delay of 1,593 frames. Alien, reverb and the output gain still occur
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
        validate_pitch_ratio(self.pitch_ratio)?;
        validate_choir_detune(self.choir_detune)?;
        validate_choir_delay(self.choir_delay)?;
        validate_choir_spread_percent(self.choir_spread_percent)?;
        validate_harmony_gain(self.harmony_gain)?;
        validate_harmonic_modulation(self.harmonic_modulation)?;
        validate_dry_mix(self.dry_mix)?;
        validate_choir_voices(self.choir_voices)
    }
}

/// Checks the range of the global pitch multiplier.
pub(crate) fn validate_pitch_ratio(pitch_ratio: f32) -> Result<(), Error> {
    check_range("resynthesis.pitch_ratio", pitch_ratio, 0.25, 4.0)
}

/// Checks the range of the choir voice count.
pub(crate) fn validate_choir_voices(choir_voices: u8) -> Result<(), Error> {
    check_count("resynthesis.choir_voices", i64::from(choir_voices), 1, 16)
}

/// Checks the range of the choir detune.
pub(crate) fn validate_choir_detune(choir_detune: f32) -> Result<(), Error> {
    check_range("resynthesis.choir_detune", choir_detune, 10.0, 100.0)
}

/// Checks the range of the choir delay scale.
pub(crate) fn validate_choir_delay(choir_delay: f32) -> Result<(), Error> {
    check_range("resynthesis.choir_delay", choir_delay, 20.0, 200.0)
}

/// Checks the range of the choir stereo width.
pub(crate) fn validate_choir_spread_percent(choir_spread_percent: f32) -> Result<(), Error> {
    check_range(
        "resynthesis.choir_spread_percent",
        choir_spread_percent,
        0.0,
        100.0,
    )
}

/// Checks the range of the linear gain of the harmony voices.
pub(crate) fn validate_harmony_gain(harmony_gain: f32) -> Result<(), Error> {
    check_range("resynthesis.harmony_gain", harmony_gain, 0.0, 1.0)
}

/// Checks the range of the harmonic modulation depth.
pub(crate) fn validate_harmonic_modulation(harmonic_modulation: f32) -> Result<(), Error> {
    check_range(
        "resynthesis.harmonic_modulation",
        harmonic_modulation,
        0.0,
        24.0,
    )
}

/// Checks the range of the resynthesis dry mix.
pub(crate) fn validate_dry_mix(dry_mix: f32) -> Result<(), Error> {
    check_range("resynthesis.dry_mix", dry_mix, 0.0, 1.0)
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

/// The length of the analysis overlap buffer.
const OVERLAP_FRAMES: usize = 1543;
/// The offset from the history position of a reader to its position for
/// formant synthesis.
const FORMANT_OFFSET_FRAMES: i32 = (OVERLAP_FRAMES - ANALYSIS_FRAMES) as i32;
/// The number of frames that a new reader stays behind the write position.
const READER_LAG_FRAMES: i32 = 50;
/// The nominal latency of resynthesis. Each reader moves with the pitch and
/// the delay.
///
/// The dry path also uses `LATENCY_FRAMES`.
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
    /// If stereo pan is off, the Q14 linear gain of the harmony voices.
    harmony_gain: Q14,
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
    /// The Q14 terminal half-life coefficient for `humanize`, to a 400 ms
    /// limit.
    sustained_decay: Q14,
    /// The Q20 retune half-life in milliseconds, for the humanize ramp.
    retune_half_life: i64,
    /// The Q20 sustained half-life in milliseconds, to a 400 ms limit.
    sustained_half_life: i64,
    /// Half of the five-frame period step times `ln(2)`, in Q20 milliseconds.
    decay_half_step: i64,
    /// The Q14 dwell progress in the current target bin, 0..=1.
    dwell: Q14,
    /// The Q14 dwell increment for each five-frame period update.
    dwell_step: Q14,
    /// The Q14 outer soft-zone width of each half-bin, 0..=1.
    flex_tune: Q14,
    /// If `true`, the current correction target and octave are selected.
    scale_initialized: bool,
    /// The number of frames that the histories hold since the last reset.
    written: u16,
    /// The index of the active target. It is valid only if the correction
    /// table is initialized and not empty.
    scale_index: usize,
    /// The signed power-of-two exponent that normalizes the input period
    /// into the range of the correction table.
    octave: i32,
    /// The target index from the previous period update, for dwell resets.
    previous_scale_index: usize,
    /// The octave exponent from the previous period update, for dwell resets.
    previous_octave: i32,
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
            scale: CorrectionTable::new(rate, config.correction.notes),
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
            harmony_gain: Q14::ZERO,
            harmony_pan: [[Q14::ZERO; 2]; HARMONY_VOICES],
            harmony_ratios: [Q14::ONE; HARMONY_VOICES],
            harmony_gains: [Q12::ZERO; HARMONY_VOICES],
            voice_ratios: [Q14::ONE; HARMONY_VOICES + 1],
            formant_position: 0,
            post: [Biquad::new(), Biquad::new()],
            correction: Q14::ONE,
            correction_decay: Q14::ZERO,
            sustained_decay: Q14::ZERO,
            retune_half_life: 0,
            sustained_half_life: 0,
            decay_half_step: 0,
            dwell: Q14::ZERO,
            dwell_step: Q14::ZERO,
            flex_tune: Q14::ZERO,
            scale_initialized: false,
            written: 0,
            scale_index: 0,
            octave: 0,
            previous_scale_index: 0,
            previous_octave: 0,
            shift: Q14::ONE,
            modulation: 0,
            modulation_phase: Q14::ZERO,
            dry_mix: Q8::ZERO,
            old_position: Q14::ZERO,
            fade: Q14::ONE,
            current_output_period: Q6::ZERO,
            control_counter: CONTROL_DUE,
            drift_decay: Q14::ZERO,
            drift_amount: Q14::ZERO,
            previous_output: [Q14::ZERO; 2],
        };
        result.configure_all();
        result.configure_shift_filters();
        result.create_delays();
        result.old_position = result.positions[0];
        result
    }

    /// Quantizes the global pitch multiplier unconditionally.
    fn configure_pitch_ratio(&mut self) {
        self.shift = Q14::from_f64(self.config.pitch_ratio as f64);
    }

    /// Quantizes the harmonic modulation depth unconditionally.
    fn configure_harmonic_modulation(&mut self) {
        self.modulation = (self.config.harmonic_modulation as f64 / 8.0 * 16.0) as i32;
    }

    /// Quantizes the dry mix weight unconditionally.
    fn configure_dry_mix(&mut self) {
        self.dry_mix = Q8::from_f64((self.config.dry_mix * 100.0) as f64 / 100.0);
    }

    /// Calculates the correction and humanize half-life coefficients
    /// unconditionally.
    fn configure_correction_decay(&mut self) {
        const Q20_SCALE: f64 = (1_u32 << 20) as f64;
        self.correction_decay = if self.config.correction.retune_ms == 0.0 {
            Q14::ZERO
        } else {
            let t = self.config.correction.retune_ms / 1000.0;
            Q14::from_f64(0.5_f64.powf(1.0 / ((self.rate.hz() / 5.0) * t) as f64))
        };
        let retune_ms = f64::from(self.config.correction.retune_ms);
        self.retune_half_life = (retune_ms * Q20_SCALE + 0.5) as i64;
        self.decay_half_step =
            (2500.0 * std::f64::consts::LN_2 / f64::from(self.rate.hz()) * Q20_SCALE + 0.5) as i64;
        if self.config.correction.humanize == 0.0 {
            self.sustained_decay = self.correction_decay;
            self.sustained_half_life = self.retune_half_life;
            self.dwell_step = Q14::ZERO;
        } else {
            let sustained_ms =
                retune_ms + (400.0 - retune_ms) * f64::from(self.config.correction.humanize);
            let t = sustained_ms / 1000.0;
            self.sustained_decay =
                Q14::from_f64(0.5_f64.powf(1.0 / ((f64::from(self.rate.hz()) / 5.0) * t)));
            self.sustained_half_life = (sustained_ms * Q20_SCALE + 0.5) as i64;
            self.dwell_step = Q14::from_f64(1.0 / ((f64::from(self.rate.hz()) / 5.0) * 0.18))
                .max(Q14::from_bits(1));
        }
    }

    /// Quantizes the outer soft-zone width of each target bin unconditionally.
    fn configure_flex_tune(&mut self) {
        self.flex_tune = if self.config.correction.flex_tune == 0.0 {
            Q14::ZERO
        } else {
            Q14::from_f64(self.config.correction.flex_tune as f64).max(Q14::from_bits(1))
        };
    }

    /// Calculates the choir drift decay and drift amount unconditionally.
    fn configure_choir_drift(&mut self) {
        let base = (self.rate.hz() / 100.0) as f64 * 0.2;
        let decay = (-std::f64::consts::LN_2 / base).exp();
        self.drift_decay = Q14::from_f64(decay);
        self.drift_amount = Q14::from_f64((1.0 - decay) * 0.008 * self.config.choir_detune as f64);
    }

    /// Calculates the stereo pan gains for the active choir voices
    /// unconditionally.
    fn configure_choir_pan(&mut self) {
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
    }

    /// Quantizes the harmony gain and calculates the stereo pan gains for the
    /// harmony voices unconditionally.
    ///
    /// The stereo pan gains include the harmony gain. The mono harmony path
    /// uses a separate Q14 gain.
    fn configure_harmony_gain(&mut self) {
        self.harmony_gain = Q14::from_f64(self.config.harmony_gain as f64);
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

    /// Initializes all quantized controls, decays and pan gains
    /// unconditionally.
    ///
    /// The histories and the controller phase do not change.
    fn configure_all(&mut self) {
        self.configure_pitch_ratio();
        self.configure_harmonic_modulation();
        self.configure_dry_mix();
        self.configure_correction_decay();
        self.configure_flex_tune();
        self.configure_choir_drift();
        self.configure_choir_pan();
        self.configure_harmony_gain();
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
        if old.pitch.threshold != config.pitch.threshold {
            self.pitch.set_config(config.pitch);
        }
        if old.formant != config.formant {
            self.formant.set_config(config.formant);
        }
        if old.harmony != config.harmony {
            self.harmony.apply_config(config.harmony);
        }
        if old.correction.notes != config.correction.notes
            || old.correction.enabled != config.correction.enabled
        {
            self.scale.clear_override(config.correction.notes);
            self.scale_initialized = false;
            self.dwell = Q14::ZERO;
        } else if old.correction.midi_mode != config.correction.midi_mode
            && self
                .scale
                .apply_mode(config.correction.midi_mode, config.correction.notes)
        {
            self.scale_initialized = false;
            self.dwell = Q14::ZERO;
        } else if config.correction.humanize == 0.0 {
            self.dwell = Q14::ZERO;
        }
        if old.correction.retune_ms != config.correction.retune_ms
            || old.correction.humanize != config.correction.humanize
        {
            self.configure_correction_decay();
        }
        if old.correction.flex_tune != config.correction.flex_tune {
            self.configure_flex_tune();
        }
        if old.pitch_ratio != config.pitch_ratio {
            self.configure_pitch_ratio();
            self.configure_shift_filters();
            self.scale_initialized = false;
            self.dwell = Q14::ZERO;
        }
        if old.mode != config.mode || old.choir_delay != config.choir_delay {
            self.create_delays();
        }
        if old.choir_voices < config.choir_voices {
            for voice in old.choir_voices as usize..config.choir_voices as usize {
                self.positions[voice] = self.positions[0];
            }
        }
        if old.choir_voices != config.choir_voices
            || old.choir_spread_percent != config.choir_spread_percent
        {
            self.configure_choir_pan();
        }
        if old.choir_detune != config.choir_detune {
            self.configure_choir_drift();
        }
        if old.harmony_gain != config.harmony_gain {
            self.configure_harmony_gain();
        }
        if old.harmonic_modulation != config.harmonic_modulation {
            self.configure_harmonic_modulation();
        }
        if old.dry_mix != config.dry_mix {
            self.configure_dry_mix();
        }
        if old.stereo_output != config.stereo_output {
            // A stereo change resets the audio state. The correction keyboard
            // keeps its held keys.
            self.reset_audio();
        }
    }

    /// Sets the temporary aspiration mute for a drain without a change to the
    /// configuration.
    pub(crate) fn set_aspiration_muted(&mut self, muted: bool) {
        self.formant.set_aspiration_muted(muted);
    }

    /// Stores the pitch acquisition threshold. If the value changed, quantizes
    /// it again.
    pub(crate) fn set_pitch_threshold(&mut self, threshold: f32) {
        let changed = self.config.pitch.threshold != threshold;
        self.config.pitch.threshold = threshold;
        if changed {
            self.pitch.set_threshold(threshold);
        }
    }

    /// Stores the retune half-life. If the value changed, updates the
    /// correction and humanize decays.
    pub(crate) fn set_retune_ms(&mut self, retune_ms: f32) {
        let changed = self.config.correction.retune_ms != retune_ms;
        self.config.correction.retune_ms = retune_ms;
        if changed {
            self.configure_correction_decay();
        }
    }

    /// Stores the relaxation on sustained notes. If the value is zero, clears
    /// the dwell progress. If the value changed, updates the humanize decay.
    pub(crate) fn set_humanize(&mut self, humanize: f32) {
        let changed = self.config.correction.humanize != humanize;
        self.config.correction.humanize = humanize;
        if humanize == 0.0 {
            self.dwell = Q14::ZERO;
        }
        if changed {
            self.configure_correction_decay();
        }
    }

    /// Stores the outer soft-zone width of each target bin. If the value
    /// changed, quantizes it again.
    pub(crate) fn set_flex_tune(&mut self, flex_tune: f32) {
        let changed = self.config.correction.flex_tune != flex_tune;
        self.config.correction.flex_tune = flex_tune;
        if changed {
            self.configure_flex_tune();
        }
    }

    /// Stores the global pitch multiplier. If the value changed, updates the
    /// shift and the anti-alias filters, and restarts target selection.
    pub(crate) fn set_pitch_ratio(&mut self, pitch_ratio: f32) {
        let changed = self.config.pitch_ratio != pitch_ratio;
        self.config.pitch_ratio = pitch_ratio;
        if changed {
            self.configure_pitch_ratio();
            self.configure_shift_filters();
            self.scale_initialized = false;
            self.dwell = Q14::ZERO;
        }
    }

    /// Stores the vocal-tract width and updates the formant stage.
    pub(crate) fn set_formant_width(&mut self, width: f32) {
        self.config.formant.width = width;
        self.formant.set_width(width);
    }

    /// Stores the vocal-tract stretch and updates the formant stage.
    pub(crate) fn set_formant_stretch(&mut self, stretch: f32) {
        self.config.formant.stretch = stretch;
        self.formant.set_stretch(stretch);
    }

    /// Stores the aspiration mix and updates the formant stage.
    pub(crate) fn set_formant_aspiration_mix(&mut self, aspiration_mix: f32) {
        self.config.formant.aspiration_mix = aspiration_mix;
        self.formant.set_aspiration_mix(aspiration_mix);
    }

    /// Stores the aspiration high-pass cutoff and updates the formant stage.
    pub(crate) fn set_formant_aspiration_cutoff_hz(&mut self, aspiration_cutoff_hz: f32) {
        self.config.formant.aspiration_cutoff_hz = aspiration_cutoff_hz;
        self.formant.set_aspiration_cutoff_hz(aspiration_cutoff_hz);
    }

    /// Stores the choir voice count. If the value changed, updates new reader
    /// positions and the choir pan gains.
    pub(crate) fn set_choir_voices(&mut self, choir_voices: u8) {
        let old = self.config.choir_voices;
        self.config.choir_voices = choir_voices;
        if old != choir_voices {
            if old < choir_voices {
                for voice in old as usize..choir_voices as usize {
                    self.positions[voice] = self.positions[0];
                }
            }
            self.configure_choir_pan();
        }
    }

    /// Stores the choir detune. If the value changed, updates the drift
    /// coefficients.
    pub(crate) fn set_choir_detune(&mut self, choir_detune: f32) {
        let changed = self.config.choir_detune != choir_detune;
        self.config.choir_detune = choir_detune;
        if changed {
            self.configure_choir_drift();
        }
    }

    /// Stores the choir delay scale. If the value changed, updates the delay
    /// offsets.
    pub(crate) fn set_choir_delay(&mut self, choir_delay: f32) {
        let changed = self.config.choir_delay != choir_delay;
        self.config.choir_delay = choir_delay;
        if changed {
            self.create_delays();
        }
    }

    /// Stores the choir stereo width. If the value changed, updates the choir
    /// pan gains.
    pub(crate) fn set_choir_spread_percent(&mut self, choir_spread_percent: f32) {
        let changed = self.config.choir_spread_percent != choir_spread_percent;
        self.config.choir_spread_percent = choir_spread_percent;
        if changed {
            self.configure_choir_pan();
        }
    }

    /// Stores the chord register and updates the harmony stage.
    pub(crate) fn set_harmony_register_percent(&mut self, register_percent: f32) {
        self.config.harmony.register_percent = register_percent;
        self.harmony.set_register_percent(register_percent);
    }

    /// Stores the chord spread and updates the harmony stage.
    pub(crate) fn set_harmony_voicing_spread_percent(&mut self, voicing_spread_percent: f32) {
        self.config.harmony.voicing_spread_percent = voicing_spread_percent;
        self.harmony
            .set_voicing_spread_percent(voicing_spread_percent);
    }

    /// Stores the portamento half-life and updates the harmony stage.
    pub(crate) fn set_harmony_portamento_ms(&mut self, portamento_ms: f32) {
        self.config.harmony.portamento_ms = portamento_ms;
        self.harmony.set_portamento_ms(portamento_ms);
    }

    /// Stores the count of interval voices and updates the harmony stage.
    pub(crate) fn set_harmony_voices(&mut self, voices: u8) {
        self.config.harmony.voices = voices;
        self.harmony.set_voices(voices);
    }

    /// Stores the harmony gain. If the value changed, updates the mono gain and
    /// the stereo pan gains.
    pub(crate) fn set_harmony_gain(&mut self, harmony_gain: f32) {
        let changed = self.config.harmony_gain != harmony_gain;
        self.config.harmony_gain = harmony_gain;
        if changed {
            self.configure_harmony_gain();
        }
    }

    /// Stores the harmonic modulation depth. If the value changed, quantizes it
    /// again.
    pub(crate) fn set_harmonic_modulation(&mut self, harmonic_modulation: f32) {
        let changed = self.config.harmonic_modulation != harmonic_modulation;
        self.config.harmonic_modulation = harmonic_modulation;
        if changed {
            self.configure_harmonic_modulation();
        }
    }

    /// Stores the dry mix weight. If the value changed, quantizes it again.
    pub(crate) fn set_dry_mix(&mut self, dry_mix: f32) {
        let changed = self.config.dry_mix != dry_mix;
        self.config.dry_mix = dry_mix;
        if changed {
            self.configure_dry_mix();
        }
    }

    /// Releases the correction keys 0..=127, clears any latched chord, loads
    /// the configured correction notes again and resets the audio state.
    ///
    /// The items that `reset_audio` keeps do not change, except the correction
    /// keyboard and the active note table.
    pub(crate) fn reset(&mut self) {
        self.scale.reset(self.config.correction.notes);
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
        let written = usize::from(self.written);
        self.written = 0;
        self.pitch.reset();
        clear_dirty_ring(self.raw.as_mut_slice(), written, Q14::ZERO);
        clear_dirty_ring(self.filtered.as_mut_slice(), written, Q14::ZERO);
        clear_dirty_ring(self.lead.as_mut_slice(), written, Q14::ZERO);
        if written > 0 {
            let lead_start = HISTORY - OVERLAP_FRAMES + 1;
            if written < OVERLAP_FRAMES {
                self.periods[1..=written].fill(Q6::ZERO);
                self.periods[lead_start..lead_start + written].fill(Q6::ZERO);
            } else if written < HISTORY - OVERLAP_FRAMES {
                self.periods[..=written].fill(Q6::ZERO);
                self.periods[lead_start..].fill(Q6::ZERO);
            } else {
                self.periods.fill(Q6::ZERO);
            }
        }
        clear_dirty_ring(self.previous_periods.as_mut_slice(), written, Q6::ZERO);
        clear_dirty_ring(self.ratios.as_mut_slice(), written, Q14::ONE);
        self.write = 0;
        self.formant.reset();
        self.harmony.reset();
        clear_dirty_ring(self.overlap.as_mut_slice(), written, Q14::ZERO);
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
        self.dwell = Q14::ZERO;
        self.scale_initialized = false;
        self.previous_scale_index = 0;
        self.previous_octave = 0;
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
        if self.scale.apply_validated_midi(
            event,
            self.config.correction.midi_mode,
            self.config.correction.notes,
        ) {
            self.scale_initialized = false;
            self.dwell = Q14::ZERO;
        }
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
    /// Returns the formant state that selects the current envelope response.
    pub(crate) fn envelope_stamp(&self) -> (bool, u64, u8, u8, u16, i32) {
        let (bank_epoch, current, previous, blend, width) =
            self.formant.envelope_stamp(self.formant_position);
        (
            self.config.formant.enabled,
            bank_epoch,
            current,
            previous,
            blend,
            width,
        )
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
    /// Clears the dwell progress and the previous target bin.
    fn reset_dwell(&mut self) {
        self.dwell = Q14::ZERO;
        self.previous_scale_index = 0;
        self.previous_octave = 0;
    }

    /// Maps the current dwell progress to a Q14 half-life decay coefficient.
    ///
    /// The Hermite ramp `u^3 * (4 - 3 * u)` interpolates the half-life in Q20
    /// milliseconds. A Padé `(1, 1)` quotient converts that half-life to Q14.
    fn humanize_decay(&self) -> Q14 {
        if self.dwell >= Q14::ONE {
            return self.sustained_decay;
        }
        let u = i64::from(self.dwell.to_bits());
        let s = (u * u * u * (65_536 - 3 * u)) >> 42;
        let half_life = self.retune_half_life
            + (((self.sustained_half_life - self.retune_half_life) * s) >> 14);
        let raw = if half_life <= self.decay_half_step {
            0
        } else {
            (((half_life - self.decay_half_step) << 14) / (half_life + self.decay_half_step)) as i32
        };
        Q14::from_bits(raw).clamp(self.correction_decay, self.sustained_decay)
    }

    /// Selects a target in the period domain and returns the Q14 target ratio
    /// and Q14 decay coefficient.
    ///
    /// The Q6 input period must be a usable result from the pitch tracker. The
    /// search moves through the midpoints, also across octave limits, and
    /// includes the global shift. On a sustained bin, `humanize` increases the
    /// half-life toward `sustained_half_life`. In the outer `flex_tune` zone,
    /// the pull toward the target decreases to zero at the midpoint boundary.
    ///
    /// If the shifted period is not positive, the function clears
    /// `scale_initialized` and dwell, and returns `None`. If correction is
    /// disabled or the active table is empty, the function returns unity ratio
    /// and `correction_decay`.
    fn select_target_ratio(&mut self, period: Q6) -> Option<(Q14, Q14)> {
        let mut target_ratio = Q14::ONE;
        let mut decay = self.correction_decay;
        if self.config.correction.enabled && self.scale.count != 0 {
            let was_initialized = self.scale_initialized;
            let count = self.scale.count;
            // The correction table holds periods normalized to one octave.
            // They use the same units as the shifted target period below.
            let table_period = |index: usize| Q6::from_bits(self.scale.midpoints[index]);
            let mut target: Q6;
            if !self.scale_initialized {
                self.scale_index = 0;
                self.octave = 0;
                target = self.shift.mul(period);
                if target <= Q6::ZERO {
                    self.reset_dwell();
                    return None;
                }
                while table_period(1) > target {
                    self.octave += 1;
                    target = target << 1;
                    if self.octave >= 30 || target <= Q6::ZERO {
                        break;
                    }
                }
                if target <= Q6::ZERO {
                    self.reset_dwell();
                    return None;
                }
                self.scale_initialized = true;
            }
            target = self.shift.mul(if self.octave >= 0 {
                period << self.octave as u32
            } else {
                period >> (-self.octave) as u32
            });
            if target <= Q6::ZERO {
                self.scale_initialized = false;
                self.reset_dwell();
                return None;
            }
            while table_period(self.scale_index + 1) < target {
                self.scale_index += 1;
                if self.scale_index == count {
                    self.scale_index = 0;
                    target = target >> 1;
                    self.octave -= 1;
                }
            }
            while target < table_period(self.scale_index) {
                if self.scale_index == 0 {
                    self.scale_index = count - 1;
                    target = target << 1;
                    self.octave += 1;
                } else {
                    self.scale_index -= 1;
                }
            }
            if !was_initialized
                || self.scale_index != self.previous_scale_index
                || self.octave != self.previous_octave
            {
                self.previous_scale_index = self.scale_index;
                self.previous_octave = self.octave;
                self.dwell = Q14::ZERO;
            } else if self.dwell_step != Q14::ZERO {
                self.dwell = (self.dwell + self.dwell_step).min(Q14::ONE);
                decay = self.humanize_decay();
            }
            let denominator = Q6::from_bits(self.scale.periods[self.scale_index]);
            if denominator != Q6::ZERO {
                target_ratio = target.div(denominator);
                if self.flex_tune != Q14::ZERO {
                    let (distance, span) = if target >= denominator {
                        let upper = Q6::from_bits(self.scale.midpoints[self.scale_index + 1]);
                        (target - denominator, upper - denominator)
                    } else {
                        let lower = Q6::from_bits(self.scale.midpoints[self.scale_index]);
                        (denominator - target, denominator - lower)
                    };
                    if span > Q6::ZERO {
                        let distance = distance.min(span);
                        let inner = span.mul::<14, 6>(Q14::ONE - self.flex_tune);
                        if distance > inner {
                            let outer = span - inner;
                            let taper = Q14::ONE - (distance - inner).div::<6, 14>(outer);
                            let pull = taper.mul::<14, 14>(taper);
                            target_ratio = Q14::ONE + pull.mul(target_ratio - Q14::ONE);
                        }
                    }
                }
            }
        } else {
            self.dwell = Q14::ZERO;
        }
        Some((target_ratio, decay))
    }

    /// Updates the Q14 correction ratio from `period`.
    ///
    /// If `select_target_ratio` finds a non-positive shifted period, the
    /// correction ratio becomes 1.0 immediately.
    fn update_correction(&mut self, period: Q6) {
        let Some((target_ratio, decay)) = self.select_target_ratio(period) else {
            self.correction = Q14::ONE;
            return;
        };
        self.correction = decay.mul(self.correction - target_ratio) + target_ratio;
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

    /// Returns the upper limit, the lower limit and the Q14 period jump for a
    /// reader.
    ///
    /// `offset` is an integer voice delay behind the writer.
    #[inline]
    fn reader_bounds(&self, period: Q6, offset: i32) -> (Q14, Q14, Q14) {
        let write = self.write as i32;
        let upper = position_of(write - 40 - offset);
        let span = (period.to_bits() >> 6).wrapping_mul(12).wrapping_div(10);
        let lower = position_of(
            write
                .wrapping_sub(span)
                .wrapping_sub(40)
                .wrapping_sub(offset),
        );
        (upper, lower, period.rescale())
    }

    /// Advances one Q14 reader by its ratio and keeps it in `bounds`.
    ///
    /// `bounds` holds the upper limit, the lower limit and the Q14 period
    /// jump. `voice` must identify an allocated reader slot. If `fade` is
    /// `true` and the reader jumps, the function saves the old position and
    /// starts the crossfade again.
    #[inline]
    fn step_reader(
        &mut self,
        voice: usize,
        ratio: Q14,
        bounds: (Q14, Q14, Q14),
        fade: bool,
    ) -> Q14 {
        let (upper, lower, jump) = bounds;
        let mut position = self.positions[voice] + ratio;
        if position >= upper {
            if fade {
                self.old_position = position;
                self.fade = Q14::ZERO;
            }
            position = position - jump;
        } else if position <= lower {
            if fade {
                self.old_position = position;
                self.fade = Q14::ZERO;
            }
            position = position + jump;
        }
        self.positions[voice] = position;
        position
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
        let bounds = self.reader_bounds(period, offset);
        self.step_reader(voice, ratio, bounds, fade)
    }

    /// Advances one secondary choir reader and returns its position and ratio.
    ///
    /// The function selects the delayed period, updates `current_output_period`
    /// and moves the reader without a crossfade. `voice` must identify a
    /// secondary choir slot, and `delayed` must be less than HISTORY.
    fn move_choir_reader(&mut self, voice: usize, delayed: usize) -> (Q14, Q14) {
        let offset = self.delays[voice];
        let delay = ring(delayed as i32 - offset);
        let (period, _) = self.period_for(delay, self.write);
        self.current_output_period = period;
        let ratio = self.drift_ratio[voice];
        let position = self.move_reader(voice, ratio, period, offset, false);
        (position, ratio)
    }

    /// Applies the glottal envelope to `sample` and updates `previous_output`
    /// for `channel`.
    ///
    /// `channel` must be `0` or `1`.
    #[inline]
    fn apply_glottal(&mut self, channel: usize, sample: Q14) -> Q14 {
        let output = Q14::from_bits(
            self.formant
                .model_glottal(sample.to_bits(), self.previous_output[channel].to_bits()),
        );
        self.previous_output[channel] = output;
        output
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
            value = self.apply_glottal(0, value);
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
        let write = record_history(&mut self.raw, &mut self.write, &mut self.written, sample);
        let was_tracking = !self.pitch.is_detecting();
        let step = self.pitch.process_sample(sample.to_bits());
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
            for position in &mut self.positions[..voices] {
                *position = *position - position_of(HISTORY as i32);
            }
            self.old_position = self.old_position - position_of(HISTORY as i32);
        }
        if step.restarted {
            self.correction = Q14::ONE;
            self.scale_initialized = false;
            self.dwell = Q14::ZERO;
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
        if self.config.pitch_ratio > 1.0 {
            for filter in &mut self.post {
                filtered = filter.process_sample(filtered);
            }
        } else {
            for filter in &mut self.post {
                filtered = filter.bypass_sample(filtered);
            }
        }
        self.filtered[write] = filtered;
        let delayed = ring(write as i32 - OVERLAP_FRAMES as i32);
        let dry = self.raw[ring(write as i32 - LATENCY_FRAMES as i32)];
        let mut mix = [Q14::ZERO; 2];
        let mut mono = !self.config.stereo_output;
        let mut final_ratio = Q14::ONE;
        if self.config.mode == MultiVoiceMode::Choir {
            let voice_count = self.config.choir_voices as usize;
            // Secondary voices read the lead history written by voice zero.
            {
                let (value, ratio) = self.render_lead(delayed, numeric);
                final_ratio = ratio;
                self.voice_ratios[0] = ratio;
                if self.config.stereo_output {
                    for (channel, sum) in mix.iter_mut().enumerate() {
                        *sum = *sum + value.mul(self.pan[0][channel]);
                    }
                } else {
                    mix[0] = mix[0] + value;
                }
            }
            {
                let mut voice = 1;
                while voice + 4 <= voice_count {
                    let mut positions = [Q14::ZERO; 4];
                    // After the lead update, set the reader positions in
                    // voice order.
                    for (lane, position) in positions.iter_mut().enumerate() {
                        (*position, final_ratio) = self.move_choir_reader(voice + lane, delayed);
                    }
                    let values =
                        positions.map(|position| history::interpolation(&self.lead, position).0);
                    // Keep the voice and channel order of each sum.
                    if self.config.stereo_output {
                        for (lane, value) in values.into_iter().enumerate() {
                            let current_voice = voice + lane;
                            for (channel, sum) in mix.iter_mut().enumerate() {
                                *sum = *sum + value.mul(self.pan[current_voice][channel]);
                            }
                        }
                    } else {
                        for value in values {
                            mix[0] = mix[0] + value;
                        }
                    }
                    voice += 4;
                }
                // The zero to three secondary readers that are left use
                // scalar code.
                for voice in voice..voice_count {
                    let (position, ratio) = self.move_choir_reader(voice, delayed);
                    final_ratio = ratio;
                    let (value, _) = history::interpolation(&self.lead, position);
                    if self.config.stereo_output {
                        for (channel, sum) in mix.iter_mut().enumerate() {
                            *sum = *sum + value.mul(self.pan[voice][channel]);
                        }
                    } else {
                        mix[0] = mix[0] + value;
                    }
                }
            }
        } else {
            let (period, ratio) = self.period_for(delayed, write);
            self.current_output_period = period;
            let counter = self.control_counter;
            self.control_counter += 1;
            if counter == CONTROL_DUE {
                self.control_counter = 0;
                let corrected_period = if ratio == Q14::ZERO {
                    Q6::ZERO
                } else {
                    period.div(ratio)
                };
                for voice in 0..HARMONY_VOICES {
                    let (voice_ratio, gain) = self.harmony.advance_voice(corrected_period, voice);
                    self.harmony_ratios[voice] = voice_ratio;
                    self.harmony_gains[voice] = gain;
                }
            }
            let bounds = self.reader_bounds(period, 0);
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
                let position = self.step_reader(slot, current, bounds, false);
                // If the Q12 gain or the stereo pan is zero, the voice adds
                // zero to the mix.
                if slot > 0 {
                    let voice = slot - 1;
                    let pan = self.harmony_pan[voice];
                    let silent_pan =
                        self.config.stereo_output && (pan[0].to_bits() | pan[1].to_bits()) == 0;
                    if self.harmony_gains[voice] == Q12::ZERO || silent_pan {
                        continue;
                    }
                }
                let (mut value, _) = history::interpolation(&self.filtered, position);
                if slot > 0 {
                    value = value.mul(self.harmony_gains[slot - 1]);
                    if !self.config.stereo_output {
                        value = value.mul(self.harmony_gain);
                    }
                }
                if self.config.stereo_output {
                    for (channel, sum) in mix.iter_mut().enumerate() {
                        *sum = *sum
                            + if slot > 0 {
                                value.mul(self.harmony_pan[slot - 1][channel])
                            } else {
                                value
                            };
                    }
                } else {
                    mix[0] = mix[0] + value;
                }
            }
            if self.config.formant.enabled {
                self.formant_position = (self.positions[0].to_bits() >> 14) - FORMANT_OFFSET_FRAMES;
                // If Choir changed only channel zero before a switch to
                // Harmony, synthesize the two channels until their histories
                // are equal.
                if mono
                    && self.formant.histories_match()
                    && self.previous_output[0] == self.previous_output[1]
                {
                    let value = Q14::from_bits(self.formant.forward_mono(
                        mix[0].to_bits(),
                        self.formant_position,
                        numeric,
                    ));
                    let value = self.apply_glottal(0, value);
                    self.previous_output[1] = value;
                    mix[0] = value;
                } else {
                    if mono {
                        mix[1] = mix[0];
                    }
                    mix = self
                        .formant
                        .forward_stereo(mix.map(Q14::to_bits), self.formant_position, numeric)
                        .map(Q14::from_bits);
                    for (channel, value) in mix.iter_mut().enumerate() {
                        *value = self.apply_glottal(channel, *value);
                    }
                    mono = mono && mix[0] == mix[1];
                }
            }
        }
        if self.modulation != 0 {
            if self.current_output_period != Q6::ZERO {
                // The phase advances by depth x 4 x ratio / period, in Q14
                // cycles.
                let step = clamp_i32(
                    (i64::from(self.modulation) * 4 * i64::from(final_ratio.to_bits()))
                        / i64::from(self.current_output_period.to_bits()),
                );
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
            let cosine = quadratic(left, middle, right, frac);
            if mono {
                mix[0] = mix[0].mul(cosine);
            } else {
                for sum in &mut mix {
                    *sum = sum.mul(cosine);
                }
            }
        }
        // The dry branch is the delayed raw input. It bypasses the shift
        // filters, the voice readers, the formant stages and the harmonic
        // modulation. Calculate the Q22 blend in `i64`, then shift and clamp
        // to Q14 one time only.
        let dry_mix = i64::from(self.dry_mix.to_bits());
        let wet = i64::from((Q8::ONE - self.dry_mix).to_bits());
        let dry = i64::from(dry.to_bits());
        if mono {
            let blended = (dry * dry_mix + i64::from(mix[0].to_bits()) * wet) >> 8;
            mix = [Q14::from_bits(clamp_i32(blended)); 2];
        } else {
            for sum in &mut mix {
                let blended = (dry * dry_mix + i64::from(sum.to_bits()) * wet) >> 8;
                *sum = Q14::from_bits(clamp_i32(blended));
            }
        }
        mix
    }
}
