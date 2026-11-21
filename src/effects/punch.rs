//! Transient enhancement with an adaptive drive and an arctangent output
//! ceiling.

use crate::error::check_range;
use crate::numeric::fixed::{Q14, Q8};
use crate::numeric::{coefficient_to_exp_value, db_to_gain, envelope_decay, NumericContext};
use crate::{Error, SampleRate};

/// The update interval of the peak follower, in frames.
const WINDOW_FRAMES: usize = 100;
/// The base arctangent drive, approximately π/2 in Q8.
const BASE_DRIVE: Q8 = Q8::from_bits(401);
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
        validate_gain_db(self.gain_db)?;
        validate_impact(self.impact)?;
        validate_ceiling_db(self.ceiling_db)?;
        validate_attack_ms(self.attack_ms)
    }
}

/// Checks the range of the input gain of Punch.
pub(crate) fn validate_gain_db(gain_db: f32) -> Result<(), Error> {
    check_range("punch.gain_db", gain_db, -96.0, 12.0)
}

/// Checks the range of the transient emphasis of Punch.
pub(crate) fn validate_impact(impact: f32) -> Result<(), Error> {
    check_range("punch.impact", impact, 0.0, 1.0)
}

/// Checks the range of the output ceiling of Punch.
pub(crate) fn validate_ceiling_db(ceiling_db: f32) -> Result<(), Error> {
    check_range("punch.ceiling_db", ceiling_db, -96.0, 0.0)
}

/// Checks the range of the attack half-life of Punch.
pub(crate) fn validate_attack_ms(attack_ms: f32) -> Result<(), Error> {
    check_range("punch.attack_ms", attack_ms, 0.01, 5000.0)
}

/// Two peak followers that control an arctangent saturator, with all buffers
/// inline.
#[derive(Debug)]
pub(crate) struct Punch {
    /// The prepared rate, which converts the envelope times to sample windows.
    rate: SampleRate,
    /// The validated controls that set the quantized coefficients.
    config: PunchConfig,
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
    /// The configured linear input gain in Q8.
    gain: Q8,
    /// The linear output ceiling in Q14. It applies to the saturator angle.
    ceiling_gain: Q14,
    /// The cached Q8 drive: base drive × envelope ratio × input gain.
    drive: Q8,
}
impl Punch {
    /// Prepares the coefficients and resets both followers from validated
    /// settings.
    pub(crate) fn new(rate: SampleRate, config: PunchConfig) -> Self {
        let fast_decay = Q14::from_f64(envelope_decay(5000.0, rate.hz() as f64, WINDOW_FRAMES));
        let mut result = Self {
            rate,
            config,
            slow: Q14::default(),
            fast: Q14::default(),
            counter: 0,
            peak: Q14::default(),
            slow_decay: Q14::default(),
            fast_decay,
            attack_decay: Q14::default(),
            gain: Q8::ONE,
            ceiling_gain: Q14::default(),
            drive: BASE_DRIVE,
        };
        result.configure_all();
        result.reset();
        result
    }
    /// Returns the cached drive divided by the base drive, as f32.
    ///
    /// The value is the envelope ratio times the configured input gain. The
    /// stage state does not change.
    pub(crate) fn drive_ratio(&self) -> f32 {
        (self.drive.to_bits() as f64 / BASE_DRIVE.to_bits() as f64) as f32
    }
    /// Quantizes a decay half-life in milliseconds at the 100-frame cadence.
    fn decay(&self, milliseconds: f64) -> Q14 {
        Q14::from_f64(envelope_decay(
            milliseconds,
            self.rate.hz() as f64,
            WINDOW_FRAMES,
        ))
    }
    /// Updates the release coefficient of the adaptive follower.
    fn configure_impact(&mut self) {
        let impact = coefficient_to_exp_value(1.0, 250.0, 5000.0, 1.0 - self.config.impact);
        self.slow_decay = self.decay(impact as f64);
    }
    /// Updates the shared attack coefficient of the two followers.
    fn configure_attack_ms(&mut self) {
        self.attack_decay = self.decay(self.config.attack_ms as f64);
    }
    /// Updates the quantized input gain.
    fn configure_gain_db(&mut self) {
        self.gain = Q8::from_f64(db_to_gain(self.config.gain_db as f64));
    }
    /// Updates the quantized output ceiling.
    fn configure_ceiling_db(&mut self) {
        self.ceiling_gain = Q14::from_f64(db_to_gain(self.config.ceiling_db as f64));
    }
    /// Initializes all configurable coefficients unconditionally.
    fn configure_all(&mut self) {
        self.configure_impact();
        self.configure_attack_ms();
        self.configure_gain_db();
        self.configure_ceiling_db();
    }
    /// Calculates the changed quantized controls again, and keeps the
    /// envelopes and the cached drive.
    ///
    /// The processor controls the bypass. This stage never reads an enabled
    /// flag.
    pub(crate) fn set_config(&mut self, config: PunchConfig) {
        let old = self.config;
        self.config = config;
        if old.impact != config.impact {
            self.configure_impact();
        }
        if old.attack_ms != config.attack_ms {
            self.configure_attack_ms();
        }
        if old.gain_db != config.gain_db {
            self.configure_gain_db();
        }
        if old.ceiling_db != config.ceiling_db {
            self.configure_ceiling_db();
        }
    }
    /// Stores the input gain. If the value changed, quantizes it again.
    pub(crate) fn set_gain_db(&mut self, gain_db: f32) {
        let changed = self.config.gain_db != gain_db;
        self.config.gain_db = gain_db;
        if changed {
            self.configure_gain_db();
        }
    }
    /// Stores the transient emphasis. If the value changed, updates the
    /// release coefficient.
    pub(crate) fn set_impact(&mut self, impact: f32) {
        let changed = self.config.impact != impact;
        self.config.impact = impact;
        if changed {
            self.configure_impact();
        }
    }
    /// Stores the output ceiling. If the value changed, quantizes it again.
    pub(crate) fn set_ceiling_db(&mut self, ceiling_db: f32) {
        let changed = self.config.ceiling_db != ceiling_db;
        self.config.ceiling_db = ceiling_db;
        if changed {
            self.configure_ceiling_db();
        }
    }
    /// Stores the attack half-life. If the value changed, updates the attack
    /// coefficient.
    pub(crate) fn set_attack_ms(&mut self, attack_ms: f32) {
        let changed = self.config.attack_ms != attack_ms;
        self.config.attack_ms = attack_ms;
        if changed {
            self.configure_attack_ms();
        }
    }
    /// Clears both envelopes and the current window, then calculates the drive
    /// again from the base drive and the input gain.
    pub(crate) fn reset(&mut self) {
        self.slow = Q14::default();
        self.fast = Q14::default();
        self.counter = 0;
        self.peak = Q14::default();
        self.drive = BASE_DRIVE.mul(self.gain);
    }

