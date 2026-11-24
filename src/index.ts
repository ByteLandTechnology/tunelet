/**
 * The shared ESM entry point for browsers and Node.js.
 *
 * Call `loadTunelet()` before you construct processors or read the default
 * configuration. Both environments use the same Wasm binary and the same
 * portable TypeScript declarations, which include `Tunelet`, `defaultConfig`,
 * `initSync` and the configuration, event, feedback and telemetry types.
 */
import init from "./wasm.js";
import type { WasmInitInput, WasmInitOutput } from "./types.js";

export type * from "./types.js";
export { Tunelet, defaultConfig, initSync } from "./wasm.js";

/** The shared initialization promise, which a failure clears. */
let initialization: Promise<WasmInitOutput> | undefined;

/**
 * Initializes the Wasm module one time for this imported package instance.
 *
 * If you give no argument, the function finds the packaged binary relative to
 * this module. Node.js reads a `file:` URL, and browsers fetch the served
 * asset. An import of the package does not load Wasm. In browsers, serve the
 * assets over HTTP(S), or give the bytes or a precompiled module as the
 * source.
 *
 * For a custom deployment, you can give one of these sources:
 *
 * - A URL or a URL string.
 * - A fetch Request or Response.
 * - An ArrayBuffer or an ArrayBufferView.
 * - A precompiled Wasm module.
 * - A promise for one of these.
 *
 * In Node.js, `loadTunelet` reads `file:` URL strings and `file:` URL objects
 * from disk. Other string sources obey fetch rules. A precompiled module must
 * use imports that agree with the packaged bindings.
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
  source?: WasmInitInput | Promise<WasmInitInput>,
): Promise<WasmInitOutput> {
  initialization ??= initialize(source).catch((error: unknown) => {
    initialization = undefined;
    throw error;
  });
  return initialization;
}

/** Converts a `file:` URL string or `file:` URL object into a host `URL`. */
function toFileUrl(input: WasmInitInput): URL | undefined {
  if (typeof input === "string") {
    return /^file:/i.test(input) ? new URL(input) : undefined;
  }
  if (input instanceof URL) {
    return input.protocol === "file:" ? input : undefined;
  }
  if (
    typeof input === "object" &&
    input !== null &&
    "protocol" in input &&
    input.protocol === "file:" &&
    "href" in input &&
    typeof input.href === "string"
  ) {
    return new URL(input.href);
  }
  return undefined;
}

/** Awaits the source one time, then initializes the Wasm module from it. */
async function initialize(
  source: WasmInitInput | Promise<WasmInitInput> | undefined,
): Promise<WasmInitOutput> {
  const input =
    source === undefined
      ? new URL("../pkg/tunelet_bg.wasm", import.meta.url)
      : await source;

  const fileUrl = toFileUrl(input);
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
