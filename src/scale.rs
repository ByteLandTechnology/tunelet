//! Pitch classes and correction tables, made from scales or held MIDI notes.

use crate::midi::MidiEvent;
use crate::SampleRate;

/// One of the twelve notes in an octave.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub enum PitchClass {
    /// The note C.
    #[default]
    C,
    /// The note C sharp.
    CSharp,
    /// The note D.
    D,
    /// The note D sharp.
    DSharp,
    /// The note E.
    E,
    /// The note F.
    F,
    /// The note F sharp.
    FSharp,
    /// The note G.
    G,
    /// The note G sharp.
    GSharp,
    /// The note A.
    A,
    /// The note A sharp.
    ASharp,
    /// The note B.
    B,
}

impl PitchClass {
    /// All pitch classes, from C upward.
    ///
    /// Index `n` is `n` semitones above C.
    pub const ALL: [Self; 12] = [
        Self::C,
        Self::CSharp,
        Self::D,
        Self::DSharp,
        Self::E,
        Self::F,
        Self::FSharp,
        Self::G,
        Self::GSharp,
        Self::A,
        Self::ASharp,
        Self::B,
    ];

    /// Returns the number of semitones above C, 0..=11.
    pub const fn semitone(self) -> u8 {
        self as u8
    }
}

/// A scale for [`NoteMask::from_key_scale`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub enum CorrectionScale {
    /// The major scale.
    Major,
    /// The natural minor scale.
    Minor,
    /// The chromatic scale with all notes.
    #[default]
    Chromatic,
}

/// How [`crate::ControlEvent::CorrectionMidi`] keeps or releases notes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub enum CorrectionMidiMode {
    /// Held MIDI keys override the configured scale until all keys in 0..=127
    /// release. This is the default.
    #[default]
    Momentary,
    /// A MIDI chord stays active after its keys release until All Notes Off, a
    /// reset or a scale change.
    Latch,
}

/// The permitted correction pitch classes, in the order C, C♯, D, D♯, E, F,
/// F♯, G, G♯, A, A♯, B.
///
/// A `true` flag enables that pitch class in all octaves.
///
/// The default is [`Self::CHROMATIC`]. All combinations of flags are valid.
/// This includes [`Self::NONE`]. If the mask is empty, the correction ratio
/// moves toward unity with the configured retune half-life. An empty mask
/// does not clear the histories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(transparent)
)]
pub struct NoteMask(pub [bool; 12]);

impl Default for NoteMask {
    fn default() -> Self {
        Self::CHROMATIC
    }
}

impl NoteMask {
    /// The mask that permits all pitch classes.
    pub const CHROMATIC: Self = Self([true; 12]);
    /// The mask with no active target notes.
    ///
    /// Correction updates move toward unity with the configured retune
    /// half-life. The current correction does not always stop immediately.
    pub const NONE: Self = Self([false; 12]);
    /// Returns a mask from twelve flags, where index 0 is C.
    pub const fn new(notes: [bool; 12]) -> Self {
        Self(notes)
    }

    /// Returns the pitch classes of `scale`, with `key` as the tonic.
    ///
    /// Major uses the semitone offsets 0, 2, 4, 5, 7, 9, 11. Natural minor
    /// uses 0, 2, 3, 5, 7, 8, 10. Chromatic permits all classes for all keys.
    /// This function only makes a value. To apply the value, use the
    /// correction configuration.
    pub fn from_key_scale(key: PitchClass, scale: CorrectionScale) -> Self {
        if scale == CorrectionScale::Chromatic {
            return Self::CHROMATIC;
        }
        let offset = usize::from(key.semitone())
            + if scale == CorrectionScale::Minor {
                3
            } else {
                0
            };
        let major = [
            true, false, true, false, true, true, false, true, false, true, false, true,
        ];
        Self(std::array::from_fn(|i| major[(i + 24 - offset) % 12]))
    }
}

/// The bits for pitch class C across MIDI notes 0..=127.
const CLASS_C_NOTES: u128 = (1 << 0)
    | (1 << 12)
    | (1 << 24)
    | (1 << 36)
    | (1 << 48)
    | (1 << 60)
    | (1 << 72)
    | (1 << 84)
    | (1 << 96)
    | (1 << 108)
    | (1 << 120);

/// Converts a 128-note bitmask into its twelve pitch-class flags.
fn mask_from_bits(bits: u128) -> NoteMask {
    NoteMask(std::array::from_fn(|class| {
        (bits & (CLASS_C_NOTES << class)) != 0
    }))
}

/// The prepared correction periods and the arithmetic midpoint boundaries.
pub(crate) struct CorrectionTable {
    /// The number of active targets.
    ///
    /// Only the first `count` entries are used.
    pub count: usize,
    /// The target words in increasing order, normalized to one octave.
    ///
    /// The correction search reads them as Q6 sample periods. Only the first
    /// `count` entries are active.
    pub periods: [i32; 12],
    /// The arithmetic boundaries, in the same word scale as `periods`.
    ///
    /// The `count + 1` entries enclose the octave.
    pub midpoints: [i32; 13],
    /// The pitch classes 0..=11 of the active periods, in reverse note order.
    pub notes: [i32; 12],
    /// The prepared periods for all classes.
    ///
    /// If the sample rate changes, `set_sample_rate` builds the active targets
    /// again from these periods.
    base_periods: [i32; 12],
    /// The held MIDI keys 0..=127.
    keyboard: u128,
    /// The latched MIDI keys 0..=127 for [`CorrectionMidiMode::Latch`].
    latched: u128,
    /// If `true`, correction MIDI currently overrides the configured notes.
    midi_active: bool,
}

