//! Four harmony voices that follow intervals, scales, chords or MIDI.
//!
//! The controller converts tracked periods into target periods and gains. It
//! keeps the portamento, the gain envelopes and the MIDI assignment order
//! from one control step to the next.

mod chords;

use crate::error::{check_count, check_range};
use crate::midi::{velocity_gain, MidiEvent};
use crate::numeric::fixed::{Q12, Q14, Q6};
use crate::scale::PitchClass;
use crate::{Error, SampleRate};

/// The number of harmony voices in addition to the lead voice: four.
/// The interval arrays and the voice allocation for MIDI channels use the
/// same count.
pub const HARMONY_VOICES: usize = 4;

/// The Q12 step that increases the gain envelope of a voice toward a higher
/// target gain (attack).
///
/// The step applies one time at each voiced control step. From zero to a gain
/// of 1.0, the attack takes 26 steps.
const GAIN_ATTACK: Q12 = Q12::from_bits(163);
/// The Q12 step that decreases the gain envelope of a voice toward a lower
/// target gain (release).
///
/// The step applies one time at each voiced control step. From a gain of 1.0
/// to zero, the release takes 51 steps. The period portamento does not use
/// this value.
const GAIN_RELEASE: Q12 = Q12::from_bits(81);

/// The source of target periods for the four harmony voices.
///
/// The default is [`Self::MidiChannels`]. The static interval and chord
/// sources use the related [`HarmonyConfig`] fields. The MIDI sources use
/// [`crate::ControlEvent::Midi`]. The configuration of the sources that are
/// not selected is still validated and kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub enum HarmonySource {
    /// Each voice is a fixed number of semitones from the singer. This source
    /// uses `chromatic_intervals`.
    FixedInterval,
    /// Each voice follows an interval in scale steps from the singer. This
    /// source uses `diatonic_intervals`, `key` and `scale`.
    SmartInterval,
    /// The voices sing a chord on a scale degree. This source uses `degree`,
    /// `key`, `scale` and `inversion`.
    ChordByDegree,
    /// The voices sing a named chord. This source uses `chord`, `key` and
    /// `inversion`.
    ChordByName,
    /// The voices sing the active MIDI notes as a chord. The register and
    /// the spread select new octaves and a new order for these notes. The
    /// key and the inversion do not apply. This source combines all
    /// channels. It uses the same rules to allocate and steal voices as
    /// [`Self::MidiOmni`].
    ChordViaMidi,
    /// A note from any channel goes to the first free voice. If all four
    /// voices are busy, the note steals the voice with the oldest assignment.
    /// The stolen note stays in the keybed until it resumes or until its Note
    /// Off. If a Note Off releases a voice, that voice resumes the held pitch
    /// with the newest assignment. A repeated Note On for an active pitch
    /// updates its gain and its assignment order. Note Off releases the note
    /// on its channel.
    MidiOmni,
    /// Voices follow MIDI channels through [`HarmonyConfig::midi_channel_map`]
    /// and [`HarmonyConfig::midi_base_channel`].
    ///
    /// Each voice plays the newest held note across the channels mapped to it.
    /// If that note stops, the voice resumes the next newest held note on its
    /// mapped channels. This is the default.
    #[default]
    MidiChannels,
}

/// How [`HarmonySource::MidiChannels`] maps the 16 MIDI channels to the four
/// harmony voices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub enum MidiChannelMap {
    /// Channel `c` in `midi_base_channel..midi_base_channel + 4` maps to
    /// voice `c - midi_base_channel`. Other channels do not control voices.
    /// This is the default.
    #[default]
    BaseWindow,
    /// Channel `c` in 0..=15 maps to voice `c % 4`.
    WrapFour,
}

/// The scale that smart intervals and chords by degree use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub enum HarmonyScale {
    /// Major (ionian): semitone offsets 0, 2, 4, 5, 7, 9, 11. This is the
    /// default scale.
    #[default]
    Major,
    /// Major with the sixth again in the seventh slot of the table:
    /// 0, 2, 4, 5, 7, 9, 9. The degree table still has seven entries.
    MajorWithoutSeventh,
    /// Major with a flat seventh (mixolydian): 0, 2, 4, 5, 7, 9, 10.
    MajorFlatSeventh,
    /// Natural minor (aeolian): 0, 2, 3, 5, 7, 8, 10.
    Minor,
    /// Harmonic minor: 0, 2, 3, 5, 7, 8, 11.
    HarmonicMinor,
    /// Minor with a natural sixth (dorian): 0, 2, 3, 5, 7, 9, 10.
    MinorNaturalSixth,
    /// Minor with a major seventh: 0, 2, 3, 5, 7, 9, 11.
    /// This table keeps the natural sixth of the ascending melodic minor
    /// scale.
    MinorMajorSeventh,
}

