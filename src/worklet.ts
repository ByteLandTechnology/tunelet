/**
 * The main-thread Web Audio entry point for `tunelet/worklet`.
 *
 * Call `createTuneletWorkletNode(context, options)` on the main thread to
 * validate the `AudioContext` sample rate, resolve the Wasm binary without
 * `fetch` in the worklet, register the worklet script and construct a live
 * `TuneletWorkletNode`.
 */
import { loadTuneletWasm } from "./index.js";
import type {
  BlockEvents,
  ControlEvent,
  Feedback,
  InitInput,
  MidiEvent,
  ProcessorConfig,
  SyncInitInput,
  Telemetry,
} from "./index.js";
import {
  DEFAULT_WORKLET_BLOCK_FRAMES,
  TUNELET_PROCESSOR_NAME,
} from "./worklet-protocol.js";
import type {
  FormantEnvelopeResult,
  TuneletDrainOptions,
  TuneletProcessorOptions,
  TuneletWorkletCommand,
  TuneletWorkletNotification,
  TuneletWorkletOutboundMessage,
  TuneletWorkletRequest,
  TuneletWorkletResponse,
} from "./worklet-protocol.js";

export {
  DEFAULT_WORKLET_BLOCK_FRAMES,
  TUNELET_PROCESSOR_NAME,
} from "./worklet-protocol.js";
export type {
  FormantEnvelopeResult,
  TuneletDrainOptions,
  TuneletProcessorOptions,
  TuneletWorkletCommand,
  TuneletWorkletNotification,
  TuneletWorkletOutboundMessage,
  TuneletWorkletRequest,
  TuneletWorkletResponse,
} from "./worklet-protocol.js";

/** The nominal resynthesis delay in frames at 44.1 kHz and 48 kHz. */
const NOMINAL_LATENCY_FRAMES = 1593;

/** Tracks `addModule` promises per `BaseAudioContext` and registration key. */
const registeredContexts = new WeakMap<
  BaseAudioContext,
  Map<string, Promise<void>>
>();

/** Resolves `AudioWorkletNode` in browsers and in test environments. */
const AudioWorkletNodeBase: typeof AudioWorkletNode =
  typeof globalThis.AudioWorkletNode === "function"
    ? globalThis.AudioWorkletNode
    : (class extends EventTarget {
        constructor(
          context: BaseAudioContext,
          name: string,
          options?: AudioWorkletNodeOptions,
        ) {
          super();
          if (typeof globalThis.AudioWorkletNode === "function") {
            return Reflect.construct(
              globalThis.AudioWorkletNode,
              [context, name, options],
              new.target,
            );
          }
          throw new Error(
            "AudioWorkletNode is not available in this environment",
          );
        }
        disconnect(): void {}
      } as unknown as typeof AudioWorkletNode);

/** Checks that the `BaseAudioContext` runs at 44.1 kHz or 48 kHz. */
function validateContextSampleRate(context: BaseAudioContext): void {
  if (
    typeof context !== "object" ||
    context === null ||
    typeof context.sampleRate !== "number"
  ) {
    throw new TypeError("context must be a valid BaseAudioContext");
  }
  const hz = context.sampleRate;
  if (hz !== 44100 && hz !== 48000) {
    throw new RangeError(
      `unsupported sample rate: ${hz} Hz (expected 44100 or 48000)`,
    );
  }
}

/** Checks `maxBlockFrames` before worklet registration or node creation. */
function validateMaxBlockFrames(maxBlockFrames: number): void {
  if (
    typeof maxBlockFrames !== "number" ||
    !Number.isInteger(maxBlockFrames) ||
    maxBlockFrames < 1 ||
    maxBlockFrames > 0xffff_ffff
  ) {
    throw new RangeError(
      "maxBlockFrames must be an integer in 1..=4294967295",
    );
  }
}

