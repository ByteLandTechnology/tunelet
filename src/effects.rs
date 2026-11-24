//! Optional stages for dynamics, saturation, reverse loops and reverberation.
//!
//! Punch and Warm operate on delayed mono PCM and a 100-frame lookahead
//! sample before resynthesis. The processor does not call their sample
//! methods while they are disabled. Alien and reverb accept stereo Q14
//! frames after resynthesis. They have their own bypass behavior. Alien
//! continues to record during bypass. Reverb keeps its current delay state
//! during bypass.
//!
//! The configuration types validate the public controls. The prepared stage
//! types assume that these checks were successful.

pub(crate) mod alien;
pub(crate) mod punch;
pub(crate) mod reverb;
pub(crate) mod warm;

use crate::numeric::fixed::Q28;

/// The length of the peak window for Punch and Warm, in frames.
pub(crate) const WINDOW_FRAMES: usize = 100;
/// The ring capacity of `SlidingPeak`, which is a power of two above
/// `WINDOW_FRAMES`.
const DEQUE_CAPACITY: usize = 128;
/// The bit mask that wraps a deque index into `0..DEQUE_CAPACITY`.
const DEQUE_MASK: usize = DEQUE_CAPACITY - 1;

/// A monotonic maximum deque that tracks the peak of the last 100 frames.
///
/// All storage is inline. Each sample update runs in amortized constant time.
#[derive(Debug)]
pub(crate) struct SlidingPeak {
    /// The candidate peaks, from the largest at `head` to the newest.
    values: [Q28; DEQUE_CAPACITY],
    /// The frame stamp of each candidate in `values`.
    frames: [u32; DEQUE_CAPACITY],
    /// The index of the oldest and largest candidate.
    head: usize,
    /// The number of valid candidates in the ring.
    len: usize,
    /// The modular count of pushed frames.
    frame: u32,
}

impl SlidingPeak {
    /// Returns an empty peak window with zeroed inline storage.
    pub(crate) fn new() -> Self {
        Self {
            values: [Q28::ZERO; DEQUE_CAPACITY],
            frames: [0; DEQUE_CAPACITY],
            head: 0,
            len: 0,
            frame: 0,
        }
    }

    /// Clears all candidates and resets the frame counter.
    pub(crate) fn reset(&mut self) {
        self.values.fill(Q28::ZERO);
        self.frames.fill(0);
        self.head = 0;
        self.len = 0;
        self.frame = 0;
    }

    /// Records `value` for the current frame and returns the 100-frame peak.
    pub(crate) fn push(&mut self, value: Q28) -> Q28 {
        let frame = self.frame;
        while self.len > 0 && frame.wrapping_sub(self.frames[self.head]) >= WINDOW_FRAMES as u32 {
            self.head = (self.head + 1) & DEQUE_MASK;
            self.len -= 1;
        }
        while self.len > 0 {
            let back = (self.head + self.len - 1) & DEQUE_MASK;
            if self.values[back] <= value {
                self.len -= 1;
            } else {
                break;
            }
        }
        let slot = (self.head + self.len) & DEQUE_MASK;
        self.values[slot] = value;
        self.frames[slot] = frame;
        self.len += 1;
        self.frame = frame.wrapping_add(1);
        self.values[self.head]
    }
}
