//! Transient enhancement with an adaptive drive and an arctangent output
//! ceiling.

use crate::error::check_range;
use crate::numeric::fixed::{Q14, Q16, Q28};
use crate::numeric::{coefficient_to_exp_value, db_to_gain, envelope_decay, NumericContext};
use crate::{Error, SampleRate};

/// The update interval of the peak follower, in frames.
const WINDOW_FRAMES: usize = 100;
/// The base arctangent drive, approximately π/2 in Q16.
const BASE_DRIVE: Q16 = Q16::from_bits(102_944);
/// The 2/π factor in Q14 that normalizes the arctangent output.
const TWO_OVER_PI: Q14 = Q14::from_bits(10430);

/// The input gain and the transient enhancement with arctangent saturation.
///
/// Punch comes before Warm and before pitch detection. It finds the peak of
/// each window of 100 processed frames. Two envelope followers set the drive
/// from these peaks. When Punch is disabled, it does not process audio, and
/// its envelope and window state do not change.
///
/// A gain change updates the cached drive at the next boundary of the peak
/// window. A reset calculates that drive again immediately from the
/// configured gain.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub struct PunchConfig {
    /// Enables this input stage. The default is `false`.
    pub enabled: bool,
    /// The input gain in decibels, -96..=12.
    ///
    /// The default is 0 dB. The saturation drive from this gain changes at the
    /// next 100-frame peak boundary.
    pub gain_db: f32,
    /// The transient emphasis without a unit, 0..=1.
    ///
    /// The default is 0.5. It sets the release half-life of the adaptive
    /// follower: 5000 ms at 0, 250 ms at 0.5 and 1 ms at 1. The attack uses
    /// `attack_ms`.
    pub impact: f32,
    /// The output ceiling of the saturation in dBFS, -96..=0.
    ///
    /// The default is -3 dBFS. Later stages and the final gain can make the
    /// final output higher than this level.
    pub ceiling_db: f32,
    /// The attack half-life of the peak follower in milliseconds, 0.01..=5000.
    ///
    /// The default is 10 ms. The coefficients include the effect of the
    /// 100-frame update interval.
    pub attack_ms: f32,
}
impl Default for PunchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            gain_db: 0.0,
            impact: 0.5,
            ceiling_db: -3.0,
            attack_ms: 10.0,
        }
    }
}
impl PunchConfig {
    /// Validates all fields, also when Punch is disabled.
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
        check_range("punch.gain_db", self.gain_db, -96.0, 12.0)?;
        check_range("punch.impact", self.impact, 0.0, 1.0)?;
        check_range("punch.ceiling_db", self.ceiling_db, -96.0, 0.0)?;
        check_range("punch.attack_ms", self.attack_ms, 0.01, 5000.0)
    }
}

