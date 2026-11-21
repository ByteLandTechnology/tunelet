/**
 * The shared ESM entry point for browsers and Node.js.
 *
 * Call `loadTunelet()` or `initSync()` before you construct processors or read
 * the default configuration. Both environments use the same generated bindings
 * and the same Wasm binary. This module exports `Tunelet`, `TuneletError`,
 * `defaultConfig`, `initSync`, `loadTunelet` and the public TypeScript types.
 */
import rawInit, {
  Tunelet as RawTunelet,
  defaultConfig as rawDefaultConfig,
  initSync as rawInitSync,
} from "../pkg/tunelet.js";
import type {
  AlienConfig,
  BufferName,
  ChordDegree,
  ChordName,
  ConfigField,
  ConfigProblemKind,
  ControlEvent,
  CorrectionConfig,
  CorrectionScale,
  ErrorDetails,
  EventProblemKind,
  Feedback,
  FormantConfig,
  HarmonyConfig,
  HarmonyScale,
  HarmonySource,
  InitInput,
  InitOutput,
  InputTarget,
  Inversion,
  MidiEvent,
  MidiProblemKind,
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
  BufferName,
  ChordDegree,
  ChordName,
  ConfigField,
  ConfigProblemKind,
  ControlEvent,
  CorrectionConfig,
  CorrectionScale,
  ErrorDetails,
  EventProblemKind,
  Feedback,
  FormantConfig,
  HarmonyConfig,
  HarmonyScale,
  HarmonySource,
  InitInput,
  InitOutput,
  InputTarget,
  Inversion,
  MidiEvent,
  MidiProblemKind,
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

/**
 * A structured error that `Tunelet`, `defaultConfig`, `initSync` or
 * `loadTunelet` throws.
 *
 * Every failure exposes both a plain `camelCase` `message` and typed
 * diagnostic fields on `details`.
 */
export class TuneletError extends Error {
  /** Machine-readable details for the rejected operation. */
  readonly details: ErrorDetails;

  /** The optional underlying cause of this error. */
  readonly cause?: unknown;

  /**
   * Constructs a `TuneletError` with a message and structured `details`.
   *
   * @param message The human-readable error message.
   * @param details The structured diagnostic payload.
   * @param options Optional settings such as `cause`.
   */
  constructor(
    message: string,
    details: ErrorDetails,
    options?: { readonly cause?: unknown },
  ) {
    super(message);
    this.name = "TuneletError";
    this.details = details;
    if (options !== undefined && "cause" in options) {
      this.cause = options.cause;
    }
    Object.setPrototypeOf(this, new.target.prototype);
  }
}

/** The packaged Wasm URL relative to this module. */
const DEFAULT_WASM_URL = new URL("../pkg/tunelet_bg.wasm", import.meta.url);

/** The required export names and kinds on a valid Tunelet Wasm module. */
const REQUIRED_WASM_EXPORTS: ReadonlyArray<readonly [string, WebAssembly.ImportExportKind]> = [
  ["memory", "memory"],
  ["__wbindgen_start", "function"],
  ["__wbg_tunelet_free", "function"],
  ["defaultConfig", "function"],
  ["tunelet_beginDrain", "function"],
  ["tunelet_config", "function"],
  ["tunelet_drain", "function"],
  ["tunelet_drainMono", "function"],
  ["tunelet_feedback", "function"],
  ["tunelet_formantEnvelope", "function"],
  ["tunelet_latencyFrames", "function"],
  ["tunelet_maxBlockFrames", "function"],
  ["tunelet_new", "function"],
  ["tunelet_process", "function"],
  ["tunelet_processMono", "function"],
  ["tunelet_reset", "function"],
  ["tunelet_sampleRate", "function"],
  ["tunelet_setConfig", "function"],
  ["tunelet_tailFrames", "function"],
  ["tunelet_takeFeedback", "function"],
  ["tunelet_takeTelemetry", "function"],
  ["tunelet_telemetry", "function"],
];

const NOT_INITIALIZED_MESSAGE =
  "WebAssembly is not initialized; call await loadTunelet() or initSync() first";
const DISPOSED_MESSAGE = "Tunelet processor is disposed after free()";
const SOURCE_MISMATCH_MESSAGE =
  "WebAssembly source does not match the active Tunelet module";

type SourceDescriptor =
  | { readonly kind: "url"; readonly href: string }
  | { readonly kind: "string"; readonly value: string }
  | { readonly kind: "request"; readonly url: string; readonly ref: Request }
  | { readonly kind: "response"; readonly ref: Response }
  | { readonly kind: "module"; readonly module: WebAssembly.Module }
  | { readonly kind: "bytes"; readonly bytes: Uint8Array }
  | { readonly kind: "invalid"; readonly value: unknown };

type SdkState =
  | { readonly status: "uninitialized" }
  | {
      readonly status: "loading";
      readonly sourcePromise: Promise<SourceDescriptor>;
      readonly initPromise: Promise<InitOutput>;
    }
  | {
      readonly status: "ready";
      readonly source: SourceDescriptor;
      readonly output: InitOutput;
    };

let sdkState: SdkState = { status: "uninitialized" };

/** Ensures WebAssembly initialization is complete before a synchronous call. */
function assertInitialized(): void {
  if (sdkState.status !== "ready") {
    throw new TuneletError(NOT_INITIALIZED_MESSAGE, { type: "notInitialized" });
  }
}

/** Converts an unknown Wasm exception into a `TuneletError`. */
function toTuneletError(
  error: unknown,
  fallbackDetails: ErrorDetails,
): TuneletError {
  if (error instanceof TuneletError) {
    return error;
  }
  if (error instanceof Error) {
    const candidate = (error as Error & { details?: unknown }).details;
    const details = isErrorDetails(candidate) ? candidate : fallbackDetails;
    return new TuneletError(error.message, details, { cause: error });
  }
  return new TuneletError(String(error), fallbackDetails, { cause: error });
}

/** Checks that an attached value has the `type` discriminant of `ErrorDetails`. */
function isErrorDetails(value: unknown): value is ErrorDetails {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as { type?: unknown }).type === "string"
  );
}

