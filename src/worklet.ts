/**
 * The `AudioWorklet` entry point for `tunelet/worklet`.
 *
 * This module installs the UTF-8 shim first, imports the generated Wasm
 * bindings, defines `TuneletWorkletProcessor` and registers it in
 * `AudioWorkletGlobalScope`.
 */
import "./utf8-shim.js";
import { initSync, Tunelet } from "../pkg/tunelet.js";
import type {
  Feedback,
  ProcessorConfig,
  Telemetry,
  TimedEvent,
} from "../pkg/tunelet.js";
import {
  DEFAULT_MAX_BLOCK_FRAMES,
  DEFAULT_REPORT_INTERVAL_MS,
  TUNELET_WORKLET_PROCESSOR_NAME,
} from "./worklet-protocol.js";
import type {
  TuneletProcessorOptions,
  TuneletReportingOptions,
  TuneletWorkletCommand,
  TuneletWorkletMessage,
} from "./worklet-protocol.js";

export { TUNELET_WORKLET_PROCESSOR_NAME } from "./worklet-protocol.js";
export type {
  TuneletProcessorOptions,
  TuneletReportingOptions,
  TuneletWorkletCommand,
  TuneletWorkletMessage,
} from "./worklet-protocol.js";

const INV_PCM_SCALE = 1 / 32768;
const INTERNAL_FALLBACK_RATE = 48000;
const RESAMPLE_CUSHION_FRAMES = 3;
const ANTI_ALIAS_CUTOFF_HZ = 20000;

interface WorkletProcessorInstance {
  readonly port: MessagePort;
}

interface WorkletProcessorConstructor {
  new (options?: AudioWorkletNodeOptions): WorkletProcessorInstance;
}

const WorkletProcessorBase: WorkletProcessorConstructor =
  typeof (globalThis as { AudioWorkletProcessor?: unknown })
    .AudioWorkletProcessor === "function"
    ? (
        globalThis as unknown as {
          AudioWorkletProcessor: WorkletProcessorConstructor;
        }
      ).AudioWorkletProcessor
    : class FallbackWorkletProcessor implements WorkletProcessorInstance {
        readonly port: MessagePort;

        constructor() {
          const channel = new MessageChannel();
          (channel.port1 as { unref?: () => void }).unref?.();
          (channel.port2 as { unref?: () => void }).unref?.();
          this.port = channel.port1;
        }
      };

/**
 * Wraps a Wasm `Tunelet` processor for Web Audio `Float32Array` blocks.
 *
 * The constructor allocates all `Int16Array` buffers, length views and rate
 * conversion arrays one time. Contexts at 44.1 kHz or 48 kHz run at their
 * native rate. Contexts at other rates use an internal 48 kHz `Tunelet`
 * instance with a stateful block resampler.
 */
export class TuneletWorkletProcessor extends WorkletProcessorBase {
  /** The sample rate of the host `AudioContext` in hertz. */
  readonly hostSampleRate: number;
  /** The internal sample rate of the `Tunelet` instance (44100 or 48000). */
  readonly sampleRate: number;
  /** The maximum host frame count for one internal chunk. */
  readonly maxBlockFrames: number;

  private readonly engine: Tunelet;
  private readonly needsResample: boolean;
  private readonly stepDown: number;
  private readonly stepUp: number;

  private readonly pcmInput: Int16Array;
  private readonly pcmLeft: Int16Array;
  private readonly pcmRight: Int16Array;
  private readonly pcmInputViews: readonly Int16Array[];
  private readonly pcmLeftViews: readonly Int16Array[];
  private readonly pcmRightViews: readonly Int16Array[];

  private readonly monoScratch: Float32Array;
  private readonly inputHistory: Float32Array;
  private readonly outQueueL: Float32Array;
  private readonly outQueueR: Float32Array;

  private readonly useAntiAlias: boolean;
  private readonly aaB0: number;
  private readonly aaB1: number;
  private readonly aaB2: number;
  private readonly aaA1: number;
  private readonly aaA2: number;
  private aaZ1 = 0;
  private aaZ2 = 0;

  private inputCursor = 0;
  private outputCursor = 0;
  private queueLength = 0;

