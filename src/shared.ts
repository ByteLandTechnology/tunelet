/**
 * Shared Wasm bindings, audio views and initialization state for the package
 * entries.
 */
import init from "../pkg/tunelet.js";
import type { InitInput, InitOutput, Tunelet } from "../pkg/tunelet.js";

export { init };
export type { InitInput, InitOutput };

/**
 * Cached `Float32Array` views over the in-place `Tunelet` buffers in Wasm
 * linear memory.
 *
 * Each access to `input`, `left` or `right` checks `memory.buffer`. If Wasm
 * memory grew and detached the previous `ArrayBuffer`, the helper binds the
 * three fixed pointers of `processor` to the new buffer. If `memory.buffer` did
 * not change, the helper returns the cached views with no allocation.
 *
 * Read `input` before you write input samples. If an in-place call passes
 * `events`, read `left` and `right` (or `input` for mono) after that call.
 * When you call `processor.free()`, also call `dispose()`. Do not use the views
 * after `processor.free()`.
 */
export class TuneletAudioViews {
  private readonly memory: InitOutput["memory"];
  private processor: Tunelet | null;
  /** The fixed frame capacity of `input`, `left` and `right`. */
  readonly maxBlockFrames: number;
  private cachedBuffer: ArrayBuffer | null = null;
  private inputView: Float32Array | null = null;
  private leftView: Float32Array | null = null;
  private rightView: Float32Array | null = null;

  /**
   * Binds `Float32Array` views for `processor` over `memory`.
   *
   * @param memory The Wasm memory from `InitOutput["memory"]`.
   * @param processor The live `Tunelet` instance that owns the buffers.
   */
  constructor(memory: InitOutput["memory"], processor: Tunelet) {
    this.memory = memory;
    this.processor = processor;
    this.maxBlockFrames = processor.maxBlockFrames;
    this.refresh();
  }

  /** Returns `true` after `dispose()` clears the cached views. */
  get disposed(): boolean {
    return this.processor === null;
  }

  /**
   * Returns the `Float32Array` view for mono input and in-place mono output.
   *
   * If `memory.buffer` changed, this getter refreshes all three views first.
   */
  get input(): Float32Array {
    this.refresh();
    return this.inputView!;
  }

  /**
   * Returns the `Float32Array` view for the left stereo output channel.
   *
   * If `memory.buffer` changed, this getter refreshes all three views first.
   */
  get left(): Float32Array {
    this.refresh();
    return this.leftView!;
  }

  /**
   * Returns the `Float32Array` view for the right stereo output channel.
   *
   * If `memory.buffer` changed, this getter refreshes all three views first.
   */
  get right(): Float32Array {
    this.refresh();
    return this.rightView!;
  }

  /**
   * If Wasm memory grew, this method refreshes the three `Float32Array` views
   * from `memory.buffer`.
   */
  refresh(): void {
    const processor = this.processor;
    if (processor === null) {
      throw new Error("TuneletAudioViews is disposed");
    }
    const buffer = this.memory.buffer;
    if (this.cachedBuffer !== buffer) {
      const frames = this.maxBlockFrames;
      const inputPtr = processor.inputF32Ptr;
      const leftPtr = processor.leftF32Ptr;
      const rightPtr = processor.rightF32Ptr;
      const input = new Float32Array(buffer, inputPtr, frames);
      const left = new Float32Array(buffer, leftPtr, frames);
      const right = new Float32Array(buffer, rightPtr, frames);
      this.inputView = input;
      this.leftView = left;
      this.rightView = right;
      this.cachedBuffer = buffer;
    }
  }

  /** Clears the cached `ArrayBuffer`, `Float32Array` views and processor link. */
  dispose(): void {
    this.processor = null;
    this.cachedBuffer = null;
    this.inputView = null;
    this.leftView = null;
    this.rightView = null;
  }
}

export interface TuneletAudioViews {
  [Symbol.dispose](): void;
}

if (Symbol.dispose) {
  TuneletAudioViews.prototype[Symbol.dispose] =
    TuneletAudioViews.prototype.dispose;
}

/** The shared initialization promise, which a failure clears. */
let initialization: Promise<InitOutput> | undefined;

/**
 * Runs one shared initialization attempt and clears the cached promise if the
 * attempt fails.
 */
export function loadOnce(
  source: InitInput | Promise<InitInput> | undefined,
  load: (source: InitInput | undefined) => Promise<InitOutput>,
): Promise<InitOutput> {
  initialization ??= resolveAndLoad(source, load).catch((error: unknown) => {
    initialization = undefined;
    throw error;
  });
  return initialization;
}

/** Awaits an explicit source, rejects `undefined` results and loads Wasm. */
async function resolveAndLoad(
  source: InitInput | Promise<InitInput> | undefined,
  load: (source: InitInput | undefined) => Promise<InitOutput>,
): Promise<InitOutput> {
  if (source === undefined) {
    return load(undefined);
  }
  const resolved = await source;
  if (resolved === undefined) {
    throw new TypeError("The Wasm source must not be undefined.");
  }
  return load(resolved);
}

/** Checks if a value is a native `URL` or a cross-realm `URL` instance. */
export function isUrlLike(
  value: unknown,
): value is { readonly href: string; readonly protocol: string } {
  if (typeof URL === "function" && value instanceof URL) {
    return true;
  }
  if (
    typeof value !== "object" ||
    value === null ||
    ArrayBuffer.isView(value) ||
    value instanceof ArrayBuffer
  ) {
    return false;
  }
  const candidate = value as {
    readonly href?: unknown;
    readonly protocol?: unknown;
  };
  return (
    typeof candidate.href === "string" && typeof candidate.protocol === "string"
  );
}
