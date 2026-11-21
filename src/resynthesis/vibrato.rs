//! The Q24 period filter that separates the mean pitch contour from vibrato.
//!
//! The pitch tracker updates the period once every five audio frames. At 44.1
//! kHz and 48 kHz, a Q14 biquad with a 3 Hz cutoff loses its numerator bits.
//! Thus, this filter uses Q24 coefficients and Q24 state words.

use crate::numeric::fixed::{Q14, Q6};
use crate::SampleRate;

/// The number of fractional bits in Q24.
const Q24_SHIFT: u32 = 24;
/// The left shift from a Q6 period word to a Q24 period word.
const Q6_TO_Q24_SHIFT: u32 = 18;
/// The geometric center of the 4..=8 Hz vibrato band, in hertz.
const BAND_CENTER_HZ: f64 = 5.656_854_249_492_381;
/// The -3 dB width of the vibrato band-pass section, in hertz.
const BAND_WIDTH_HZ: f64 = 8.2;
/// The one-pole cutoff after the vibrato notch, in hertz.
///
/// The combined secular -3 dB cutoff is below 3 Hz, and the group delay is
/// below 80 ms at both sample rates.
const SECULAR_LP_HZ: f64 = 4.4;
/// The lowest Q14 modulation ratio (0.25).
const MIN_MOD_RATIO_BITS: i128 = 4_096;
/// The highest Q14 modulation ratio (4.0).
const MAX_MOD_RATIO_BITS: i128 = 65_536;

/// Scales a finite control in 0..=1 to a Q24 word.
pub(super) fn quantize_preserve(value: f32) -> i64 {
    PeriodFilter::quantize_q24(f64::from(value))
}

/// A Q24 contour filter that runs at the five-frame period cadence.
///
/// A second-order band-pass section extracts the 4..=8 Hz vibrato residual. A
/// first-order low-pass section filters the difference to get the secular
/// period below 3 Hz. Error feedback keeps the sub-unit remainders, so a step
/// converges to the exact input period.
pub(super) struct PeriodFilter {
    /// The Q24 feed-forward gain of the band-pass section.
    bp_b0: i64,
    /// The first Q24 feedback coefficient of the band-pass section.
    bp_a1: i64,
    /// The second Q24 feedback coefficient of the band-pass section.
    bp_a2: i64,
    /// The Q24 step coefficient of the secular one-pole low-pass section.
    lp_alpha: i64,
    /// If `true`, the state holds a valid initial period.
    seeded: bool,
    /// The two previous Q24 input periods for the band-pass section.
    bp_x: [i64; 2],
    /// The two previous Q24 outputs of the band-pass section.
    bp_y: [i64; 2],
    /// The sub-unit remainder of the band-pass sum.
    bp_rem: i64,
    /// The current Q24 secular period estimate.
    lp_y: i64,
    /// The sub-unit remainder of the low-pass product.
    lp_rem: i64,
}

impl PeriodFilter {
    /// Scales a finite value to Q24 with a half-unit adjustment.
    fn quantize_q24(value: f64) -> i64 {
        let scaled = value * f64::from(1_u32 << Q24_SHIFT);
        if scaled >= 0.0 {
            (scaled + 0.5) as i64
        } else {
            (scaled - 0.5) as i64
        }
    }

    /// Creates a cleared filter with coefficients for `rate`.
    pub(super) fn new(rate: SampleRate) -> Self {
        let mut filter = Self {
            bp_b0: 0,
            bp_a1: 0,
            bp_a2: 0,
            lp_alpha: 0,
            seeded: false,
            bp_x: [0; 2],
            bp_y: [0; 2],
            bp_rem: 0,
            lp_y: 0,
            lp_rem: 0,
        };
        filter.configure(rate);
        filter
    }

    /// Calculates the Q24 coefficients for the five-frame update rate.
    ///
    /// The filter state does not change.
    pub(super) fn configure(&mut self, rate: SampleRate) {
        let update_hz = f64::from(rate.as_u32()) / 5.0;
        let q = BAND_CENTER_HZ / BAND_WIDTH_HZ;
        let w0 = 2.0 * std::f64::consts::PI * BAND_CENTER_HZ / update_hz;
        let sin = w0.sin();
        let cos = w0.cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        self.bp_b0 = Self::quantize_q24(alpha / a0);
        self.bp_a1 = Self::quantize_q24((-2.0 * cos) / a0);
        self.bp_a2 = Self::quantize_q24((1.0 - alpha) / a0);
        let w_lp = 2.0 * std::f64::consts::PI * SECULAR_LP_HZ / update_hz;
        self.lp_alpha = Self::quantize_q24(1.0 - (-w_lp).exp());
    }

