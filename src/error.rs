//! Structured errors for configuration, buffer shapes, MIDI and timed controls.

use std::fmt;

/// The reason for a rejected call.
///
/// A call that returns this error validates before it acts. If it returns
/// `Err`, the state and the output do not change.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum Error {
    /// The numeric rate has no [`crate::SampleRate`] variant that matches it.
    UnsupportedSampleRate {
        /// The unsupported rate, in hertz.
        hz: u32,
    },
    /// A frequency is not finite or is outside the interval `0 < hz <= max_hz`.
    InvalidFrequency {
        /// The rejected frequency, in hertz.
        hz: f32,
        /// The inclusive upper limit, half of the sample rate in hertz.
        max_hz: f32,
    },
    /// A configuration field is not valid.
    InvalidConfig {
        /// The configuration field that is not valid.
        field: ConfigField,
        /// The zero-based voice index in `0..=3` for a harmony interval array.
        ///
        /// For scalar controls, this value is `None`.
        voice: Option<usize>,
        /// The problem with the field.
        problem: ConfigProblem,
    },
    /// A buffer length is not equal to the input length or to the length of
    /// the other output channel.
    BufferLength {
        /// The output buffer whose length is not valid.
        buffer: BufferName,
        /// The necessary slice length. For `[[i16; 2]]`, this is the number of
        /// stereo frames. For planar or mono audio, it is the number of samples
        /// for each channel.
        expected: usize,
        /// The actual slice length, in the same units as `expected`.
        actual: usize,
    },
    /// A block has more frames than the prepared maximum of the processor.
    BlockTooLarge {
        /// The number of frames in the block.
        frames: usize,
        /// The prepared maximum.
        max: usize,
    },
    /// The timed event at `index` is not valid.
    InvalidEvent {
        /// The zero-based index of the event in the slice.
        index: usize,
        /// The zero-based frame offset of the event.
        offset: usize,
        /// The problem with the event.
        problem: EventProblem,
    },
    /// The MIDI bytes or the MIDI fields are not valid.
    InvalidMidi {
        /// The specific MIDI byte or field problem.
        problem: MidiProblem,
    },
}

/// The output buffer whose length is not equal to the expected length.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify),
    serde(rename_all = "camelCase")
)]
pub enum BufferName {
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The stereo frame buffer of [`crate::Processor::process`] or the synthesis buffer of [`crate::Processor::formant_envelope`]."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The synthesis buffer of `Tunelet.formantEnvelope`."
    )]
    Output,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The left channel buffer of [`crate::Processor::process_planar`]."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The left channel buffer of `Tunelet.process`."
    )]
    Left,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The right channel buffer of [`crate::Processor::process_planar`] or [`crate::Processor::drain_planar`]."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The right channel buffer of `Tunelet.process` or `Tunelet.drain`."
    )]
    Right,
}