impl CorrectionTable {
    /// Prepares the base periods, loads the selected mask and clears all keys.
    pub(crate) fn new(rate: SampleRate, mask: NoteMask) -> Self {
        let mut result = Self {
            count: 0,
            periods: [0; 12],
            midpoints: [0; 13],
            notes: [0; 12],
            base_periods: [0; 12],
            keyboard: 0,
            latched: 0,
            midi_active: false,
        };
        result.set_sample_rate(rate);
        result.set_notes(mask);
        result
    }

    /// Calculates the period table again for `rate` and builds the active
    /// targets again.
    ///
    /// If `count` is zero, the active targets and their midpoint boundaries do
    /// not change.
    pub(crate) fn set_sample_rate(&mut self, rate: SampleRate) {
        let frequency = ((-9.0 * std::f64::consts::LN_2 / 12.0).exp() * 440.0) as f32;
        let mut base = rate.hz() / frequency;
        while base < 10820.625 {
            base += base;
        }
        self.base_periods = std::array::from_fn(|n| {
            (f64::from(base) * 2.0_f64.powf((100.0 + n as f64 * 100.0) / 1200.0) + 0.5) as i32
        });
        if self.count > 0 {
            let mut mask = [false; 12];
            for &note in &self.notes[..self.count] {
                mask[note as usize] = true;
            }
            self.set_notes(NoteMask(mask));
        }
    }
    /// Builds the active targets and their midpoint boundaries again.
    ///
    /// The boundaries are in period space. Entries after `count` stay unused.
    /// An empty mask sets `count` to zero. The midpoint entries are then
    /// ignored, not cleared.
    pub(crate) fn set_notes(&mut self, mask: NoteMask) {
        self.count = 0;
        for i in 0..12 {
            if mask.0[11 - i] {
                self.periods[self.count] = self.base_periods[i];
                self.notes[self.count] = (11 - i) as i32;
                self.count += 1;
            }
        }
        if self.count > 0 {
            for i in 1..self.count {
                self.midpoints[i] = (self.periods[i - 1] + self.periods[i]) / 2;
            }
            self.midpoints[self.count] = (self.periods[self.count - 1] + self.periods[0] * 2) / 2;
            self.midpoints[0] = self.midpoints[self.count] / 2;
        }
    }
    /// Clears any latched chord and loads the configured `fallback` mask.
    pub(crate) fn clear_override(&mut self, fallback: NoteMask) {
        self.latched = 0;
        self.midi_active = false;
        self.set_notes(fallback);
    }
    /// Updates the override state for a new `mode`, and returns `true` if the
    /// active target table changed.
    pub(crate) fn apply_mode(&mut self, mode: CorrectionMidiMode, fallback: NoteMask) -> bool {
        match mode {
            CorrectionMidiMode::Momentary => {
                self.latched = 0;
                if self.keyboard == 0 {
                    if self.midi_active {
                        self.midi_active = false;
                        self.set_notes(fallback);
                        return true;
                    }
                } else {
                    self.midi_active = true;
                    self.set_notes(mask_from_bits(self.keyboard));
                    return true;
                }
                false
            }
            CorrectionMidiMode::Latch => {
                if self.keyboard != 0 {
                    self.latched = self.keyboard;
                    self.midi_active = true;
                    self.set_notes(mask_from_bits(self.latched));
                    return true;
                }
                false
            }
        }
    }
    /// Releases all keys 0..=127 and, if a MIDI override was active, restores
    /// `fallback`.
    ///
    /// If no MIDI override was active, the prepared target table does not
    /// reload.
    pub(crate) fn reset(&mut self, fallback: NoteMask) {
        self.keyboard = 0;
        self.latched = 0;
        if self.midi_active {
            self.midi_active = false;
            self.set_notes(fallback);
        }
    }
    /// Applies an event whose MIDI ranges were already checked.
    pub(crate) fn apply_validated_midi(
        &mut self,
        event: MidiEvent,
        mode: CorrectionMidiMode,
        fallback: NoteMask,
    ) {
        match event {
            MidiEvent::NoteOn { note, velocity, .. } if velocity != 0 => {
                let bit = 1_u128 << note;
                let was_empty = self.keyboard == 0;
                self.keyboard |= bit;
                self.latched = if was_empty { bit } else { self.latched | bit };
                self.midi_active = true;
                let active = match mode {
                    CorrectionMidiMode::Momentary => self.keyboard,
                    CorrectionMidiMode::Latch => self.latched,
                };
                self.set_notes(mask_from_bits(active));
            }
            MidiEvent::NoteOn { note, .. } | MidiEvent::NoteOff { note, .. } => {
                self.keyboard &= !(1_u128 << note);
                if mode == CorrectionMidiMode::Momentary {
                    if self.keyboard == 0 {
                        self.midi_active = false;
                        self.set_notes(fallback);
                    } else {
                        self.midi_active = true;
                        self.set_notes(mask_from_bits(self.keyboard));
                    }
                }
            }
            MidiEvent::AllNotesOff => {
                self.keyboard = 0;
                self.latched = 0;
                self.midi_active = false;
                self.set_notes(fallback);
            }
        }
    }
}
