//! Signed fixed-point values that use `i32` storage.
//!
//! `Fixed<F>` represents `bits / 2^F`. Addition, subtraction, negation and
//! absolute value saturate in `i32`. Multiplication, division and upward
//! rescale use `i64` arithmetic and clamp to `i32::MIN..=i32::MAX`. Callers
//! must supply valid formats and divisors that are not zero.

use crate::numeric::clamp_i32;
use std::ops::{Add, Neg, Shl, Shr, Sub};

/// A signed `i32` word that represents `bits / 2^FRAC`.
///
/// This type marks the binary format. Addition, subtraction, negation and
/// absolute value saturate in `i32`. Products, quotients and upward rescale
/// use `i64` arithmetic and clamp to `i32::MIN..=i32::MAX`. Callers must
/// use fractional bit counts and shift differences that are less than 32.
/// Conversions and division have the conditions that their documentation
/// gives.
///
/// Comparison compares the signed words in the same format. `Default` is
/// zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub(crate) struct Fixed<const FRAC: u32>(i32);

/// The format with 30 fractional bits, for per-sample envelope coefficients.
pub(crate) type Q30 = Fixed<30>;
/// The format with 28 fractional bits, for envelopes, gains and output
/// ceilings.
pub(crate) type Q28 = Fixed<28>;
/// The format with 24 fractional bits, for saturation drive and envelope
/// ratios.
pub(crate) type Q24 = Fixed<24>;
/// The format with 22 fractional bits, for normalization gains and saturator
/// inputs.
pub(crate) type Q22 = Fixed<22>;
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

    /// Returns `(self * rhs) >> (F + B - R)` in format `R`, clamped to `i32`.
    #[inline]
    pub(crate) fn mul<const B: u32, const R: u32>(self, rhs: Fixed<B>) -> Fixed<R> {
        const { assert!(F + B >= R, "product cannot gain fraction bits") };
        const { assert!(F + B - R < 64, "product shift must be less than 64") };
        Fixed(clamp_i32(
            (i64::from(self.0) * i64::from(rhs.0)) >> (F + B - R),
        ))
    }

    /// Returns `(self << (R + B - F)) / rhs` in format `R`, clamped to `i32`.
    ///
    /// The caller must supply a divisor that is not zero. Division panics if
    /// the divisor is zero.
    #[inline]
    pub(crate) fn div<const B: u32, const R: u32>(self, rhs: Fixed<B>) -> Fixed<R> {
        const { assert!(R + B >= F, "quotient cannot lose fraction bits") };
        const { assert!(R + B - F < 32, "quotient shift must be less than 32") };
        Fixed(clamp_i32(
            (i64::from(self.0) << (R + B - F)) / i64::from(rhs.0),
        ))
    }

    /// Returns `self << (R - F)` in format `R`, clamped to `i32`.
    ///
    /// `R` must be at least `F`. `R - F` must be less than 32. The shift
    /// uses `i64` arithmetic.
    #[inline]
    pub(crate) fn rescale<const R: u32>(self) -> Fixed<R> {
        const { assert!(R >= F, "rescale cannot lose fraction bits") };
        const { assert!(R - F < 32, "rescale shift must be less than 32") };
        Fixed(clamp_i32(i64::from(self.0) << (R - F)))
    }

    /// Returns the saturated absolute value. `i32::MIN` becomes `i32::MAX`.
    #[inline]
    pub(crate) fn abs(self) -> Self {
        Self(self.0.saturating_abs())
    }
}

impl<const F: u32> Add for Fixed<F> {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }
}
impl<const F: u32> Sub for Fixed<F> {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}
impl<const F: u32> Neg for Fixed<F> {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self(self.0.saturating_neg())
    }
}
/// Raw bit shifts that keep the format and wrap at 32 bits, for approximate
/// comparisons and for scale changes.
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