impl BufferName {
    /// Returns the lowercase name of the buffer.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Output => "output",
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

impl fmt::Display for BufferName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A configuration field that validation examines.
#[cfg_attr(
    not(all(feature = "wasm", target_arch = "wasm32")),
    doc = "",
    doc = " [`Self::as_str`] returns the `snake_case` Rust path. [`Self::camel_case`] returns the `camelCase` JavaScript path."
)]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    doc = "",
    doc = " Each string value is the dotted `camelCase` path of a field on `ProcessorConfig` or the constructor."
)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
#[cfg_attr(
    all(feature = "wasm", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize, tsify::Tsify)
)]
pub enum ConfigField {
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The block limit [`crate::PrepareConfig::max_block_frames`], `1..=usize::MAX`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The block limit `maxBlockFrames`, `1..=4294967295`.",
        serde(rename = "prepare.maxBlockFrames")
    )]
    PrepareMaxBlockFrames,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The Punch input gain [`crate::PunchConfig::gain_db`], `-96..=12`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The Punch input gain `punch.gainDb`, `-96..=12`.",
        serde(rename = "punch.gainDb")
    )]
    PunchGainDb,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The Punch transient emphasis [`crate::PunchConfig::impact`], `0..=1`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The Punch transient emphasis `punch.impact`, `0..=1`.",
        serde(rename = "punch.impact")
    )]
    PunchImpact,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The Punch output ceiling [`crate::PunchConfig::ceiling_db`], `-96..=0`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The Punch output ceiling `punch.ceilingDb`, `-96..=0`.",
        serde(rename = "punch.ceilingDb")
    )]
    PunchCeilingDb,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The Punch attack half-life [`crate::PunchConfig::attack_ms`], `0.01..=5000`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The Punch attack half-life `punch.attackMs`, `0.01..=5000`.",
        serde(rename = "punch.attackMs")
    )]
    PunchAttackMs,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The Warm drive [`crate::WarmConfig::drive_db`], `0..=6`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The Warm drive `warm.driveDb`, `0..=6`.",
        serde(rename = "warm.driveDb")
    )]
    WarmDriveDb,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The pitch acquisition threshold [`crate::PitchConfig::threshold`], `0.002..=0.2`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The pitch acquisition threshold `resynthesis.pitch.threshold`, `0.002..=0.2`.",
        serde(rename = "resynthesis.pitch.threshold")
    )]
    ResynthesisPitchThreshold,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The lower pitch feedback bound [`crate::PitchConfig::min_frequency_hz`], `40..=2000`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The lower pitch feedback bound `resynthesis.pitch.minFrequencyHz`, `40..=2000`.",
        serde(rename = "resynthesis.pitch.minFrequencyHz")
    )]
    ResynthesisPitchMinFrequencyHz,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The upper pitch feedback bound [`crate::PitchConfig::max_frequency_hz`], `40..=2000`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The upper pitch feedback bound `resynthesis.pitch.maxFrequencyHz`, `40..=2000`.",
        serde(rename = "resynthesis.pitch.maxFrequencyHz")
    )]
    ResynthesisPitchMaxFrequencyHz,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The correction retune half-life [`crate::CorrectionConfig::retune_ms`], `0..=400`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The correction retune half-life `resynthesis.correction.retuneMs`, `0..=400`.",
        serde(rename = "resynthesis.correction.retuneMs")
    )]
    ResynthesisCorrectionRetuneMs,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The vocal-tract width factor [`crate::FormantConfig::width`], `0.5..=2`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The vocal-tract width factor `resynthesis.formant.width`, `0.5..=2`.",
        serde(rename = "resynthesis.formant.width")
    )]
    ResynthesisFormantWidth,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The vocal-tract length factor [`crate::FormantConfig::stretch`], `0.5..=2`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The vocal-tract length factor `resynthesis.formant.stretch`, `0.5..=2`.",
        serde(rename = "resynthesis.formant.stretch")
    )]
    ResynthesisFormantStretch,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The aspiration level [`crate::FormantConfig::aspiration_mix`], `0..=1`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The aspiration level `resynthesis.formant.aspirationMix`, `0..=1`.",
        serde(rename = "resynthesis.formant.aspirationMix")
    )]
    ResynthesisFormantAspirationMix,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The aspiration high-pass cutoff [`crate::FormantConfig::aspiration_cutoff_hz`], `500..=10000`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The aspiration high-pass cutoff `resynthesis.formant.aspirationCutoffHz`, `500..=10000`.",
        serde(rename = "resynthesis.formant.aspirationCutoffHz")
    )]
    ResynthesisFormantAspirationCutoffHz,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The fixed semitone offsets [`crate::HarmonyConfig::chromatic_intervals`], each in `-24..=24`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The fixed semitone offsets `resynthesis.harmony.chromaticIntervals`, each in `-24..=24`.",
        serde(rename = "resynthesis.harmony.chromaticIntervals")
    )]
    ResynthesisHarmonyChromaticIntervals,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The scale-step offsets [`crate::HarmonyConfig::diatonic_intervals`], each in `-14..=14`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The scale-step offsets `resynthesis.harmony.diatonicIntervals`, each in `-14..=14`.",
        serde(rename = "resynthesis.harmony.diatonicIntervals")
    )]
    ResynthesisHarmonyDiatonicIntervals,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The interval voice count [`crate::HarmonyConfig::voices`], `0..=4`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The interval voice count `resynthesis.harmony.voices`, `0..=4`.",
        serde(rename = "resynthesis.harmony.voices")
    )]
    ResynthesisHarmonyVoices,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The chord register [`crate::HarmonyConfig::register_percent`], `0..=100`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The chord register `resynthesis.harmony.registerPercent`, `0..=100`.",
        serde(rename = "resynthesis.harmony.registerPercent")
    )]
    ResynthesisHarmonyRegisterPercent,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The chord voicing spread [`crate::HarmonyConfig::voicing_spread_percent`], `0..=100`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The chord voicing spread `resynthesis.harmony.voicingSpreadPercent`, `0..=100`.",
        serde(rename = "resynthesis.harmony.voicingSpreadPercent")
    )]
    ResynthesisHarmonyVoicingSpreadPercent,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The harmony portamento half-life [`crate::HarmonyConfig::portamento_ms`], `0..=10000`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The harmony portamento half-life `resynthesis.harmony.portamentoMs`, `0..=10000`.",
        serde(rename = "resynthesis.harmony.portamentoMs")
    )]
    ResynthesisHarmonyPortamentoMs,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The pitch multiplier [`crate::ResynthesisConfig::pitch_ratio`], `0.25..=4`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The pitch multiplier `resynthesis.pitchRatio`, `0.25..=4`.",
        serde(rename = "resynthesis.pitchRatio")
    )]
    ResynthesisPitchRatio,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The total choir voice count [`crate::ResynthesisConfig::choir_voices`], `1..=16`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The total choir voice count `resynthesis.choirVoices`, `1..=16`.",
        serde(rename = "resynthesis.choirVoices")
    )]
    ResynthesisChoirVoices,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The choir drift control [`crate::ResynthesisConfig::choir_detune`], `10..=100`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The choir drift control `resynthesis.choirDetune`, `10..=100`.",
        serde(rename = "resynthesis.choirDetune")
    )]
    ResynthesisChoirDetune,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The choir delay scale [`crate::ResynthesisConfig::choir_delay`], `20..=200`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The choir delay scale `resynthesis.choirDelay`, `20..=200`.",
        serde(rename = "resynthesis.choirDelay")
    )]
    ResynthesisChoirDelay,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The choir stereo width [`crate::ResynthesisConfig::choir_spread_percent`], `0..=100`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The choir stereo width `resynthesis.choirSpreadPercent`, `0..=100`.",
        serde(rename = "resynthesis.choirSpreadPercent")
    )]
    ResynthesisChoirSpreadPercent,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The harmony pan gain [`crate::ResynthesisConfig::harmony_gain`], `0..=1`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The harmony pan gain `resynthesis.harmonyGain`, `0..=1`.",
        serde(rename = "resynthesis.harmonyGain")
    )]
    ResynthesisHarmonyGain,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The harmonic modulation depth [`crate::ResynthesisConfig::harmonic_modulation`], `0..=24`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The harmonic modulation depth `resynthesis.harmonicModulation`, `0..=24`.",
        serde(rename = "resynthesis.harmonicModulation")
    )]
    ResynthesisHarmonicModulation,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The resynthesis dry fraction [`crate::ResynthesisConfig::dry_mix`], `0..=1`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The resynthesis dry fraction `resynthesis.dryMix`, `0..=1`.",
        serde(rename = "resynthesis.dryMix")
    )]
    ResynthesisDryMix,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The Alien loop length [`crate::AlienConfig::loop_frames`], `1..=96000`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The Alien loop length `alien.loopFrames`, `1..=96000`.",
        serde(rename = "alien.loopFrames")
    )]
    AlienLoopFrames,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The reverb wet fraction [`crate::ReverbConfig::mix`], `0..=1`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The reverb wet fraction `reverb.mix`, `0..=1`.",
        serde(rename = "reverb.mix")
    )]
    ReverbMix,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The reverb diffusion [`crate::ReverbConfig::diffusion`], `0..=1`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The reverb diffusion `reverb.diffusion`, `0..=1`.",
        serde(rename = "reverb.diffusion")
    )]
    ReverbDiffusion,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The reverb feedback decay [`crate::ReverbConfig::decay`], `0..=1`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The reverb feedback decay `reverb.decay`, `0..=1`.",
        serde(rename = "reverb.decay")
    )]
    ReverbDecay,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The reverb damping cutoff [`crate::ReverbConfig::brightness_hz`], `80..=20000`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The reverb damping cutoff `reverb.brightnessHz`, `80..=20000`.",
        serde(rename = "reverb.brightnessHz")
    )]
    ReverbBrightnessHz,
    #[cfg_attr(
        not(all(feature = "wasm", target_arch = "wasm32")),
        doc = " The final linear gain [`crate::ProcessorConfig::output_gain`], `0..=64`."
    )]
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        doc = " The final linear gain `outputGain`, `0..=64`.",
        serde(rename = "outputGain")
    )]
    OutputGain,
}

