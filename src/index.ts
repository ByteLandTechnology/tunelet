/**
 * The shared ESM entry point for browsers and Node.js.
 *
 * Call `loadTunelet()` before you construct processors or read the default
 * configuration. For live Web Audio graphs in browsers, call
 * `createTuneletNode(audioContext)` to register `tunelet/worklet` and construct
 * a typed `AudioWorkletNode`. Both environments use the same generated bindings
 * and the same Wasm binary. This module re-exports all named exports of the
 * module that wasm-bindgen generates, which include `Tunelet`, `defaultConfig`,
 * `initSync` and the configuration, event and feedback types.
 */
import init from "../pkg/tunelet.js";
import type {
  ControlEvent,
  Feedback,
  InitInput,
  InitOutput,
  ProcessorConfig,
  Telemetry,
  TimedEvent,
} from "../pkg/tunelet.js";
import { TUNELET_WORKLET_PROCESSOR_NAME } from "./worklet-protocol.js";
import type {
  TuneletProcessorOptions,
  TuneletReportingOptions,
  TuneletWorkletCommand,
  TuneletWorkletMessage,
} from "./worklet-protocol.js";

export * from "../pkg/tunelet.js";
export { TUNELET_WORKLET_PROCESSOR_NAME } from "./worklet-protocol.js";
export type {
  TuneletProcessorOptions,
  TuneletReportingOptions,
  TuneletWorkletCommand,
  TuneletWorkletMessage,
} from "./worklet-protocol.js";

/** Options for `createTuneletNode`. */
export interface TuneletNodeOptions extends TuneletReportingOptions {
  /** The initial configuration for the worklet processor. */
  config?: ProcessorConfig;
  /**
   * The optional Wasm binary source. The default is the packaged
   * `tunelet_bg.wasm` asset.
   */
  module?: InitInput | Promise<InitInput>;
  /**
   * The optional module URL for `audioWorklet.addModule`. The default is the
   * packaged `worklet.js` entry.
   */
  workletUrl?: string | URL;
  /** The prepared host block limit in frames. The default is 128. */
  maxBlockFrames?: number;
  /** The output channel count for each output. The default is `[2]`. */
  outputChannelCount?: number[];
  /** Additional fields for `AudioWorkletNodeOptions.processorOptions`. */
  processorOptions?: Record<string, unknown>;
  /** Receives each throttled `Feedback` snapshot from the audio thread. */
  onFeedback?: ((feedback: Feedback) => void) | null;
  /** Receives each throttled `Telemetry` snapshot from the audio thread. */
  onTelemetry?: ((telemetry: Telemetry) => void) | null;
  /** Receives validation errors from asynchronous control messages. */
  onError?: ((error: Error) => void) | null;
}

/**
 * An `AudioWorkletNode` with typed controls and throttled state snapshots for
 * `TuneletWorkletProcessor`.
 */
export interface TuneletWorkletNode extends AudioWorkletNode {
  /** The latest throttled `Feedback` snapshot, or `undefined` before reports. */
  readonly feedback: Feedback | undefined;
  /** The latest throttled `Telemetry` snapshot, or `undefined` before reports. */
  readonly telemetry: Telemetry | undefined;
  /** A callback that receives each throttled `Feedback` snapshot. */
  onFeedback: ((feedback: Feedback) => void) | null;
  /** A callback that receives each throttled `Telemetry` snapshot. */
  onTelemetry: ((telemetry: Telemetry) => void) | null;
  /** A callback that receives validation errors from control messages. */
  onError: ((error: Error) => void) | null;
  /** Replaces the full `ProcessorConfig` on the audio thread. */
  setConfig(config: ProcessorConfig): void;
  /** Queues one `ControlEvent` (at offset 0) or `TimedEvent` for the next block. */
  sendEvent(event: ControlEvent | TimedEvent): void;
  /** Queues a slice of `TimedEvent` items for the next block. */
  sendEvents(events: readonly TimedEvent[]): void;
  /** Resets the audio histories, resampler state and feedback. */
  reset(): void;
  /** Starts a finite tail drain on the audio thread. */
  beginDrain(): void;
  /** Updates the UI report interval and active report fields. */
  setReporting(options: TuneletReportingOptions): void;
  /** Releases the Wasm processor memory and disconnects the node. */
  dispose(): void;
  [Symbol.dispose](): void;
}

