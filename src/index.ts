/**
 * The shared ESM entry point for browsers and Node.js.
 *
 * Call `loadTunelet()` before you construct processors or read the default
 * configuration. Both environments use the same generated bindings and the
 * same Wasm binary. This module exports `TuneletAudioViews` and re-exports all
 * named exports from wasm-bindgen. Those exports include `Tunelet`,
 * `defaultConfig`, `initSync` and the configuration, event and feedback types.
 */
import init from "../pkg/tunelet.js";
import type { InitInput, InitOutput, Tunelet } from "../pkg/tunelet.js";

export * from "../pkg/tunelet.js";

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
  private readonly memory: WebAssembly.Memory;
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
   * @param memory The `WebAssembly.Memory` from `InitOutput.memory`.
   * @param processor The live `Tunelet` instance that owns the buffers.
   */
  constructor(memory: WebAssembly.Memory, processor: Tunelet) {
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
    return init({ module_or_path: await fs.readFile(input) });
  }

  return init({ module_or_path: input });
}

/** The default export is the same function as `loadTunelet`. */
export default loadTunelet;
