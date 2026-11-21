//! Echo and room networks that share one circular delay buffer.

use crate::error::check_range;
use crate::filter::{Biquad, Design};
use crate::numeric::fixed::{Fixed, Q14};
use crate::{Error, SampleRate};

/// The echo or reverberation network, with the controls that apply to that
/// network.
///
/// The default is [`Self::Echo`], with a diffusion of 0.5 and a decay of 0.6.
/// All networks share one delay buffer. A network change uses the same buffer
/// contents and damping filter memories. Thus a transition can play audio from
/// the previous network. The stereo layout of the buffer does not make all
/// networks give different left and right signals. Schroeder gives the same
/// value on the two channels.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(
        rename_all = "camelCase",
        rename_all_fields = "camelCase",
        tag = "type"
    )
)]
pub enum ReverbNetwork {
    /// A single feedback echo.
    Echo {
        /// The distance of the feedback tap without a unit, 0..=1.
        ///
        /// The default is 0.5.
        diffusion: f32,
        /// The feedback control without a unit, 0..=1.
        ///
        /// The default is 0.6. The prepared coefficient is approximately
        /// `decay / 1.05`. It is less than unity also at the maximum.
        decay: f32,
    },
    /// Four parallel comb filters that feed two allpass filters (mono).
    Schroeder {
        /// The allpass control without a unit, 0..=1.
        ///
        /// The prepared coefficient is approximately `decay / 1.05`.
        decay: f32,
    },
    /// A stereo small room.
    SmallRoom {
        /// The feedback control without a unit, 0..=1.
        ///
        /// The prepared coefficient is approximately `decay / 1.05`.
        decay: f32,
        /// The low-pass cutoff in hertz for the damped paths, 80..=20000.
        ///
        /// The preparation also limits it to 0.49 × sample rate. A change of
        /// the cutoff keeps the sample histories of the filters.
        brightness_hz: f32,
    },
    /// A short binaural room.
    Binaural {
        /// The gain control of the early taps without a unit, 0..=1.
        diffusion: f32,
        /// The low-pass cutoff in hertz for the filtered taps, 80..=20000.
        ///
        /// The preparation also limits it to 0.49 × sample rate. A change of
        /// the cutoff keeps the sample histories of the filters.
        brightness_hz: f32,
    },
    /// A medium hall.
    MediumHall {
        /// The scale of the allpass coefficients without a unit, 0..=1.
        diffusion: f32,
        /// The feedback control without a unit, 0..=1.
        ///
        /// The prepared coefficient is approximately `decay / 1.05`.
        decay: f32,
        /// The low-pass cutoff in hertz for the damped paths, 80..=20000.
        ///
        /// The preparation also limits it to 0.49 × sample rate. A change of
        /// the cutoff keeps the sample histories of the filters.
        brightness_hz: f32,
    },
    /// A large hall.
    LargeHall {
        /// The scale of the allpass coefficients without a unit, 0..=1.
        diffusion: f32,
        /// The feedback control without a unit, 0..=1.
        ///
        /// The prepared coefficient is approximately `decay / 1.05`.
        decay: f32,
        /// The low-pass cutoff in hertz for the damped paths, 80..=20000.
        ///
        /// The preparation also limits it to 0.49 × sample rate. A change of
        /// the cutoff keeps the sample histories of the filters.
        brightness_hz: f32,
    },
}

impl Default for ReverbNetwork {
    fn default() -> Self {
        Self::Echo {
            diffusion: 0.5,
            decay: 0.6,
        }
    }
}