  private pendingEvents: TimedEvent[] = [];
  private reportIntervalFrames = 0;
  private framesSinceReport = 0;
  private feedbackEnabled = true;
  private telemetryEnabled = true;
  private draining = false;
  private disposed = false;

  constructor(options?: AudioWorkletNodeOptions) {
    super(options);

    const processorOptions = (options?.processorOptions ??
      {}) as TuneletProcessorOptions;

    if (processorOptions.module !== undefined) {
      initSync({ module: processorOptions.module });
    }

    const resolvedHostRate =
      processorOptions.sampleRate ?? readGlobalSampleRate();
    if (
      typeof resolvedHostRate !== "number" ||
      !Number.isFinite(resolvedHostRate) ||
      resolvedHostRate <= 0
    ) {
      throw new RangeError("sampleRate must be a positive finite number");
    }

    const maxHostFrames =
      processorOptions.maxBlockFrames ?? DEFAULT_MAX_BLOCK_FRAMES;
    if (
      typeof maxHostFrames !== "number" ||
      !Number.isInteger(maxHostFrames) ||
      maxHostFrames < 1 ||
      maxHostFrames > 0xffffffff
    ) {
      throw new RangeError(
        "maxBlockFrames must be an integer in 1..=4294967295",
      );
    }

    this.hostSampleRate = resolvedHostRate;
    this.sampleRate =
      resolvedHostRate === 44100 || resolvedHostRate === 48000
        ? resolvedHostRate
        : INTERNAL_FALLBACK_RATE;
    this.maxBlockFrames = maxHostFrames;
    this.needsResample = this.hostSampleRate !== this.sampleRate;
    this.stepDown = this.hostSampleRate / this.sampleRate;
    this.stepUp = this.sampleRate / this.hostSampleRate;

    const maxResampledFrames = this.needsResample
      ? Math.ceil((maxHostFrames * this.sampleRate) / this.hostSampleRate) + 2
      : maxHostFrames;
    const enginePreparedFrames = Math.max(maxHostFrames, maxResampledFrames);

    this.engine = new Tunelet(
      this.sampleRate,
      enginePreparedFrames,
      processorOptions.config,
    );

    this.pcmInput = new Int16Array(enginePreparedFrames);
    this.pcmLeft = new Int16Array(enginePreparedFrames);
    this.pcmRight = new Int16Array(enginePreparedFrames);
    this.pcmInputViews = createLengthViews(
      this.pcmInput,
      enginePreparedFrames,
    );
    this.pcmLeftViews = createLengthViews(this.pcmLeft, enginePreparedFrames);
    this.pcmRightViews = createLengthViews(
      this.pcmRight,
      enginePreparedFrames,
    );
    this.monoScratch = new Float32Array(maxHostFrames);

    if (this.needsResample) {
      this.inputHistory = new Float32Array(maxHostFrames + 1);
      const queueCapacity = enginePreparedFrames + 16;
      this.outQueueL = new Float32Array(queueCapacity);
      this.outQueueR = new Float32Array(queueCapacity);
      this.queueLength = RESAMPLE_CUSHION_FRAMES;
    } else {
      this.inputHistory = new Float32Array(0);
      this.outQueueL = new Float32Array(0);
      this.outQueueR = new Float32Array(0);
    }

    if (this.hostSampleRate > this.sampleRate) {
      this.useAntiAlias = true;
      const omega = (2 * Math.PI * ANTI_ALIAS_CUTOFF_HZ) / this.hostSampleRate;
      const cosOmega = Math.cos(omega);
      const sinOmega = Math.sin(omega);
      const alpha = sinOmega / Math.SQRT2;
      const a0 = 1 + alpha;
      this.aaB0 = (1 - cosOmega) / (2 * a0);
      this.aaB1 = (1 - cosOmega) / a0;
      this.aaB2 = this.aaB0;
      this.aaA1 = (-2 * cosOmega) / a0;
      this.aaA2 = (1 - alpha) / a0;
    } else {
      this.useAntiAlias = false;
      this.aaB0 = 1;
      this.aaB1 = 0;
      this.aaB2 = 0;
      this.aaA1 = 0;
      this.aaA2 = 0;
    }

    this.applyReporting({
      reportIntervalMs:
        processorOptions.reportIntervalMs ?? DEFAULT_REPORT_INTERVAL_MS,
      feedback: processorOptions.feedback ?? true,
      telemetry: processorOptions.telemetry ?? true,
    });

    this.port.onmessage = (event: MessageEvent<TuneletWorkletCommand>) => {
      this.handleCommand(event.data);
    };
    (this.port as { unref?: () => void }).unref?.();
  }

