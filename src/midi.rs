//! MIDI note messages, pitch periods and the conversion of velocity to gain.

use crate::numeric::fixed::{Q12, Q6};
use crate::Error;

/// The supported MIDI note controls, with zero-based channel numbers.
///
/// Use [`crate::ControlEvent::Midi`] for harmony. Use
/// [`crate::ControlEvent::CorrectionMidi`] for the independent correction
/// keyboard. The typed representation can contain `u8` values that are not
/// valid. [`Self::validate`] and the event preflight of the processor check
/// the MIDI ranges. Note numbers use A4 = 69 at 440 Hz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// All notes stop (controller 123).
    ///
    /// For harmony, it clears the active MIDI assignments and the stolen notes
    /// that are still held, on all channels. In MIDI-chord mode, it also
    /// releases the chord voices. The gain envelopes do not change, so the
    /// released voices decay. The prepared targets of the named and degree
    /// chord sources do not change. For the correction keyboard, it clears
    /// keys 0..=126 and keeps key 127.
    AllNotesOff,
}

impl MidiEvent {
    /// Decodes one complete MIDI message of three bytes.
    ///
    /// This function accepts Note On, Note Off and controller 123 (All Notes
    /// Off). A Note On with velocity zero becomes Note Off. The function
    /// ignores the release velocity of Note Off and the value of controller
    /// 123. All Notes Off does not keep the source channel. Thus, for harmony,
    /// it acts on all channels. Refer to [`Self::AllNotesOff`] for the envelope
    /// and chord state that it keeps.
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
            0xb0 if bytes[1] == 123 => Some(Self::AllNotesOff),
            _ => None,
        })
    }

    /// Checks the MIDI ranges and does not apply the event.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidMidi`] if a channel is more than 15, or if a
    /// note or a velocity is more than 127. Zero velocity and All Notes Off
    /// are valid.
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
            _ => Ok(()),
        }
    }
}

/// Returns the period of MIDI `note` at `rate`, as a Q6 count of samples.
pub(crate) fn note_period(rate: f32, note: i32) -> Q6 {
    let period = ((f64::from(rate) / 440.0)
        / ((f64::from(note as f32 - 69.0) * std::f64::consts::LN_2) / 12.0).exp())
        as f32;
    Q6::from_f64(f64::from(period))
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
