/**
 * The direct Wasm initialization entry point (`tunelet/wasm`).
 *
 * Use this subpath in workers or AudioWorklet processors when you initialize
 * the Wasm binary directly with `initSync` or the default initializer.
 */
import rawInit, {
  Tunelet as RawTunelet,
  defaultConfig as rawDefaultConfig,
  initSync as rawInitSync,
} from "../pkg/tunelet.js";
import type {
  ProcessorConfig,
  TuneletConstructor,
  TuneletInstance,
  WasmInitInput,
  WasmInitOutput,
  WasmSyncInitInput,
} from "./types.js";

export type * from "./types.js";

/** A stateful 16-bit PCM processor instance. */
export type Tunelet = TuneletInstance;

/** A stateful 16-bit PCM processor class. */
export const Tunelet: TuneletConstructor = RawTunelet;

/**
 * Returns a complete, independent default configuration for JavaScript.
 *
 * Change its fields and give it to `Tunelet` or to `setConfig`.
 */
export const defaultConfig: () => ProcessorConfig = rawDefaultConfig;

/**
 * Instantiates the Wasm module synchronously from raw bytes or a precompiled
 * module object.
 */
export function initSync(
  module: { readonly module: WasmSyncInitInput } | WasmSyncInitInput,
): WasmInitOutput {
  return (
    rawInitSync as (
      input: { readonly module: WasmSyncInitInput } | WasmSyncInitInput,
    ) => WasmInitOutput
  )(module);
}

/**
 * Initializes the Wasm module asynchronously from a URL, fetch object, raw
 * bytes or precompiled module object.
 */
export default function init(
  module_or_path?:
    | {
        readonly module_or_path?:
          | WasmInitInput
          | Promise<WasmInitInput>
          | undefined;
      }
    | WasmInitInput
    | Promise<WasmInitInput>,
): Promise<WasmInitOutput> {
  return (
    rawInit as (
      input?:
        | {
            readonly module_or_path?:
              | WasmInitInput
              | Promise<WasmInitInput>
              | undefined;
          }
        | WasmInitInput
        | Promise<WasmInitInput>,
    ) => Promise<WasmInitOutput>
  )(module_or_path);
}