  /** The nominal processor latency in host frames. */
  get latencyFrames(): number {
    if (!this.needsResample) {
      return this.engine.latencyFrames;
    }
    return Math.round(
      (this.engine.latencyFrames + RESAMPLE_CUSHION_FRAMES) * this.stepDown,
    );
  }

  /** Processes one Web Audio render quantum of `Float32Array` channels. */
  process(
    inputs: readonly (readonly Float32Array[])[],
    outputs: readonly (readonly Float32Array[])[],
  ): boolean {
    if (this.disposed) {
      return false;
    }

    const outputChannels = outputs[0];
    if (!outputChannels || outputChannels.length === 0) {
      return true;
    }

    const outL = outputChannels[0];
    const totalHostFrames = outL.length;
    if (totalHostFrames === 0) {
      return true;
    }

    const outR = outputChannels.length > 1 ? outputChannels[1] : undefined;
    for (let ch = 2; ch < outputChannels.length; ch++) {
      outputChannels[ch].fill(0);
    }

    const inputChannels = inputs[0];
    let offset = 0;
    while (offset < totalHostFrames) {
      const chunkFrames = Math.min(
        this.maxBlockFrames,
        totalHostFrames - offset,
      );
      if (this.needsResample) {
        this.processResampledChunk(
          inputChannels,
          outL,
          outR,
          offset,
          chunkFrames,
        );
      } else {
        this.processDirectChunk(
          inputChannels,
          outL,
          outR,
          offset,
          chunkFrames,
        );
      }
      offset += chunkFrames;
    }

    this.maybePostReport(totalHostFrames);
    return !this.disposed;
  }

  /** Processes a chunk at 44.1 kHz or 48 kHz without rate conversion. */
  private processDirectChunk(
    inputChannels: readonly Float32Array[] | undefined,
    outL: Float32Array,
    outR: Float32Array | undefined,
    offset: number,
    frames: number,
  ): void {
    const inView = this.pcmInputViews[frames];
    const leftView = this.pcmLeftViews[frames];
    const rightView = this.pcmRightViews[frames];

    if (this.draining && this.pendingEvents.length === 0) {
      const rendered =
        outR !== undefined
          ? this.engine.drain(leftView, rightView)
          : this.engine.drainMono(inView);
      if (outR !== undefined) {
        writeStereoFloat32(this.pcmLeft, this.pcmRight, outL, outR, offset, frames);
      } else {
        writeMonoFloat32(this.pcmInput, outL, offset, frames);
      }
      if (rendered === 0) {
        this.draining = false;
        this.postMessage({ type: "drainComplete" });
      }
      return;
    }

    this.draining = false;
    this.fillMonoPcm(inputChannels, offset, frames);

    const events = this.consumeEvents(frames, frames, false);
    if (outR !== undefined) {
      this.runStereoProcess(inView, leftView, rightView, events);
      writeStereoFloat32(this.pcmLeft, this.pcmRight, outL, outR, offset, frames);
    } else {
      this.runMonoProcess(inView, events);
      writeMonoFloat32(this.pcmInput, outL, offset, frames);
    }
  }

