//! Fixed-point 2x and 4x resampling with Q14 biquad low-pass sections.
//!
//! A 2x stage filters the input with two second-order Butterworth sections,
//! runs the core path on every second frame and filters the zero-stuffed
//! stereo output with two second-order Butterworth sections on each channel.
//! A 4x resampler puts an outer 2x stage around an inner 2x stage.

use crate::filter::{Biquad, Design};
use crate::numeric::clamp_i32;
use crate::numeric::fixed::Q14;
use crate::SampleRate;

/// The quality factors of a fourth-order Butterworth low-pass cascade.
const BUTTERWORTH_Q: [f32; 2] = [0.541_196_1, 1.306_563];

/// The cutoff of each 2x low-pass cascade, as a fraction of the lower rate.
const CUTOFF_RATIO: f32 = 0.42;

/// The nominal delay of one 2x stage, in frames at the upper rate of that
/// stage.
const STAGE_DELAY_FRAMES: usize = 4;

/// The drain allowance of one 2x stage, in frames at the upper rate of that
/// stage.
const STAGE_TAIL_FRAMES: usize = 64;

/// One 2:1 decimation and 1:2 interpolation stage in Q14 format.
#[derive(Debug)]
struct Stage2x {
    /// Two anti-alias low-pass sections for the mono input.
    decimate: [Biquad; 2],
    /// Two anti-image low-pass sections for each stereo output channel.
    interpolate: [[Biquad; 2]; 2],
    /// If `false`, the next frame runs the lower-rate path.
    odd: bool,
}

impl Stage2x {
    /// Designs the input and output cascades for a 2x stage above `low_rate`.
    fn new(low_rate: f32) -> Self {
        let high_rate = low_rate * 2.0;
        let cutoff = low_rate * CUTOFF_RATIO;
        let make_cascade = || {
            std::array::from_fn(|section| {
                let mut filter = Biquad::new();
                filter.set_design(Design::low_pass(high_rate, cutoff, BUTTERWORTH_Q[section]));
                filter
            })
        };
        Self {
            decimate: make_cascade(),
            interpolate: [make_cascade(), make_cascade()],
            odd: false,
        }
    }

    /// Clears the phase and all filter histories, and keeps the coefficients.
    fn reset(&mut self) {
        self.odd = false;
        for filter in &mut self.decimate {
            filter.reset();
        }
        self.reset_output();
    }

    /// Clears only the output filter histories.
    fn reset_output(&mut self) {
        for channel in &mut self.interpolate {
            for filter in channel {
                filter.reset();
            }
        }
    }

    /// Processes one upper-rate Q14 frame and calls `core` on even frames.
    #[inline]
    fn process(&mut self, mut input: Q14, core: impl FnOnce(Q14) -> [Q14; 2]) -> [Q14; 2] {
        for filter in &mut self.decimate {
            input = filter.process_sample(input);
        }
        let excited = if !self.odd {
            let step = core(input);
            step.map(|sample| Q14::from_bits(clamp_i32(i64::from(sample.to_bits()) << 1)))
        } else {
            [Q14::ZERO; 2]
        };
        self.odd = !self.odd;
        let mut output = [Q14::ZERO; 2];
        for (slot, (mut sample, filters)) in output
            .iter_mut()
            .zip(excited.into_iter().zip(&mut self.interpolate))
        {
            for filter in filters {
                sample = filter.process_sample(sample);
            }
            *slot = sample;
        }
        output
    }
}

/// The 2x or 4x rate converter around the 44.1 kHz or 48 kHz core path.
#[derive(Debug)]
pub(crate) struct Resampler {
    /// The host-rate 2x stage (between 2x and 1x, or between 4x and 2x).
    outer: Stage2x,
    /// The optional inner 2x stage between 2x and 1x for 4x rates.
    inner: Option<Stage2x>,
}

impl Resampler {
    /// Returns a boxed resampler for 2x or 4x rates, or `None` for 1x rates.
    pub(crate) fn new(rate: SampleRate) -> Option<Box<Self>> {
        let base_hz = rate.base_rate().hz();
        match rate.factor() {
            1 => None,
            2 => Some(Box::new(Self {
                outer: Stage2x::new(base_hz),
                inner: None,
            })),
            4 => Some(Box::new(Self {
                outer: Stage2x::new(base_hz * 2.0),
                inner: Some(Stage2x::new(base_hz)),
            })),
            _ => None,
        }
    }

    /// Returns the nominal filter and commutation delay in host frames.
    pub(crate) const fn delay_frames(&self) -> usize {
        if self.inner.is_some() {
            STAGE_DELAY_FRAMES + 2 * STAGE_DELAY_FRAMES
        } else {
            STAGE_DELAY_FRAMES
        }
    }

    /// Returns the finite tail allowance of the filter cascades in host frames.
    pub(crate) const fn tail_frames(&self) -> usize {
        if self.inner.is_some() {
            STAGE_TAIL_FRAMES + 2 * STAGE_TAIL_FRAMES
        } else {
            STAGE_TAIL_FRAMES
        }
    }

    /// Clears the phase and all filter histories.
    pub(crate) fn reset(&mut self) {
        self.outer.reset();
        if let Some(inner) = &mut self.inner {
            inner.reset();
        }
    }

    /// Clears the output filter histories after a stereo pan mode change.
    pub(crate) fn reset_output(&mut self) {
        self.outer.reset_output();
        if let Some(inner) = &mut self.inner {
            inner.reset_output();
        }
    }

    /// Processes one host-rate PCM sample around the base-rate core callback.
    pub(crate) fn process_sample(
        &mut self,
        input: i16,
        mut core: impl FnMut(i16) -> [i16; 2],
    ) -> [i16; 2] {
        let sample = pcm_to_q14(input);
        let mut run_core = |decimated: Q14| core(q14_to_pcm(decimated)).map(pcm_to_q14);
        let output = match &mut self.inner {
            None => self.outer.process(sample, &mut run_core),
            Some(inner) => self
                .outer
                .process(sample, |mid| inner.process(mid, &mut run_core)),
        };
        output.map(q14_to_pcm)
    }
}

/// Converts a 16-bit PCM sample to Q14 audio, where 1.0 is full scale.
#[inline]
fn pcm_to_q14(sample: i16) -> Q14 {
    Q14::from_bits(i32::from(sample) >> 1)
}

/// Converts a Q14 audio sample to symmetric 16-bit PCM.
#[inline]
fn q14_to_pcm(sample: Q14) -> i16 {
    (i64::from(sample.to_bits()) << 1).clamp(-32767, 32767) as i16
}
