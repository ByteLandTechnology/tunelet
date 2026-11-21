//! Reverse-loop playback with a continuous input history and boundary fades.

use crate::error::check_count;
use crate::numeric::clamp_i32;
use crate::numeric::fixed::{Fixed, Q14};
use crate::Error;

/// The number of history frames that the loop reads from.
const HISTORY_FRAMES: usize = 192_000;
/// The number of frames for the boundary fade at each loop boundary.
const FADE_FRAMES: usize = 200;

/// Reverse playback of recent stereo audio in repeated loops with fades.
///
/// Alien comes after resynthesis. Its input history and loop phase advance
/// also when Alien is disabled. If you enable Alien, it starts a new capture
/// epoch and ramps the wet mix over 200 frames. Thus, audio from bypass does
/// not play. If you disable Alien, it ramps the wet mix to zero over 200
/// frames. A change of the loop length clamps the phase into the new loop and
/// crossfades from the previous cursor over 200 frames. The boundary fade
/// length is `min(200, loop_frames / 2)`, which is zero at `loop_frames` of 1.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub struct AlienConfig {
    /// Selects reversed playback instead of the direct input. The default is
    /// `false`. A change ramps the wet mix over 200 frames. If you enable
    /// Alien, it starts a new capture epoch.
    pub enabled: bool,
    /// The loop length in frames, 1..=96000. The default is 95,999.
    /// This value is a frame count at the two rates. Thus, its duration in
    /// seconds changes with the sample rate. At 1, the boundary fade is zero.
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

/// Blends two Q14 samples in `i64` with `weight` for `right`, clamped to `i32`.
#[inline]
fn blend_q14(left: Q14, right: Q14, weight: Q14) -> Q14 {
    let inv = i64::from((Q14::ONE - weight).to_bits());
    let weight = i64::from(weight.to_bits());
    Q14::from_bits(clamp_i32(
        (i64::from(left.to_bits()) * inv + i64::from(right.to_bits()) * weight) >> 14,
    ))
}

/// Returns the boundary fade length for `loop_frames`: zero at 1, and at most
/// 200 frames.
#[inline]
fn loop_fade_frames(loop_frames: usize) -> usize {
    if loop_frames <= 1 {
        0
    } else {
        FADE_FRAMES.min(loop_frames / 2)
    }
}

/// One reverse cursor and the count of frames recorded in its capture epoch.
#[derive(Clone, Copy, Debug)]
struct ReverseCursor {
    /// The current frame in the loop, always less than `loop_len`.
    phase: usize,
    /// The validated loop length, not more than half of the history capacity.
    loop_len: usize,
    /// The number of frames recorded in this epoch before the current frame.
    captured: usize,
}

impl ReverseCursor {
    /// Creates a cursor at phase zero for `loop_len` and `captured` frames.
    fn new(loop_len: usize, captured: usize) -> Self {
        Self {
            phase: 0,
            loop_len,
            captured,
        }
    }

    /// Reads the reversed stereo sample for this cursor, or zero before its
    /// capture epoch.
    fn sample(self, buffer: &[[Q14; 2]], write: usize) -> [Q14; 2] {
        let offset = self.phase * 2;
        if offset > self.captured {
            return [Q14::ZERO; 2];
        }
        let read = write + HISTORY_FRAMES - offset;
        let read = if read >= HISTORY_FRAMES {
            read - HISTORY_FRAMES
        } else {
            read
        };
        let mut result = buffer[read];
        let fade_frames = loop_fade_frames(self.loop_len);
        let distance = self
            .phase
            .min(self.loop_len - self.phase)
            .min(self.captured - offset);
        if distance < fade_frames {
            let gain: Q14 = Fixed::<0>::from_bits(distance as i32)
                .div(Fixed::<0>::from_bits(fade_frames as i32));
            result = result.map(|sample| sample.mul::<14, 14>(gain));
        }
        result
    }

    /// Advances the loop phase and the epoch frame count by one frame.
    fn step(&mut self) {
        self.phase += 1;
        if self.phase == self.loop_len {
            self.phase = 0;
        }
        if self.captured < HISTORY_FRAMES {
            self.captured += 1;
        }
    }
}

