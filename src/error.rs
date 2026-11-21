//! Structured errors for configuration, buffer shapes and timed controls.

use std::fmt;

/// The reason for a rejected call.
///
/// A call that returns this error validates before it acts. If it returns
/// `Err`, the state and the output do not change.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum Error {
    /// The numeric rate has no [`crate::SampleRate`] variant that matches it.
    UnsupportedSampleRate {
        /// The unsupported rate, in hertz.
        hz: u32,
    },
    /// A frequency is not finite or is outside the interval `0 < hz <= max_hz`.
    InvalidFrequency {
        /// The rejected frequency, in hertz.
        hz: f32,
        /// The inclusive upper limit, half of the sample rate in hertz.
        max_hz: f32,
    },
    /// A configuration field is not valid.
    ///
    /// `field` is the dotted path of the field from the root configuration,
    /// for example `"resynthesis.choir_detune"`, `"reverb.mix"` or
    /// `"prepare.max_block_frames"`. For an array control, `index` gives the
    /// zero-based element index.
    InvalidConfig {
        /// The dotted path of the field from the root configuration.
        field: &'static str,
        /// The zero-based element index, if the field is an array.
        index: Option<usize>,
        /// The problem with the field.
        problem: ConfigProblem,
    },
    /// A buffer length is not equal to the input length or to the length of
    /// the other output channel.
    BufferLength {
        /// The name of the mismatched buffer (`"output"`, `"left"` or
        /// `"right"`).
        buffer: &'static str,
        /// The necessary slice length. For `[[i16; 2]]`, this is the number of
        /// stereo frames. For planar or mono audio, it is the number of samples
        /// for each channel.
        expected: usize,
        /// The actual slice length, in the same units as `expected`.
        actual: usize,
    },
    /// A block has more frames than the prepared maximum of the processor.
    BlockTooLarge {
        /// The number of frames in the block.
        frames: usize,
        /// The prepared maximum.
        max: usize,
    },
    /// The timed event at `index` is not valid.
    InvalidEvent {
        /// The index of the event in the slice.
        index: usize,
        /// The problem with the event.
        problem: EventProblem,
    },
    /// The MIDI bytes or the MIDI fields are not valid.
    InvalidMidi {
        /// The problem with the MIDI bytes or fields.
        problem: MidiProblem,
    },
}

/// The problem with a configuration field.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum ConfigProblem {
    /// The value is outside `min..=max`.
    OutOfRange {
        /// The rejected value.
        value: f64,
        /// The lowest valid value.
        min: f64,
        /// The highest valid value.
        max: f64,
    },
    /// The size is outside `min..=max`.
    SizeOutOfRange {
        /// The rejected size.
        value: usize,
        /// The lowest valid size.
        min: usize,
        /// The highest valid size.
        max: usize,
    },
    /// The value is NaN or infinite.
    NotFinite {
        /// The rejected value.
        value: f64,
    },
    /// The value must be below the value of the field `other`.
    Unordered {
        /// The rejected value of the field.
        value: f32,
        /// The dotted path of the field that the value must stay below.
        other: &'static str,
        /// The value of `other`.
        other_value: f32,
    },
}

/// The problem with raw MIDI bytes or typed MIDI fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MidiProblem {
    /// The status byte is below `0x80`.
    StatusByte {
        /// The rejected status byte.
        byte: u8,
    },
    /// A data byte is above 127.
    DataByte {
        /// The rejected data byte.
        byte: u8,
    },
    /// The zero-based MIDI channel is above 15.
    Channel {
        /// The rejected channel number.
        channel: u8,
    },
    /// The MIDI note number is above 127.
    Note {
        /// The rejected note number.
        note: u8,
    },
    /// The MIDI velocity is above 127.
    Velocity {
        /// The rejected velocity.
        velocity: u8,
    },
}

/// The reason for the rejection of a timed event during the block preflight.
///
/// The preflight examines the full block. For each event, it checks the offset
/// bounds first, then the order, then the payload. The [`Error::InvalidEvent`]
/// that contains this value identifies the first event in slice order that
/// fails.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum EventProblem {
    /// The zero-based offset is equal to or more than the block frame count.
    ///
    /// Each event in an empty block has this problem.
    OffsetOutsideBlock {
        /// The rejected zero-based frame offset.
        offset: usize,
        /// The number of frames in the block.
        frames: usize,
    },
    /// The offset is less than the offset of the previous event.
    ///
    /// Equal offsets are valid.
    OffsetDecreasing {
        /// The rejected zero-based frame offset.
        offset: usize,
        /// The zero-based frame offset of the previous event.
        previous: usize,
    },
    /// The event has a configuration field that is not valid.
    Config {
        /// The dotted path of the field from the root configuration.
        field: &'static str,
        /// The zero-based element index, if the field is an array.
        index: Option<usize>,
        /// The problem with the field.
        problem: ConfigProblem,
    },
    /// The event has a MIDI message that is not valid.
    Midi {
        /// The problem with the MIDI message.
        problem: MidiProblem,
    },
}

impl Error {
    /// Returns an `InvalidConfig` error with a stable, static dotted path.
    pub(crate) fn config(field: &'static str, problem: ConfigProblem) -> Self {
        Self::InvalidConfig {
            field,
            index: None,
            problem,
        }
    }

