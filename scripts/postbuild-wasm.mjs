/**
 * Removes `pkg/.gitignore` and replaces ambient `DOM`, `WebAssembly` and
 * `Symbol.dispose` declarations in `pkg/tunelet.d.ts`.
 */
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const pkgDir = fileURLToPath(new URL("../pkg", import.meta.url));
const gitignorePath = path.join(pkgDir, ".gitignore");
const declarationsPath = path.join(pkgDir, "tunelet.d.ts");

fs.rmSync(gitignorePath, { force: true });

const STRUCTURAL_WASM_TYPES = [
  "type WasmBufferSource = ArrayBuffer | ArrayBufferView;",
  "interface WasmUrl {",
  "    readonly href: string;",
  "    readonly protocol: string;",
  "}",
  "interface WasmRequest {",
  "    readonly url: string;",
  "    arrayBuffer(): Promise<ArrayBuffer>;",
  "}",
  "interface WasmResponse {",
  "    readonly ok: boolean;",
  "    readonly status: number;",
  "    readonly statusText: string;",
  "    readonly url: string;",
  "    arrayBuffer(): Promise<ArrayBuffer>;",
  "}",
  "interface WasmMemory {",
  "    readonly buffer: ArrayBuffer;",
  "    grow(delta: number): number;",
  "}",
  "interface WasmTable {",
  "    readonly length: number;",
  "    get(index: number): any;",
  "    grow(delta: number, value?: any): number;",
  "    set(index: number, value?: any): void;",
  "}",
  "type WasmModule = object;",
  "export type InitInput = string | WasmRequest | WasmUrl | WasmResponse | WasmBufferSource | WasmModule;",
].join("\n");

/** Replaces one required pattern in `source` and checks that it matched. */
function replaceRequired(source, pattern, replacement, label) {
  assert.match(source, pattern, `Expected ${label} in pkg/tunelet.d.ts`);
  return source.replace(pattern, replacement);
}

let declarations = fs.readFileSync(declarationsPath, "utf8");

declarations = replaceRequired(
  declarations,
  /^[ \t]*\[Symbol\.dispose\]\(\): void;\r?\n/m,
  "",
  "[Symbol.dispose](): void",
);
declarations = replaceRequired(
  declarations,
  /^export type InitInput = RequestInfo \| URL \| Response \| BufferSource \| WebAssembly\.Module;$/m,
  STRUCTURAL_WASM_TYPES,
  "InitInput declaration",
);
declarations = replaceRequired(
  declarations,
  /^([ \t]*readonly memory: )WebAssembly\.Memory;$/m,
  "$1WasmMemory;",
  "InitOutput.memory declaration",
);
declarations = replaceRequired(
  declarations,
  /^([ \t]*readonly __wbindgen_externrefs: )WebAssembly\.Table;$/m,
  "$1WasmTable;",
  "InitOutput.__wbindgen_externrefs declaration",
);
declarations = replaceRequired(
  declarations,
  /^export type SyncInitInput = BufferSource \| WebAssembly\.Module;$/m,
  "export type SyncInitInput = WasmBufferSource | WasmModule;",
  "SyncInitInput declaration",
);

const withoutComments = declarations
  .replace(/\/\*[\s\S]*?\*\//g, "")
  .replace(/\/\/.*$/gm, "");

for (const forbidden of [
  /Symbol\.dispose/,
  /\bWebAssembly\b/,
  /\bRequestInfo\b/,
  /\bBufferSource\b/,
  /\bResponse\b/,
  /\bURL\b/,
  /\bdeclare\s+global\b/,
  /\/\/\/\s*<reference\b/,
]) {
  assert.doesNotMatch(
    withoutComments,
    forbidden,
    `Unexpected ambient reference ${forbidden} in pkg/tunelet.d.ts`,
  );
}

fs.writeFileSync(declarationsPath, declarations, "utf8");