/** Checks `outputChannelCount` before node creation. */
function validateOutputChannels(
  outputChannelCount: readonly number[],
): [1] | [2] {
  if (
    !Array.isArray(outputChannelCount) ||
    outputChannelCount.length !== 1 ||
    (outputChannelCount[0] !== 1 && outputChannelCount[0] !== 2)
  ) {
    throw new RangeError("outputChannelCount must be [1] or [2]");
  }
  return [outputChannelCount[0]];
}

/** Options for `createTuneletWorkletNode`. */
export interface TuneletWorkletOptions {
  /**
   * Optional Wasm source for `loadTuneletWasm`. The default is the binary in
   * this package.
   */
  wasm?: InitInput | Promise<InitInput>;
  /**
   * Optional URL for the worklet processor script. The default is
   * `./worklet-processor.js` relative to this module.
   */
  workletUrl?: string | URL;
  /**
   * Custom processor registration name. The default is
   * `TUNELET_PROCESSOR_NAME`.
   */
  processorName?: string;
  /**
   * Maximum frame count for each internal worklet chunk. The default is 128.
   */
  maxBlockFrames?: number;
  /** Optional initial `ProcessorConfig`. */
  config?: ProcessorConfig | null;
  /** Output channel count for output zero: `[2]` (default) or `[1]`. */
  outputChannelCount?: [1] | [2];
  /** Input channel count on the Web Audio node. The default is 1. */
  channelCount?: number;
  /** Input channel count mode. The default is `"explicit"`. */
  channelCountMode?: ChannelCountMode;
  /** Input channel interpretation. The default is `"speakers"`. */
  channelInterpretation?: ChannelInterpretation;
}

/** Options for synchronous `new TuneletWorkletNode(context, options)`. */
export interface TuneletWorkletNodeOptions {
  /** Preloaded Wasm bytes or compiled `WebAssembly.Module` for `initSync`. */
  wasm: SyncInitInput;
  /**
   * Custom processor registration name. The default is
   * `TUNELET_PROCESSOR_NAME`.
   */
  processorName?: string;
  /**
   * Maximum frame count for each internal worklet chunk. The default is 128.
   */
  maxBlockFrames?: number;
  /** Optional initial `ProcessorConfig`. */
  config?: ProcessorConfig | null;
  /** Output channel count for output zero: `[2]` (default) or `[1]`. */
  outputChannelCount?: [1] | [2];
  /** Input channel count on the Web Audio node. The default is 1. */
  channelCount?: number;
  /** Input channel count mode. The default is `"explicit"`. */
  channelCountMode?: ChannelCountMode;
  /** Input channel interpretation. The default is `"speakers"`. */
  channelInterpretation?: ChannelInterpretation;
}

interface PendingRequest {
  resolve: (value: unknown) => void;
  reject: (reason: unknown) => void;
}

/**
 * Main-thread `AudioWorkletNode` connected to `TuneletWorkletProcessor` over
 * `MessagePort`.
 *
 * Prefer `createTuneletWorkletNode(context, options)` to load the Wasm binary,
 * register the worklet module and await processor initialization.
 */
export class TuneletWorkletNode extends AudioWorkletNodeBase {
  /** Resolves when the worklet processor finishes `initSync` and construction. */
  readonly ready: Promise<void>;

  #sampleRate: number;
  #maxBlockFrames: number;
  #latencyFrames = NOMINAL_LATENCY_FRAMES;
  #nextRequestId = 1;
  #pending = new Map<number, PendingRequest>();
  #drainWaiters: Array<() => void> = [];
  #draining = false;
  #disposed = false;

