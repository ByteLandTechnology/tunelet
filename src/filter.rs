//! Q14 biquad filters with coefficients that are calculated in single
//! precision.
//!
//! The floating-point casts and the order of operations set the quantized
//! coefficients.

use crate::numeric::fixed::{Fixed, Q14};

/// The biquad coefficients in single precision, before normalization by
/// `a[0]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Design {
    /// The feed-forward coefficients for the input delays 0, 1 and 2, before
    /// normalization.
    b: [f32; 3],
    /// The denominator coefficients, before normalization. `a[0]` normalizes
    /// the two arrays.
    a: [f32; 3],
}

/// The value of π, rounded to single precision, as the designs use it.
const PI: f32 = std::f32::consts::PI;

impl Design {
    /// Returns a first-order low-pass or high-pass section from a bilinear
    /// transform.
    ///
    /// The rate and the frequency are in hertz. Callers must supply finite,
    /// positive values, with a frequency below Nyquist. This internal
    /// function does not validate the values.
    fn one_pole(rate: f32, frequency: f32, high: bool) -> Self {
        let w = (PI * frequency) / rate;
        let sin = (w as f64).sin() as f32;
        let cos = (w as f64).cos() as f32;
        let b = if high {
            [cos, -cos, 0.0]
        } else {
            [sin, sin, 0.0]
        };
        Self {
            b,
            a: [sin + cos, sin - cos, 0.0],
        }
    }
    /// Returns a first-order low-pass design. The cutoff must be valid and
    /// below half the sample rate.
    pub(crate) fn low_pass_one_pole(rate: f32, frequency: f32) -> Self {
        Self::one_pole(rate, frequency, false)
    }
    /// Returns a first-order high-pass design. The cutoff must be valid and
    /// below half the sample rate.
    pub(crate) fn high_pass_one_pole(rate: f32, frequency: f32) -> Self {
        Self::one_pole(rate, frequency, true)
    }
    /// Returns the sine, the cosine and the damping alpha for a second-order
    /// section.
    ///
    /// The quality factor must be positive. The f32 rounding between the f64
    /// calculations is intentional. It sets the final Q14 coefficients.
    fn two_pole_terms(rate: f32, frequency: f32, q: f32) -> (f32, f32, f32) {
        let w = (((PI as f64 + PI as f64) * frequency as f64) / rate as f64) as f32;
        let sin = (w as f64).sin() as f32;
        let cos = (w as f64).cos() as f32;
        let alpha = (sin as f64 / (q as f64 + q as f64)) as f32;
        (sin, cos, alpha)
    }
    /// Returns a second-order low-pass or high-pass design from a valid rate,
    /// cutoff and Q.
    fn two_pole(rate: f32, frequency: f32, q: f32, high: bool) -> Self {
        let (_, cos, alpha) = Self::two_pole_terms(rate, frequency, q);
        let b = if high {
            [
                ((cos as f64 + 1.0) * 0.5) as f32,
                -(cos + 1.0),
                ((cos as f64 + 1.0) * 0.5) as f32,
            ]
        } else {
            [
                ((1.0 - cos as f64) * 0.5) as f32,
                1.0 - cos,
                ((1.0 - cos as f64) * 0.5) as f32,
            ]
        };
        Self {
            b,
            a: [alpha + 1.0, cos * -2.0, 1.0 - alpha],
        }
    }
    /// Returns a second-order low-pass design. The rate and the cutoff are in
    /// hertz. `q` has no unit.
    pub(crate) fn low_pass(rate: f32, frequency: f32, q: f32) -> Self {
        Self::two_pole(rate, frequency, q, false)
    }
    /// Returns a second-order high-pass design. The rate and the cutoff are in
    /// hertz. `q` has no unit.
    pub(crate) fn high_pass(rate: f32, frequency: f32, q: f32) -> Self {
        Self::two_pole(rate, frequency, q, true)
    }
}

/// A direct-form Q14 filter that keeps two input samples and two output
/// samples.
///
/// Each product and each sum wraps at 32 bits before the signed rounding.
#[derive(Clone, Debug)]
pub(crate) struct Biquad {
    /// The normalized Q14 feed-forward coefficients for the current input and
    /// the previous inputs.
    b: [Q14; 3],
    /// The normalized Q14 denominator coefficients for the previous outputs.
    /// The recurrence subtracts these terms.
    a: [Q14; 2],
    /// The previous Q14 inputs, newest first.
    x: [Q14; 2],
    /// The previous Q14 outputs, newest first.
    y: [Q14; 2],
}
impl Biquad {
    /// Creates a filter with a clear history, a feed-forward coefficient of
    /// 1.0 and feedback coefficients of zero.
    ///
    /// The filter still uses Q14 arithmetic and its signed rounding rule.
    pub(crate) fn new() -> Self {
        Self {
            b: [Q14::ONE, Q14::default(), Q14::default()],
            a: [Q14::default(); 2],
            x: [Q14::default(); 2],
            y: [Q14::default(); 2],
        }
    }
    /// Sets a feed-forward coefficient of 1.0 and feedback coefficients of
    /// zero. The history does not change.
    ///
    /// The filter still uses fixed-point signed rounding. Thus, each negative
    /// input comes out one Q14 unit lower, if the product does not wrap.
    /// Inputs that are zero or positive do not change.
    pub(crate) fn bypass(&mut self) {
        self.b = [Q14::ONE, Q14::default(), Q14::default()];
        self.a = [Q14::default(); 2];
    }
    /// Clears the input and output history. The coefficients do not change.
    pub(crate) fn reset(&mut self) {
        self.x = [Q14::default(); 2];
        self.y = [Q14::default(); 2];
    }
    /// Normalizes `design` by `a[0]` in single precision, then quantizes the
    /// coefficients to Q14.
    ///
    /// `a[0]` must not be zero. The input and output history does not change.
    pub(crate) fn set_design(&mut self, design: Design) {
        let Design { b, a } = design;
        self.b = b.map(|x| Q14::from_f64((x / a[0]) as f64));
        self.a = [a[1], a[2]].map(|x| Q14::from_f64((x / a[0]) as f64));
    }
    /// Filters one sample with the direct-form recurrence and advances the
    /// two-sample history.
    ///
    /// The products stay in Q28 format until one final signed half-unit
    /// rounding. Do not round each product. Do not widen the accumulator.
    #[inline]
    pub(crate) fn process_sample(&mut self, input: Q14) -> Q14 {
        let sum: Fixed<28> =
            self.b[0].mul(input) + self.b[1].mul(self.x[0]) + self.b[2].mul(self.x[1])
                - self.a[0].mul(self.y[0])
                - self.a[1].mul(self.y[1]);
        let output = sum.round();
        self.x[1] = self.x[0];
        self.x[0] = input;
        self.y[1] = self.y[0];
        self.y[0] = output;
        output
    }
}