impl ReverbNetwork {
    /// Validates the controls of the selected network.
    ///
    /// Each floating-point control must be finite and in its documented
    /// inclusive range. Validation does not clamp values and does not change
    /// processor state.
    ///
    /// # Errors
    ///
    /// Returns the first [`Error::InvalidConfig`] with its dotted field path
    /// and the problem.
    pub fn validate(self) -> Result<(), Error> {
        match self {
            Self::Echo { diffusion, decay } => {
                check_range("reverb.network.diffusion", diffusion, 0.0, 1.0)?;
                check_range("reverb.network.decay", decay, 0.0, 1.0)
            }
            Self::Schroeder { decay } => check_range("reverb.network.decay", decay, 0.0, 1.0),
            Self::SmallRoom {
                decay,
                brightness_hz,
            } => {
                check_range("reverb.network.decay", decay, 0.0, 1.0)?;
                check_range("reverb.network.brightness_hz", brightness_hz, 80.0, 20000.0)
            }
            Self::Binaural {
                diffusion,
                brightness_hz,
            } => {
                check_range("reverb.network.diffusion", diffusion, 0.0, 1.0)?;
                check_range("reverb.network.brightness_hz", brightness_hz, 80.0, 20000.0)
            }
            Self::MediumHall {
                diffusion,
                decay,
                brightness_hz,
            }
            | Self::LargeHall {
                diffusion,
                decay,
                brightness_hz,
            } => {
                check_range("reverb.network.diffusion", diffusion, 0.0, 1.0)?;
                check_range("reverb.network.decay", decay, 0.0, 1.0)?;
                check_range("reverb.network.brightness_hz", brightness_hz, 80.0, 20000.0)
            }
        }
    }
}

/// The reverb after Alien, with controls for bypass, the wet mix and the
/// active network.
///
/// The reverb is disabled by default. In bypass, the stage returns the input
/// and does not advance the delay network or the damping filters. If the
/// stage is enabled and the mix is zero, the network still operates. A reset
/// clears the delay buffer but keeps the memories of the damping filters. The
/// mix, diffusion and decay controls are quantized to Q14.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub struct ReverbConfig {
    /// Enables the selected network. The default is `false`. In bypass, the
    /// histories do not change.
    pub enabled: bool,
    /// The wet fraction, 0..=1.
    ///
    /// The default is 0.5. The dry weight is `1 - mix`. Schroeder and Binaural
    /// use a mono dry signal. The other networks keep the stereo dry input. If
    /// the network is enabled, a zero mix still advances it.
    pub mix: f32,
    /// The delay network and its active controls. The default is
    /// [`ReverbNetwork::default`].
    pub network: ReverbNetwork,
}
impl Default for ReverbConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mix: 0.5,
            network: ReverbNetwork::default(),
        }
    }
}
impl ReverbConfig {
    /// Validates the wet mix and the active controls of the selected network,
    /// also when the stage is disabled.
    ///
    /// The floating-point controls must be finite and in their documented
    /// inclusive ranges. Validation does not clamp values. It does not change
    /// the processor state.
    ///
    /// # Errors
    ///
    /// Returns the first [`Error::InvalidConfig`] with its dotted field path
    /// and the problem.
    pub fn validate(self) -> Result<(), Error> {
        check_range("reverb.mix", self.mix, 0.0, 1.0)?;
        self.network.validate()
    }
}

/// The shared Q14 delay network with two low-pass damping filters that keep
/// state.
#[derive(Debug)]
pub(crate) struct Reverb {
    /// The prepared sample rate for the cutoff and tap calculations.
    rate: SampleRate,
    /// The validated controls and the selected network.
    config: ReverbConfig,
    /// The shared circular storage for all network topologies.
    delay: DelayLine,
    /// The first damping filter. A delay reset or a mode change does not
    /// clear its memories.
    lpf1: Biquad,
    /// The second damping filter. It keeps its memories in the same way as
    /// the first filter.
    lpf2: Biquad,
    /// 67 prepared frame offsets into the shared ring.
    taps: [usize; 67],
    /// The Q14 wet contribution.
    mix: Q14,
    /// The Q14 control for the diffusion or the echo distance.
    diffuse: Q14,
    /// The Q14 feedback coefficient, scaled by 100/105.
    decay: Q14,
    /// The Binaural delay of approximately 0.8 ms between the tap that gets
    /// the early sum and the filtered tap. Both channels use it.
    binaural_offset: usize,
    /// The reserved last ring offset. The output sums go to this slot, and
    /// the next ring step clears it.
    scratch: usize,
    /// The largest absolute Q14 wet contribution in the telemetry window. No
    /// audio calculation reads it.
    wet_peak: i32,
}

