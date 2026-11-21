/**
 * Shared constants and message types for `createTuneletNode` and
 * `tunelet/worklet`.
 */
import type {
  Feedback,
  ProcessorConfig,
  SyncInitInput,
  Telemetry,
  TimedEvent,
} from "../pkg/tunelet.js";

/** The registered `AudioWorkletProcessor` name for `tunelet/worklet`. */
export const TUNELET_WORKLET_PROCESSOR_NAME = "tunelet";

/** The default Web Audio block limit in frames. */
export const DEFAULT_MAX_BLOCK_FRAMES = 128;

/** The default interval between UI reports in milliseconds. */
export const DEFAULT_REPORT_INTERVAL_MS = 50;

/** Controls periodic feedback and telemetry snapshots from the worklet. */
export interface TuneletReportingOptions {
  /**
   * The minimum interval between UI reports in milliseconds. Set 0 to disable
   * periodic reports. The default is 50.
   */
  reportIntervalMs?: number;
  /** Enables periodic `Feedback` reports. The default is `true`. */
  feedback?: boolean;
  /** Enables periodic `Telemetry` reports. The default is `true`. */
  telemetry?: boolean;
}

/**
 * The `processorOptions` payload that `TuneletWorkletProcessor` reads at
 * construction.
 */
export interface TuneletProcessorOptions extends TuneletReportingOptions {
  /** The compiled Wasm module or bytes for synchronous initialization. */
  module?: SyncInitInput;
  /** The initial configuration for the `Tunelet` processor. */
  config?: ProcessorConfig;
  /**
   * The host `AudioContext` sample rate in hertz. The default is the global
   * `sampleRate` of `AudioWorkletGlobalScope`.
   */
  sampleRate?: number;
  /** The maximum host block size for one chunk in frames. The default is 128. */
  maxBlockFrames?: number;
}

/** Commands that the main thread sends to `TuneletWorkletProcessor`. */
export type TuneletWorkletCommand =
  | { type: "setConfig"; config: ProcessorConfig }
  | { type: "events"; events: TimedEvent[] }
  | { type: "reset" }
  | { type: "beginDrain" }
  | ({ type: "setReporting" } & TuneletReportingOptions)
  | { type: "dispose" };

/** Messages that `TuneletWorkletProcessor` sends to the main thread. */
export type TuneletWorkletMessage =
  | { type: "report"; feedback?: Feedback; telemetry?: Telemetry }
  | { type: "drainStarted"; tailFrames: number }
  | { type: "drainComplete" }
  | { type: "error"; message: string };
