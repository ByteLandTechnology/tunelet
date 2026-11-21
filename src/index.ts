/**
 * The shared ESM entry point for browsers and Node.js.
 *
 * Call `loadTunelet()` or `initSync()` before you construct processors or read
 * the default configuration. Both environments use the same generated bindings
 * and the same Wasm binary. This module wraps `loadTunelet`, `initSync`,
 * `defaultConfig` and `Tunelet` with lifecycle checks, and re-exports the
 * configuration, event, feedback and telemetry types.
 */
import wasmInit, {
  Tunelet as WasmTunelet,
  defaultConfig as wasmDefaultConfig,
  initSync as wasmInitSync,
} from "../pkg/tunelet.js";
import type {
  AlienConfig,
  BlockEvents,
  ChordDegree,
  ChordName,
  ControlEvent,
  CorrectionConfig,
  CorrectionScale,
  Feedback,
  FormantConfig,
  HarmonyConfig,
  HarmonyScale,
  HarmonySource,
  InitInput,
  InitOutput,
  Inversion,
  MidiEvent,
  MultiVoiceMode,
  NoteMask,
  PitchClass,
  PitchConfig,
  ProcessorConfig,
  PunchConfig,
  ResynthesisConfig,
  ReverbConfig,
  ReverbMode,
  SyncInitInput,
  Telemetry,
  TimedEvent,
  WarmConfig,
} from "../pkg/tunelet.js";

export type {
  AlienConfig,
  BlockEvents,
  ChordDegree,
  ChordName,
  ControlEvent,
  CorrectionConfig,
  CorrectionScale,
  Feedback,
  FormantConfig,
  HarmonyConfig,
  HarmonyScale,
  HarmonySource,
  InitInput,
  InitOutput,
  Inversion,
  MidiEvent,
  MultiVoiceMode,
  NoteMask,
  PitchClass,
  PitchConfig,
  ProcessorConfig,
  PunchConfig,
  ResynthesisConfig,
  ReverbConfig,
  ReverbMode,
  SyncInitInput,
  Telemetry,
  TimedEvent,
  WarmConfig,
};

/** State for the active or completed Wasm initialization attempt. */
interface InitAttempt {
  rawSource: InitInput | Promise<InitInput> | undefined;
  resolvedInput: InitInput | undefined;
  inputPromise: Promise<InitInput>;
  outputPromise: Promise<InitOutput>;
}

/** The active or completed initialization state, which a failure clears. */
let activeAttempt: InitAttempt | undefined;

/** The initialized Wasm exports after a successful initialization. */
let initializedOutput: InitOutput | undefined;

/** Returns the default URL of the packaged Wasm binary. */
function defaultWasmUrl(): URL {
  return new URL("../pkg/tunelet_bg.wasm", import.meta.url);
}

/** Checks if a value is a Promise-like object. */
function isPromiseLike(
  value: InitInput | Promise<InitInput> | undefined,
): value is Promise<InitInput> {
  return (
    (typeof value === "object" || typeof value === "function") &&
    value !== null &&
    "then" in value &&
    typeof (value as { then?: unknown }).then === "function"
  );
}

/** Checks if two resolved Wasm sources identify the same input. */
function sameInput(left: InitInput, right: InitInput): boolean {
  if (Object.is(left, right)) {
    return true;
  }
  return left instanceof URL && right instanceof URL && left.href === right.href;
}

/** Checks if two unawaited `loadTunelet` arguments are immediately equal. */
function sameRawSource(
  left: InitInput | Promise<InitInput> | undefined,
  right: InitInput | Promise<InitInput> | undefined,
): boolean {
  if (Object.is(left, right)) {
    return true;
  }
  if (isPromiseLike(left) || isPromiseLike(right)) {
    return false;
  }
  const leftInput = left === undefined ? defaultWasmUrl() : left;
  const rightInput = right === undefined ? defaultWasmUrl() : right;
  return sameInput(leftInput, rightInput);
}

/** Resolves an optional `loadTunelet` argument to a concrete `InitInput`. */
async function resolveInput(
  source: InitInput | Promise<InitInput> | undefined,
): Promise<InitInput> {
  return source === undefined ? defaultWasmUrl() : await source;
}

/** Throws if the Wasm module has not completed initialization. */
function ensureInitialized(): void {
  if (initializedOutput === undefined) {
    throw new Error(
      "Tunelet Wasm is not initialized. Await loadTunelet() or call initSync() first.",
    );
  }
}

/** Checks if a synchronous init argument uses the `{ module }` wrapper. */
function isSyncModuleObject(
  value: { module: SyncInitInput } | SyncInitInput,
): value is { module: SyncInitInput } {
  return (
    typeof value === "object" &&
    value !== null &&
    Object.getPrototypeOf(value) === Object.prototype &&
    "module" in value
  );
}

