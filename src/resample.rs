//! The fixed-point polyphase sinc rate converter around the internal DSP core.
//!
//! At host rates other than 44,100 Hz and 48,000 Hz, this module converts
//! mono PCM input to the core rate and converts stereo PCM output back to the
//! host rate. Construction precomputes Q14 polyphase FIR tables and allocates
//! the conversion buffers. Audio calls use 32-bit integer phase accumulators
//! and Q14 dot products without heap allocation or floating-point operations.

use crate::error::ConfigProblem;
use crate::numeric::fixed::Q14;
use crate::numeric::round_q14;
use crate::{Error, SampleRate};
use std::f64::consts::PI;

/// The number of fractional phase intervals in each polyphase table.
///
/// Each table stores `PHASES + 1` branches so rounded phase lookups stay in
/// bounds at both endpoints of the unit interval.
const PHASES: usize = 512;

/// The half-length in taps of the prototype sinc filter before expansion for
/// lower output rates.
const BASE_HALF_TAPS: usize = 12;

/// The normalized low-pass cutoff relative to the lower Nyquist frequency.
const CUTOFF_RATIO: f64 = 0.90;

/// The preallocated mono-input and stereo-output polyphase rate converters.
#[derive(Debug)]
pub(crate) struct Resampler {
    /// The host sample rate in hertz.
    host_hz: u32,
    /// The internal core sample rate in hertz (44,100 or 48,000).
    core_hz: u32,
    /// The reduced host step (`host_hz / gcd`).
    host_step: u32,
    /// The reduced core step (`core_hz / gcd`).
    core_step: u32,
    /// The mono polyphase filter from the host rate to the core rate.
    input_filter: MonoPolyphase,
    /// The stereo polyphase filter from the core rate to the host rate.
    output_filter: StereoPolyphase,
    /// The preallocated mono core samples for one host block.
    pub(crate) core_input: Vec<i16>,
    /// The preallocated stereo core frames for one host block.
    pub(crate) core_output: Vec<[i16; 2]>,
}

/// A mono polyphase FIR filter with a double-length contiguous history ring.
#[derive(Debug)]
struct MonoPolyphase {
    /// The flattened `(PHASES + 1) * taps` Q14 coefficients.
    table: Vec<Q14>,
    /// A double-length ring of size `2 * taps` for contiguous slice reads.
    history: Vec<i16>,
    /// The number of taps in each polyphase branch.
    taps: usize,
    /// Half of `taps`, equal to the nominal delay in input frames.
    half: usize,
    /// The write cursor in `0..taps`.
    write: usize,
    /// The 32-bit modular phase accumulator.
    phase: u32,
}

/// A stereo polyphase FIR filter with a double-length contiguous history ring.
#[derive(Debug)]
struct StereoPolyphase {
    /// The flattened `(PHASES + 1) * taps` Q14 coefficients.
    table: Vec<Q14>,
    /// A double-length stereo ring of size `2 * taps`.
    history: Vec<[i16; 2]>,
    /// The number of taps in each polyphase branch.
    taps: usize,
    /// Half of `taps`, equal to the nominal delay in core frames.
    half: usize,
    /// The write cursor in `0..taps`.
    write: usize,
    /// The 32-bit modular phase accumulator, locked to `MonoPolyphase::phase`.
    phase: u32,
}

impl Resampler {
    /// Precomputes Q14 polyphase tables and allocates conversion buffers for
    /// `max_block_frames`.
    ///
    /// Returns `Ok(None)` at 44,100 Hz and 48,000 Hz so the processor bypasses
    /// rate conversion.
    pub(crate) fn new(
        host_rate: SampleRate,
        core_rate: SampleRate,
        max_block_frames: usize,
    ) -> Result<Option<Self>, Error> {
        let host_hz = host_rate.as_u32();
        let core_hz = core_rate.as_u32();
        if host_hz == core_hz {
            return Ok(None);
        }
        let divisor = gcd(host_hz, core_hz);
        let host_step = host_hz / divisor;
        let core_step = core_hz / divisor;

        let max_core_u128 =
            (max_block_frames as u128 * core_step as u128).div_ceil(host_step as u128);
        let max_limit = (isize::MAX as usize) / std::mem::size_of::<[i16; 2]>();
        if max_core_u128 > max_limit as u128 {
            return Err(Error::config(
                "prepare.max_block_frames",
                ConfigProblem::OutOfRange {
                    min: 1.0,
                    max: ((max_limit as u128 * host_step as u128) / core_step as u128) as f64,
                },
            ));
        }
        let max_core_frames = max_core_u128 as usize;

        let mut core_input = Vec::new();
        core_input.try_reserve_exact(max_core_frames).map_err(|_| {
            Error::config(
                "prepare.max_block_frames",
                ConfigProblem::OutOfRange {
                    min: 1.0,
                    max: max_limit as f64,
                },
            )
        })?;
        core_input.resize(max_core_frames, 0);

        let mut core_output = Vec::new();
        core_output
            .try_reserve_exact(max_core_frames)
            .map_err(|_| {
                Error::config(
                    "prepare.max_block_frames",
                    ConfigProblem::OutOfRange {
                        min: 1.0,
                        max: max_limit as f64,
                    },
                )
            })?;
        core_output.resize(max_core_frames, [0; 2]);

        Ok(Some(Self {
            host_hz,
            core_hz,
            host_step,
            core_step,
            input_filter: MonoPolyphase::new(host_hz, core_hz),
            output_filter: StereoPolyphase::new(core_hz, host_hz),
            core_input,
            core_output,
        }))
    }

