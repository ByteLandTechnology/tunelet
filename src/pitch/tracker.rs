//! A two-stage pitch estimator with circular sample histories.
//!
//! Acquisition applies a low-pass filter to the signal and examines one
//! sample in eight. The minima of the normalized cycle difference must repeat
//! with a constant spacing. Then acquisition starts a full-rate tracking
//! window. Tracking updates the energy and cross terms at each frame. It
//! refines the minimum every five frames. A parabola gives the Q6 sub-sample
//! resolution of the period.
//!
//! The 40-lag window moves to keep the minimum near its center. If the
//! minimum is at a boundary or the curvature is not usable, acquisition
//! starts again. The last usable period stays available as a fallback for
//! resynthesis. The public frequency bounds apply only to feedback reports.
//! They do not change the size of these windows.

use super::search::{tracking_minimum, TRACKING_LAGS};
use crate::filter::{Biquad, Design};
use crate::numeric::fixed::{Q14, Q6};
use crate::numeric::{mul14, ring, HISTORY, ONE};
use crate::{PitchConfig, SampleRate};

/// The result of one full-rate input sample.
///
/// The flags show transitions in this call, not persistent modes. A new
/// search window does not always give a refined period immediately. The
/// previous usable period stays after a restart, so resynthesis can fill
/// unvoiced gaps.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PitchStep {
    /// The current Q6 period. It is zero while acquisition has no refined
    /// tracking result.
    pub period: Q6,
    /// The last refined Q6 period. It stays when the current tracking starts
    /// again.
    pub previous_period: Q6,
    /// `true` if this sample gave a new refined period.
    pub tracking_update: bool,
    /// `true` if acquisition found a candidate and started a full-rate
    /// tracking window.
    pub acquired: bool,
    /// `true` if tracking rejected its current window and went back to
    /// acquisition.
    pub restarted: bool,
}

/// The number of lags that acquisition compares.
const ACQUISITION_LAGS: usize = 137;

/// The two-stage pitch tracker.
///
/// The audio history and the filtered samples are Q14 words. The acquisition
/// energies and cross terms are plain wrapping words.
pub(crate) struct PitchTracker {
    /// Two low-pass sections before the 8:1 decimation for acquisition.
    ///
    /// Full-rate tracking uses the raw audio.
    pre: [Biquad; 2],
    /// Full-rate Q14 sample words in a fixed ring on the heap.
    raw: Box<[i32; HISTORY]>,
    /// The current full-rate history frame. It also sets the acquisition
    /// cadence of one frame in eight.
    write: usize,
    /// `true` if acquisition is active instead of full-rate local tracking.
    detecting: bool,
    /// A request to clear the acquisition tables at the next decimated input
    /// sample.
    first: bool,
    /// Low-pass filtered and decimated Q14 audio, with space for two maximum
    /// acquisition lags.
    detection: [i32; 2 * ACQUISITION_LAGS],
    /// The next slot in the decimated ring, less than twice
    /// `ACQUISITION_LAGS`.
    detection_index: usize,
    /// The wrapping count of decimated samples since acquisition started.
    detection_count: i32,
    /// The window energies for each lag. They normalize the acquisition cycle
    /// difference.
    energy: [i32; ACQUISITION_LAGS],
    /// The cross products for each lag, with the same binary scale as their
    /// energies.
    cross: [i32; ACQUISITION_LAGS],
    /// Full-rate energies for the 40 adjacent candidate lags.
    tracking_energy: [i32; TRACKING_LAGS],
    /// Full-rate cross products for the tracking energies.
    tracking_cross: [i32; TRACKING_LAGS],
    /// The full-rate integer lag of tracking slot zero.
    base: i32,
    /// The number of frames since the last local minimum search. A search
    /// occurs at five.
    tracking_counter: i32,
    /// The latest refined Q6 period. A restart of acquisition sets it to zero.
    period: Q6,
    /// The last refined Q6 period. A restart or an audio reset does not clear
    /// it.
    previous_period: Q6,
    /// The Q14 threshold of the normalized difference. It applies only to the
    /// acceptance of acquisition minima.
    threshold: Q14,
    /// The tracking slot of the last refined minimum. Only the clarity
    /// report reads it.
    best: usize,
}