impl ConfigField {
    /// Returns the dotted `snake_case` Rust path of the field.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PrepareMaxBlockFrames => "prepare.max_block_frames",
            Self::PunchGainDb => "punch.gain_db",
            Self::PunchImpact => "punch.impact",
            Self::PunchCeilingDb => "punch.ceiling_db",
            Self::PunchAttackMs => "punch.attack_ms",
            Self::WarmDriveDb => "warm.drive_db",
            Self::ResynthesisPitchThreshold => "resynthesis.pitch.threshold",
            Self::ResynthesisPitchMinFrequencyHz => "resynthesis.pitch.min_frequency_hz",
            Self::ResynthesisPitchMaxFrequencyHz => "resynthesis.pitch.max_frequency_hz",
            Self::ResynthesisCorrectionRetuneMs => "resynthesis.correction.retune_ms",
            Self::ResynthesisFormantWidth => "resynthesis.formant.width",
            Self::ResynthesisFormantStretch => "resynthesis.formant.stretch",
            Self::ResynthesisFormantAspirationMix => "resynthesis.formant.aspiration_mix",
            Self::ResynthesisFormantAspirationCutoffHz => {
                "resynthesis.formant.aspiration_cutoff_hz"
            }
            Self::ResynthesisHarmonyChromaticIntervals => "resynthesis.harmony.chromatic_intervals",
            Self::ResynthesisHarmonyDiatonicIntervals => "resynthesis.harmony.diatonic_intervals",
            Self::ResynthesisHarmonyVoices => "resynthesis.harmony.voices",
            Self::ResynthesisHarmonyRegisterPercent => "resynthesis.harmony.register_percent",
            Self::ResynthesisHarmonyVoicingSpreadPercent => {
                "resynthesis.harmony.voicing_spread_percent"
            }
            Self::ResynthesisHarmonyPortamentoMs => "resynthesis.harmony.portamento_ms",
            Self::ResynthesisPitchRatio => "resynthesis.pitch_ratio",
            Self::ResynthesisChoirVoices => "resynthesis.choir_voices",
            Self::ResynthesisChoirDetune => "resynthesis.choir_detune",
            Self::ResynthesisChoirDelay => "resynthesis.choir_delay",
            Self::ResynthesisChoirSpreadPercent => "resynthesis.choir_spread_percent",
            Self::ResynthesisHarmonyGain => "resynthesis.harmony_gain",
            Self::ResynthesisHarmonicModulation => "resynthesis.harmonic_modulation",
            Self::ResynthesisDryMix => "resynthesis.dry_mix",
            Self::AlienLoopFrames => "alien.loop_frames",
            Self::ReverbMix => "reverb.mix",
            Self::ReverbDiffusion => "reverb.diffusion",
            Self::ReverbDecay => "reverb.decay",
            Self::ReverbBrightnessHz => "reverb.brightness_hz",
            Self::OutputGain => "output_gain",
        }
    }

    /// Returns the dotted `camelCase` JavaScript path of the field.
    pub const fn camel_case(self) -> &'static str {
        match self {
            Self::PrepareMaxBlockFrames => "prepare.maxBlockFrames",
            Self::PunchGainDb => "punch.gainDb",
            Self::PunchImpact => "punch.impact",
            Self::PunchCeilingDb => "punch.ceilingDb",
            Self::PunchAttackMs => "punch.attackMs",
            Self::WarmDriveDb => "warm.driveDb",
            Self::ResynthesisPitchThreshold => "resynthesis.pitch.threshold",
            Self::ResynthesisPitchMinFrequencyHz => "resynthesis.pitch.minFrequencyHz",
            Self::ResynthesisPitchMaxFrequencyHz => "resynthesis.pitch.maxFrequencyHz",
            Self::ResynthesisCorrectionRetuneMs => "resynthesis.correction.retuneMs",
            Self::ResynthesisFormantWidth => "resynthesis.formant.width",
            Self::ResynthesisFormantStretch => "resynthesis.formant.stretch",
            Self::ResynthesisFormantAspirationMix => "resynthesis.formant.aspirationMix",
            Self::ResynthesisFormantAspirationCutoffHz => "resynthesis.formant.aspirationCutoffHz",
            Self::ResynthesisHarmonyChromaticIntervals => "resynthesis.harmony.chromaticIntervals",
            Self::ResynthesisHarmonyDiatonicIntervals => "resynthesis.harmony.diatonicIntervals",
            Self::ResynthesisHarmonyVoices => "resynthesis.harmony.voices",
            Self::ResynthesisHarmonyRegisterPercent => "resynthesis.harmony.registerPercent",
            Self::ResynthesisHarmonyVoicingSpreadPercent => {
                "resynthesis.harmony.voicingSpreadPercent"
            }
            Self::ResynthesisHarmonyPortamentoMs => "resynthesis.harmony.portamentoMs",
            Self::ResynthesisPitchRatio => "resynthesis.pitchRatio",
            Self::ResynthesisChoirVoices => "resynthesis.choirVoices",
            Self::ResynthesisChoirDetune => "resynthesis.choirDetune",
            Self::ResynthesisChoirDelay => "resynthesis.choirDelay",
            Self::ResynthesisChoirSpreadPercent => "resynthesis.choirSpreadPercent",
            Self::ResynthesisHarmonyGain => "resynthesis.harmonyGain",
            Self::ResynthesisHarmonicModulation => "resynthesis.harmonicModulation",
            Self::ResynthesisDryMix => "resynthesis.dryMix",
            Self::AlienLoopFrames => "alien.loopFrames",
            Self::ReverbMix => "reverb.mix",
            Self::ReverbDiffusion => "reverb.diffusion",
            Self::ReverbDecay => "reverb.decay",
            Self::ReverbBrightnessHz => "reverb.brightnessHz",
            Self::OutputGain => "outputGain",
        }
    }
}

