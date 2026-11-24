/**
 * The ESM entry point for browsers, workers and bundlers.
 *
 * Call `loadTunelet()` before you construct processors or read the default
 * configuration. This module re-exports all named exports of the module that
 * wasm-bindgen generates, which include `Tunelet`, `defaultConfig`, `initSync`
 * and the configuration, event and feedback types.
 */
import { init, loadOnce } from "./shared.js";
import type { InitInput, InitOutput } from "./shared.js";

export * from "../pkg/tunelet.js";

/**
 * Initializes the Wasm module one time for this imported package instance.
 *
 * If you give no argument, the function fetches the packaged binary relative
 * to this module. An import of the package does not load Wasm. Serve the
 * assets over HTTP(S), or give the bytes or a compiled module as the source.
 *
 * For a custom deployment, you can give one of these sources:
 *
 * - A URL or URL string.
 * - A fetch Request or Response.
 * - A byte buffer (ArrayBuffer or ArrayBufferView).
 * - A compiled WebAssembly.Module.
 * - A promise for one of these.
 *
 * A compiled module must use imports that agree with the packaged bindings.
 *
 * Concurrent calls share one promise, and the first call selects the source.
 * After a successful initialization, later calls use it again and do not load
 * Wasm again, whatever their arguments are. If an attempt fails, the promise
 * rejects and the function clears the cached promise. Then you can try again.
 * Network and Wasm errors go to the caller. The function does not try a
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
  return loadOnce(source, (resolved) => {
    const input =
      resolved === undefined
        ? new URL("../pkg/tunelet_bg.wasm", import.meta.url)
        : resolved;
    return init({ module_or_path: input });
  });
}

/** The default export is the same function as `loadTunelet`. */
export default loadTunelet;
