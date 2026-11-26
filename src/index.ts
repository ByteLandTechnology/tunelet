/**
 * The shared ESM entry point for browsers and Node.js.
 *
 * Call `loadTunelet()` or `initSync()` before you construct processors or read
 * the default configuration. Both environments use the same generated bindings
 * and the same Wasm binary. This module exports `TuneletAudioViews`, wraps
 * `Tunelet`, `defaultConfig` and `initSync` with lifecycle checks, and
 * re-exports the configuration, control-activity, event, feedback and
 * telemetry types.
 */
import init, {
  Tunelet as WasmTunelet,
  defaultConfig as wasmDefaultConfig,
  initSync as wasmInitSync,
} from "../pkg/tunelet.js";
import type {
  BlockEvents,
  ControlActivity,
  Feedback,
  InitInput,
  InitOutput,
  ProcessorConfig,
  SyncInitInput,
  Telemetry,
} from "../pkg/tunelet.js";

export * from "../pkg/tunelet.js";

/**
 * Cached `Float32Array` views over the in-place `Tunelet` buffers in Wasm
 * linear memory.
 *
 * Each access to `input`, `left` or `right` checks `memory.buffer`. If Wasm
 * memory grew and detached the previous `ArrayBuffer`, the helper binds the
 * three fixed pointers of `processor` to the new buffer. If `memory.buffer` did
 * not change, the helper returns the cached views with no allocation.
 *
 * Read `input` before you write input samples. If an in-place call passes
 * `events`, read `left` and `right` (or `input` for mono) after that call.
 * When you call `processor.free()`, also call `dispose()`. Do not use the views
 * after `processor.free()`.
 */
export class TuneletAudioViews {
  private readonly memory: InitOutput["memory"];
  private processor: Tunelet | null;
  /** The fixed frame capacity of `input`, `left` and `right`. */
  readonly maxBlockFrames: number;
  private cachedBuffer: ArrayBuffer | null = null;
  private inputView: Float32Array | null = null;
  private leftView: Float32Array | null = null;
  private rightView: Float32Array | null = null;

  /**
   * Binds `Float32Array` views for `processor` over `memory`.
   *
   * @param memory The Wasm memory from `InitOutput["memory"]`.
   * @param processor The live `Tunelet` instance that owns the buffers.
   */
  constructor(memory: InitOutput["memory"], processor: Tunelet) {
    this.memory = memory;
    this.processor = processor;
    this.maxBlockFrames = processor.maxBlockFrames;
    this.refresh();
  }

  /** Returns `true` after `dispose()` clears the cached views. */
  get disposed(): boolean {
    return this.processor === null;
  }

  /**
   * Returns the `Float32Array` view for mono input and in-place mono output.
   *
   * If `memory.buffer` changed, this getter refreshes all three views first.
   */
  get input(): Float32Array {
    this.refresh();
    return this.inputView!;
  }

  /**
   * Returns the `Float32Array` view for the left stereo output channel.
   *
   * If `memory.buffer` changed, this getter refreshes all three views first.
   */
  get left(): Float32Array {
    this.refresh();
    return this.leftView!;
  }

  /**
   * Returns the `Float32Array` view for the right stereo output channel.
   *
   * If `memory.buffer` changed, this getter refreshes all three views first.
   */
  get right(): Float32Array {
    this.refresh();
    return this.rightView!;
  }

  /**
   * If Wasm memory grew, this method refreshes the three `Float32Array` views
   * from `memory.buffer`.
   */
  refresh(): void {
    const processor = this.processor;
    if (processor === null) {
      throw new Error("TuneletAudioViews is disposed");
    }
    const buffer = this.memory.buffer;
    if (this.cachedBuffer !== buffer) {
      const frames = this.maxBlockFrames;
      const inputPtr = processor.inputF32Ptr;
      const leftPtr = processor.leftF32Ptr;
      const rightPtr = processor.rightF32Ptr;
      const input = new Float32Array(buffer, inputPtr, frames);
      const left = new Float32Array(buffer, leftPtr, frames);
      const right = new Float32Array(buffer, rightPtr, frames);
      this.inputView = input;
      this.leftView = left;
      this.rightView = right;
      this.cachedBuffer = buffer;
    }
  }

  /** Clears the cached `ArrayBuffer`, `Float32Array` views and processor link. */
  dispose(): void {
    this.processor = null;
    this.cachedBuffer = null;
    this.inputView = null;
    this.leftView = null;
    this.rightView = null;
  }
}

export interface TuneletAudioViews {
  [Symbol.dispose](): void;
}