/**
 * Initializes the Wasm module one time for this imported package instance.
 *
 * If you give no argument, the function finds the packaged binary relative to
 * this module. Node.js reads a `file:` URL, and browsers fetch the served
 * asset. An import of the package does not load Wasm. In browsers, serve the
 * assets over HTTP(S), or give the bytes or a compiled module as the source.
 *
 * For a custom deployment, you can give one of these sources:
 *
 * - A URL.
 * - A fetch Response.
 * - A byte buffer (BufferSource).
 * - A compiled WebAssembly.Module.
 * - A promise for one of these.
 *
 * A string source obeys fetch rules. For a Node.js `file:` source, use a
 * URL object. A compiled module must use imports that agree with the packaged
 * bindings.
 *
 * Concurrent calls with the same source share one promise. After a successful
 * initialization, later calls with the same source return that promise and do
 * not load Wasm again. Calls with a different source reject with an `Error`.
 * If an attempt fails, the promise rejects and the function clears the cached
 * promise. Then you can try again. File, network and Wasm errors go to the
 * caller. The function does not try a different load method after an error.
 *
 * @param source The optional Wasm source. The default is the binary in this
 * package.
 * @returns The initialized Wasm exports. Use the exported `Tunelet` class for
 * processors.
 */
export function loadTunelet(
  source?: InitInput | Promise<InitInput>,
): Promise<InitOutput> {
  if (activeAttempt !== undefined) {
    if (sameRawSource(activeAttempt.rawSource, source)) {
      return activeAttempt.outputPromise;
    }
    if (!isPromiseLike(activeAttempt.rawSource) && !isPromiseLike(source)) {
      return Promise.reject(
        new Error(
          "Cannot call loadTunelet() with a different Wasm source after initialization starts.",
        ),
      );
    }
    return verifyMatchingSource(activeAttempt, source);
  }

  const inputPromise = resolveInput(source);
  const attempt: InitAttempt = {
    rawSource: source,
    resolvedInput: undefined,
    inputPromise,
    outputPromise: Promise.resolve(undefined as never),
  };
  attempt.outputPromise = initializeFromAttempt(attempt).catch(
    (error: unknown) => {
      if (activeAttempt === attempt && initializedOutput === undefined) {
        activeAttempt = undefined;
      }
      throw error;
    },
  );
  activeAttempt = attempt;
  return attempt.outputPromise;
}

/** Verifies that a Promise-wrapped source resolves to the active source. */
async function verifyMatchingSource(
  attempt: InitAttempt,
  source: InitInput | Promise<InitInput> | undefined,
): Promise<InitOutput> {
  const [expected, actual] = await Promise.all([
    attempt.inputPromise,
    resolveInput(source),
  ]);
  if (!sameInput(expected, actual)) {
    throw new Error(
      "Cannot call loadTunelet() with a different Wasm source after initialization starts.",
    );
  }
  return attempt.outputPromise;
}

/** Instantiates the Wasm module for a new initialization attempt. */
async function initializeFromAttempt(
  attempt: InitAttempt,
): Promise<InitOutput> {
  const input = await attempt.inputPromise;
  attempt.resolvedInput = input;

  let output: InitOutput;
  if (input instanceof URL && input.protocol === "file:") {
    // Keep the Node.js built-in module out of browser bundles. TypeScript
    // removes the type-only import. The runtime import runs only for `file:`
    // URLs.
    const moduleName = "node:fs/promises";
    const fs: typeof import("node:fs/promises") = await import(
      /* webpackIgnore: true */ /* @vite-ignore */ moduleName
    );
    output = wasmInitSync({ module: await fs.readFile(input) });
  } else {
    output = await wasmInit({ module_or_path: input });
  }

  initializedOutput = output;
  return output;
}

/**
 * Instantiates the Wasm module synchronously from bytes or a compiled
 * `WebAssembly.Module`.
 *
 * Repeated calls with the same source return the initialized exports. A call
 * with a different source, or while `loadTunelet()` is active, throws an
 * `Error`.
 */
export function initSync(
  module: { module: SyncInitInput } | SyncInitInput,
): InitOutput {
  const input = isSyncModuleObject(module) ? module.module : module;
  if (activeAttempt !== undefined) {
    if (
      initializedOutput !== undefined &&
      activeAttempt.resolvedInput !== undefined &&
      sameInput(activeAttempt.resolvedInput, input)
    ) {
      return initializedOutput;
    }
    throw new Error(
      "Cannot call initSync() with a different Wasm source after initialization starts.",
    );
  }

  const output =
    input === undefined ? wasmInitSync(module) : wasmInitSync({ module: input });
  initializedOutput = output;
  const outputPromise = Promise.resolve(output);
  activeAttempt = {
    rawSource: input,
    resolvedInput: input,
    inputPromise: Promise.resolve(input),
    outputPromise,
  };
  return output;
}