/** The shared initialization promise, which a failure clears. */
let initialization: Promise<InitOutput> | undefined;

/** The cached compiled Wasm module for the default packaged binary. */
let compiledDefaultModule: Promise<WebAssembly.Module> | undefined;

/** Registered worklet module promises for each `BaseAudioContext`. */
const registeredWorklets = new WeakMap<
  BaseAudioContext,
  Map<string, Promise<void>>
>();

type AudioWorkletNodeConstructor = typeof AudioWorkletNode;
type BuiltNodeConstructor = new (
  context: BaseAudioContext,
  nodeOptions: AudioWorkletNodeOptions,
  callbacks: {
    onFeedback?: ((feedback: Feedback) => void) | null;
    onTelemetry?: ((telemetry: Telemetry) => void) | null;
    onError?: ((error: Error) => void) | null;
  },
) => TuneletWorkletNode;

/** Cached `TuneletWorkletNode` subclasses for each `AudioWorkletNode` class. */
const nodeSubclasses = new WeakMap<
  AudioWorkletNodeConstructor,
  BuiltNodeConstructor
>();

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
 * @returns The initialized Wasm exports. Use the re-exported `Tunelet` class
 * for processors.
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

/**
 * Compiles the Wasm binary, registers `tunelet/worklet` on `context` and
 * returns a live `TuneletWorkletNode`.
 *
 * The node accepts mono or multi-channel `Float32` input and produces stereo
 * output by default. Contexts at 44.1 kHz or 48 kHz run at their native rate.
 * Contexts at other rates (such as 88.2 kHz or 96 kHz) resample to and from
 * an internal 48 kHz `Tunelet` processor.
 *
 * @param context The target `AudioContext` or `OfflineAudioContext`.
 * @param options Optional configuration, asset URLs and report settings.
 * @returns A connected-ready `TuneletWorkletNode`.
 */
export async function createTuneletNode(
  context: BaseAudioContext,
  options?: TuneletNodeOptions,
): Promise<TuneletWorkletNode> {
  if (!context || !context.audioWorklet) {
    throw new Error("The AudioContext does not provide an audioWorklet.");
  }

  const BaseNode = readAudioWorkletNodeConstructor();
  const [wasmModule] = await Promise.all([
    compileTuneletModule(options?.module),
    registerWorkletModule(context, options?.workletUrl),
  ]);
  await init({ module_or_path: wasmModule });

  const processorOptions: TuneletProcessorOptions & Record<string, unknown> = {
    ...options?.processorOptions,
    module: wasmModule,
    config: options?.config,
    sampleRate: context.sampleRate,
    maxBlockFrames: options?.maxBlockFrames,
    reportIntervalMs: options?.reportIntervalMs,
    feedback: options?.feedback,
    telemetry: options?.telemetry,
  };

  const nodeOptions: AudioWorkletNodeOptions = {
    numberOfInputs: 1,
    numberOfOutputs: 1,
    outputChannelCount: options?.outputChannelCount ?? [2],
    processorOptions,
  };

  const NodeClass = getOrCreateNodeSubclass(BaseNode);
  return new NodeClass(context, nodeOptions, {
    onFeedback: options?.onFeedback,
    onTelemetry: options?.onTelemetry,
    onError: options?.onError,
  });
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
    // Keep the Node.js built-in module out of browser bundles. TypeScript
    // removes the type-only import. The runtime import runs only for `file:`
    // URLs.
    const moduleName = "node:fs/promises";
    const fs: typeof import("node:fs/promises") = await import(
      /* webpackIgnore: true */ /* @vite-ignore */ moduleName
    );
    return init({ module_or_path: await fs.readFile(input) });
  }

  return init({ module_or_path: input });
}