impl fmt::Display for ConfigField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The problem with a configuration field.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum ConfigProblem {
    /// The floating-point value is outside `min..=max`.
    OutOfRange {
        /// The rejected floating-point value.
        value: f32,
        /// The lowest valid value.
        min: f32,
        /// The highest valid value.
        max: f32,
    },
    /// The integer value is outside `min..=max`.
    IntegerOutOfRange {
        /// The rejected integer value.
        value: i128,
        /// The lowest valid integer value.
        min: i128,
        /// The highest valid integer value.
        max: i128,
    },
    /// The floating-point value is `NaN` or infinite.
    NotFinite {
        /// The rejected non-finite value.
        value: f32,
    },
    /// The value must be strictly less than the value of `other`.
    Unordered {
        /// The rejected value of the field.
        value: f32,
        /// The field that `value` must stay below.
        other: ConfigField,
        /// The value of `other`.
        other_value: f32,
    },
}

/// The problem with a three-byte MIDI message or a typed [`crate::MidiEvent`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum MidiProblem {
    /// The status byte at index `0` is below `0x80`.
    StatusByte {
        /// The rejected status byte.
        byte: u8,
    },
    /// A data byte at `index` (`1` or `2`) is more than `127`.
    DataByte {
        /// The byte index in the three-byte message (`1` or `2`).
        index: usize,
        /// The rejected data byte.
        byte: u8,
    },
    /// The zero-based MIDI channel is more than `15`.
    Channel {
        /// The rejected channel value.
        channel: u8,
    },
    /// The MIDI note number is more than `127`.
    Note {
        /// The rejected note value.
        note: u8,
    },
    /// The MIDI velocity is more than `127`.
    Velocity {
        /// The rejected velocity value.
        velocity: u8,
    },
}