/// A named chord for [`HarmonySource::ChordByName`].
///
/// Each variant lists the semitones above the root. These values do not
/// include the inversion, the key transposition, the register or the voicing
/// spread. Triads use three voices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub enum ChordName {
    /// Major triad: 0, 4, 7. This is the default chord.
    #[default]
    Major,
    /// Major sixth: 0, 4, 7, 9.
    MajorSixth,
    /// Major seventh: 0, 4, 7, 11.
    MajorSeventh,
    /// Major seventh with suspended fourth: 0, 5, 7, 11.
    MajorSeventhSuspendedFourth,
    /// Minor triad: 0, 3, 7.
    Minor,
    /// Minor sixth: 0, 3, 7, 9.
    MinorSixth,
    /// Minor seventh: 0, 3, 7, 10.
    MinorSeventh,
    /// Minor seventh with flat fifth: 0, 3, 6, 10.
    MinorSeventhFlatFifth,
    /// Diminished triad: 0, 3, 6.
    Diminished,
    /// Diminished seventh: 0, 3, 6, 9.
    DiminishedSeventh,
    /// Dominant seventh: 0, 4, 7, 10.
    DominantSeventh,
    /// Dominant seventh with flat fifth: 0, 4, 6, 10.
    DominantSeventhFlatFifth,
    /// Augmented triad: 0, 4, 8.
    Augmented,
    /// Augmented seventh: 0, 4, 8, 10.
    AugmentedSeventh,
    /// Suspended fourth: 0, 5, 7.
    SuspendedFourth,
    /// Suspended second: 0, 2, 7.
    SuspendedSecond,
    /// Suspended seventh: 0, 5, 7, 10.
    SuspendedSeventh,
}

/// A diatonic chord with its root on a degree of the selected
/// [`HarmonyScale`].
///
/// Triads use the root, third and fifth scale degrees. Seventh chords also
/// use the seventh. Degrees above the seventh go into higher octaves. The
/// degree names do not set a major or minor quality. The selected scale
/// supplies the tones. The default is the tonic triad, [`Self::I`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub enum ChordDegree {
    /// A triad on the first degree.
    #[default]
    I,
    /// A triad on the second degree.
    Ii,
    /// A seventh chord on the second degree.
    IiSeventh,
    /// A triad on the third degree.
    Iii,
    /// A seventh chord on the third degree.
    IiiSeventh,
    /// A triad on the fourth degree.
    Iv,
    /// A seventh chord on the fourth degree.
    IvSeventh,
    /// A triad on the fifth degree.
    V,
    /// A seventh chord on the fifth degree.
    VSeventh,
    /// A triad on the sixth degree.
    Vi,
    /// A seventh chord on the sixth degree.
    ViSeventh,
    /// A triad on the seventh degree.
    Vii,
    /// A seventh chord on the seventh degree.
    ViiSeventh,
}

/// The chord inversion raises the lowest chord tones by one octave.
///
/// The inversion applies before the register and the spread. It applies
/// only to named chords and degree chords. Root position is the default.
/// The first inversion moves one tone and the second inversion moves two
/// tones. The voicing of MIDI chords uses the played notes, not this control.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub enum Inversion {
    /// The root position.
    #[default]
    Root,
    /// The first inversion.
    First,
    /// The second inversion.
    Second,
}

/// The configuration of four harmony voices that follow intervals, chords or
/// MIDI.
///
/// Positive intervals make the pitch higher. The defaults are:
///
/// - MIDI channels 0..=3 as the source.
/// - The key of C and the major scale.
/// - Unison intervals.
/// - A tonic major chord in root position.
/// - A register of 50%.
/// - A closed voicing and zero portamento.
/// - Four voices for the interval sources.
/// - [`MidiChannelMap::BaseWindow`] with base channel 0.
///
/// These settings configure all sources. Only the fields for the selected
/// source change the target notes. Validation always checks all fields.
///
/// Harmony makes its voices from the tracked input. MIDI controls the pitch
/// and the gain. It is not an independent oscillator. The gain in decibels
/// is linear in the velocity: velocities 1..=127 give approximately -23.72
/// to +12 dB. A velocity of zero releases the note.
///
/// The gain attack, the gain release and the period portamento advance at
/// the controller cadence. This cadence is usually every 101 frames. A MIDI
/// event can make the next control step occur sooner. The envelopes and the
/// portamento advance only while the input is voiced.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub struct HarmonyConfig {
    /// The pitch source. The default is [`HarmonySource::MidiChannels`].
    pub source: HarmonySource,
    /// The pitch class of the tonic. The default is C. Smart intervals, named
    /// chords and degree chords use it. It does not transpose direct MIDI
    /// notes.
    pub key: PitchClass,
    /// The scale for smart intervals and degree chords. The default is major.
    pub scale: HarmonyScale,
    /// Four fixed semitone offsets, each in -24..=24. The default is all
    /// zeros. [`HarmonySource::FixedInterval`] uses them. A value of 12 makes
    /// the pitch one octave higher.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        tsify(type = "[number, number, number, number]")
    )]
    pub chromatic_intervals: [i8; HARMONY_VOICES],
    /// Four offsets in scale steps, each in -14..=14. The default is all
    /// zeros. [`HarmonySource::SmartInterval`] uses them. Seven steps make
    /// the pitch one octave higher.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        tsify(type = "[number, number, number, number]")
    )]
    pub diatonic_intervals: [i8; HARMONY_VOICES],
    /// The named chord for [`HarmonySource::ChordByName`]. The default is
    /// major.
    pub chord: ChordName,
    /// The chord on a scale degree for [`HarmonySource::ChordByDegree`]. The
    /// default is I.
    pub degree: ChordDegree,
    /// The inversion for named chords and degree chords. The default is root
    /// position.
    pub inversion: Inversion,
    /// The chord register from low to high, 0..=100%.
    ///
    /// The default is 50%. All chord sources use it, which includes MIDI chords.
    /// The value is truncated to a whole percentage before it selects the
    /// octave shift.
    pub register_percent: f32,
    /// The chord voicing from close to open, 0..=100%.
    ///
    /// The default is 0%. All chord sources use it. The value is truncated to a
    /// whole percentage. The result selects discrete octave positions, not
    /// continuous detuning.
    pub voicing_spread_percent: f32,
    /// The half-life of the target-period smoothing in milliseconds,
    /// 0..=10000. The default is 0. The smoothing applies at each control
    /// step. The coefficient assumes 100 frames for each step. At the usual
    /// cadence of 101 frames, the half-life is approximately 1% longer. If
    /// the value is zero, the voice uses the target period immediately. The
    /// gain envelopes still apply. A note on a free voice can start without
    /// a glide. In the interval sources, if the input moves more than one
    /// semitone bin, each voice starts at its new target without a glide.
    pub portamento_ms: f32,
    /// The number of voices that sound in the interval sources, 0..=4.
    ///
    /// The default is 4. [`HarmonySource::FixedInterval`] and
    /// [`HarmonySource::SmartInterval`] use the first `voices` voices. The
    /// other voices release with the usual gain envelope. The chord and MIDI
    /// sources ignore this field. In JavaScript, the field is optional and
    /// a missing value gives the default.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        serde(default = "default_voices"),
        tsify(optional)
    )]
    pub voices: u8,
    /// How [`HarmonySource::MidiChannels`] maps channels to voices.
    ///
    /// The default is [`MidiChannelMap::BaseWindow`]. Other harmony sources
    /// ignore this field. In JavaScript, the field is optional and an absent
    /// value gives the default.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        serde(default),
        tsify(optional)
    )]
    pub midi_channel_map: MidiChannelMap,
    /// The first MIDI channel of the four-channel window for
    /// [`MidiChannelMap::BaseWindow`], 0..=12.
    ///
    /// The default is 0. Voice `n` follows `midi_base_channel + n`. Validation
    /// checks this range for all sources and channel maps. In JavaScript, the
    /// field is optional and an absent value gives 0.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        serde(default),
        tsify(optional)
    )]
    pub midi_base_channel: u8,
}