/**
 * Returns a complete, independent default configuration for JavaScript.
 *
 * Call `loadTunelet()` or `initSync()` before this function. Change the fields
 * of the returned object and give it to `new Tunelet(...)` or `setConfig`.
 */
export function defaultConfig(): ProcessorConfig {
  ensureInitialized();
  return wasmDefaultConfig();
}

/**
 * A stateful PCM processor with lifecycle guards.
 *
 * Call `loadTunelet()` or `initSync()` before you construct an instance. Call
 * `free()` when you do not need the instance any more. Duplicate `free()`
 * calls do no work, and later method or property calls throw an `Error`.
 */
export class Tunelet {
  #inner: WasmTunelet | undefined;

  declare [Symbol.dispose]: () => void;

  /**
   * Prepares a processor and its reusable PCM buffers.
   *
   * `sampleRate` must be 44100 or 48000. `maxBlockFrames` must be an integer
   * in 1..=4294967295. If you omit `config`, the processor uses the default
   * configuration.
   */
  constructor(
    sampleRate: number,
    maxBlockFrames: number,
    config?: ProcessorConfig | null,
  ) {
    ensureInitialized();
    this.#inner = new WasmTunelet(sampleRate, maxBlockFrames, config);
  }

  #active(): WasmTunelet {
    if (this.#inner === undefined) {
      throw new Error(
        "Tunelet instance is disposed. Do not use it after free().",
      );
    }
    return this.#inner;
  }

  /** Releases the Wasm processor memory. Duplicate calls do no work. */
  free(): void {
    const inner = this.#inner;
    if (inner === undefined) {
      return;
    }
    this.#inner = undefined;
    inner.free();
  }

  /** The sample rate of the input and the output in hertz. */
  get sampleRate(): number {
    return this.#active().sampleRate;
  }

  /** The maximum frame count for each call that processes or drains audio. */
  get maxBlockFrames(): number {
    return this.#active().maxBlockFrames;
  }

  /** The nominal resynthesis delay in frames. */
  get latencyFrames(): number {
    return this.#active().latencyFrames;
  }

  /** Returns a copy of the active configuration as a new JavaScript object. */
  config(): ProcessorConfig {
    return this.#active().config();
  }

  /** Validates a configuration and replaces the complete configuration. */
  setConfig(config: ProcessorConfig): void {
    this.#active().setConfig(config);
  }

  /** Processes mono input into left and right `Int16Array` buffers. */
  process(
    input: Int16Array,
    left: Int16Array,
    right: Int16Array,
    events?: BlockEvents | null,
  ): void {
    this.#active().process(input, left, right, events);
  }

  /** Replaces the audio in a mono `Int16Array` with the stereo downmix. */
  processMono(audio: Int16Array, events?: BlockEvents | null): void {
    this.#active().processMono(audio, events);
  }

  /** Starts a finite tail budget and returns its frame count. */
  beginDrain(): number {
    return this.#active().beginDrain();
  }

  /** Returns the tail budget of the current configuration. */
  tailFrames(): number {
    return this.#active().tailFrames();
  }

  /** Renders stereo tail frames into `Int16Array` buffers of equal length. */
  drain(left: Int16Array, right: Int16Array): number {
    return this.#active().drain(left, right);
  }

  /** Renders a mono tail block and returns its frame count. */
  drainMono(audio: Int16Array): number {
    return this.#active().drainMono(audio);
  }

  /** Returns a new feedback object and does not clear the peak window. */
  feedback(): Feedback {
    return this.#active().feedback();
  }

  /** Returns the feedback and clears only the stereo peak window. */
  takeFeedback(): Feedback {
    return this.#active().takeFeedback();
  }

  /** Returns a new telemetry object and does not clear its peak windows. */
  telemetry(): Telemetry {
    return this.#active().telemetry();
  }

  /** Returns the telemetry and clears only its three peak windows. */
  takeTelemetry(): Telemetry {
    return this.#active().takeTelemetry();
  }

  /** Writes the formant envelopes into two `Float32Array` buffers. */
  formantEnvelope(
    maxFrequencyHz: number,
    input: Float32Array,
    output: Float32Array,
  ): boolean {
    return this.#active().formantEnvelope(maxFrequencyHz, input, output);
  }

  /** Resets the audio histories and feedback. */
  reset(): void {
    this.#active().reset();
  }
}

if (typeof Symbol.dispose === "symbol") {
  Tunelet.prototype[Symbol.dispose] = Tunelet.prototype.free;
}

/** The default export is the same function as `loadTunelet`. */
export default loadTunelet;