/// A stereo ring recorder with paired reverse cursors and wet-mix ramps.
#[derive(Debug)]
pub(crate) struct Alien {
    /// The validated controls, which include the selection of the wet output.
    config: AlienConfig,
    /// 192,000 stereo Q14 frames, which are allocated one time at
    /// construction.
    buffer: Vec<[Q14; 2]>,
    /// The current recording position in the ring buffer.
    read: usize,
    /// The active reverse cursor, which telemetry also reports.
    active: ReverseCursor,
    /// The previous reverse cursor for crossfades across loop changes.
    shadow: ReverseCursor,
    /// The crossfade progress toward `active`, in `0..=FADE_FRAMES`.
    cursor_fade_frames: usize,
    /// The wet mix ramp position, in `0..=FADE_FRAMES`.
    wet_frames: usize,
}
impl Alien {
    /// Allocates a silent stereo ring from validated loop settings.
    pub(crate) fn new(config: AlienConfig) -> Self {
        let cursor = ReverseCursor::new(config.loop_frames as usize, 0);
        Self {
            config,
            buffer: vec![[Q14::default(); 2]; HISTORY_FRAMES],
            read: 0,
            active: cursor,
            shadow: cursor,
            cursor_fade_frames: FADE_FRAMES,
            wet_frames: if config.enabled { FADE_FRAMES } else { 0 },
        }
    }
    /// Applies a validated configuration. If Alien becomes enabled, starts a
    /// new capture epoch. If the loop length changes, crossfades the cursors.
    pub(crate) fn set_config(&mut self, config: AlienConfig) {
        let was_enabled = self.config.enabled;
        let new_loop_len = config.loop_frames as usize;
        self.config = config;
        if !was_enabled && config.enabled {
            if self.wet_frames > 0 && self.cursor_fade_frames >= FADE_FRAMES / 2 {
                self.shadow = self.active;
                self.cursor_fade_frames = 0;
            }
            self.active = ReverseCursor::new(new_loop_len, 0);
        } else if new_loop_len != self.active.loop_len {
            if (was_enabled || self.wet_frames > 0) && self.cursor_fade_frames >= FADE_FRAMES / 2 {
                self.shadow = self.active;
                self.cursor_fade_frames = 0;
            }
            self.active.loop_len = new_loop_len;
            self.active.phase = self.active.phase.min(new_loop_len - 1);
        }
    }
    /// Returns the loop length and the current frame in the loop.
    ///
    /// The frame is always less than the length. The stage state does not
    /// change.
    pub(crate) fn loop_state(&self) -> (usize, usize) {
        (self.active.loop_len, self.active.phase)
    }
    /// Clears the recorded audio and resets the cursors and ramps, but keeps
    /// the configured loop.
    pub(crate) fn reset(&mut self) {
        self.buffer.fill([Q14::default(); 2]);
        self.read = 0;
        let cursor = ReverseCursor::new(self.active.loop_len, 0);
        self.active = cursor;
        self.shadow = cursor;
        self.cursor_fade_frames = FADE_FRAMES;
        self.wet_frames = if self.config.enabled { FADE_FRAMES } else { 0 };
    }
    /// Records a frame, blends the reverse cursors and Q14 wet mix, then
    /// advances the cursors.
    ///
    /// The recording cursor moves forward by one and each reverse offset
    /// increases by two. Thus, the playback moves backward by one frame. The
    /// validated loop limit keeps that subtraction in a single ring wrap.
    pub(crate) fn process_sample(&mut self, input: [Q14; 2]) -> [Q14; 2] {
        self.buffer[self.read] = input;
        if self.config.enabled {
            if self.wet_frames < FADE_FRAMES {
                self.wet_frames += 1;
            }
        } else if self.wet_frames > 0 {
            self.wet_frames -= 1;
        }
        let result = if self.wet_frames == 0 {
            self.cursor_fade_frames = FADE_FRAMES;
            input
        } else {
            let mut wet = self.active.sample(&self.buffer, self.read);
            if self.cursor_fade_frames < FADE_FRAMES {
                self.cursor_fade_frames += 1;
                if self.cursor_fade_frames < FADE_FRAMES {
                    let old = self.shadow.sample(&self.buffer, self.read);
                    let fade: Q14 = Fixed::<0>::from_bits(self.cursor_fade_frames as i32)
                        .div(Fixed::<0>::from_bits(FADE_FRAMES as i32));
                    wet = [
                        blend_q14(old[0], wet[0], fade),
                        blend_q14(old[1], wet[1], fade),
                    ];
                }
                self.shadow.step();
            }
            if self.wet_frames == FADE_FRAMES {
                wet
            } else {
                let mix: Q14 = Fixed::<0>::from_bits(self.wet_frames as i32)
                    .div(Fixed::<0>::from_bits(FADE_FRAMES as i32));
                [
                    blend_q14(input[0], wet[0], mix),
                    blend_q14(input[1], wet[1], mix),
                ]
            }
        };
        self.read += 1;
        if self.read == HISTORY_FRAMES {
            self.read = 0;
        }
        self.active.step();
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