/** Compiles a `WebAssembly.Module` and caches the default packaged binary. */
function compileTuneletModule(
  source?: InitInput | Promise<InitInput>,
): Promise<WebAssembly.Module> {
  if (source !== undefined) {
    return compileFromSource(source);
  }
  compiledDefaultModule ??= compileFromSource(undefined).catch(
    (error: unknown) => {
      compiledDefaultModule = undefined;
      throw error;
    },
  );
  return compiledDefaultModule;
}

/** Resolves any `InitInput` source into a compiled `WebAssembly.Module`. */
async function compileFromSource(
  source: InitInput | Promise<InitInput> | undefined,
): Promise<WebAssembly.Module> {
  const input =
    source === undefined
      ? new URL("../pkg/tunelet_bg.wasm", import.meta.url)
      : await source;

  if (input instanceof WebAssembly.Module) {
    return input;
  }

  if (input instanceof URL && input.protocol === "file:") {
    const moduleName = "node:fs/promises";
    const fs: typeof import("node:fs/promises") = await import(
      /* webpackIgnore: true */ /* @vite-ignore */ moduleName
    );
    const bytes = await fs.readFile(input);
    return WebAssembly.compile(
      new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength),
    );
  }

  if (ArrayBuffer.isView(input)) {
    return WebAssembly.compile(
      new Uint8Array(input.buffer, input.byteOffset, input.byteLength),
    );
  }

  if (input instanceof ArrayBuffer) {
    return WebAssembly.compile(input);
  }

  if (typeof Response === "function" && input instanceof Response) {
    return compileFromResponse(input);
  }

  const response = await fetch(input as RequestInfo | URL);
  return compileFromResponse(response);
}

/** Compiles a `Response` via `WebAssembly.compileStreaming` when supported. */
async function compileFromResponse(
  response: Response,
): Promise<WebAssembly.Module> {
  if (!response.ok) {
    throw new Error(
      `failed to fetch Wasm: ${response.status} ${response.statusText} fetching '${response.url}'`,
    );
  }
  const contentType = response.headers?.get("Content-Type")?.toLowerCase() ?? "";
  if (
    typeof WebAssembly.compileStreaming === "function" &&
    contentType.includes("application/wasm")
  ) {
    return WebAssembly.compileStreaming(response);
  }
  return WebAssembly.compile(await response.arrayBuffer());
}

/** Registers the worklet module one time for each `(context, url)` pair. */
function registerWorkletModule(
  context: BaseAudioContext,
  workletUrl?: string | URL,
): Promise<void> {
  const resolved = workletUrl ?? new URL("./worklet.js", import.meta.url);
  const href = typeof resolved === "string" ? resolved : resolved.href;

  let byUrl = registeredWorklets.get(context);
  if (!byUrl) {
    byUrl = new Map<string, Promise<void>>();
    registeredWorklets.set(context, byUrl);
  }

  let pending = byUrl.get(href);
  if (!pending) {
    const mapRef = byUrl;
    pending = context.audioWorklet.addModule(href).catch((error: unknown) => {
      mapRef.delete(href);
      throw error;
    });
    byUrl.set(href, pending);
  }
  return pending;
}

/** Returns `globalThis.AudioWorkletNode` or throws if Web Audio is absent. */
function readAudioWorkletNodeConstructor(): AudioWorkletNodeConstructor {
  const candidate = (globalThis as { AudioWorkletNode?: unknown })
    .AudioWorkletNode;
  if (typeof candidate !== "function") {
    throw new Error(
      "AudioWorkletNode is not available in this JavaScript environment.",
    );
  }
  return candidate as AudioWorkletNodeConstructor;
}

