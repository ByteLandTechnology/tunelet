/**
 * The `AudioWorkletProcessor` module for Web Audio graphs.
 *
 * Load this module with `audioContext.audioWorklet.addModule(...)` or with
 * `createTuneletWorkletNode` from `tunelet/worklet`. The processor calls
 * `initSync` with the `WebAssembly.Module` or `BufferSource` in
 * `processorOptions.module` and does not call `fetch()`.
 */
import "./text-codec.js";
import { initSync, Tunelet } from "../pkg/tunelet.js";
import type {
  Feedback,
  MidiEvent,
  ProcessorConfig,
  SyncInitInput,
  Telemetry,
  TimedEvent,
} from "../pkg/tunelet.js";

/** The registered Web Audio processor name for `TuneletWorkletProcessor`. */
export const TUNELET_WORKLET_PROCESSOR_NAME = "tunelet-processor";

/** The default maximum frame count for one internal DSP chunk. */
const DEFAULT_MAX_BLOCK_FRAMES = 128;

/** The options inside `AudioWorkletNodeOptions.processorOptions`. */
export interface TuneletWorkletProcessorOptions {
  /** The precompiled `WebAssembly.Module` or raw Wasm bytes for `initSync`. */
  module: SyncInitInput;
  /** The maximum frame count for one DSP call. The default is 128. */
  maxBlockFrames?: number;
  /** The optional initial processor configuration. */
  config?: ProcessorConfig;
}

/** The options object that Web Audio passes to the processor constructor. */
export interface TuneletAudioWorkletNodeInit {
  /** The `Tunelet` initialization options from the main thread. */
  processorOptions?: TuneletWorkletProcessorOptions;
}

/** The LPC formant envelopes that `formantEnvelope` writes. */
export interface FormantEnvelopeResult {
  /** `true` if the envelopes come from an active LPC synthesis filter. */
  valid: boolean;
  /** The vocal-tract envelope from the LPC analysis. */
  input: Float32Array;
  /** The vocal-tract envelope after the formant width and stretch controls. */
  output: Float32Array;
}

/** A control or query message sent to `TuneletWorkletProcessor` over `port`. */
export type TuneletWorkletRequest =
  | { type: "config"; requestId?: number }
  | { type: "setConfig"; requestId?: number; config: ProcessorConfig }
  | { type: "midi"; requestId?: number; event: MidiEvent; offset?: number }
  | {
      type: "correctionMidi";
      requestId?: number;
      event: MidiEvent;
      offset?: number;
    }
  | { type: "events"; requestId?: number; events: TimedEvent[] }
  | { type: "reset"; requestId?: number }
  | { type: "beginDrain"; requestId?: number }
  | { type: "drain"; requestId?: number }
  | { type: "tailFrames"; requestId?: number }
  | { type: "feedback"; requestId?: number }
  | { type: "takeFeedback"; requestId?: number }
  | { type: "telemetry"; requestId?: number }
  | { type: "takeTelemetry"; requestId?: number }
  | {
      type: "formantEnvelope";
      requestId?: number;
      maxFrequencyHz: number;
      points: number;
    }
  | { type: "dispose"; requestId?: number };

/** A reply or event message posted from `TuneletWorkletProcessor` on `port`. */
export type TuneletWorkletResponse =
  | { type: "config"; requestId?: number; value: ProcessorConfig }
  | { type: "setConfig"; requestId?: number }
  | { type: "midi"; requestId?: number }
  | { type: "correctionMidi"; requestId?: number }
  | { type: "events"; requestId?: number }
  | { type: "reset"; requestId?: number }
  | { type: "beginDrain"; requestId?: number; value: number }
  | { type: "drain"; requestId?: number; value: number }
  | { type: "drainComplete" }
  | { type: "tailFrames"; requestId?: number; value: number }
  | { type: "feedback"; requestId?: number; value: Feedback }
  | { type: "takeFeedback"; requestId?: number; value: Feedback }
  | { type: "telemetry"; requestId?: number; value: Telemetry }
  | { type: "takeTelemetry"; requestId?: number; value: Telemetry }
  | {
      type: "formantEnvelope";
      requestId?: number;
      value: FormantEnvelopeResult;
    }
  | { type: "dispose"; requestId?: number }
  | { type: "error"; requestId?: number; message: string };

/** Minimal interface of the host `AudioWorkletProcessor` base class. */
export interface AudioWorkletProcessorBase {
  /** The bidirectional `MessagePort` connected to the `AudioWorkletNode`. */
  readonly port: MessagePort;
}

