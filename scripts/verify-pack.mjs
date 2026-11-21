/**
 * Builds the npm archive, checks its file list and imports the package in
 * Node.js.
 */
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { gunzipSync } from "node:zlib";

const EXPECTED_ARCHIVE_ENTRIES = [
  "package/LICENSE",
  "package/README.md",
  "package/dist/index.d.ts",
  "package/dist/index.js",
  "package/package.json",
  "package/pkg/tunelet.d.ts",
  "package/pkg/tunelet.js",
  "package/pkg/tunelet_bg.wasm",
];

/** Reads a null-terminated UTF-8 field from a tar header slice. */
function readHeaderText(slice) {
  const zeroIndex = slice.indexOf(0);
  const end = zeroIndex === -1 ? slice.length : zeroIndex;
  return slice.subarray(0, end).toString("utf8");
}

/** Reads the `path` record from a PAX header block. */
function readPaxPath(block) {
  const text = block.toString("utf8");
  let index = 0;
  while (index < text.length) {
    const spaceIndex = text.indexOf(" ", index);
    if (spaceIndex === -1) {
      break;
    }
    const recordLength = Number.parseInt(text.slice(index, spaceIndex), 10);
    if (!Number.isFinite(recordLength) || recordLength <= 0) {
      break;
    }
    const record = text.slice(spaceIndex + 1, index + recordLength - 1);
    if (record.startsWith("path=")) {
      return record.slice("path=".length);
    }
    index += recordLength;
  }
  return undefined;
}

/** Decompresses a `.tgz` archive and returns its regular file entries. */
function readTarEntries(tarballPath) {
  const tarBytes = gunzipSync(readFileSync(tarballPath));
  const entries = [];
  let offset = 0;
  let nextLongPath;
  let nextPaxPath;

  while (offset + 512 <= tarBytes.length) {
    const header = tarBytes.subarray(offset, offset + 512);
    // Stop at the first zero block that marks the end of the archive.
    if (header.every((byte) => byte === 0)) {
      break;
    }

    const sizeText = readHeaderText(header.subarray(124, 136)).trim();
    const size = sizeText.length === 0 ? 0 : Number.parseInt(sizeText, 8);
    assert.ok(
      Number.isInteger(size) && size >= 0,
      `Invalid tar entry size: ${sizeText}`,
    );

    const typeByte = header[156];
    const typeFlag = typeByte === 0 ? "0" : String.fromCharCode(typeByte);
    const name = readHeaderText(header.subarray(0, 100));
    const prefix = readHeaderText(header.subarray(345, 500));
    const headerPath = prefix.length > 0 ? `${prefix}/${name}` : name;

    const dataStart = offset + 512;
    const dataEnd = dataStart + size;
    assert.ok(dataEnd <= tarBytes.length, "Truncated tar archive.");
    const data = tarBytes.subarray(dataStart, dataEnd);
    offset = dataStart + Math.ceil(size / 512) * 512;

    if (typeFlag === "L") {
      nextLongPath = data.toString("utf8").replace(/\0+$/, "");
      continue;
    }
    if (typeFlag === "x") {
      // Read the path from the PAX header block.
      nextPaxPath = readPaxPath(data);
      continue;
    }
    if (typeFlag === "g") {
      continue;
    }

    const entryPath = nextPaxPath ?? nextLongPath ?? headerPath;
    nextPaxPath = undefined;
    nextLongPath = undefined;

    if (typeFlag === "0") {
      entries.push({ path: entryPath, data });
    }
  }

  return entries;
}

/** Runs `npm pack` into `packDir` from `repoRoot`. */
function runNpmPack(repoRoot, packDir) {
  if (process.platform === "win32") {
    execFileSync(
      "cmd.exe",
      ["/d", "/s", "/c", "npm.cmd", "pack", "--pack-destination", packDir],
      { cwd: repoRoot, stdio: "inherit" },
    );
    return;
  }
  execFileSync("npm", ["pack", "--pack-destination", packDir], {
    cwd: repoRoot,
    stdio: "inherit",
  });
}

const repoRoot = fileURLToPath(new URL("..", import.meta.url));
const packageMetadata = JSON.parse(
  readFileSync(join(repoRoot, "package.json"), "utf8"),
);
const packageName = packageMetadata.name;
assert.ok(
  typeof packageName === "string" && packageName.length > 0,
  "package.json must define a non-empty name.",
);

// Create a temporary directory and delete it in the finally block.
const tempDir = mkdtempSync(join(tmpdir(), "tunelet-pack-"));
try {
  const packDir = join(tempDir, "pack");
  const consumerDir = join(tempDir, "consumer");
  mkdirSync(packDir);
  mkdirSync(consumerDir);

  runNpmPack(repoRoot, packDir);

  const tarballs = readdirSync(packDir).filter((file) => file.endsWith(".tgz"));
  assert.equal(
    tarballs.length,
    1,
    `Expected one .tgz archive, found ${tarballs.length}.`,
  );

  const entries = readTarEntries(join(packDir, tarballs[0]));
  const actualPaths = entries.map((entry) => entry.path).sort();
  assert.deepEqual(actualPaths, EXPECTED_ARCHIVE_ENTRIES);

  // Install the archive files into node_modules under the package name.
  const installDir = join(
    consumerDir,
    "node_modules",
    ...packageName.split("/"),
  );
  for (const entry of entries) {
    assert.ok(entry.data.byteLength > 0, `Empty archive entry: ${entry.path}`);
    assert.ok(
      entry.path.startsWith("package/"),
      `Unexpected archive prefix: ${entry.path}`,
    );
    const relativePath = entry.path.slice("package/".length);
    const destination = join(installDir, ...relativePath.split("/"));
    mkdirSync(dirname(destination), { recursive: true });
    writeFileSync(destination, entry.data);
  }

  writeFileSync(
    join(consumerDir, "package.json"),
    JSON.stringify({ private: true, type: "module" }),
  );

  // Import the package from an isolated directory and construct a processor.
  const smokeScriptPath = join(consumerDir, "smoke.mjs");
  writeFileSync(
    smokeScriptPath,
    [
      'import assert from "node:assert/strict";',
      "",
      "const name = process.argv[2];",
      "const entry = await import(name);",
      "const wasm = await import(`${name}/wasm`);",
      "",
      'assert.equal(typeof entry.loadTunelet, "function");',
      "assert.equal(entry.default, entry.loadTunelet);",
      "assert.equal(entry.Tunelet, wasm.Tunelet);",
      "",
      "await entry.loadTunelet();",
      "const config = entry.defaultConfig();",
      "const processor = new entry.Tunelet(48000, 128, config);",
      "try {",
      "  assert.equal(processor.sampleRate, 48000);",
      "  assert.equal(processor.maxBlockFrames, 128);",
      "  const input = new Int16Array(128);",
      "  const left = new Int16Array(128);",
      "  const right = new Int16Array(128);",
      "  processor.process(input, left, right);",
      "} finally {",
      "  processor.free();",
      "}",
      "",
    ].join("\n"),
  );

  execFileSync(process.execPath, [smokeScriptPath, packageName], {
    cwd: consumerDir,
    stdio: "inherit",
  });
} finally {
  rmSync(tempDir, { recursive: true, force: true });
}