    /// Clears the filter histories and marks the filter as not seeded.
    ///
    /// The coefficients do not change.
    pub(super) fn reset(&mut self) {
        self.seeded = false;
        self.bp_x = [0; 2];
        self.bp_y = [0; 2];
        self.bp_rem = 0;
        self.lp_y = 0;
        self.lp_rem = 0;
    }

    /// Sets the secular state to `period` and clears the vibrato residual.
    pub(super) fn seed(&mut self, period: Q6) {
        let p_q24 = i64::from(period.to_bits()) << Q6_TO_Q24_SHIFT;
        self.bp_x = [p_q24; 2];
        self.bp_y = [0; 2];
        self.bp_rem = 0;
        self.lp_y = p_q24;
        self.lp_rem = 0;
        self.seeded = true;
    }

    /// Splits `period` into a secular Q6 period and a Q14 modulation ratio.
    ///
    /// If the filter is not seeded yet, this call seeds the state to `period`
    /// and returns unity modulation. All calculations use integer arithmetic.
    pub(super) fn split(&mut self, period: Q6, preserve_q24: i64) -> (Q6, Q14) {
        if period <= Q6::ZERO {
            return (period, Q14::ONE);
        }
        if !self.seeded {
            self.seed(period);
            return (period, Q14::ONE);
        }
        let x = i64::from(period.to_bits()) << Q6_TO_Q24_SHIFT;
        let acc = i128::from(self.bp_b0) * i128::from(x.wrapping_sub(self.bp_x[1]))
            - i128::from(self.bp_a1) * i128::from(self.bp_y[0])
            - i128::from(self.bp_a2) * i128::from(self.bp_y[1])
            + i128::from(self.bp_rem);
        let vib = (acc >> Q24_SHIFT) as i64;
        self.bp_rem = (acc - (i128::from(vib) << Q24_SHIFT)) as i64;
        self.bp_x[1] = self.bp_x[0];
        self.bp_x[0] = x;
        self.bp_y[1] = self.bp_y[0];
        self.bp_y[0] = vib;

        let notched = x.wrapping_sub(vib);
        let lp_acc = i128::from(self.lp_alpha) * i128::from(notched.wrapping_sub(self.lp_y))
            + i128::from(self.lp_rem);
        let lp_step = (lp_acc >> Q24_SHIFT) as i64;
        self.lp_rem = (lp_acc - (i128::from(lp_step) << Q24_SHIFT)) as i64;
        self.lp_y = self.lp_y.wrapping_add(lp_step);

        let half_q6 = 1_i64 << (Q6_TO_Q24_SHIFT - 1);
        let secular_bits = if self.lp_y >= 0 {
            self.lp_y.saturating_add(half_q6) >> Q6_TO_Q24_SHIFT
        } else {
            self.lp_y.saturating_sub(half_q6) >> Q6_TO_Q24_SHIFT
        };
        let secular_q6 = Q6::from_bits(secular_bits.clamp(1, i64::from(i32::MAX)) as i32);

        let half_q24 = 1_i128 << (Q24_SHIFT - 1);
        let vib_prod = i128::from(vib) * i128::from(preserve_q24);
        let preserved = if vib_prod >= 0 {
            ((vib_prod + half_q24) >> Q24_SHIFT) as i64
        } else {
            ((vib_prod - half_q24) >> Q24_SHIFT) as i64
        };
        let min_denom = (x >> 2).max(1);
        let max_denom = x.saturating_mul(4);
        let denom = i128::from(
            self.lp_y
                .saturating_add(preserved)
                .clamp(min_denom, max_denom),
        );
        let ratio_bits = (((i128::from(x) << 14) + (denom >> 1)) / denom)
            .clamp(MIN_MOD_RATIO_BITS, MAX_MOD_RATIO_BITS) as i32;
        (secular_q6, Q14::from_bits(ratio_bits))
    }
}