  /** Processes a chunk through the block resampler to and from 48 kHz. */
  private processResampledChunk(
    inputChannels: readonly Float32Array[] | undefined,
    outL: Float32Array,
    outR: Float32Array | undefined,
    offset: number,
    hostFrames: number,
  ): void {
    if (this.draining && this.pendingEvents.length === 0) {
      let engineFrames = 0;
      while (this.inputCursor < hostFrames) {
        engineFrames += 1;
        this.inputCursor += this.stepDown;
      }
      this.inputCursor -= hostFrames;

      let rendered = 0;
      if (engineFrames > 0) {
        rendered =
          outR !== undefined
            ? this.engine.drain(
                this.pcmLeftViews[engineFrames],
                this.pcmRightViews[engineFrames],
              )
            : this.engine.drainMono(this.pcmInputViews[engineFrames]);
        this.appendEngineOutput(engineFrames, outR !== undefined);
      }
      this.upsampleOutput(outL, outR, offset, hostFrames);

      if (engineFrames > 0 && rendered === 0) {
        this.draining = false;
        this.postMessage({ type: "drainComplete" });
      }
      return;
    }

    this.draining = false;
    this.fillInputHistory(inputChannels, offset, hostFrames);

    let engineFrames = 0;
    while (this.inputCursor < hostFrames) {
      const index = this.inputCursor | 0;
      const frac = this.inputCursor - index;
      const sample =
        this.inputHistory[index] +
        (this.inputHistory[index + 1] - this.inputHistory[index]) * frac;
      this.pcmInput[engineFrames] = floatToInt16(sample);
      engineFrames += 1;
      this.inputCursor += this.stepDown;
    }
    this.inputHistory[0] = this.inputHistory[hostFrames];
    this.inputCursor -= hostFrames;

    if (engineFrames > 0) {
      const events = this.consumeEvents(hostFrames, engineFrames, true);
      const inView = this.pcmInputViews[engineFrames];
      if (outR !== undefined) {
        this.runStereoProcess(
          inView,
          this.pcmLeftViews[engineFrames],
          this.pcmRightViews[engineFrames],
          events,
        );
      } else {
        this.runMonoProcess(inView, events);
      }
      this.appendEngineOutput(engineFrames, outR !== undefined);
    }

    this.upsampleOutput(outL, outR, offset, hostFrames);
  }

  /** Executes `Tunelet.process` and recovers if queued events are invalid. */
  private runStereoProcess(
    inView: Int16Array,
    leftView: Int16Array,
    rightView: Int16Array,
    events: TimedEvent[] | undefined,
  ): void {
    if (events === undefined) {
      this.engine.process(inView, leftView, rightView);
      return;
    }
    try {
      this.engine.process(inView, leftView, rightView, events);
    } catch (error: unknown) {
      this.postError(error);
      this.engine.process(inView, leftView, rightView);
    }
  }

  /** Executes `Tunelet.processMono` and recovers if queued events are invalid. */
  private runMonoProcess(
    inView: Int16Array,
    events: TimedEvent[] | undefined,
  ): void {
    if (events === undefined) {
      this.engine.processMono(inView);
      return;
    }
    try {
      this.engine.processMono(inView, events);
    } catch (error: unknown) {
      this.postError(error);
      this.engine.processMono(inView);
    }
  }

  /** Converts mono or multi-channel `Float32` input directly into `pcmInput`. */
  private fillMonoPcm(
    inputChannels: readonly Float32Array[] | undefined,
    offset: number,
    frames: number,
  ): void {
    const channelCount = inputChannels?.length ?? 0;
    if (channelCount === 0) {
      this.pcmInputViews[frames].fill(0);
      return;
    }
    if (channelCount === 1) {
      const ch0 = inputChannels![0];
      const available = Math.max(0, Math.min(frames, ch0.length - offset));
      for (let i = 0; i < available; i++) {
        this.pcmInput[i] = floatToInt16(ch0[offset + i]);
      }
      for (let i = available; i < frames; i++) {
        this.pcmInput[i] = 0;
      }
      return;
    }

    this.mixChannelsToScratch(inputChannels!, offset, frames);
    for (let i = 0; i < frames; i++) {
      this.pcmInput[i] = floatToInt16(this.monoScratch[i]);
    }
  }