    /// Clears both filter histories and resets both phase accumulators to zero.
    pub(crate) fn reset(&mut self) {
        self.input_filter.reset();
        self.output_filter.reset();
    }

    /// Converts `host_frames` input samples from `read` into `self.core_input`
    /// and returns the number of generated core frames.
    pub(crate) fn resample_input(
        &mut self,
        host_frames: usize,
        mut read: impl FnMut(usize) -> i16,
    ) -> usize {
        let mut core_count = 0;
        let host_step = self.host_step;
        let core_step = self.core_step;
        let half_core = core_step >> 1;
        for index in 0..host_frames {
            self.input_filter.push(read(index));
            while self.input_filter.phase < core_step {
                let phase_index =
                    ((self.input_filter.phase * PHASES as u32 + half_core) / core_step) as usize;
                self.core_input[core_count] = self.input_filter.evaluate(phase_index);
                core_count += 1;
                self.input_filter.phase += host_step;
            }
            self.input_filter.phase -= core_step;
        }
        core_count
    }

    /// Converts `host_frames` of silent host input into `self.core_input` and
    /// returns the number of generated core frames.
    pub(crate) fn resample_silence(&mut self, host_frames: usize) -> usize {
        self.resample_input(host_frames, |_| 0)
    }

    /// Converts `core_count` frames from `self.core_output` into `host_frames`
    /// stereo frames at the host rate.
    pub(crate) fn resample_output(
        &mut self,
        core_count: usize,
        host_frames: usize,
        mut write: impl FnMut(usize, [i16; 2]),
    ) {
        let mut core_index = 0;
        let host_step = self.host_step;
        let core_step = self.core_step;
        let half_host = host_step >> 1;
        for host_index in 0..host_frames {
            while self.output_filter.phase < core_step {
                self.output_filter.push(self.core_output[core_index]);
                core_index += 1;
                self.output_filter.phase += host_step;
            }
            self.output_filter.phase -= core_step;
            let lag_index =
                ((self.output_filter.phase * PHASES as u32 + half_host) / host_step) as usize;
            let frame = self.output_filter.evaluate(PHASES - lag_index);
            write(host_index, frame);
        }
        debug_assert_eq!(core_index, core_count);
    }

    /// Returns the total nominal latency in host frames from `core_latency`.
    pub(crate) fn latency_frames(&self, core_latency: usize) -> usize {
        let core_total = (core_latency + self.output_filter.half) as u64;
        let scaled = (core_total * u64::from(self.host_hz) + u64::from(self.core_hz / 2))
            / u64::from(self.core_hz);
        self.input_filter.half.saturating_add(scaled as usize)
    }

    /// Converts a core tail budget into a host frame budget that also flushes
    /// both polyphase filters.
    pub(crate) fn tail_frames(&self, core_tail: usize) -> usize {
        let core_total = core_tail.saturating_add(self.output_filter.taps) as u128;
        let host_tail = (core_total * self.host_hz as u128).div_ceil(self.core_hz as u128);
        self.input_filter.taps.saturating_add(host_tail as usize)
    }
}

impl MonoPolyphase {
    /// Builds a mono polyphase sinc filter for `in_hz` to `out_hz`.
    fn new(in_hz: u32, out_hz: u32) -> Self {
        let (table, taps, half) = build_polyphase_table(in_hz, out_hz);
        Self {
            table,
            history: vec![0; 2 * taps],
            taps,
            half,
            write: 0,
            phase: 0,
        }
    }

    /// Clears the sample history and sets the phase accumulator to zero.
    fn reset(&mut self) {
        self.history.fill(0);
        self.write = 0;
        self.phase = 0;
    }

    /// Stores one input sample into both halves of the contiguous ring.
    #[inline]
    fn push(&mut self, sample: i16) {
        self.history[self.write] = sample;
        self.history[self.write + self.taps] = sample;
        self.write += 1;
        if self.write == self.taps {
            self.write = 0;
        }
    }

