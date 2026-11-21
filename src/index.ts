/**
 * The shared ESM entry point for browsers and Node.js.
 *
 * Call `loadTunelet()` before you construct processors or read the default
 * configuration. For an `AudioWorklet`, call `loadTuneletWasm()` on the main
 * thread and pass the result to `initSync({ module })` or use
 * `tunelet/worklet`. Both environments use the same generated bindings and the
 * same Wasm binary. This module re-exports all named exports of the module
 * that wasm-bindgen generates, which include `Tunelet`, `defaultConfig`,
 * `initSync` and the configuration, event and feedback types.
 */
import "./text-codec-shim.js";
import init, { Tunelet } from "../pkg/tunelet.js";
import type {
  BlockEvents,
  Feedback,
  InitInput,
  InitOutput,
  ProcessorConfig,
  Telemetry,
} from "../pkg/tunelet.js";

export * from "../pkg/tunelet.js";

/** Scale factor from signed 16-bit PCM to normalized Float32 audio. */
const INV_PCM_SCALE = 1.0 / 32768.0;

/** Shared empty buffers so zero-frame calls allocate no typed arrays. */
const EMPTY_INPUT = new Int16Array(0);
const EMPTY_LEFT = new Int16Array(0);
const EMPTY_RIGHT = new Int16Array(0);

/** Error text for fetch-based Wasm calls inside `AudioWorkletGlobalScope`. */
const WORKLET_FETCH_ERROR =
  "loadTunelet() cannot fetch Wasm in AudioWorkletGlobalScope. Load the binary on the main thread with loadTuneletWasm(), then call initSync({ module }) or use tunelet/worklet.";

/** The shared initialization promise, which a failure clears. */
let initialization: Promise<InitOutput> | undefined;

/** The shared default Wasm binary promise, which a failure clears. */
let defaultWasmBinary: Promise<ArrayBuffer> | undefined;

/** Returns `true` if the current global object is an `AudioWorkletGlobalScope`. */
function isAudioWorkletScope(): boolean {
  const scope = globalThis as Record<string, unknown>;
  const workletCtor = scope.AudioWorkletGlobalScope;
  if (
    typeof workletCtor === "function" &&
    globalThis instanceof (workletCtor as Function)
  ) {
    return true;
  }
  return (
    typeof scope.registerProcessor === "function" &&
    typeof scope.AudioWorkletProcessor === "function"
  );
}

/** Copies an `ArrayBufferView` into an independent `ArrayBuffer`. */
function copyViewToArrayBuffer(view: ArrayBufferView): ArrayBuffer {
  const bytes = new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  return copy.buffer;
}

/** Reads a Node.js `file:` URL into a byte view. */
async function readFileUrlBuffer(url: URL): Promise<Uint8Array> {
  // Keep the Node.js built-in module out of browser bundles. TypeScript
  // removes the type-only import. The runtime import runs only for `file:`
  // URLs.
  const moduleName = "node:fs/promises";
  const fs: typeof import("node:fs/promises") = await import(
    /* webpackIgnore: true */ /* @vite-ignore */ moduleName
  );
  return fs.readFile(url);
}

/** Reads a Node.js `file:` URL into an `ArrayBuffer`. */
async function readFileUrlBytes(url: URL): Promise<ArrayBuffer> {
  return copyViewToArrayBuffer(await readFileUrlBuffer(url));
}

/** Returns `true` if `input` requires `fetch` in non-file environments. */
function isFetchSource(input: unknown): boolean {
  if (typeof input === "string") {
    return true;
  }
  if (input instanceof URL) {
    return input.protocol !== "file:";
  }
  return typeof Request === "function" && input instanceof Request;
}

/**
 * Initializes the Wasm module one time for this imported package instance.
 *
 * If you give no argument, the function finds the packaged binary relative to
 * this module. Node.js reads a `file:` URL, and browsers fetch the served
 * asset. In `AudioWorkletGlobalScope`, a default call rejects with an error
 * because `fetch` is not available. An import of the package does not load
 * Wasm. In browsers, serve the assets over HTTP(S), or give the bytes or a
 * compiled module as the source.
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
 * @returns The initialized Wasm exports. Use `Tunelet` or `TuneletFloat32` for
 * processors.
 */
