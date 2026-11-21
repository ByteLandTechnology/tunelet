/**
 * The `AudioWorkletProcessor` entry point for `tunelet/worklet-processor`.
 *
 * This module initializes the Wasm binary synchronously with `initSync` from
 * `processorOptions.wasm`. It does not call `fetch`. It preallocates
 * `Float32Array` scratch arrays at construction and chunks Web Audio render
 * blocks into `TuneletFloat32`.
 */
import "./text-codec-shim.js";
import { initSync, TuneletFloat32 } from "./index.js";
import type {
  BlockEvents,
  ControlEvent,
  MidiEvent,
  TimedEvent,
} from "./index.js";
import {
  DEFAULT_WORKLET_BLOCK_FRAMES,
  TUNELET_PROCESSOR_NAME,
} from "./worklet-protocol.js";
import type {
  FormantEnvelopeResult,
  TuneletProcessorOptions,
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

/** Minimal `AudioWorkletProcessor` instance contract in `AudioWorkletGlobalScope`. */
export interface AudioWorkletProcessorInstance {
  readonly port: MessagePort;
  process(
    inputs: Float32Array[][],
    outputs: Float32Array[][],
    parameters: Record<string, Float32Array>,
  ): boolean;
}

/** Constructor type for `AudioWorkletProcessor` in `AudioWorkletGlobalScope`. */
export interface AudioWorkletProcessorConstructor {
  new (options?: AudioWorkletNodeOptions): AudioWorkletProcessorInstance;
  prototype: AudioWorkletProcessorInstance;
}

type RegisterProcessorFn = (
  name: string,
  processorCtor: AudioWorkletProcessorConstructor,
) => void;

const EMPTY_FLOAT32 = new Float32Array(0);

/** Resolves `AudioWorkletProcessor` in browsers and in test environments. */
const WorkletProcessorBase: AudioWorkletProcessorConstructor =
  typeof (globalThis as Record<string, unknown>).AudioWorkletProcessor ===
  "function"
    ? ((globalThis as Record<string, unknown>)
        .AudioWorkletProcessor as AudioWorkletProcessorConstructor)
    : (class {
        declare readonly port: MessagePort;
        constructor(options?: AudioWorkletNodeOptions) {
          const runtimeCtor = (globalThis as Record<string, unknown>)
            .AudioWorkletProcessor;
          if (typeof runtimeCtor === "function") {
            return Reflect.construct(runtimeCtor, [options], new.target);
          }
          throw new Error(
            "AudioWorkletProcessor is not available in this environment",
          );
        }
        process(): boolean {
          return false;
        }
      } as unknown as AudioWorkletProcessorConstructor);

/** Formats a thrown value as an error message string. */
function formatError(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  return String(error);
}

/** Checks that a number is an integer in `min..=max`. */
function isIntegerInRange(value: unknown, min: number, max: number): boolean {
  return (
    typeof value === "number" &&
    Number.isInteger(value) &&
    value >= min &&
    value <= max
  );
}

/** Validates a `MidiEvent` before the processor queues it for the next block. */
function validateMidiEvent(event: MidiEvent): void {
  if (typeof event !== "object" || event === null) {
    throw new Error("MIDI message is not valid");
  }
  if (event.type === "noteOn") {
    if (
      !isIntegerInRange(event.channel, 0, 15) ||
      !isIntegerInRange(event.note, 0, 127) ||
      !isIntegerInRange(event.velocity, 0, 127)
    ) {
      throw new Error("MIDI message is not valid");
    }
    return;
  }
  if (event.type === "noteOff") {
    if (
      !isIntegerInRange(event.channel, 0, 15) ||
      !isIntegerInRange(event.note, 0, 127)
    ) {
      throw new Error("MIDI message is not valid");
    }
    return;
  }
  if (event.type === "allNotesOff") {
    return;
  }
  throw new Error("MIDI message is not valid");
}

/** Validates one `ControlEvent` payload before it enters the event queue. */
function validateControlEventShape(
  event: ControlEvent,
  processor: TuneletFloat32,
): void {
  if (typeof event !== "object" || event === null || !("type" in event)) {
    throw new Error("control event is not valid");
  }
  switch (event.type) {
    case "midi":
    case "correctionMidi":
      validateMidiEvent(event.value);
      return;
    case "reset":
      return;
    case "outputGain":
      if (typeof event.value !== "number" || !Number.isFinite(event.value)) {
        throw new Error("invalid config: output_gain must be finite");
      }
      if (event.value < 0 || event.value > 64) {
        throw new Error("invalid config: output_gain must be in 0..=64");
      }
      return;
    case "configure":
    case "punch":
    case "warm":
    case "resynthesis":
    case "alien":
    case "reverb": {
      if (typeof event.value !== "object" || event.value === null) {
        throw new Error("control event configuration must be an object");
      }
      // For timed offsets above zero, keep the processor state untouched here.
      void processor;
      return;
    }
    default:
      throw new Error("control event is not valid");
  }
}

/**
 * Web Audio `AudioWorkletProcessor` that wraps `TuneletFloat32`.
 *
 * The constructor calls `initSync({ module })` with the Wasm binary from
 * `options.processorOptions.wasm`. It preallocates `Float32Array` scratch
 * buffers for `maxBlockFrames` and processes Web Audio blocks in chunks.
 *
 * Steady-state audio calls allocate no new JavaScript typed arrays. The
 * generated wasm-bindgen glue can still create temporary views on Wasm memory,
 * and `MessagePort` control, event, feedback, telemetry and envelope messages
 * allocate JavaScript objects.
 */
export class TuneletWorkletProcessor extends WorkletProcessorBase {
  #processor!: TuneletFloat32;
  #maxBlockFrames = DEFAULT_WORKLET_BLOCK_FRAMES;
  #scratchInput: Float32Array = EMPTY_FLOAT32;
  #scratchLeft: Float32Array = EMPTY_FLOAT32;
  #scratchRight: Float32Array = EMPTY_FLOAT32;
  #partialFrames = 0;
  #partialInput: Float32Array = EMPTY_FLOAT32;
  #partialLeft: Float32Array = EMPTY_FLOAT32;
  #partialRight: Float32Array = EMPTY_FLOAT32;
  #pendingEvents: TimedEvent[] = [];
  #draining = false;
  #drainRemaining = 0;
  #stopAfterDrain = false;
  #disposed = false;

  constructor(options?: AudioWorkletNodeOptions) {
    super(options);
    try {
      const processorOptions = (options?.processorOptions ??
        {}) as Partial<TuneletProcessorOptions>;
      if (processorOptions.wasm === undefined || processorOptions.wasm === null) {
        throw new Error(
          "processorOptions.wasm must provide a WebAssembly.Module or byte buffer for initSync",
        );
      }

      const maxBlockFrames =
        processorOptions.maxBlockFrames ?? DEFAULT_WORKLET_BLOCK_FRAMES;
      const scopeSampleRate = (globalThis as Record<string, unknown>)
        .sampleRate;
      if (typeof scopeSampleRate !== "number") {
        throw new Error(
          "AudioWorkletGlobalScope sampleRate is not available",
        );
      }

      initSync({ module: processorOptions.wasm });
      const processor = new TuneletFloat32(
        scopeSampleRate,
        maxBlockFrames,
        processorOptions.config,
      );

      try {
        this.#scratchInput = new Float32Array(maxBlockFrames);
        this.#scratchLeft = new Float32Array(maxBlockFrames);
        this.#scratchRight = new Float32Array(maxBlockFrames);
      } catch (error) {
        processor.free();
        throw error;
      }

      this.#processor = processor;
      this.#maxBlockFrames = maxBlockFrames;
      this.port.onmessage = (event: MessageEvent<TuneletWorkletRequest>) => {
        this.#handleMessage(event.data);
      };

      this.#postNotification({
        __tunelet: "ready",
        sampleRate: processor.sampleRate,
        maxBlockFrames: processor.maxBlockFrames,
        latencyFrames: processor.latencyFrames,
      });
    } catch (error) {
      this.#disposed = true;
      this.#postNotification({
        __tunelet: "initError",
        error: formatError(error),
      });
      throw error;
    }
  }

  /** Processes one Web Audio render quantum in chunks with scratch arrays. */
  override process(
    inputs: Float32Array[][],
    outputs: Float32Array[][],
    _parameters: Record<string, Float32Array>,
  ): boolean {
    if (this.#disposed) {
      return false;
    }

    const outputChannels = outputs[0];
    if (!outputChannels || outputChannels.length === 0) {
      return true;
    }
    const firstOutput = outputChannels[0];
    if (!firstOutput || firstOutput.length === 0) {
      return true;
    }

    const blockFrames = firstOutput.length;
    const inputChannels = inputs[0];
    const monoOutput = outputChannels.length === 1;
    const rightOutput = monoOutput ? undefined : outputChannels[1];

    // Zero any extra channels beyond stereo.
    for (let ch = 2; ch < outputChannels.length; ch += 1) {
      outputChannels[ch].fill(0);
    }

    let drainFinishedInBlock = false;
    let offset = 0;

    while (offset < blockFrames) {
      const chunkFrames = Math.min(
        this.#maxBlockFrames,
        blockFrames - offset,
      );

      if (drainFinishedInBlock) {
        firstOutput.fill(0, offset, offset + chunkFrames);
        if (rightOutput) {
          rightOutput.fill(0, offset, offset + chunkFrames);
        }
        offset += chunkFrames;
        continue;
      }

      const [chunkInput, chunkLeft, chunkRight] =
        this.#scratchViews(chunkFrames);

      if (this.#draining) {
        if (monoOutput) {
          const rendered = this.#processor.drainMono(chunkLeft);
          this.#copyToOutput(chunkLeft, firstOutput, offset, chunkFrames);
          this.#drainRemaining -= rendered;
          if (rendered < chunkFrames || this.#drainRemaining <= 0) {
            this.#finishDrain();
            drainFinishedInBlock = true;
          }
        } else {
          const rendered = this.#processor.drain(chunkLeft, chunkRight);
          this.#copyToOutput(chunkLeft, firstOutput, offset, chunkFrames);
          if (rightOutput) {
            this.#copyToOutput(chunkRight, rightOutput, offset, chunkFrames);
          }
          this.#drainRemaining -= rendered;
          if (rendered < chunkFrames || this.#drainRemaining <= 0) {
            this.#finishDrain();
            drainFinishedInBlock = true;
          }
        }
        offset += chunkFrames;
        continue;
      }

      this.#fillInputChunk(inputChannels, chunkInput, offset, chunkFrames);
      const chunkEvents = this.#takeEventsForChunk(offset, chunkFrames);

      try {
        if (monoOutput) {
          this.#processor.processMono(chunkInput, chunkEvents);
          this.#copyToOutput(chunkInput, firstOutput, offset, chunkFrames);
        } else {
          this.#processor.process(
            chunkInput,
            chunkLeft,
            chunkRight,
            chunkEvents,
          );
          this.#copyToOutput(chunkLeft, firstOutput, offset, chunkFrames);
          if (rightOutput) {
            this.#copyToOutput(chunkRight, rightOutput, offset, chunkFrames);
          }
        }
      } catch (error) {
        this.#postNotification({
          __tunelet: "processError",
          error: formatError(error),
        });
        if (chunkEvents !== undefined) {
          if (monoOutput) {
            this.#fillInputChunk(inputChannels, chunkInput, offset, chunkFrames);
            this.#processor.processMono(chunkInput);
            this.#copyToOutput(chunkInput, firstOutput, offset, chunkFrames);
          } else {
            this.#processor.process(chunkInput, chunkLeft, chunkRight);
            this.#copyToOutput(chunkLeft, firstOutput, offset, chunkFrames);
            if (rightOutput) {
              this.#copyToOutput(chunkRight, rightOutput, offset, chunkFrames);
            }
          }
        } else {
          firstOutput.fill(0, offset, offset + chunkFrames);
          if (rightOutput) {
            rightOutput.fill(0, offset, offset + chunkFrames);
          }
        }
      }

      offset += chunkFrames;
    }

    if (this.#pendingEvents.length > 0) {
      for (let index = 0; index < this.#pendingEvents.length; index += 1) {
        const current = this.#pendingEvents[index];
        current.offset = Math.max(0, current.offset - blockFrames);
      }
    }

    if (drainFinishedInBlock && this.#stopAfterDrain) {
      this.#stopAfterDrain = false;
      return false;
    }
    return !this.#disposed;
  }

  /** Copies or downmixes `inputs[0]` into the mono `chunkInput` buffer. */
  #fillInputChunk(
    inputChannels: Float32Array[] | undefined,
    chunkInput: Float32Array,
    offset: number,
    chunkFrames: number,
  ): void {
    if (!inputChannels || inputChannels.length === 0) {
      chunkInput.fill(0);
      return;
    }
    const channelCount = inputChannels.length;
    if (channelCount === 1) {
      const channel = inputChannels[0];
      if (!channel || channel.length === 0) {
        chunkInput.fill(0);
        return;
      }
      if (channel.length === 1) {
        chunkInput.fill(channel[0]);
        return;
      }
      for (let i = 0; i < chunkFrames; i += 1) {
        const srcIndex = offset + i;
        chunkInput[i] = srcIndex < channel.length ? channel[srcIndex] : 0;
      }
      return;
    }

    const invChannels = 1.0 / channelCount;
    for (let i = 0; i < chunkFrames; i += 1) {
      const srcIndex = offset + i;
      let sum = 0;
      for (let ch = 0; ch < channelCount; ch += 1) {
        const channel = inputChannels[ch];
        if (channel && channel.length > 0) {
          sum +=
            channel.length === 1
              ? channel[0]
              : srcIndex < channel.length
                ? channel[srcIndex]
                : 0;
        }
      }
      chunkInput[i] = sum * invChannels;
    }
  }

  /** Copies `chunkFrames` samples from a scratch buffer into an output slice. */
  #copyToOutput(
    source: Float32Array,
    destination: Float32Array,
    destinationOffset: number,
    chunkFrames: number,
  ): void {
    for (let i = 0; i < chunkFrames; i += 1) {
      destination[destinationOffset + i] = source[i];
    }
  }

  /** Returns reusable `Float32Array` scratch views of length `chunkFrames`. */
  #scratchViews(
    chunkFrames: number,
  ): [Float32Array, Float32Array, Float32Array] {
    if (chunkFrames === this.#maxBlockFrames) {
      return [this.#scratchInput, this.#scratchLeft, this.#scratchRight];
    }
    if (this.#partialFrames !== chunkFrames) {
      this.#partialInput = this.#scratchInput.subarray(0, chunkFrames);
      this.#partialLeft = this.#scratchLeft.subarray(0, chunkFrames);
      this.#partialRight = this.#scratchRight.subarray(0, chunkFrames);
      this.#partialFrames = chunkFrames;
    }
    return [this.#partialInput, this.#partialLeft, this.#partialRight];
  }

  /** Extracts queued events that fall inside `[chunkStart, chunkStart + chunkFrames)`. */
  #takeEventsForChunk(
    chunkStart: number,
    chunkFrames: number,
  ): BlockEvents | undefined {
    if (this.#pendingEvents.length === 0) {
      return undefined;
    }
    const chunkEnd = chunkStart + chunkFrames;
    let count = 0;
    while (
      count < this.#pendingEvents.length &&
      this.#pendingEvents[count].offset < chunkEnd
    ) {
      count += 1;
    }
    if (count === 0) {
      return undefined;
    }
    const selected = this.#pendingEvents.splice(0, count);
    for (let index = 0; index < selected.length; index += 1) {
      const localOffset = selected[index].offset - chunkStart;
      selected[index] = {
        offset: localOffset < 0 ? 0 : localOffset,
        event: selected[index].event,
      };
    }
    return selected;
  }

  /** Inserts timed events and keeps offsets in frame order. */
  #enqueueTimedEvents(events: BlockEvents): void {
    if (!Array.isArray(events)) {
      throw new Error("events must be an array");
    }
    let previous = 0;
    for (let index = 0; index < events.length; index += 1) {
      const entry = events[index];
      if (
        typeof entry !== "object" ||
        entry === null ||
        typeof entry.offset !== "number" ||
        !Number.isInteger(entry.offset) ||
        entry.offset < 0
      ) {
        throw new Error(`event ${index}: offset is outside the block`);
      }
      if (entry.offset < previous) {
        throw new Error(
          `event ${index}: offset is below the previous event's offset`,
        );
      }
      validateControlEventShape(entry.event, this.#processor);
      previous = entry.offset;
    }

    this.#draining = false;
    this.#drainRemaining = 0;
    this.#stopAfterDrain = false;

    for (let index = 0; index < events.length; index += 1) {
      const item: TimedEvent = {
        offset: events[index].offset,
        event: events[index].event,
      };
      if (
        this.#pendingEvents.length === 0 ||
        this.#pendingEvents[this.#pendingEvents.length - 1].offset <=
          item.offset
      ) {
        this.#pendingEvents.push(item);
      } else {
        const insertIndex = this.#pendingEvents.findIndex(
          (existing) => existing.offset > item.offset,
        );
        this.#pendingEvents.splice(insertIndex, 0, item);
      }
    }
  }

  /** Completes an active drain and notifies the main thread. */
  #finishDrain(): void {
    this.#draining = false;
    this.#drainRemaining = 0;
    this.#postNotification({ __tunelet: "drainComplete" });
  }

  /** Handles one command message from `TuneletWorkletNode` over `MessagePort`. */
  #handleMessage(request: TuneletWorkletRequest): void {
    if (
      typeof request !== "object" ||
      request === null ||
      typeof request.type !== "string"
    ) {
      return;
    }

    const requestId = request.id;
    const requestType = request.type;

    if (this.#disposed && requestType !== "free") {
      this.#postResponse({
        __tunelet: "response",
        id: requestId,
        requestType,
        ok: false,
        error: "TuneletWorkletProcessor is already disposed",
      });
      return;
    }

    try {
      switch (request.type) {
        case "config": {
          const value = this.#processor.config();
          this.#postResponse({
            __tunelet: "response",
            id: requestId,
            requestType,
            ok: true,
            value,
          });
          return;
        }
        case "setConfig": {
          this.#processor.setConfig(request.config);
          this.#draining = false;
          this.#stopAfterDrain = false;
          this.#postResponse({
            __tunelet: "response",
            id: requestId,
            requestType,
            ok: true,
          });
          return;
        }
        case "midi": {
          validateMidiEvent(request.event);
          this.#draining = false;
          this.#stopAfterDrain = false;
          this.#enqueueZeroOffsetEvent({ type: "midi", value: request.event });
          this.#postResponse({
            __tunelet: "response",
            id: requestId,
            requestType,
            ok: true,
          });
          return;
        }
        case "correctionMidi": {
          validateMidiEvent(request.event);
          this.#draining = false;
          this.#stopAfterDrain = false;
          this.#enqueueZeroOffsetEvent({
            type: "correctionMidi",
            value: request.event,
          });
          this.#postResponse({
            __tunelet: "response",
            id: requestId,
            requestType,
            ok: true,
          });
          return;
        }
        case "control": {
          this.#applyOrQueueControl(request.event);
          this.#postResponse({
            __tunelet: "response",
            id: requestId,
            requestType,
            ok: true,
          });
          return;
        }
        case "events": {
          this.#enqueueTimedEvents(request.events);
          this.#postResponse({
            __tunelet: "response",
            id: requestId,
            requestType,
            ok: true,
          });
          return;
        }
        case "reset": {
          this.#pendingEvents.length = 0;
          this.#draining = false;
          this.#stopAfterDrain = false;
          this.#processor.reset();
          this.#postResponse({
            __tunelet: "response",
            id: requestId,
            requestType,
            ok: true,
          });
          return;
        }
        case "beginDrain": {
          this.#pendingEvents.length = 0;
          const budget = this.#processor.beginDrain();
          this.#drainRemaining = budget;
          this.#draining = budget > 0;
          this.#stopAfterDrain = Boolean(request.stopAfterDrain);
          this.#postResponse({
            __tunelet: "response",
            id: requestId,
            requestType,
            ok: true,
            value: budget,
          });
          if (budget === 0) {
            this.#postNotification({ __tunelet: "drainComplete" });
          }
          return;
        }
        case "tailFrames": {
          const value = this.#processor.tailFrames();
          this.#postResponse({
            __tunelet: "response",
            id: requestId,
            requestType,
            ok: true,
            value,
          });
          return;
        }
        case "feedback": {
          const value = this.#processor.feedback();
          this.#postResponse({
            __tunelet: "response",
            id: requestId,
            requestType,
            ok: true,
            value,
          });
          return;
        }
        case "takeFeedback": {
          const value = this.#processor.takeFeedback();
          this.#postResponse({
            __tunelet: "response",
            id: requestId,
            requestType,
            ok: true,
            value,
          });
          return;
        }
        case "telemetry": {
          const value = this.#processor.telemetry();
          this.#postResponse({
            __tunelet: "response",
            id: requestId,
            requestType,
            ok: true,
            value,
          });
          return;
        }
        case "takeTelemetry": {
          const value = this.#processor.takeTelemetry();
          this.#postResponse({
            __tunelet: "response",
            id: requestId,
            requestType,
            ok: true,
            value,
          });
          return;
        }
        case "formantEnvelope": {
          if (
            typeof request.points !== "number" ||
            !Number.isInteger(request.points) ||
            request.points < 0
          ) {
            throw new Error("points must be a non-negative integer");
          }
          const input = new Float32Array(request.points);
          const output = new Float32Array(request.points);
          const valid = this.#processor.formantEnvelope(
            request.maxFrequencyHz,
            input,
            output,
          );
          const value: FormantEnvelopeResult = { valid, input, output };
          this.#postResponse(
            {
              __tunelet: "response",
              id: requestId,
              requestType,
              ok: true,
              value,
            },
            [input.buffer, output.buffer],
          );
          return;
        }
        case "free": {
          if (!this.#disposed) {
            this.#pendingEvents.length = 0;
            this.#draining = false;
            this.#stopAfterDrain = false;
            this.#disposed = true;
            this.#processor.free();
          }
          this.#postResponse({
            __tunelet: "response",
            id: requestId,
            requestType,
            ok: true,
          });
          return;
        }
      }
    } catch (error) {
      this.#postResponse({
        __tunelet: "response",
        id: requestId,
        requestType,
        ok: false,
        error: formatError(error),
      });
    }
  }

  /** Applies immediate stage controls with full Rust validation, or queues MIDI. */
  #applyOrQueueControl(event: ControlEvent): void {
    validateControlEventShape(event, this.#processor);
    switch (event.type) {
      case "configure":
      case "punch":
      case "warm":
      case "resynthesis":
      case "alien":
      case "reverb":
      case "outputGain": {
        if (this.#pendingEvents.length > 0) {
          this.#draining = false;
          this.#stopAfterDrain = false;
          this.#enqueueZeroOffsetEvent(event);
          return;
        }
        const nextConfig =
          event.type === "configure"
            ? event.value
            : this.#processor.config();
        if (event.type === "punch") {
          nextConfig.punch = event.value;
        } else if (event.type === "warm") {
          nextConfig.warm = event.value;
        } else if (event.type === "resynthesis") {
          nextConfig.resynthesis = event.value;
        } else if (event.type === "alien") {
          nextConfig.alien = event.value;
        } else if (event.type === "reverb") {
          nextConfig.reverb = event.value;
        } else if (event.type === "outputGain") {
          nextConfig.outputGain = event.value;
        }
        this.#processor.setConfig(nextConfig);
        this.#draining = false;
        this.#stopAfterDrain = false;
        return;
      }
      case "reset":
        this.#pendingEvents.length = 0;
        this.#draining = false;
        this.#stopAfterDrain = false;
        this.#processor.reset();
        return;
      case "midi":
      case "correctionMidi":
        this.#draining = false;
        this.#stopAfterDrain = false;
        this.#enqueueZeroOffsetEvent(event);
        return;
    }
  }

  /** Inserts an event at frame offset 0 after earlier offset-0 events. */
  #enqueueZeroOffsetEvent(event: ControlEvent): void {
    let insertIndex = 0;
    while (
      insertIndex < this.#pendingEvents.length &&
      this.#pendingEvents[insertIndex].offset === 0
    ) {
      insertIndex += 1;
    }
    this.#pendingEvents.splice(insertIndex, 0, { offset: 0, event });
  }

  /** Posts a command response over `MessagePort`. */
  #postResponse(
    response: TuneletWorkletResponse,
    transfer?: Transferable[],
  ): void {
    const message: TuneletWorkletOutboundMessage = response;
    if (transfer && transfer.length > 0) {
      this.port.postMessage(message, transfer);
    } else {
      this.port.postMessage(message);
    }
  }

  /** Posts a lifecycle notification over `MessagePort`. */
  #postNotification(notification: TuneletWorkletNotification): void {
    const message: TuneletWorkletOutboundMessage = notification;
    this.port.postMessage(message);
  }
}

/**
 * Registers `TuneletWorkletProcessor` in `AudioWorkletGlobalScope`.
 *
 * When this module loads inside an `AudioWorkletGlobalScope`, it calls this
 * function automatically with `TUNELET_PROCESSOR_NAME`.
 */
export function registerTuneletProcessor(
  name = TUNELET_PROCESSOR_NAME,
): void {
  const register = (globalThis as Record<string, unknown>)
    .registerProcessor as RegisterProcessorFn | undefined;
  if (typeof register !== "function") {
    throw new Error(
      "registerProcessor is only available in AudioWorkletGlobalScope",
    );
  }
  register(name, TuneletWorkletProcessor);
}

if (
  typeof (globalThis as Record<string, unknown>).registerProcessor ===
  "function"
) {
  registerTuneletProcessor(TUNELET_PROCESSOR_NAME);
}
