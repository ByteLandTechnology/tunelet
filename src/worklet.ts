/**
 * The `AudioWorklet` runtime module (`tunelet/worklet`).
 *
 * This module installs the internal UTF-8 shim before the generated Wasm
 * bindings evaluate. Inside an `AudioWorkletGlobalScope`, it registers
 * `"tunelet-processor"` and runs 128-frame `Float32Array` blocks, MIDI
 * controls and meter snapshots with preallocated buffers.
 */
import "./utf8-shim.js";
import { initSync, Tunelet } from "../pkg/tunelet.js";
import type { ProcessorConfig, SyncInitInput } from "../pkg/tunelet.js";

/** The registered `AudioWorkletProcessor` name. */
export const TUNELET_PROCESSOR_NAME = "tunelet-processor";

/** The number of `f64` elements in a typed-array `Feedback` snapshot. */
export const FEEDBACK_LEN = 8;

/** The number of `f64` elements in a typed-array `Telemetry` snapshot. */
export const TELEMETRY_LEN = 45;

/** The number of `u32` words for each timed MIDI event in a `Uint32Array`. */
export const MIDI_EVENT_WORDS = 4;

/** The status flag that selects the correction MIDI keyboard in a `Uint32Array`. */
export const CORRECTION_MIDI_FLAG = 0x100;

/** The maximum number of MIDI events in one worklet block buffer. */
const MAX_BLOCK_MIDI = 128;

/** The options in `AudioWorkletNodeOptions.processorOptions`. */
export interface TuneletProcessorOptions {
  /** The compiled Wasm module or raw bytes for synchronous setup. */
  module: SyncInitInput;
  /** The optional initial processor configuration. */
  config?: ProcessorConfig;
  /** The maximum frames in one render call. The default is 128. */
  maxBlockFrames?: number;
  /** An explicit sample rate. The default is the global worklet rate. */
  sampleRate?: number;
  /** The blocks between meter updates. Zero disables automatic meter updates. */
  meterIntervalBlocks?: number;
  /** An optional `SharedArrayBuffer` for the MIDI ring words. */
  midiRingBuffer?: SharedArrayBuffer;
  /** An optional `SharedArrayBuffer` for the feedback and telemetry `f64` values. */
  meterBuffer?: SharedArrayBuffer;
  /** If `true` and no shared buffer exists, posts meter updates on the `MessagePort`. */
  postMeters?: boolean;
}

/** The control messages from `TuneletWorkletNode` to the worklet port. */
export type TuneletWorkletInboundMessage =
  | { type: "config"; config: ProcessorConfig }
  | {
      type: "midi";
      offset: number;
      status: number;
      data1: number;
      data2: number;
    }
  | { type: "midiWords"; words: Uint32Array }
  | { type: "drain" }
  | { type: "reset" }
  | { type: "free" }
  | { type: "postMeters"; enabled: boolean }
  | { type: "requestMeters" };

/** The minimal shape of `AudioWorkletProcessor` in a worklet scope. */
export interface AudioWorkletProcessorInstance {
  readonly port: MessagePort;
  process(
    inputs: Float32Array[][],
    outputs: Float32Array[][],
    parameters: Record<string, Float32Array>,
  ): boolean;
}

interface AudioWorkletProcessorConstructor {
  new (options?: {
    processorOptions?: TuneletProcessorOptions;
  }): AudioWorkletProcessorInstance;
}

interface WorkletScope {
  readonly sampleRate?: number;
  readonly AudioWorkletProcessor?: AudioWorkletProcessorConstructor;
  readonly registerProcessor?: (
    name: string,
    processorCtor: unknown,
  ) => void;
  __tuneletRegistered?: boolean;
}

const workletScope = globalThis as unknown as WorkletScope;