/// Two peak followers that control an arctangent saturator, with all buffers
/// inline.
#[derive(Debug)]
pub(crate) struct Punch {
    /// The prepared rate, which converts the envelope times to sample windows.
    rate: SampleRate,
    /// The adaptive Q14 peak envelope. The impact control sets its release.
    slow: Q14,
    /// The second Q14 peak envelope, with a fixed release half-life of 5
    /// seconds.
    fast: Q14,
    /// The number of samples in the current 100-frame peak window.
    counter: usize,
    /// The largest absolute Q14 input in the current window.
    peak: Q14,
    /// The Q14 release coefficient of the adaptive follower.
    slow_decay: Q14,
    /// The Q14 release coefficient of the second follower.
    fast_decay: Q14,
    /// The shared Q14 coefficient when a follower moves up.
    attack_decay: Q14,
    /// The configured linear input gain in Q16.
    gain: Q16,
    /// The linear output ceiling in Q28. It scales the saturator input and
    /// output.
    ceiling_gain: Q28,
    /// The current Q16 drive: base drive × envelope ratio × input gain.
    drive: Q16,
    /// The per-sample Q16 step of the drive in this window.
    drive_step: Q16,
}
impl Punch {
    /// Prepares the coefficients and resets both followers from validated
    /// settings.
    pub(crate) fn new(rate: SampleRate, config: PunchConfig) -> Self {
        let mut result = Self {
            rate,
            slow: Q14::default(),
            fast: Q14::default(),
            counter: 0,
            peak: Q14::default(),
            slow_decay: Q14::default(),
            fast_decay: Q14::default(),
            attack_decay: Q14::default(),
            gain: Q16::ONE,
            ceiling_gain: Q28::ONE,
            drive: BASE_DRIVE,
            drive_step: Q16::default(),
        };
        result.set_config(config);
        result.reset();
        result
    }
    /// Returns the effective linear gain of the stage at the current envelope
    /// level, as f32.
    ///
    /// If the adaptive envelope is zero, the method returns the linear drive
    /// ratio. The stage state does not change.
    pub(crate) fn stage_gain(&self) -> f32 {
        if self.slow <= Q14::ZERO {
            return (self.drive.to_bits() as f64 / BASE_DRIVE.to_bits() as f64) as f32;
        }
        let ceiling = self.ceiling_gain.to_bits() as f64 / f64::from(1u32 << 28);
        if ceiling <= 0.0 {
            return 0.0;
        }
        let drive = self.drive.to_bits() as f64 / 65536.0;
        let envelope = self.slow.to_bits() as f64 / 16384.0;
        let output = ceiling * std::f64::consts::FRAC_2_PI * (drive * envelope / ceiling).atan();
        (output / envelope) as f32
    }
    /// Calculates the quantized controls again, and keeps the envelopes and
    /// the current drive ramp.
    /// The processor controls the bypass. This stage never reads an enabled
    /// flag.
    pub(crate) fn set_config(&mut self, config: PunchConfig) {
        let impact = coefficient_to_exp_value(1.0, 250.0, 5000.0, 1.0 - config.impact);
        let decay = |milliseconds: f64| {
            Q14::from_f64(envelope_decay(
                milliseconds,
                self.rate.hz() as f64,
                WINDOW_FRAMES,
            ))
        };
        self.slow_decay = decay(impact as f64);
        self.fast_decay = decay(5000.0);
        self.attack_decay = decay(config.attack_ms as f64);
        self.gain = Q16::from_f64(db_to_gain(config.gain_db as f64));
        self.ceiling_gain = Q28::from_f64(db_to_gain(config.ceiling_db as f64));
    }
    /// Clears both envelopes and the current window, then calculates the drive
    /// again from the base drive and the input gain.
    pub(crate) fn reset(&mut self) {
        self.slow = Q14::default();
        self.fast = Q14::default();
        self.counter = 0;
        self.peak = Q14::default();
        self.drive = BASE_DRIVE.mul(self.gain);
        self.drive_step = Q16::default();
    }

    /// Moves `current` toward `target` with a one-pole filter.
    ///
    /// The filter uses `attack` if `target` is higher than `current`.
    /// Otherwise, it uses `release`.
    fn follow(current: Q14, target: Q14, attack: Q14, release: Q14) -> Q14 {
        let decay = if target > current { attack } else { release };
        target + decay.mul(current - target)
    }

    /// Tracks the current peak, ramps the drive across each 100-frame window
    /// and saturates one PCM sample.
    /// The envelope ratio has a limit of 16 before multiplication by the input
    /// gain. While the adaptive envelope is zero, the ratio is 1. The inverse
    /// ceiling scales the arctangent input, and the ceiling scales the output.
    pub(crate) fn process_sample(&mut self, input: i16, context: &mut NumericContext) -> i16 {
        let sample = Q14::from_bits(i32::from(input) >> 1);
        self.peak = self.peak.max(sample.abs());
        self.counter += 1;
        if self.counter == WINDOW_FRAMES {
            self.counter = 0;
            self.slow = Self::follow(self.slow, self.peak, self.attack_decay, self.slow_decay);
            self.fast = Self::follow(self.fast, self.peak, self.attack_decay, self.fast_decay);
            self.peak = Q14::default();
            let smooth: Q16 = if self.slow > Q14::default() {
                self.fast
                    .div::<14, 16>(self.slow)
                    .min(Q16::from_bits(16 << 16))
            } else {
                Q16::ONE
            };
            let target: Q16 = BASE_DRIVE.mul::<16, 16>(smooth).mul(self.gain);
            self.drive_step =
                Q16::from_bits((target.to_bits() - self.drive.to_bits()) / WINDOW_FRAMES as i32);
        }
        self.drive = self.drive + self.drive_step;
        let driven = ((i64::from(sample.to_bits()) * i64::from(self.drive.to_bits())) << 12)
            / i64::from(self.ceiling_gain.to_bits());
        let ratio = driven.clamp(-i64::from(i32::MAX), i64::from(i32::MAX)) as i32;
        let angle = context.atan_q14.process(ratio, 0);
        let scaled = i64::from(TWO_OVER_PI.to_bits())
            * ((i64::from(self.ceiling_gain.to_bits()) * i64::from(angle)) / (1 << 14));
        let rounded = if scaled >= 0 {
            (scaled + (1 << 27)) / (1 << 28)
        } else {
            (scaled - (1 << 27)) / (1 << 28)
        };
        ((rounded as i32) << 1).clamp(-32767, 32767) as i16
    }
}