/// The divisor for a division by two that truncates toward zero, also for
/// negative values.
const TWO: Fixed<0> = Fixed::from_bits(2);

/// Quantizes a fixed network coefficient to Q14.
#[inline]
fn q14(coefficient: f64) -> Q14 {
    Q14::from_f64(coefficient)
}

/// Multiplies two Q14 values with a wrapping 32-bit product before the shift.
#[inline]
fn mul(left: Q14, right: Q14) -> Q14 {
    left.mul(right)
}

impl Reverb {
    /// Allocates the shared delay layout and prepares the filters from
    /// validated controls.
    pub(crate) fn new(rate: SampleRate, config: ReverbConfig) -> Self {
        let layout = reverb_layout(rate.hz());
        let brightness = 4000.0_f32.min((rate.hz() as f64 * 0.49) as f32);
        let design = Design::low_pass_one_pole(rate.hz(), brightness);
        let mut lpf1 = Biquad::new();
        let mut lpf2 = Biquad::new();
        lpf1.set_design(design);
        lpf2.set_design(design);
        let mut result = Self {
            rate,
            config,
            delay: DelayLine::new(layout.capacity),
            lpf1,
            lpf2,
            taps: layout.taps,
            mix: Q14::default(),
            diffuse: Q14::from_f64((0.5_f32 * 100.0) as f64 / 100.0),
            decay: Q14::from_f64((0.6_f32 * 100.0) as f64 / 105.0),
            binaural_offset: layout.binaural_offset,
            scratch: layout.capacity - 1,
            wet_peak: 0,
        };
        result.set_config(config);
        result
    }
    /// Updates the coefficients and does not clear the delay or filter state.
    /// The f32 multiplication by 100 occurs before the f64 division and the
    /// quantization. If you combine these expressions, the quantized
    /// coefficients can change.
    pub(crate) fn set_config(&mut self, config: ReverbConfig) {
        self.config = config;
        self.mix = Q14::from_f64((config.mix * 100.0) as f64 / 100.0);
        let (diffusion, decay, brightness_hz) = match config.network {
            ReverbNetwork::Echo { diffusion, decay } => (Some(diffusion), Some(decay), None),
            ReverbNetwork::Schroeder { decay } => (None, Some(decay), None),
            ReverbNetwork::SmallRoom {
                decay,
                brightness_hz,
            } => (None, Some(decay), Some(brightness_hz)),
            ReverbNetwork::Binaural {
                diffusion,
                brightness_hz,
            } => (Some(diffusion), None, Some(brightness_hz)),
            ReverbNetwork::MediumHall {
                diffusion,
                decay,
                brightness_hz,
            }
            | ReverbNetwork::LargeHall {
                diffusion,
                decay,
                brightness_hz,
            } => (Some(diffusion), Some(decay), Some(brightness_hz)),
        };
        if let Some(diffusion) = diffusion {
            self.diffuse = Q14::from_f64((diffusion * 100.0) as f64 / 100.0);
        }
        if let Some(decay) = decay {
            self.decay = Q14::from_f64((decay * 100.0) as f64 / 105.0);
        }
        if let Some(brightness_hz) = brightness_hz {
            let brightness = brightness_hz.min((self.rate.hz() as f64 * 0.49) as f32);
            let design = Design::low_pass_one_pole(self.rate.hz(), brightness);
            self.lpf1.set_design(design);
            self.lpf2.set_design(design);
        }
    }
    /// Clears only the delay contents, and keeps the write cursor and the
    /// damping memories.
    pub(crate) fn reset(&mut self) {
        self.delay.reset();
    }
    /// Returns the largest absolute Q14 wet contribution since the last
    /// clear, as f32.
    pub(crate) fn wet_peak(&self) -> f32 {
        (self.wet_peak as f64 / 16384.0) as f32
    }
    /// Starts a new window for the wet peak. The audio state does not change.
    pub(crate) fn clear_wet_peak(&mut self) {
        self.wet_peak = 0;
    }
    /// Calculates the selected network one time and advances the ring by one
    /// frame.
    /// In bypass, no delay or filter history advances. The internal mono
    /// averages use integer division toward zero. The PCM downmix in the
    /// processor uses a shift.
    pub(crate) fn process_sample(&mut self, frame: [Q14; 2]) -> [Q14; 2] {
        let [left, right] = frame;
        let mono = (left + right).div::<0, 14>(TWO);
        let t = self.taps;
        let dry = Q14::ONE - self.mix;
        let d = &mut self.delay;
        if !self.config.enabled {
            return frame;
        }
        let mut result = match self.config.network {
            ReverbNetwork::Echo { .. } => {
                d.put(mono, t[0]);
                let echo: Fixed<0> = Fixed::<0>::from_bits(t[39] as i32).mul(self.diffuse);
                d.comb(t[0], echo.to_bits() as usize, self.decay);
                [
                    mul(left, dry) + mul(self.mix, d.get(t[25]) - d.get(t[32])),
                    mul(right, dry) - mul(self.mix, d.get(t[26]) - d.get(t[31])),
                ]
            }
            ReverbNetwork::Schroeder { .. } => {
                for i in [0, 13, 23, 29] {
                    d.put(mono.div::<0, 14>(TWO), t[i]);
                }
                for (a, b, g) in [(0, 12, 0.9), (13, 22, 0.89), (23, 28, 0.87), (29, 34, 0.86)] {
                    d.comb(t[a], t[b], q14(g));
                }
                d.fir(
                    t[40],
                    &[
                        (t[12], q14(0.25)),
                        (t[22], q14(0.25)),
                        (t[28], q14(0.25)),
                        (t[34], q14(0.25)),
                    ],
                );
                d.allpass(t[40], t[43], self.decay);
                d.allpass(t[44], t[45], self.decay);
                let value = mul(mono, dry) + mul(self.mix, d.get(t[45]));
                [value, value]
            }
            ReverbNetwork::SmallRoom { .. } => {
                d.put(left, t[0]);
                d.put(right, t[40]);
                for (a, b, g) in [(1, 15, 0.3), (2, 7, 0.7), (8, 14, 0.5), (18, 24, 0.5)] {
                    d.allpass(t[a], t[b], q14(g));
                }
                d.comb_lpf(t[40], t[35], self.decay, &mut self.lpf1);
                for (a, b, g) in [(41, 50, 0.3), (42, 46, 0.6), (53, 57, 0.5)] {
                    d.allpass(t[a], t[b], q14(g));
                }
                d.comb_lpf(t[0], t[66], self.decay, &mut self.lpf2);
                [
                    d.fir(
                        self.scratch,
                        &[(t[16], q14(0.2)), (t[32], q14(0.2)), (t[51], q14(0.2))],
                    ),
                    d.fir(
                        self.scratch,
                        &[(t[49], q14(0.2)), (t[65], q14(0.2)), (t[17], q14(0.2))],
                    ),
                ]
            }
            ReverbNetwork::Binaural { .. } => {
                let seed = t[10];
                let tap = t[25];
                let filter = tap + self.binaural_offset;
                d.put(left, t[0]);
                d.put(right, t[40]);
                let a = mul(
                    self.diffuse >> 1,
                    d.get(t[0] + seed) + d.get(t[11] + seed) + d.get(t[21] + seed),
                );
                d.put(a, tap);
                d.lpf(filter, &mut self.lpf1);
                let out = mul(mono, dry) + mul(self.mix, d.get(t[40] + filter) + a);
                let b = mul(
                    self.diffuse >> 1,
                    d.get(t[47] + seed) + d.get(t[48] + seed) + d.get(t[54] + seed),
                );
                d.put(b, t[40] + tap);
                d.lpf(t[40] + filter, &mut self.lpf2);
                [out, mul(mono, dry) + mul(self.mix, d.get(filter) + b)]
            }
            ReverbNetwork::MediumHall { .. } => {
                d.put(mono.div::<0, 14>(TWO), t[0]);
                d.lpf(t[2], &mut self.lpf1);
                for (a, b, g) in [(3, 15, 0.4), (4, 7, 0.9), (8, 14, 0.6), (18, 24, 0.6)] {
                    d.allpass(t[a], t[b], mul(self.diffuse, q14(g)));
                }
                d.put(d.get(t[35]), t[40]);
                for (a, b, g) in [(41, 50, 0.5), (42, 46, 0.8)] {
                    d.allpass(t[a], t[b], mul(self.diffuse, q14(g)));
                }
                d.comb_lpf(t[0], t[66], self.decay, &mut self.lpf2);
                [
                    d.fir(
                        self.scratch,
                        &[
                            (t[16], q14(-0.25)),
                            (t[33], q14(0.25)),
                            (t[51], q14(-0.25)),
                            (t[55], q14(0.25)),
                        ],
                    ),
                    d.fir(
                        self.scratch,
                        &[
                            (t[16], q14(0.25)),
                            (t[32], q14(0.25)),
                            (t[52], q14(0.25)),
                            (t[56], q14(0.2)),
                        ],
                    ),
                ]
            }
            ReverbNetwork::LargeHall { .. } => {
                d.put(mono, t[0]);
                d.lpf(t[2], &mut self.lpf1);
                for (a, b, g) in [(3, 5, 0.6), (6, 9, 0.6), (19, 30, 0.95), (20, 27, 0.5)] {
                    d.allpass(t[a], t[b], mul(self.diffuse, q14(g)));
                }
                d.put(d.get(t[38]), t[40]);
                for (a, b, g) in [(41, 63, 0.95), (42, 58, 0.5), (59, 62, 0.5)] {
                    d.allpass(t[a], t[b], mul(self.diffuse, q14(g)));
                }
                d.comb_lpf(t[1], t[64], self.decay, &mut self.lpf2);
                [
                    d.fir(
                        self.scratch,
                        &[(t[10], q14(0.6)), (t[36], q14(0.3)), (t[61], q14(0.3))],
                    ),
                    d.fir(
                        self.scratch,
                        &[(t[10], q14(0.6)), (t[37], q14(0.3)), (t[60], q14(0.3))],
                    ),
                ]
            }
        };
        if matches!(
            self.config.network,
            ReverbNetwork::SmallRoom { .. }
                | ReverbNetwork::MediumHall { .. }
                | ReverbNetwork::LargeHall { .. }
        ) {
            result[0] = mul(left, dry) + mul(self.mix, result[0]);
            result[1] = mul(right, dry) + mul(self.mix, result[1]);
        }
        d.step();
        // Telemetry only. The wet contribution is the output minus the dry
        // term. Schroeder and Binaural use the mono dry signal.
        let dry_input = if matches!(
            self.config.network,
            ReverbNetwork::Schroeder { .. } | ReverbNetwork::Binaural { .. }
        ) {
            [mono, mono]
        } else {
            [left, right]
        };
        for (value, input) in result.iter().zip(dry_input) {
            let wet = (*value - mul(input, dry)).to_bits().wrapping_abs();
            self.wet_peak = self.wet_peak.max(wet);
        }
        result
    }
}