  constructor(context: BaseAudioContext, options: TuneletWorkletNodeOptions) {
    validateContextSampleRate(context);
    if (
      typeof options !== "object" ||
      options === null ||
      options.wasm === undefined ||
      options.wasm === null
    ) {
      throw new TypeError(
        "options.wasm must provide a WebAssembly.Module or byte buffer",
      );
    }

    const maxBlockFrames =
      options.maxBlockFrames ?? DEFAULT_WORKLET_BLOCK_FRAMES;
    validateMaxBlockFrames(maxBlockFrames);
    const outputChannelCount = validateOutputChannels(
      options.outputChannelCount ?? [2],
    );
    const processorName = options.processorName ?? TUNELET_PROCESSOR_NAME;
    const processorOptions: TuneletProcessorOptions = {
      wasm: options.wasm,
      maxBlockFrames,
      config: options.config,
    };

    super(context, processorName, {
      numberOfInputs: 1,
      numberOfOutputs: 1,
      outputChannelCount,
      channelCount: options.channelCount ?? 1,
      channelCountMode: options.channelCountMode ?? "explicit",
      channelInterpretation: options.channelInterpretation ?? "speakers",
      processorOptions,
    });

    this.#sampleRate = context.sampleRate;
    this.#maxBlockFrames = maxBlockFrames;

    let resolveReady!: () => void;
    let rejectReady!: (reason: unknown) => void;
    let settledReady = false;
    this.ready = new Promise<void>((resolve, reject) => {
      resolveReady = () => {
        if (!settledReady) {
          settledReady = true;
          resolve();
        }
      };
      rejectReady = (reason: unknown) => {
        if (!settledReady) {
          settledReady = true;
          reject(reason);
        }
      };
    });

    this.port.onmessage = (
      event: MessageEvent<TuneletWorkletOutboundMessage>,
    ) => {
      const message = event.data;
      if (
        typeof message !== "object" ||
        message === null ||
        !("__tunelet" in message)
      ) {
        return;
      }

      if (message.__tunelet === "ready") {
        this.#sampleRate = message.sampleRate;
        this.#maxBlockFrames = message.maxBlockFrames;
        this.#latencyFrames = message.latencyFrames;
        resolveReady();
        return;
      }

      if (message.__tunelet === "initError") {
        const error = new Error(message.error);
        this.#disposed = true;
        this.#rejectAllPending(error);
        rejectReady(error);
        return;
      }

      if (message.__tunelet === "drainComplete") {
        this.#draining = false;
        this.#resolveDrainWaiters();
        if (typeof CustomEvent === "function") {
          this.dispatchEvent(new CustomEvent("draincomplete"));
        }
        return;
      }

      if (message.__tunelet === "processError") {
        if (typeof CustomEvent === "function") {
          this.dispatchEvent(
            new CustomEvent("tuneleterror", { detail: new Error(message.error) }),
          );
        }
        return;
      }

      if (message.__tunelet === "response" && message.id !== undefined) {
        const entry = this.#pending.get(message.id);
        if (!entry) {
          return;
        }
        this.#pending.delete(message.id);
        if (message.ok) {
          entry.resolve(message.value);
        } else {
          entry.reject(new Error(message.error));
        }
      }
    };

    if (typeof this.addEventListener === "function") {
      this.addEventListener("processorerror", () => {
        const error = new Error("TuneletWorkletProcessor encountered an error");
        this.#disposed = true;
        this.#rejectAllPending(error);
        rejectReady(error);
      });
    }
  }

  /** The sample rate of the processor in hertz. */
  get processorSampleRate(): number {
    return this.#sampleRate;
  }

  /** The maximum frame count for one internal processor chunk. */
  get maxBlockFrames(): number {
    return this.#maxBlockFrames;
  }

  /** The nominal resynthesis latency in frames. */
  get latencyFrames(): number {
    return this.#latencyFrames;
  }

  /** Reads a copy of the active `ProcessorConfig` over `MessagePort`. */
  config(): Promise<ProcessorConfig> {
    return this.#request<ProcessorConfig>({ type: "config" });
  }

  /** Validates and replaces the full `ProcessorConfig` over `MessagePort`. */
  async setConfig(config: ProcessorConfig): Promise<void> {
    await this.#request<undefined>({ type: "setConfig", config });
    this.#draining = false;
    this.#resolveDrainWaiters();
  }

  /** Queues a harmony `MidiEvent` for the next audio block. */
  async sendMidi(event: MidiEvent): Promise<void> {
    await this.#request<undefined>({ type: "midi", event });
    this.#draining = false;
    this.#resolveDrainWaiters();
  }