if (Symbol.dispose) {
  TuneletAudioViews.prototype[Symbol.dispose] =
    TuneletAudioViews.prototype.dispose;
}

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
 * - A URL or URL string.
 * - A fetch Response.
 * - A byte buffer (BufferSource).
 * - A compiled WebAssembly.Module.
 * - A promise for one of these.
 *
 * In Node.js, the function reads `file:` URL objects and `"file:"` URL strings
 * from disk. Other URLs and string sources obey fetch rules. A compiled module
 * must use imports that agree with the packaged bindings.
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

/** Converts a `file:` URL string or `URL` object into a local `URL` instance. */
function resolveFileUrl(input: InitInput): URL | undefined {
  if (typeof URL === "function" && input instanceof URL) {
    return input.protocol === "file:" ? input : undefined;
  }
  if (typeof input === "string") {
    if (!/^file:/i.test(input)) {
      return undefined;
    }
    const url = new URL(input);
    return url.protocol === "file:" ? url : undefined;
  }
  if (
    typeof input === "object" &&
    input !== null &&
    "protocol" in input &&
    input.protocol === "file:" &&
    "href" in input &&
    typeof input.href === "string"
  ) {
    const url = new URL(input.href);
    return url.protocol === "file:" ? url : undefined;
  }
  return undefined;
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
  const fileUrl = resolveFileUrl(input);
  if (fileUrl !== undefined) {
    // Keep the Node.js built-in module out of browser bundles. TypeScript
    // removes the type-only import. The runtime import runs only for `file:`
    // URLs.
    const moduleName = "node:fs/promises";
    const fs: typeof import("node:fs/promises") = await import(
      /* webpackIgnore: true */ /* @vite-ignore */ moduleName
    );
    output = await init({ module_or_path: await fs.readFile(fileUrl) });
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
 * A stateful vocal processor for mono `Int16Array` PCM and `Float32Array`
 * audio.
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
   * from 1 to 4294967295 (inclusive). If you omit `config`, the processor uses
   * the default configuration.
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

  /**
   * The byte offset in Wasm memory of the `maxBlockFrames` `f32` input buffer.
   */
  get inputF32Ptr(): number {
    return active(this).inputF32Ptr;
  }

  /**
   * The byte offset in Wasm memory of the `maxBlockFrames` left `f32` output
   * buffer.
   */
  get leftF32Ptr(): number {
    return active(this).leftF32Ptr;
  }

  /**
   * The byte offset in Wasm memory of the `maxBlockFrames` right `f32` output
   * buffer.
   */
  get rightF32Ptr(): number {
    return active(this).rightF32Ptr;
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

  /**
   * Processes mono `Float32Array` input into left and right `Float32Array`
   * buffers.
   */
  processF32(
    input: Float32Array,
    left: Float32Array,
    right: Float32Array,
    events?: BlockEvents | null,
  ): void {
    active(this).processF32(input, left, right, events);
  }

  /** Replaces the audio in a mono `Float32Array` with the stereo downmix. */
  processMonoF32(audio: Float32Array, events?: BlockEvents | null): void {
    active(this).processMonoF32(audio, events);
  }

  /**
   * Processes `frames` samples from `inputF32Ptr` into `leftF32Ptr` and
   * `rightF32Ptr`.
   */
  processF32InPlace(frames: number, events?: BlockEvents | null): void {
    active(this).processF32InPlace(frames, events);
  }

  /**
   * Replaces `frames` samples at `inputF32Ptr` with the mono `f32` downmix.
   */
  processMonoF32InPlace(frames: number, events?: BlockEvents | null): void {
    active(this).processMonoF32InPlace(frames, events);
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

  /**
   * Renders stereo tail frames into `Float32Array` buffers of equal length.
   */
  drainF32(left: Float32Array, right: Float32Array): number {
    return active(this).drainF32(left, right);
  }

  /**
   * Renders a mono `Float32Array` tail block and returns its frame count.
   */
  drainMonoF32(audio: Float32Array): number {
    return active(this).drainMonoF32(audio);
  }

  /**
   * Renders up to `frames` stereo tail frames into `leftF32Ptr` and
   * `rightF32Ptr`.
   */
  drainF32InPlace(frames: number): number {
    return active(this).drainF32InPlace(frames);
  }

  /**
   * Renders up to `frames` mono tail frames into `inputF32Ptr` and returns
   * their count.
   */
  drainMonoF32InPlace(frames: number): number {
    return active(this).drainMonoF32InPlace(frames);
  }

  /** Returns a new feedback object and does not clear the peak window. */
  feedback(): Feedback {
    return active(this).feedback();
  }

  /** Returns the feedback and clears only the peak window. */
  takeFeedback(): Feedback {
    return active(this).takeFeedback();
  }

  /**
   * Returns which choir, harmony and reverb controls change the audio in the
   * active configuration.
   */
  controlActivity(): ControlActivity {
    return active(this).controlActivity();
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