  /** Writes filtered mono host samples into `inputHistory[1..hostFrames]`. */
  private fillInputHistory(
    inputChannels: readonly Float32Array[] | undefined,
    offset: number,
    frames: number,
  ): void {
    const channelCount = inputChannels?.length ?? 0;
    if (channelCount === 0) {
      for (let i = 0; i < frames; i++) {
        this.inputHistory[i + 1] = this.filterSample(0);
      }
      return;
    }
    if (channelCount === 1) {
      const ch0 = inputChannels![0];
      const available = Math.max(0, Math.min(frames, ch0.length - offset));
      for (let i = 0; i < available; i++) {
        this.inputHistory[i + 1] = this.filterSample(ch0[offset + i]);
      }
      for (let i = available; i < frames; i++) {
        this.inputHistory[i + 1] = this.filterSample(0);
      }
      return;
    }

    this.mixChannelsToScratch(inputChannels!, offset, frames);
    for (let i = 0; i < frames; i++) {
      this.inputHistory[i + 1] = this.filterSample(this.monoScratch[i]);
    }
  }

  /** Averages all connected input channels into `monoScratch[0..frames]`. */
  private mixChannelsToScratch(
    channels: readonly Float32Array[],
    offset: number,
    frames: number,
  ): void {
    const count = channels.length;
    const scale = 1 / count;
    for (let i = 0; i < frames; i++) {
      let sum = 0;
      const sampleIndex = offset + i;
      for (let ch = 0; ch < count; ch++) {
        const channel = channels[ch];
        if (sampleIndex < channel.length) {
          sum += channel[sampleIndex] || 0;
        }
      }
      this.monoScratch[i] = sum * scale;
    }
  }

  /** Applies the anti-alias filter when the host rate is above 48 kHz. */
  private filterSample(raw: number): number {
    const x = raw <= -1 ? -1 : raw >= 1 ? 1 : raw || 0;
    if (!this.useAntiAlias) {
      return x;
    }
    const y = this.aaB0 * x + this.aaZ1;
    this.aaZ1 = this.aaB1 * x - this.aaA1 * y + this.aaZ2;
    this.aaZ2 = this.aaB2 * x - this.aaA2 * y;
    return y;
  }

  /** Appends rendered 48 kHz `Int16` frames to the float output queue. */
  private appendEngineOutput(engineFrames: number, stereo: boolean): void {
    let writeIndex = this.queueLength;
    if (stereo) {
      for (let i = 0; i < engineFrames; i++) {
        this.outQueueL[writeIndex] = this.pcmLeft[i] * INV_PCM_SCALE;
        this.outQueueR[writeIndex] = this.pcmRight[i] * INV_PCM_SCALE;
        writeIndex += 1;
      }
    } else {
      for (let i = 0; i < engineFrames; i++) {
        this.outQueueL[writeIndex] = this.pcmInput[i] * INV_PCM_SCALE;
        writeIndex += 1;
      }
    }
    this.queueLength = writeIndex;
  }

  /** Interpolates `hostFrames` output frames from the 48 kHz output queue. */
  private upsampleOutput(
    outL: Float32Array,
    outR: Float32Array | undefined,
    offset: number,
    hostFrames: number,
  ): void {
    const maxIndex = Math.max(0, this.queueLength - 2);
    if (outR !== undefined) {
      for (let i = 0; i < hostFrames; i++) {
        const rawIndex = this.outputCursor | 0;
        const index = rawIndex <= maxIndex ? rawIndex : maxIndex;
        const frac = rawIndex <= maxIndex ? this.outputCursor - rawIndex : 1;
        outL[offset + i] =
          this.outQueueL[index] +
          (this.outQueueL[index + 1] - this.outQueueL[index]) * frac;
        outR[offset + i] =
          this.outQueueR[index] +
          (this.outQueueR[index + 1] - this.outQueueR[index]) * frac;
        this.outputCursor += this.stepUp;
      }
    } else {
      for (let i = 0; i < hostFrames; i++) {
        const rawIndex = this.outputCursor | 0;
        const index = rawIndex <= maxIndex ? rawIndex : maxIndex;
        const frac = rawIndex <= maxIndex ? this.outputCursor - rawIndex : 1;
        outL[offset + i] =
          this.outQueueL[index] +
          (this.outQueueL[index + 1] - this.outQueueL[index]) * frac;
        this.outputCursor += this.stepUp;
      }
    }

    const maxConsume = Math.max(0, this.queueLength - 2);
    const consumed = Math.min(this.outputCursor | 0, maxConsume);
    if (consumed > 0) {
      const kept = this.queueLength - consumed;
      for (let i = 0; i < kept; i++) {
        this.outQueueL[i] = this.outQueueL[consumed + i];
      }
      if (outR !== undefined) {
        for (let i = 0; i < kept; i++) {
          this.outQueueR[i] = this.outQueueR[consumed + i];
        }
      }
      this.queueLength = kept;
      this.outputCursor -= consumed;
    }
  }

