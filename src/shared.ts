/**
 * Shared Wasm bindings and initialization state for the package entries.
 */
import init from "../pkg/tunelet.js";
import type { InitInput, InitOutput } from "../pkg/tunelet.js";

export { init };
export type { InitInput, InitOutput };

/** The shared initialization promise, which a failure clears. */
let initialization: Promise<InitOutput> | undefined;

/**
 * Runs one shared initialization attempt and clears the cached promise if the
 * attempt fails.
 */
export function loadOnce(
  source: InitInput | Promise<InitInput> | undefined,
  load: (source: InitInput | undefined) => Promise<InitOutput>,
): Promise<InitOutput> {
  initialization ??= resolveAndLoad(source, load).catch((error: unknown) => {
    initialization = undefined;
    throw error;
  });
  return initialization;
}

/** Awaits an explicit source, rejects `undefined` results and loads Wasm. */
async function resolveAndLoad(
  source: InitInput | Promise<InitInput> | undefined,
  load: (source: InitInput | undefined) => Promise<InitOutput>,
): Promise<InitOutput> {
  if (source === undefined) {
    return load(undefined);
  }
  const resolved = await source;
  if (resolved === undefined) {
    throw new TypeError("The Wasm source must not be undefined.");
  }
  return load(resolved);
}