    /// Returns an `InvalidConfig` error for one element of an array field.
    pub(crate) fn config_element(
        field: &'static str,
        index: usize,
        problem: ConfigProblem,
    ) -> Self {
        Self::InvalidConfig {
            field,
            index: Some(index),
            problem,
        }
    }

    /// Returns an `InvalidMidi` error with the specific MIDI problem.
    pub(crate) fn midi(problem: MidiProblem) -> Self {
        Self::InvalidMidi { problem }
    }

    /// Returns an error if `actual` is not equal to `expected`.
    ///
    /// The stereo and planar process calls, the planar drain call and the
    /// formant envelope call use this length check. The mono calls have only
    /// one buffer.
    pub(crate) fn check_length(
        buffer: &'static str,
        expected: usize,
        actual: usize,
    ) -> Result<(), Self> {
        if expected == actual {
            Ok(())
        } else {
            Err(Self::BufferLength {
                buffer,
                expected,
                actual,
            })
        }
    }
}

/// Checks that a control is finite and in `min..=max`.
///
/// The comparison uses single precision, because the controls are defined in
/// single precision.
pub(crate) fn check_range(
    field: &'static str,
    value: f32,
    min: f32,
    max: f32,
) -> Result<(), Error> {
    if !value.is_finite() {
        return Err(Error::config(
            field,
            ConfigProblem::NotFinite {
                value: f64::from(value),
            },
        ));
    }
    if !(min..=max).contains(&value) {
        let (value, min, max) = (f64::from(value), f64::from(min), f64::from(max));
        return Err(Error::config(
            field,
            ConfigProblem::OutOfRange { value, min, max },
        ));
    }
    Ok(())
}

/// Checks an integer control against `min..=max`.
pub(crate) fn check_count(
    field: &'static str,
    value: i64,
    min: i64,
    max: i64,
) -> Result<(), Error> {
    if !(min..=max).contains(&value) {
        let (value, min, max) = (value as f64, min as f64, max as f64);
        return Err(Error::config(
            field,
            ConfigProblem::OutOfRange { value, min, max },
        ));
    }
    Ok(())
}

/// Checks one integer element of an array control against `min..=max`.
pub(crate) fn check_indexed_count(
    field: &'static str,
    index: usize,
    value: i64,
    min: i64,
    max: i64,
) -> Result<(), Error> {
    if !(min..=max).contains(&value) {
        let (value, min, max) = (value as f64, min as f64, max as f64);
        return Err(Error::config_element(
            field,
            index,
            ConfigProblem::OutOfRange { value, min, max },
        ));
    }
    Ok(())
}

impl fmt::Display for ConfigProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfRange { value, min, max } => {
                write!(f, "is {value}, must be in {min}..={max}")
            }
            Self::SizeOutOfRange { value, min, max } => {
                write!(f, "is {value}, must be in {min}..={max}")
            }
            Self::NotFinite { value } => write!(f, "is {value}, must be finite"),
            Self::Unordered {
                value,
                other,
                other_value,
            } => write!(f, "is {value}, must be below {other} ({other_value})"),
        }
    }
}

impl fmt::Display for MidiProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StatusByte { byte } => {
                write!(
                    f,
                    "MIDI status byte 0x{byte:02x} ({byte}) must be in 0x80..=0xff"
                )
            }
            Self::DataByte { byte } => write!(f, "MIDI data byte {byte} must be in 0..=127"),
            Self::Channel { channel } => write!(f, "MIDI channel {channel} must be in 0..=15"),
            Self::Note { note } => write!(f, "MIDI note {note} must be in 0..=127"),
            Self::Velocity { velocity } => write!(f, "MIDI velocity {velocity} must be in 0..=127"),
        }
    }
}

impl fmt::Display for EventProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OffsetOutsideBlock { offset, frames } => {
                write!(f, "offset {offset} is outside the block of {frames} frames")
            }
            Self::OffsetDecreasing { offset, previous } => {
                write!(
                    f,
                    "offset {offset} is below the previous event's offset {previous}"
                )
            }
            Self::Config {
                field,
                index: Some(index),
                problem,
            } => write!(f, "{field}[{index}] {problem}"),
            Self::Config {
                field,
                index: None,
                problem,
            } => write!(f, "{field} {problem}"),
            Self::Midi { problem } => write!(f, "{problem}"),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSampleRate { hz } => write!(f, "unsupported sample rate: {hz} Hz"),
            Self::InvalidFrequency { hz, max_hz } => {
                write!(
                    f,
                    "frequency {hz} Hz must be finite and in 0 < hz <= {max_hz}"
                )
            }
            Self::InvalidConfig {
                field,
                index: Some(index),
                problem,
            } => {
                write!(f, "invalid config: {field}[{index}] {problem}")
            }
            Self::InvalidConfig {
                field,
                index: None,
                problem,
            } => {
                write!(f, "invalid config: {field} {problem}")
            }
            Self::BufferLength {
                buffer,
                expected,
                actual,
            } => {
                write!(f, "{buffer} buffer length is {actual}, expected {expected}")
            }
            Self::BlockTooLarge { frames, max } => {
                write!(
                    f,
                    "block of {frames} frames is larger than the prepared {max}"
                )
            }
            Self::InvalidEvent { index, problem } => write!(f, "event {index}: {problem}"),
            Self::InvalidMidi { problem } => write!(f, "{problem}"),
        }
    }
}

impl std::error::Error for Error {}