  /** Consumes queued `TimedEvent` items for one processed engine block. */
  private consumeEvents(
    hostFrames: number,
    engineFrames: number,
    scaleOffsets: boolean,
  ): TimedEvent[] | undefined {
    if (this.pendingEvents.length === 0) {
      return undefined;
    }
    const batch = this.pendingEvents;
    this.pendingEvents = [];
    if (scaleOffsets) {
      for (let i = 0; i < batch.length; i++) {
        const current = batch[i];
        if (current.offset >= 0 && current.offset < hostFrames) {
          current.offset = Math.min(
            engineFrames - 1,
            Math.floor((current.offset * engineFrames) / hostFrames),
          );
        } else if (current.offset >= hostFrames) {
          current.offset = engineFrames;
        }
      }
    }
    return batch;
  }

  /** Posts throttled `Feedback` and `Telemetry` snapshots at the UI interval. */
  private maybePostReport(processedHostFrames: number): void {
    if (
      this.reportIntervalFrames <= 0 ||
      (!this.feedbackEnabled && !this.telemetryEnabled)
    ) {
      return;
    }

    this.framesSinceReport += processedHostFrames;
    if (this.framesSinceReport < this.reportIntervalFrames) {
      return;
    }
    this.framesSinceReport %= this.reportIntervalFrames;

    const feedback: Feedback | undefined = this.feedbackEnabled
      ? this.engine.takeFeedback()
      : undefined;
    const telemetry: Telemetry | undefined = this.telemetryEnabled
      ? this.engine.takeTelemetry()
      : undefined;
    this.postMessage({ type: "report", feedback, telemetry });
  }

  /** Handles control messages from `TuneletWorkletNode`. */
  private handleCommand(command: TuneletWorkletCommand | undefined): void {
    if (!command || this.disposed) {
      return;
    }

    try {
      switch (command.type) {
        case "setConfig":
          this.draining = false;
          this.engine.setConfig(command.config);
          break;
        case "events":
          this.enqueueEvents(command.events);
          break;
        case "reset":
          this.resetProcessorState();
          break;
        case "beginDrain": {
          const tailFrames = this.engine.beginDrain();
          this.draining = tailFrames > 0;
          this.postMessage({ type: "drainStarted", tailFrames });
          if (!this.draining) {
            this.postMessage({ type: "drainComplete" });
          }
          break;
        }
        case "setReporting":
          this.applyReporting(command);
          break;
        case "dispose":
          this.disposeProcessor();
          break;
      }
    } catch (error: unknown) {
      this.postError(error);
    }
  }

  /** Appends new `TimedEvent` items in ascending offset order. */
  private enqueueEvents(events: readonly TimedEvent[]): void {
    if (!Array.isArray(events) || events.length === 0) {
      return;
    }
    let needsSort = false;
    let lastOffset =
      this.pendingEvents.length > 0
        ? this.pendingEvents[this.pendingEvents.length - 1].offset
        : 0;
    for (let i = 0; i < events.length; i++) {
      const item = events[i];
      if (item.offset < lastOffset) {
        needsSort = true;
      }
      lastOffset = item.offset;
      this.pendingEvents.push({ offset: item.offset, event: item.event });
    }
    if (needsSort) {
      this.pendingEvents.sort((a, b) => a.offset - b.offset);
    }
  }

