/**
 * The shared ESM entry point for browsers and Node.js.
 *
 * In browsers, call `createTuneletNode(audioContext)` to compile Wasm, register
 * the `AudioWorklet` processor and construct a stereo `TuneletWorkletNode`.
 * Call `loadTunelet()` for direct `Tunelet` instances in Node.js or on the main
 * thread. This module re-exports all named exports of the generated module,
 * which include `Tunelet`, `defaultConfig`, `initSync` and the configuration,
 * event and feedback types.
 */
import init, { initSync } from "../pkg/tunelet.js";
import type {
  Feedback,
  InitInput,
  InitOutput,
  ProcessorConfig,
  Telemetry,
} from "../pkg/tunelet.js";
import {
  CORRECTION_MIDI_FLAG,
  FEEDBACK_LEN,
  MIDI_EVENT_WORDS,
  TELEMETRY_LEN,
  TUNELET_PROCESSOR_NAME,
} from "./worklet.js";
import type { TuneletProcessorOptions } from "./worklet.js";

export * from "../pkg/tunelet.js";
export {
  CORRECTION_MIDI_FLAG,
  FEEDBACK_LEN,
  MIDI_EVENT_WORDS,
  TELEMETRY_LEN,
  TUNELET_PROCESSOR_NAME,
};

/** The named indices for the 8-element `Float64Array` of `feedbackInto`. */
export const FEEDBACK_INDEX = {
  resampleRatio: 0,
  periodSamples: 1,
  frequencyHz: 2,
  targetMidiNote: 3,
  voiced: 4,
  peakLevel: 5,
  processedFrames: 6,
  latencyFrames: 7,
} as const;

/** The named indices for the 45-element `Float64Array` of `telemetryInto`. */
export const TELEMETRY_INDEX = {
  processedFrames: 0,
  inputPeak: 1,
  outputPeakLeft: 2,
  outputPeakRight: 3,
  reverbWetPeak: 4,
  clarity: 5,
  tracking: 6,
  voiceCount: 7,
  voiceRatiosOffset: 8,
  voiceGainsOffset: 24,
  punchGain: 40,
  warmGain: 41,
  alienLoopFrames: 42,
  alienPhaseFrames: 43,
  formantGeneration: 44,
} as const;

/** The options for `createTuneletNode` and `TuneletWorkletNode`. */
export interface TuneletNodeOptions {
  /** The optional initial processor configuration. */
  config?: ProcessorConfig;
  /** The maximum frames in one render quantum. The default is 128. */
  maxBlockFrames?: number;
  /** The blocks between meter updates. The default is 4. */
  meterIntervalBlocks?: number;
  /** A custom Wasm source for `createTuneletNode`. */
  wasmSource?: InitInput | Promise<InitInput>;
  /** A custom URL for the `tunelet/worklet` module script. */
  workletUrl?: string | URL;
  /** If you construct `TuneletWorkletNode` directly, give the compiled Wasm module here. */
  module?: WebAssembly.Module;
}

/** A listener callback for meter updates on `TuneletWorkletNode`. */
export type TuneletMeterListener = (
  feedback: Float64Array,
  telemetry: Float64Array,
) => void;

/** The shared initialization promise, which a failure clears. */
let initialization: Promise<InitOutput> | undefined;

/** The cached default compiled `WebAssembly.Module`, which a failure clears. */
let defaultWasmModule: Promise<WebAssembly.Module> | undefined;

/** Tracks worklet module registration for each `BaseAudioContext`. */
const registeredContexts = new WeakMap<BaseAudioContext, Promise<void>>();

/** The MIDI ring capacity in 4-word events when `SharedArrayBuffer` exists. */
const SHARED_MIDI_CAPACITY = 256;

/** Returns the nominal latency in host frames for a supported sample rate. */
function nominalLatencyForRate(sampleRate: number): number {
  switch (sampleRate) {
    case 44100:
    case 48000:
      return 1593;
    case 88200:
    case 96000:
      return 3190;
    case 176400:
    case 192000:
      return 6384;
    default:
      throw new Error(
        "sampleRate must be 44100, 48000, 88200, 96000, 176400 or 192000",
      );
  }
}

const BaseAudioWorkletNode: typeof AudioWorkletNode =
  typeof AudioWorkletNode === "function"
    ? AudioWorkletNode
    : (class {} as unknown as typeof AudioWorkletNode);

/**
 * A stereo `AudioWorkletNode` that runs `tunelet` on the Web Audio render
 * thread.
 *
 * Construct it with `await createTuneletNode(audioContext, options)`. The node
 * accepts mono or multi-channel input, outputs stereo audio, applies
 * sample-accurate MIDI controls and exposes typed-array feedback and telemetry
 * buffers.
 */
