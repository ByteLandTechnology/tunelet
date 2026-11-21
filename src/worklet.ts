/**
 * Main-thread Web Audio helpers for `TuneletWorkletNode`.
 *
 * Call `createTuneletWorkletNode(context, options)` on the main thread. The
 * helper validates `context.sampleRate` (44.1 kHz or 48 kHz), compiles the
 * packaged Wasm binary, registers `tunelet/worklet-processor` on the context
 * and returns a live stereo `TuneletWorkletNode`.
 */
import { loadTuneletModule } from "./index.js";
import type {
  Feedback,
  InitInput,
  MidiEvent,
  ProcessorConfig,
  SyncInitInput,
  Telemetry,
  TimedEvent,
} from "./index.js";
import { TUNELET_WORKLET_PROCESSOR_NAME } from "./worklet-processor.js";
import type {
  FormantEnvelopeResult,
  TuneletWorkletProcessorOptions,
  TuneletWorkletRequest,
  TuneletWorkletResponse,
} from "./worklet-processor.js";

export { TUNELET_WORKLET_PROCESSOR_NAME };
export type {
  FormantEnvelopeResult,
  TuneletWorkletProcessorOptions,
  TuneletWorkletRequest,
  TuneletWorkletResponse,
};

/** Options for the synchronous `new TuneletWorkletNode(context, options)` constructor. */
export interface TuneletWorkletNodeOptions
  extends Omit<AudioWorkletNodeOptions, "processorOptions"> {
  /** The compiled `WebAssembly.Module` or Wasm bytes for `initSync` in the worklet. */
  module: SyncInitInput;
  /** The maximum frame count for one DSP call in the worklet. The default is 128. */
  maxBlockFrames?: number;
  /** The optional initial processor configuration. */
  config?: ProcessorConfig;
}

/** Options for `createTuneletWorkletNode(context, options)`. */
export interface CreateTuneletWorkletNodeOptions
  extends Omit<TuneletWorkletNodeOptions, "module"> {
  /** An optional precompiled `WebAssembly.Module` or raw `BufferSource`. */
  module?: SyncInitInput;
  /** An optional Wasm source for `loadTuneletModule` if `module` is omitted. */
  wasmSource?: InitInput | Promise<InitInput>;
  /** An optional custom URL for the worklet processor module. */
  processorUrl?: string | URL;
}

/** Cached `addModule` promises per `BaseAudioContext` and processor URL. */
const registeredModules = new WeakMap<
  BaseAudioContext,
  Map<string, Promise<void>>
>();

const BaseWorkletNode: typeof AudioWorkletNode =
  typeof AudioWorkletNode === "function"
    ? AudioWorkletNode
    : (class {
        constructor() {
          throw new Error(
            "TuneletWorkletNode requires Web Audio AudioWorkletNode support",
          );
        }
      } as unknown as typeof AudioWorkletNode);

/** Rejects sample rates outside 44.1 kHz and 48 kHz before Wasm or worklet setup. */
function validateSampleRate(sampleRate: number): void {
  if (sampleRate !== 44100 && sampleRate !== 48000) {
    throw new RangeError(
      `Unsupported AudioContext sampleRate ${sampleRate} Hz; tunelet supports 44100 or 48000 Hz`,
    );
  }
}

/** Validates an optional `maxBlockFrames` option before worklet setup. */
function validateMaxBlockFrames(maxBlockFrames: number | undefined): void {
  if (
    maxBlockFrames !== undefined &&
    (!Number.isInteger(maxBlockFrames) ||
      maxBlockFrames < 1 ||
      maxBlockFrames > 0xffff_ffff)
  ) {
    throw new RangeError(
      "maxBlockFrames must be an integer in 1..=4294967295",
    );
  }
}

