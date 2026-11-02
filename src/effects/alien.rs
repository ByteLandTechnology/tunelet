//! Reverse-loop playback with a continuous input history and boundary fades.

use crate::error::check_count;
use crate::numeric::fixed::{Fixed, Q14};
use crate::Error;

/// The number of history frames that the loop reads from.
const HISTORY_FRAMES: usize = 192_000;
/// The number of frames for the boundary fade at each loop boundary.
const FADE_FRAMES: usize = 200;

/// Reverse playback of recent stereo audio in repeated loops with fades.
///
/// Alien comes after resynthesis. Its input history and loop phase advance
/// also when Alien is disabled. Thus, when you enable Alien, it can play
/// audio that it recorded during bypass. A change of the loop length keeps
/// the history and clamps the current phase into the new loop. The boundary
/// fades are 200 frames long at all sample rates.
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
    /// The loop length in frames, 1..=96000. The default is 95,999.
    /// This value is a frame count at the two rates. Thus, its duration in
    /// seconds changes with the sample rate. A short loop can be fully inside
    /// the fades.
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
        check_count("alien.loop_frames", i64::from(self.loop_frames), 1, 96_000)
    }
}

/// A stereo ring recorder with a reverse reader and an independent loop
/// phase.
#[derive(Debug)]
pub(crate) struct Alien {
    /// The validated controls, which include the selection of the wet output.
    config: AlienConfig,
    /// 192,000 stereo Q14 frames, which are allocated one time at
    /// construction.
    buffer: Vec<[Q14; 2]>,
    /// The current recording position. The position of the reverse reader
    /// comes from this cursor.
    read: usize,
    /// The current frame in the loop. It is always less than `loop_len`.
    phase: usize,
    /// The validated loop length. It is not more than half of the history
    /// capacity.
    loop_len: usize,
}
impl Alien {
    /// Allocates a silent stereo ring from validated loop settings.
    pub(crate) fn new(config: AlienConfig) -> Self {
        Self {
            config,
            buffer: vec![[Q14::default(); 2]; HISTORY_FRAMES],
            read: 0,
            phase: 0,
            loop_len: config.loop_frames as usize,
        }
    }
    /// Applies a validated configuration, keeps the recorded audio and clamps
    /// the phase into the new loop, which is not empty.
    pub(crate) fn set_config(&mut self, config: AlienConfig) {
        self.config = config;
        self.loop_len = config.loop_frames as usize;
        self.phase = self.phase.min(self.loop_len - 1);
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
    /// validated loop limit keeps that subtraction in a single ring wrap.
    pub(crate) fn process_sample(&mut self, input: [Q14; 2]) -> [Q14; 2] {
        self.buffer[self.read] = input;
        let result = if self.config.enabled {
            let read = self.read + HISTORY_FRAMES - self.phase * 2;
            let read = if read >= HISTORY_FRAMES {
                read - HISTORY_FRAMES
            } else {
                read
            };
            let distance = self.phase.min(self.loop_len - self.phase);
            let mut result = self.buffer[read];
            if distance < FADE_FRAMES {
                let gain: Q14 = Fixed::<0>::from_bits(distance as i32)
                    .div(Fixed::<0>::from_bits(FADE_FRAMES as i32));
                result = result.map(|sample| sample.mul::<14, 14>(gain));
            }
            result
        } else {
            input
        };
        self.read += 1;
        if self.read == HISTORY_FRAMES {
            self.read = 0;
        }
        self.phase += 1;
        if self.phase == self.loop_len {
            self.phase = 0;
        }
        result
    }
}

/// Returns the finite tail budget: two loop lengths plus the boundary
/// fade.
///
/// The value is zero when Alien is disabled. It is a configured budget,
/// not a measured tail.
pub(crate) fn tail_frames(config: AlienConfig) -> usize {
    if config.enabled {
        2 * config.loop_frames as usize + FADE_FRAMES
    } else {
        0
    }
}
