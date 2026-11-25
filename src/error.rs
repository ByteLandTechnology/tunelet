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
    /// `"prepare.max_block_frames"`.
    InvalidConfig {
        /// The dotted path of the field from the root configuration.
        field: &'static str,
        /// The problem with the field.
        problem: ConfigProblem,
    },
    /// A buffer length is not equal to the input length or to the length of
    /// the other output channel.
    BufferLength {
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
    ///
    /// The causes are:
    ///
    /// - a status byte below 0x80
    /// - a data byte above 127
    /// - a typed event with a channel, note or velocity out of range.
    InvalidMidi,
}

/// The problem with a configuration field.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum ConfigProblem {
    /// The value is outside `min..=max`.
    OutOfRange {
        /// The lowest valid value.
        min: f64,
        /// The highest valid value.
        max: f64,
    },
    /// The value is NaN or infinite.
    NotFinite,
    /// The value must be below the value of the field `other`.
    Unordered {
        /// The dotted path of the field that the value must stay below.
        other: &'static str,
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
    OffsetOutsideBlock,
    /// The offset is less than the offset of the previous event.
    ///
    /// Equal offsets are valid.
    OffsetDecreasing,
    /// The event has a configuration field that is not valid.
    Config {
        /// The dotted path of the field from the root configuration.
        field: &'static str,
        /// The problem with the field.
        problem: ConfigProblem,
    },
    /// The event has a MIDI message that is not valid.
    Midi,
}

impl Error {
    /// Returns an `InvalidConfig` error with a stable, static dotted path.
    pub(crate) fn config(field: &'static str, problem: ConfigProblem) -> Self {
        Self::InvalidConfig { field, problem }
    }

    /// Returns an error if `actual` is not equal to `expected`.
    ///
    /// The stereo and planar process calls and the planar drain call use this
    /// length check. The mono calls have only one buffer.
    pub(crate) fn check_length(expected: usize, actual: usize) -> Result<(), Self> {
        if expected == actual {
            Ok(())
        } else {
            Err(Self::BufferLength { expected, actual })
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
        return Err(Error::config(field, ConfigProblem::NotFinite));
    }
    if !(min..=max).contains(&value) {
        let (min, max) = (f64::from(min), f64::from(max));
        return Err(Error::config(field, ConfigProblem::OutOfRange { min, max }));
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
        let (min, max) = (min as f64, max as f64);
        return Err(Error::config(field, ConfigProblem::OutOfRange { min, max }));
    }
    Ok(())
}

impl fmt::Display for ConfigProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfRange { min, max } => write!(f, "must be in {min}..={max}"),
            Self::NotFinite => f.write_str("must be finite"),
            Self::Unordered { other } => write!(f, "must be below {other}"),
        }
    }
}

impl fmt::Display for EventProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OffsetOutsideBlock => f.write_str("offset is outside the block"),
            Self::OffsetDecreasing => f.write_str("offset is below the previous event's offset"),
            Self::Config { field, problem } => write!(f, "{field} {problem}"),
            Self::Midi => f.write_str("MIDI message is not valid"),
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
            Self::InvalidConfig { field, problem } => {
                write!(f, "invalid config: {field} {problem}")
            }
            Self::BufferLength { expected, actual } => {
                write!(f, "buffer length is {actual}, expected {expected}")
            }
            Self::BlockTooLarge { frames, max } => {
                write!(
                    f,
                    "block of {frames} frames is larger than the prepared {max}"
                )
            }
            Self::InvalidEvent { index, problem } => write!(f, "event {index}: {problem}"),
            Self::InvalidMidi => f.write_str("MIDI message is not valid"),
        }
    }
}

impl std::error::Error for Error {}
