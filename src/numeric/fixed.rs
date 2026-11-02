//! Signed fixed-point values that use `i32` storage.
//!
//! `Fixed<F>` represents `bits / 2^F`. Addition, subtraction and products
//! wrap at 32 bits. Multiplication and division state the fractional format
//! of the result. Callers must supply valid formats and divisors that are
//! not zero.

use std::ops::{Add, Neg, Shl, Shr, Sub};

/// A signed `i32` word that represents `bits / 2^FRAC`.
///
/// This type only marks the format. It does not find overflow. Arithmetic
/// wraps at 32 bits before each shift. This is also true for a product
/// whose rescaled result fits. Callers must use fractional bit counts and
/// shift differences that are less than 32. Conversions and division have
/// the conditions that their documentation gives.
///
/// Comparison compares the signed words in the same format. `Default` is
/// zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub(crate) struct Fixed<const FRAC: u32>(i32);

/// The format with 14 fractional bits, for audio, ratios, coefficients and
/// frame positions.
///
/// PCM audio comes in as `i32::from(sample) >> 1`, thus 1.0 is digital full
/// scale. The type does not identify which of these physical units it holds.
pub(crate) type Q14 = Fixed<14>;
/// The format with 12 fractional bits, for MIDI gains, gain envelopes and
/// the drive of Warm.
pub(crate) type Q12 = Fixed<12>;
/// The format with 8 fractional bits, for drive, normalization and wet/dry
/// controls.
pub(crate) type Q8 = Fixed<8>;
/// The format with 6 fractional bits, for periods in samples.
pub(crate) type Q6 = Fixed<6>;

impl<const F: u32> Fixed<F> {
    /// The exact value zero in this format.
    pub(crate) const ZERO: Self = Self(0);
    /// The exact value 1.0, for valid formats with less than 31 fractional
    /// bits.
    pub(crate) const ONE: Self = Self(1 << F);

    /// Makes a value from a signed word that has the correct scale. The
    /// function does not convert or validate the word.
    #[inline]
    pub(crate) const fn from_bits(bits: i32) -> Self {
        Self(bits)
    }
    /// Returns the stored scaled word. The binary point does not change.
    #[inline]
    pub(crate) const fn to_bits(self) -> i32 {
        self.0
    }
    /// Makes a value from an `f64`: the function scales the value, then casts
    /// it to `i32`.
    ///
    /// The cast follows the rules of Rust `as`:
    /// - A finite value in the range truncates toward zero.
    /// - A value out of the range saturates.
    /// - NaN becomes zero.
    #[inline]
    pub(crate) fn from_f64(value: f64) -> Self {
        Self((value * f64::from(1u32 << F)) as i32)
    }

    /// Returns `(self * rhs) >> (F + B - R)`, the product in format `R`.
    #[inline]
    pub(crate) fn mul<const B: u32, const R: u32>(self, rhs: Fixed<B>) -> Fixed<R> {
        const { assert!(F + B >= R, "product cannot gain fraction bits") };
        Fixed(self.0.wrapping_mul(rhs.0).wrapping_shr(F + B - R))
    }

    /// Returns `(self << (R + B - F)) / rhs`, the quotient in format `R`.
    ///
    /// The caller must supply a divisor that is not zero. The wrapping
    /// division panics if the divisor is zero.
    #[inline]
    pub(crate) fn div<const B: u32, const R: u32>(self, rhs: Fixed<B>) -> Fixed<R> {
        const { assert!(R + B >= F, "quotient cannot lose fraction bits") };
        Fixed(self.0.wrapping_shl(R + B - F).wrapping_div(rhs.0))
    }

    /// Returns the same bits, shifted into format `R`.
    ///
    /// If `R` adds fractional bits, the shift wraps. If `R` removes
    /// fractional bits, the shift truncates toward -inf.
    #[inline]
    pub(crate) fn rescale<const R: u32>(self) -> Fixed<R> {
        if R >= F {
            Fixed(self.0.wrapping_shl(R - F))
        } else {
            Fixed(self.0.wrapping_shr(F - R))
        }
    }

    /// Removes fractional bits to get format `R`, with a signed half-unit
    /// adjustment.
    ///
    /// If the bits are nonnegative, the function adds half a unit. If the
    /// bits are negative, the function subtracts half a unit. Then an
    /// arithmetic shift to the right removes the bits. The shift moves
    /// negative values down. For example, Q1 bits -2 become Q0 bits -2,
    /// but the input represents -1.
    #[inline]
    pub(crate) fn round<const R: u32>(self) -> Fixed<R> {
        const { assert!(F > R, "rounding must drop fraction bits") };
        let half = 1 << (F - R - 1);
        let bits = if self.0 >= 0 {
            self.0.wrapping_add(half)
        } else {
            self.0.wrapping_sub(half)
        };
        Fixed(bits >> (F - R))
    }

    /// Returns the wrapped absolute value. `i32::MIN` stays negative.
    ///
    /// The result does not saturate. This agrees with the word arithmetic of
    /// the signal path.
    #[inline]
    pub(crate) fn abs(self) -> Self {
        Self(self.0.wrapping_abs())
    }
}

impl<const F: u32> Add for Fixed<F> {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self(self.0.wrapping_add(rhs.0))
    }
}
impl<const F: u32> Sub for Fixed<F> {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self(self.0.wrapping_sub(rhs.0))
    }
}
impl<const F: u32> Neg for Fixed<F> {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self(self.0.wrapping_neg())
    }
}
/// Raw bit shifts that keep the format, for approximate comparisons and
/// for scale changes.
impl<const F: u32> Shr<u32> for Fixed<F> {
    type Output = Self;
    #[inline]
    fn shr(self, bits: u32) -> Self {
        Self(self.0.wrapping_shr(bits))
    }
}
impl<const F: u32> Shl<u32> for Fixed<F> {
    type Output = Self;
    #[inline]
    fn shl(self, bits: u32) -> Self {
        Self(self.0.wrapping_shl(bits))
    }
}