impl PitchTracker {
    /// Prepares the acquisition filters and allocates the silent full-rate
    /// history.
    /// The two supported rates use low-pass cutoffs at rate/32 and at 1.5
    /// times that cutoff. The validated sensitivity is quantized to Q14.
    pub(crate) fn new(rate: SampleRate, config: PitchConfig) -> Self {
        let mut pre = [Biquad::new(), Biquad::new()];
        let cutoff = rate.hz() / 16.0 / 2.0;
        pre[0].set_design(Design::low_pass(rate.hz(), cutoff, 0.707));
        pre[1].set_design(Design::low_pass(rate.hz(), cutoff * 1.5, 0.707));
        Self {
            pre,
            raw: vec![0; HISTORY]
                .into_boxed_slice()
                .try_into()
                .expect("prepared pitch history length"),
            write: 0,
            detecting: true,
            first: true,
            detection: [0; 2 * ACQUISITION_LAGS],
            detection_index: 0,
            detection_count: 0,
            energy: [0; ACQUISITION_LAGS],
            cross: [0; ACQUISITION_LAGS],
            tracking_energy: [0; TRACKING_LAGS],
            tracking_cross: [0; TRACKING_LAGS],
            base: 0,
            tracking_counter: 0,
            period: Q6::default(),
            previous_period: Q6::default(),
            threshold: Q14::from_f64(config.threshold as f64),
            best: 0,
        }
    }

    /// Updates only the acquisition sensitivity.
    /// The reporting bounds apply outside the tracker. A change of the
    /// sensitivity does not reset a tracked period.
    pub(crate) fn set_config(&mut self, config: PitchConfig) {
        self.threshold = Q14::from_f64(config.threshold as f64);
    }

    /// Clears the full-rate audio, the current period and the prefilter
    /// memories, and requests acquisition.
    /// The acquisition tables clear at the next decimated call. The previous
    /// usable period stays until another tracking update replaces it.
    pub(crate) fn reset(&mut self) {
        self.raw.fill(0);
        self.write = 0;
        self.restart();
        for pre in &mut self.pre {
            pre.reset();
        }
    }

    /// Invalidates the current period and keeps the previous usable period.
    /// The acquisition tables clear later, at the next acquisition sample.
    fn restart(&mut self) {
        self.period = Q6::default();
        self.detecting = true;
        self.first = true;
    }

    /// Returns `true` if acquisition is active before the next frame.
    pub(crate) fn is_detecting(&self) -> bool {
        self.detecting
    }

    /// Returns how periodic the tracked input is at the last refined lag,
    /// from 0 to 1.
    ///
    /// The value is one minus two times the normalized cycle difference. A
    /// perfect repeat gives 1. Input with no relation between cycles gives
    /// 0. During acquisition, or if the window energy is not positive, the
    /// value is 0. The tracker state does not change.
    pub(crate) fn clarity(&self) -> f32 {
        if self.detecting {
            return 0.0;
        }
        let energy = self.tracking_energy[self.best] as f64;
        if energy <= 0.0 {
            return 0.0;
        }
        let difference = energy / 2.0 - self.tracking_cross[self.best] as f64;
        (1.0 - 2.0 * difference / energy).clamp(0.0, 1.0) as f32
    }

