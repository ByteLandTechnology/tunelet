/**
 * The ESM entry point for browsers, workers and bundlers.
 *
 * Call `loadTunelet()` before you construct processors or read the default
 * configuration. This module exports `TuneletAudioViews` and re-exports all
 * named exports of the module that wasm-bindgen generates. Those exports
 * include `Tunelet`, `defaultConfig`, `initSync` and the configuration,
 * control-activity, event and feedback types.
 */
import { init, isUrlLike, loadOnce, TuneletAudioViews } from "./shared.js";
import type { InitInput, InitOutput } from "./shared.js";

export * from "../pkg/tunelet.js";
export { TuneletAudioViews };

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
    if (resolved === undefined) {
      return init({
        module_or_path: new URL("../pkg/tunelet_bg.wasm", import.meta.url),
      });
    }
    if (isUrlLike(resolved)) {
      const url =
        typeof URL === "function" && resolved instanceof URL
          ? resolved
          : new URL(resolved.href);
      return init({ module_or_path: url });
    }
    return init({ module_or_path: resolved });
  });
}

/** The default export is the same function as `loadTunelet`. */
export default loadTunelet;
