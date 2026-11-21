/**
 * Shared constants and `MessagePort` payloads for `tunelet/worklet` and
 * `tunelet/worklet-processor`.
 */
import type {
  BlockEvents,
  ControlEvent,
  Feedback,
  MidiEvent,
  ProcessorConfig,
  SyncInitInput,
  Telemetry,
} from "./index.js";

/** The default Web Audio processor registration name. */
export const TUNELET_PROCESSOR_NAME = "tunelet-processor";

/** The default maximum block length in frames for Web Audio worklet chunks. */
export const DEFAULT_WORKLET_BLOCK_FRAMES = 128;

/**
 * The initialization fields in `AudioWorkletNodeOptions.processorOptions`.
 */
export interface TuneletProcessorOptions {
  /** The WebAssembly bytes or compiled module for `initSync`. */
  wasm: SyncInitInput;
  /** The maximum frame count for one internal chunk. The default is 128. */
  maxBlockFrames?: number;
  /** The optional initial processor configuration. */
  config?: ProcessorConfig | null;
}

/** Options for `beginDrain` on a `TuneletWorkletNode`. */
export interface TuneletDrainOptions {
  /**
   * If `true`, the worklet processor returns `false` from `process` after the
   * tail budget completes. The default is `false`.
   */
  stopAfterDrain?: boolean;
}

/** The result of a formant-envelope request over `MessagePort`. */
export interface FormantEnvelopeResult {
  /** `true` if the formant stage produced valid envelopes. */
  valid: boolean;
  /** The analysis vocal-tract envelope. */
  input: Float32Array;
  /** The synthesis vocal-tract envelope after width and stretch controls. */
  output: Float32Array;
}

/** Commands that the main thread sends to `TuneletWorkletProcessor`. */
export type TuneletWorkletCommand =
  | { type: "config" }
  | { type: "setConfig"; config: ProcessorConfig }
  | { type: "midi"; event: MidiEvent }
  | { type: "correctionMidi"; event: MidiEvent }
  | { type: "control"; event: ControlEvent }
  | { type: "events"; events: BlockEvents }
  | { type: "reset" }
  | { type: "beginDrain"; stopAfterDrain?: boolean }
  | { type: "tailFrames" }
  | { type: "feedback" }
  | { type: "takeFeedback" }
  | { type: "telemetry" }
  | { type: "takeTelemetry" }
  | { type: "formantEnvelope"; maxFrequencyHz: number; points: number }
  | { type: "free" };

/** A command message on `MessagePort`, with an optional request identifier. */
export type TuneletWorkletRequest = TuneletWorkletCommand & {
  id?: number;
};

/** A response to a `TuneletWorkletRequest` from `TuneletWorkletProcessor`. */
export type TuneletWorkletResponse =
  | {
      __tunelet: "response";
      id?: number;
      requestType: TuneletWorkletCommand["type"];
      ok: true;
      value?:
        | ProcessorConfig
        | Feedback
        | Telemetry
        | FormantEnvelopeResult
        | number;
    }
  | {
      __tunelet: "response";
      id?: number;
      requestType: TuneletWorkletCommand["type"];
      ok: false;
      error: string;
    };

/** Asynchronous lifecycle notifications from `TuneletWorkletProcessor`. */
export type TuneletWorkletNotification =
  | {
      __tunelet: "ready";
      sampleRate: number;
      maxBlockFrames: number;
      latencyFrames: number;
    }
  | {
      __tunelet: "initError";
      error: string;
    }
  | {
      __tunelet: "drainComplete";
    }
  | {
      __tunelet: "processError";
      error: string;
    };

/** All messages that `TuneletWorkletProcessor` posts on `MessagePort`. */
export type TuneletWorkletOutboundMessage =
  | TuneletWorkletResponse
  | TuneletWorkletNotification;