export function loadTunelet(
  source?: InitInput | Promise<InitInput>,
): Promise<InitOutput> {
  if (
    isAudioWorkletScope() &&
    (source === undefined || isFetchSource(source))
  ) {
    return Promise.reject(new Error(WORKLET_FETCH_ERROR));
  }
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

  if (input instanceof URL && input.protocol === "file:") {
    return init({ module_or_path: await readFileUrlBuffer(input) });
  }

  if (
    isAudioWorkletScope() &&
    (typeof input === "string" ||
      input instanceof URL ||
      (typeof Request === "function" && input instanceof Request))
  ) {
    throw new Error(WORKLET_FETCH_ERROR);
  }

  return init({ module_or_path: input });
}

/** Checks if a value is a Fetch API `Response` instance. */
function isFetchResponse(value: unknown): value is Response {
  return typeof Response === "function" && value instanceof Response;
}

/**
 * Resolves the Wasm binary into an `ArrayBuffer` or a `WebAssembly.Module`.
 *
 * Run this function on the main thread or in Node.js to load the Wasm asset
 * without `fetch` inside an `AudioWorkletGlobalScope`. Then pass the result to
 * `initSync({ module })` or to `TuneletWorkletNode`.
 *
 * If you omit `source`, the function loads the packaged `tunelet_bg.wasm`
 * asset one time and caches that `ArrayBuffer`. If that load fails, the
 * function clears the cached promise so a later call can try again.
 *
 * @param source The optional Wasm source. The default is the binary in this
 * package.
 * @returns An `ArrayBuffer` of Wasm bytes or a compiled `WebAssembly.Module`.
 */
export function loadTuneletWasm(): Promise<ArrayBuffer>;
export function loadTuneletWasm(
  source: WebAssembly.Module | Promise<WebAssembly.Module>,
): Promise<WebAssembly.Module>;
export function loadTuneletWasm(
  source?: InitInput | Promise<InitInput>,
): Promise<ArrayBuffer | WebAssembly.Module>;
export function loadTuneletWasm(
  source?: InitInput | Promise<InitInput>,
): Promise<ArrayBuffer | WebAssembly.Module> {
  if (source === undefined) {
    if (isAudioWorkletScope()) {
      return Promise.reject(new Error(WORKLET_FETCH_ERROR));
    }
    defaultWasmBinary ??= resolveWasmSource(undefined).catch(
      (error: unknown) => {
        defaultWasmBinary = undefined;
        throw error;
      },
    ) as Promise<ArrayBuffer>;
    return defaultWasmBinary;
  }
  return resolveWasmSource(source);
}

/** Resolves one `InitInput` value into an `ArrayBuffer` or `WebAssembly.Module`. */
async function resolveWasmSource(
  source: InitInput | Promise<InitInput> | undefined,
): Promise<ArrayBuffer | WebAssembly.Module> {
  const input =
    source === undefined
      ? new URL("../pkg/tunelet_bg.wasm", import.meta.url)
      : await source;

  if (input instanceof WebAssembly.Module) {
    return input;
  }
  if (input instanceof ArrayBuffer) {
    return input;
  }
  if (ArrayBuffer.isView(input)) {
    return copyViewToArrayBuffer(input);
  }
  if (isFetchResponse(input)) {
    if (!input.ok) {
      throw new Error(
        `Failed to fetch Wasm: ${input.status} ${input.statusText}`,
      );
    }
    return input.arrayBuffer();
  }
  if (input instanceof URL && input.protocol === "file:") {
    return readFileUrlBytes(input);
  }
  if (isAudioWorkletScope() || typeof fetch !== "function") {
    throw new Error(WORKLET_FETCH_ERROR);
  }

  const response = await fetch(input);
  if (!response.ok) {
    throw new Error(
      `Failed to fetch Wasm: ${response.status} ${response.statusText}`,
    );
  }
  return response.arrayBuffer();
}

/** Checks that two channel lengths are equal. */
function checkBufferLength(expected: number, actual: number): void {
  if (expected !== actual) {
    throw new Error(`buffer length is ${actual}, expected ${expected}`);
  }
}

/** Checks that two output `Float32Array` views do not share memory ranges. */
function checkDisjointOutputs(left: Float32Array, right: Float32Array): void {
  if (
    left.byteLength > 0 &&
    left.buffer === right.buffer &&
    left.byteOffset < right.byteOffset + right.byteLength &&
    right.byteOffset < left.byteOffset + left.byteLength
  ) {
    throw new Error("left and right output buffers must not overlap");
  }
}

/** Converts normalized `Float32Array` samples into signed 16-bit PCM. */
function convertFloat32ToInt16(
  source: Float32Array,
  target: Int16Array,
  frames: number,
): void {
  for (let index = 0; index < frames; index += 1) {
    const sample = source[index];
    if (sample !== sample) {
      target[index] = 0;
    } else {
      const clamped = sample < -1.0 ? -1.0 : sample > 1.0 ? 1.0 : sample;
      const scaled = Math.round(clamped * 32768.0);
      target[index] = scaled > 32767 ? 32767 : scaled;
    }
  }
}

