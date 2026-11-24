/**
 * Portable TypeScript declarations for `tunelet` and `tunelet/wasm`.
 *
 * These declarations use only standard ECMAScript types and structural
 * interfaces. They do not depend on browser, worker, Node.js or disposable
 * global libraries.
 */

/** The method that resynthesis uses to render its voices. */
export type MultiVoiceMode = "choir" | "harmony";

/** One of the twelve notes in an octave. */
export type PitchClass =
  | "c"
  | "cSharp"
  | "d"
  | "dSharp"
  | "e"
  | "f"
  | "fSharp"
  | "g"
  | "gSharp"
  | "a"
  | "aSharp"
  | "b";

/** A scale for pitch-class correction masks. */
export type CorrectionScale = "major" | "minor" | "chromatic";

/**
 * The permitted correction pitch classes, in the order C, C#, D, D#, E, F,
 * F#, G, G#, A, A#, B.
 *
 * A `true` flag enables that pitch class in all octaves. All combinations of
 * flags are valid. If the mask has no `true` flags, the correction ratio
 * moves toward unity with the configured retune half-life.
 */
export type NoteMask = [
  boolean,
  boolean,
  boolean,
  boolean,
  boolean,
  boolean,
  boolean,
  boolean,
  boolean,
  boolean,
  boolean,
  boolean,
];

/**
 * The source of target periods for the four harmony voices.
 *
 * The default is `"midiChannels"`. Static interval and chord sources use the
 * related `HarmonyConfig` fields. MIDI sources use `ControlEvent` values with
 * `type: "midi"`. Validation always checks all fields.
 */
export type HarmonySource =
  | "fixedInterval"
  | "smartInterval"
  | "chordByDegree"
  | "chordByName"
  | "chordViaMidi"
  | "midiOmni"
  | "midiChannels";

/** The scale that smart intervals and chords by degree use. */
export type HarmonyScale =
  | "major"
  | "majorWithoutSeventh"
  | "majorFlatSeventh"
  | "minor"
  | "harmonicMinor"
  | "minorNaturalSixth"
  | "minorMajorSeventh";

/**
 * A named chord for `"chordByName"`.
 *
 * Triads use three voices. Seventh and sixth chords use four voices.
 */
export type ChordName =
  | "major"
  | "majorSixth"
  | "majorSeventh"
  | "majorSeventhSuspendedFourth"
  | "minor"
  | "minorSixth"
  | "minorSeventh"
  | "minorSeventhFlatFifth"
  | "diminished"
  | "diminishedSeventh"
  | "dominantSeventh"
  | "dominantSeventhFlatFifth"
  | "augmented"
  | "augmentedSeventh"
  | "suspendedFourth"
  | "suspendedSecond"
  | "suspendedSeventh";

/**
 * A diatonic chord with its root on a degree of the selected `HarmonyScale`.
 *
 * The default is `"i"`.
 */
export type ChordDegree =
  | "i"
  | "ii"
  | "iiSeventh"
  | "iii"
  | "iiiSeventh"
  | "iv"
  | "ivSeventh"
  | "v"
  | "vSeventh"
  | "vi"
  | "viSeventh"
  | "vii"
  | "viiSeventh";

/**
 * The chord inversion raises the lowest chord tones by one octave.
 *
 * The inversion applies only to named chords and degree chords. The default
 * is `"root"`.
 */
export type Inversion = "root" | "first" | "second";

/**
 * The echo or reverberation network.
 *
 * The default is `"echo"`. All modes share one delay buffer.
 */
export type ReverbMode =
  | "echo"
  | "schroeder"
  | "smallRoom"
  | "binaural"
  | "mediumHall"
  | "largeHall";

/**
 * The input gain and the transient enhancement with arctangent saturation.
 *
 * Punch operates before Warm and before pitch detection. When Punch is
 * disabled, it does not process audio, and its state does not change.
 */