/// The default count of interval voices: all of them.
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
fn default_voices() -> u8 {
    HARMONY_VOICES as u8
}

impl Default for HarmonyConfig {
    fn default() -> Self {
        Self {
            source: HarmonySource::MidiChannels,
            key: PitchClass::C,
            scale: HarmonyScale::Major,
            chromatic_intervals: [0; HARMONY_VOICES],
            diatonic_intervals: [0; HARMONY_VOICES],
            chord: ChordName::Major,
            degree: ChordDegree::I,
            inversion: Inversion::Root,
            register_percent: 50.0,
            voicing_spread_percent: 0.0,
            portamento_ms: 0.0,
            voices: HARMONY_VOICES as u8,
            midi_channel_map: MidiChannelMap::BaseWindow,
            midi_base_channel: 0,
        }
    }
}

impl HarmonyConfig {
    /// Validates all fields, which includes the fields that the selected
    /// source does not use.
    ///
    /// Each element of the two interval arrays must be in its documented
    /// inclusive range. The floating-point controls must be finite and in
    /// their documented inclusive ranges. Validation does not clamp values.
    /// It does not change the processor state.
    ///
    /// # Errors
    ///
    /// Returns the first [`Error::InvalidConfig`] with its dotted field path
    /// and the problem. For an array control, the error identifies the array
    /// field, not the index of an element.
    pub fn validate(self) -> Result<(), Error> {
        for interval in self.chromatic_intervals {
            let field = "resynthesis.harmony.chromatic_intervals";
            check_count(field, i64::from(interval), -24, 24)?;
        }
        for interval in self.diatonic_intervals {
            let field = "resynthesis.harmony.diatonic_intervals";
            check_count(field, i64::from(interval), -14, 14)?;
        }
        let field = "resynthesis.harmony.voices";
        check_count(field, i64::from(self.voices), 0, HARMONY_VOICES as i64)?;
        let field = "resynthesis.harmony.midi_base_channel";
        check_count(field, i64::from(self.midi_base_channel), 0, 12)?;
        let field = "resynthesis.harmony.register_percent";
        check_range(field, self.register_percent, 0.0, 100.0)?;
        let field = "resynthesis.harmony.voicing_spread_percent";
        check_range(field, self.voicing_spread_percent, 0.0, 100.0)?;
        check_range(
            "resynthesis.harmony.portamento_ms",
            self.portamento_ms,
            0.0,
            10_000.0,
        )
    }

    /// If `channel` is in the active window, returns its voice index 0..=3.
    fn map_channel(self, channel: u8) -> Option<usize> {
        match self.midi_channel_map {
            MidiChannelMap::WrapFour => Some(usize::from(channel) % HARMONY_VOICES),
            MidiChannelMap::BaseWindow => {
                let offset = channel.checked_sub(self.midi_base_channel)?;
                let voice = usize::from(offset);
                (voice < HARMONY_VOICES).then_some(voice)
            }
        }
    }
}

/// Allocates a 16-channel by 128-note table on the heap and keeps that shape
/// in the boxed-array type.
fn prepare_keybed_table<T: Copy>(value: T) -> Box<[[T; 128]; 16]> {
    vec![[value; 128]; 16]
        .into_boxed_slice()
        .try_into()
        .unwrap_or_else(|_| unreachable!("prepared MIDI keybed length"))
}

/// A fixed-size 16-channel by 128-note MIDI keybed.
struct MidiKeybed {
    /// Held notes for each channel 0..=15. Bit `n` is note `n` in 0..=127.
    held: [u128; 16],
    /// Nonzero velocity 1..=127 for each held note on each channel.
    velocity: Box<[[u8; 128]; 16]>,
    /// Monotonic press order for each held note on each channel.
    order: Box<[[u32; 128]; 16]>,
    /// Monotonic counter of Note On events.
    next_order: u32,
}