/** Scales signed 16-bit PCM samples into `Float32Array` output. */
function convertInt16ToFloat32(
  source: Int16Array,
  target: Float32Array,
  frames: number,
): void {
  for (let index = 0; index < frames; index += 1) {
    target[index] = source[index] * INV_PCM_SCALE;
  }
}

/**
 * A `Float32Array` adapter around `Tunelet` for Web Audio and Node.js.
 *
 * Call `loadTunelet()` or `initSync({ module })` before you construct this
 * class. The constructor preallocates three `Int16Array` conversion buffers
 * (`input`, `left` and `right`) of length `maxBlockFrames`.
 *
 * Audio methods map `NaN` to zero, clamp inputs to `-1.0..=1.0`, delegate to
 * `Tunelet` and multiply outputs by `1.0 / 32768.0`. Separate stereo output
 * arrays must not overlap. Input can overlap an output. If a call fails, it
 * throws before the caller buffers or the DSP state change.
 *
 * This adapter allocates no new JavaScript typed arrays during steady-state
 * audio calls. The generated wasm-bindgen glue can still make temporary views
 * on Wasm memory during copies, and object, event, feedback, telemetry and
 * error conversions can allocate memory. Call `free()` when you do not need
 * the instance any more.
 */
export class TuneletFloat32 {
  readonly #inner: Tunelet;
  readonly #maxBlockFrames: number;
  readonly #input: Int16Array;
  readonly #left: Int16Array;
  readonly #right: Int16Array;
  #partialFrames = 0;
  #partialInput: Int16Array = EMPTY_INPUT;
  #partialLeft: Int16Array = EMPTY_LEFT;
  #partialRight: Int16Array = EMPTY_RIGHT;

  /**
   * Prepares a `Tunelet` processor and preallocates `Int16Array` conversion
   * buffers.
   *
   * `sampleRate` must be 44100 or 48000. `maxBlockFrames` must be an integer
   * in `1..=4294967295`. If you omit `config`, the processor uses the default
   * configuration.
   */
  constructor(
    sampleRate: number,
    maxBlockFrames: number,
    config?: ProcessorConfig | null,
  ) {
    const inner = new Tunelet(sampleRate, maxBlockFrames, config);
    try {
      this.#input = new Int16Array(maxBlockFrames);
      this.#left = new Int16Array(maxBlockFrames);
      this.#right = new Int16Array(maxBlockFrames);
    } catch (error) {
      inner.free();
      throw error;
    }
    this.#inner = inner;
    this.#maxBlockFrames = maxBlockFrames;
  }

  /** The sample rate of the input and the output in hertz. */
  get sampleRate(): number {
    return this.#inner.sampleRate;
  }

  /** The maximum frame count for each process or drain call. */
  get maxBlockFrames(): number {
    return this.#inner.maxBlockFrames;
  }

  /** The nominal resynthesis delay in frames. */
  get latencyFrames(): number {
    return this.#inner.latencyFrames;
  }

  /** Returns a copy of the active configuration. */
  config(): ProcessorConfig {
    return this.#inner.config();
  }

  /** Validates and replaces the complete configuration. */
  setConfig(config: ProcessorConfig): void {
    this.#inner.setConfig(config);
  }

  /**
   * Processes mono `Float32Array` input into planar stereo `Float32Array`
   * outputs of equal length.
   */
  process(
    input: Float32Array,
    left: Float32Array,
    right: Float32Array,
    events?: BlockEvents | null,
  ): void {
    const frames = input.length;
    checkBufferLength(frames, left.length);
    checkBufferLength(frames, right.length);
    this.#checkFrames(frames);
    checkDisjointOutputs(left, right);

    const [pcmInput, pcmLeft, pcmRight] = this.#viewsForFrames(frames);
    convertFloat32ToInt16(input, pcmInput, frames);
    this.#inner.process(pcmInput, pcmLeft, pcmRight, events);
    convertInt16ToFloat32(pcmLeft, left, frames);
    convertInt16ToFloat32(pcmRight, right, frames);
  }