interface AudioWorkletGlobalBindings {
  sampleRate?: number;
  AudioWorkletProcessor?: new (
    options?: TuneletAudioWorkletNodeInit,
  ) => AudioWorkletProcessorBase;
  registerProcessor?: (
    name: string,
    processorCtor: new (
      options?: TuneletAudioWorkletNodeInit,
    ) => AudioWorkletProcessorBase,
  ) => void;
}

const workletGlobal = globalThis as unknown as AudioWorkletGlobalBindings;

const BaseAudioWorkletProcessor: new (
  options?: TuneletAudioWorkletNodeInit,
) => AudioWorkletProcessorBase =
  typeof workletGlobal.AudioWorkletProcessor === "function"
    ? workletGlobal.AudioWorkletProcessor
    : class {
        readonly port!: MessagePort;
        constructor() {
          throw new Error(
            "TuneletWorkletProcessor must run in AudioWorkletGlobalScope",
          );
        }
      };

/** Checks that a number is an integer in `min..=max`. */
function isIntegerInRange(value: unknown, min: number, max: number): boolean {
  return (
    typeof value === "number" &&
    Number.isInteger(value) &&
    value >= min &&
    value <= max
  );
}

/** Validates a `MidiEvent` object before it enters the block queue. */
function validateMidiEvent(event: MidiEvent): void {
  if (typeof event !== "object" || event === null) {
    throw new Error("MIDI message is not valid");
  }
  switch (event.type) {
    case "noteOn":
      if (
        !isIntegerInRange(event.channel, 0, 15) ||
        !isIntegerInRange(event.note, 0, 127) ||
        !isIntegerInRange(event.velocity, 0, 127)
      ) {
        throw new Error("MIDI message is not valid");
      }
      return;
    case "noteOff":
      if (
        !isIntegerInRange(event.channel, 0, 15) ||
        !isIntegerInRange(event.note, 0, 127)
      ) {
        throw new Error("MIDI message is not valid");
      }
      return;
    case "allNotesOff":
      return;
    default:
      throw new Error("MIDI message is not valid");
  }
}

/** Validates a non-negative integer frame offset. */
function validateOffset(offset: unknown): number {
  if (!isIntegerInRange(offset, 0, 0xffff_ffff)) {
    throw new Error("event offset must be a non-negative integer");
  }
  return offset as number;
}

/** Converts an unknown error value to a readable message string. */
function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Processes Web Audio `Float32Array` channels with a synchronous `Tunelet`
 * instance.
 *
 * Stereo and multi-channel inputs mix to mono with equal channel weights
 * before `Tunelet` renders stereo (or mono) output. Disconnected inputs output
 * silence unless a tail drain or queued control event is active. If a host
 * block is larger than `maxBlockFrames`, this processor splits the block into
 * chunks of at most `maxBlockFrames` frames.
 */
export class TuneletWorkletProcessor extends BaseAudioWorkletProcessor {
  private readonly processor: Tunelet;
  private readonly maxBlockFrames: number;
  private readonly monoChunk: Float32Array;
  private readonly monoChunk128: Float32Array | undefined;
  private readonly silentChunk: Float32Array;
  private readonly silentChunk128: Float32Array | undefined;
  private pendingEvents: TimedEvent[] = [];
  private draining = false;
  private disposed = false;

  constructor(options?: TuneletAudioWorkletNodeInit) {
    super(options);
    const procOptions = options?.processorOptions;
    if (!procOptions || procOptions.module === undefined || procOptions.module === null) {
      throw new Error(
        "TuneletWorkletProcessor requires processorOptions.module for initSync",
      );
    }
    const hostSampleRate = workletGlobal.sampleRate;
    if (hostSampleRate !== 44100 && hostSampleRate !== 48000) {
      throw new Error(
        `Unsupported AudioWorklet sampleRate ${String(hostSampleRate)} Hz; sampleRate must be 44100 or 48000`,
      );
    }
    const maxBlockFrames =
      procOptions.maxBlockFrames ?? DEFAULT_MAX_BLOCK_FRAMES;
    initSync({ module: procOptions.module });
    this.processor = new Tunelet(
      hostSampleRate,
      maxBlockFrames,
      procOptions.config,
    );
    this.maxBlockFrames = this.processor.maxBlockFrames;
    this.monoChunk = new Float32Array(this.maxBlockFrames);
    this.silentChunk = new Float32Array(this.maxBlockFrames);
    if (this.maxBlockFrames > 128) {
      this.monoChunk128 = this.monoChunk.subarray(0, 128);
      this.silentChunk128 = this.silentChunk.subarray(0, 128);
    }
    this.port.onmessage = (event: MessageEvent<TuneletWorkletRequest>) => {
      this.handleMessage(event.data);
    };
  }

