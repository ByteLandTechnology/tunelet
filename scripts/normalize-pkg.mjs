/**
 * This script removes `pkg/.gitignore` and makes `pkg/tunelet.d.ts`
 * portable across browser, worker and Node.js TypeScript configurations.
 */
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const DISPOSABLE_REFERENCE = '/// <reference lib="esnext.disposable" />';

const RAW_INIT_INPUT =
  "export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;";
const PORTABLE_INIT_INPUT =
  "export type InitInput = Request | string | URL | Response | ArrayBufferView | ArrayBuffer | WebAssembly.Module;";

const RAW_SYNC_INIT_INPUT =
  "export type SyncInitInput = BufferSource | WebAssembly.Module;";
const PORTABLE_SYNC_INIT_INPUT =
  "export type SyncInitInput = ArrayBufferView | ArrayBuffer | WebAssembly.Module;";

const AMBIENT_INTERFACES_BLOCK = [
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
].join("\n");

const repoRoot = fileURLToPath(new URL("..", import.meta.url));
const pkgDir = path.join(repoRoot, "pkg");
const gitignorePath = path.join(pkgDir, ".gitignore");
const declarationsPath = path.join(pkgDir, "tunelet.d.ts");

/** Remove the ignore file that wasm-pack writes into pkg/. */
fs.rmSync(gitignorePath, { force: true });

/** Rewrite the generated declarations into a self-contained module. */
let content = fs.readFileSync(declarationsPath, "utf8").replace(/\r\n/g, "\n");

if (!content.startsWith(`${DISPOSABLE_REFERENCE}\n`)) {
  content = `${DISPOSABLE_REFERENCE}\n${content}`;
}

if (content.includes(RAW_INIT_INPUT)) {
  content = content.replace(RAW_INIT_INPUT, PORTABLE_INIT_INPUT);
}

if (content.includes(RAW_SYNC_INIT_INPUT)) {
  content = content.replace(RAW_SYNC_INIT_INPUT, PORTABLE_SYNC_INIT_INPUT);
}

if (!content.includes(AMBIENT_INTERFACES_BLOCK.trim())) {
  content = `${content.trimEnd()}\n\n${AMBIENT_INTERFACES_BLOCK}`;
}

assert.ok(
  content.startsWith(`${DISPOSABLE_REFERENCE}\n`),
  "pkg/tunelet.d.ts must start with the esnext.disposable reference",
);
assert.ok(
  content.includes("[Symbol.dispose](): void;"),
  "pkg/tunelet.d.ts must declare [Symbol.dispose]()",
);
assert.ok(
  content.includes(PORTABLE_INIT_INPUT),
  "pkg/tunelet.d.ts must declare the portable InitInput union",
);
assert.ok(
  content.includes(PORTABLE_SYNC_INIT_INPUT),
  "pkg/tunelet.d.ts must declare the portable SyncInitInput union",
);
assert.ok(
  content.includes(AMBIENT_INTERFACES_BLOCK.trim()),
  "pkg/tunelet.d.ts must declare open-ended ambient interfaces",
);

/** Check that no non-portable ambient aliases remain in code lines. */
const codeOnly = content
  .replace(/\/\*[\s\S]*?\*\//g, "")
  .replace(/^\s*\/\/(?!\/).*$/gm, "");
assert.equal(
  /\b(?:RequestInfo|BufferSource)\b/.test(codeOnly),
  false,
  "pkg/tunelet.d.ts must not reference RequestInfo or BufferSource in types",
);

fs.writeFileSync(declarationsPath, content);
