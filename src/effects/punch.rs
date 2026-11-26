//! Transient enhancement with an adaptive drive and an arctangent output
//! ceiling.

use crate::error::check_range;
use crate::numeric::fixed::{Fixed, Q14, Q22, Q24, Q28, Q30};
use crate::numeric::{coefficient_to_exp_value, db_to_gain, envelope_decay, NumericContext};
use crate::{Error, SampleRate};

/// The duration of the input peak hold, in milliseconds.
const HOLD_MS: usize = 6;
/// The largest peak-hold length across supported sample rates (6 ms at 48 kHz).
const MAX_HOLD_FRAMES: usize = 288;
/// The release half-life of the second envelope follower, in milliseconds.
const FAST_RELEASE_MS: f64 = 5000.0;
/// The half-life of the per-sample drive filter, in milliseconds.
const DRIVE_SMOOTH_MS: f64 = 1.0;
/// The base arctangent drive, π/2 in Q24.
const BASE_DRIVE: Q24 = Q24::from_bits(26_353_589);
/// The maximum ratio between the two followers, 16.0 in Q24.
const MAX_RATIO: Q24 = Q24::from_bits(16 << 24);
/// The 2/π factor in Q14 that normalizes the arctangent output.
const TWO_OVER_PI: Q14 = Q14::from_bits(10430);
/// The clamp bound in Q14 above the last tangent table entry (3,129,084).
const MAX_TANGENT_Q14: i64 = 4_000_000;

/// The input gain and the transient enhancement with arctangent saturation.
///
/// Punch comes before Warm and before pitch detection. A 6 ms peak hold feeds
/// two per-sample envelope followers that set the drive. The stage scales the
/// saturator input by the inverse ceiling gain so quiet signals stay near
/// unity. When Punch is disabled, it does not process audio, and its hold and
/// envelope state do not change.
///
/// Construction and reset initialize the drive from `gain_db`. On each
/// processed frame, the drive moves smoothly toward the target value.
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
    /// The default is 0 dB. Construction and reset set the initial drive from
    /// this gain, and later changes update it smoothly on each frame.
    pub gain_db: f32,
    /// The transient emphasis without a unit, 0..=1.
    ///
    /// The default is 0.5. It sets the release half-life of the adaptive
    /// follower: 5000 ms at 0, 250 ms at 0.5 and 1 ms at 1. The attack uses
    /// `attack_ms`.
    pub impact: f32,
    /// The output ceiling of the saturation in dBFS, -96..=0.
    ///
    /// The default is -3 dBFS. The stage normalizes the saturator input by this
    /// ceiling, then scales the saturated output back to `ceiling_db`. Later
    /// stages and the final gain can make the final output higher than this
    /// level.
    pub ceiling_db: f32,
    /// The attack half-life of the peak followers in milliseconds, 0.01..=5000.
    ///
    /// The default is 10 ms. Each enabled frame updates both followers with
    /// per-sample coefficients.
    pub attack_ms: f32,
    /// The wet fraction, 0..=1.
    ///
    /// The default is 1. The dry weight is `1 - mix` in Q14. If Punch is
    /// enabled, a zero mix still advances the peak hold, the envelopes and the
    /// drive. In JavaScript, this field is optional and an absent or
    /// `undefined` value gives 1.
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        serde(
            default = "crate::wasm::default_mix",
            deserialize_with = "crate::wasm::deserialize_mix"
        ),
        tsify(optional, type = "number | undefined")
    )]
    pub mix: f32,
}
impl Default for PunchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            gain_db: 0.0,
            impact: 0.5,
            ceiling_db: -3.0,
            attack_ms: 10.0,
            mix: 1.0,
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
        check_range("punch.attack_ms", self.attack_ms, 0.01, 5000.0)?;
        check_range("punch.mix", self.mix, 0.0, 1.0)
    }
}

