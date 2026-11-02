//! The search for the lag with the minimum value, with four independent
//! comparison chains.

/// The number of candidate lags around the tracked period.
pub(super) const TRACKING_LAGS: usize = 40;
/// The number of independent comparison chains. They decrease the dependency
/// depth of the lag search.
const LANES: usize = 4;

/// Returns the index of the first minimum of exactly 40 candidates.
///
/// The candidates are examined in index order. Four independent minima
/// decrease the dependencies between comparisons. Then a final reduction
/// selects the lowest index if two values are equal. `value_at` must return
/// the same value for an index, independent of the order of comparisons.
#[inline]
pub(super) fn tracking_minimum(value_at: impl Fn(usize) -> i32) -> usize {
    let first = value_at(0);
    let mut minima = [first; LANES];
    let mut indices = [0; LANES];
    for lane in 1..LANES {
        let value = value_at(lane);
        if minima[lane] > value {
            minima[lane] = value;
            indices[lane] = lane;
        }
    }
    for group in 1..TRACKING_LAGS / LANES {
        for lane in 0..LANES {
            let index = group * LANES + lane;
            let value = value_at(index);
            if minima[lane] > value {
                minima[lane] = value;
                indices[lane] = index;
            }
        }
    }
    let mut minimum = minima[0];
    let mut best = indices[0];
    for lane in 1..LANES {
        let value = minima[lane];
        let index = indices[lane];
        if minimum > value || (minimum == value && index < best) {
            minimum = value;
            best = index;
        }
    }
    best
}