impl MidiKeybed {
    /// Returns an empty keybed with heap-allocated velocity and order tables.
    fn new() -> Self {
        Self {
            held: [0; 16],
            velocity: prepare_keybed_table(0),
            order: prepare_keybed_table(0),
            next_order: 0,
        }
    }

    /// Releases all held notes on all 16 channels.
    fn clear(&mut self) {
        self.held = [0; 16];
        self.next_order = 0;
    }

    /// If `next_order` is `u32::MAX`, renumbers held notes into `1..=count`.
    fn compact_orders(&mut self, voice_orders: &mut [u32; HARMONY_VOICES]) {
        let mut rank = 0_u32;
        let mut floor = 0_u32;
        loop {
            let mut min_order = u32::MAX;
            let mut found = false;
            for ch in 0..16 {
                let mut bits = self.held[ch];
                while bits != 0 {
                    let n = bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    let ord = self.order[ch][n];
                    if ord > floor && (!found || ord < min_order) {
                        min_order = ord;
                        found = true;
                    }
                }
            }
            if !found {
                break;
            }
            rank += 1;
            for ch in 0..16 {
                let mut bits = self.held[ch];
                while bits != 0 {
                    let n = bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    if self.order[ch][n] == min_order {
                        self.order[ch][n] = rank;
                    }
                }
            }
            for voice_order in voice_orders.iter_mut() {
                if *voice_order == min_order {
                    *voice_order = rank;
                }
            }
            if min_order == u32::MAX {
                break;
            }
            floor = min_order;
        }
        self.next_order = rank;
    }

    /// Records a MIDI event whose channel, note and velocity are valid.
    fn record(&mut self, event: MidiEvent, voice_orders: &mut [u32; HARMONY_VOICES]) {
        match event {
            MidiEvent::AllNotesOff => self.clear(),
            MidiEvent::NoteOn {
                channel,
                note,
                velocity,
            } if velocity != 0 => {
                if self.next_order == u32::MAX {
                    self.compact_orders(voice_orders);
                }
                let ch = usize::from(channel);
                let n = usize::from(note);
                self.next_order += 1;
                self.held[ch] |= 1_u128 << n;
                self.velocity[ch][n] = velocity;
                self.order[ch][n] = self.next_order;
            }
            MidiEvent::NoteOn { channel, note, .. } | MidiEvent::NoteOff { channel, note } => {
                let ch = usize::from(channel);
                let n = usize::from(note);
                self.held[ch] &= !(1_u128 << n);
            }
        }
    }
}

/// The prepared controller of periods and gains, and the MIDI allocator, for
/// four voices.
///
/// Periods use Q6 sample counts, ratios use Q14 and gains use Q12. The audio
/// path reads the outputs of `advance_voice`. MIDI events and changes
/// to the configuration update the target state separately. A zero period
/// marks an inactive note slot. The kept note number and velocity gain do
/// not mark this.
pub(crate) struct HarmonyState {
    /// The prepared sample rate in hertz. It is kept as f32 for the
    /// coefficient conversion.
    rate: f32,
    /// The validated source and voicing controls.
    config: HarmonyConfig,
    /// The last MIDI note number for each voice. It is valid only while the
    /// period of the voice is not zero.
    midi_note: [i32; HARMONY_VOICES],
    /// The Q6 periods of the allocated MIDI notes. Zero marks a free voice.
    midi_period: [Q6; HARMONY_VOICES],
    /// The Q12 velocity gain of each allocated note.
    midi_gain: [Q12; HARMONY_VOICES],
    /// The monotonic event order of each voice assignment.
    midi_order: [u32; HARMONY_VOICES],
    /// The smoothed Q6 target period. If it is zero, the next active target
    /// sets it directly.
    portamento_period: [Q6; HARMONY_VOICES],
    /// The last usable Q14 ratio of the input period to the smoothed target
    /// period for each voice.
    current_ratio: [Q14; HARMONY_VOICES],
    /// The Q12 attack and release envelope. A MIDI release or a reset does
    /// not clear it.
    smoothed_gain: [Q12; HARMONY_VOICES],
    /// The last active Q6 target. It is kept while the gain of a released
    /// voice decreases to zero.
    period_output: [Q6; HARMONY_VOICES],
    /// The Q6 target that the current source selected at the latest control
    /// step.
    target_period: [Q6; HARMONY_VOICES],
    /// The prepared Q6 chord periods. Slot 3 holds the lowest tone, and
    /// higher tones use lower slots. Zero marks an inactive voice.
    chord_period: [Q6; HARMONY_VOICES],
    /// The Q12 gains for the chord voices. An inactive voice has a zero
    /// period, so its gain is not used. This gain is usually a negative
    /// sentinel value.
    chord_gain: [Q12; HARMONY_VOICES],
    /// The Q14 pitch multipliers from the fixed semitone offsets.
    fixed_interval: [Q14; HARMONY_VOICES],
    /// 79 ascending Q6 boundaries for the 78 input-note bins of each voice.
    /// The boundaries include sentinels.
    scale_periods: [[Q6; 79]; HARMONY_VOICES],
    /// The Q6 output target period for each input-note bin and voice.
    scale_targets: [[Q6; 78]; HARMONY_VOICES],
    /// The cached input-note bin for each voice, in the range 0..=77.
    current_idx: [usize; HARMONY_VOICES],
    /// The last nonzero scale target for each voice. It is kept if a lookup
    /// gives zero.
    scale_target: [Q6; HARMONY_VOICES],
    /// The last scale-bin index for each voice. The controller uses it to
    /// decide if the portamento of the voice starts again.
    scale_note: [i32; HARMONY_VOICES],
    /// The Q14 half-life coefficient. The controller applies it one time at
    /// each voice control step.
    porta_decay: Q14,
    /// The 16-channel by 128-note MIDI keybed.
    keybed: MidiKeybed,
    /// The precomputed Q12 velocity gains for all 128 MIDI velocity values.
    gains: [Q12; 128],
}

