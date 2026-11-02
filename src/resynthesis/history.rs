//! Audio histories on the heap and three-point parabolic interpolation.

use crate::numeric::fixed::Q14;
use crate::numeric::{ring, HISTORY};

/// Allocates exactly HISTORY copies of `value` on the heap and keeps that
/// length in the boxed-array type.
///
/// The conversion from slice to array cannot fail, because the allocation
/// gives the slice the necessary length.
pub(super) fn prepare_history<T: Copy>(value: T) -> Box<[T; HISTORY]> {
    // Allocate directly on the heap. The fixed history length is then part
    // of the type for index operations, and no large temporary array goes on
    // the stack.
    vec![value; HISTORY]
        .into_boxed_slice()
        .try_into()
        .unwrap_or_else(|_| unreachable!("prepared voice history length"))
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

/// Reads three adjacent taps.
///
/// If possible, the function reads a contiguous slice. If not, it wraps the
/// center and the two adjacent positions. The two paths keep the tap order.
fn interpolation_taps<T: Copy>(buffer: &[T; HISTORY], center: i32) -> [T; 3] {
    if center > 0 && center < (HISTORY - 1) as i32 {
        let center = center as usize;
        let taps = &buffer[center - 1..center + 2];
        [taps[0], taps[1], taps[2]]
    } else {
        let [left, middle, right] = interpolation_indices(center);
        [buffer[left], buffer[middle], buffer[right]]
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
pub(super) fn interpolation(buffer: &[Q14; HISTORY], position: Q14) -> (Q14, i32) {
    let bits = position.to_bits();
    let center = if position > Q14::ZERO {
        bits.wrapping_add(8192) >> 14
    } else {
        -(8192_i32.wrapping_sub(bits) >> 14)
    };
    let fraction = Q14::from_bits(bits.wrapping_sub(center.wrapping_shl(14)));
    let [left, middle, right] = interpolation_taps(buffer, center);
    let curvature = ((left + right) >> 1) - middle;
    let slope = (right - left) >> 1;
    (
        middle + (curvature.mul::<14, 14>(fraction) + slope).mul(fraction),
        center,
    )
}