/// Estimates a finite tail budget from the largest modeled feedback
/// gain.
///
/// Each assumed circulation is 0.8 seconds long. The budget has the
/// circulations necessary to reach an amplitude of 1e-6, plus two seconds.
/// Fixed-point limit cycles can continue longer than this budget. If the
/// stage is disabled or the mix is zero, the function returns zero.
pub(crate) fn tail_frames(rate: SampleRate, config: ReverbConfig) -> usize {
    if !config.enabled || config.mix == 0.0 {
        return 0;
    }
    let gain = match config.network {
        ReverbNetwork::Binaural { .. } => 0.0,
        ReverbNetwork::Echo { decay, .. } => decay as f64 / 1.05,
        ReverbNetwork::Schroeder { decay } => 0.9f64.max(decay as f64 / 1.05),
        ReverbNetwork::SmallRoom { decay, .. } => 0.7f64.max(decay as f64 / 1.05),
        ReverbNetwork::MediumHall {
            diffusion, decay, ..
        } => (0.9 * diffusion as f64).max(decay as f64 / 1.05),
        ReverbNetwork::LargeHall {
            diffusion, decay, ..
        } => (0.95 * diffusion as f64).max(decay as f64 / 1.05),
    };
    let turns = if gain > 0.0 {
        (1e-6f64.ln() / gain.ln()).ceil()
    } else {
        1.0
    };
    (rate.hz() as f64 * (turns * 0.8 + 2.0)).ceil() as usize
}