  /** Queues a correction-keyboard `MidiEvent` for the next audio block. */
  async sendCorrectionMidi(event: MidiEvent): Promise<void> {
    await this.#request<undefined>({ type: "correctionMidi", event });
    this.#draining = false;
    this.#resolveDrainWaiters();
  }

  /** Applies or queues one `ControlEvent` over `MessagePort`. */
  async sendControl(event: ControlEvent): Promise<void> {
    await this.#request<undefined>({ type: "control", event });
    this.#draining = false;
    this.#resolveDrainWaiters();
  }

  /** Queues sample-offset `TimedEvent` entries for the next render quantum. */
  async sendEvents(events: BlockEvents): Promise<void> {
    await this.#request<undefined>({ type: "events", events });
    this.#draining = false;
    this.#resolveDrainWaiters();
  }

  /** Resets the audio histories and feedback, and cancels any active drain. */
  async reset(): Promise<void> {
    await this.#request<undefined>({ type: "reset" });
    this.#draining = false;
    this.#resolveDrainWaiters();
  }

  /**
   * Starts a finite tail drain in the worklet and returns the configured tail
   * budget in frames.
   */
  async beginDrain(options?: TuneletDrainOptions): Promise<number> {
    this.#draining = true;
    const budget = await this.#request<number>({
      type: "beginDrain",
      stopAfterDrain: options?.stopAfterDrain,
    });
    if (budget === 0) {
      this.#draining = false;
      this.#resolveDrainWaiters();
    }
    return budget;
  }

  /**
   * Resolves when the active drain finishes its tail budget or when another
   * command cancels the drain.
   */
  waitForDrain(): Promise<void> {
    if (!this.#draining || this.#disposed) {
      return Promise.resolve();
    }
    return new Promise<void>((resolve) => {
      this.#drainWaiters.push(resolve);
    });
  }

  /** Returns the configured tail budget in frames. */
  tailFrames(): Promise<number> {
    return this.#request<number>({ type: "tailFrames" });
  }

  /** Reads a `Feedback` snapshot and does not clear the stereo peak window. */
  feedback(): Promise<Feedback> {
    return this.#request<Feedback>({ type: "feedback" });
  }

  /** Reads a `Feedback` snapshot and clears the stereo peak window. */
  takeFeedback(): Promise<Feedback> {
    return this.#request<Feedback>({ type: "takeFeedback" });
  }

  /** Reads a `Telemetry` snapshot and does not clear its peak windows. */
  telemetry(): Promise<Telemetry> {
    return this.#request<Telemetry>({ type: "telemetry" });
  }

  /** Reads a `Telemetry` snapshot and clears its three peak windows. */
  takeTelemetry(): Promise<Telemetry> {
    return this.#request<Telemetry>({ type: "takeTelemetry" });
  }

  /** Reads the analysis and synthesis formant envelopes with `points` samples. */
  formantEnvelope(
    maxFrequencyHz: number,
    points: number,
  ): Promise<FormantEnvelopeResult> {
    return this.#request<FormantEnvelopeResult>({
      type: "formantEnvelope",
      maxFrequencyHz,
      points,
    });
  }

  /** Releases the Wasm processor in the worklet and disconnects the node. */
  async free(): Promise<void> {
    if (this.#disposed) {
      return;
    }
    this.#disposed = true;
    this.#draining = false;
    this.#resolveDrainWaiters();
    try {
      await this.#requestAllowDisposed<undefined>({ type: "free" });
    } finally {
      this.disconnect();
      this.port.close();
      this.#rejectAllPending(new Error("TuneletWorkletNode was disposed"));
    }
  }

  /** Starts asynchronous cleanup for `Symbol.dispose`. */
  [Symbol.dispose](): void {
    void this.free();
  }

  /** Sends one request with a correlation ID over `MessagePort`. */
  #request<T>(command: TuneletWorkletCommand): Promise<T> {
    if (this.#disposed) {
      return Promise.reject(new Error("TuneletWorkletNode is disposed"));
    }
    return this.#requestAllowDisposed<T>(command);
  }

  /** Sends one request also when `free()` shuts down the node. */
  #requestAllowDisposed<T>(command: TuneletWorkletCommand): Promise<T> {
    const id = this.#nextRequestId;
    this.#nextRequestId += 1;
    const message: TuneletWorkletRequest = { ...command, id };
    return new Promise<T>((resolve, reject) => {
      this.#pending.set(id, {
        resolve: resolve as (value: unknown) => void,
        reject,
      });
      this.port.postMessage(message);
    });
  }

  /** Rejects all unresolved `MessagePort` requests. */
  #rejectAllPending(error: Error): void {
    for (const pending of this.#pending.values()) {
      pending.reject(error);
    }
    this.#pending.clear();
    this.#draining = false;
    this.#resolveDrainWaiters();
  }

  /** Resolves all callers that await `waitForDrain()`. */
  #resolveDrainWaiters(): void {
    if (this.#drainWaiters.length === 0) {
      return;
    }
    const waiters = this.#drainWaiters.splice(0, this.#drainWaiters.length);
    for (const resolve of waiters) {
      resolve();
    }
  }
}

