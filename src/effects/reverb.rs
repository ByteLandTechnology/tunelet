//! Echo and room networks with active and shadow circular delay buffers.

use crate::error::check_range;
use crate::filter::{Biquad, Design};
use crate::numeric::fixed::{Fixed, Q14};
use crate::{Error, SampleRate};

/// The step for 256-frame Q14 dry transitions across mode and bypass switches.
const DRY_FADE_STEP: Q14 = Q14::from_bits(64);

/// The echo or reverberation network. The default is [`Self::Echo`].
///
/// Two preallocated delay banks let a new mode start cleanly while the
/// previous network decays in parallel. If a third mode switch occurs before a
/// decay completes, the quieter bank starts the new mode. Schroeder gives the
/// same value on the two channels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub enum ReverbMode {
    /// A single feedback echo.
    #[default]
    Echo,
    /// Four parallel comb filters that feed two allpass filters (mono).
    Schroeder,
    /// A stereo small room.
    SmallRoom,
    /// A short binaural room.
    Binaural,
    /// A medium hall.
    MediumHall,
    /// A large hall.
    LargeHall,
}
/// The reverb after Alien, with controls for the network, the wet mix and the
/// decay.
///
/// The reverb is disabled by default. If you disable the reverb, active banks
/// get zero input across their tail budget and then clear their delay and
/// filter memories. If all banks are clear in bypass, the stage returns the
/// input directly. If the stage is enabled and the mix is zero, the active
/// network still operates. A reset clears the delay buffers and keeps the
/// memories of the active damping filters. The mix, diffusion and decay
/// controls are quantized to Q14.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub struct ReverbConfig {
    /// Enables the selected network. The default is `false`. If disabled,
    /// active tails decay with zero input and then clear.
    pub enabled: bool,
    /// The topology of the delay network. The default is
    /// [`ReverbMode::Echo`].
    pub mode: ReverbMode,
    /// The wet fraction, 0..=1.
    ///
    /// The default is 0.5. The dry weight is `1 - mix`. Schroeder and Binaural
    /// use a mono dry signal. The other modes keep the stereo dry input. If
    /// the network is enabled, a zero mix still advances it.
    pub mix: f32,
    /// The diffusion control without a unit, 0..=1.
    ///
    /// The default is 0.5. MediumHall and LargeHall scale their allpass
    /// coefficients with it. Binaural uses it as the gain of its early taps.
    /// In Echo, it sets the distance of the feedback tap. Schroeder and
    /// SmallRoom do not use it.
    pub diffusion: f32,
    /// The feedback control without a unit, 0..=1.
    ///
    /// The default is 0.6. The prepared coefficient is approximately
    /// `decay / 1.05`. It is less than unity also at the maximum. This value
    /// is not a decay time in seconds. Some paths also use fixed feedback
    /// coefficients. Binaural does not use this control.
    pub decay: f32,
    /// The low-pass cutoff in hertz for networks with damped paths, 80..=20000.
    ///
    /// The default is 4000 Hz. The preparation also limits it to 0.49 ×
    /// sample rate. A change of the cutoff keeps the sample histories of the
    /// filters.
    pub brightness_hz: f32,
}
impl Default for ReverbConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: ReverbMode::Echo,
            mix: 0.5,
            diffusion: 0.5,
            decay: 0.6,
            brightness_hz: 4000.0,
        }
    }
}
impl ReverbConfig {
    /// Validates all fields, also when the stage is disabled or the selected
    /// mode does not use a field.
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
        check_range("reverb.diffusion", self.diffusion, 0.0, 1.0)?;
        check_range("reverb.decay", self.decay, 0.0, 1.0)?;
        check_range("reverb.brightness_hz", self.brightness_hz, 80.0, 20000.0)
    }
}

/// One preallocated 17,000-sample delay network with its own damping filters.
#[derive(Debug)]
struct ReverbBank {
    /// The configuration snapshot that drives this bank.
    config: ReverbConfig,
    /// The circular storage for this bank.
    delay: DelayLine,
    /// The first damping filter of this bank.
    lpf1: Biquad,
    /// The second damping filter of this bank.
    lpf2: Biquad,
    /// The Q14 wet contribution of this bank.
    mix: Q14,
    /// The Q14 control for diffusion or echo distance.
    diffuse: Q14,
    /// The Q14 feedback coefficient, scaled by 100/105.
    decay: Q14,
    /// True while this bank receives input or decays a tail.
    active: bool,
    /// The zero-input tail frames that are left, or zero for live input.
    decay_remaining: usize,
    /// The smoothed absolute Q14 wet level, which selects the quieter bank.
    level: i32,
}

