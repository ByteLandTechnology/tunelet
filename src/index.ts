/**
 * The shared ESM entry point for browsers and Node.js.
 *
 * Call `loadTunelet()` or `initSync()` before you construct processors or read
 * the default configuration. Both environments use the same generated bindings
 * and the same Wasm binary. This module wraps `Tunelet`, `defaultConfig` and
 * `initSync` with lifecycle checks, and re-exports the configuration, event,
 * feedback and telemetry types.
 */
import init, {
  Tunelet as WasmTunelet,
  defaultConfig as wasmDefaultConfig,
  initSync as wasmInitSync,
} from "../pkg/tunelet.js";
import type {
  BlockEvents,
  Feedback,
  InitInput,
  InitOutput,
  ProcessorConfig,
  SyncInitInput,
  Telemetry,
} from "../pkg/tunelet.js";

export * from "../pkg/tunelet.js";

/** The shared initialization promise, which a failure clears. */
let initialization: Promise<InitOutput> | undefined;

/** True after the Wasm module completes initialization. */
let initialized = false;

/** Maps each active wrapper instance to its Wasm processor. */
const instances = new WeakMap<Tunelet, WasmTunelet>();

/** Throws if the Wasm module is not initialized yet. */
function ensureInitialized(): void {
  if (initialized) {
    return;
  }
  try {
    wasmInitSync({ module: undefined as never });
    initialized = true;
  } catch {
    throw new Error(
      "Tunelet Wasm is not initialized. Call loadTunelet() or initSync() first.",
    );
  }
}

