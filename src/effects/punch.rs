//! Transient enhancement with an adaptive drive and an arctangent output
//! ceiling.

use crate::effects::SlidingPeak;
use crate::error::check_range;
use crate::numeric::fixed::{Q14, Q20, Q28};
use crate::numeric::{
    coefficient_to_exp_value, db_to_gain, envelope_decay, AtanQ14, NumericContext,
};
use crate::{Error, SampleRate};

/// The base arctangent drive, approximately π/2 in Q20.
const BASE_DRIVE: Q20 = Q20::from_bits(1_647_099);
/// The 2/π factor in Q14 that normalizes the arctangent output.
const TWO_OVER_PI: Q14 = Q14::from_bits(10430);

/// The input gain and the transient enhancement with arctangent saturation.
///
/// Punch comes before Warm and before pitch detection. It tracks the peak of a
/// 100-frame lookahead window on each processed frame. Two envelope followers
/// set the drive from this peak. When Punch is disabled, it does not process
/// audio, and its envelope and window state do not change.
///
/// Construction, configuration changes and resets update the cached drive
/// immediately from the configured gain.
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
    /// The default is 0 dB. It scales the saturation drive directly.
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
    /// The default is 10 ms. Both followers update on each processed frame.
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
    /// The prepared rate, which converts the envelope times to sample counts.
    rate: SampleRate,
    /// The 100-frame lookahead peak window.
    window: SlidingPeak,
    /// The adaptive Q28 peak envelope. The impact control sets its release.
    slow: Q28,
    /// The second Q28 peak envelope, with a fixed release half-life of 5
    /// seconds.
    fast: Q28,
    /// The Q28 peak envelope of the delayed input, for stage-gain telemetry.
    envelope: Q28,
    /// The Q28 release coefficient of the adaptive follower.
    slow_decay: Q28,
    /// The Q28 release coefficient of the second follower.
    fast_decay: Q28,
    /// The shared Q28 coefficient when a follower moves up.
    attack_decay: Q28,
    /// The Q28 release coefficient of `envelope` (200 ms half-life).
    envelope_decay: Q28,
    /// The configured linear input gain in Q20.
    gain: Q20,
    /// The linear output ceiling in Q28. It normalizes the saturator curve.
    ceiling_gain: Q28,
    /// The cached Q20 drive: base drive × envelope ratio × input gain.
    drive: Q20,
}
impl Punch {
    /// Prepares the coefficients and resets all followers from validated
    /// settings.
    pub(crate) fn new(rate: SampleRate, config: PunchConfig) -> Self {
        let mut result = Self {
            rate,
            window: SlidingPeak::new(),
            slow: Q28::ZERO,
            fast: Q28::ZERO,
            envelope: Q28::ZERO,
            slow_decay: Q28::ZERO,
            fast_decay: Q28::ZERO,
            attack_decay: Q28::ZERO,
            envelope_decay: Q28::from_f64(envelope_decay(200.0, rate.hz() as f64, 1)),
            gain: Q20::ONE,
            ceiling_gain: Q28::ONE,
            drive: BASE_DRIVE,
        };
        result.set_config(config);
        result.reset();
        result
    }
    /// Returns the effective stage gain across the delayed envelope, as f32.
    ///
    /// At zero envelope, the method returns the linear small-signal gain. The
    /// stage state does not change.
    pub(crate) fn stage_gain(&self) -> f32 {
        let envelope = self.envelope.to_bits();
        if envelope <= 0 {
            return (self.drive.to_bits() as f64 / BASE_DRIVE.to_bits() as f64) as f32;
        }
        let ceiling = i64::from(self.ceiling_gain.to_bits()).max(1);
        let driven = ((i64::from(envelope) * i64::from(self.drive.to_bits())) / ceiling) / 64;
        let scaled = driven.clamp(-0x4000_0000, 0x4000_0000) as i32;
        let angle = AtanQ14::default().process(scaled, 0);
        let unit = i64::from(TWO_OVER_PI.to_bits()) * i64::from(angle);
        let output_q28 = (unit * ceiling) as f64 / f64::from(1u32 << 28);
        (output_q28 / f64::from(envelope)) as f32
    }
    /// Calculates the quantized controls again, keeps the envelopes and
    /// updates the cached drive.
    ///
    /// The processor controls the bypass. This stage never reads an enabled
    /// flag.
    pub(crate) fn set_config(&mut self, config: PunchConfig) {
        let impact = coefficient_to_exp_value(1.0, 250.0, 5000.0, 1.0 - config.impact);
        let decay = |milliseconds: f64| {
            Q28::from_f64(envelope_decay(milliseconds, self.rate.hz() as f64, 1))
        };
        self.slow_decay = decay(impact as f64);
        self.fast_decay = decay(5000.0);
        self.attack_decay = decay(config.attack_ms as f64);
        self.gain = Q20::from_f64(db_to_gain(config.gain_db as f64));
        self.ceiling_gain = Q28::from_f64(db_to_gain(config.ceiling_db as f64));
        self.update_drive();
    }
    /// Clears the peak window and all envelopes, then calculates the drive
    /// from the base drive and the input gain.
    pub(crate) fn reset(&mut self) {
        self.window.reset();
        self.slow = Q28::ZERO;
        self.fast = Q28::ZERO;
        self.envelope = Q28::ZERO;
        self.drive = BASE_DRIVE.mul(self.gain);
    }

