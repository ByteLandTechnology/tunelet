//! The Q14 first-order, second-order and cascade filters.
//!
//! Single-precision calculations and explicit casts set the normalized Q14
//! coefficients. Each design keeps at least 15 bits of `i64` headroom for all
//! `i32` sample words.

use crate::numeric::fixed::Q14;
use crate::numeric::round_q14;

/// The value of π, rounded to single precision, as the designs use it.
const PI: f32 = std::f32::consts::PI;

/// The strict upper limit on the sum of absolute Q14 tap weights, `2^17`.
///
/// Below this limit, products with `i32` samples stay within `2^48 - 1` and
/// keep 15 bits of `i64` headroom.
const HEADROOM_LIMIT: i64 = 1 << 17;

/// The normalized Q14 coefficients for a first-order filter with 15 bits of
/// `i64` headroom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OnePoleDesign {
    /// The normalized Q14 feed-forward coefficients for delays 0 and 1.
    b: [Q14; 2],
    /// The normalized Q14 denominator coefficient for delay 1.
    a: Q14,
}

impl OnePoleDesign {
    /// Calculates a normalized first-order low-pass or high-pass design with a
    /// bilinear transform.
    ///
    /// `rate` and `frequency` are in hertz. `rate` must be finite and
    /// positive. `frequency` must be finite, positive and below half the
    /// sample rate.
    fn one_pole(rate: f32, frequency: f32, high: bool) -> Self {
        assert!(
            rate.is_finite()
                && rate > 0.0
                && frequency.is_finite()
                && frequency > 0.0
                && frequency < rate * 0.5,
            "one-pole cutoff must be positive and below Nyquist"
        );
        let w = (PI * frequency) / rate;
        let sin = (w as f64).sin() as f32;
        let cos = (w as f64).cos() as f32;
        let raw_b = if high { [cos, -cos] } else { [sin, sin] };
        let a0 = sin + cos;
        assert!(
            a0.is_finite() && a0 > 0.0,
            "one-pole denominator must be finite and positive"
        );
        let b = raw_b.map(|x| Q14::from_f64((x / a0) as f64));
        let a = Q14::from_f64(((sin - cos) / a0) as f64);
        let tap_sum = i64::from(b[0].to_bits()).abs()
            + i64::from(b[1].to_bits()).abs()
            + i64::from(a.to_bits()).abs();
        assert!(
            tap_sum < HEADROOM_LIMIT,
            "one-pole Q14 coefficients must keep 15 bits of i64 headroom"
        );
        Self { b, a }
    }
    /// Returns a normalized first-order low-pass design.
    ///
    /// `rate` and `frequency` are in hertz. `frequency` must be positive and
    /// below half the sample rate.
    pub(crate) fn low_pass(rate: f32, frequency: f32) -> Self {
        Self::one_pole(rate, frequency, false)
    }
    /// Returns a normalized first-order high-pass design.
    ///
    /// `rate` and `frequency` are in hertz. `frequency` must be positive and
    /// below half the sample rate.
    pub(crate) fn high_pass(rate: f32, frequency: f32) -> Self {
        Self::one_pole(rate, frequency, true)
    }
}

/// The normalized Q14 coefficients for a second-order filter with 15 bits of
/// `i64` headroom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TwoPoleDesign {
    /// The normalized Q14 feed-forward coefficients for delays 0, 1 and 2.
    b: [Q14; 3],
    /// The normalized Q14 denominator coefficients for delays 1 and 2.
    a: [Q14; 2],
}