const BaseProcessor: AudioWorkletProcessorConstructor =
  typeof workletScope.AudioWorkletProcessor === "function"
    ? workletScope.AudioWorkletProcessor
    : (class {
        readonly port: MessagePort;
        constructor() {
          const channel =
            typeof MessageChannel === "function" ? new MessageChannel() : null;
          this.port = (channel ? channel.port1 : {}) as MessagePort;
        }
        process(): boolean {
          return true;
        }
      } as unknown as AudioWorkletProcessorConstructor);

/**
 * The `AudioWorkletProcessor` implementation for `tunelet`.
 *
 * Construction initializes Wasm synchronously and preallocates all audio,
 * MIDI and meter views. Steady-state `process` calls execute without scalar
 * sample conversion loops or heap allocations.
 */
export class TuneletWorkletProcessor extends BaseProcessor {
  private processor: Tunelet | null;
  private readonly maxBlockFrames: number;
  private readonly silentViews: Float32Array[];
  private readonly monoViews: Float32Array[];
  private readonly rightViews: Float32Array[];
  private readonly midiWords: Uint32Array;
  private readonly midiViews: Uint32Array[];
  private queuedMidiCount = 0;
  private readonly midiRingHeader: Int32Array | null;
  private readonly midiRingData: Uint32Array | null;
  private readonly midiRingCapacity: number;
  private readonly feedbackView: Float64Array;
  private readonly telemetryView: Float64Array;
  private readonly sharedMeters: boolean;
  private postMeters: boolean;
  private readonly meterIntervalBlocks: number;
  private meterBlockCounter = 0;
  private readonly meterMessage: {
    readonly type: "meters";
    readonly feedback: Float64Array;
    readonly telemetry: Float64Array;
  };
  private draining = false;
  private disposed = false;

  constructor(options?: { processorOptions?: TuneletProcessorOptions }) {
    super(options);
    const procOptions = options?.processorOptions;
    if (!procOptions || !procOptions.module) {
      throw new Error("processorOptions.module must provide a Wasm module.");
    }
    initSync({ module: procOptions.module });

    const rate = procOptions.sampleRate ?? workletScope.sampleRate ?? 48000;
    const maxBlockFrames = procOptions.maxBlockFrames ?? 128;
    this.maxBlockFrames = maxBlockFrames;
    this.processor = new Tunelet(rate, maxBlockFrames, procOptions.config);

    const silentBacking = new Float32Array(maxBlockFrames);
    const monoBacking = new Float32Array(maxBlockFrames);
    const rightBacking = new Float32Array(maxBlockFrames);
    this.silentViews = new Array<Float32Array>(maxBlockFrames + 1);
    this.monoViews = new Array<Float32Array>(maxBlockFrames + 1);
    this.rightViews = new Array<Float32Array>(maxBlockFrames + 1);
    for (let length = 0; length <= maxBlockFrames; length += 1) {
      this.silentViews[length] = silentBacking.subarray(0, length);
      this.monoViews[length] = monoBacking.subarray(0, length);
      this.rightViews[length] = rightBacking.subarray(0, length);
    }

    this.midiWords = new Uint32Array(MAX_BLOCK_MIDI * MIDI_EVENT_WORDS);
    this.midiViews = new Array<Uint32Array>(MAX_BLOCK_MIDI + 1);
    for (let count = 0; count <= MAX_BLOCK_MIDI; count += 1) {
      this.midiViews[count] = this.midiWords.subarray(
        0,
        count * MIDI_EVENT_WORDS,
      );
    }

    if (
      procOptions.midiRingBuffer &&
      typeof Atomics === "object" &&
      procOptions.midiRingBuffer.byteLength >= 24
    ) {
      this.midiRingHeader = new Int32Array(procOptions.midiRingBuffer, 0, 2);
      const wordLength = (procOptions.midiRingBuffer.byteLength - 8) >>> 2;
      this.midiRingCapacity = (wordLength / MIDI_EVENT_WORDS) | 0;
      this.midiRingData = new Uint32Array(
        procOptions.midiRingBuffer,
        8,
        this.midiRingCapacity * MIDI_EVENT_WORDS,
      );
    } else {
      this.midiRingHeader = null;
      this.midiRingData = null;
      this.midiRingCapacity = 0;
    }

    if (
      procOptions.meterBuffer &&
      procOptions.meterBuffer.byteLength >=
        (FEEDBACK_LEN + TELEMETRY_LEN) * Float64Array.BYTES_PER_ELEMENT
    ) {
      this.feedbackView = new Float64Array(
        procOptions.meterBuffer,
        0,
        FEEDBACK_LEN,
      );
      this.telemetryView = new Float64Array(
        procOptions.meterBuffer,
        FEEDBACK_LEN * Float64Array.BYTES_PER_ELEMENT,
        TELEMETRY_LEN,
      );
      this.sharedMeters = true;
    } else {
      this.feedbackView = new Float64Array(FEEDBACK_LEN);
      this.telemetryView = new Float64Array(TELEMETRY_LEN);
      this.sharedMeters = false;
    }

    this.meterIntervalBlocks = procOptions.meterIntervalBlocks ?? 4;
    this.postMeters = Boolean(procOptions.postMeters);
    this.meterMessage = {
      type: "meters",
      feedback: this.feedbackView,
      telemetry: this.telemetryView,
    };

    this.processor.feedbackInto(this.feedbackView);
    this.processor.telemetryInto(this.telemetryView);

    if (this.port && typeof this.port.onmessage !== "undefined") {
      this.port.onmessage = (event: MessageEvent<TuneletWorkletInboundMessage>) => {
        this.handleMessage(event.data);
      };
    }
  }

