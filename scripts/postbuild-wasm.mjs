/**
 * This script deletes `pkg/.gitignore` and replaces ambient DOM and
 * `Symbol.dispose` declarations in `pkg/tunelet.d.ts` with portable
 * structural interfaces.
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const pkgDir = fileURLToPath(new URL("../pkg", import.meta.url));
const gitignorePath = path.join(pkgDir, ".gitignore");
const dtsPath = path.join(pkgDir, "tunelet.d.ts");

const TUNELET_DISPOSE_ANCHOR = [
  "export class Tunelet {",
  "    free(): void;",
  "    [Symbol.dispose](): void;",
].join("\n");

const TUNELET_DISPOSE_REPLACEMENT = [
  "export class Tunelet {",
  "    free(): void;",
].join("\n");

const INIT_INPUT_ANCHOR =
  "export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;";

const INIT_INPUT_REPLACEMENT = [
  "interface WasmRequest {",
  "    readonly method: string;",
  "    readonly url: string;",
  "    arrayBuffer(): Promise<ArrayBuffer>;",
  "}",
  "",
  "type WasmRequestInfo = string | WasmRequest;",
  "",
  "interface WasmUrl {",
  "    readonly href: string;",
  "    readonly protocol: string;",
  "}",
  "",
  "interface WasmResponse {",
  "    readonly ok: boolean;",
  "    readonly status: number;",
  "    readonly statusText: string;",
  "    readonly url: string;",
  "    arrayBuffer(): Promise<ArrayBuffer>;",
  "}",
  "",
  "interface WasmBufferView {",
  "    readonly buffer: ArrayBufferLike;",
  "    readonly byteLength: number;",
  "    readonly byteOffset: number;",
  "}",
  "",
  "type WasmBufferSource = ArrayBuffer | WasmBufferView;",
  "",
  "interface WasmModule {",
  "    readonly [Symbol.toStringTag]?: string | undefined;",
  "}",
  "",
  "interface WasmMemory {",
  "    readonly buffer: ArrayBuffer;",
  "    grow(delta: number): number;",
  "    toFixedLengthBuffer(): ArrayBuffer;",
  "    toResizableBuffer(): ArrayBuffer;",
  "}",
  "",
  "interface WasmTable {",
  "    readonly length: number;",
  "    get(index: number): any;",
  "    grow(delta: number, value?: any): number;",
  "    set(index: number, value?: any): void;",
  "}",
  "",
  "export type InitInput = WasmRequestInfo | WasmUrl | WasmResponse | WasmBufferSource | WasmModule;",
].join("\n");

const MEMORY_ANCHOR = "    readonly memory: WebAssembly.Memory;";
const MEMORY_REPLACEMENT = "    readonly memory: WasmMemory;";

const TABLE_ANCHOR = "    readonly __wbindgen_externrefs: WebAssembly.Table;";
const TABLE_REPLACEMENT = "    readonly __wbindgen_externrefs: WasmTable;";

const SYNC_INIT_ANCHOR =
  "export type SyncInitInput = BufferSource | WebAssembly.Module;";
const SYNC_INIT_REPLACEMENT =
  "export type SyncInitInput = WasmBufferSource | WasmModule;";

const REPLACEMENTS = [
  {
    name: "Tunelet[Symbol.dispose]",
    anchor: TUNELET_DISPOSE_ANCHOR,
    replacement: TUNELET_DISPOSE_REPLACEMENT,
  },
  {
    name: "InitInput",
    anchor: INIT_INPUT_ANCHOR,
    replacement: INIT_INPUT_REPLACEMENT,
  },
  {
    name: "InitOutput.memory",
    anchor: MEMORY_ANCHOR,
    replacement: MEMORY_REPLACEMENT,
  },
  {
    name: "InitOutput.__wbindgen_externrefs",
    anchor: TABLE_ANCHOR,
    replacement: TABLE_REPLACEMENT,
  },
  {
    name: "SyncInitInput",
    anchor: SYNC_INIT_ANCHOR,
    replacement: SYNC_INIT_REPLACEMENT,
  },
];

const FORBIDDEN_DECLARATION_PATTERNS = [
  { name: "Symbol.dispose", pattern: /\bSymbol\.dispose\b/ },
  { name: "RequestInfo", pattern: /\bRequestInfo\b/ },
  { name: "Response", pattern: /\bResponse\b/ },
  { name: "URL", pattern: /\bURL\b/ },
  { name: "BufferSource", pattern: /\bBufferSource\b/ },
  { name: "WebAssembly", pattern: /\bWebAssembly\b/ },
  { name: "declare global", pattern: /\bdeclare\s+global\b/ },
];

/** Replaces one exact anchor and throws if the anchor count is not one. */
function replaceExactAnchor(source, name, anchor, replacement) {
  const firstIndex = source.indexOf(anchor);
  if (firstIndex === -1) {
    throw new Error(
      `Expected wasm-bindgen anchor "${name}" was not found in pkg/tunelet.d.ts`,
    );
  }
  const secondIndex = source.indexOf(anchor, firstIndex + anchor.length);
  if (secondIndex !== -1) {
    throw new Error(
      `Expected wasm-bindgen anchor "${name}" appeared more than one time in pkg/tunelet.d.ts`,
    );
  }
  return (
    source.slice(0, firstIndex) +
    replacement +
    source.slice(firstIndex + anchor.length)
  );
}

/** Removes comments before a check for unguarded ambient identifiers. */
function stripComments(source) {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^(?!\s*\/\/\/\s*<reference).*\/\/.*$/gm, "");
}

/** Sanitizes a declaration text and returns the updated content. */
export function sanitizeDeclarations(rawSource) {
  const newline = rawSource.includes("\r\n") ? "\r\n" : "\n";
  let content = rawSource.replace(/\r\n/g, "\n");

  for (const item of REPLACEMENTS) {
    content = replaceExactAnchor(
      content,
      item.name,
      item.anchor,
      item.replacement,
    );
  }

  if (/\/\/\/\s*<reference\b/.test(content)) {
    throw new Error(
      "Triple-slash reference directives are not permitted in pkg/tunelet.d.ts",
    );
  }

  const codeOnly = stripComments(content);
  for (const check of FORBIDDEN_DECLARATION_PATTERNS) {
    if (check.pattern.test(codeOnly)) {
      throw new Error(
        `Unsanitized ambient type "${check.name}" stayed in pkg/tunelet.d.ts`,
      );
    }
  }

  return newline === "\r\n" ? content.replace(/\n/g, "\r\n") : content;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  fs.rmSync(gitignorePath, { force: true });
  if (!fs.existsSync(dtsPath)) {
    throw new Error(`Declaration file was not found at ${dtsPath}`);
  }
  const original = fs.readFileSync(dtsPath, "utf8");
  const sanitized = sanitizeDeclarations(original);
  fs.writeFileSync(dtsPath, sanitized, "utf8");
}
