//! Pitch classes and correction tables, made from scales or held MIDI notes.

use crate::midi::{bend_ratio, MidiEvent};
use crate::numeric::fixed::Q14;
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
    pub keyboard: [bool; 128],
    /// The keys that Note Off released while sustain was active.
    sustained: [bool; 128],
    /// The sustain-pedal state (controller 64) for each MIDI channel.
    sustain_held: [bool; 16],
    /// The centered 14-bit pitch bend across all channels of correction MIDI.
    pitch_bend: i16,
    /// The configured maximum pitch bend in semitones.
    pitch_bend_semitones: f32,
    /// The Q14 frequency multiplier from `pitch_bend` and
    /// `pitch_bend_semitones`.
    pub bend_ratio: Q14,
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
            keyboard: [false; 128],
            sustained: [false; 128],
            sustain_held: [false; 16],
            pitch_bend: 0,
            pitch_bend_semitones: 2.0,
            bend_ratio: Q14::ONE,
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
    /// Updates the pitch-bend range and calculates `bend_ratio` again.
    pub(crate) fn set_pitch_bend_semitones(&mut self, semitones: f32) {
        self.pitch_bend_semitones = semitones;
        self.bend_ratio = bend_ratio(self.pitch_bend, semitones);
    }
    /// Releases keys 0..=127, clears sustain and pitch bend, and loads `notes`
    /// again.
    pub(crate) fn reset(&mut self, notes: NoteMask) {
        self.keyboard.fill(false);
        self.sustained.fill(false);
        self.sustain_held.fill(false);
        self.pitch_bend = 0;
        self.bend_ratio = Q14::ONE;
        self.set_notes(notes);
    }
    /// Applies an event whose MIDI ranges were already checked, and returns
    /// `true` if the active note table reloaded.
    ///
    /// If no key in 0..=127 is held or sustained, this method loads `notes`
    /// again.
    pub(crate) fn apply_validated_midi(&mut self, event: MidiEvent, notes: NoteMask) -> bool {
        match event {
            MidiEvent::NoteOn { note, velocity, .. } if velocity != 0 => {
                let idx = usize::from(note);
                self.keyboard[idx] = true;
                self.sustained[idx] = false;
            }
            MidiEvent::NoteOn { note, .. } | MidiEvent::NoteOff { note, .. } => {
                let idx = usize::from(note);
                if self.keyboard[idx] {
                    self.keyboard[idx] = false;
                    if self.sustain_held.contains(&true) {
                        self.sustained[idx] = true;
                    }
                }
            }
            MidiEvent::PitchBend { value, .. } => {
                self.pitch_bend = value;
                self.bend_ratio = bend_ratio(value, self.pitch_bend_semitones);
                return false;
            }
            MidiEvent::SustainPedal { channel, held } => {
                self.sustain_held[usize::from(channel)] = held;
                if held || self.sustain_held.contains(&true) {
                    return false;
                }
                let had_sustained = self.sustained.contains(&true);
                self.sustained.fill(false);
                if !had_sustained {
                    return false;
                }
            }
            MidiEvent::AllNotesOff => {
                self.reset(notes);
                return true;
            }
        }
        let mut mask = [false; 12];
        for i in 0..128 {
            mask[i % 12] |= self.keyboard[i] || self.sustained[i];
        }
        if mask == [false; 12] {
            self.set_notes(notes);
        } else {
            self.set_notes(NoteMask(mask));
        }
        true
    }
}