export interface PunchConfig {
  /** Enables this input stage. The default is `false`. */
  enabled: boolean;
  /** The input gain in decibels, -96..=12. The default is 0 dB. */
  gainDb: number;
  /** The transient emphasis without a unit, 0..=1. The default is 0.5. */
  impact: number;
  /** The output ceiling of the saturation in dBFS, -96..=0. The default is -3 dBFS. */
  ceilingDb: number;
  /** The attack half-life of the peak follower in milliseconds, 0.01..=5000. The default is 10 ms. */
  attackMs: number;
}

/**
 * Asymmetric input saturation, followed by a 50 Hz high-pass filter.
 *
 * Warm operates after Punch and before pitch detection.
 */
export interface WarmConfig {
  /** Enables the saturation stage. The default is `false`. */
  enabled: boolean;
  /** The drive in decibels, 0..=6. The default is 3 dB. */
  driveDb: number;
}

/** The acquisition sensitivity and the bounds for pitch reports to the host. */
export interface PitchConfig {
  /** The normalized cycle-difference threshold for acquisition, 0.002..=0.2. The default is 0.1. */
  threshold: number;
  /** The lower bound of the feedback in hertz, 40..=2000. The default is 40. It must be less than `maxFrequencyHz`. */
  minFrequencyHz: number;
  /** The upper bound of the feedback in hertz, 40..=2000. The default is 1100. It must be greater than `minFrequencyHz`. */
  maxFrequencyHz: number;
}

/** The controls for pitch-class correction in the period domain. */
export interface CorrectionConfig {
  /** Enables the selection of correction targets. The default is `true`. */
  enabled: boolean;
  /** The configured target pitch classes. The default enables all twelve classes. */
  notes: NoteMask;
  /** The half-life of the ratio smoothing in milliseconds, 0..=400. The default is 0. */
  retuneMs: number;
}

/** The controls for vocal-tract changes and synthesized aspiration. */
export interface FormantConfig {
  /** Enables formant analysis and synthesis. The default is `false`. */
  enabled: boolean;
  /** The vocal-tract width factor, 0.5..=2. The default is 1. */
  width: number;
  /** The vocal-tract length factor, 0.5..=2. The default is 1. */
  stretch: number;
  /** The aspiration level, 0..=1, without a unit. The default is 0. */
  aspirationMix: number;
  /** The high-pass cutoff for aspiration in hertz, 500..=10000. The default is 4000. */
  aspirationCutoffHz: number;
}

/** The configuration of four harmony voices that follow intervals, chords or MIDI. */
export interface HarmonyConfig {
  /** The pitch source. The default is `"midiChannels"`. */
  source: HarmonySource;
  /** The pitch class of the tonic. The default is `"c"`. */
  key: PitchClass;
  /** The scale for smart intervals and degree chords. The default is `"major"`. */
  scale: HarmonyScale;
  /** Four fixed semitone offsets, each in -24..=24. The default is `[0, 0, 0, 0]`. */
  chromaticIntervals: [number, number, number, number];
  /** Four offsets in scale steps, each in -14..=14. The default is `[0, 0, 0, 0]`. */
  diatonicIntervals: [number, number, number, number];
  /** The named chord for `"chordByName"`. The default is `"major"`. */
  chord: ChordName;
  /** The chord on a scale degree for `"chordByDegree"`. The default is `"i"`. */
  degree: ChordDegree;
  /** The inversion for named chords and degree chords. The default is `"root"`. */
  inversion: Inversion;
  /** The chord register from low to high, 0..=100%. The default is 50%. */
  registerPercent: number;
  /** The chord voicing spread from close to open, 0..=100%. The default is 0%. */
  voicingSpreadPercent: number;
  /** The half-life of the target-period glide in milliseconds, 0..=10000. The default is 0. */
  portamentoMs: number;
  /**
   * The number of voices that sound in the interval sources, 0..=4.
   *
   * This field is optional. If it is absent or `undefined`, the value is 4.
   */
  voices?: number | undefined;
}

