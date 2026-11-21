//! Audio histories on the heap and three-point parabolic interpolation.

use crate::numeric::fixed::Q14;
use crate::numeric::{clamp_i32, ring};

/// Allocates `len` copies of `value` on the heap as a boxed slice.
pub(super) fn prepare_history<T: Copy>(value: T, len: usize) -> Box<[T]> {
    vec![value; len].into_boxed_slice()
}

/// Returns the previous, center and next positions in a ring buffer of length
/// `len`.
///
/// The center can be negative or more than one full history length.
#[inline]
fn interpolation_indices(center: i32, len: usize) -> [usize; 3] {
    let middle = ring(center, len);
    let left = if middle == 0 { len - 1 } else { middle - 1 };
    let right = if middle + 1 == len { 0 } else { middle + 1 };
    [left, middle, right]
}

/// Reads three adjacent taps.
///
/// If possible, the function reads a contiguous slice. If not, it wraps the
/// center and the two adjacent positions. The two paths keep the tap order.
fn interpolation_taps<T: Copy>(buffer: &[T], center: i32) -> [T; 3] {
    let len = buffer.len();
    if center > 0 && center < (len - 1) as i32 {
        let center = center as usize;
        let taps = &buffer[center - 1..center + 2];
        [taps[0], taps[1], taps[2]]
    } else {
        let [left, middle, right] = interpolation_indices(center, len);
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
/// returned center does not. Coordinate offsets wrap at 32 bits. Sample
/// products use `i64` arithmetic and clamp to `i32`.
pub(super) fn interpolation(buffer: &[Q14], position: Q14) -> (Q14, i32) {
    let bits = position.to_bits();
    let center = if position > Q14::ZERO {
        bits.wrapping_add(8192) >> 14
    } else {
        -(8192_i32.wrapping_sub(bits) >> 14)
    };
    let fraction = Q14::from_bits(bits.wrapping_sub(center.wrapping_shl(14)));
    let [left, middle, right] = interpolation_taps(buffer, center);
    let left_i64 = i64::from(left.to_bits());
    let right_i64 = i64::from(right.to_bits());
    let curvature = Q14::from_bits(clamp_i32((left_i64 + right_i64) >> 1)) - middle;
    let slope = Q14::from_bits(clamp_i32((right_i64 - left_i64) >> 1));
    (
        middle + (curvature.mul::<14, 14>(fraction) + slope).mul(fraction),
        center,
    )
}