    /// Evaluates one polyphase FIR branch in Q14 fixed-point arithmetic.
    #[inline]
    fn evaluate(&self, phase_index: usize) -> i16 {
        let samples = &self.history[self.write..self.write + self.taps];
        let offset = phase_index * self.taps;
        let coeffs = &self.table[offset..offset + self.taps];
        let mut sum = 0_i64;
        for (&sample, &coeff) in samples.iter().zip(coeffs) {
            sum = sum.saturating_add(i64::from(sample) * i64::from(coeff.to_bits()));
        }
        round_q14(sum).clamp(-32768, 32767) as i16
    }
}

impl StereoPolyphase {
    /// Builds a stereo polyphase sinc filter for `in_hz` to `out_hz`.
    fn new(in_hz: u32, out_hz: u32) -> Self {
        let (table, taps, half) = build_polyphase_table(in_hz, out_hz);
        Self {
            table,
            history: vec![[0; 2]; 2 * taps],
            taps,
            half,
            write: 0,
            phase: 0,
        }
    }

    /// Clears the stereo sample history and sets the phase accumulator to zero.
    fn reset(&mut self) {
        self.history.fill([0; 2]);
        self.write = 0;
        self.phase = 0;
    }

    /// Stores one stereo frame into both halves of the contiguous ring.
    #[inline]
    fn push(&mut self, frame: [i16; 2]) {
        self.history[self.write] = frame;
        self.history[self.write + self.taps] = frame;
        self.write += 1;
        if self.write == self.taps {
            self.write = 0;
        }
    }

    /// Evaluates one polyphase FIR branch on both channels and clips to
    /// `-32767..=32767`.
    #[inline]
    fn evaluate(&self, phase_index: usize) -> [i16; 2] {
        let frames = &self.history[self.write..self.write + self.taps];
        let offset = phase_index * self.taps;
        let coeffs = &self.table[offset..offset + self.taps];
        let mut left = 0_i64;
        let mut right = 0_i64;
        for (&frame, &coeff) in frames.iter().zip(coeffs) {
            let weight = i64::from(coeff.to_bits());
            left = left.saturating_add(i64::from(frame[0]) * weight);
            right = right.saturating_add(i64::from(frame[1]) * weight);
        }
        [
            round_q14(left).clamp(-32767, 32767) as i16,
            round_q14(right).clamp(-32767, 32767) as i16,
        ]
    }
}

/// Computes the `(PHASES + 1) * taps` table of Q14 windowed-sinc coefficients.
///
/// Each polyphase branch is normalized so its Q14 integer sum equals `16384`.
fn build_polyphase_table(in_hz: u32, out_hz: u32) -> (Vec<Q14>, usize, usize) {
    let ratio = f64::from(out_hz) / f64::from(in_hz);
    let (half, cutoff) = if out_hz < in_hz {
        (
            (BASE_HALF_TAPS as f64 / ratio).ceil() as usize,
            CUTOFF_RATIO * ratio,
        )
    } else {
        (BASE_HALF_TAPS, CUTOFF_RATIO)
    };
    let taps = 2 * half;
    let mut table = Vec::with_capacity((PHASES + 1) * taps);
    let mut weights = vec![0.0_f64; taps];

    for phase in 0..=PHASES {
        let fraction = phase as f64 / PHASES as f64;
        let center = (half - 1) as f64 + fraction;
        let mut weight_sum = 0.0_f64;
        for (tap, slot) in weights.iter_mut().enumerate() {
            let delta = center - tap as f64;
            let window_pos = delta / half as f64;
            if window_pos.abs() >= 1.0 {
                *slot = 0.0;
            } else {
                let window = 0.4243801
                    + 0.4973406 * (PI * window_pos).cos()
                    + 0.0782793 * (2.0 * PI * window_pos).cos();
                let arg = delta * cutoff;
                let sinc = if arg.abs() < 1e-12 {
                    1.0
                } else {
                    (PI * arg).sin() / (PI * arg)
                };
                let value = window * sinc * cutoff;
                *slot = value;
                weight_sum += value;
            }
        }

        let start = table.len();
        let mut quantized_sum: i32 = 0;
        for &weight in &weights {
            let q = ((weight / weight_sum) * 16384.0).round() as i32;
            quantized_sum += q;
            table.push(Q14::from_bits(q));
        }
        let center_tap = if fraction < 0.5 { half - 1 } else { half };
        let adjusted = table[start + center_tap].to_bits() + (16_384 - quantized_sum);
        table[start + center_tap] = Q14::from_bits(adjusted);
    }

    (table, taps, half)
}

/// Returns the greatest common divisor of two positive sample rates.
const fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}
