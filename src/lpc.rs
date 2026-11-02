//! Integer linear prediction and the conversion to and from vocal-tract
//! areas.
//!
//! Predictor coefficients use wrapping arithmetic. All area values use one
//! binary scale that the code selects at runtime. They do not use a fixed
//! fractional format.

use crate::numeric::mul14;

/// Runs the Levinson–Durbin recurrence for a predictor of order
/// `coefficients.len()`.
///
/// All autocorrelations use one binary scale that the caller selects.
/// Predictor and reflection (`parcor`) coefficients use Q14. `correlation`
/// must hold at least `order + 1` values. `parcor` and `scratch` must each
/// hold at least `order` values.
///
/// Returns `false` for each of these conditions:
/// - The order is zero.
/// - The initial energy is not positive.
/// - A denominator of the recursion is zero.
/// - The magnitude of a reflection coefficient is 1.0 or more.
///
/// If the function fails, the coefficients, `parcor` and `scratch` keep the
/// values that it wrote before the failure. This internal function does
/// not restore them. In all of the recurrence, products wrap before the shift.
pub(crate) fn durbin(
    correlation: &[i32],
    coefficients: &mut [i32],
    parcor: &mut [i32],
    scratch: &mut [i32],
) -> bool {
    let order = coefficients.len();
    if order == 0 {
        return false;
    }
    if correlation[0] <= 0_i32 {
        return false;
    }
    let mut alpha = correlation[0];
    let one = 16384;
    for i in 0..order {
        let mut sum = 0_i32;
        for j in 0..i {
            sum = sum.wrapping_add(coefficients[j].wrapping_mul(correlation[i - j]));
        }
        sum = sum.wrapping_shr(14);
        if alpha == 0_i32 {
            return false;
        }
        let k = correlation[i + 1]
            .wrapping_sub(sum)
            .wrapping_shl(14)
            .wrapping_div(alpha);
        parcor[i] = k;
        if k.wrapping_abs() >= one {
            return false;
        }
        scratch[i] = k;
        for j in 0..i {
            scratch[j] = coefficients[j]
                .wrapping_shl(14)
                .wrapping_sub(k.wrapping_mul(coefficients[i - j - 1]))
                .wrapping_shr(14);
        }
        coefficients[..=i].copy_from_slice(&scratch[..=i]);
        alpha = mul14(alpha, one.wrapping_sub(mul14(k, k)));
    }
    true
}

/// Converts Q14 reflection coefficients into relative vocal-tract areas.
///
/// `areas` must have at least `parcor.len()` entries. Returns `Some` with
/// the number of binary renormalization shifts for all areas.
/// [`areas_to_parcor`] needs this value. If a later area is too large for
/// the range, the function shifts the earlier entries down. The function
/// replaces each zero area with one, so that each section has a value that
/// is not zero.
///
/// Returns `None` if a denominator is zero or if the renormalization gets to
/// 32 shifts. If the conversion fails, the areas that the function wrote
/// before the failure keep their changed values.
pub(crate) fn parcor_to_areas(parcor: &[i32], areas: &mut [i32]) -> Option<u32> {
    let one = 16384;
    let mut area = one;
    let mut scale = 0;
    for (i, &k) in parcor.iter().enumerate() {
        let mut sum = k.wrapping_add(one);
        let numerator = mul14(one.wrapping_sub(k), area);
        while numerator.wrapping_abs() > sum.wrapping_abs() {
            for a in &mut areas[..i] {
                *a = a.wrapping_shr(1);
                if *a == 0_i32 {
                    *a = 1;
                }
            }
            sum = sum.wrapping_shl(1);
            scale += 1;
            if sum == 0_i32 || scale >= 32 {
                return None;
            }
        }
        if sum == 0_i32 {
            return None;
        }
        area = numerator.wrapping_shl(14).wrapping_div(sum);
        if area == 0_i32 {
            area = 1;
        }
        areas[i] = area;
    }
    Some(scale)
}
/// Converts tract areas with one common scale back into Q14 reflection
/// coefficients.
///
/// `parcor` must hold at least `areas.len()` entries. `scale` is the value
/// that [`parcor_to_areas`] returns (less than 32). The function uses it for
/// the reference area. If a denominator is zero, the function returns
/// `false` and keeps the output that it wrote before. A denominator is the
/// sum of an area and the area before it. For the first area, the reference
/// area is the area before it. All other arithmetic keeps its signed wrap.
pub(crate) fn areas_to_parcor(areas: &[i32], parcor: &mut [i32], scale: u32) -> bool {
    let mut previous = 16384_i32.wrapping_shr(scale);
    for (i, &area) in areas.iter().enumerate() {
        let denominator = area.wrapping_add(previous);
        if denominator == 0_i32 {
            return false;
        }
        parcor[i] = previous
            .wrapping_sub(area)
            .wrapping_shl(14)
            .wrapping_div(denominator);
        previous = area;
    }
    true
}
/// Converts reflection coefficients into the coefficients of a
/// prediction-error filter.
///
/// `coefficients` and `scratch` must each hold at least `parcor.len()`
/// entries. `scale` gives the number of fractional bits of the reflection
/// coefficients and must be less than 32. Formant synthesis uses 14. The
/// recurrence keeps more fractional bits internally. At the end, it shifts
/// the full output one time. This function does not do a stability check.
/// The caller must supply reflection coefficients with a magnitude less
/// than 1.0.
pub(crate) fn parcor_to_pef(
    parcor: &[i32],
    coefficients: &mut [i32],
    scratch: &mut [i32],
    scale: u32,
) {
    for (i, &k) in parcor.iter().enumerate() {
        scratch[i] = k.wrapping_shl(scale);
        for j in 0..i {
            scratch[j] = coefficients[j]
                .wrapping_sub(k.wrapping_mul(coefficients[i - j - 1].wrapping_shr(scale)));
        }
        coefficients[..=i].copy_from_slice(&scratch[..=i]);
    }
    for coefficient in &mut coefficients[..parcor.len()] {
        *coefficient = coefficient.wrapping_shr(scale);
    }
}
