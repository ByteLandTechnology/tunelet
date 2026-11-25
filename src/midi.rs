//! MIDI messages, pitch-bend ratios and the conversion of velocity to gain.

use crate::numeric::fixed::{Q12, Q14};
use crate::Error;

/// The supported MIDI controls, with zero-based channel numbers.
///
/// Use [`crate::ControlEvent::Midi`] for harmony. Use
/// [`crate::ControlEvent::CorrectionMidi`] for the independent correction
/// keyboard. The typed representation can contain `u8` and `i16` values that
/// are not valid. [`Self::validate`] and the event preflight of the processor
/// check the MIDI ranges. Note numbers use A4 = 69 at 440 Hz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase", tag = "type")
)]
pub enum MidiEvent {
    /// A note starts.
    NoteOn {
        /// The MIDI channel, 0..=15.
        channel: u8,
        /// The MIDI note, 0..=127.
        note: u8,
        /// The velocity, 0..=127.
        ///
        /// Zero releases the note, as [`Self::NoteOff`] does.
        velocity: u8,
    },
    /// A note stops.
    NoteOff {
        /// The MIDI channel, 0..=15.
        channel: u8,
        /// The MIDI note, 0..=127.
        note: u8,
    },
    /// The pitch wheel moves on a channel.
    ///
    /// A value of `0` gives no bend. `-8192` lowers the pitch by the
    /// configured bend range, and `8191` raises it by that range. In
    /// [`crate::HarmonySource::MidiChannels`], channels `0..=3` bend voices
    /// `0..=3`. Other harmony sources and the correction keyboard use the
    /// latest bend from any channel.
    PitchBend {
        /// The MIDI channel, 0..=15.
        channel: u8,
        /// The centered 14-bit bend value, -8192..=8191.
        value: i16,
    },
    /// The sustain pedal changes state (controller 64).
    ///
    /// While sustain is active, [`Self::NoteOff`] holds active notes until the
    /// pedal releases. [`crate::HarmonySource::MidiChannels`] tracks channels
    /// `0..=3` separately. [`crate::HarmonySource::MidiOmni`],
    /// [`crate::HarmonySource::ChordViaMidi`] and the correction keyboard
    /// combine all channels. When all 16 channels release the pedal, the
    /// sustained notes release.
    SustainPedal {
        /// The MIDI channel, 0..=15.
        channel: u8,
        /// `true` holds the pedal, and `false` releases it.
        held: bool,
    },
    /// All notes stop (controller 123).
    ///
    /// For harmony, it clears the active MIDI assignments, the stolen notes,
    /// the sustained notes, the sustain pedals and the pitch bend on all
    /// channels. In MIDI-chord mode, it also releases the chord voices. The
    /// gain envelopes do not change, so the released voices decay. The
    /// prepared targets of the named and degree chord sources do not change.
    /// For the correction keyboard, it clears keys 0..=126, sustained notes,
    /// sustain pedals and pitch bend, and keeps key 127.
    AllNotesOff,
}

impl MidiEvent {
    /// Decodes one complete MIDI message of three bytes.
    ///
    /// This function accepts Note On, Note Off, Pitch Bend (`0xE0`),
    /// controller 64 (Sustain Pedal) and controller 123 (All Notes Off). A
    /// Note On with velocity zero becomes Note Off. Pitch Bend combines the
    /// 7-bit LSB in `bytes[1]` and the 7-bit MSB in `bytes[2]` into
    /// `-8192..=8191`. Controller 64 sets `held` to `true` when `bytes[2]` is
    /// at least 64. The function ignores the release velocity of Note Off and
    /// the value of controller 123. All Notes Off acts on all channels.
    ///
    /// For other messages with valid ranges of status and data bytes, the
    /// function returns `Ok(None)`. This function is not a parser for a MIDI
    /// stream. It does not assemble running status, messages of variable
    /// length or system-exclusive data.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidMidi`] if the status is below `0x80` or a data
    /// byte is more than 127. This also applies to an unsupported message.
    pub fn from_bytes(bytes: [u8; 3]) -> Result<Option<Self>, Error> {
        if bytes[0] < 0x80 || bytes[1] > 127 || bytes[2] > 127 {
            return Err(Error::InvalidMidi);
        }
        let channel = bytes[0] & 15;
        Ok(match bytes[0] & 0xf0 {
            0x90 if bytes[2] != 0 => Some(Self::NoteOn {
                channel,
                note: bytes[1],
                velocity: bytes[2],
            }),
            0x80 | 0x90 => Some(Self::NoteOff {
                channel,
                note: bytes[1],
            }),
            0xb0 if bytes[1] == 64 => Some(Self::SustainPedal {
                channel,
                held: bytes[2] >= 64,
            }),
            0xb0 if bytes[1] == 123 => Some(Self::AllNotesOff),
            0xe0 => Some(Self::PitchBend {
                channel,
                value: ((i16::from(bytes[2]) << 7) | i16::from(bytes[1])) - 8192,
            }),
            _ => None,
        })
    }

    /// Checks the MIDI ranges and does not apply the event.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidMidi`] if a channel is more than 15, if a note
    /// or a velocity is more than 127, or if a pitch-bend value is outside
    /// `-8192..=8191`. Zero velocity and All Notes Off are valid.
    pub fn validate(&self) -> Result<(), Error> {
        match *self {
            Self::NoteOn {
                channel,
                note,
                velocity,
            } if channel > 15 || note > 127 || velocity > 127 => Err(Error::InvalidMidi),
            Self::NoteOff { channel, note } if channel > 15 || note > 127 => {
                Err(Error::InvalidMidi)
            }
            Self::PitchBend { channel, value }
                if channel > 15 || !(-8192..=8191).contains(&value) =>
            {
                Err(Error::InvalidMidi)
            }
            Self::SustainPedal { channel, .. } if channel > 15 => Err(Error::InvalidMidi),
            _ => Ok(()),
        }
    }
}

/// Returns the Q14 frequency multiplier for `value` and `semitones`.
///
/// If `value` is zero or `semitones` is zero, this function returns unity.
/// Negative values divide by 8192, and positive values divide by 8191.
pub(crate) fn bend_ratio(value: i16, semitones: f32) -> Q14 {
    if value == 0 || semitones == 0.0 {
        return Q14::ONE;
    }
    let scale = if value < 0 { 8192.0 } else { 8191.0 };
    let offset = (f64::from(value) / scale) * f64::from(semitones);
    Q14::from_f64((offset * std::f64::consts::LN_2 / 12.0).exp())
}

/// Returns the MIDI gain for `velocity`, quantized to Q12.
///
/// For velocities 1..=127, the gain is `36 * velocity / 127 - 24` dB.
/// Velocity 0 is silent. Velocity 1 gives approximately -23.72 dB, and
/// velocity 127 gives +12 dB.
pub(crate) fn velocity_gain(velocity: u8) -> Q12 {
    if velocity == 0 {
        return Q12::ZERO;
    }
    let db = (36.0 / 127.0) * f64::from(velocity) - 24.0;
    let gain = 10.0_f64.powf(db / 20.0) as f32;
    Q12::from_f64(f64::from(gain))
}
