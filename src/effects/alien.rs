//! Reverse-loop playback with a captured input history and boundary fades.

use crate::error::check_count;
use crate::numeric::fixed::{Fixed, Q14};
use crate::Error;

/// The number of history frames that the loop reads from.
const HISTORY_FRAMES: usize = 192_000;
/// The maximum number of frames for the boundary fade and the wet ramp.
const FADE_FRAMES: usize = 200;

/// Returns the boundary fade length for a validated loop length.
#[inline]
fn boundary_fade_frames(loop_frames: usize) -> usize {
    if loop_frames == 1 {
        0
    } else {
        FADE_FRAMES.min(loop_frames / 2)
    }
}

/// Reverse playback of recent stereo audio in repeated loops with fades.
///
/// Alien comes after resynthesis. When you enable Alien, it captures one loop
/// of fresh input while it holds the dry signal. Then it ramps the wet mix to
/// full level over 200 frames. When you disable Alien, it ramps the wet mix to
/// zero over 200 frames. A change of the loop length keeps the captured history
/// and clamps the current phase into the new loop. The boundary fade length is
/// zero at a loop length of one frame, and `min(200, loop_frames / 2)` at
/// longer loop lengths.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub struct AlienConfig {
    /// Selects reversed playback instead of the direct input. The default is
    /// `false`. Each toggle ramps the wet mix over 200 frames.
    pub enabled: bool,
    /// The loop length in frames, 1..=96000. The default is 95,999.
    /// This value is a frame count at the two rates. Thus, its duration in
    /// seconds changes with the sample rate. The boundary fade is shorter for
    /// short loops and is zero when `loop_frames` is 1.
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
    /// The current write position. The position of the reverse reader comes
    /// from this cursor.
    read: usize,
    /// The current frame in the loop. It is always less than `loop_len`.
    phase: usize,
    /// The validated loop length. It is not more than half of the history
    /// capacity.
    loop_len: usize,
    /// The number of frames captured since Alien became active.
    captured: usize,
    /// The current frame in the 200-frame wet mix ramp, in 0..=200.
    wet_frames: usize,
    /// If `true`, the wet mix ramps to zero before a new capture starts.
    restart_pending: bool,
}
impl Alien {
    /// Allocates a silent stereo ring from validated loop settings.
    pub(crate) fn new(config: AlienConfig) -> Self {
        let (captured, wet_frames) = if config.enabled {
            (HISTORY_FRAMES, FADE_FRAMES)
        } else {
            (0, 0)
        };
        Self {
            config,
            buffer: vec![[Q14::default(); 2]; HISTORY_FRAMES],
            read: 0,
            phase: 0,
            loop_len: config.loop_frames as usize,
            captured,
            wet_frames,
            restart_pending: false,
        }
    }
    /// Applies a validated configuration and does not clear the buffer.
    ///
    /// If you enable Alien, this method resets the loop phase and the captured
    /// frame count. It also clamps the phase into the new loop, which is not
    /// empty.
    pub(crate) fn set_config(&mut self, config: AlienConfig) {
        let was_enabled = self.config.enabled;
        self.config = config;
        self.loop_len = config.loop_frames as usize;
        self.phase = self.phase.min(self.loop_len - 1);
        if !was_enabled && config.enabled {
            if self.wet_frames == 0 {
                self.phase = 0;
                self.captured = 0;
                self.restart_pending = false;
            } else {
                self.restart_pending = true;
            }
        } else if was_enabled && config.enabled {
            if self.captured < self.loop_len && self.wet_frames > 0 {
                self.restart_pending = true;
            }
        } else if !config.enabled {
            self.restart_pending = false;
        }
    }
    /// Returns the loop length and the current frame in the loop.
    ///
    /// The frame is always less than the length. The stage state does not
    /// change.
    pub(crate) fn loop_state(&self) -> (usize, usize) {
        (self.loop_len, self.phase)
    }
    /// Returns the finite tail budget for the active state or an active ramp.
    pub(crate) fn tail_frames(&self) -> usize {
        if self.config.enabled {
            2 * self.config.loop_frames as usize + FADE_FRAMES
        } else {
            self.wet_frames
        }
    }
    /// Clears the recorded audio and resets the cursors and ramps, while it
    /// keeps the configured loop.
    pub(crate) fn reset(&mut self) {
        self.buffer.fill([Q14::default(); 2]);
        self.read = 0;
        self.phase = 0;
        if self.config.enabled {
            self.captured = HISTORY_FRAMES;
            self.wet_frames = FADE_FRAMES;
        } else {
            self.captured = 0;
            self.wet_frames = 0;
        }
        self.restart_pending = false;
    }
    /// Reads the current reverse frame and applies the loop boundary fade.
    #[inline]
    fn reverse_sample(&self) -> [Q14; 2] {
        let read = self.read + HISTORY_FRAMES - self.phase * 2;
        let read = if read >= HISTORY_FRAMES {
            read - HISTORY_FRAMES
        } else {
            read
        };
        let distance = self.phase.min(self.loop_len - self.phase);
        let fade_frames = boundary_fade_frames(self.loop_len);
        let mut result = self.buffer[read];
        if distance < fade_frames {
            let gain: Q14 = Fixed::<0>::from_bits(distance as i32)
                .div(Fixed::<0>::from_bits(fade_frames as i32));
            result = result.map(|sample| sample.mul::<14, 14>(gain));
        }
        result
    }
    /// Blends the direct input and the reverse sample with the Q14 wet ramp.
    #[inline]
    fn mix_wet(&self, input: [Q14; 2], reverse: [Q14; 2]) -> [Q14; 2] {
        if self.wet_frames == 0 {
            input
        } else if self.wet_frames >= FADE_FRAMES {
            reverse
        } else {
            let wet: Q14 = Fixed::<0>::from_bits(self.wet_frames as i32)
                .div(Fixed::<0>::from_bits(FADE_FRAMES as i32));
            let dry = Q14::ONE - wet;
            [
                input[0].mul::<14, 14>(dry) + reverse[0].mul::<14, 14>(wet),
                input[1].mul::<14, 14>(dry) + reverse[1].mul::<14, 14>(wet),
            ]
        }
    }
    /// Advances the write cursor by one frame in the history ring.
    #[inline]
    fn advance_read(&mut self) {
        self.read += 1;
        if self.read == HISTORY_FRAMES {
            self.read = 0;
        }
    }
    /// Advances the loop phase by one frame and wraps at `loop_len`.
    #[inline]
    fn advance_phase(&mut self) {
        self.phase += 1;
        if self.phase == self.loop_len {
            self.phase = 0;
        }
    }
    /// Captures input, holds the dry signal until the loop fills, and ramps
    /// the Q14 wet mix over 200 frames on each toggle.
    pub(crate) fn process_sample(&mut self, input: [Q14; 2]) -> [Q14; 2] {
        if !self.config.enabled && self.wet_frames == 0 {
            return input;
        }
        self.buffer[self.read] = input;
        if self.restart_pending {
            let reverse = self.reverse_sample();
            self.wet_frames -= 1;
            let result = self.mix_wet(input, reverse);
            self.advance_read();
            self.advance_phase();
            if self.wet_frames == 0 {
                self.phase = 0;
                self.captured = 0;
                self.restart_pending = false;
            }
            return result;
        }
        if self.config.enabled {
            self.captured = (self.captured + 1).min(HISTORY_FRAMES);
            if self.captured < self.loop_len {
                self.advance_read();
                return input;
            }
            let reverse = self.reverse_sample();
            if self.wet_frames < FADE_FRAMES {
                self.wet_frames += 1;
            }
            let result = self.mix_wet(input, reverse);
            self.advance_read();
            self.advance_phase();
            result
        } else {
            let reverse = self.reverse_sample();
            self.wet_frames -= 1;
            let result = self.mix_wet(input, reverse);
            self.advance_read();
            self.advance_phase();
            if self.wet_frames == 0 {
                self.phase = 0;
                self.captured = 0;
            }
            result
        }
    }
}