impl ReverbBank {
    /// Allocates one silent bank of `capacity` frames.
    fn new(capacity: usize, config: ReverbConfig) -> Self {
        Self {
            config,
            delay: DelayLine::new(capacity),
            lpf1: Biquad::new(),
            lpf2: Biquad::new(),
            mix: Q14::default(),
            diffuse: Q14::default(),
            decay: Q14::default(),
            active: false,
            decay_remaining: 0,
            level: 0,
        }
    }

    /// Quantizes the coefficients and filter design of `config`, and keeps the
    /// filter memories.
    fn apply_coefficients(&mut self, rate: SampleRate, config: ReverbConfig) {
        self.config = config;
        self.mix = Q14::from_f64((config.mix * 100.0) as f64 / 100.0);
        self.diffuse = Q14::from_f64((config.diffusion * 100.0) as f64 / 100.0);
        self.decay = Q14::from_f64((config.decay * 100.0) as f64 / 105.0);
        let brightness = config.brightness_hz.min((rate.hz() as f64 * 0.49) as f32);
        let design = Design::low_pass_one_pole(rate.hz(), brightness);
        self.lpf1.set_design(design);
        self.lpf2.set_design(design);
    }

    /// Clears the delay buffer, the two damping filters and the level state.
    fn clear(&mut self) {
        self.delay.reset();
        self.lpf1.reset();
        self.lpf2.reset();
        self.active = false;
        self.decay_remaining = 0;
        self.level = 0;
    }

    /// Starts zero-input decay across the tail budget, or clears immediately
    /// if the budget is zero.
    fn start_decay(&mut self, rate: SampleRate) {
        if !self.active {
            return;
        }
        if self.decay_remaining > 0 {
            return;
        }
        let mut enabled_config = self.config;
        enabled_config.enabled = true;
        let budget = tail_frames(rate, enabled_config);
        if budget == 0 {
            self.clear();
        } else {
            self.decay_remaining = budget;
        }
    }

    /// Advances this bank by one frame and returns its stereo Q14 wet signal.
    fn process_wet(
        &mut self,
        frame: [Q14; 2],
        taps: &[usize; 67],
        binaural_offset: usize,
        scratch: usize,
    ) -> [Q14; 2] {
        let [left, right] = frame;
        let mono = (left + right).div::<0, 14>(TWO);
        let t = *taps;
        let d = &mut self.delay;
        let wet = match self.config.mode {
            ReverbMode::Echo => {
                d.put(mono, t[0]);
                let echo: Fixed<0> = Fixed::<0>::from_bits(t[39] as i32).mul(self.diffuse);
                d.comb(t[0], echo.to_bits() as usize, self.decay);
                [
                    mul(self.mix, d.get(t[25]) - d.get(t[32])),
                    -mul(self.mix, d.get(t[26]) - d.get(t[31])),
                ]
            }
            ReverbMode::Schroeder => {
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
                let value = mul(self.mix, d.get(t[45]));
                [value, value]
            }
            ReverbMode::SmallRoom => {
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
                let raw_left = d.fir(
                    scratch,
                    &[(t[16], q14(0.2)), (t[32], q14(0.2)), (t[51], q14(0.2))],
                );
                let raw_right = d.fir(
                    scratch,
                    &[(t[49], q14(0.2)), (t[65], q14(0.2)), (t[17], q14(0.2))],
                );
                [mul(self.mix, raw_left), mul(self.mix, raw_right)]
            }
            ReverbMode::Binaural => {
                let seed = t[10];
                let tap = t[25];
                let filter = tap + binaural_offset;
                d.put(left, t[0]);
                d.put(right, t[40]);
                let a = mul(
                    self.diffuse >> 1,
                    d.get(t[0] + seed) + d.get(t[11] + seed) + d.get(t[21] + seed),
                );
                d.put(a, tap);
                d.lpf(filter, &mut self.lpf1);
                let wet_left = mul(self.mix, d.get(t[40] + filter) + a);
                let b = mul(
                    self.diffuse >> 1,
                    d.get(t[47] + seed) + d.get(t[48] + seed) + d.get(t[54] + seed),
                );
                d.put(b, t[40] + tap);
                d.lpf(t[40] + filter, &mut self.lpf2);
                let wet_right = mul(self.mix, d.get(filter) + b);
                [wet_left, wet_right]
            }
            ReverbMode::MediumHall => {
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
                let raw_left = d.fir(
                    scratch,
                    &[
                        (t[16], q14(-0.25)),
                        (t[33], q14(0.25)),
                        (t[51], q14(-0.25)),
                        (t[55], q14(0.25)),
                    ],
                );
                let raw_right = d.fir(
                    scratch,
                    &[
                        (t[16], q14(0.25)),
                        (t[32], q14(0.25)),
                        (t[52], q14(0.25)),
                        (t[56], q14(0.2)),
                    ],
                );
                [mul(self.mix, raw_left), mul(self.mix, raw_right)]
            }
            ReverbMode::LargeHall => {
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
                let raw_left = d.fir(
                    scratch,
                    &[(t[10], q14(0.6)), (t[36], q14(0.3)), (t[61], q14(0.3))],
                );
                let raw_right = d.fir(
                    scratch,
                    &[(t[10], q14(0.6)), (t[37], q14(0.3)), (t[60], q14(0.3))],
                );
                [mul(self.mix, raw_left), mul(self.mix, raw_right)]
            }
        };
        d.step();
        let peak = wet[0]
            .to_bits()
            .wrapping_abs()
            .max(wet[1].to_bits().wrapping_abs());
        self.level = (self.level - (self.level >> 8)).max(peak);
        wet
    }
}