/** Returns a `Uint8Array` view over an `ArrayBuffer` or `ArrayBufferView`. */
function asByteView(buffer: BufferSource): Uint8Array {
  if (buffer instanceof ArrayBuffer) {
    return new Uint8Array(buffer);
  }
  return new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength);
}

/** Checks whether two byte views contain identical bytes. */
function bytesEqual(left: Uint8Array, right: Uint8Array): boolean {
  if (left.byteLength !== right.byteLength) {
    return false;
  }
  for (let index = 0; index < left.byteLength; index += 1) {
    if (left[index] !== right[index]) {
      return false;
    }
  }
  return true;
}

/** Checks whether a value is a Fetch API `Response`. */
function isResponse(value: unknown): value is Response {
  return typeof Response === "function" && value instanceof Response;
}

/** Normalizes an initialization input so repeated calls can compare sources. */
function describeSource(input: InitInput | undefined): SourceDescriptor {
  if (input === undefined) {
    return { kind: "url", href: DEFAULT_WASM_URL.href };
  }
  if (input instanceof URL) {
    return { kind: "url", href: input.href };
  }
  if (typeof input === "string") {
    return { kind: "string", value: input };
  }
  if (input instanceof WebAssembly.Module) {
    return { kind: "module", module: input };
  }
  if (input instanceof ArrayBuffer || ArrayBuffer.isView(input)) {
    return { kind: "bytes", bytes: asByteView(input) };
  }
  if (typeof Request === "function" && input instanceof Request) {
    return { kind: "request", url: input.url, ref: input };
  }
  if (isResponse(input)) {
    return { kind: "response", ref: input };
  }
  return { kind: "invalid", value: input };
}