/** Registers the worklet module one time per `BaseAudioContext` and URL. */
function ensureWorkletModule(
  context: BaseAudioContext,
  processorUrl?: string | URL,
): Promise<void> {
  if (!context.audioWorklet) {
    return Promise.reject(
      new Error(
        "AudioContext.audioWorklet is unavailable; use a secure context (HTTPS or localhost)",
      ),
    );
  }
  const resolvedUrl =
    processorUrl ?? new URL("./worklet-processor.js", import.meta.url);
  const key = String(resolvedUrl);
  let byUrl = registeredModules.get(context);
  if (!byUrl) {
    byUrl = new Map<string, Promise<void>>();
    registeredModules.set(context, byUrl);
  }
  let registration = byUrl.get(key);
  if (!registration) {
    const currentMap = byUrl;
    registration = context.audioWorklet
      .addModule(resolvedUrl)
      .catch((error: unknown) => {
        currentMap.delete(key);
        throw error;
      });
    byUrl.set(key, registration);
  }
  return registration;
}

interface PendingRequest {
  resolve: (value: unknown) => void;
  reject: (reason: unknown) => void;
}

type TuneletRequestPayload = TuneletWorkletRequest extends infer Item
  ? Item extends unknown
    ? Omit<Item, "requestId">
    : never
  : never;

/**
 * An `AudioWorkletNode` that runs `Tunelet` in the Web Audio thread.
 *
 * By default, the node accepts one mono or stereo input and writes one stereo
 * (`[left, right]`) output. Control, MIDI, reset, drain, feedback and
 * telemetry methods communicate with `TuneletWorkletProcessor` over
 * `this.port`.
 */
export class TuneletWorkletNode extends BaseWorkletNode {
  private nextRequestId = 1;
  private readonly pending = new Map<number, PendingRequest>();
  private isDisposed = false;

  /**
   * Compiles the Wasm module, registers the worklet module and creates a
   * `TuneletWorkletNode`.
   */
  static create(
    context: BaseAudioContext,
    options?: CreateTuneletWorkletNodeOptions,
  ): Promise<TuneletWorkletNode> {
    return createTuneletWorkletNode(context, options);
  }

  constructor(context: BaseAudioContext, options: TuneletWorkletNodeOptions) {
    validateSampleRate(context.sampleRate);
    validateMaxBlockFrames(options?.maxBlockFrames);
    if (!options || options.module === undefined || options.module === null) {
      throw new Error(
        "TuneletWorkletNode requires options.module; use createTuneletWorkletNode() to compile it automatically",
      );
    }
    const { module, maxBlockFrames, config, ...nodeOptions } = options;
    const processorOptions: TuneletWorkletProcessorOptions = {
      module,
      maxBlockFrames,
      config,
    };
    super(context, TUNELET_WORKLET_PROCESSOR_NAME, {
      numberOfInputs: 1,
      numberOfOutputs: 1,
      outputChannelCount: [2],
      ...nodeOptions,
      processorOptions,
    });

    this.port.addEventListener(
      "message",
      (event: MessageEvent<TuneletWorkletResponse>) => {
        this.handleResponse(event.data);
      },
    );
    this.port.start();

    this.addEventListener("processorerror", () => {
      this.rejectAll(new Error("TuneletWorkletProcessor encountered a runtime error"));
    });
  }

  /** Reads a copy of the active `ProcessorConfig` from the worklet. */
  config(): Promise<ProcessorConfig> {
    return this.request<ProcessorConfig>({ type: "config" });
  }

  /** Validates and applies a complete `ProcessorConfig` in the worklet. */
  setConfig(config: ProcessorConfig): Promise<void> {
    return this.request<void>({ type: "setConfig", config });
  }

  /** Queues a harmony `MidiEvent` for the next audio block at `offset` (default 0). */
  sendMidi(event: MidiEvent, offset = 0): Promise<void> {
    return this.request<void>({ type: "midi", event, offset });
  }

  /** Queues a correction `MidiEvent` for the next audio block at `offset` (default 0). */
  sendCorrectionMidi(event: MidiEvent, offset = 0): Promise<void> {
    return this.request<void>({ type: "correctionMidi", event, offset });
  }

  /** Queues a sorted `TimedEvent[]` slice for the next audio block. */
  sendEvents(events: TimedEvent[]): Promise<void> {
    return this.request<void>({ type: "events", events });
  }

  /** Starts the finite tail drain in the worklet and returns its frame budget. */
  beginDrain(): Promise<number> {
    return this.request<number>({ type: "beginDrain" });
  }

  /** Returns the configured tail budget in frames without starting a drain. */
  tailFrames(): Promise<number> {
    return this.request<number>({ type: "tailFrames" });
  }