    /// Updates a contiguous run of acquisition lags that does not wrap.
    ///
    /// For `n = energy.len() > 0`, `current` and `cross` have length
    /// n, and `delayed` has length `2*n - 1`. The function reads the input
    /// windows backward with strides of one and two. Four independent lanes
    /// make parallel work visible. They do not change the order of the
    /// wrapping operations for each lag.
    #[inline]
    fn update_detection_segment(
        sample: i32,
        square: i32,
        current: &[i32],
        delayed: &[i32],
        energy: &mut [i32],
        cross: &mut [i32],
    ) {
        let count = energy.len();
        debug_assert!(count > 0);
        debug_assert_eq!(cross.len(), count);
        debug_assert_eq!(current.len(), count);
        debug_assert_eq!(delayed.len(), (count - 1) * 2 + 1);

        // Fixed-size views make four independent updates visible to the
        // optimizer.
        let quad_end = count & !3;
        let mut lane = 0;
        while lane < quad_end {
            let end = lane + 4;
            let current_end = count - lane;
            let delayed_end = (count - lane) * 2 - 1;
            let current: &[i32; 4] = (&current[current_end - 4..current_end])
                .try_into()
                .expect("four acquisition current lanes");
            let delayed: &[i32; 7] = (&delayed[delayed_end - 7..delayed_end])
                .try_into()
                .expect("four strided acquisition delayed lanes");
            let energy: &mut [i32; 4] = (&mut energy[lane..end])
                .try_into()
                .expect("four acquisition energy lanes");
            let cross: &mut [i32; 4] = (&mut cross[lane..end])
                .try_into()
                .expect("four acquisition cross lanes");

            let delta_cross = mul14(sample, current[3]).wrapping_sub(mul14(current[3], delayed[6]));
            let delta_energy = square.wrapping_sub(mul14(delayed[6], delayed[6]));
            cross[0] = cross[0].wrapping_add(delta_cross);
            energy[0] = energy[0].wrapping_add(delta_energy);

            let delta_cross = mul14(sample, current[2]).wrapping_sub(mul14(current[2], delayed[4]));
            let delta_energy = square.wrapping_sub(mul14(delayed[4], delayed[4]));
            cross[1] = cross[1].wrapping_add(delta_cross);
            energy[1] = energy[1].wrapping_add(delta_energy);

            let delta_cross = mul14(sample, current[1]).wrapping_sub(mul14(current[1], delayed[2]));
            let delta_energy = square.wrapping_sub(mul14(delayed[2], delayed[2]));
            cross[2] = cross[2].wrapping_add(delta_cross);
            energy[2] = energy[2].wrapping_add(delta_energy);

            let delta_cross = mul14(sample, current[0]).wrapping_sub(mul14(current[0], delayed[0]));
            let delta_energy = square.wrapping_sub(mul14(delayed[0], delayed[0]));
            cross[3] = cross[3].wrapping_add(delta_cross);
            energy[3] = energy[3].wrapping_add(delta_energy);
            lane += 4;
        }

        // Each contiguous run has a maximum of three scalar lanes at the end.
        while lane < count {
            let current = current[count - 1 - lane];
            let delayed = delayed[(count - 1 - lane) * 2];
            let delta_cross = mul14(sample, current).wrapping_sub(mul14(current, delayed));
            let delta_energy = square.wrapping_sub(mul14(delayed, delayed));
            cross[lane] = cross[lane].wrapping_add(delta_cross);
            energy[lane] = energy[lane].wrapping_add(delta_energy);
            lane += 1;
        }
    }

