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

/** Checks if a value is a native `URL` or a cross-realm `URL` instance. */
export function isUrlLike(
  value: unknown,
): value is { readonly href: string; readonly protocol: string } {
  if (typeof URL === "function" && value instanceof URL) {
    return true;
  }
  if (
    typeof value !== "object" ||
    value === null ||
    ArrayBuffer.isView(value) ||
    value instanceof ArrayBuffer
  ) {
    return false;
  }
  const candidate = value as {
    readonly href?: unknown;
    readonly protocol?: unknown;
  };
  return (
    typeof candidate.href === "string" && typeof candidate.protocol === "string"
  );
}