/** Compares two normalized source descriptors for equivalence. */
function sourcesMatch(left: SourceDescriptor, right: SourceDescriptor): boolean {
  if (left.kind === "invalid" || right.kind === "invalid") {
    return false;
  }
  if (left.kind !== right.kind) {
    return false;
  }
  switch (left.kind) {
    case "url":
      return left.href === (right as typeof left).href;
    case "string":
      return left.value === (right as typeof left).value;
    case "request":
      return (
        left.ref === (right as typeof left).ref ||
        left.url === (right as typeof left).url
      );
    case "response":
      return left.ref === (right as typeof left).ref;
    case "module":
      return left.module === (right as typeof left).module;
    case "bytes":
      return bytesEqual(left.bytes, (right as typeof left).bytes);
  }
}

/**
 * Checks that a compiled `WebAssembly.Module` has the exports and import
 * namespace of the packaged Tunelet bindings.
 */
function validateWasmModule(module: WebAssembly.Module): void {
  const exportMap = new Map<string, WebAssembly.ImportExportKind>();
  for (const entry of WebAssembly.Module.exports(module)) {
    exportMap.set(entry.name, entry.kind);
  }
  for (const [name, kind] of REQUIRED_WASM_EXPORTS) {
    if (exportMap.get(name) !== kind) {
      throw new TuneletError(
        `custom WebAssembly module is missing required ${kind} export '${name}'`,
        { type: "sourceMismatch" },
      );
    }
  }
  for (const entry of WebAssembly.Module.imports(module)) {
    if (entry.module !== "./tunelet_bg.js") {
      throw new TuneletError(
        `custom WebAssembly module imports unexpected namespace '${entry.module}'`,
        { type: "sourceMismatch" },
      );
    }
  }
}

/** Compiles bytes to a `WebAssembly.Module` or throws a `sourceMismatch` error. */
async function compileWasmBytes(bytes: BufferSource): Promise<WebAssembly.Module> {
  try {
    const view = asByteView(bytes);
    const copy = new Uint8Array(view.byteLength);
    copy.set(view);
    return await WebAssembly.compile(copy);
  } catch (error: unknown) {
    throw new TuneletError(
      "custom WebAssembly source failed to compile",
      { type: "sourceMismatch" },
      { cause: error },
    );
  }
}

/** Compiles a `Response` body to a `WebAssembly.Module`. */
async function compileWasmResponse(
  response: Response,
): Promise<WebAssembly.Module> {
  if (!response.ok) {
    throw new Error(
      `failed to fetch Wasm: ${response.status} ${response.statusText} for '${response.url}'`,
    );
  }
  if (typeof WebAssembly.compileStreaming === "function") {
    try {
      return await WebAssembly.compileStreaming(response);
    } catch (error: unknown) {
      const canFallback =
        (response.type === "basic" ||
          response.type === "cors" ||
          response.type === "default") &&
        response.headers.get("Content-Type") !== "application/wasm";
      if (!canFallback) {
        throw new TuneletError(
          "custom WebAssembly response failed to compile",
          { type: "sourceMismatch" },
          { cause: error },
        );
      }
    }
  }
  const bytes = await response.arrayBuffer();
  return compileWasmBytes(bytes);
}

/** Loads, validates and instantiates a `WebAssembly.Module` from an `InitInput`. */
async function instantiateFromInput(
  input: InitInput | undefined,
): Promise<{ readonly source: SourceDescriptor; readonly output: InitOutput }> {
  const resolved = input === undefined ? DEFAULT_WASM_URL : input;
  const source = describeSource(input);
  if (source.kind === "invalid") {
    throw new TuneletError(
      "unsupported WebAssembly initialization source",
      { type: "sourceMismatch" },
    );
  }

  let module: WebAssembly.Module;
  if (resolved instanceof WebAssembly.Module) {
    module = resolved;
  } else if (resolved instanceof ArrayBuffer || ArrayBuffer.isView(resolved)) {
    module = await compileWasmBytes(resolved);
  } else if (resolved instanceof URL && resolved.protocol === "file:") {
    // Keep the Node.js built-in module out of browser bundles. TypeScript
    // removes the type-only import. The runtime import runs only for `file:`
    // URLs.
    const moduleName = "node:fs/promises";
    const fs: typeof import("node:fs/promises") = await import(
      /* webpackIgnore: true */ /* @vite-ignore */ moduleName
    );
    const fileBytes = await fs.readFile(resolved);
    module = await compileWasmBytes(fileBytes);
  } else if (isResponse(resolved)) {
    module = await compileWasmResponse(resolved);
  } else {
    const response = await fetch(resolved);
    module = await compileWasmResponse(response);
  }

  validateWasmModule(module);

  try {
    const output = await rawInit({ module_or_path: module });
    return { source, output };
  } catch (error: unknown) {
    throw new TuneletError(
      "custom WebAssembly module failed to link with Tunelet bindings",
      { type: "sourceMismatch" },
      { cause: error },
    );
  }
}