/**
 * The controls for pitch correction, pitch shift, voice arrangement and
 * formants.
 */
export interface ResynthesisConfig {
  /** The acquisition sensitivity and the pitch report bounds. */
  pitch: PitchConfig;
  /** The target notes and the retune time. */
  correction: CorrectionConfig;
  /** The pitch multiplier, 0.25..=4. The default is 1. */
  pitchRatio: number;
  /** The vocal-tract changes and aspiration. */
  formant: FormantConfig;
  /** The voice arrangement. The default is `"choir"`. */
  mode: MultiVoiceMode;
  /** The total number of choir voices, with the lead voice, 1..=16. The default is 1. */
  choirVoices: number;
  /** The drift control for secondary voices, 10..=100, without a unit. The default is 50. */
  choirDetune: number;
  /** The scale of the choir delay table, 20..=200, without a unit. The default is 60. */
  choirDelay: number;
  /** The stereo width of the choir, 0..=100%. The default is 100%. */
  choirSpreadPercent: number;
  /** Enables a stereo pan for each voice. The default is `false`. */
  stereoOutput: boolean;
  /** The controls for the four harmony voices. */
  harmony: HarmonyConfig;
  /** The linear gain of the four harmony voices, 0..=1. The default is 0.5. */
  harmonyGain: number;
  /** The modulation depth, 0..=24, without a unit. The default is 0. */
  harmonicModulation: number;
  /** The dry fraction at the resynthesis stage, 0..=1. The default is 0. */
  dryMix: number;
  /** The 64-bit seed for choir drift and aspiration. The default is `0x1234_abcd`. */
  randomSeed: bigint;
}

/** Reverse playback of recent stereo audio in repeated loops with fades. */
export interface AlienConfig {
  /** Selects reversed playback instead of the direct input. The default is `false`. */
  enabled: boolean;
  /** The loop length in frames, 1..=96000. The default is 95,999. */
  loopFrames: number;
}

/** The reverb after Alien, with controls for the network, the wet mix and the decay. */
export interface ReverbConfig {
  /** Enables the selected network. The default is `false`. */
  enabled: boolean;
  /** The topology of the delay network. The default is `"echo"`. */
  mode: ReverbMode;
  /** The wet fraction, 0..=1. The default is 0.5. */
  mix: number;
  /** The diffusion control without a unit, 0..=1. The default is 0.5. */
  diffusion: number;
  /** The feedback control without a unit, 0..=1. The default is 0.6. */
  decay: number;
  /** The low-pass cutoff in hertz for damped paths, 80..=20000. The default is 4000 Hz. */
  brightnessHz: number;
}

/** The settings for the complete signal chain, in the order of the stages. */
export interface ProcessorConfig {
  /** The input dynamics and saturation stage, which applies first. */
  punch: PunchConfig;
  /** The asymmetric saturation stage after Punch. */
  warm: WarmConfig;
  /** The stage for pitch analysis, correction, voice resynthesis and formants. */
  resynthesis: ResynthesisConfig;
  /** The reverse-loop effect after resynthesis. */
  alien: AlienConfig;
  /** The echo or reverberation stage after Alien. */
  reverb: ReverbConfig;
  /** The linear gain after all effects, 0..=64. The default is 1. */
  outputGain: number;
}

/** The supported MIDI note controls, with zero-based channel numbers. */
export type MidiEvent =
  | { type: "noteOn"; channel: number; note: number; velocity: number }
  | { type: "noteOff"; channel: number; note: number }
  | { type: "allNotesOff" };