    /// Processes one decimated sample and returns a full-rate search base or
    /// zero.
    ///
    /// The result is positive when repeated minima agree. It is zero while no
    /// candidate is accepted. Two or three accepted minima must have a
    /// spacing within 25/256 of the reference interval. A minimum is accepted
    /// only if its interpolated value is less than the threshold.
    ///
    /// The first 10 decimated samples after a restart give no candidate.
    /// The function subtracts 20 from the selected full-rate period. This puts
    /// the period at the center of the 40-lag tracking window before the
    /// window starts.
    fn detection_lag(&mut self, sample: i32) -> i32 {
        if self.first {
            self.first = false;
            self.detection.fill(0);
            self.energy.fill(0);
            self.cross.fill(0);
            self.detection_index = 0;
            self.detection_count = 0;
            self.detecting = true;
        }
        let mut index1 = self.detection_index;
        let mut index2 = index1;
        self.detection_index += 1;
        if self.detection_index == 2 * ACQUISITION_LAGS {
            self.detection_index = 0;
        }
        self.detection[index1] = sample;
        let square = mul14(sample, sample);
        let mut lag = 0;
        while lag < ACQUISITION_LAGS {
            // Neither input window wraps in this run. For all lanes, the two
            // input cursors cross a maximum of two different boundaries.
            let count = (ACQUISITION_LAGS - lag).min(index1 + 1).min(index2 / 2 + 1);
            let end = lag + count;
            let current_low = index1 + 1 - count;
            let delayed_low = index2 - (count - 1) * 2;
            Self::update_detection_segment(
                sample,
                square,
                &self.detection[current_low..=index1],
                &self.detection[delayed_low..=index2],
                &mut self.energy[lag..end],
                &mut self.cross[lag..end],
            );
            index1 = if count == index1 + 1 {
                273
            } else {
                index1 - count
            };
            index2 = if count == index2 / 2 + 1 {
                index2 + 2 * ACQUISITION_LAGS - count * 2
            } else {
                index2 - count * 2
            };
            lag = end;
        }
        self.detection_count = self.detection_count.wrapping_add(1);
        if self.detection_count <= 10 {
            return 0;
        }
        let maximum = ((self.detection_count - 6) / 2).min(ACQUISITION_LAGS as i32 - 1) as usize;
        let mut normalized = [0_i32; ACQUISITION_LAGS];
        #[expect(
            clippy::needless_range_loop,
            reason = "one lag index drives three tables in the hot loop"
        )]
        for lag in 1..maximum {
            let mut energy = self.energy[lag];
            let mut cross = self.cross[lag];
            while energy > ONE {
                energy >>= 1;
                cross >>= 1;
            }
            normalized[lag] = if energy > 0 {
                (energy / 2).wrapping_sub(cross).wrapping_shl(14) / energy
            } else {
                ONE
            };
        }
        let mut lag = 2;
        while lag < maximum.saturating_sub(2) {
            if normalized[lag] > 9830 {
                break;
            }
            lag += 1;
        }
        lag += 1;
        let mut peaks = [0_i32; 3];
        let mut count = 0;
        while lag < maximum.saturating_sub(1) {
            if normalized[lag] < normalized[lag - 1] && normalized[lag] < normalized[lag + 1] {
                let difference = normalized[lag - 1].wrapping_sub(normalized[lag + 1]) >> 1;
                let denominator = normalized[lag - 1]
                    .wrapping_sub(normalized[lag].wrapping_mul(2))
                    .wrapping_add(normalized[lag + 1]);
                // Wrapped curvature can be zero. Skip that candidate before
                // the division.
                if denominator == 0 {
                    lag += 1;
                    continue;
                }
                let delta = difference.wrapping_shl(14).wrapping_div(denominator);
                let value = mul14(
                    normalized[lag - 1] >> 1,
                    mul14(delta.wrapping_sub(ONE), delta),
                )
                .wrapping_sub(mul14(
                    normalized[lag],
                    mul14(ONE.wrapping_add(delta), delta.wrapping_sub(ONE)),
                ))
                .wrapping_add(mul14(
                    normalized[lag + 1] >> 1,
                    mul14(ONE.wrapping_add(delta), delta),
                ));
                if self.threshold.to_bits() > value {
                    peaks[count] = (lag as i32).wrapping_shl(11).wrapping_add(delta >> 3);
                    count += 1;
                    if count == 3 {
                        break;
                    }
                }
            }
            lag += 1;
        }
        let mut best = 0;
        if count == 3 {
            let a = peaks[0];
            let b = peaks[1].wrapping_sub(peaks[0]);
            let c = peaks[2].wrapping_sub(peaks[1]);
            if a.wrapping_sub(b).wrapping_abs() < (a.wrapping_mul(25) >> 8)
                && b.wrapping_sub(c).wrapping_abs() < (a.wrapping_mul(25) >> 8)
            {
                best = peaks[2] / 3;
            } else if b.wrapping_sub(c).wrapping_abs() < (b.wrapping_mul(25) >> 8) {
                best = b.wrapping_add(c) >> 1;
            }
        } else if count == 2 {
            let a = peaks[0];
            let b = peaks[1].wrapping_sub(peaks[0]);
            if a.wrapping_sub(b).wrapping_abs() < (a.wrapping_mul(25) >> 8) {
                best = a.wrapping_add(b) >> 1;
            }
        }
        if best == 0 {
            0
        } else {
            ((128_i32.wrapping_add(best)) >> 8).wrapping_sub(20)
        }
    }

    /// Builds the energies and cross terms for 40 full-rate lags from a
    /// positive base.
    /// Candidate acquisition makes sure that the necessary history fits in
    /// the ring. The counter starts at four, so the next tracking sample
    /// searches immediately.
    fn initialize_tracking(&mut self, period: i32) {
        self.base = period;
        let mut energy: i32 = 0;
        let mut index = self.write;
        for _ in 0..period * 2 {
            let sample = self.raw[index];
            index = if index == 0 { HISTORY - 1 } else { index - 1 };
            energy = energy.wrapping_add(mul14(sample, sample));
        }
        self.tracking_energy[0] = energy;
        index = ring(self.write as i32 - period * 2);
        for lag in 1..TRACKING_LAGS {
            let first = self.raw[index];
            index = if index == 0 { HISTORY - 1 } else { index - 1 };
            let second = self.raw[index];
            index = if index == 0 { HISTORY - 1 } else { index - 1 };
            energy = energy.wrapping_add(mul14(first, first).wrapping_add(mul14(second, second)));
            self.tracking_energy[lag] = energy;
        }
        for lag in 0..TRACKING_LAGS {
            self.tracking_cross[lag] = self.cross_at(period + lag as i32);
        }
        self.tracking_counter = 4;
        self.detecting = false;
    }

    /// Calculates the wrapping Q14 cross sum of two adjacent periods of raw
    /// audio.
    /// The positive integer period and its two-period window must fit in the
    /// history.
    fn cross_at(&self, period: i32) -> i32 {
        let mut first = self.write;
        let mut second = ring(self.write as i32 - period);
        let mut cross = 0_i32;
        for _ in 0..period {
            cross = cross.wrapping_add(mul14(self.raw[first], self.raw[second]));
            first = if first == 0 { HISTORY - 1 } else { first - 1 };
            second = if second == 0 { HISTORY - 1 } else { second - 1 };
        }
        cross
    }

    /// Increments the full-rate energy and cross windows when all reads are
    /// contiguous.
    /// The fixed array lengths encode 40 lag samples and 79 samples with a
    /// stride of two. The separate wrapping path in `process_sample` uses the
    /// same arithmetic.
    #[inline]
    fn track_contiguous(
        current: i32,
        middle: &[i32; TRACKING_LAGS],
        delayed: &[i32; 2 * TRACKING_LAGS - 1],
        energy: &mut [i32; TRACKING_LAGS],
        cross: &mut [i32; TRACKING_LAGS],
    ) {
        for lag in 0..TRACKING_LAGS {
            let middle = middle[TRACKING_LAGS - 1 - lag];
            let delayed = delayed[2 * (TRACKING_LAGS - 1) - lag * 2];
            energy[lag] = energy[lag]
                .wrapping_add(mul14(current, current).wrapping_sub(mul14(delayed, delayed)));
            cross[lag] = cross[lag]
                .wrapping_add(mul14(current, middle).wrapping_sub(mul14(middle, delayed)));
        }
    }

    /// Stores one Q14 sample word, advances acquisition or tracking, and
    /// reports transitions.
    ///
    /// The filters always advance. Acquisition uses only every eighth frame.
    /// Every five frames, tracking finds its minimum again and interpolates a
    /// Q6 period. If the minimum is outside slots 18..=21, the lag window
    /// first moves one lag toward it.
    ///
    /// If the minimum is at a window boundary, its lag is 19 or less, or the
    /// curvature is zero, acquisition starts again. The last usable period is
    /// not discarded.
    pub(crate) fn process_sample(&mut self, sample: i32) -> PitchStep {
        self.write += 1;
        if self.write == HISTORY {
            self.write = 0;
        }
        self.raw[self.write] = sample;
        let mut filtered = Q14::from_bits(sample);
        for pre in &mut self.pre {
            filtered = pre.process_sample(filtered);
        }
        let filtered = filtered.to_bits();
        let mut step = PitchStep::default();
        if self.detecting {
            if self.write & 7 == 0 {
                let lag = self.detection_lag(filtered);
                if lag > 0 {
                    self.initialize_tracking(lag);
                    step.acquired = true;
                }
            }
        } else {
            let current = self.raw[self.write];
            let mut first = ring(self.write as i32 - self.base);
            let mut second = ring(self.write as i32 - self.base * 2);
            if first >= TRACKING_LAGS - 1 && second >= 2 * (TRACKING_LAGS - 1) {
                // Contiguous windows let each lag update without a wrap of
                // its indices.
                Self::track_contiguous(
                    current,
                    self.raw[first - (TRACKING_LAGS - 1)..=first]
                        .try_into()
                        .expect("a full tracking window"),
                    self.raw[second - 2 * (TRACKING_LAGS - 1)..=second]
                        .try_into()
                        .expect("79 tracking samples"),
                    &mut self.tracking_energy,
                    &mut self.tracking_cross,
                );
            } else {
                for lag in 0..TRACKING_LAGS {
                    let middle = self.raw[first];
                    first = if first == 0 { HISTORY - 1 } else { first - 1 };
                    let delayed = self.raw[second];
                    second = if second < 2 {
                        second + HISTORY - 2
                    } else {
                        second - 2
                    };
                    self.tracking_energy[lag] = self.tracking_energy[lag].wrapping_add(
                        mul14(current, current).wrapping_sub(mul14(delayed, delayed)),
                    );
                    self.tracking_cross[lag] = self.tracking_cross[lag]
                        .wrapping_add(mul14(current, middle).wrapping_sub(mul14(middle, delayed)));
                }
            }
            self.tracking_counter += 1;
            if self.tracking_counter == 5 {
                self.tracking_counter = 0;
                let mut best = tracking_minimum(|lag| {
                    (self.tracking_energy[lag] >> 1).wrapping_sub(self.tracking_cross[lag])
                });
                if best == TRACKING_LAGS - 1 || best == 0 || self.base + best as i32 <= 19 {
                    self.restart();
                    step.restarted = true;
                } else {
                    if best <= 17 && self.base > 1 {
                        self.base -= 1;
                        best += 1;
                        self.tracking_energy.copy_within(0..TRACKING_LAGS - 1, 1);
                        self.tracking_cross.copy_within(0..TRACKING_LAGS - 1, 1);
                        let first = self.raw[ring(self.write as i32 - self.base * 2)];
                        let second = self.raw[ring(self.write as i32 - self.base * 2 - 1)];
                        self.tracking_energy[0] = self.tracking_energy[1]
                            .wrapping_sub(mul14(first, first).wrapping_add(mul14(second, second)));
                        self.tracking_cross[0] = self.cross_at(self.base);
                    } else if best > 21 {
                        self.base += 1;
                        best -= 1;
                        self.tracking_energy.copy_within(1..TRACKING_LAGS, 0);
                        self.tracking_cross.copy_within(1..TRACKING_LAGS, 0);
                        let total = self.base + TRACKING_LAGS as i32 - 1;
                        let first = self.raw[ring(self.write as i32 - total * 2 + 2)];
                        let second = self.raw[ring(self.write as i32 - total * 2 + 1)];
                        self.tracking_energy[TRACKING_LAGS - 1] = self.tracking_energy
                            [TRACKING_LAGS - 2]
                            .wrapping_add(mul14(second, second).wrapping_add(mul14(first, first)));
                        self.tracking_cross[TRACKING_LAGS - 1] = self.cross_at(total);
                    }
                    let y0 = (self.tracking_energy[best - 1] >> 1)
                        .wrapping_sub(self.tracking_cross[best - 1]);
                    let y1 =
                        (self.tracking_energy[best] >> 1).wrapping_sub(self.tracking_cross[best]);
                    let y2 = (self.tracking_energy[best + 1] >> 1)
                        .wrapping_sub(self.tracking_cross[best + 1]);
                    let mut numerator = y0.wrapping_sub(y2) >> 1;
                    let mut denominator = y1.wrapping_mul(-2).wrapping_add(y0).wrapping_add(y2);
                    while numerator.wrapping_abs() > ONE {
                        numerator >>= 1;
                        denominator >>= 1;
                    }
                    if denominator == 0 {
                        self.restart();
                        step.restarted = true;
                    } else {
                        let fraction = numerator.wrapping_shl(14).wrapping_div(denominator);
                        // The lag in Q8 plus the parabolic fraction
                        // (Q14 >> 6), rounded to Q6.
                        let lag = self.base.wrapping_add(best as i32).wrapping_shl(8);
                        self.period =
                            Q6::from_bits(lag.wrapping_add(fraction >> 6).wrapping_add(2) >> 2);
                        self.previous_period = self.period;
                        self.best = best;
                        step.tracking_update = true;
                    }
                }
            }
        }
        step.period = self.period;
        step.previous_period = self.previous_period;
        step
    }
}