  private insertMidi(
    offset: number,
    status: number,
    data1: number,
    data2: number,
  ): void {
    if (this.queuedMidiCount >= MAX_BLOCK_MIDI) {
      return;
    }
    const clampedOffset = offset > 0 ? offset >>> 0 : 0;
    let slot = this.queuedMidiCount;
    while (
      slot > 0 &&
      this.midiWords[(slot - 1) * MIDI_EVENT_WORDS] > clampedOffset
    ) {
      const src = (slot - 1) * MIDI_EVENT_WORDS;
      const dst = slot * MIDI_EVENT_WORDS;
      this.midiWords[dst] = this.midiWords[src];
      this.midiWords[dst + 1] = this.midiWords[src + 1];
      this.midiWords[dst + 2] = this.midiWords[src + 2];
      this.midiWords[dst + 3] = this.midiWords[src + 3];
      slot -= 1;
    }
    const base = slot * MIDI_EVENT_WORDS;
    this.midiWords[base] = clampedOffset;
    this.midiWords[base + 1] = status >>> 0;
    this.midiWords[base + 2] = data1 & 0x7f;
    this.midiWords[base + 3] = data2 & 0x7f;
    this.queuedMidiCount += 1;
  }

  private pullSharedMidi(): void {
    if (!this.midiRingHeader || !this.midiRingData) {
      return;
    }
    const write = Atomics.load(this.midiRingHeader, 0);
    let read = Atomics.load(this.midiRingHeader, 1);
    while (read !== write && this.queuedMidiCount < MAX_BLOCK_MIDI) {
      const slot = (read % this.midiRingCapacity) * MIDI_EVENT_WORDS;
      this.insertMidi(
        this.midiRingData[slot],
        this.midiRingData[slot + 1],
        this.midiRingData[slot + 2],
        this.midiRingData[slot + 3],
      );
      read = (read + 1) >>> 0;
    }
    Atomics.store(this.midiRingHeader, 1, read);
  }