impl TwoPoleDesign {
    /// Returns the sine, the cosine and the alpha term for a second-order
    /// section.
    ///
    /// `q` must be finite and positive. The intermediate `f32` casts between
    /// `f64` operations are intentional and set the final Q14 coefficients.
    fn two_pole_terms(rate: f32, frequency: f32, q: f32) -> (f32, f32, f32) {
        let w = (((PI as f64 + PI as f64) * frequency as f64) / rate as f64) as f32;
        let sin = (w as f64).sin() as f32;
        let cos = (w as f64).cos() as f32;
        let alpha = (sin as f64 / (q as f64 + q as f64)) as f32;
        (sin, cos, alpha)
    }
    /// Calculates a normalized second-order low-pass or high-pass design.
    ///
    /// `rate` and `frequency` are in hertz, and `q` has no unit. `rate` and
    /// `q` must be finite and positive, with `frequency` below half the sample
    /// rate.
    fn two_pole(rate: f32, frequency: f32, q: f32, high: bool) -> Self {
        assert!(
            rate.is_finite()
                && rate > 0.0
                && frequency.is_finite()
                && frequency > 0.0
                && frequency < rate * 0.5
                && q.is_finite()
                && q > 0.0,
            "two-pole cutoff and Q must be finite and in bounds"
        );
        let (_, cos, alpha) = Self::two_pole_terms(rate, frequency, q);
        let raw_b = if high {
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
        let a0 = alpha + 1.0;
        assert!(
            alpha.is_finite() && a0.is_finite() && a0 > 0.0,
            "two-pole denominator must be finite and positive"
        );
        let b = raw_b.map(|x| Q14::from_f64((x / a0) as f64));
        let a = [cos * -2.0, 1.0 - alpha].map(|x| Q14::from_f64((x / a0) as f64));
        let tap_sum = i64::from(b[0].to_bits()).abs()
            + i64::from(b[1].to_bits()).abs()
            + i64::from(b[2].to_bits()).abs()
            + i64::from(a[0].to_bits()).abs()
            + i64::from(a[1].to_bits()).abs();
        assert!(
            tap_sum < HEADROOM_LIMIT,
            "two-pole Q14 coefficients must keep 15 bits of i64 headroom"
        );
        Self { b, a }
    }
    /// Returns a normalized second-order low-pass design.
    ///
    /// `rate` and `frequency` are in hertz, and `q` has no unit.
    pub(crate) fn low_pass(rate: f32, frequency: f32, q: f32) -> Self {
        Self::two_pole(rate, frequency, q, false)
    }
    /// Returns a normalized second-order high-pass design.
    ///
    /// `rate` and `frequency` are in hertz, and `q` has no unit.
    pub(crate) fn high_pass(rate: f32, frequency: f32, q: f32) -> Self {
        Self::two_pole(rate, frequency, q, true)
    }
}

impl From<OnePoleDesign> for TwoPoleDesign {
    fn from(design: OnePoleDesign) -> Self {
        Self {
            b: [design.b[0], design.b[1], Q14::ZERO],
            a: [design.a, Q14::ZERO],
        }
    }
}

/// The constructors for normalized first-order and second-order filter
/// designs.
pub(crate) struct Design;

impl Design {
    /// Returns a normalized first-order low-pass design. The cutoff must be
    /// positive and below half the sample rate.
    pub(crate) fn low_pass_one_pole(rate: f32, frequency: f32) -> OnePoleDesign {
        OnePoleDesign::low_pass(rate, frequency)
    }
    /// Returns a normalized first-order high-pass design. The cutoff must be
    /// positive and below half the sample rate.
    pub(crate) fn high_pass_one_pole(rate: f32, frequency: f32) -> OnePoleDesign {
        OnePoleDesign::high_pass(rate, frequency)
    }
    /// Returns a normalized second-order low-pass design. The rate and the
    /// cutoff are in hertz. `q` has no unit.
    pub(crate) fn low_pass(rate: f32, frequency: f32, q: f32) -> TwoPoleDesign {
        TwoPoleDesign::low_pass(rate, frequency, q)
    }
    /// Returns a normalized second-order high-pass design. The rate and the
    /// cutoff are in hertz. `q` has no unit.
    pub(crate) fn high_pass(rate: f32, frequency: f32, q: f32) -> TwoPoleDesign {
        TwoPoleDesign::high_pass(rate, frequency, q)
    }
}

/// A direct-form Q14 first-order filter that keeps one input sample and one
/// output sample.
///
/// The three Q28 products accumulate in `i64` before one signed half-unit
/// shift and an `i32` clamp.
#[derive(Clone, Debug)]
pub(crate) struct OnePole {
    /// The normalized Q14 feed-forward coefficients for the current input and
    /// the previous input.
    b: [Q14; 2],
    /// The normalized Q14 denominator coefficient for the previous output. The
    /// recurrence subtracts this term.
    a: Q14,
    /// The previous Q14 input.
    x: Q14,
    /// The previous Q14 output.
    y: Q14,
}

impl OnePole {
    /// Creates a filter with a clear history, a feed-forward coefficient of
    /// 1.0 and zero for the other coefficients.
    pub(crate) fn new() -> Self {
        Self {
            b: [Q14::ONE, Q14::ZERO],
            a: Q14::ZERO,
            x: Q14::ZERO,
            y: Q14::ZERO,
        }
    }
    /// Clears the input and output history. The coefficients do not change.
    pub(crate) fn reset(&mut self) {
        self.x = Q14::ZERO;
        self.y = Q14::ZERO;
    }
    /// Loads the normalized Q14 coefficients from `design`.
    ///
    /// The input and output history does not change.
    pub(crate) fn set_design(&mut self, design: OnePoleDesign) {
        self.b = design.b;
        self.a = design.a;
    }
    /// Filters one sample with the three-tap recurrence and updates the
    /// one-sample history.
    ///
    /// The three Q28 products accumulate in `i64` until one final signed
    /// half-unit shift and an `i32` clamp. Do not round each product
    /// separately.
    #[inline]
    pub(crate) fn process_sample(&mut self, input: Q14) -> Q14 {
        let sum = i64::from(self.b[0].to_bits()) * i64::from(input.to_bits())
            + i64::from(self.b[1].to_bits()) * i64::from(self.x.to_bits())
            - i64::from(self.a.to_bits()) * i64::from(self.y.to_bits());
        let output = Q14::from_bits(round_q14(sum));
        self.x = input;
        self.y = output;
        output
    }
}

/// A direct-form Q14 filter that keeps two input samples and two output
/// samples.
///
/// The five Q28 products accumulate in `i64` before one signed half-unit
/// shift and an `i32` clamp.
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
    /// The filter still uses Q14 arithmetic and its signed quantization rule.
    pub(crate) fn new() -> Self {
        Self {
            b: [Q14::ONE, Q14::default(), Q14::default()],
            a: [Q14::default(); 2],
            x: [Q14::default(); 2],
            y: [Q14::default(); 2],
        }
    }
    /// Clears the input and output history. The coefficients do not change.
    pub(crate) fn reset(&mut self) {
        self.x = [Q14::default(); 2];
        self.y = [Q14::default(); 2];
    }
    /// Loads the normalized Q14 coefficients from `design`.
    ///
    /// The input and output history does not change.
    pub(crate) fn set_design(&mut self, design: impl Into<TwoPoleDesign>) {
        let design = design.into();
        self.b = design.b;
        self.a = design.a;
    }
    /// Filters one sample with the direct-form recurrence and advances the
    /// two-sample history.
    ///
    /// The five Q28 products accumulate in `i64` until one final signed
    /// half-unit shift and an `i32` clamp. Do not round each product
    /// separately.
    #[inline]
    pub(crate) fn process_sample(&mut self, input: Q14) -> Q14 {
        let sum = i64::from(self.b[0].to_bits()) * i64::from(input.to_bits())
            + i64::from(self.b[1].to_bits()) * i64::from(self.x[0].to_bits())
            + i64::from(self.b[2].to_bits()) * i64::from(self.x[1].to_bits())
            - i64::from(self.a[0].to_bits()) * i64::from(self.y[0].to_bits())
            - i64::from(self.a[1].to_bits()) * i64::from(self.y[1].to_bits());
        let output = Q14::from_bits(round_q14(sum));
        self.x[1] = self.x[0];
        self.x[0] = input;
        self.y[1] = self.y[0];
        self.y[0] = output;
        output
    }
}