/// Dual Q14 delay networks with independent damping filter pairs.
#[derive(Debug)]
pub(crate) struct Reverb {
    /// The prepared sample rate for the cutoff and tap calculations.
    rate: SampleRate,
    /// The validated controls and the selected network.
    config: ReverbConfig,
    /// Two preallocated 17,000-sample delay and filter banks.
    banks: [ReverbBank; 2],
    /// The index (`0` or `1`) of the bank for the active mode.
    active_bank: usize,
    /// 67 prepared frame offsets into each ring.
    taps: [usize; 67],
    /// The Binaural delay of approximately 0.8 ms between the tap that gets
    /// the early sum and the filtered tap. Both channels use it.
    binaural_offset: usize,
    /// The reserved last ring offset. The output sums go to this slot, and
    /// the next ring step clears it.
    scratch: usize,
    /// The current Q14 dry gain, which ramps over 256 frames across switches.
    dry_gain: Q14,
    /// The current Q14 weight of the mono dry input (`Schroeder` and
    /// `Binaural`).
    mono_dry_mix: Q14,
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

/// Returns `true` if `mode` uses a mono dry signal.
#[inline]
fn uses_mono_dry(mode: ReverbMode) -> bool {
    matches!(mode, ReverbMode::Schroeder | ReverbMode::Binaural)
}

impl Reverb {
    /// Allocates both 17,000-sample delay banks and prepares the filters from
    /// validated controls.
    pub(crate) fn new(rate: SampleRate, config: ReverbConfig) -> Self {
        let layout = reverb_layout(rate.hz());
        let mut banks = [
            ReverbBank::new(layout.capacity, config),
            ReverbBank::new(layout.capacity, config),
        ];
        banks[0].apply_coefficients(rate, config);
        banks[1].apply_coefficients(rate, config);
        banks[0].active = config.enabled;
        let dry_gain = if config.enabled {
            Q14::ONE - banks[0].mix
        } else {
            Q14::ONE
        };
        let mono_dry_mix = if config.enabled && uses_mono_dry(config.mode) {
            Q14::ONE
        } else {
            Q14::ZERO
        };
        Self {
            rate,
            config,
            banks,
            active_bank: 0,
            taps: layout.taps,
            binaural_offset: layout.binaural_offset,
            scratch: layout.capacity - 1,
            dry_gain,
            mono_dry_mix,
            wet_peak: 0,
        }
    }
    /// Applies `config`, starts tail decay on old banks, and prepares the idle
    /// or quieter bank.
    ///
    /// The f32 multiplication by 100 occurs before the f64 division and the
    /// quantization. If you combine these expressions, the quantized
    /// coefficients can change.
    pub(crate) fn set_config(&mut self, config: ReverbConfig) {
        let was_enabled = self.config.enabled;
        let old_mode = self.config.mode;
        self.config = config;
        if !config.enabled {
            if was_enabled {
                for bank in &mut self.banks {
                    bank.start_decay(self.rate);
                }
            }
            return;
        }
        if !was_enabled {
            let current = self.active_bank;
            if !self.banks[current].active {
                self.banks[current].clear();
                self.banks[current].apply_coefficients(self.rate, config);
                self.banks[current].active = true;
            } else {
                self.banks[current].start_decay(self.rate);
                let other = 1 - current;
                let target = if !self.banks[other].active
                    || self.banks[other].level <= self.banks[current].level
                {
                    other
                } else {
                    current
                };
                self.banks[target].clear();
                self.banks[target].apply_coefficients(self.rate, config);
                self.banks[target].active = true;
                self.active_bank = target;
            }
        } else if config.mode != old_mode {
            let current = self.active_bank;
            self.banks[current].start_decay(self.rate);
            let other = 1 - current;
            let target = if !self.banks[other].active
                || self.banks[other].level <= self.banks[current].level
            {
                other
            } else {
                current
            };
            self.banks[target].clear();
            self.banks[target].apply_coefficients(self.rate, config);
            self.banks[target].active = true;
            self.active_bank = target;
        } else {
            let current = self.active_bank;
            self.banks[current].apply_coefficients(self.rate, config);
            self.banks[current].active = true;
            self.banks[current].decay_remaining = 0;
        }
    }
    /// Clears the delay contents of both banks and keeps the write cursor and
    /// damping memories of the active bank.
    pub(crate) fn reset(&mut self) {
        let current = self.active_bank;
        let other = 1 - current;
        self.banks[other].clear();
        self.banks[current].delay.reset();
        self.banks[current].decay_remaining = 0;
        self.banks[current].level = 0;
        self.banks[current].active = self.config.enabled;
        self.dry_gain = if self.config.enabled {
            Q14::ONE - self.banks[current].mix
        } else {
            Q14::ONE
        };
        self.mono_dry_mix = if self.config.enabled && uses_mono_dry(self.config.mode) {
            Q14::ONE
        } else {
            Q14::ZERO
        };
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
    /// Advances the active reverb banks by one frame.
    ///
    /// If the reverb is disabled and all tails are clear, no delay or filter
    /// history advances. The internal mono averages use integer division
    /// toward zero.
    pub(crate) fn process_sample(&mut self, frame: [Q14; 2]) -> [Q14; 2] {
        let target_dry = if self.config.enabled {
            Q14::ONE - self.banks[self.active_bank].mix
        } else {
            Q14::ONE
        };
        if self.dry_gain < target_dry {
            self.dry_gain = (self.dry_gain + DRY_FADE_STEP).min(target_dry);
        } else if self.dry_gain > target_dry {
            self.dry_gain = (self.dry_gain - DRY_FADE_STEP).max(target_dry);
        }
        let target_mono = if self.config.enabled && uses_mono_dry(self.config.mode) {
            Q14::ONE
        } else {
            Q14::ZERO
        };
        if self.mono_dry_mix < target_mono {
            self.mono_dry_mix = (self.mono_dry_mix + DRY_FADE_STEP).min(target_mono);
        } else if self.mono_dry_mix > target_mono {
            self.mono_dry_mix = (self.mono_dry_mix - DRY_FADE_STEP).max(target_mono);
        }
        if !self.config.enabled
            && !self.banks[0].active
            && !self.banks[1].active
            && self.dry_gain == Q14::ONE
            && self.mono_dry_mix == Q14::ZERO
        {
            return frame;
        }
        let [left, right] = frame;
        let mono = (left + right).div::<0, 14>(TWO);
        let dry_input = if self.mono_dry_mix == Q14::ZERO {
            [left, right]
        } else if self.mono_dry_mix == Q14::ONE {
            [mono, mono]
        } else {
            let stereo_weight = Q14::ONE - self.mono_dry_mix;
            [
                (left.mul::<14, 28>(stereo_weight) + mono.mul::<14, 28>(self.mono_dry_mix))
                    .rescale(),
                (right.mul::<14, 28>(stereo_weight) + mono.mul::<14, 28>(self.mono_dry_mix))
                    .rescale(),
            ]
        };
        let dry_part = if self.dry_gain == Q14::ONE {
            dry_input
        } else {
            [
                mul(dry_input[0], self.dry_gain),
                mul(dry_input[1], self.dry_gain),
            ]
        };
        let mut wet_sum = [Q14::ZERO; 2];
        for index in 0..2 {
            if !self.banks[index].active {
                continue;
            }
            let bank_input = if self.config.enabled
                && index == self.active_bank
                && self.banks[index].decay_remaining == 0
            {
                frame
            } else {
                [Q14::ZERO; 2]
            };
            let wet = self.banks[index].process_wet(
                bank_input,
                &self.taps,
                self.binaural_offset,
                self.scratch,
            );
            wet_sum[0] = wet_sum[0] + wet[0];
            wet_sum[1] = wet_sum[1] + wet[1];
            if self.banks[index].decay_remaining > 0 {
                self.banks[index].decay_remaining -= 1;
                if self.banks[index].decay_remaining == 0 {
                    self.banks[index].clear();
                }
            }
        }
        if self.config.enabled || self.banks[0].active || self.banks[1].active {
            for sample in wet_sum {
                self.wet_peak = self.wet_peak.max(sample.to_bits().wrapping_abs());
            }
        }
        [dry_part[0] + wet_sum[0], dry_part[1] + wet_sum[1]]
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
    let decay = config.decay as f64 / 1.05;
    let diffuse = config.diffusion as f64;
    let gain = match config.mode {
        ReverbMode::Binaural => 0.0,
        ReverbMode::Echo => decay,
        ReverbMode::Schroeder => 0.9f64.max(decay),
        ReverbMode::SmallRoom => 0.7f64.max(decay),
        ReverbMode::MediumHall => (0.9 * diffuse).max(decay),
        ReverbMode::LargeHall => (0.95 * diffuse).max(decay),
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
