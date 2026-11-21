//! Reverse-loop playback with a continuous input history and boundary fades.

use crate::error::check_count;
use crate::numeric::fixed::{Fixed, Q14};
use crate::{Error, SampleRate};

/// Returns the stereo history capacity in frames for `rate` (approximately
/// four seconds; 192,000 frames at 44.1 and 48 kHz).
const fn history_frames(rate: SampleRate) -> usize {
    match rate {
        SampleRate::Hz8000 => 32_000,
        SampleRate::Hz16000 => 64_000,
        SampleRate::Hz22050 | SampleRate::Hz24000 => 96_000,
        SampleRate::Hz32000 => 128_000,
        SampleRate::Hz44100 | SampleRate::Hz48000 => 192_000,
        SampleRate::Hz88200 | SampleRate::Hz96000 => 384_000,
        SampleRate::Hz192000 => 768_000,
    }
}

/// Returns the boundary fade length in frames (~4.54 ms; 200 frames at
/// 44.1 kHz).
const fn fade_frames(rate: SampleRate) -> usize {
    let frames = (200 * rate.as_u32() as usize + 22_050) / 44_100;
    if frames < 1 {
        1
    } else {
        frames
    }
}

/// Reverse playback of recent stereo audio in repeated loops with fades.
///
/// Alien comes after resynthesis. Its input history and loop phase advance
/// also when Alien is disabled. Thus, when you enable Alien, it can play
/// audio that it recorded during bypass. A change of the loop length keeps
/// the history and clamps the current phase into the new loop. The boundary
/// fades scale with the sample rate (approximately 4.54 ms; 200 frames at
/// 44.1 kHz).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub struct AlienConfig {
    /// Selects reversed playback instead of the direct input. The default is
    /// `false`. The recording and the phase continue to advance in both
    /// states.
    pub enabled: bool,
    /// The loop length in frames, 1..=384000. The default is 95,999.
    ///
    /// This value is a frame count, so its duration in seconds changes with
    /// the sample rate. The active loop clamps to half of the prepared
    /// history capacity (approximately two seconds; 96,000 frames at 44.1 and
    /// 48 kHz). A short loop can be fully inside the fades.
    pub loop_frames: u32,
}
impl Default for AlienConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            loop_frames: 95_999,
        }
    }
}
impl AlienConfig {
    /// Validates all fields, also when Alien is disabled.
    ///
    /// `loop_frames` must be in its documented inclusive range. Validation
    /// does not clamp values. It does not change the processor state.
    ///
    /// # Errors
    ///
    /// Returns the first [`Error::InvalidConfig`] with its dotted field path
    /// and the problem.
    pub fn validate(self) -> Result<(), Error> {
        check_count("alien.loop_frames", i64::from(self.loop_frames), 1, 384_000)
    }
}

/// A stereo ring recorder with a reverse reader and an independent loop
/// phase.
#[derive(Debug)]
pub(crate) struct Alien {
    /// The validated controls, which include the selection of the wet output.
    config: AlienConfig,
    /// The boundary fade length in frames for the prepared rate.
    fade_frames: usize,
    /// The stereo Q14 history frames, allocated one time at construction.
    buffer: Vec<[Q14; 2]>,
    /// The current recording position. The position of the reverse reader
    /// comes from this cursor.
    read: usize,
    /// The current frame in the loop. It is always less than `loop_len`.
    phase: usize,
    /// The active loop length. It is not more than half of the history
    /// capacity.
    loop_len: usize,
}
impl Alien {
    /// Allocates a silent stereo ring from validated loop settings.
    pub(crate) fn new(rate: SampleRate, config: AlienConfig) -> Self {
        let history = history_frames(rate);
        let loop_len = (config.loop_frames as usize).min(history / 2);
        Self {
            config,
            fade_frames: fade_frames(rate),
            buffer: vec![[Q14::default(); 2]; history],
            read: 0,
            phase: 0,
            loop_len,
        }
    }
    /// Applies a validated configuration, keeps the recorded audio and clamps
    /// the phase into the new loop, which is not empty.
    pub(crate) fn set_config(&mut self, config: AlienConfig) {
        self.config = config;
        self.loop_len = (config.loop_frames as usize).min(self.buffer.len() / 2);
        self.phase = self.phase.min(self.loop_len - 1);
    }
    /// Returns the loop length and the current frame in the loop.
    ///
    /// The frame is always less than the length. The stage state does not
    /// change.
    pub(crate) fn loop_state(&self) -> (usize, usize) {
        (self.loop_len, self.phase)
    }
    /// Clears the recorded audio and sets both cursors to zero, but keeps the
    /// configured loop.
    pub(crate) fn reset(&mut self) {
        self.buffer.fill([Q14::default(); 2]);
        self.read = 0;
        self.phase = 0;
    }
    /// Records a frame, selects the direct or reverse output, then advances
    /// both cursors.
    /// The recording cursor moves forward by one and the reverse offset
    /// increases by two. Thus, the playback moves backward by one frame. The
    /// active loop limit keeps that subtraction in a single ring wrap.
    pub(crate) fn process_sample(&mut self, input: [Q14; 2]) -> [Q14; 2] {
        let history = self.buffer.len();
        self.buffer[self.read] = input;
        let result = if self.config.enabled {
            let read = self.read + history - self.phase * 2;
            let read = if read >= history {
                read - history
            } else {
                read
            };
            let distance = self.phase.min(self.loop_len - self.phase);
            let mut result = self.buffer[read];
            if distance < self.fade_frames {
                let gain: Q14 = Fixed::<0>::from_bits(distance as i32)
                    .div(Fixed::<0>::from_bits(self.fade_frames as i32));
                result = result.map(|sample| sample.mul::<14, 14>(gain));
            }
            result
        } else {
            input
        };
        self.read += 1;
        if self.read == history {
            self.read = 0;
        }
        self.phase += 1;
        if self.phase == self.loop_len {
            self.phase = 0;
        }
        result
    }
}

/// Returns the finite tail budget: two active loop lengths plus the boundary
/// fade.
///
/// The value is zero when Alien is disabled. It is a configured budget,
/// not a measured tail.
pub(crate) fn tail_frames(rate: SampleRate, config: AlienConfig) -> usize {
    if config.enabled {
        let loop_len = (config.loop_frames as usize).min(history_frames(rate) / 2);
        2 * loop_len + fade_frames(rate)
    } else {
        0
    }
}