    /// Moves `current` toward `target` with a one-pole filter.
    ///
    /// The filter uses `attack` if `target` is higher than `current`.
    /// Otherwise, it uses `release`.
    fn follow(current: Q14, target: Q14, attack: Q14, release: Q14) -> Q14 {
        let decay = if target > current { attack } else { release };
        target + decay.mul(current - target)
    }

    /// Tracks the current peak, updates the drive every 100 calls and
    /// saturates one PCM sample.
    /// The envelope ratio is approximately limited to 16 before it is
    /// multiplied by the input gain. While the adaptive envelope is zero, the
    /// ratio is 1. The Q8 drive goes into the cached arctangent lookup. The
    /// final PCM value clips symmetrically.
    pub(crate) fn process_sample(&mut self, input: i16, context: &mut NumericContext) -> i16 {
        let sample = Q14::from_bits(i32::from(input) >> 1);
        self.peak = self.peak.max(sample.abs());
        self.counter += 1;
        if self.counter == WINDOW_FRAMES {
            self.counter = 0;
            self.slow = Self::follow(self.slow, self.peak, self.attack_decay, self.slow_decay);
            self.fast = Self::follow(self.fast, self.peak, self.attack_decay, self.fast_decay);
            self.peak = Q14::default();
            let smooth: Q8 = if self.slow > Q14::default() {
                if self.fast >> 6 <= (self.slow >> 6) << 4 {
                    self.fast.div(self.slow)
                } else {
                    Q8::from_bits(16 << 8)
                }
            } else {
                Q8::ONE
            };
            self.drive = BASE_DRIVE.mul::<8, 8>(smooth).mul(self.gain);
        }
        let driven: Q8 = sample.mul(self.drive);
        let angle = Q14::from_bits(context.atan_q14.process(driven.to_bits(), 14 - 8));
        let output: Q14 = TWO_OVER_PI.mul(self.ceiling_gain.mul::<14, 14>(angle));
        output.to_bits().wrapping_shl(1).clamp(-32767, 32767) as i16
    }
}