export class TuneletWorkletNode extends BaseAudioWorkletNode {
  /** The live 8-element `Float64Array` view of the latest `Feedback` snapshot. */
  readonly feedbackData: Float64Array;
  /** The live 45-element `Float64Array` view of the latest `Telemetry` snapshot. */
  readonly telemetryData: Float64Array;
  /** The nominal processor latency in host frames, with resampler delay included. */
  readonly latencyFrames: number;

  private readonly midiRingHeader: Int32Array | null;
  private readonly midiRingData: Uint32Array | null;
  private readonly sharedMeters: boolean;
  private readonly listeners = new Set<TuneletMeterListener>();

  constructor(
    context: BaseAudioContext,
    options: TuneletNodeOptions & { module: WebAssembly.Module },
  ) {
    if (typeof AudioWorkletNode !== "function") {
      throw new Error("AudioWorkletNode is not available in this environment.");
    }
    const latencyFrames = nominalLatencyForRate(context.sampleRate);
    const canShare =
      typeof SharedArrayBuffer === "function" && typeof Atomics === "object";

    const midiRingBuffer = canShare
      ? new SharedArrayBuffer(8 + SHARED_MIDI_CAPACITY * MIDI_EVENT_WORDS * 4)
      : undefined;
    const meterBuffer = canShare
      ? new SharedArrayBuffer(
          (FEEDBACK_LEN + TELEMETRY_LEN) * Float64Array.BYTES_PER_ELEMENT,
        )
      : undefined;

    const processorOptions: TuneletProcessorOptions = {
      module: options.module,
      config: options.config,
      maxBlockFrames: options.maxBlockFrames ?? 128,
      sampleRate: context.sampleRate,
      meterIntervalBlocks: options.meterIntervalBlocks ?? 4,
      midiRingBuffer,
      meterBuffer,
      postMeters: !canShare,
    };

    super(context, TUNELET_PROCESSOR_NAME, {
      numberOfInputs: 1,
      numberOfOutputs: 1,
      outputChannelCount: [2],
      channelCount: 1,
      channelCountMode: "explicit",
      channelInterpretation: "speakers",
      processorOptions,
    });

    this.latencyFrames = latencyFrames;
    this.sharedMeters = canShare;

    if (midiRingBuffer && meterBuffer) {
      this.midiRingHeader = new Int32Array(midiRingBuffer, 0, 2);
      this.midiRingData = new Uint32Array(
        midiRingBuffer,
        8,
        SHARED_MIDI_CAPACITY * MIDI_EVENT_WORDS,
      );
      this.feedbackData = new Float64Array(meterBuffer, 0, FEEDBACK_LEN);
      this.telemetryData = new Float64Array(
        meterBuffer,
        FEEDBACK_LEN * Float64Array.BYTES_PER_ELEMENT,
        TELEMETRY_LEN,
      );
    } else {
      this.midiRingHeader = null;
      this.midiRingData = null;
      this.feedbackData = new Float64Array(FEEDBACK_LEN);
      this.feedbackData[FEEDBACK_INDEX.latencyFrames] = latencyFrames;
      this.telemetryData = new Float64Array(TELEMETRY_LEN);
    }

    this.port.onmessage = (
      event: MessageEvent<{
        type?: string;
        feedback?: Float64Array;
        telemetry?: Float64Array;
      }>,
    ) => {
      const data = event.data;
      if (data && data.type === "meters" && data.feedback && data.telemetry) {
        if (!this.sharedMeters) {
          this.feedbackData.set(data.feedback);
          this.telemetryData.set(data.telemetry);
        }
        for (const listener of this.listeners) {
          listener(this.feedbackData, this.telemetryData);
        }
      }
    };
  }

  /** Replaces the active processor configuration on the audio thread. */
  setConfig(config: ProcessorConfig): void {
    this.port.postMessage({ type: "config", config });
  }

  /** Sends a 3-byte harmony MIDI message at an optional frame offset. */
  sendMidi(bytes: ArrayLike<number>, offset = 0): void {
    if (bytes.length !== 3) {
      throw new RangeError("MIDI message must contain 3 bytes.");
    }
    this.enqueueMidi(offset, bytes[0] & 0xff, bytes[1] & 0x7f, bytes[2] & 0x7f);
  }

  /** Sends a 3-byte correction keyboard MIDI message at an optional offset. */
  sendCorrectionMidi(bytes: ArrayLike<number>, offset = 0): void {
    if (bytes.length !== 3) {
      throw new RangeError("MIDI message must contain 3 bytes.");
    }
    this.enqueueMidi(
      offset,
      (bytes[0] & 0xff) | CORRECTION_MIDI_FLAG,
      bytes[1] & 0x7f,
      bytes[2] & 0x7f,
    );
  }

