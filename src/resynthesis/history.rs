//! Audio histories on the heap and three-point parabolic interpolation.

use crate::numeric::fixed::Q14;
use crate::numeric::{ring, HISTORY};

/// Allocates `HISTORY` copies of `value` on the heap and keeps that length in
/// the boxed-array type.
///
/// The conversion from slice to array cannot fail, because the allocation
/// gives the slice the necessary length.
fn prepare_array<T: Copy>(value: T) -> Box<[T; HISTORY]> {
    // Allocate directly on the heap. The fixed history length is then part
    // of the type for index operations, and no large temporary array goes on
    // the stack.
    vec![value; HISTORY]
        .into_boxed_slice()
        .try_into()
        .unwrap_or_else(|_| unreachable!("prepared voice history length"))
}

/// A fixed-length ring on the heap with one generation tag for each slot.
pub(crate) struct History<T: Copy> {
    /// The sample, period or ratio values.
    values: Box<[T; HISTORY]>,
    /// The generation tag of the last write to each slot.
    tags: Box<[u32; HISTORY]>,
    /// The value that a read returns when the slot has an older tag.
    default: T,
}

impl<T: Copy> History<T> {
    /// Allocates the ring and its zeroed tag array on the heap.
    pub(crate) fn new(default: T) -> Self {
        Self {
            values: prepare_array(default),
            tags: prepare_array(0),
            default,
        }
    }

    /// Reads one slot, or returns the reset default if its tag is older.
    #[inline]
    pub(crate) fn get(&self, index: usize, generation: u32) -> T {
        if self.tags[index] == generation {
            self.values[index]
        } else {
            self.default
        }
    }

    /// Writes a value and records the active generation tag in that slot.
    #[inline]
    pub(crate) fn set(&mut self, index: usize, value: T, generation: u32) {
        self.values[index] = value;
        self.tags[index] = generation;
    }

    /// Clears all generation tags to zero when the generation counter wraps.
    pub(crate) fn clear_tags(&mut self) {
        self.tags.fill(0);
    }
}

/// Returns the previous, center and next positions in the ring buffer.
///
/// The center can be negative or more than one full history length.
#[inline]
fn interpolation_indices(center: i32) -> [usize; 3] {
    let middle = ring(center);
    let left = if middle == 0 { HISTORY - 1 } else { middle - 1 };
    let right = if middle + 1 == HISTORY { 0 } else { middle + 1 };
    [left, middle, right]
}

/// Reads three adjacent taps and replaces entries from older generations.
///
/// If possible, the function reads a contiguous slice. If not, it wraps the
/// center and the two adjacent positions. The two paths keep the tap order.
fn interpolation_taps<T: Copy>(history: &History<T>, center: i32, generation: u32) -> [T; 3] {
    if center > 0 && center < (HISTORY - 1) as i32 {
        let center = center as usize;
        let taps = &history.values[center - 1..center + 2];
        let tags = &history.tags[center - 1..center + 2];
        let default = history.default;
        [
            if tags[0] == generation {
                taps[0]
            } else {
                default
            },
            if tags[1] == generation {
                taps[1]
            } else {
                default
            },
            if tags[2] == generation {
                taps[2]
            } else {
                default
            },
        ]
    } else {
        let [left, middle, right] = interpolation_indices(center);
        [
            history.get(left, generation),
            history.get(middle, generation),
            history.get(right, generation),
        ]
    }
}

/// Returns the value of a three-point parabola at a Q14 frame position.
///
/// The result has two parts: the Q14 sample and the integer center frame
/// without wrap. The formant model uses this center for alignment. To find
/// the center, the function adds a half frame to a positive position. For
/// other positions, it subtracts a half frame.
///
/// Then it truncates the result toward zero. The fraction that is left is
/// relative to that center. The taps wrap through the history, but the
/// returned center does not. All intermediate products and sums keep the
/// 32-bit wrap order of the engine.
pub(super) fn interpolation(history: &History<Q14>, position: Q14, generation: u32) -> (Q14, i32) {
    let bits = position.to_bits();
    let center = if position > Q14::ZERO {
        bits.wrapping_add(8192) >> 14
    } else {
        -(8192_i32.wrapping_sub(bits) >> 14)
    };
    let fraction = Q14::from_bits(bits.wrapping_sub(center.wrapping_shl(14)));
    let [left, middle, right] = interpolation_taps(history, center, generation);
    let curvature = ((left + right) >> 1) - middle;
    let slope = (right - left) >> 1;
    (
        middle + (curvature.mul::<14, 14>(fraction) + slope).mul(fraction),
        center,
    )
}