impl HarmonyState {
    /// Prepares the voice state from a validated configuration.
    pub(crate) fn new(rate: SampleRate, config: HarmonyConfig) -> Self {
        let mut result = Self {
            rate: rate.hz(),
            config,
            midi_note: [0; HARMONY_VOICES],
            midi_period: [Q6::ZERO; HARMONY_VOICES],
            midi_gain: [Q12::ZERO; HARMONY_VOICES],
            midi_order: [0; HARMONY_VOICES],
            portamento_period: [Q6::ZERO; HARMONY_VOICES],
            current_ratio: [Q14::ONE; HARMONY_VOICES],
            smoothed_gain: [Q12::ZERO; HARMONY_VOICES],
            period_output: [Q6::ZERO; HARMONY_VOICES],
            target_period: [Q6::ZERO; HARMONY_VOICES],
            chord_period: [Q6::ZERO; HARMONY_VOICES],
            // A chord that gets voices replaces these inactive gains.
            chord_gain: [Q12::from_bits(-16_384); HARMONY_VOICES],
            fixed_interval: [Q14::ONE; HARMONY_VOICES],
            scale_periods: [[Q6::ZERO; 79]; HARMONY_VOICES],
            scale_targets: [[Q6::ZERO; 78]; HARMONY_VOICES],
            current_idx: [39; HARMONY_VOICES],
            scale_target: [Q6::ZERO; HARMONY_VOICES],
            scale_note: [0; HARMONY_VOICES],
            porta_decay: Q14::ZERO,
            keybed: MidiKeybed::new(),
            gains: std::array::from_fn(|i| velocity_gain(i as u8)),
        };
        result.prepare_intervals();
        result.setup_chord();
        result
    }