  /** Renders one Web Audio block of `Float32Array` channels. */
  process(inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
    if (this.disposed) {
      this.clearOutputs(outputs);
      return false;
    }
    const outputChannels = outputs[0];
    if (!outputChannels || outputChannels.length === 0) {
      return true;
    }
    const out0 = outputChannels[0];
    const frames = out0.length;
    if (frames === 0) {
      return true;
    }
    for (let ch = 2; ch < outputChannels.length; ch++) {
      outputChannels[ch].fill(0);
    }

    const inputChannels = inputs[0];
    const hasConnectedInput =
      inputChannels !== undefined &&
      inputChannels.length > 0 &&
      inputChannels[0].length === frames;

    if (this.draining && this.pendingEvents.length === 0) {
      this.renderDrain(outputChannels, frames);
      return true;
    }

    if (!hasConnectedInput && this.pendingEvents.length === 0) {
      for (let ch = 0; ch < outputChannels.length; ch++) {
        outputChannels[ch].fill(0);
      }
      return true;
    }

    const events = this.takeBlockEvents(frames);
    this.renderProcess(
      hasConnectedInput ? inputChannels : undefined,
      outputChannels,
      frames,
      events,
    );
    return true;
  }

  /** Handles one control, drain or feedback message from `this.port`. */
  private handleMessage(message: TuneletWorkletRequest): void {
    if (typeof message !== "object" || message === null) {
      return;
    }
    const requestId = message.requestId;
    if (this.disposed && message.type !== "dispose") {
      this.postResponse({
        type: "error",
        requestId,
        message: "TuneletWorkletProcessor is disposed",
      });
      return;
    }
    try {
      switch (message.type) {
        case "config":
          this.postResponse({
            type: "config",
            requestId,
            value: this.processor.config(),
          });
          return;
        case "setConfig":
          this.processor.setConfig(message.config);
          this.draining = false;
          this.postResponse({ type: "setConfig", requestId });
          return;
        case "midi": {
          const offset = validateOffset(message.offset ?? 0);
          validateMidiEvent(message.event);
          this.enqueueEvent({
            offset,
            event: { type: "midi", value: message.event },
          });
          this.draining = false;
          this.postResponse({ type: "midi", requestId });
          return;
        }
        case "correctionMidi": {
          const offset = validateOffset(message.offset ?? 0);
          validateMidiEvent(message.event);
          this.enqueueEvent({
            offset,
            event: { type: "correctionMidi", value: message.event },
          });
          this.draining = false;
          this.postResponse({ type: "correctionMidi", requestId });
          return;
        }
        case "events": {
          if (!Array.isArray(message.events)) {
            throw new Error("events must be an array of TimedEvent objects");
          }
          let previous = 0;
          for (const item of message.events) {
            const offset = validateOffset(item?.offset);
            if (offset < previous) {
              throw new Error(
                "event offset is below the previous event's offset",
              );
            }
            if (
              item.event?.type === "midi" ||
              item.event?.type === "correctionMidi"
            ) {
              validateMidiEvent(item.event.value);
            }
            previous = offset;
          }
          for (const item of message.events) {
            this.enqueueEvent(item);
          }
          if (message.events.length > 0) {
            this.draining = false;
          }
          this.postResponse({ type: "events", requestId });
          return;
        }
        case "reset":
          this.pendingEvents.length = 0;
          this.draining = false;
          this.processor.reset();
          this.postResponse({ type: "reset", requestId });
          return;
        case "beginDrain":
        case "drain": {
          this.pendingEvents.length = 0;
          const tailFrames = this.processor.beginDrain();
          this.draining = tailFrames > 0;
          this.postResponse({ type: message.type, requestId, value: tailFrames });
          if (tailFrames === 0) {
            this.postResponse({ type: "drainComplete" });
          }
          return;
        }
        case "tailFrames":
          this.postResponse({
            type: "tailFrames",
            requestId,
            value: this.processor.tailFrames(),
          });
          return;
        case "feedback":
          this.postResponse({
            type: "feedback",
            requestId,
            value: this.processor.feedback(),
          });
          return;
        case "takeFeedback":
          this.postResponse({
            type: "takeFeedback",
            requestId,
            value: this.processor.takeFeedback(),
          });
          return;
        case "telemetry":
          this.postResponse({
            type: "telemetry",
            requestId,
            value: this.processor.telemetry(),
          });
          return;
        case "takeTelemetry":
          this.postResponse({
            type: "takeTelemetry",
            requestId,
            value: this.processor.takeTelemetry(),
          });
          return;
        case "formantEnvelope": {
          if (!isIntegerInRange(message.points, 0, 0x0fff_ffff)) {
            throw new Error("points must be a non-negative integer");
          }
          const input = new Float32Array(message.points);
          const output = new Float32Array(message.points);
          const valid = this.processor.formantEnvelope(
            message.maxFrequencyHz,
            input,
            output,
          );
          this.port.postMessage(
            {
              type: "formantEnvelope",
              requestId,
              value: { valid, input, output },
            } satisfies TuneletWorkletResponse,
            [input.buffer, output.buffer],
          );
          return;
        }
        case "dispose":
          if (!this.disposed) {
            this.pendingEvents.length = 0;
            this.draining = false;
            this.disposed = true;
            this.processor.free();
          }
          this.postResponse({ type: "dispose", requestId });
          return;
      }
    } catch (error: unknown) {
      this.postResponse({
        type: "error",
        requestId,
        message: errorMessage(error),
      });
    }
  }