/// The reason for the rejection of a timed event during the block preflight.
///
/// The preflight examines the full block. For each event, it checks the offset
/// bounds first, then the order, then the payload. The [`Error::InvalidEvent`]
/// that contains this value identifies the first event in slice order that
/// fails.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum EventProblem {
    /// The zero-based offset is equal to or more than the block frame count.
    ///
    /// Each event in an empty block has this problem.
    OffsetOutsideBlock {
        /// The rejected zero-based frame offset of the event.
        offset: usize,
        /// The number of frames in the block.
        frames: usize,
    },
    /// The offset is less than the offset of the previous event.
    ///
    /// Equal offsets are valid.
    OffsetDecreasing {
        /// The rejected zero-based frame offset of the event.
        offset: usize,
        /// The frame offset of the previous event in the slice.
        previous: usize,
    },
    /// The event has a configuration field that is not valid.
    Config {
        /// The configuration field that is not valid.
        field: ConfigField,
        /// The zero-based voice index in `0..=3` for a harmony interval array.
        voice: Option<usize>,
        /// The problem with the field.
        problem: ConfigProblem,
    },
    /// The event has a MIDI message that is not valid.
    Midi {
        /// The specific MIDI field problem.
        problem: MidiProblem,
    },
}

impl Error {
    /// Returns an `InvalidConfig` error for a scalar field.
    pub(crate) const fn config(field: ConfigField, problem: ConfigProblem) -> Self {
        Self::InvalidConfig {
            field,
            voice: None,
            problem,
        }
    }

