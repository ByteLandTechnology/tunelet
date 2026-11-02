//! The fixed-point operations, the lookup tables and the state that each
//! processor keeps for these operations.
//!
//! Integer products wrap before the shift. Each conversion between integer
//! and floating-point values is explicit, because intermediate rounding
//! changes the audio.

pub(crate) mod fixed;
pub(crate) mod rng;
mod tables;
use rng::RandomStream;
use tables::{SINC, TAN};

/// Multiplies two signed words with 32-bit wrap, then does an arithmetic
/// shift to the right.
///
/// The caller must supply a shift that is less than 32. Do not widen the
/// product. If the intermediate product overflows, a wider product gives a
/// different result.
#[inline]
pub(crate) fn mul_shift(a: i32, b: i32, shift: u32) -> i32 {
    a.wrapping_mul(b) >> shift
}

/// Removes 14 fractional bits with a signed half-unit adjustment.
///
/// If the word is nonnegative, the function adds half a unit. If the word is
/// negative, the function subtracts half a unit. Then an arithmetic shift
/// removes the bits. The shift moves negative values down. This is not
/// round-half-to-even, and it is not truncation toward zero. The half-unit
/// adjustment also wraps.
#[inline]
pub(crate) fn round_q14(value: i32) -> i32 {
    if value >= 0 {
        value.wrapping_add(8192) >> 14
    } else {
        value.wrapping_sub(8192) >> 14
    }
}

/// Converts an amplitude in decibels to a linear multiplier with
/// `10^(db/20)`.
///
/// The validation of the public stages sets the limits of the values that
/// callers supply.
pub(crate) fn db_to_gain(db: f64) -> f64 {
    10.0_f64.powf(db / 20.0)
}

/// Maps a normalized control value through the exponential curve of Punch.
///
/// The function requires these conditions:
/// - `max > min`.
/// - `(start-min)/(max-min) > 1`.
/// - `(start-min)/(max-min)` is not 2. This value gives a division by zero.
/// - The position is finite.
///
/// The function keeps the intermediate f32 rounding before each f64
/// exponential calculation. Callers supply the fixed, valid curve.
pub(crate) fn coefficient_to_exp_value(min: f32, max: f32, start: f32, position: f32) -> f32 {
    let f = max - min;
    let g = start - min;
    let b = (((g / f) as f64 - 1.0).ln() * 2.0) as f32;
    let a = (g as f64 / ((b as f64).exp() - 1.0)) as f32;
    (a as f64 * (((b * position) as f64).exp() - 1.0) + min as f64) as f32
}

/// Returns the value of a three-point parabola at a Q14 fractional offset
/// from the center sample.
///
/// The table words, the sums and the intermediate products wrap.
#[inline]
fn quadratic(y0: i32, y1: i32, y2: i32, fraction: i32) -> i32 {
    let a = (y0.wrapping_add(y2) >> 1).wrapping_sub(y1);
    let b = y2.wrapping_sub(y0) >> 1;
    mul_shift(mul_shift(a, fraction, 14).wrapping_add(b), fraction, 14).wrapping_add(y1)
}

/// Interpolates the table of sinc coefficients at a Q14 phase position.
///
/// The function clamps the nearest table index to 1..=254, so that the two
/// adjacent entries exist. The function keeps the fractional offset from
/// that center index. The result is a Q14 weight for LPC correlations.
pub(crate) fn fast_sinc(input: i32) -> i32 {
    let scaled = (input >> 1).wrapping_mul(256);
    let index = (scaled.wrapping_add(8192) >> 14).clamp(1, 254);
    let fraction = scaled.wrapping_sub(index.wrapping_shl(14));
    let i = index as usize;
    quadratic(SINC[i - 1], SINC[i], SINC[i + 1], fraction)
}

/// An arctangent approximation that keeps the last index into the tangent
/// table.
///
/// The kept index makes lookups of near values faster. Each processor has
/// its own instance.
#[derive(Debug)]
pub(crate) struct AtanQ14 {
    /// The upper search index into the tangent table. The initial value is 1.
    /// The entries at this index and the index below it contain the input.
    index: usize,
}
impl Default for AtanQ14 {
    fn default() -> Self {
        Self { index: 1 }
    }
}
impl AtanQ14 {
    /// Returns the interpolated arctangent of the input as Q14 radians.
    ///
    /// The function shifts the input word into Q14 and finds the two adjacent
    /// tangent entries. The caller selects the shift for the input format.
    /// For example, Punch uses Q8 and a shift of 6. If the magnitude is
    /// larger than the last entry, the function uses a fixed terminal angle.
    /// The signed arithmetic wraps.
    pub(crate) fn process(&mut self, input: i32, shift: u32) -> i32 {
        let value = input.wrapping_shl(shift);
        let negative = value < 0;
        let value = value.wrapping_abs();
        if TAN[self.index] > value {
            while TAN[self.index - 1] > value {
                self.index -= 1;
            }
        } else {
            while self.index <= 298 && TAN[self.index] < value {
                self.index += 1;
            }
        }
        let index = self.index;
        let angle = if TAN[index] >= value {
            let fraction = value.wrapping_sub(TAN[index - 1]).wrapping_mul(64)
                / TAN[index].wrapping_sub(TAN[index - 1]);
            ((index as i32 - 1) * 16384) / 300 + fraction.wrapping_shl(8) / 300
        } else {
            0x3fc9
        };
        let result = mul_shift(angle, 25722, 14);
        if negative {
            result.wrapping_neg()
        } else {
            result
        }
    }
}

/// Returns the coefficient for an exponential decay to one half in
/// `milliseconds`.
///
/// `window` is the number of audio frames between two envelope updates.
/// Callers must supply a positive time, rate and window. This prevents a
/// division by zero.
pub(crate) fn envelope_decay(milliseconds: f64, rate: f64, window: usize) -> f64 {
    let time = milliseconds * 0.001 * rate / window as f64;
    0.5_f64.powf(1.0 / time)
}

/// The lookup cache and the pseudo-random state for all stages of one
/// processor chain.
///
/// An audio reset keeps this state.
#[derive(Debug)]
pub(crate) struct NumericContext {
    /// The lookup cursor for Punch. An audio reset keeps its position.
    pub atan_q14: AtanQ14,
    /// The random stream for choir and aspiration. The order in which stages
    /// read values sets the random values that later stages get.
    pub random: RandomStream,
}
impl NumericContext {
    /// Creates the context with a new random stream from `seed` and the
    /// initial arctangent lookup cursor.
    pub(crate) fn new(seed: u32) -> Self {
        Self {
            atan_q14: AtanQ14::default(),
            random: RandomStream::new(seed),
        }
    }
}
/// The capacity in frames of the history for pitch and resynthesis. The
/// value is the same at the two supported rates.
pub(crate) const HISTORY: usize = 11_568;
/// The value 1.0 as a plain Q14 word, for algorithms that set the binary
/// scale at runtime.
pub(crate) const ONE: i32 = 16_384;

/// Multiplies two Q14 words with 32-bit wrap and arithmetic truncation.
///
/// This function does not add a half-unit adjustment. `round_q14` adds one.
#[inline]
pub(crate) fn mul14(left: i32, right: i32) -> i32 {
    left.wrapping_mul(right) >> 14
}

/// Wraps a signed frame index into 0..HISTORY with the Euclidean remainder.
///
/// Negative reader positions also wrap correctly.
#[inline]
pub(crate) fn ring(index: i32) -> usize {
    index.rem_euclid(HISTORY as i32) as usize
}