  /** Clears the resampler histories, event queue and native processor state. */
  private resetProcessorState(): void {
    this.pendingEvents.length = 0;
    this.draining = false;
    this.aaZ1 = 0;
    this.aaZ2 = 0;
    this.inputCursor = 0;
    this.outputCursor = 0;
    if (this.needsResample) {
      this.inputHistory.fill(0);
      this.outQueueL.fill(0);
      this.outQueueR.fill(0);
      this.queueLength = RESAMPLE_CUSHION_FRAMES;
    }
    this.engine.reset();
  }

  /** Updates the throttled UI report cadence and active report fields. */
  private applyReporting(options: TuneletReportingOptions): void {
    if (options.feedback !== undefined) {
      this.feedbackEnabled = Boolean(options.feedback);
    }
    if (options.telemetry !== undefined) {
      this.telemetryEnabled = Boolean(options.telemetry);
    }
    if (options.reportIntervalMs !== undefined) {
      const intervalMs = options.reportIntervalMs;
      if (!Number.isFinite(intervalMs) || intervalMs < 0) {
        throw new RangeError(
          "reportIntervalMs must be zero or a positive finite number",
        );
      }
      if (intervalMs === 0) {
        this.reportIntervalFrames = 0;
        this.framesSinceReport = 0;
      } else {
        this.reportIntervalFrames = Math.max(
          1,
          Math.round((intervalMs / 1000) * this.hostSampleRate),
        );
        if (this.framesSinceReport >= this.reportIntervalFrames) {
          this.framesSinceReport = 0;
        }
      }
    }
  }

  /** Releases the Wasm `Tunelet` memory and marks the worklet as complete. */
  private disposeProcessor(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    this.pendingEvents.length = 0;
    this.port.onmessage = null;
    this.engine.free();
  }

  /** Sends a typed message to the main thread port. */
  private postMessage(message: TuneletWorkletMessage): void {
    this.port.postMessage(message);
  }

  /** Reports an error message on the port and keeps audio output active. */
  private postError(error: unknown): void {
    const message = error instanceof Error ? error.message : String(error);
    this.postMessage({ type: "error", message });
  }
}

/** Pre-allocates `Int16Array` prefix views for lengths `0..=maxFrames`. */
function createLengthViews(
  buffer: Int16Array,
  maxFrames: number,
): Int16Array[] {
  const views = new Array<Int16Array>(maxFrames + 1);
  for (let length = 0; length <= maxFrames; length++) {
    views[length] = buffer.subarray(0, length);
  }
  return views;
}

/** Clamps one `Float32` sample to `-1..=1` and quantizes it to `i16`. */
function floatToInt16(sample: number): number {
  const clamped = sample <= -1 ? -1 : sample >= 1 ? 1 : sample || 0;
  return clamped < 0 ? Math.round(clamped * 32768) : Math.round(clamped * 32767);
}

/** Converts stereo `Int16` buffers to `Float32` output slices. */
function writeStereoFloat32(
  pcmL: Int16Array,
  pcmR: Int16Array,
  outL: Float32Array,
  outR: Float32Array,
  offset: number,
  frames: number,
): void {
  for (let i = 0; i < frames; i++) {
    outL[offset + i] = pcmL[i] * INV_PCM_SCALE;
    outR[offset + i] = pcmR[i] * INV_PCM_SCALE;
  }
}

/** Converts a mono `Int16` buffer to a `Float32` output slice. */
function writeMonoFloat32(
  pcm: Int16Array,
  out: Float32Array,
  offset: number,
  frames: number,
): void {
  for (let i = 0; i < frames; i++) {
    out[offset + i] = pcm[i] * INV_PCM_SCALE;
  }
}

/** Reads the `sampleRate` global from `AudioWorkletGlobalScope` if present. */
function readGlobalSampleRate(): number | undefined {
  const candidate = (globalThis as { sampleRate?: unknown }).sampleRate;
  return typeof candidate === "number" ? candidate : undefined;
}

if (
  typeof (globalThis as { registerProcessor?: unknown }).registerProcessor ===
  "function"
) {
  (
    globalThis as unknown as {
      registerProcessor: (
        name: string,
        processorCtor: typeof TuneletWorkletProcessor,
      ) => void;
    }
  ).registerProcessor(TUNELET_WORKLET_PROCESSOR_NAME, TuneletWorkletProcessor);
}