  /** Inserts one `TimedEvent` in non-decreasing offset order. */
  private enqueueEvent(item: TimedEvent): void {
    const list = this.pendingEvents;
    if (list.length === 0 || list[list.length - 1].offset <= item.offset) {
      list.push(item);
      return;
    }
    let index = list.length;
    while (index > 0 && list[index - 1].offset > item.offset) {
      index--;
    }
    list.splice(index, 0, item);
  }

  /** Takes the queued events for a block of `frames` samples. */
  private takeBlockEvents(frames: number): TimedEvent[] | undefined {
    if (this.pendingEvents.length === 0) {
      return undefined;
    }
    const events = this.pendingEvents;
    this.pendingEvents = [];
    for (const item of events) {
      if (item.offset >= frames) {
        this.postResponse({
          type: "error",
          message: `event offset ${item.offset} is outside block of ${frames} frames`,
        });
        return undefined;
      }
    }
    return events;
  }

  /** Renders active or silent input in chunks of at most `maxBlockFrames`. */
  private renderProcess(
    inputChannels: Float32Array[] | undefined,
    outputChannels: Float32Array[],
    frames: number,
    events: TimedEvent[] | undefined,
  ): void {
    const stereoOut = outputChannels.length >= 2;
    const outLeft = outputChannels[0];
    const outRight = stereoOut ? outputChannels[1] : undefined;
    let eventCursor = 0;

    for (let pos = 0; pos < frames; pos += this.maxBlockFrames) {
      const chunkLen = Math.min(this.maxBlockFrames, frames - pos);
      const monoIn = this.prepareMonoChunk(inputChannels, pos, chunkLen, frames);
      let chunkEvents: TimedEvent[] | undefined;
      if (events !== undefined && eventCursor < events.length) {
        const chunkEnd = pos + chunkLen;
        if (pos === 0 && chunkLen === frames) {
          chunkEvents = events;
          eventCursor = events.length;
        } else if (events[eventCursor].offset < chunkEnd) {
          chunkEvents = [];
          while (
            eventCursor < events.length &&
            events[eventCursor].offset < chunkEnd
          ) {
            const current = events[eventCursor];
            chunkEvents.push({
              offset: current.offset - pos,
              event: current.event,
            });
            eventCursor++;
          }
        }
      }

      if (stereoOut && outRight !== undefined) {
        const leftSlice =
          pos === 0 && chunkLen === frames
            ? outLeft
            : outLeft.subarray(pos, pos + chunkLen);
        const rightSlice =
          pos === 0 && chunkLen === frames
            ? outRight
            : outRight.subarray(pos, pos + chunkLen);
        this.runProcessStereo(monoIn, leftSlice, rightSlice, chunkEvents);
      } else {
        const monoSlice =
          pos === 0 && chunkLen === frames
            ? outLeft
            : outLeft.subarray(pos, pos + chunkLen);
        monoSlice.set(monoIn);
        this.runProcessMono(monoSlice, chunkEvents);
      }
    }
  }