  private handleMessage(message: TuneletWorkletInboundMessage): void {
    if (!this.processor || !message) {
      return;
    }
    switch (message.type) {
      case "config":
        this.processor.setConfig(message.config);
        this.draining = false;
        break;
      case "midi":
        this.insertMidi(
          message.offset,
          message.status,
          message.data1,
          message.data2,
        );
        this.draining = false;
        break;
      case "midiWords": {
        const words = message.words;
        for (let i = 0; i + 3 < words.length; i += MIDI_EVENT_WORDS) {
          this.insertMidi(words[i], words[i + 1], words[i + 2], words[i + 3]);
        }
        this.draining = false;
        break;
      }
      case "drain":
        this.processor.beginDrain();
        this.draining = true;
        break;
      case "reset":
        this.processor.reset();
        this.queuedMidiCount = 0;
        this.draining = false;
        break;
      case "postMeters":
        this.postMeters = message.enabled;
        break;
      case "requestMeters":
        this.processor.takeFeedbackInto(this.feedbackView);
        this.processor.takeTelemetryInto(this.telemetryView);
        if (!this.sharedMeters) {
          this.port.postMessage(this.meterMessage);
        }
        break;
      case "free":
        this.processor.free();
        this.processor = null;
        this.disposed = true;
        break;
    }
  }

  override process(
    inputs: Float32Array[][],
    outputs: Float32Array[][],
  ): boolean {
    if (this.disposed || !this.processor) {
      return false;
    }
    const outputBus = outputs[0];
    if (!outputBus || outputBus.length === 0) {
      return true;
    }
    const outLeft = outputBus[0];
    const frames = outLeft.length;
    if (frames === 0 || frames > this.maxBlockFrames) {
      return true;
    }
    const hasRight = outputBus.length > 1 && outputBus[1].length === frames;
    const outRight = hasRight ? outputBus[1] : this.rightViews[frames];

    this.pullSharedMidi();

    if (this.draining && this.queuedMidiCount === 0) {
      const rendered = this.processor.drainFloat32(outLeft, outRight);
      if (rendered === 0) {
        this.draining = false;
      }
    } else {
      this.draining = false;
      const inputBus = inputs[0];
      let monoInput: Float32Array;
      if (!inputBus || inputBus.length === 0 || inputBus[0].length !== frames) {
        monoInput = this.silentViews[frames];
      } else if (inputBus.length === 1) {
        monoInput = inputBus[0];
      } else {
        const downmix = this.monoViews[frames];
        const ch0 = inputBus[0];
        const ch1 = inputBus[1];
        for (let i = 0; i < frames; i += 1) {
          downmix[i] = (ch0[i] + ch1[i]) * 0.5;
        }
        monoInput = downmix;
      }

      const midiCount = this.queuedMidiCount;
      if (midiCount > 0) {
        const maxOffset = frames - 1;
        for (let i = 0; i < midiCount; i += 1) {
          const idx = i * MIDI_EVENT_WORDS;
          if (this.midiWords[idx] > maxOffset) {
            this.midiWords[idx] = maxOffset;
          }
        }
        this.queuedMidiCount = 0;
        this.processor.processFloat32(
          monoInput,
          outLeft,
          outRight,
          this.midiViews[midiCount],
        );
      } else {
        this.processor.processFloat32(monoInput, outLeft, outRight);
      }
    }

    if (!hasRight) {
      for (let i = 0; i < frames; i += 1) {
        outLeft[i] = (outLeft[i] + outRight[i]) * 0.5;
      }
    }

    if (this.meterIntervalBlocks > 0) {
      this.meterBlockCounter += 1;
      if (this.meterBlockCounter >= this.meterIntervalBlocks) {
        this.meterBlockCounter = 0;
        this.processor.takeFeedbackInto(this.feedbackView);
        this.processor.takeTelemetryInto(this.telemetryView);
        if (!this.sharedMeters && this.postMeters) {
          this.port.postMessage(this.meterMessage);
        }
      }
    }

    return true;
  }
}

if (
  typeof workletScope.registerProcessor === "function" &&
  !workletScope.__tuneletRegistered
) {
  workletScope.__tuneletRegistered = true;
  workletScope.registerProcessor(
    TUNELET_PROCESSOR_NAME,
    TuneletWorkletProcessor,
  );
}