/// Circular Q14 storage. Its prepared capacity limits the tap offsets.
#[derive(Debug)]
struct DelayLine {
    /// Fixed-length storage that the preparation allocates. The values use
    /// the Q14 audio scale.
    buffer: Vec<Q14>,
    /// The current frame position, always less than the buffer length. A
    /// reset does not change it.
    write: usize,
}
impl DelayLine {
    /// Allocates a silent ring that is not empty. The caller supplies the
    /// prepared capacity.
    fn new(length: usize) -> Self {
        Self {
            buffer: vec![Q14::default(); length],
            write: 0,
        }
    }
    /// Clears the samples and does not move the current write position.
    fn reset(&mut self) {
        self.buffer.fill(Q14::default());
    }
    /// Maps a delay to a storage index with a maximum of one wrap.
    ///
    /// Each offset from the caller must not be more than the buffer length.
    /// The prepared network taps obey this limit.
    #[inline]
    fn index(&self, delay: usize) -> usize {
        if self.write >= delay {
            self.write - delay
        } else {
            self.buffer.len() - (delay - self.write)
        }
    }
    /// Reads a prepared delay offset and does not advance the ring.
    #[inline]
    fn get(&self, delay: usize) -> Q14 {
        self.buffer[self.index(delay)]
    }
    /// Writes a value at a prepared delay offset and returns the value.
    #[inline]
    fn put(&mut self, value: Q14, delay: usize) -> Q14 {
        let index = self.index(delay);
        self.buffer[index] = value;
        value
    }
    /// Advances by one frame and clears the slot at the new current position.
    fn step(&mut self) {
        self.write += 1;
        if self.write == self.buffer.len() {
            self.write = 0;
        }
        self.buffer[self.write] = Q14::default();
    }
    /// Calculates one allpass section in place.
    ///
    /// The order of reads and writes is important when taps share storage.
    fn allpass(&mut self, first: usize, second: usize, coefficient: Q14) -> Q14 {
        let output = self.get(second) - mul(coefficient, self.get(first));
        self.put(output, second);
        self.put(mul(coefficient, output) + self.get(first), first);
        output
    }
    /// Adds delayed feedback into the first tap and returns the updated
    /// value.
    fn comb(&mut self, first: usize, second: usize, coefficient: Q14) -> Q14 {
        self.put(mul(coefficient, self.get(second)) + self.get(first), first)
    }
    /// Filters a tap through the persistent damping filter, then replaces
    /// the tap.
    fn lpf(&mut self, delay: usize, filter: &mut Biquad) -> Q14 {
        self.put(filter.process_sample(self.get(delay)), delay)
    }
    /// Damps the feedback tap before it is added to the comb input.
    fn comb_lpf(
        &mut self,
        first: usize,
        second: usize,
        coefficient: Q14,
        filter: &mut Biquad,
    ) -> Q14 {
        let filtered = self.lpf(second, filter);
        self.put(mul(coefficient, filtered) + self.get(first), first)
    }
    /// Adds the weighted taps in the listed order, then writes the sum to the
    /// target.
    fn fir(&mut self, target: usize, taps: &[(usize, Q14)]) -> Q14 {
        let mut accumulator = Q14::default();
        for &(delay, coefficient) in taps {
            accumulator = accumulator + mul(coefficient, self.get(delay));
        }
        self.put(accumulator, target)
    }
}

