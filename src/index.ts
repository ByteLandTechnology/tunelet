/**
 * The shared ESM entry point for browsers and Node.js.
 *
 * Call `loadTunelet()` before you construct processors or read the default
 * configuration. To compile a `WebAssembly.Module` for `tunelet/worklet`, call
 * `loadTuneletModule()` on the main thread. This module re-exports all named
 * exports of the module that wasm-bindgen generates, which include `Tunelet`,
 * `defaultConfig`, `initSync` and the configuration, event and feedback types.
 */
import "./text-codec.js";
import init, { initSync } from "../pkg/tunelet.js";
import type { InitInput, InitOutput } from "../pkg/tunelet.js";

export * from "../pkg/tunelet.js";

/** The shared initialization promise, which a failure clears. */
let initialization: Promise<InitOutput> | undefined;

/** The cached compilation promise for the packaged Wasm binary. */
let packagedModule: Promise<WebAssembly.Module> | undefined;

/**
 * Initializes the Wasm module one time for this imported package instance.
 *
 * If you give no argument on the main thread or in Node.js, the function
 * compiles and instantiates the packaged binary relative to this module.
 * Node.js reads a `file:` URL, and browsers fetch the served asset. If you
 * call `loadTunelet()` with no argument in `AudioWorkletGlobalScope`, the
 * returned promise rejects with an `Error` and does not call `fetch()`.
 *
 * For a custom deployment or an `AudioWorkletGlobalScope`, you can give one of
 * these sources:
 *
 * - A URL (on the main thread or in Node.js).
 * - A fetch Response (on the main thread).
 * - A byte buffer (`BufferSource`).
 * - A compiled `WebAssembly.Module`.
 * - A promise for one of these.
 *
 * Concurrent calls share one promise, and the first call selects the source.
 * After a successful initialization, later calls use it again and do not load
 * Wasm again, whatever their arguments are. If an attempt fails, the promise
 * rejects and the function clears the cached promise. Then you can try again.
 *
 * @param source The optional Wasm source. The default is the binary in this
 * package on the main thread or in Node.js.
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

/**
 * Loads or compiles a `WebAssembly.Module` on the main thread or in Node.js.
 *
 * Give the returned `WebAssembly.Module` to `tunelet/worklet`, to
 * `initSync({ module })` or to `loadTunelet(module)`. If you omit `source`,
 * this function compiles the packaged binary one time and caches the promise.
 * If compilation fails, the function clears the cached promise so you can try
 * again.
 *
 * If you call `loadTuneletModule()` with no argument in
 * `AudioWorkletGlobalScope`, the returned promise rejects with an `Error`.
 *
 * @param source The optional Wasm source. The default is the binary in this
 * package.
 * @returns The compiled `WebAssembly.Module`.
 */
export function loadTuneletModule(
  source?: InitInput | Promise<InitInput>,
): Promise<WebAssembly.Module> {
  if (source !== undefined) {
    return compileSource(source);
  }
  packagedModule ??= compileDefaultModule().catch((error: unknown) => {
    packagedModule = undefined;
    throw error;
  });
  return packagedModule;
}

/** Checks if the current global scope is an `AudioWorkletGlobalScope`. */
function isAudioWorkletScope(): boolean {
  const scope = globalThis as {
    AudioWorkletGlobalScope?: abstract new () => object;
  };
  return (
    typeof scope.AudioWorkletGlobalScope === "function" &&
    globalThis instanceof scope.AudioWorkletGlobalScope
  );
}

/** Reads a `file:` URL with the Node.js file system module. */
async function readFileUrl(url: URL): Promise<Uint8Array> {
  // Keep the Node.js built-in module out of browser bundles. The runtime
  // import runs only for `file:` URLs.
  const moduleName = "node:fs/promises";
  const fs: typeof import("node:fs/promises") = await import(
    /* webpackIgnore: true */ /* @vite-ignore */ moduleName
  );
  return fs.readFile(url);
}

/** Compiles a Wasm `Response` through `WebAssembly.compileStreaming` when available. */
async function compileResponse(
  response: Response,
): Promise<WebAssembly.Module> {
  if (!response.ok) {
    throw new Error(
      `Failed to fetch Wasm: ${response.status} ${response.statusText} from '${response.url}'`,
    );
  }
  if (typeof WebAssembly.compileStreaming === "function") {
    const contentType = response.headers.get("Content-Type") ?? "";
    if (contentType.toLowerCase().includes("application/wasm")) {
      return WebAssembly.compileStreaming(response);
    }
  }
  return WebAssembly.compile(await response.arrayBuffer());
}

/** Compiles the packaged Wasm binary from a `file:` or HTTP(S) URL. */
async function compileDefaultModule(): Promise<WebAssembly.Module> {
  if (isAudioWorkletScope()) {
    throw new Error(
      "loadTuneletModule() without a source cannot run in AudioWorkletGlobalScope because fetch() is unavailable. Compile the module on the main thread and pass it in processorOptions.",
    );
  }
  return compileInput(new URL("../pkg/tunelet_bg.wasm", import.meta.url));
}

/** Resolves a custom `source` and compiles it to a `WebAssembly.Module`. */
async function compileSource(
  source: InitInput | Promise<InitInput>,
): Promise<WebAssembly.Module> {
  return compileInput(await source);
}

/** Compiles one resolved `InitInput` into a `WebAssembly.Module`. */
async function compileInput(input: InitInput): Promise<WebAssembly.Module> {
  if (input instanceof WebAssembly.Module) {
    return input;
  }
  if (input instanceof ArrayBuffer || ArrayBuffer.isView(input)) {
    return WebAssembly.compile(input as BufferSource);
  }
  if (input instanceof URL && input.protocol === "file:") {
    const bytes = await readFileUrl(input);
    return WebAssembly.compile(bytes as unknown as BufferSource);
  }
  if (typeof Response === "function" && input instanceof Response) {
    return compileResponse(input);
  }
  if (typeof fetch !== "function") {
    throw new Error(
      "fetch() is not available in this scope. Give a WebAssembly.Module or BufferSource instead.",
    );
  }
  return compileResponse(await fetch(input as RequestInfo | URL));
}

/** Awaits the source one time, then initializes the Wasm module from it. */
async function initialize(
  source: InitInput | Promise<InitInput> | undefined,
): Promise<InitOutput> {
  if (source === undefined) {
    if (isAudioWorkletScope()) {
      throw new Error(
        "loadTunelet() without a source cannot run in AudioWorkletGlobalScope because fetch() is unavailable. Compile the module on the main thread with loadTuneletModule(), or call initSync({ module }).",
      );
    }
    const module = await loadTuneletModule();
    return init({ module_or_path: module });
  }

  const input = await source;

  if (input instanceof URL && input.protocol === "file:") {
    return initSync({ module: (await readFileUrl(input)) as unknown as BufferSource });
  }

  if (
    typeof fetch !== "function" &&
    (typeof input === "string" ||
      (typeof URL === "function" && input instanceof URL) ||
      (typeof Request === "function" && input instanceof Request))
  ) {
    throw new Error(
      "fetch() is not available in this scope. Give a WebAssembly.Module or BufferSource instead.",
    );
  }

  return init({ module_or_path: input });
}

/** The default export is the same function as `loadTunelet`. */
export default loadTunelet;