    /// Moves `current` toward `target` with a one-pole filter.
    ///
    /// The filter uses `attack` if `target` is higher than `current`.
    /// Otherwise, it uses `release`.
    fn follow(current: Q28, target: Q28, attack: Q28, release: Q28) -> Q28 {
        let decay = if target > current { attack } else { release };
        target + decay.mul(current - target)
    }

    /// Updates `drive` from the current follower ratio and `gain`.
    fn update_drive(&mut self) {
        let smooth: Q20 = if self.slow > Q28::ZERO {
            let limit = i64::from(self.slow.to_bits()) << 4;
            if i64::from(self.fast.to_bits()) <= limit {
                self.fast.div(self.slow)
            } else {
                Q20::from_bits(16 << 20)
            }
        } else {
            Q20::ONE
        };
        self.drive = BASE_DRIVE.mul::<20, 20>(smooth).mul(self.gain);
    }

    /// Applies ceiling-normalized arctangent saturation and symmetric PCM
    /// quantization to one Q28 sample.
    fn saturate(sample: Q28, drive: Q20, ceiling_gain: Q28, atan: &mut AtanQ14) -> i16 {
        let ceiling = i64::from(ceiling_gain.to_bits()).max(1);
        let driven = ((i64::from(sample.to_bits()) * i64::from(drive.to_bits())) / ceiling) / 64;
        let scaled = driven.clamp(-0x4000_0000, 0x4000_0000) as i32;
        let angle = atan.process(scaled, 0);
        let unit = i64::from(TWO_OVER_PI.to_bits()) * i64::from(angle);
        let product = unit * ceiling;
        let half = 1_i64 << 40;
        let rounded = if product >= 0 {
            (product + half) / (1_i64 << 41)
        } else {
            (product - half) / (1_i64 << 41)
        };
        rounded.clamp(-32767, 32767) as i16
    }

    /// Updates the lookahead followers on each call and saturates both the
    /// delayed sample and the lookahead sample.
    pub(crate) fn process_sample(
        &mut self,
        input: i16,
        lookahead: i16,
        context: &mut NumericContext,
    ) -> (i16, i16) {
        let ahead = Q28::from_bits(i32::from(lookahead) << 13);
        let peak = self.window.push(ahead.abs());
        self.slow = Self::follow(self.slow, peak, self.attack_decay, self.slow_decay);
        self.fast = Self::follow(self.fast, peak, self.attack_decay, self.fast_decay);
        self.update_drive();

        let sample = Q28::from_bits(i32::from(input) << 13);
        let delayed_abs = sample.abs();
        self.envelope = if delayed_abs > self.envelope {
            delayed_abs
        } else {
            delayed_abs + self.envelope_decay.mul(self.envelope - delayed_abs)
        };

        let saturated_ahead =
            Self::saturate(ahead, self.drive, self.ceiling_gain, &mut context.atan_q14);
        let saturated =
            Self::saturate(sample, self.drive, self.ceiling_gain, &mut context.atan_q14);
        (saturated, saturated_ahead)
    }
}