/** A configuration or MIDI control that applies before a frame in the block. */
export type ControlEvent =
  | { type: "configure"; value: ProcessorConfig }
  | { type: "resynthesis"; value: ResynthesisConfig }
  | { type: "punch"; value: PunchConfig }
  | { type: "warm"; value: WarmConfig }
  | { type: "alien"; value: AlienConfig }
  | { type: "reverb"; value: ReverbConfig }
  | { type: "outputGain"; value: number }
  | { type: "midi"; value: MidiEvent }
  | { type: "correctionMidi"; value: MidiEvent }
  | { type: "reset" };

/**
 * A control that applies immediately before the zero-based input frame
 * `offset`.
 */
export interface TimedEvent {
  /** The frame offset in the block. */
  offset: number;
  /** The control change. */
  event: ControlEvent;
}

/** The events for one call that processes audio (`TimedEvent[]`). */
export type BlockEvents = TimedEvent[];

/** A snapshot of the pitch analysis and the output level. */
export interface Feedback {
  /** The pitch-correction ratio from the last processed frame, where 1 is unity. */
  resampleRatio: number;
  /** The reported input pitch period in samples. */
  periodSamples: number;
  /** The reported input pitch frequency in hertz. */
  frequencyHz: number;
  /** The correction target or the input pitch as a MIDI note, or `undefined` if `voiced` is `false`. */
  targetMidiNote: number | undefined;
  /** `true` if the current pitch estimate is positive and inside the report bounds. */
  voiced: boolean;
  /** The largest absolute clipped stereo sample in the window, divided by 32768. */
  peakLevel: number;
  /** The number of frames processed since construction or the last reset. */
  processedFrames: bigint;
  /** The nominal resynthesis latency in frames. */
  latencyFrames: number;
}

/** A snapshot of the voices, the levels and the effect state for meters and displays. */
export interface Telemetry {
  /** The number of frames processed since construction or the last reset. */
  processedFrames: bigint;
  /** The largest absolute input sample in the window, divided by 32768. */
  inputPeak: number;
  /** The largest absolute clipped sample of the left and right output in the window, divided by 32768. */
  outputPeak: [number, number];
  /** The largest absolute wet contribution of the reverb in the window. */
  reverbWetPeak: number;
  /** How periodic the tracked input is, in 0..=1. */
  clarity: number;
  /** `true` while the pitch tracker follows a period. */
  tracking: boolean;
  /** The number of valid entries in `voiceRatios` and `voiceGains`. */
  voiceCount: number;
  /** The pitch ratio of each voice on the last frame, where 1 is the input pitch. */
  voiceRatios: [
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
  ];
  /** The linear gain of each voice, in the order of `voiceRatios`. */
  voiceGains: [
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
  ];
  /** The current drive of Punch, divided by its base drive. */
  punchGain: number;
  /** The linear gain that Warm applies before its saturation curve and removes after it. */
  warmGain: number;
  /** The length of the Alien loop in frames. */
  alienLoopFrames: number;
  /** The current frame in the Alien loop. */
  alienPhaseFrames: number;
  /** The count of formant analyses since construction, which wraps at `2^32`. */
  formantGeneration: number;
}