/** Returns the active Wasm processor, or throws if it was freed. */
function active(instance: Tunelet): WasmTunelet {
  const inner = instances.get(instance);
  if (inner === undefined) {
    throw new Error(
      "Tunelet instance is disposed. Do not use it after free().",
    );
  }
  return inner;
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
 * Concurrent calls share one promise, and the first call selects the source.
 * After a successful initialization, later calls use it again and do not load
 * Wasm again, whatever their arguments are. If an attempt fails, the promise
 * rejects and the function clears the cached promise. Then you can try again.
 * File, network and Wasm errors go to the caller. The function does not try a
 * different load method after an error.
 *
 * @param source The optional Wasm source. The default is the binary in this
 * package.
 * @returns The initialized Wasm exports. Use the exported `Tunelet` class for
 * processors.
 */
export function loadTunelet(
  source?: InitInput | Promise<InitInput>,
): Promise<InitOutput> {
  initialization ??= initialize(source).catch((error: unknown) => {
    initialization = undefined;
    throw error;
  });
  return initialization;
}

/** Awaits the source one time, then initializes the Wasm module from it. */
async function initialize(
  source: InitInput | Promise<InitInput> | undefined,
): Promise<InitOutput> {
  const input =
    source === undefined
      ? new URL("../pkg/tunelet_bg.wasm", import.meta.url)
      : await source;

  let output: InitOutput;
  if (input instanceof URL && input.protocol === "file:") {
    // Keep the Node.js built-in module out of browser bundles. TypeScript
    // removes the type-only import. The runtime import runs only for `file:`
    // URLs.
    const moduleName = "node:fs/promises";
    const fs: typeof import("node:fs/promises") = await import(
      /* webpackIgnore: true */ /* @vite-ignore */ moduleName
    );
    output = await init({ module_or_path: await fs.readFile(input) });
  } else {
    output = await init({ module_or_path: input });
  }

  initialized = true;
  return output;
}

/**
 * Instantiates the Wasm module synchronously from bytes or a compiled
 * `WebAssembly.Module`.
 *
 * If the module is already initialized, later calls return the active exports
 * and do not load Wasm again.
 */
export function initSync(
  module: { module: SyncInitInput } | SyncInitInput,
): InitOutput {
  const output = wasmInitSync(module);
  initialized = true;
  initialization ??= Promise.resolve(output);
  return output;
}

/**
 * Returns a complete, independent default configuration for JavaScript.
 *
 * Call `loadTunelet()` or `initSync()` before this function. Change the fields
 * of the returned object and give it to `Tunelet` or to `setConfig`.
 */
export function defaultConfig(): ProcessorConfig {
  ensureInitialized();
  return wasmDefaultConfig();
}

export interface Tunelet {
  [Symbol.dispose](): void;
}

/**
 * A stateful PCM processor for signed 16-bit mono input.
 *
 * Call `loadTunelet()` or `initSync()` before you construct an instance. If
 * you do not need the instance any more, call `free()`. Duplicate `free()`
 * calls do no work. After `free()`, method and property calls throw an
 * `Error`.
 */
export class Tunelet {
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
    instances.set(this, new WasmTunelet(sampleRate, maxBlockFrames, config));
  }

  /** Releases the Wasm processor memory. Duplicate calls do no work. */
  free(): void {
    const inner = instances.get(this);
    if (inner === undefined) {
      return;
    }
    instances.delete(this);
    inner.free();
  }

  /** The sample rate of the input and the output in hertz. */
  get sampleRate(): number {
    return active(this).sampleRate;
  }

  /** The maximum frame count for each call that processes or drains audio. */
  get maxBlockFrames(): number {
    return active(this).maxBlockFrames;
  }

  /** The nominal resynthesis latency: 1,593 frames at both sample rates. */
  get latencyFrames(): number {
    return active(this).latencyFrames;
  }

  /** Returns a copy of the active configuration as a new JavaScript object. */
  config(): ProcessorConfig {
    return active(this).config();
  }

  /** Validates a configuration and replaces the complete configuration. */
  setConfig(config: ProcessorConfig): void {
    active(this).setConfig(config);
  }

  /** Processes mono input into left and right `Int16Array` buffers. */
  process(
    input: Int16Array,
    left: Int16Array,
    right: Int16Array,
    events?: BlockEvents | null,
  ): void {
    active(this).process(input, left, right, events);
  }

  /** Replaces the audio in a mono `Int16Array` with the stereo downmix. */
  processMono(audio: Int16Array, events?: BlockEvents | null): void {
    active(this).processMono(audio, events);
  }

  /** Starts a finite tail budget and returns its frame count. */
  beginDrain(): number {
    return active(this).beginDrain();
  }

  /** Returns the tail budget of the current configuration. */
  tailFrames(): number {
    return active(this).tailFrames();
  }

  /** Renders stereo tail frames into `Int16Array` buffers of equal length. */
  drain(left: Int16Array, right: Int16Array): number {
    return active(this).drain(left, right);
  }

  /** Renders a mono tail block and returns its frame count. */
  drainMono(audio: Int16Array): number {
    return active(this).drainMono(audio);
  }

  /** Returns a new feedback object and does not clear the peak window. */
  feedback(): Feedback {
    return active(this).feedback();
  }

  /** Returns the feedback and clears only the peak window. */
  takeFeedback(): Feedback {
    return active(this).takeFeedback();
  }

  /** Returns a new telemetry object and does not clear its peak windows. */
  telemetry(): Telemetry {
    return active(this).telemetry();
  }

  /** Returns the telemetry and clears only its three peak windows. */
  takeTelemetry(): Telemetry {
    return active(this).takeTelemetry();
  }

  /** Writes the formant envelopes into two `Float32Array` buffers. */
  formantEnvelope(
    maxFrequencyHz: number,
    input: Float32Array,
    output: Float32Array,
  ): boolean {
    return active(this).formantEnvelope(maxFrequencyHz, input, output);
  }

  /** Resets the audio histories and feedback. */
  reset(): void {
    active(this).reset();
  }
}

if (typeof Symbol.dispose === "symbol") {
  Tunelet.prototype[Symbol.dispose] = Tunelet.prototype.free;
}

/** The default export is the same function as `loadTunelet`. */
export default loadTunelet;
