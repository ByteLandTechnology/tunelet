/**
 * This script removes `pkg/.gitignore` and makes `pkg/tunelet.d.ts`
 * independent of the DOM TypeScript library.
 */
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const DISPOSABLE_DIRECTIVE = '/// <reference lib="esnext.disposable" />';

const RAW_INIT_INPUT =
  "export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;";

const NORMALIZED_INIT_INPUT = [
  "declare global {",
  "    interface Request {}",
  "    interface Response {}",
  "    interface URL {}",
  "    namespace WebAssembly {",
  "        interface Memory {}",
  "        interface Module {}",
  "        interface Table {}",
  "    }",
  "}",
  "",
  "export type InitInput = Request | string | URL | Response | ArrayBufferView | ArrayBuffer | WebAssembly.Module;",
].join("\n");

const RAW_SYNC_INIT_INPUT =
  "export type SyncInitInput = BufferSource | WebAssembly.Module;";

const NORMALIZED_SYNC_INIT_INPUT =
  "export type SyncInitInput = ArrayBufferView | ArrayBuffer | WebAssembly.Module;";

const pkgDir = fileURLToPath(new URL("../pkg", import.meta.url));
const gitignorePath = path.join(pkgDir, ".gitignore");
const declarationsPath = path.join(pkgDir, "tunelet.d.ts");

fs.rmSync(gitignorePath, { force: true });

let declarations = fs
  .readFileSync(declarationsPath, "utf8")
  .replaceAll("\r\n", "\n");

assert.ok(
  declarations.includes("[Symbol.dispose](): void;"),
  "Expected [Symbol.dispose](): void; in pkg/tunelet.d.ts",
);

if (!declarations.startsWith(`${DISPOSABLE_DIRECTIVE}\n`)) {
  declarations = `${DISPOSABLE_DIRECTIVE}\n${declarations}`;
}

if (declarations.includes(RAW_INIT_INPUT)) {
  declarations = declarations.replace(RAW_INIT_INPUT, NORMALIZED_INIT_INPUT);
}

if (declarations.includes(RAW_SYNC_INIT_INPUT)) {
  declarations = declarations.replace(
    RAW_SYNC_INIT_INPUT,
    NORMALIZED_SYNC_INIT_INPUT,
  );
}

declarations = declarations.replaceAll("{RequestInfo}", "{Request | string}");

assert.ok(
  declarations.startsWith(`${DISPOSABLE_DIRECTIVE}\n`),
  "Expected the esnext.disposable reference at the start of pkg/tunelet.d.ts",
);
assert.ok(
  declarations.includes(NORMALIZED_INIT_INPUT),
  "Expected portable InitInput and global interfaces in pkg/tunelet.d.ts",
);
assert.ok(
  declarations.includes(NORMALIZED_SYNC_INIT_INPUT),
  "Expected portable SyncInitInput in pkg/tunelet.d.ts",
);
assert.ok(
  !/\b(?:RequestInfo|BufferSource)\b/.test(declarations),
  "Unexpected DOM alias in pkg/tunelet.d.ts",
);

fs.writeFileSync(declarationsPath, declarations);
