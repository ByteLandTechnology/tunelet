/**
 * The ESM entry point for Node.js.
 *
 * Call `loadTunelet()` before you construct processors or read the default
 * configuration. This module re-exports all named exports of the module that
 * wasm-bindgen generates, which include `Tunelet`, `defaultConfig`, `initSync`
 * and the configuration, control-activity, event and feedback types.
 */
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
import { init, isUrlLike, loadOnce } from "./shared.js";
import type { InitInput, InitOutput } from "./shared.js";

export * from "../pkg/tunelet.js";

/** Package subpaths for bundled Node.js scripts without `../pkg/`. */
const PACKAGE_WASM_SPECIFIERS = [
  "tunelet/pkg/tunelet_bg.wasm",
  "@bytelandtechnology/tunelet/pkg/tunelet_bg.wasm",
] as const;

/**
 * Initializes the Wasm module one time for this imported package instance.
 *
 * If you give no argument, the function reads the packaged binary relative to
 * this module. If a bundler moves this module and `../pkg/tunelet_bg.wasm` is
 * absent, the function resolves `pkg/tunelet_bg.wasm` from the installed
 * package. An import of the package does not load Wasm.
 *
 * For a custom deployment, you can give one of these sources:
 *
 * - A URL or URL string, which includes `file:` URLs.
 * - A fetch Request or Response.
 * - A byte buffer (ArrayBuffer or ArrayBufferView).
 * - A compiled WebAssembly.Module.
 * - A promise for one of these.
 *
 * Non-`file:` string sources obey fetch rules. A compiled module must use
 * imports that agree with the packaged bindings.
 *
 * Concurrent calls share one promise, and the first call selects the source.
 * After a successful initialization, later calls use it again and do not load
 * Wasm again, whatever their arguments are. If an attempt fails, the promise
 * rejects and the function clears the cached promise. Then you can try again.
 * File, network and Wasm errors go to the caller. The function does not try a
 * different load method after Wasm corruption or an invalid explicit source.
 *
 * @param source The optional Wasm source. The default is the binary in this
 * package.
 * @returns The initialized Wasm exports. Use the re-exported `Tunelet` class
 * for processors.
 */
export function loadTunelet(
  source?: InitInput | Promise<InitInput>,
): Promise<InitOutput> {
  return loadOnce(source, initializeNode);
}

/** Loads the default binary or an explicit Node.js or fetch source. */
async function initializeNode(
  source: InitInput | undefined,
): Promise<InitOutput> {
  if (source === undefined) {
    return init({ module_or_path: await readDefaultWasm() });
  }

  if (typeof source === "string") {
    if (/^file:/i.test(source)) {
      const url = new URL(source);
      if (url.protocol === "file:") {
        return init({ module_or_path: await readFile(url) });
      }
    }
    return init({ module_or_path: source });
  }

  if (isUrlLike(source)) {
    const url = source instanceof URL ? source : new URL(source.href);
    if (url.protocol === "file:") {
      return init({ module_or_path: await readFile(url) });
    }
    return init({ module_or_path: url });
  }

  return init({ module_or_path: source });
}

/**
 * Reads `../pkg/tunelet_bg.wasm` first, or resolves the installed package
 * binary if the relative path is absent.
 */
async function readDefaultWasm(): Promise<Uint8Array> {
  let absentError: unknown;
  if (typeof import.meta.url === "string") {
    try {
      return await readFile(new URL("../pkg/tunelet_bg.wasm", import.meta.url));
    } catch (error: unknown) {
      if (!hasErrorCode(error, "ENOENT") && !hasErrorCode(error, "ENOTDIR")) {
        throw error;
      }
      absentError = error;
    }
  }

  const baseUrl =
    typeof import.meta.url === "string"
      ? import.meta.url
      : pathToFileURL(`${process.cwd()}/`).href;
  const nodeRequire = createRequire(baseUrl);

  for (const specifier of PACKAGE_WASM_SPECIFIERS) {
    let resolvedPath: string;
    try {
      resolvedPath = nodeRequire.resolve(specifier);
    } catch (error: unknown) {
      if (
        hasErrorCode(error, "MODULE_NOT_FOUND") ||
        hasErrorCode(error, "ERR_MODULE_NOT_FOUND")
      ) {
        continue;
      }
      throw error;
    }
    return readFile(resolvedPath);
  }

  if (absentError !== undefined) {
    throw absentError;
  }
  throw new Error("Cannot find the packaged tunelet_bg.wasm binary.");
}

/** Checks if an error object has a specific Node.js `code` string. */
function hasErrorCode(error: unknown, code: string): boolean {
  return (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    (error as { readonly code?: unknown }).code === code
  );
}

/** The default export is the same function as `loadTunelet`. */
export default loadTunelet;