/** A stateful 16-bit PCM processor instance. */
export interface TuneletInstance {
  /** Releases the Wasm memory of this processor instance. */
  free(): void;
  /** Starts a finite tail budget and returns its frame count as a number. */
  beginDrain(): number;
  /** Returns a copy of the active configuration as a new JavaScript object. */
  config(): ProcessorConfig;
  /** Renders a mono tail block and returns its frame count. */
  drainMono(audio: Int16Array): number;
  /** Renders stereo tail frames into `Int16Array` buffers of equal length. */
  drain(left: Int16Array, right: Int16Array): number;
  /** Returns a new feedback object and does not clear the peak window. */
  feedback(): Feedback;
  /** Writes the formant envelopes of the current model into two `Float32Array` buffers of equal length. */
  formantEnvelope(
    max_frequency_hz: number,
    input: Float32Array,
    output: Float32Array,
  ): boolean;
  /** Replaces the audio in a mono `Int16Array` with the stereo downmix. */
  processMono(audio: Int16Array, events?: BlockEvents | null): void;
  /** Processes mono input into left and right `Int16Array` buffers of equal length. */
  process(
    input: Int16Array,
    left: Int16Array,
    right: Int16Array,
    events?: BlockEvents | null,
  ): void;
  /** Resets the audio histories and feedback, as the native processor does. */
  reset(): void;
  /** Decodes and validates a configuration, then replaces the complete configuration with it. */
  setConfig(config: ProcessorConfig): void;
  /** Returns the tail budget of the current configuration. */
  tailFrames(): number;
  /** Returns the feedback and clears only the stereo peak window. */
  takeFeedback(): Feedback;
  /** Returns the telemetry and clears only its three peak windows. */
  takeTelemetry(): Telemetry;
  /** Returns a new telemetry object and does not clear its peak windows. */
  telemetry(): Telemetry;
  /** The nominal resynthesis latency: 1,593 frames at both sample rates. */
  readonly latencyFrames: number;
  /** The maximum frame count for each call that processes or drains audio. */
  readonly maxBlockFrames: number;
  /** The sample rate of the input and the output in hertz. */
  readonly sampleRate: number;
}

/** A stateful 16-bit PCM processor instance. */
export type Tunelet = TuneletInstance;

/** The constructor interface of the `Tunelet` class. */
export interface TuneletConstructor {
  readonly prototype: Tunelet;
  /**
   * Prepares a processor and its reusable PCM buffers.
   *
   * `sample_rate` must be 44100 or 48000. `max_block_frames` must be an
   * integer in 1..=4294967295. If you omit `config`, the processor uses the
   * default configuration.
   */
  new (
    sample_rate: number,
    max_block_frames: number,
    config?: ProcessorConfig | null,
  ): Tunelet;
}

/** A structural URL object with a protocol and a full `href` string. */
export interface WasmUrlLike {
  readonly href: string;
  readonly protocol: string;
}

/** A structural fetch `Request` object. */
export interface WasmRequestLike {
  readonly url: string;
  readonly headers: unknown;
}

/** A structural fetch `Response` object. */
export interface WasmResponseLike {
  readonly ok: boolean;
  readonly status: number;
  arrayBuffer(): Promise<ArrayBuffer>;
}

/**
 * A structural guard for a precompiled Wasm module object.
 *
 * Host `Module` interfaces have no own properties. The excluded keys reject
 * URLs, fetch objects, promises and option wrappers.
 */
export interface WasmCompiledModule {
  readonly href?: never;
  readonly module?: never;
  readonly module_or_path?: never;
  readonly then?: never;
  readonly url?: never;
}

/** Raw Wasm bytes in an `ArrayBuffer` or a typed view. */
export type WasmByteSource = ArrayBufferLike | ArrayBufferView;

/** A synchronous Wasm source: raw bytes or a precompiled module object. */
export type WasmSyncInitInput = WasmByteSource | (object & WasmCompiledModule);

/**
 * An asynchronous Wasm source: a URL string, a URL object, a fetch request or
 * response, raw bytes or a precompiled module object.
 */
export type WasmInitInput =
  | string
  | WasmUrlLike
  | WasmRequestLike
  | WasmResponseLike
  | WasmSyncInitInput;

/** Linear Wasm memory exported by an initialized module instance. */
export interface WasmMemory {
  readonly buffer: ArrayBufferLike;
}

/** The public exports of an initialized Wasm instance. */
export interface WasmInitOutput {
  readonly memory: WasmMemory;
}

/** Alias of `WasmSyncInitInput` for compatibility. */
export type SyncInitInput = WasmSyncInitInput;

/** Alias of `WasmInitInput` for compatibility. */
export type InitInput = WasmInitInput;

/** Alias of `WasmInitOutput` for compatibility. */
export type InitOutput = WasmInitOutput;
