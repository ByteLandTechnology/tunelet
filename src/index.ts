/**
 * The shared ESM entry point for browsers, `AudioWorkletGlobalScope` and
 * Node.js.
 *
 * Call `loadTunelet()` or `initTuneletSync(module)` before you construct
 * processors or read the default configuration. All environments use the same
 * generated bindings and the same Wasm binary. This module installs an
 * internal UTF-8 fallback when `TextDecoder` or `TextEncoder` is undefined.
 * It also re-exports all named exports of the module that wasm-bindgen
 * generates, which include `Tunelet`, `defaultConfig`, `initSync` and the
 * configuration, event and feedback types.
 */
import "./utf8.js";
import init, { initSync } from "../pkg/tunelet.js";
import type { InitInput, InitOutput, SyncInitInput } from "../pkg/tunelet.js";

export * from "../pkg/tunelet.js";

/** The initialized Wasm exports, kept after synchronous or asynchronous load. */
let initialized: InitOutput | undefined;

/** The shared initialization promise, which a failure clears. */
let initialization: Promise<InitOutput> | undefined;

/**
 * Initializes the Wasm module synchronously for this imported package
 * instance.
 *
 * Use this function in `AudioWorkletGlobalScope`, where `fetch` is not
 * available. Compile a `WebAssembly.Module` on the main thread, send it to the
 * worklet and pass it to `initTuneletSync(module)`. You can also give a byte
 * buffer (`BufferSource`).
 *
 * After a successful initialization, later calls return the same exports and
 * do not instantiate Wasm again. If instantiation fails, the function throws
 * and does not cache the failure.
 *
 * @param module A compiled `WebAssembly.Module` or Wasm bytes.
 * @returns The initialized Wasm exports.
 */
export function initTuneletSync(module: SyncInitInput): InitOutput {
  initialized ??= initSync({ module });
  return initialized;
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
  if (initialized !== undefined) {
    initialization ??= Promise.resolve(initialized);
    return initialization;
  }
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
    initialized = await init({ module_or_path: await fs.readFile(input) });
    return initialized;
  }

  initialized = await init({ module_or_path: input });
  return initialized;
}

/** The default export is the same function as `loadTunelet`. */
export default loadTunelet;
