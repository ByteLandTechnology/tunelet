/**
 * The shared ESM entry point for browsers and Node.js.
 *
 * Call `loadTunelet()` before you construct processors or read the default
 * configuration. Both environments use the same generated bindings and the
 * same Wasm binary. This module exports `TuneletAudioViews` and re-exports all
 * named exports of the module that wasm-bindgen generates. Those exports
 * include `Tunelet`, `defaultConfig`, `initSync` and the configuration,
 * control-activity, event and feedback types.
 */
import init from "../pkg/tunelet.js";
import type { InitInput, InitOutput, Tunelet } from "../pkg/tunelet.js";

export * from "../pkg/tunelet.js";

interface CachedAudioViews {
  readonly buffer: ArrayBuffer;
  readonly input: Float32Array;
  readonly left: Float32Array;
  readonly right: Float32Array;
}

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
  declare [Symbol.dispose]: () => void;
  private readonly memory: InitOutput["memory"];
  private processor: Tunelet | null;
  /** The fixed frame capacity of `input`, `left` and `right`. */
  readonly maxBlockFrames: number;
  private cachedViews: CachedAudioViews | null = null;

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
    return this.syncViews().input;
  }

  /**
   * Returns the `Float32Array` view for the left stereo output channel.
   *
   * If `memory.buffer` changed, this getter refreshes all three views first.
   */
  get left(): Float32Array {
    return this.syncViews().left;
  }

  /**
   * Returns the `Float32Array` view for the right stereo output channel.
   *
   * If `memory.buffer` changed, this getter refreshes all three views first.
   */
  get right(): Float32Array {
    return this.syncViews().right;
  }

  /**
   * If Wasm memory grew, this method refreshes the three `Float32Array` views
   * from `memory.buffer`.
   */
  refresh(): void {
    this.syncViews();
  }

  private syncViews(): CachedAudioViews {
    const processor = this.processor;
    if (processor === null) {
      throw new Error("TuneletAudioViews is disposed");
    }
    const buffer = this.memory.buffer;
    const cached = this.cachedViews;
    if (cached !== null && cached.buffer === buffer) {
      return cached;
    }
    const frames = this.maxBlockFrames;
    const inputPtr = processor.inputF32Ptr;
    const leftPtr = processor.leftF32Ptr;
    const rightPtr = processor.rightF32Ptr;
    const next: CachedAudioViews = {
      buffer,
      input: new Float32Array(buffer, inputPtr, frames),
      left: new Float32Array(buffer, leftPtr, frames),
      right: new Float32Array(buffer, rightPtr, frames),
    };
    this.cachedViews = next;
    return next;
  }

  /** Clears the cached `ArrayBuffer`, `Float32Array` views and processor link. */
  dispose(): void {
    this.processor = null;
    this.cachedViews = null;
  }
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
 * - A URL or URL string.
 * - A fetch Response.
 * - A byte buffer (BufferSource).
 * - A compiled WebAssembly.Module.
 * - A promise for one of these.
 *
 * In Node.js, the function reads `file:` URL objects and `"file:"` URL strings
 * from disk. Other URLs and string sources obey fetch rules. A compiled module
 * must use imports that agree with the packaged bindings.
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

/** Converts a `file:` URL string or `URL` object into a local `URL` instance. */
function resolveFileUrl(input: InitInput): URL | undefined {
  if (typeof URL === "function" && input instanceof URL) {
    return input.protocol === "file:" ? input : undefined;
  }
  if (typeof input === "string") {
    if (!/^file:/i.test(input)) {
      return undefined;
    }
    const url = new URL(input);
    return url.protocol === "file:" ? url : undefined;
  }
  if (
    typeof input === "object" &&
    input !== null &&
    "protocol" in input &&
    input.protocol === "file:" &&
    "href" in input &&
    typeof input.href === "string"
  ) {
    const url = new URL(input.href);
    return url.protocol === "file:" ? url : undefined;
  }
  return undefined;
}

/** Awaits the source one time, then initializes the Wasm module from it. */
async function initialize(
  source: InitInput | Promise<InitInput> | undefined,
): Promise<InitOutput> {
  const input =
    source === undefined
      ? new URL("../pkg/tunelet_bg.wasm", import.meta.url)
      : await source;

  const fileUrl = resolveFileUrl(input);
  if (fileUrl !== undefined) {
    // Keep the Node.js built-in module out of browser bundles. TypeScript
    // removes the type-only import. The runtime import runs only for `file:`
    // URLs.
    const moduleName = "node:fs/promises";
    const fs: typeof import("node:fs/promises") = await import(
      /* webpackIgnore: true */ /* @vite-ignore */ moduleName
    );
    return init({ module_or_path: await fs.readFile(fileUrl) });
  }

  return init({ module_or_path: input });
}

/** The default export is the same function as `loadTunelet`. */
export default loadTunelet;