  /** Sends packed `[offset, status, data1, data2]` MIDI words. */
  sendMidiWords(words: Uint32Array): void {
    if (words.length % MIDI_EVENT_WORDS !== 0) {
      throw new RangeError("MIDI word length must be a multiple of 4.");
    }
    for (let i = 0; i < words.length; i += MIDI_EVENT_WORDS) {
      this.enqueueMidi(words[i], words[i + 1], words[i + 2], words[i + 3]);
    }
  }

  /** Sends a harmony MIDI Note On event (`channel` 0..15, `note` 0..127). */
  noteOn(channel: number, note: number, velocity: number, offset = 0): void {
    this.enqueueMidi(
      offset,
      0x90 | (channel & 0x0f),
      note & 0x7f,
      velocity & 0x7f,
    );
  }

  /** Sends a harmony MIDI Note Off event (`channel` 0..15, `note` 0..127). */
  noteOff(channel: number, note: number, offset = 0): void {
    this.enqueueMidi(offset, 0x80 | (channel & 0x0f), note & 0x7f, 0);
  }

  /** Releases all active harmony MIDI notes (controller 123). */
  allNotesOff(offset = 0): void {
    this.enqueueMidi(offset, 0xb0, 123, 0);
  }

  /** Starts the finite tail drain on the audio thread. */
  beginDrain(): void {
    this.port.postMessage({ type: "drain" });
  }

  /** Resets the processor audio histories and feedback on the audio thread. */
  reset(): void {
    this.port.postMessage({ type: "reset" });
  }

  /** Requests an immediate meter snapshot from the audio thread. */
  requestMeters(): void {
    this.port.postMessage({ type: "requestMeters" });
  }

  /** Subscribes to meter updates and returns an unsubscribe function. */
  onMeters(listener: TuneletMeterListener): () => void {
    this.listeners.add(listener);
    if (this.sharedMeters) {
      this.port.postMessage({ type: "postMeters", enabled: true });
    }
    return () => {
      this.listeners.delete(listener);
      if (this.sharedMeters && this.listeners.size === 0) {
        this.port.postMessage({ type: "postMeters", enabled: false });
      }
    };
  }

  /** Converts the current `feedbackData` buffer into a `Feedback` object. */
  feedback(): Feedback {
    const data = this.feedbackData;
    const voiced = data[FEEDBACK_INDEX.voiced] !== 0;
    const note = data[FEEDBACK_INDEX.targetMidiNote];
    return {
      resampleRatio: data[FEEDBACK_INDEX.resampleRatio],
      periodSamples: data[FEEDBACK_INDEX.periodSamples],
      frequencyHz: data[FEEDBACK_INDEX.frequencyHz],
      targetMidiNote: voiced && !Number.isNaN(note) ? note : undefined,
      voiced,
      peakLevel: data[FEEDBACK_INDEX.peakLevel],
      processedFrames: BigInt(Math.trunc(data[FEEDBACK_INDEX.processedFrames])),
      latencyFrames: data[FEEDBACK_INDEX.latencyFrames],
    };
  }

  /** Converts the current `telemetryData` buffer into a `Telemetry` object. */
  telemetry(): Telemetry {
    const d = this.telemetryData;
    const ratios = Array.from(
      d.subarray(
        TELEMETRY_INDEX.voiceRatiosOffset,
        TELEMETRY_INDEX.voiceRatiosOffset + 16,
      ),
    ) as Telemetry["voiceRatios"];
    const gains = Array.from(
      d.subarray(
        TELEMETRY_INDEX.voiceGainsOffset,
        TELEMETRY_INDEX.voiceGainsOffset + 16,
      ),
    ) as Telemetry["voiceGains"];
    return {
      processedFrames: BigInt(Math.trunc(d[TELEMETRY_INDEX.processedFrames])),
      inputPeak: d[TELEMETRY_INDEX.inputPeak],
      outputPeak: [
        d[TELEMETRY_INDEX.outputPeakLeft],
        d[TELEMETRY_INDEX.outputPeakRight],
      ],
      reverbWetPeak: d[TELEMETRY_INDEX.reverbWetPeak],
      clarity: d[TELEMETRY_INDEX.clarity],
      tracking: d[TELEMETRY_INDEX.tracking] !== 0,
      voiceCount: d[TELEMETRY_INDEX.voiceCount],
      voiceRatios: ratios,
      voiceGains: gains,
      punchGain: d[TELEMETRY_INDEX.punchGain],
      warmGain: d[TELEMETRY_INDEX.warmGain],
      alienLoopFrames: d[TELEMETRY_INDEX.alienLoopFrames],
      alienPhaseFrames: d[TELEMETRY_INDEX.alienPhaseFrames],
      formantGeneration: d[TELEMETRY_INDEX.formantGeneration],
    };
  }