/** Extracts the raw `SyncInitInput` from `initSync` arguments. */
function unwrapSyncInput(
  module: SyncInitInput | { module: SyncInitInput },
): SyncInitInput {
  if (
    typeof module === "object" &&
    module !== null &&
    !(module instanceof WebAssembly.Module) &&
    !(module instanceof ArrayBuffer) &&
    !ArrayBuffer.isView(module) &&
    "module" in module
  ) {
    return module.module;
  }
  return module as SyncInitInput;
}

/**
 * Synchronously initializes the Wasm module from bytes or a compiled
 * `WebAssembly.Module`.
 *
 * If the module is already initialized with the same source, this function
 * returns the active exports. If a different source is active or in flight,
 * or if the custom module does not match the Tunelet bindings, this function
 * throws a `TuneletError`.
 *
 * @param module A `BufferSource`, a `WebAssembly.Module`, or `{ module }`.
 * @returns The initialized Wasm exports.
 */
export function initSync(
  module: SyncInitInput | { module: SyncInitInput },
): InitOutput {
  const rawInput = unwrapSyncInput(module);
  const candidate = describeSource(rawInput);
  if (candidate.kind !== "module" && candidate.kind !== "bytes") {
    throw new TuneletError(
      "initSync requires a BufferSource or WebAssembly.Module",
      { type: "sourceMismatch" },
    );
  }

  if (sdkState.status === "ready") {
    if (!sourcesMatch(sdkState.source, candidate)) {
      throw new TuneletError(SOURCE_MISMATCH_MESSAGE, {
        type: "sourceMismatch",
      });
    }
    return sdkState.output;
  }

  if (sdkState.status === "loading") {
    throw new TuneletError(SOURCE_MISMATCH_MESSAGE, {
      type: "sourceMismatch",
    });
  }

  let compiled: WebAssembly.Module;
  if (rawInput instanceof WebAssembly.Module) {
    compiled = rawInput;
  } else {
    try {
      const view = asByteView(rawInput);
      const copy = new Uint8Array(view.byteLength);
      copy.set(view);
      compiled = new WebAssembly.Module(copy);
    } catch (error: unknown) {
      throw new TuneletError(
        "custom WebAssembly source failed to compile",
        { type: "sourceMismatch" },
        { cause: error },
      );
    }
  }

  validateWasmModule(compiled);

  try {
    const output = rawInitSync({ module: compiled });
    sdkState = { status: "ready", source: candidate, output };
    return output;
  } catch (error: unknown) {
    throw new TuneletError(
      "custom WebAssembly module failed to link with Tunelet bindings",
      { type: "sourceMismatch" },
      { cause: error },
    );
  }
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
 * - A byte buffer (`BufferSource`).
 * - A compiled `WebAssembly.Module`.
 * - A promise for one of these.
 *
 * Concurrent or repeated calls with the same source share one initialization
 * result. A call with a different source, or with a binary that does not match
 * the Tunelet bindings, rejects with a `TuneletError`. If the initial load
 * fails, the cached promise clears so the caller can try again.
 *
 * @param source The optional Wasm source. The default is the binary in this
 * package.
 * @returns The initialized Wasm exports. Use the exported `Tunelet` class for
 * processors.
 */
export async function loadTunelet(
  source?: InitInput | Promise<InitInput>,
): Promise<InitOutput> {
  if (sdkState.status === "ready") {
    const resolved = source === undefined ? undefined : await source;
    const candidate = describeSource(resolved);
    if (!sourcesMatch(sdkState.source, candidate)) {
      throw new TuneletError(SOURCE_MISMATCH_MESSAGE, {
        type: "sourceMismatch",
      });
    }
    return sdkState.output;
  }

  if (sdkState.status === "loading") {
    const current = sdkState;
    const resolved = source === undefined ? undefined : await source;
    const candidate = describeSource(resolved);
    const activeSource = await current.sourcePromise;
    if (!sourcesMatch(activeSource, candidate)) {
      throw new TuneletError(SOURCE_MISMATCH_MESSAGE, {
        type: "sourceMismatch",
      });
    }
    return current.initPromise;
  }

  const inputPromise = Promise.resolve(source);
  const sourcePromise = inputPromise.then((resolved) =>
    describeSource(resolved),
  );
  // Suppress an unhandled rejection on the helper branch if inputPromise rejects.
  sourcePromise.catch(() => undefined);

  const initPromise = inputPromise
    .then((resolved) => instantiateFromInput(resolved))
    .then(
      ({ source: finalSource, output }) => {
        sdkState = { status: "ready", source: finalSource, output };
        return output;
      },
      (error: unknown) => {
        sdkState = { status: "uninitialized" };
        throw error;
      },
    );

  sdkState = { status: "loading", sourcePromise, initPromise };
  return initPromise;
}

/**
 * Returns a complete, independent default configuration for JavaScript.
 *
 * Call `await loadTunelet()` or `initSync()` before this function. Change the
 * returned fields and pass the object to `new Tunelet(...)` or `setConfig`.
 * `resynthesis.randomSeed` is a `bigint`, so it keeps all 64 bits.
 *
 * @returns A new `ProcessorConfig` with default values.
 */
export function defaultConfig(): ProcessorConfig {
  assertInitialized();
  try {
    return rawDefaultConfig();
  } catch (error: unknown) {
    throw toTuneletError(error, { type: "serializationFailed" });
  }
}

/**
 * A stateful PCM vocal processor that runs in WebAssembly.
 *
 * The input is signed 16-bit mono PCM. The output is planar stereo or an
 * in-place mono downmix. Buffer lengths count frames, not bytes.
 *
 * Call `await loadTunelet()` or `initSync()` before you construct an instance.
 * Every validation or lifecycle failure throws a `TuneletError` before the
 * audio state or output buffers change. Call `free()` or `[Symbol.dispose]()`
 * to release WebAssembly memory. Repeated `free()` calls do nothing, and calls
 * after `free()` throw a `TuneletError`.
 */
export class Tunelet {
  #inner: RawTunelet | undefined;

  /**
   * Prepares a processor and its reusable PCM buffers.
   *
   * `sampleRate` must be `44100` or `48000`. `maxBlockFrames` must be an
   * integer in `1..=4294967295`. Omit `config` to start from `defaultConfig()`.
   *
   * @param sampleRate The input and output sample rate in hertz (`44100` or `48000`).
   * @param maxBlockFrames The maximum frame count for each process or drain call.
   * @param config Optional initial `ProcessorConfig`.
   */
  constructor(
    sampleRate: number,
    maxBlockFrames: number,
    config?: ProcessorConfig,
  ) {
    assertInitialized();
    if (config === null) {
      throw new TuneletError(
        "invalid ProcessorConfig: expected an object, got null",
        { type: "invalidInput", target: "ProcessorConfig" },
      );
    }
    try {
      this.#inner = new RawTunelet(sampleRate, maxBlockFrames, config);
    } catch (error: unknown) {
      throw toTuneletError(error, {
        type: "invalidInput",
        target: "ProcessorConfig",
      });
    }
  }

  /** Returns the live Wasm handle or throws a disposed `TuneletError`. */
  #requireInner(): RawTunelet {
    const inner = this.#inner;
    if (inner === undefined) {
      throw new TuneletError(DISPOSED_MESSAGE, { type: "disposed" });
    }
    return inner;
  }

  /** The fixed sample rate of the input and the output in hertz. */
  get sampleRate(): number {
    return this.#requireInner().sampleRate;
  }

  /** The maximum frame count for each call that processes or drains audio. */
  get maxBlockFrames(): number {
    return this.#requireInner().maxBlockFrames;
  }

  /** The nominal resynthesis delay in frames. */
  get latencyFrames(): number {
    return this.#requireInner().latencyFrames;
  }

  /**
   * Returns a copy of the active configuration as a new JavaScript object.
   *
   * A change to the returned object has no effect until `setConfig` or a timed
   * event applies it.
   */
  config(): ProcessorConfig {
    const inner = this.#requireInner();
    try {
      return inner.config();
    } catch (error: unknown) {
      throw toTuneletError(error, { type: "serializationFailed" });
    }
  }

  /**
   * Validates a configuration and replaces the active configuration with it.
   *
   * If validation fails, this method throws a `TuneletError` and keeps the
   * processor state and any active drain unchanged.
   *
   * @param config The complete `ProcessorConfig` to apply.
   */
  setConfig(config: ProcessorConfig): void {
    const inner = this.#requireInner();
    if (config === null || config === undefined) {
      throw new TuneletError(
        `invalid ProcessorConfig: expected an object, got ${String(config)}`,
        { type: "invalidInput", target: "ProcessorConfig" },
      );
    }
    try {
      inner.setConfig(config);
    } catch (error: unknown) {
      throw toTuneletError(error, {
        type: "invalidInput",
        target: "ProcessorConfig",
      });
    }
  }

  /**
   * Processes mono PCM input into left and right `Int16Array` buffers of equal
   * length.
   *
   * Optional `events` apply immediately before their zero-based frame offset.
   * If an error occurs, this method throws a `TuneletError` before the output
   * arrays or the DSP state change.
   *
   * @param input Mono signed 16-bit PCM input frames.
   * @param left Left-channel signed 16-bit PCM output buffer.
   * @param right Right-channel signed 16-bit PCM output buffer.
   * @param events Optional ordered `TimedEvent[]` controls for this block.
   */
  process(
    input: Int16Array,
    left: Int16Array,
    right: Int16Array,
    events?: TimedEvent[],
  ): void {
    const inner = this.#requireInner();
    if (events === null) {
      throw new TuneletError(
        "invalid TimedEvent[]: expected an array, got null",
        { type: "invalidInput", target: "TimedEvent[]" },
      );
    }
    try {
      inner.process(input, left, right, events);
    } catch (error: unknown) {
      throw toTuneletError(error, {
        type: "invalidInput",
        target: "TimedEvent[]",
      });
    }
  }

  /**
   * Replaces mono PCM in `audio` with the stereo downmix.
   *
   * Optional `events` apply immediately before their zero-based frame offset.
   * If an error occurs, this method throws a `TuneletError` before `audio` or
   * the DSP state changes.
   *
   * @param audio In-place mono signed 16-bit PCM buffer.
   * @param events Optional ordered `TimedEvent[]` controls for this block.
   */
  processMono(audio: Int16Array, events?: TimedEvent[]): void {
    const inner = this.#requireInner();
    if (events === null) {
      throw new TuneletError(
        "invalid TimedEvent[]: expected an array, got null",
        { type: "invalidInput", target: "TimedEvent[]" },
      );
    }
    try {
      inner.processMono(audio, events);
    } catch (error: unknown) {
      throw toTuneletError(error, {
        type: "invalidInput",
        target: "TimedEvent[]",
      });
    }
  }

  /**
   * Starts a finite tail budget and returns its frame count.
   *
   * Call this method one time after the final input block.
   */
  beginDrain(): number {
    return this.#requireInner().beginDrain();
  }

  /** Returns the tail budget of the current configuration in frames. */
  tailFrames(): number {
    return this.#requireInner().tailFrames();
  }

  /**
   * Renders stereo tail frames into `left` and `right` buffers of equal length.
   *
   * Returns the number of rendered frames and fills unused frames with zeros.
   *
   * @param left Left-channel signed 16-bit PCM output buffer.
   * @param right Right-channel signed 16-bit PCM output buffer.
   */
  drain(left: Int16Array, right: Int16Array): number {
    const inner = this.#requireInner();
    try {
      return inner.drain(left, right);
    } catch (error: unknown) {
      throw toTuneletError(error, {
        type: "bufferLength",
        buffer: "right",
        expected: left.length,
        actual: right.length,
      });
    }
  }

  /**
   * Renders a mono tail block into `audio` and returns its rendered frame count.
   *
   * Fills unused frames with zeros.
   *
   * @param audio Mono signed 16-bit PCM output buffer.
   */
  drainMono(audio: Int16Array): number {
    const inner = this.#requireInner();
    try {
      return inner.drainMono(audio);
    } catch (error: unknown) {
      throw toTuneletError(error, {
        type: "blockTooLarge",
        frames: audio.length,
        max: inner.maxBlockFrames,
      });
    }
  }

  /** Returns a feedback snapshot and does not clear the stereo peak window. */
  feedback(): Feedback {
    const inner = this.#requireInner();
    try {
      return inner.feedback();
    } catch (error: unknown) {
      throw toTuneletError(error, { type: "serializationFailed" });
    }
  }

  /** Returns a feedback snapshot and clears only the stereo peak window. */
  takeFeedback(): Feedback {
    const inner = this.#requireInner();
    try {
      return inner.takeFeedback();
    } catch (error: unknown) {
      throw toTuneletError(error, { type: "serializationFailed" });
    }
  }

  /** Returns a telemetry snapshot and does not clear its three peak windows. */
  telemetry(): Telemetry {
    const inner = this.#requireInner();
    try {
      return inner.telemetry();
    } catch (error: unknown) {
      throw toTuneletError(error, { type: "serializationFailed" });
    }
  }

  /** Returns a telemetry snapshot and clears only its three peak windows. */
  takeTelemetry(): Telemetry {
    const inner = this.#requireInner();
    try {
      return inner.takeTelemetry();
    } catch (error: unknown) {
      throw toTuneletError(error, { type: "serializationFailed" });
    }
  }

  /**
   * Writes the LPC analysis and synthesis formant envelopes into two
   * `Float32Array` buffers of equal length.
   *
   * `maxFrequencyHz` must be finite, greater than zero and at most half of
   * `sampleRate`. Returns `false` and fills both arrays with zeros when no
   * formant model is active.
   *
   * @param maxFrequencyHz The frequency of the final envelope point in hertz.
   * @param input Output buffer for the LPC analysis envelope.
   * @param output Output buffer for the shifted synthesis envelope.
   */
  formantEnvelope(
    maxFrequencyHz: number,
    input: Float32Array,
    output: Float32Array,
  ): boolean {
    const inner = this.#requireInner();
    try {
      return inner.formantEnvelope(maxFrequencyHz, input, output);
    } catch (error: unknown) {
      throw toTuneletError(error, { type: "allocationFailed" });
    }
  }

  /** Resets the audio histories, held notes, pitch feedback, peak and frame count. */
  reset(): void {
    this.#requireInner().reset();
  }

  /**
   * Releases the WebAssembly processor memory.
   *
   * Repeated calls are safe no-ops. Any other method or getter call after
   * `free()` throws a `TuneletError`.
   */
  free(): void {
    const inner = this.#inner;
    if (inner !== undefined) {
      this.#inner = undefined;
      inner.free();
    }
  }

  /** Releases the WebAssembly processor memory for `using` scopes. */
  [Symbol.dispose](): void {
    this.free();
  }
}

/** The default export is the same function as `loadTunelet`. */
export default loadTunelet;