    /// Returns an `InvalidConfig` error for one voice slot of an interval array.
    pub(crate) const fn config_voice(
        field: ConfigField,
        voice: usize,
        problem: ConfigProblem,
    ) -> Self {
        Self::InvalidConfig {
            field,
            voice: Some(voice),
            problem,
        }
    }

    /// Returns an error if `actual` is not equal to `expected`.
    ///
    /// The stereo and planar process calls, the planar drain call and the
    /// formant envelope call use this length check.
    pub(crate) fn check_length(
        buffer: BufferName,
        expected: usize,
        actual: usize,
    ) -> Result<(), Self> {
        if expected == actual {
            Ok(())
        } else {
            Err(Self::BufferLength {
                buffer,
                expected,
                actual,
            })
        }
    }
}

/// Checks that a control is finite and in `min..=max`.
///
/// The comparison uses single precision, because the controls are defined in
/// single precision.
pub(crate) fn check_range(field: ConfigField, value: f32, min: f32, max: f32) -> Result<(), Error> {
    if !value.is_finite() {
        return Err(Error::config(field, ConfigProblem::NotFinite { value }));
    }
    if !(min..=max).contains(&value) {
        return Err(Error::config(
            field,
            ConfigProblem::OutOfRange { value, min, max },
        ));
    }
    Ok(())
}