  /**
   * Replaces mono `Float32Array` audio in place with the stereo downmix.
   */
  processMono(audio: Float32Array, events?: BlockEvents | null): void {
    const frames = audio.length;
    this.#checkFrames(frames);

    const pcmAudio = this.#monoViewForFrames(frames);
    convertFloat32ToInt16(audio, pcmAudio, frames);
    this.#inner.processMono(pcmAudio, events);
    convertInt16ToFloat32(pcmAudio, audio, frames);
  }

  /** Starts a finite tail budget and returns its frame count. */
  beginDrain(): number {
    return this.#inner.beginDrain();
  }

  /** Returns the configured tail budget in frames. */
  tailFrames(): number {
    return this.#inner.tailFrames();
  }

  /**
   * Renders stereo tail frames into `Float32Array` buffers of equal length and
   * returns the rendered frame count.
   */
  drain(left: Float32Array, right: Float32Array): number {
    const frames = left.length;
    checkBufferLength(frames, right.length);
    this.#checkFrames(frames);
    checkDisjointOutputs(left, right);

    const [, pcmLeft, pcmRight] = this.#viewsForFrames(frames);
    const rendered = this.#inner.drain(pcmLeft, pcmRight);
    convertInt16ToFloat32(pcmLeft, left, frames);
    convertInt16ToFloat32(pcmRight, right, frames);
    return rendered;
  }

  /**
   * Renders mono tail frames into a `Float32Array` buffer and returns the
   * rendered frame count.
   */
  drainMono(audio: Float32Array): number {
    const frames = audio.length;
    this.#checkFrames(frames);

    const pcmAudio = this.#monoViewForFrames(frames);
    const rendered = this.#inner.drainMono(pcmAudio);
    convertInt16ToFloat32(pcmAudio, audio, frames);
    return rendered;
  }

  /** Returns a feedback snapshot and does not clear the peak window. */
  feedback(): Feedback {
    return this.#inner.feedback();
  }

  /** Returns a feedback snapshot and clears the stereo peak window. */
  takeFeedback(): Feedback {
    return this.#inner.takeFeedback();
  }

  /** Returns a telemetry snapshot and does not clear its peak windows. */
  telemetry(): Telemetry {
    return this.#inner.telemetry();
  }

  /** Returns a telemetry snapshot and clears its three peak windows. */
  takeTelemetry(): Telemetry {
    return this.#inner.takeTelemetry();
  }

  /** Writes the analysis and synthesis formant envelopes into two buffers. */
  formantEnvelope(
    maxFrequencyHz: number,
    input: Float32Array,
    output: Float32Array,
  ): boolean {
    return this.#inner.formantEnvelope(maxFrequencyHz, input, output);
  }

  /** Resets the audio histories and feedback, and cancels any active drain. */
  reset(): void {
    this.#inner.reset();
  }

  /** Releases the owned Wasm processor memory. */
  free(): void {
    this.#inner.free();
  }

  /** Releases the owned Wasm processor memory for `Symbol.dispose`. */
  [Symbol.dispose](): void {
    this.free();
  }

  /** Rejects blocks that exceed `maxBlockFrames`. */
  #checkFrames(frames: number): void {
    if (frames > this.#maxBlockFrames) {
      throw new Error(
        `block of ${frames} frames is larger than the prepared ${this.#maxBlockFrames}`,
      );
    }
  }

  /** Returns reusable `Int16Array` views of length `frames`. */
  #viewsForFrames(frames: number): [Int16Array, Int16Array, Int16Array] {
    if (frames === this.#maxBlockFrames) {
      return [this.#input, this.#left, this.#right];
    }
    if (frames === 0) {
      return [EMPTY_INPUT, EMPTY_LEFT, EMPTY_RIGHT];
    }
    if (this.#partialFrames !== frames) {
      this.#partialInput = this.#input.subarray(0, frames);
      this.#partialLeft = this.#left.subarray(0, frames);
      this.#partialRight = this.#right.subarray(0, frames);
      this.#partialFrames = frames;
    }
    return [this.#partialInput, this.#partialLeft, this.#partialRight];
  }

  /** Returns a reusable mono `Int16Array` view of length `frames`. */
  #monoViewForFrames(frames: number): Int16Array {
    if (frames === this.#maxBlockFrames) {
      return this.#input;
    }
    if (frames === 0) {
      return EMPTY_INPUT;
    }
    if (this.#partialFrames !== frames) {
      this.#partialInput = this.#input.subarray(0, frames);
      this.#partialLeft = this.#left.subarray(0, frames);
      this.#partialRight = this.#right.subarray(0, frames);
      this.#partialFrames = frames;
    }
    return this.#partialInput;
  }
}

/** The default export is the same function as `loadTunelet`. */
export default loadTunelet;