/// Two peak followers that control an arctangent saturator, with all buffers
/// inline.
#[derive(Debug)]
pub(crate) struct Punch {
    /// The prepared rate, which converts the envelope times to sample counts.
    rate: SampleRate,
    /// The number of frames in the 6 ms peak-hold window at `rate`.
    hold_frames: usize,
    /// The write offset into the leaf half of `hold_tree`.
    hold_cursor: usize,
    /// A complete binary max-tree over `hold_frames` Q28 input magnitudes.
    hold_tree: [Q28; MAX_HOLD_FRAMES * 2],
    /// The adaptive Q28 peak envelope. The impact control sets its release.
    slow: Q28,
    /// The second Q28 peak envelope, with a fixed release half-life of 5
    /// seconds.
    fast: Q28,
    /// The Q30 release coefficient of the adaptive follower.
    slow_decay: Q30,
    /// The Q30 release coefficient of the second follower.
    fast_decay: Q30,
    /// The shared Q30 coefficient when a follower moves up.
    attack_decay: Q30,
    /// The Q30 coefficient that smooths the drive on each frame.
    drive_decay: Q30,
    /// The configured linear input gain in Q28.
    gain: Q28,
    /// The linear output ceiling in Q28, nonzero across -96..=0 dBFS.
    ceiling_gain: Q28,
    /// The Q14 wet weight. The dry weight is `1 - mix`.
    mix: Q14,
    /// The smoothed Q24 drive: base drive × envelope ratio × input gain.
    drive: Q24,
}
impl Punch {
    /// Prepares the coefficients and resets both followers from validated
    /// settings.
    pub(crate) fn new(rate: SampleRate, config: PunchConfig) -> Self {
        let hold_frames = (rate.as_u32() as usize * HOLD_MS).div_ceil(1000);
        let drive_decay = Q30::from_f64(envelope_decay(DRIVE_SMOOTH_MS, rate.hz() as f64, 1));
        let mut result = Self {
            rate,
            hold_frames,
            hold_cursor: 0,
            hold_tree: [Q28::ZERO; MAX_HOLD_FRAMES * 2],
            slow: Q28::ZERO,
            fast: Q28::ZERO,
            slow_decay: Q30::ZERO,
            fast_decay: Q30::ZERO,
            attack_decay: Q30::ZERO,
            drive_decay,
            gain: Q28::ONE,
            ceiling_gain: Q28::ONE,
            mix: Q14::ONE,
            drive: BASE_DRIVE,
        };
        result.set_config(config);
        result.reset();
        result
    }
    /// Returns the effective linear gain of the stage, as f32.
    ///
    /// The value reflects the smoothed drive and any saturation from the output
    /// ceiling at the current envelope level. The stage state does not change.
    pub(crate) fn effective_gain(&self) -> f32 {
        let drive_ratio = self.drive.to_bits() as f64 / BASE_DRIVE.to_bits() as f64;
        let level = self.hold_tree[1].max(self.slow);
        if level <= Q28::ZERO {
            return drive_ratio as f32;
        }
        let env = level.to_bits() as f64 / f64::from(1u32 << 28);
        let ceiling = self.ceiling_gain.to_bits() as f64 / f64::from(1u32 << 28);
        let drive = self.drive.to_bits() as f64 / f64::from(1u32 << 24);
        let two_over_pi = TWO_OVER_PI.to_bits() as f64 / f64::from(1u32 << 14);
        let out = ceiling * two_over_pi * (drive * env / ceiling).atan();
        (out / env) as f32
    }
    /// Calculates the quantized controls again, and keeps the envelopes and
    /// the current drive.
    /// The processor controls the bypass. This stage never reads an enabled
    /// flag.
    pub(crate) fn set_config(&mut self, config: PunchConfig) {
        let impact = coefficient_to_exp_value(1.0, 250.0, 5000.0, 1.0 - config.impact);
        let decay = |milliseconds: f64| {
            Q30::from_f64(envelope_decay(milliseconds, self.rate.hz() as f64, 1))
        };
        self.slow_decay = decay(impact as f64);
        self.fast_decay = decay(FAST_RELEASE_MS);
        self.attack_decay = decay(config.attack_ms as f64);
        self.gain = Q28::from_f64(db_to_gain(config.gain_db as f64));
        self.ceiling_gain =
            Q28::from_f64(db_to_gain(config.ceiling_db as f64)).max(Q28::from_bits(1));
        self.mix = Q14::from_f64((config.mix * 100.0) as f64 / 100.0);
    }
    /// Clears the peak hold and both envelopes, then sets the drive from the
    /// base drive and the input gain.
    pub(crate) fn reset(&mut self) {
        self.hold_cursor = 0;
        self.hold_tree.fill(Q28::ZERO);
        self.slow = Q28::ZERO;
        self.fast = Q28::ZERO;
        self.drive = BASE_DRIVE.mul(self.gain);
    }