/// Applies one bypass step with Q14 quantization.
///
/// If the input is negative and above `i32::MIN`, the output is one Q14 unit
/// lower. Inputs that are `i32::MIN`, zero or positive do not change.
#[inline]
fn bypass_step(sample: Q14) -> Q14 {
    let bits = sample.to_bits();
    Q14::from_bits(bits.saturating_add(bits >> 31))
}

/// Two cascaded Q14 second-order sections with shared coefficients, shared
/// intermediate state and a dedicated bypass mode.
#[derive(Clone, Debug)]
pub(crate) struct BypassCascade {
    /// `true` if the cascade applies two bypass steps instead of `b` and `a`.
    bypassed: bool,
    /// The normalized Q14 feed-forward coefficients of each active section.
    b: [Q14; 3],
    /// The normalized Q14 denominator coefficients of each active section.
    /// Each recurrence subtracts these terms.
    a: [Q14; 2],
    /// The previous Q14 inputs of the first section, newest first.
    x: [Q14; 2],
    /// The shared Q14 intermediate samples between the two sections, newest
    /// first.
    mid: [Q14; 2],
    /// The previous Q14 outputs of the second section, newest first.
    y: [Q14; 2],
}

impl BypassCascade {
    /// Creates a bypassed cascade with clear histories.
    pub(crate) fn new() -> Self {
        Self {
            bypassed: true,
            b: [Q14::ONE, Q14::ZERO, Q14::ZERO],
            a: [Q14::ZERO; 2],
            x: [Q14::ZERO; 2],
            mid: [Q14::ZERO; 2],
            y: [Q14::ZERO; 2],
        }
    }
    /// Sets both sections to bypass mode. The histories do not change.
    ///
    /// Each section still applies signed Q14 quantization. Thus, if a negative
    /// sample does not saturate at `i32::MIN`, each section decreases it by
    /// one Q14 unit. Samples that are zero or positive do not change.
    pub(crate) fn bypass(&mut self) {
        self.bypassed = true;
    }
    /// Clears the input, intermediate and output histories. The mode and the
    /// coefficients do not change.
    pub(crate) fn reset(&mut self) {
        self.x = [Q14::ZERO; 2];
        self.mid = [Q14::ZERO; 2];
        self.y = [Q14::ZERO; 2];
    }
    /// Loads the normalized Q14 coefficients from `design` and enables both
    /// sections.
    ///
    /// The histories do not change.
    pub(crate) fn set_design(&mut self, design: TwoPoleDesign) {
        self.bypassed = false;
        self.b = design.b;
        self.a = design.a;
    }
    /// Processes one sample through both sections and advances the shared
    /// histories.
    #[inline]
    pub(crate) fn process_sample(&mut self, input: Q14) -> Q14 {
        let (mid, output) = if self.bypassed {
            let mid = bypass_step(input);
            let output = bypass_step(mid);
            (mid, output)
        } else {
            let b0 = i64::from(self.b[0].to_bits());
            let b1 = i64::from(self.b[1].to_bits());
            let b2 = i64::from(self.b[2].to_bits());
            let a0 = i64::from(self.a[0].to_bits());
            let a1 = i64::from(self.a[1].to_bits());
            let sum0 = b0 * i64::from(input.to_bits())
                + b1 * i64::from(self.x[0].to_bits())
                + b2 * i64::from(self.x[1].to_bits())
                - a0 * i64::from(self.mid[0].to_bits())
                - a1 * i64::from(self.mid[1].to_bits());
            let mid = Q14::from_bits(round_q14(sum0));
            let sum1 = b0 * i64::from(mid.to_bits())
                + b1 * i64::from(self.mid[0].to_bits())
                + b2 * i64::from(self.mid[1].to_bits())
                - a0 * i64::from(self.y[0].to_bits())
                - a1 * i64::from(self.y[1].to_bits());
            let output = Q14::from_bits(round_q14(sum1));
            (mid, output)
        };
        self.x[1] = self.x[0];
        self.x[0] = input;
        self.mid[1] = self.mid[0];
        self.mid[0] = mid;
        self.y[1] = self.y[0];
        self.y[0] = output;
        output
    }
}