/// The prepared delay offsets and storage limits for the selected sample
/// rate.
struct ReverbLayout {
    /// Indexed network positions as frame delays from the write cursor.
    taps: [usize; 67],
    /// The ring length in frames, which includes the reserved scratch
    /// position.
    capacity: usize,
    /// The additional Binaural delay after the early-sum tap, truncated from
    /// 0.8 ms at the prepared rate. Both channels use it.
    binaural_offset: usize,
}
/// Prepares the taps for 44.1 or 48 kHz from the 44.1 kHz reference
/// distances.
///
/// The distances are truncated after the scaling. Adjacent taps stay one
/// frame apart. The base of the second network stays at frame 9,000, and the
/// total capacity stays at 17,000. These limits are correct only for the two
/// supported sample rates.
fn reverb_layout(rate: f32) -> ReverbLayout {
    let scale = |samples: f64| (rate as f64 / 44100.0 * samples) as usize;
    let mut taps = [0; 67];
    taps[..5].copy_from_slice(&[0, 1, 2, 3, 4]);
    for (index, samples) in [
        (5, 334.0),
        (7, 369.0),
        (9, 846.0),
        (10, 1000.0),
        (11, 1191.0),
        (12, 1327.0),
        (14, 1345.0),
        (15, 1541.0),
        (16, 1600.0),
        (17, 1700.0),
        (18, 1742.0),
        (19, 1768.0),
        (21, 1940.0),
        (22, 2851.0),
        (24, 3066.0),
        (25, 4000.0),
        (26, 4030.0),
        (27, 4501.0),
        (28, 4629.0),
        (30, 5623.0),
        (31, 5970.0),
        (32, 6000.0),
        (33, 6007.0),
        (34, 6543.0),
        (35, 6645.0),
        (36, 7000.0),
        (37, 7035.0),
        (38, 7120.0),
        (39, 8000.0),
    ] {
        taps[index] = scale(samples);
    }
    for (index, prior) in [(6, 5), (8, 7), (13, 12), (20, 19), (23, 22), (29, 28)] {
        taps[index] = taps[prior] + 1;
    }
    let base = 9000;
    taps[40] = base;
    taps[41] = base + 1;
    taps[42] = base + 2;
    for (index, samples) in [
        (43, 223.0),
        (45, 303.0),
        (46, 432.0),
        (47, 617.0),
        (48, 1544.0),
        (49, 1600.0),
        (50, 1710.0),
        (51, 1800.0),
        (52, 1807.0),
        (53, 1911.0),
        (54, 2205.0),
        (55, 2672.0),
        (56, 2699.0),
        (57, 3014.0),
        (58, 3335.0),
        (60, 4415.0),
        (61, 4420.0),
        (62, 4659.0),
        (63, 5293.0),
        (64, 5300.0),
        (65, 6100.0),
        (66, 6460.0),
    ] {
        taps[index] = base + scale(samples);
    }
    taps[44] = taps[43] + 1;
    taps[59] = taps[58] + 1;
    ReverbLayout {
        taps,
        capacity: 17000,
        binaural_offset: (rate as f64 * 0.8 * 0.001) as usize,
    }
}
