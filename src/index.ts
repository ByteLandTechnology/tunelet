/**
 * The shared ESM entry point for browsers and Node.js.
 *
 * Call `loadTunelet()` before you construct processors or read the default
 * configuration. Both environments use the same generated bindings and the
 * same Wasm binary. This module re-exports all named exports of the module
 * that wasm-bindgen generates, which include `Tunelet`, `defaultConfig`,
 * `initSync` and the configuration, control-activity, event and feedback
 * types.
 */
import { init, isUrlLike, loadOnce } from "./shared.js";
import type { InitInput, InitOutput } from "./shared.js";

export * from "../pkg/tunelet.js";

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
 * - A fetch Request or Response.
 * - A byte buffer (ArrayBuffer or ArrayBufferView).
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
  return loadOnce(source, initializeIndex);
}

/** Loads the default binary or an explicit source in browsers or Node.js. */
async function initializeIndex(
  source: InitInput | undefined,
): Promise<InitOutput> {
  const input =
    source === undefined
      ? new URL("../pkg/tunelet_bg.wasm", import.meta.url)
      : source;

  if (typeof input === "string") {
    if (/^file:/i.test(input)) {
      const url = new URL(input);
      if (url.protocol === "file:") {
        return readFileUrl(url);
      }
    }
    return init({ module_or_path: input });
  }

  if (isUrlLike(input)) {
    const url =
      typeof URL === "function" && input instanceof URL
        ? input
        : new URL(input.href);
    if (url.protocol === "file:") {
      return readFileUrl(url);
    }
    return init({ module_or_path: url });
  }

  return init({ module_or_path: input });
}

/** Reads a `file:` URL in Node.js without static Node.js imports. */
async function readFileUrl(fileUrl: URL): Promise<InitOutput> {
  // Keep the Node.js built-in module out of browser bundles. TypeScript
  // removes the type-only import. The runtime import runs only for `file:`
  // URLs.
  const moduleName = "node:fs/promises";
  const fs: typeof import("node:fs/promises") = await import(
    /* webpackIgnore: true */ /* @vite-ignore */ moduleName
  );
  return init({ module_or_path: await fs.readFile(fileUrl) });
}

/** The default export is the same function as `loadTunelet`. */
export default loadTunelet;