    /// Moves `current` toward `target` with a one-pole filter.
    ///
    /// If `target` is higher than `current`, the filter uses `attack`.
    /// Otherwise, it uses `release`.
    fn follow<const F: u32>(
        current: Fixed<F>,
        target: Fixed<F>,
        attack: Q30,
        release: Q30,
    ) -> Fixed<F> {
        if target > current {
            target - attack.mul(target - current)
        } else {
            target + release.mul(current - target)
        }
    }

    /// Inserts `level` into the 6 ms peak hold and returns the held maximum.
    fn push_hold(&mut self, level: Q28) -> Q28 {
        let mut node = self.hold_frames + self.hold_cursor;
        self.hold_tree[node] = level;
        while node > 1 {
            node >>= 1;
            self.hold_tree[node] = self.hold_tree[node * 2].max(self.hold_tree[node * 2 + 1]);
        }
        self.hold_cursor += 1;
        if self.hold_cursor == self.hold_frames {
            self.hold_cursor = 0;
        }
        self.hold_tree[1]
    }

    /// Updates the 6 ms peak hold, both followers and the smoothed drive, then
    /// saturates one PCM sample against the normalized ceiling.
    ///
    /// While the adaptive envelope is zero, the envelope ratio is 1. The stage
    /// limits this ratio to 16, then multiplies it by the input gain. Next, it
    /// divides the driven sample by the ceiling gain and clamps the quotient in
    /// `i64` before the arctangent lookup. Finally, it mixes the dry input and
    /// the wet output in Q14 and clips the PCM output symmetrically.
    pub(crate) fn process_sample(&mut self, input: i16, context: &mut NumericContext) -> i16 {
        let sample: Q28 = Fixed::<15>::from_bits(i32::from(input)).rescale();
        let held = self.push_hold(sample.abs());
        self.slow = Self::follow(self.slow, held, self.attack_decay, self.slow_decay);
        self.fast = Self::follow(self.fast, held, self.attack_decay, self.fast_decay);
        let smooth: Q24 = if self.slow > Q28::ZERO {
            if i64::from(self.fast.to_bits()) < i64::from(self.slow.to_bits()) << 4 {
                self.fast.div::<28, 24>(self.slow).min(MAX_RATIO)
            } else {
                MAX_RATIO
            }
        } else {
            Q24::ONE
        };
        let target_drive: Q24 = BASE_DRIVE.mul::<24, 24>(smooth).mul(self.gain);
        self.drive = Self::follow(self.drive, target_drive, self.drive_decay, self.drive_decay);
        let driven: Q22 = sample.mul(self.drive);
        let normalized = ((i64::from(driven.to_bits()) << 20)
            / i64::from(self.ceiling_gain.to_bits()))
        .clamp(-MAX_TANGENT_Q14, MAX_TANGENT_Q14) as i32;
        let angle = Q14::from_bits(context.atan_q14.process(normalized));
        let shaped: Q28 = TWO_OVER_PI.mul(angle);
        let scaled_bits = i64::from(self.ceiling_gain.mul::<28, 28>(shaped).to_bits());
        let pcm = if scaled_bits >= 0 {
            (scaled_bits + 4096) >> 13
        } else {
            -((-scaled_bits + 4096) >> 13)
        };
        let wet = pcm.clamp(-32767, 32767);
        let dry = i64::from(input);
        let wet_w = i64::from(self.mix.to_bits());
        let dry_w = i64::from((Q14::ONE - self.mix).to_bits());
        (((dry * dry_w + wet * wet_w) >> 14).clamp(-32767, 32767)) as i16
    }
}