  /** Calls `processF32` and isolates event payload errors from the audio graph. */
  private runProcessStereo(
    input: Float32Array,
    left: Float32Array,
    right: Float32Array,
    events: TimedEvent[] | undefined,
  ): void {
    if (events === undefined) {
      this.processor.processF32(input, left, right);
      return;
    }
    try {
      this.processor.processF32(input, left, right, events);
    } catch (error: unknown) {
      this.postResponse({ type: "error", message: errorMessage(error) });
      this.processor.processF32(input, left, right);
    }
  }

  /** Calls `processMonoF32` and isolates event payload errors from the audio graph. */
  private runProcessMono(
    audio: Float32Array,
    events: TimedEvent[] | undefined,
  ): void {
    if (events === undefined) {
      this.processor.processMonoF32(audio);
      return;
    }
    try {
      this.processor.processMonoF32(audio, events);
    } catch (error: unknown) {
      this.postResponse({ type: "error", message: errorMessage(error) });
      this.processor.processMonoF32(audio);
    }
  }

  /** Renders tail blocks across chunks until the drain budget completes. */
  private renderDrain(outputChannels: Float32Array[], frames: number): void {
    const stereoOut = outputChannels.length >= 2;
    const outLeft = outputChannels[0];
    const outRight = stereoOut ? outputChannels[1] : undefined;

    for (let pos = 0; pos < frames; pos += this.maxBlockFrames) {
      const chunkLen = Math.min(this.maxBlockFrames, frames - pos);
      const leftSlice =
        pos === 0 && chunkLen === frames
          ? outLeft
          : outLeft.subarray(pos, pos + chunkLen);

      if (!this.draining) {
        leftSlice.fill(0);
        if (stereoOut && outRight !== undefined) {
          const rightSlice =
            pos === 0 && chunkLen === frames
              ? outRight
              : outRight.subarray(pos, pos + chunkLen);
          rightSlice.fill(0);
        }
        continue;
      }

      let rendered: number;
      if (stereoOut && outRight !== undefined) {
        const rightSlice =
          pos === 0 && chunkLen === frames
            ? outRight
            : outRight.subarray(pos, pos + chunkLen);
        rendered = this.processor.drainF32(leftSlice, rightSlice);
      } else {
        rendered = this.processor.drainMonoF32(leftSlice);
      }

      if (rendered < chunkLen) {
        this.draining = false;
        this.postResponse({ type: "drainComplete" });
      }
    }
  }

  /** Returns a mono `Float32Array` view for one chunk without heap allocation. */
  private prepareMonoChunk(
    inputChannels: Float32Array[] | undefined,
    pos: number,
    chunkLen: number,
    totalFrames: number,
  ): Float32Array {
    if (inputChannels === undefined || inputChannels.length === 0) {
      if (chunkLen === this.maxBlockFrames) {
        return this.silentChunk;
      }
      if (chunkLen === 128 && this.silentChunk128 !== undefined) {
        return this.silentChunk128;
      }
      return this.silentChunk.subarray(0, chunkLen);
    }

    if (inputChannels.length === 1) {
      const ch0 = inputChannels[0];
      return pos === 0 && chunkLen === totalFrames
        ? ch0
        : ch0.subarray(pos, pos + chunkLen);
    }

    const target =
      chunkLen === this.maxBlockFrames
        ? this.monoChunk
        : chunkLen === 128 && this.monoChunk128 !== undefined
          ? this.monoChunk128
          : this.monoChunk.subarray(0, chunkLen);

    if (inputChannels.length === 2) {
      const left = inputChannels[0];
      const right = inputChannels[1];
      for (let i = 0; i < chunkLen; i++) {
        target[i] = (left[pos + i] + right[pos + i]) * 0.5;
      }
      return target;
    }

    const scale = 1 / inputChannels.length;
    for (let i = 0; i < chunkLen; i++) {
      let sum = 0;
      for (let ch = 0; ch < inputChannels.length; ch++) {
        sum += inputChannels[ch][pos + i];
      }
      target[i] = sum * scale;
    }
    return target;
  }

  /** Fills all output channels with zeros. */
  private clearOutputs(outputs: Float32Array[][]): void {
    for (let i = 0; i < outputs.length; i++) {
      const channels = outputs[i];
      for (let ch = 0; ch < channels.length; ch++) {
        channels[ch].fill(0);
      }
    }
  }

  /** Sends a typed response message on `this.port`. */
  private postResponse(response: TuneletWorkletResponse): void {
    this.port.postMessage(response);
  }
}

if (typeof workletGlobal.registerProcessor === "function") {
  workletGlobal.registerProcessor(
    TUNELET_WORKLET_PROCESSOR_NAME,
    TuneletWorkletProcessor,
  );
}