/** Builds a subclass of `BaseNode` lazily so Node.js imports stay safe. */
function getOrCreateNodeSubclass(
  BaseNode: AudioWorkletNodeConstructor,
): BuiltNodeConstructor {
  const existing = nodeSubclasses.get(BaseNode);
  if (existing) {
    return existing;
  }

  class TuneletWorkletNodeImpl extends BaseNode implements TuneletWorkletNode {
    onFeedback: ((feedback: Feedback) => void) | null;
    onTelemetry: ((telemetry: Telemetry) => void) | null;
    onError: ((error: Error) => void) | null;

    #feedback: Feedback | undefined;
    #telemetry: Telemetry | undefined;
    #disposed = false;
    readonly #listener: (event: MessageEvent<TuneletWorkletMessage>) => void;

    constructor(
      context: BaseAudioContext,
      nodeOptions: AudioWorkletNodeOptions,
      callbacks: {
        onFeedback?: ((feedback: Feedback) => void) | null;
        onTelemetry?: ((telemetry: Telemetry) => void) | null;
        onError?: ((error: Error) => void) | null;
      },
    ) {
      super(context, TUNELET_WORKLET_PROCESSOR_NAME, nodeOptions);
      this.onFeedback = callbacks.onFeedback ?? null;
      this.onTelemetry = callbacks.onTelemetry ?? null;
      this.onError = callbacks.onError ?? null;

      this.#listener = (event: MessageEvent<TuneletWorkletMessage>) => {
        this.#handleMessage(event.data);
      };

      if (typeof this.port.addEventListener === "function") {
        this.port.addEventListener("message", this.#listener);
        this.port.start?.();
      } else {
        this.port.onmessage = this.#listener;
      }
      (this.port as { unref?: () => void }).unref?.();
    }

    get feedback(): Feedback | undefined {
      return this.#feedback;
    }

    get telemetry(): Telemetry | undefined {
      return this.#telemetry;
    }

    setConfig(config: ProcessorConfig): void {
      this.#postCommand({ type: "setConfig", config });
    }

    sendEvent(event: ControlEvent | TimedEvent): void {
      const timed: TimedEvent =
        "offset" in event && "event" in event
          ? { offset: event.offset, event: event.event }
          : { offset: 0, event };
      this.#postCommand({ type: "events", events: [timed] });
    }

    sendEvents(events: readonly TimedEvent[]): void {
      if (events.length === 0) {
        return;
      }
      this.#postCommand({ type: "events", events: Array.from(events) });
    }

    reset(): void {
      this.#postCommand({ type: "reset" });
    }

    beginDrain(): void {
      this.#postCommand({ type: "beginDrain" });
    }

    setReporting(options: TuneletReportingOptions): void {
      this.#postCommand({ type: "setReporting", ...options });
    }

    dispose(): void {
      if (this.#disposed) {
        return;
      }
      this.#disposed = true;
      this.port.postMessage({ type: "dispose" } satisfies TuneletWorkletCommand);
      if (typeof this.port.removeEventListener === "function") {
        this.port.removeEventListener("message", this.#listener);
      } else if (this.port.onmessage === this.#listener) {
        this.port.onmessage = null;
      }
      this.port.close?.();
      try {
        this.disconnect();
      } catch {
        // Ignore disconnect errors if the node was never connected.
      }
    }

    [Symbol.dispose](): void {
      this.dispose();
    }

    #postCommand(command: TuneletWorkletCommand): void {
      if (this.#disposed) {
        throw new Error("The TuneletWorkletNode is disposed.");
      }
      this.port.postMessage(command);
    }

    #handleMessage(message: TuneletWorkletMessage | undefined): void {
      if (!message || typeof message !== "object") {
        return;
      }
      if (message.type === "report") {
        if (message.feedback !== undefined) {
          this.#feedback = message.feedback;
          this.onFeedback?.(message.feedback);
        }
        if (message.telemetry !== undefined) {
          this.#telemetry = message.telemetry;
          this.onTelemetry?.(message.telemetry);
        }
      } else if (message.type === "error") {
        this.onError?.(new Error(message.message));
      }
    }
  }

  nodeSubclasses.set(BaseNode, TuneletWorkletNodeImpl);
  return TuneletWorkletNodeImpl;
}

/** The default export is the same function as `loadTunelet`. */
export default loadTunelet;