/// Checks an integer control against `min..=max`.
pub(crate) fn check_count(
    field: ConfigField,
    value: i128,
    min: i128,
    max: i128,
) -> Result<(), Error> {
    if !(min..=max).contains(&value) {
        return Err(Error::config(
            field,
            ConfigProblem::IntegerOutOfRange { value, min, max },
        ));
    }
    Ok(())
}

/// Checks one element of a harmony interval array against `min..=max`.
pub(crate) fn check_voice_count(
    field: ConfigField,
    voice: usize,
    value: i128,
    min: i128,
    max: i128,
) -> Result<(), Error> {
    if !(min..=max).contains(&value) {
        return Err(Error::config_voice(
            field,
            voice,
            ConfigProblem::IntegerOutOfRange { value, min, max },
        ));
    }
    Ok(())
}

/// Writes a configuration field and its optional voice slot.
fn write_config_target(
    f: &mut fmt::Formatter<'_>,
    path: &str,
    voice: Option<usize>,
) -> fmt::Result {
    match voice {
        Some(index) => write!(f, "{path}[{index}]"),
        None => f.write_str(path),
    }
}

impl fmt::Display for ConfigProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfRange { value, min, max } => {
                write!(f, "({value}) must be in {min}..={max}")
            }
            Self::IntegerOutOfRange { value, min, max } => {
                write!(f, "({value}) must be in {min}..={max}")
            }
            Self::NotFinite { value } => write!(f, "({value}) must be finite"),
            Self::Unordered {
                value,
                other,
                other_value,
            } => {
                write!(f, "({value}) must be below {other} ({other_value})")
            }
        }
    }
}

impl fmt::Display for MidiProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StatusByte { byte } => {
                write!(f, "MIDI status byte 0x{byte:02x} must be in 0x80..=0xff")
            }
            Self::DataByte { index, byte } => {
                write!(f, "MIDI data byte {index} ({byte}) must be in 0..=127")
            }
            Self::Channel { channel } => write!(f, "MIDI channel {channel} must be in 0..=15"),
            Self::Note { note } => write!(f, "MIDI note {note} must be in 0..=127"),
            Self::Velocity { velocity } => {
                write!(f, "MIDI velocity {velocity} must be in 0..=127")
            }
        }
    }
}

impl fmt::Display for EventProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OffsetOutsideBlock { offset, frames } => {
                write!(f, "offset {offset} is outside the block of {frames} frames")
            }
            Self::OffsetDecreasing { offset, previous } => {
                write!(f, "offset {offset} is below previous offset {previous}")
            }
            Self::Config {
                field,
                voice,
                problem,
            } => {
                write_config_target(f, field.as_str(), *voice)?;
                write!(f, " {problem}")
            }
            Self::Midi { problem } => write!(f, "{problem}"),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSampleRate { hz } => write!(f, "unsupported sample rate: {hz} Hz"),
            Self::InvalidFrequency { hz, max_hz } => {
                write!(
                    f,
                    "frequency {hz} Hz must be finite and in 0 < hz <= {max_hz}"
                )
            }
            Self::InvalidConfig {
                field,
                voice,
                problem,
            } => {
                f.write_str("invalid config: ")?;
                write_config_target(f, field.as_str(), *voice)?;
                write!(f, " {problem}")
            }
            Self::BufferLength {
                buffer,
                expected,
                actual,
            } => {
                write!(f, "{buffer} buffer length is {actual}, expected {expected}")
            }
            Self::BlockTooLarge { frames, max } => {
                write!(
                    f,
                    "block of {frames} frames is larger than the prepared {max}"
                )
            }
            Self::InvalidEvent {
                index,
                offset,
                problem,
            } => match problem {
                EventProblem::OffsetOutsideBlock { .. } | EventProblem::OffsetDecreasing { .. } => {
                    write!(f, "event {index}: {problem}")
                }
                EventProblem::Config { .. } | EventProblem::Midi { .. } => {
                    write!(f, "event {index} at offset {offset}: {problem}")
                }
            },
            Self::InvalidMidi { problem } => write!(f, "{problem}"),
        }
    }
}

impl std::error::Error for Error {}