    /// Applies a configuration that validation already accepted.
    ///
    /// If the source or the channel map changes, the function projects the
    /// held keybed notes onto the new source. Unselected voices then decay and
    /// held notes stay active. It prepares the tables again only for the
    /// fields that changed.
    pub(crate) fn apply_config(&mut self, config: HarmonyConfig) {
        let old = self.config;
        self.config = config;
        if old.source != config.source
            || old.midi_channel_map != config.midi_channel_map
            || old.midi_base_channel != config.midi_base_channel
        {
            self.reproject_keybed();
        }
        if old.key != config.key
            || old.scale != config.scale
            || old.diatonic_intervals != config.diatonic_intervals
        {
            for channel in 0..HARMONY_VOICES {
                self.make_scale_periods(channel);
            }
        }
        if old.chromatic_intervals != config.chromatic_intervals {
            self.prepare_fixed_intervals();
        }
        if old.portamento_ms != config.portamento_ms {
            self.prepare_portamento();
        }
        if old != config {
            self.setup_chord();
        }
    }
    /// Releases all MIDI notes and clears the keybed.
    ///
    /// The function clears these items:
    ///
    /// - The 16-channel keybed.
    /// - The active MIDI periods, which releases the voices.
    /// - The portamento start state.
    ///
    /// It keeps the gain envelopes, the current ratios, the output and target
    /// periods, the scale tables and the last note numbers, gains and orders.
    /// Thus each released voice decays with the gain release.
    ///
    /// For [`HarmonySource::ChordViaMidi`], the function also prepares the
    /// chord again from the released notes. Thus the chord voices decay
    /// with the gain release. The other chord sources keep their chord
    /// preparation.
    pub(crate) fn reset(&mut self) {
        self.keybed.clear();
        self.midi_period.fill(Q6::ZERO);
        self.portamento_period.fill(Q6::ZERO);
        if self.config.source == HarmonySource::ChordViaMidi {
            self.setup_chord();
        }
    }
    /// Converts cents above C4 to a Q6 sample period. C4 is 900 cents below
    /// A4 at 440 Hz.
    /// The intermediate period is rounded to f32 before the fixed-point
    /// conversion.
    fn cents_period(&self, cents: f64) -> Q6 {
        let period = ((f64::from(self.rate) / 440.0)
            * ((900.0 - cents) * std::f64::consts::LN_2 / 1200.0).exp())
            as f32;
        Q6::from_f64(f64::from(period))
    }
    /// Converts each validated semitone interval to a Q14 frequency
    /// multiplier.
    fn prepare_fixed_intervals(&mut self) {
        self.fixed_interval = std::array::from_fn(|i| {
            Q14::from_f64(
                (f64::from(self.config.chromatic_intervals[i]) * std::f64::consts::LN_2 / 12.0)
                    .exp(),
            )
        });
    }
    /// Prepares the Q14 half-life coefficient for a nominal control step of
    /// 100 frames.
    /// A time of zero disables the period smoothing. Keep the f32 division of
    /// the rate.
    fn prepare_portamento(&mut self) {
        // Keep the f32 division of the rate before the f64 decay calculation.
        self.porta_decay = if self.config.portamento_ms == 0.0 {
            Q14::ZERO
        } else {
            let param = f64::from(self.config.portamento_ms) * 5.0;
            Q14::from_f64(
                (-std::f64::consts::LN_2 / (f64::from(self.rate / 100.0) * (param / 5000.0))).exp(),
            )
        };
    }
    /// Prepares the fixed ratios, the portamento and all four scale lookup
    /// tables.
    fn prepare_intervals(&mut self) {
        self.prepare_fixed_intervals();
        self.prepare_portamento();
        for i in 0..HARMONY_VOICES {
            self.make_scale_periods(i);
        }
    }
    /// Maps a semitone note relative to C4 to a one-based diatonic degree in
    /// the selected key.
    /// Euclidean division gives the correct result for negative octaves.
    /// All scales in the major family use one fixed table, and all scales in
    /// the minor family use another table. Notes outside the scale thus get
    /// the degree from that table. The function does not search for the
    /// nearest note.
    fn chromatic_to_degree(&self, note: i32) -> i32 {
        let relative = note - i32::from(self.config.key.semitone());
        let table = match self.config.scale {
            HarmonyScale::Major
            | HarmonyScale::MajorWithoutSeventh
            | HarmonyScale::MajorFlatSeventh => [1, 1, 2, 2, 3, 4, 4, 5, 5, 6, 7, 7],
            HarmonyScale::Minor
            | HarmonyScale::HarmonicMinor
            | HarmonyScale::MinorNaturalSixth
            | HarmonyScale::MinorMajorSeventh => [1, 1, 2, 3, 3, 4, 4, 5, 6, 6, 7, 7],
        };
        relative.div_euclid(12) * 7 + table[relative.rem_euclid(12) as usize]
    }
    /// Maps a one-based degree through the selected scale and key.
    fn degree_to_chromatic(&self, degree: i32) -> i32 {
        chords::scale_degree_semitones(self.config.scale, degree - 1)
            + i32::from(self.config.key.semitone())
    }
    /// Builds 78 target bins and 79 period boundaries for one voice.
    ///
    /// Bin 0 is 36 semitones above C4, and each next bin is one semitone
    /// lower. The boundaries go up in period. The outer boundaries are zero
    /// and `i32::MAX`. The function sets the cached bin to 39. Intervals
    /// that are multiples of seven steps give whole octaves and do not use
    /// the scale.
    fn make_scale_periods(&mut self, channel: usize) {
        let interval = i32::from(self.config.diatonic_intervals[channel]);
        for i in 0..78 {
            let cents = 3600 - i as i32 * 100;
            let note = 36 - i as i32;
            self.scale_periods[channel][i] = self.cents_period(f64::from(cents) + 50.0);
            let diff = if interval % 7 == 0 {
                interval / 7 * 1200
            } else {
                (self.degree_to_chromatic(self.chromatic_to_degree(note) + interval) - note) * 100
            };
            self.scale_targets[channel][i] = self.cents_period(f64::from(cents + diff));
        }
        self.scale_periods[channel][0] = Q6::ZERO;
        self.scale_periods[channel][78] = Q6::from_bits(i32::MAX);
        self.current_idx[channel] = 39;
    }
    /// Moves the cached bin toward a Q6 input period and returns the target
    /// and the bin.
    /// The voice index must be less than `HARMONY_VOICES`. If the target of a
    /// bin is zero, the function returns the previous nonzero target. If no
    /// target exists yet, it returns the input period.
    #[inline(always)]
    fn find_scale_period(&mut self, period: Q6, channel: usize) -> (Q6, i32) {
        let mut index = self.current_idx[channel];
        while index < 77 && self.scale_periods[channel][index + 1] < period {
            index += 1;
        }
        while index > 0 && self.scale_periods[channel][index] > period {
            index -= 1;
        }
        if self.scale_targets[channel][index] != Q6::ZERO {
            self.scale_target[channel] = self.scale_targets[channel][index];
        }
        self.current_idx[channel] = index;
        (
            if self.scale_target[channel] == Q6::ZERO {
                period
            } else {
                self.scale_target[channel]
            },
            index as i32,
        )
    }
    /// Projects the held keybed notes onto the voice slots of the active
    /// source.
    fn reproject_keybed(&mut self) {
        match self.config.source {
            HarmonySource::MidiOmni | HarmonySource::ChordViaMidi => self.reproject_omni_voices(),
            HarmonySource::MidiChannels => self.reproject_channels(),
            HarmonySource::FixedInterval
            | HarmonySource::SmartInterval
            | HarmonySource::ChordByDegree
            | HarmonySource::ChordByName => {
                self.midi_period.fill(Q6::ZERO);
                self.portamento_period.fill(Q6::ZERO);
            }
        }
    }
    /// Projects the 16-channel keybed onto up to four omni voices.
    fn reproject_omni_voices(&mut self) {
        let mut any_held = 0_u128;
        for &held in &self.keybed.held {
            any_held |= held;
        }
        let mut selected_note = [0_usize; HARMONY_VOICES];
        let mut selected_gain = [Q12::ZERO; HARMONY_VOICES];
        let mut selected_order = [0_u32; HARMONY_VOICES];
        let mut count = 0_usize;
        let mut bits = any_held;
        while bits != 0 {
            let note = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let mut max_order = 0_u32;
            let mut max_vel = 0_u8;
            for ch in 0..16 {
                if (self.keybed.held[ch] & (1_u128 << note)) != 0 {
                    let order = self.keybed.order[ch][note];
                    if order > max_order {
                        max_order = order;
                        max_vel = self.keybed.velocity[ch][note];
                    }
                }
            }
            let gain = self.gains[usize::from(max_vel)];
            if count < HARMONY_VOICES {
                let mut pos = count;
                while pos > 0 && selected_order[pos - 1] > max_order {
                    selected_note[pos] = selected_note[pos - 1];
                    selected_gain[pos] = selected_gain[pos - 1];
                    selected_order[pos] = selected_order[pos - 1];
                    pos -= 1;
                }
                selected_note[pos] = note;
                selected_gain[pos] = gain;
                selected_order[pos] = max_order;
                count += 1;
            } else if max_order > selected_order[0] {
                let mut pos = 0;
                while pos + 1 < HARMONY_VOICES && selected_order[pos + 1] < max_order {
                    selected_note[pos] = selected_note[pos + 1];
                    selected_gain[pos] = selected_gain[pos + 1];
                    selected_order[pos] = selected_order[pos + 1];
                    pos += 1;
                }
                selected_note[pos] = note;
                selected_gain[pos] = gain;
                selected_order[pos] = max_order;
            }
        }
        let was_active: [bool; HARMONY_VOICES] =
            std::array::from_fn(|v| self.midi_period[v] != Q6::ZERO);
        let mut voice_kept = [false; HARMONY_VOICES];
        let mut pitch_placed = [false; HARMONY_VOICES];
        for voice in 0..HARMONY_VOICES {
            if was_active[voice] {
                if let Some(idx) = (0..count)
                    .find(|&i| !pitch_placed[i] && selected_note[i] as i32 == self.midi_note[voice])
                {
                    voice_kept[voice] = true;
                    pitch_placed[idx] = true;
                    self.midi_gain[voice] = selected_gain[idx];
                    self.midi_order[voice] = selected_order[idx];
                }
            }
        }
        for idx in 0..count {
            if pitch_placed[idx] {
                continue;
            }
            let voice = if let Some(free) =
                (0..HARMONY_VOICES).find(|&v| !voice_kept[v] && !was_active[v])
            {
                self.portamento_period[free] = Q6::ZERO;
                free
            } else {
                let mut oldest = None;
                for (v, &kept) in voice_kept.iter().enumerate() {
                    if !kept && oldest.is_none_or(|prev| self.midi_order[v] < self.midi_order[prev])
                    {
                        oldest = Some(v);
                    }
                }
                oldest.unwrap_or(0)
            };
            self.assign(
                voice,
                selected_note[idx],
                selected_gain[idx],
                selected_order[idx],
            );
            voice_kept[voice] = true;
        }
        for (voice, &kept) in voice_kept.iter().enumerate() {
            if !kept {
                self.midi_period[voice] = Q6::ZERO;
            }
        }
    }
    /// Projects the 16-channel keybed onto the four channel-mapped voices.
    fn reproject_channels(&mut self) {
        let mut best_note = [0_usize; HARMONY_VOICES];
        let mut best_vel = [0_u8; HARMONY_VOICES];
        let mut best_order = [0_u32; HARMONY_VOICES];
        for ch in 0..16 {
            let Some(voice) = self.config.map_channel(ch as u8) else {
                continue;
            };
            let mut bits = self.keybed.held[ch];
            while bits != 0 {
                let note = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let order = self.keybed.order[ch][note];
                if order > best_order[voice] {
                    best_order[voice] = order;
                    best_note[voice] = note;
                    best_vel[voice] = self.keybed.velocity[ch][note];
                }
            }
        }
        for voice in 0..HARMONY_VOICES {
            if best_order[voice] != 0 {
                if self.midi_period[voice] == Q6::ZERO {
                    self.portamento_period[voice] = Q6::ZERO;
                }
                let gain = self.gains[usize::from(best_vel[voice])];
                self.assign(voice, best_note[voice], gain, best_order[voice]);
            } else {
                self.midi_period[voice] = Q6::ZERO;
            }
        }
    }
    /// Applies a MIDI event with channel, note and velocity values that are
    /// already in range.
    ///
    /// Every event updates the 16-channel keybed. Returns `true` if resynthesis
    /// must make its next control step due. All Notes Off calls `reset` and
    /// returns `false`.
    pub(crate) fn apply_validated_midi(&mut self, event: MidiEvent) -> bool {
        let (channel, note, velocity) = match event {
            MidiEvent::AllNotesOff => {
                self.reset();
                return false;
            }
            MidiEvent::NoteOn {
                channel,
                note,
                velocity,
            } => (channel, usize::from(note), velocity),
            MidiEvent::NoteOff { channel, note } => (channel, usize::from(note), 0),
        };
        self.keybed.record(event, &mut self.midi_order);
        let control_due = match self.config.source {
            HarmonySource::MidiOmni | HarmonySource::ChordViaMidi => {
                let already_active =
                    velocity != 0 && (0..HARMONY_VOICES).any(|v| self.voice_has_note(v, note));
                self.reproject_omni_voices();
                !already_active
            }
            HarmonySource::MidiChannels => {
                let Some(voice) = self.config.map_channel(channel) else {
                    return false;
                };
                let old_period = self.midi_period[voice];
                let old_note = self.midi_note[voice];
                self.reproject_channels();
                if velocity != 0 {
                    true
                } else {
                    self.midi_period[voice] != Q6::ZERO
                        && (old_period == Q6::ZERO || self.midi_note[voice] != old_note)
                }
            }
            HarmonySource::FixedInterval
            | HarmonySource::SmartInterval
            | HarmonySource::ChordByDegree
            | HarmonySource::ChordByName => false,
        };
        if self.config.source == HarmonySource::ChordViaMidi {
            self.setup_chord();
        }
        control_due
    }
    /// If `voice` has an active MIDI assignment for `note`, returns `true`.
    fn voice_has_note(&self, voice: usize, note: usize) -> bool {
        self.midi_period[voice] != Q6::ZERO && self.midi_note[voice] == note as i32
    }
    /// Assigns a validated MIDI pitch, gain and event order to a voice.
    fn assign(&mut self, voice: usize, note: usize, gain: Q12, order: u32) {
        self.midi_period[voice] = self.cents_period(f64::from((note as i32 - 60) * 100));
        self.midi_note[voice] = note as i32;
        self.midi_gain[voice] = gain;
        self.midi_order[voice] = order;
    }
    /// Advances one voice from the Q6 tracked input period to a Q14 ratio and
    /// a Q12 gain.
    /// The voice index must be less than `HARMONY_VOICES`. If the input is
    /// unvoiced (zero), the function returns a unity ratio and silence. It
    /// also clears the portamento start state and keeps the gain envelope.
    /// If the input is voiced, the function advances the gain envelope. It
    /// keeps the target of a released voice until the gain is zero. Then, in
    /// the same step, it applies portamento in the period domain.
    pub(crate) fn advance_voice(&mut self, period: Q6, voice: usize) -> (Q14, Q12) {
        if period == Q6::ZERO {
            self.portamento_period[voice] = Q6::ZERO;
            return (Q14::ONE, Q12::ZERO);
        }
        let target_gain = self.select_target(period, voice);
        if self.smoothed_gain[voice] < target_gain {
            self.smoothed_gain[voice] = (self.smoothed_gain[voice] + GAIN_ATTACK).min(target_gain);
        } else if self.smoothed_gain[voice] > target_gain {
            self.smoothed_gain[voice] = (self.smoothed_gain[voice] - GAIN_RELEASE).max(target_gain);
        }
        if self.target_period[voice] != Q6::ZERO {
            self.period_output[voice] = self.target_period[voice];
        } else if self.smoothed_gain[voice] == Q12::ZERO {
            self.period_output[voice] = Q6::ZERO;
            self.portamento_period[voice] = Q6::ZERO;
        }
        if self.period_output[voice] > Q6::ZERO {
            self.portamento_period[voice] = if self.portamento_period[voice] == Q6::ZERO {
                self.period_output[voice]
            } else {
                self.porta_decay.mul(self.portamento_period[voice])
                    + (Q14::ONE - self.porta_decay).mul(self.period_output[voice])
            };
            if self.portamento_period[voice] > Q6::ZERO {
                self.current_ratio[voice] = period.div(self.portamento_period[voice]);
            }
        }
        (self.current_ratio[voice], self.smoothed_gain[voice])
    }
    /// Selects a Q6 target and an unsmoothed Q12 gain for one voice.
    /// The interval sources also update the scale-bin index of the voice. If
    /// this index changes by more than one bin, the voice restarts its
    /// portamento.
    fn select_target(&mut self, period: Q6, voice: usize) -> Q12 {
        match self.config.source {
            HarmonySource::FixedInterval | HarmonySource::SmartInterval
                if voice >= self.config.voices as usize =>
            {
                // A voice after the configured count releases. A zero target
                // keeps its last period until the gain gets to zero.
                self.target_period[voice] = Q6::ZERO;
                Q12::ZERO
            }
            HarmonySource::FixedInterval | HarmonySource::SmartInterval => {
                let previous = self.scale_note[voice];
                let (target, note) = self.find_scale_period(period, voice);
                self.scale_note[voice] = note;
                self.target_period[voice] = if self.config.source == HarmonySource::FixedInterval {
                    period.div(self.fixed_interval[voice])
                } else {
                    target
                };
                if (previous - self.scale_note[voice]).abs() > 1 {
                    self.portamento_period[voice] = Q6::ZERO;
                }
                Q12::ONE
            }
            HarmonySource::ChordByDegree
            | HarmonySource::ChordByName
            | HarmonySource::ChordViaMidi
            | HarmonySource::MidiOmni
            | HarmonySource::MidiChannels => {
                let (target, gain) = match self.config.source {
                    HarmonySource::ChordViaMidi => {
                        (self.chord_period[voice], self.chord_gain[voice])
                    }
                    HarmonySource::MidiOmni | HarmonySource::MidiChannels => {
                        (self.midi_period[voice], self.midi_gain[voice])
                    }
                    _ => (self.chord_period[voice], Q12::ONE),
                };
                self.target_period[voice] = target;
                if self.target_period[voice] == Q6::ZERO {
                    Q12::ZERO
                } else {
                    gain
                }
            }
        }
    }
    /// Prepares the chord periods and their gains from the selected source.
    /// The lowest tones go into the voice slots with the highest numbers.
    /// Inactive slots get zero periods, so the output does not use their
    /// sentinel gains.
    fn setup_chord(&mut self) {
        let (tones, gains, count) = chords::voice_chord(
            &self.config,
            self.midi_note,
            self.midi_period,
            self.midi_gain,
        );
        if count == 0 {
            self.chord_period.fill(Q6::ZERO);
            return;
        }
        for i in 0..count {
            self.chord_gain[3 - i] = gains[i];
            self.chord_period[3 - i] = self.cents_period(f64::from(tones[i]) * 100.0);
        }
        for i in count..4 {
            self.chord_period[3 - i] = Q6::ZERO;
            self.chord_gain[3 - i] = -Q12::ONE;
        }
    }
}