  /** Releases the Wasm processor on the audio thread and disconnects the node. */
  free(): void {
    this.port.postMessage({ type: "free" });
    this.disconnect();
  }

  [Symbol.dispose](): void {
    this.free();
  }

  private enqueueMidi(
    offset: number,
    status: number,
    data1: number,
    data2: number,
  ): void {
    if (this.midiRingHeader && this.midiRingData) {
      const write = Atomics.load(this.midiRingHeader, 0) >>> 0;
      const read = Atomics.load(this.midiRingHeader, 1) >>> 0;
      if (((write - read) >>> 0) < SHARED_MIDI_CAPACITY) {
        const slot = (write % SHARED_MIDI_CAPACITY) * MIDI_EVENT_WORDS;
        this.midiRingData[slot] = offset > 0 ? offset >>> 0 : 0;
        this.midiRingData[slot + 1] = status >>> 0;
        this.midiRingData[slot + 2] = data1 & 0x7f;
        this.midiRingData[slot + 3] = data2 & 0x7f;
        Atomics.store(this.midiRingHeader, 0, (write + 1) >>> 0);
        return;
      }
    }
    this.port.postMessage({
      type: "midi",
      offset: offset > 0 ? offset >>> 0 : 0,
      status: status >>> 0,
      data1: data1 & 0x7f,
      data2: data2 & 0x7f,
    });
  }
}

/**
 * Compiles the Wasm binary, registers `tunelet/worklet` on `context` and
 * returns a stereo `TuneletWorkletNode`.
 *
 * Supported context sample rates are 44100, 48000, 88200, 96000, 176400 and
 * 192000 Hz.
 */
export async function createTuneletNode(
  context: BaseAudioContext,
  options?: TuneletNodeOptions,
): Promise<TuneletWorkletNode> {
  if (!context || !context.audioWorklet) {
    throw new Error("context.audioWorklet is not available.");
  }
  nominalLatencyForRate(context.sampleRate);

  const modulePromise =
    options?.module !== undefined
      ? Promise.resolve(options.module)
      : options?.wasmSource !== undefined
        ? compileWasmModule(options.wasmSource)
        : (defaultWasmModule ??= compileWasmModule(
            new URL("../pkg/tunelet_bg.wasm", import.meta.url),
          ).catch((error: unknown) => {
            defaultWasmModule = undefined;
            throw error;
          }));

  const workletUrl =
    options?.workletUrl ?? new URL("./worklet.js", import.meta.url);
  let registration = registeredContexts.get(context);
  if (!registration) {
    registration = context.audioWorklet
      .addModule(workletUrl)
      .catch((error: unknown) => {
        registeredContexts.delete(context);
        throw error;
      });
    registeredContexts.set(context, registration);
  }

  const [module] = await Promise.all([modulePromise, registration]);
  await (initialization ??= init({ module_or_path: module }).catch(
    (error: unknown) => {
      initialization = undefined;
      throw error;
    },
  ));
  return new TuneletWorkletNode(context, { ...options, module });
}

/** Compiles a `WebAssembly.Module` from a URL, `Response`, buffer or module. */
async function compileWasmModule(
  source: InitInput | Promise<InitInput>,
): Promise<WebAssembly.Module> {
  const input = await source;
  if (input instanceof WebAssembly.Module) {
    return input;
  }
  if (input instanceof URL && input.protocol === "file:") {
    const moduleName = "node:fs/promises";
    const fs: typeof import("node:fs/promises") = await import(
      /* webpackIgnore: true */ /* @vite-ignore */ moduleName
    );
    return WebAssembly.compile(await fs.readFile(input));
  }
  if (
    typeof input === "string" ||
    input instanceof URL ||
    (typeof Request === "function" && input instanceof Request)
  ) {
    return compileResponse(await fetch(input));
  }
  if (typeof Response === "function" && input instanceof Response) {
    return compileResponse(input);
  }
  return WebAssembly.compile(input as BufferSource);
}

/** Compiles a `WebAssembly.Module` from an HTTP `Response`. */
async function compileResponse(
  response: Response,
): Promise<WebAssembly.Module> {
  if (!response.ok) {
    throw new Error(
      `Failed to fetch Wasm: ${response.status} ${response.statusText}`,
    );
  }
  if (typeof WebAssembly.compileStreaming === "function") {
    try {
      return await WebAssembly.compileStreaming(response.clone());
    } catch {
      // If the server omits application/wasm, compile from the byte buffer.
    }
  }
  return WebAssembly.compile(await response.arrayBuffer());
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
    return initSync({ module: await fs.readFile(input) });
  }

  return init({ module_or_path: input });
}

/** The default export is the same function as `loadTunelet`. */
export default loadTunelet;