  /** Reads the current `Feedback` snapshot without clearing the peak window. */
  feedback(): Promise<Feedback> {
    return this.request<Feedback>({ type: "feedback" });
  }

  /** Reads the current `Feedback` snapshot and clears the stereo peak window. */
  takeFeedback(): Promise<Feedback> {
    return this.request<Feedback>({ type: "takeFeedback" });
  }

  /** Reads the current `Telemetry` snapshot without clearing peak windows. */
  telemetry(): Promise<Telemetry> {
    return this.request<Telemetry>({ type: "telemetry" });
  }

  /** Reads the current `Telemetry` snapshot and clears its three peak windows. */
  takeTelemetry(): Promise<Telemetry> {
    return this.request<Telemetry>({ type: "takeTelemetry" });
  }

  /** Reads the LPC analysis and synthesis formant envelopes with `points` entries. */
  formantEnvelope(
    maxFrequencyHz: number,
    points: number,
  ): Promise<FormantEnvelopeResult> {
    return this.request<FormantEnvelopeResult>({
      type: "formantEnvelope",
      maxFrequencyHz,
      points,
    });
  }

  /** Resets the audio histories and feedback in the worklet. */
  reset(): Promise<void> {
    return this.request<void>({ type: "reset" });
  }

  /** Releases the Wasm `Tunelet` instance in the worklet and stops audio output. */
  async dispose(): Promise<void> {
    if (this.isDisposed) {
      return;
    }
    await this.request<void>({ type: "dispose" });
    this.isDisposed = true;
    this.rejectAll(new Error("TuneletWorkletNode is disposed"));
  }

  /** Sends one correlated request message over `this.port`. */
  private request<T>(payload: TuneletRequestPayload): Promise<T> {
    if (this.isDisposed) {
      return Promise.reject(new Error("TuneletWorkletNode is disposed"));
    }
    const requestId = this.nextRequestId++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(requestId, {
        resolve: resolve as (value: unknown) => void,
        reject,
      });
      this.port.postMessage({
        ...payload,
        requestId,
      } as TuneletWorkletRequest);
    });
  }

  /** Resolves or rejects a pending request when a reply arrives on `this.port`. */
  private handleResponse(message: TuneletWorkletResponse): void {
    if (
      typeof message !== "object" ||
      message === null ||
      !("requestId" in message) ||
      message.requestId === undefined
    ) {
      return;
    }
    const entry = this.pending.get(message.requestId);
    if (!entry) {
      return;
    }
    this.pending.delete(message.requestId);
    if (message.type === "error") {
      entry.reject(new Error(message.message));
      return;
    }
    if ("value" in message) {
      entry.resolve(message.value);
    } else {
      entry.resolve(undefined);
    }
  }

  /** Rejects all unfinished requests when the node stops or fails. */
  private rejectAll(error: Error): void {
    for (const entry of this.pending.values()) {
      entry.reject(error);
    }
    this.pending.clear();
  }
}

/**
 * Compiles the Wasm module, registers the processor module on `context` and
 * returns a live `TuneletWorkletNode`.
 *
 * `context.sampleRate` must be 44100 or 48000. If the rate is not supported,
 * this function rejects before it loads Wasm or registers the worklet module.
 *
 * @param context The `AudioContext` or `OfflineAudioContext` at 44.1 kHz or 48 kHz.
 * @param options Optional initial configuration, block limit or custom module source.
 * @returns A configured `TuneletWorkletNode` ready to connect in the audio graph.
 */
export async function createTuneletWorkletNode(
  context: BaseAudioContext,
  options?: CreateTuneletWorkletNodeOptions,
): Promise<TuneletWorkletNode> {
  validateSampleRate(context.sampleRate);
  validateMaxBlockFrames(options?.maxBlockFrames);
  const { module: providedModule, wasmSource, processorUrl, ...rest } =
    options ?? {};
  const [module] = await Promise.all([
    providedModule !== undefined
      ? Promise.resolve(providedModule)
      : loadTuneletModule(wasmSource),
    ensureWorkletModule(context, processorUrl),
  ]);
  return new TuneletWorkletNode(context, {
    ...rest,
    module,
  });
}