/**
 * Registers the `tunelet` worklet processor module on `context.audioWorklet`.
 *
 * The function rejects unsupported `AudioContext` sample rates before it calls
 * `addModule`. Concurrent or repeated calls for the same context and URL share
 * one registration promise. If `addModule` fails, the cached entry clears so a
 * later call can try again.
 */
export async function registerTuneletWorklet(
  context: BaseAudioContext,
  workletUrl?: string | URL,
): Promise<void> {
  validateContextSampleRate(context);
  if (!context.audioWorklet || typeof context.audioWorklet.addModule !== "function") {
    throw new Error(
      "context.audioWorklet is not available (use a secure HTTPS or localhost origin)",
    );
  }

  const resolvedHref =
    workletUrl === undefined
      ? new URL("./worklet-processor.js", import.meta.url).href
      : workletUrl instanceof URL
        ? workletUrl.href
        : String(workletUrl);

  let perContext = registeredContexts.get(context);
  if (!perContext) {
    perContext = new Map<string, Promise<void>>();
    registeredContexts.set(context, perContext);
  }

  let registration = perContext.get(resolvedHref);
  if (!registration) {
    registration = context.audioWorklet
      .addModule(resolvedHref)
      .catch((error: unknown) => {
        perContext?.delete(resolvedHref);
        throw error;
      });
    perContext.set(resolvedHref, registration);
  }
  await registration;
}

/**
 * Loads the Wasm binary on the main thread, registers the worklet processor
 * and returns an initialized `TuneletWorkletNode`.
 *
 * Rejects unsupported `AudioContext` sample rates (rates other than 44100 Hz
 * and 48000 Hz) before it registers the worklet, loads Wasm or creates a node.
 */
export async function createTuneletWorkletNode(
  context: BaseAudioContext,
  options?: TuneletWorkletOptions,
): Promise<TuneletWorkletNode> {
  validateContextSampleRate(context);
  const maxBlockFrames =
    options?.maxBlockFrames ?? DEFAULT_WORKLET_BLOCK_FRAMES;
  validateMaxBlockFrames(maxBlockFrames);
  const outputChannelCount = validateOutputChannels(
    options?.outputChannelCount ?? [2],
  );

  const [wasm] = await Promise.all([
    loadTuneletWasm(options?.wasm),
    registerTuneletWorklet(context, options?.workletUrl),
  ]);

  const node = new TuneletWorkletNode(context, {
    wasm,
    processorName: options?.processorName,
    maxBlockFrames,
    config: options?.config,
    outputChannelCount,
    channelCount: options?.channelCount,
    channelCountMode: options?.channelCountMode,
    channelInterpretation: options?.channelInterpretation,
  });

  try {
    await node.ready;
  } catch (error) {
    node.disconnect();
    node.port.close();
    throw error;
  }
  return node;
}

export default createTuneletWorkletNode;
