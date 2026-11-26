/**
 * This script builds the package one time and packs `tunelet` and
 * `@bytelandtechnology/tunelet`. For each archive, it checks the fourteen
 * files, runtime Node.js behavior, HTTP Wasm loaders, strict TypeScript
 * profiles and the JSDoc comments.
 *
 * If you give `--ignore-scripts`, the script checks the pre-built `pkg/` and
 * `dist/` files without a build. If you give `--out-dir <dir>`, the script
 * writes both `.tgz` archives and `SHA256SUMS.txt` to `<dir>` after all checks
 * succeed.
 */
import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { normalizeJsDoc } from "./normalize-jsdoc.mjs";

const UNSCOPED_PACKAGE_NAME = "tunelet";
const SCOPED_PACKAGE_NAME = "@bytelandtechnology/tunelet";
const DISPOSABLE_LIB_REFERENCE = '/// <reference lib="esnext.disposable" />';

const EXPECTED_ARCHIVE_PATHS = [
  "package/LICENSE",
  "package/README.md",
  "package/dist/browser.d.ts",
  "package/dist/browser.js",
  "package/dist/index.d.ts",
  "package/dist/index.js",
  "package/dist/node.d.ts",
  "package/dist/node.js",
  "package/dist/shared.d.ts",
  "package/dist/shared.js",
  "package/package.json",
  "package/pkg/tunelet.d.ts",
  "package/pkg/tunelet.js",
  "package/pkg/tunelet_bg.wasm",
];

const PACKAGE_NAME_PATTERN =
  /^(?:@[a-z0-9-~][a-z0-9-._~]*\/)?[a-z0-9-~][a-z0-9-._~]*$/;

const repoRoot = fileURLToPath(new URL("..", import.meta.url));

/** If `filePath` is a regular file on disk, return `true`. */
function isRegularFile(filePath) {
  try {
    return fs.statSync(filePath).isFile();
  } catch {
    return false;
  }
}

/** Check that all expected source files exist in `repoRoot`. */
function verifyPrebuiltFiles() {
  for (const archivePath of EXPECTED_ARCHIVE_PATHS) {
    const relativeFile = archivePath.slice("package/".length);
    const fullPath = path.join(repoRoot, relativeFile);
    assert.ok(
      isRegularFile(fullPath),
      `Expected built file at ${relativeFile}`,
    );
  }
}

/** Read the optional `--ignore-scripts` and `--out-dir` flags from `process.argv`. */
function parseCliOptions(argv) {
  const { values, positionals } = parseArgs({
    args: argv,
    options: {
      "ignore-scripts": { type: "boolean", default: false },
      "out-dir": { type: "string" },
    },
    strict: true,
  });
  assert.equal(
    positionals.length,
    0,
    `Unexpected positional arguments: ${positionals.join(" ")}`,
  );
  const ignoreScripts = Boolean(values["ignore-scripts"]);
  const rawOutDir = values["out-dir"];
  if (rawOutDir === undefined) {
    return { ignoreScripts, outDir: undefined };
  }
  assert.ok(
    rawOutDir.trim().length > 0 && !rawOutDir.startsWith("-"),
    "--out-dir requires a directory path",
  );
  return { ignoreScripts, outDir: path.resolve(process.cwd(), rawOutDir) };
}

/** Read and check the manifest in `package.json`. */
function readPackageManifest() {
  const packageJsonPath = path.join(repoRoot, "package.json");
  const manifest = JSON.parse(fs.readFileSync(packageJsonPath, "utf8"));
  assert.equal(
    typeof manifest.name,
    "string",
    "package.json name must be a string",
  );
  assert.match(
    manifest.name,
    PACKAGE_NAME_PATTERN,
    `Invalid package name: ${manifest.name}`,
  );
  assert.equal(
    manifest.name,
    UNSCOPED_PACKAGE_NAME,
    `Expected package.json name to be ${UNSCOPED_PACKAGE_NAME}, found ${manifest.name}`,
  );
  assert.equal(
    typeof manifest.version,
    "string",
    "package.json version must be a string",
  );
  assert.ok(
    manifest.version.trim().length > 0,
    "package.json version must not be empty",
  );
  return manifest;
}

/** Return the `.tgz` file name that `npm pack` writes for `packageName`. */
function expectedArchiveFileName(packageName, version) {
  return `${packageName.replace(/^@/, "").replace(/\//g, "-")}-${version}.tgz`;
}

/** Copy the built package files into `stageDir` with `packageName`. */
function stageScopedPackage(stageDir, manifest, packageName) {
  assert.match(
    packageName,
    PACKAGE_NAME_PATTERN,
    `Invalid package name: ${packageName}`,
  );
  fs.mkdirSync(stageDir, { recursive: true });

  const manifestFiles = Array.isArray(manifest.files) ? manifest.files : [];
  const topLevelEntries = new Set(["LICENSE", "README.md"]);
  for (const entry of manifestFiles) {
    if (typeof entry === "string") {
      const firstSegment = entry.replace(/^\.\//, "").split("/")[0];
      if (firstSegment.length > 0) {
        topLevelEntries.add(firstSegment);
      }
    }
  }

  for (const entry of topLevelEntries) {
    const sourcePath = path.join(repoRoot, entry);
    if (fs.existsSync(sourcePath)) {
      fs.cpSync(sourcePath, path.join(stageDir, entry), { recursive: true });
    }
  }

  const stagedManifest = { ...manifest, name: packageName };
  fs.writeFileSync(
    path.join(stageDir, "package.json"),
    `${JSON.stringify(stagedManifest, null, 2)}\n`,
  );
}

/** Run `npm pack` without a shell and return the relative archive path. */
function packArchive(
  sourceDir,
  packDir,
  expectedFileName,
  { ignoreScripts = false } = {},
) {
  const packArgs = ["pack", "--pack-destination", packDir];
  if (ignoreScripts) {
    packArgs.push("--ignore-scripts");
  }

  const npmExecPath = process.env.npm_execpath;
  if (npmExecPath) {
    execFileSync(process.execPath, [npmExecPath, ...packArgs], {
      cwd: sourceDir,
      stdio: "inherit",
    });
  } else if (process.platform === "win32") {
    const npmCliPath = path.join(
      path.dirname(process.execPath),
      "node_modules",
      "npm",
      "bin",
      "npm-cli.js",
    );
    if (!fs.existsSync(npmCliPath)) {
      throw new Error(`npm-cli.js was not found at ${npmCliPath}`);
    }
    execFileSync(process.execPath, [npmCliPath, ...packArgs], {
      cwd: sourceDir,
      stdio: "inherit",
    });
  } else {
    execFileSync("npm", packArgs, {
      cwd: sourceDir,
      stdio: "inherit",
    });
  }

  const archives = fs
    .readdirSync(packDir)
    .filter((entry) => entry.endsWith(".tgz"));
  assert.equal(
    archives.length,
    1,
    `Expected one archive, found ${archives.length}`,
  );
  assert.equal(
    archives[0],
    expectedFileName,
    `Expected archive ${expectedFileName}, found ${archives[0]}`,
  );
  return path.posix.join("pack", archives[0]);
}

/** Check that the archive contains only the fourteen expected files. */
function verifyArchiveEntries(tempDir, relativeArchivePath) {
  const rawList = execFileSync("tar", ["-tzf", relativeArchivePath], {
    cwd: tempDir,
    encoding: "utf8",
  });
  const actualPaths = rawList
    .split(/\r?\n/)
    .map((line) => line.trim().replace(/^\.\//, ""))
    .filter((line) => line.length > 0 && !line.endsWith("/"))
    .sort();

  assert.deepEqual(actualPaths, [...EXPECTED_ARCHIVE_PATHS].sort());
}

/** Extract the archive into an isolated `node_modules` directory. */
function installArchive(
  tempDir,
  relativeArchivePath,
  consumerDir,
  packageName,
) {
  const extractDir = path.join(tempDir, "extract");
  fs.mkdirSync(extractDir);
  execFileSync("tar", ["-xzf", relativeArchivePath, "-C", "extract"], {
    cwd: tempDir,
  });

  const installedDir = path.join(
    consumerDir,
    "node_modules",
    ...packageName.split("/"),
  );
  fs.mkdirSync(path.dirname(installedDir), { recursive: true });
  fs.renameSync(path.join(extractDir, "package"), installedDir);

  for (const archivePath of EXPECTED_ARCHIVE_PATHS) {
    const relativeFile = archivePath.slice("package/".length);
    const fullPath = path.join(installedDir, relativeFile);
    const stat = fs.statSync(fullPath);
    assert.ok(stat.isFile(), `Expected file at ${relativeFile}`);
    assert.ok(stat.size > 0, `Expected non-empty file at ${relativeFile}`);
  }

  const packedMetadata = JSON.parse(
    fs.readFileSync(path.join(installedDir, "package.json"), "utf8"),
  );
  assert.equal(packedMetadata.name, packageName);
  assert.equal(packedMetadata.main, "./dist/index.js");
  assert.equal(packedMetadata.types, "./dist/index.d.ts");

  fs.writeFileSync(
    path.join(consumerDir, "package.json"),
    JSON.stringify({
      name: "tunelet-pack-consumer",
      private: true,
      type: "module",
    }),
  );

  return installedDir;
}

/** Remove comments from JavaScript or TypeScript source text. */
function stripComments(source) {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^(?!\s*\/\/\/\s*<reference).*\/\/.*$/gm, "");
}

/** Check browser bundle isolation and declaration parity on packed files. */
function verifyInstalledFiles(installedDir) {
  const browserJs = fs.readFileSync(
    path.join(installedDir, "dist", "browser.js"),
    "utf8",
  );
  const indexJs = fs.readFileSync(
    path.join(installedDir, "dist", "index.js"),
    "utf8",
  );
  const sharedJs = fs.readFileSync(
    path.join(installedDir, "dist", "shared.js"),
    "utf8",
  );
  assert.doesNotMatch(browserJs, /node:fs\/promises/);
  assert.doesNotMatch(browserJs, /\bnode:/);
  assert.doesNotMatch(sharedJs, /node:fs\/promises/);
  assert.doesNotMatch(sharedJs, /\bnode:/);
  assert.doesNotMatch(indexJs, /^\s*import\b[^;]*["']node:/m);

  const indexDts = fs.readFileSync(
    path.join(installedDir, "dist", "index.d.ts"),
    "utf8",
  );
  const nodeDts = fs.readFileSync(
    path.join(installedDir, "dist", "node.d.ts"),
    "utf8",
  );
  const browserDts = fs.readFileSync(
    path.join(installedDir, "dist", "browser.d.ts"),
    "utf8",
  );
  const sharedDts = fs.readFileSync(
    path.join(installedDir, "dist", "shared.d.ts"),
    "utf8",
  );
  const wasmDts = fs.readFileSync(
    path.join(installedDir, "pkg", "tunelet.d.ts"),
    "utf8",
  );

  const strippedIndexDts = stripComments(indexDts).trim();
  assert.equal(
    stripComments(nodeDts).trim(),
    strippedIndexDts,
    "dist/node.d.ts and dist/index.d.ts must expose identical signatures",
  );
  assert.equal(
    stripComments(browserDts).trim(),
    strippedIndexDts,
    "dist/browser.d.ts and dist/index.d.ts must expose identical signatures",
  );

  const normalizedWasmDts = wasmDts.replace(/\r\n/g, "\n");
  const wasmReferenceDirectives =
    normalizedWasmDts.match(/\/\/\/\s*<reference\b[^\n]*/g) ?? [];
  assert.deepEqual(wasmReferenceDirectives, [DISPOSABLE_LIB_REFERENCE]);
  assert.ok(
    normalizedWasmDts.startsWith(`${DISPOSABLE_LIB_REFERENCE}\n`),
    "pkg/tunelet.d.ts must start with /// <reference lib=\"esnext.disposable\" />",
  );
  assert.match(wasmDts, /\[Symbol\.dispose\]\(\): void;/);
  assert.match(wasmDts, /humanize\?: number \| undefined;/);
  assert.match(wasmDts, /flexTune\?: number \| undefined;/);
  assert.match(wasmDts, /voices\?: number \| undefined;/);
  assert.match(wasmDts, /toFixedLengthBuffer\(\): ArrayBuffer;/);
  assert.match(wasmDts, /toResizableBuffer\(\): ArrayBuffer;/);

  for (const [label, content] of [
    ["dist/index.d.ts", indexDts],
    ["dist/node.d.ts", nodeDts],
    ["dist/browser.d.ts", browserDts],
    ["dist/shared.d.ts", sharedDts],
  ]) {
    const directives =
      stripComments(content).match(/\/\/\/\s*<reference\b[^\n]*/g) ?? [];
    for (const directive of directives) {
      assert.equal(
        directive,
        DISPOSABLE_LIB_REFERENCE,
        `Unexpected triple-slash reference in ${label}: ${directive}`,
      );
    }
  }

  for (const [label, content] of [
    ["dist/index.d.ts", indexDts],
    ["dist/node.d.ts", nodeDts],
    ["dist/browser.d.ts", browserDts],
    ["dist/shared.d.ts", sharedDts],
    ["pkg/tunelet.d.ts", wasmDts],
  ]) {
    const code = stripComments(content);
    for (const forbidden of [
      /\bWebAssembly\b/,
      /\bRequestInfo\b/,
      /\bResponse\b/,
      /\bURL\b/,
      /\bBufferSource\b/,
      /\bdeclare\s+global\b/,
      /\bnode:/,
    ]) {
      assert.doesNotMatch(
        code,
        forbidden,
        `Unexpected ${forbidden} in ${label}`,
      );
    }
  }
}

/** Run a short Node.js module script in `consumerDir`. */
function runNodeCheck(consumerDir, filename, lines, args = []) {
  const scriptPath = path.join(consumerDir, filename);
  fs.writeFileSync(scriptPath, `${lines.join("\n")}\n`, "utf8");
  execFileSync(process.execPath, [scriptPath, ...args], {
    cwd: consumerDir,
    stdio: "inherit",
  });
}

/** Check Node.js exports, `dist/index.js`, Float32 views, disposal and bundles. */
function verifyRuntimeImport(consumerDir, installedDir, packageName) {
  runNodeCheck(
    consumerDir,
    "check-default.mjs",
    [
      'import assert from "node:assert/strict";',
      'import fs from "node:fs";',
      'import { createRequire } from "node:module";',
      'import { fileURLToPath, pathToFileURL } from "node:url";',
      "",
      "const packageName = process.argv[2];",
      "const installedDir = process.argv[3];",
      "const require = createRequire(import.meta.url);",
      "",
      "const rootResolved = import.meta.resolve(packageName);",
      "const wasmJsResolved = import.meta.resolve(`${packageName}/wasm`);",
      "const pkgWasmResolved = import.meta.resolve(`${packageName}/pkg/tunelet_bg.wasm`);",
      "const shortWasmResolved = import.meta.resolve(`${packageName}/tunelet_bg.wasm`);",
      'assert.match(rootResolved, /\\/dist\\/node\\.js$/);',
      'assert.match(wasmJsResolved, /\\/pkg\\/tunelet\\.js$/);',
      'assert.match(pkgWasmResolved, /^file:.*\\/pkg\\/tunelet_bg\\.wasm$/);',
      "assert.equal(shortWasmResolved, pkgWasmResolved);",
      "const wasmFilePath = fileURLToPath(pkgWasmResolved);",
      "assert.ok(fs.statSync(wasmFilePath).isFile());",
      "assert.equal(require.resolve(`${packageName}/pkg/tunelet_bg.wasm`), wasmFilePath);",
      "assert.equal(require.resolve(`${packageName}/tunelet_bg.wasm`), wasmFilePath);",
      "assert.match(require.resolve(packageName), /dist[\\\\/]node\\.js$/);",
      "",
      "const rootModule = await import(packageName);",
      "const indexModule = await import(pathToFileURL(`${installedDir}/dist/index.js`).href);",
      "const browserModule = await import(pathToFileURL(`${installedDir}/dist/browser.js`).href);",
      "const wasmModule = await import(`${packageName}/wasm`);",
      "",
      "assert.deepEqual(Object.keys(rootModule).sort(), Object.keys(indexModule).sort());",
      "assert.deepEqual(Object.keys(rootModule).sort(), Object.keys(browserModule).sort());",
      'assert.equal(typeof rootModule.loadTunelet, "function");',
      "assert.equal(rootModule.default, rootModule.loadTunelet);",
      "assert.equal(indexModule.default, indexModule.loadTunelet);",
      "assert.equal(browserModule.default, browserModule.loadTunelet);",
      'assert.equal(typeof rootModule.initSync, "function");',
      'assert.equal(typeof rootModule.Tunelet, "function");',
      'assert.equal(typeof rootModule.TuneletAudioViews, "function");',
      "assert.equal(rootModule.TuneletAudioViews, indexModule.TuneletAudioViews);",
      "assert.equal(rootModule.TuneletAudioViews, browserModule.TuneletAudioViews);",
      'assert.equal(typeof rootModule.defaultConfig, "function");',
      'assert.equal(typeof wasmModule.default, "function");',
      'assert.equal(typeof wasmModule.initSync, "function");',
      "",
      "const wasmExports = await rootModule.loadTunelet();",
      "assert.ok(wasmExports);",
      "assert.ok(wasmExports.memory instanceof WebAssembly.Memory);",
      "",
      "const baseConfig = rootModule.defaultConfig();",
      "assert.equal(baseConfig.resynthesis.harmony.voices, 4);",
      "",
      "const constructConfig = {",
      "  ...baseConfig,",
      "  resynthesis: {",
      "    ...baseConfig.resynthesis,",
      "    correction: {",
      "      ...baseConfig.resynthesis.correction,",
      "      humanize: undefined,",
      "      flexTune: undefined,",
      "    },",
      "    harmony: {",
      "      ...baseConfig.resynthesis.harmony,",
      "      voices: undefined,",
      "    },",
      "  },",
      "};",
      "const processor = new rootModule.Tunelet(48000, 128, constructConfig);",
      "try {",
      "  assert.equal(processor.config().resynthesis.correction.humanize, 0);",
      "  assert.equal(processor.config().resynthesis.correction.flexTune, 0);",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 4);",
      '  assert.equal(typeof processor.controlActivity().choirVoices, "boolean");',
      "",
      "  const twoVoiceConfig = rootModule.defaultConfig();",
      "  twoVoiceConfig.resynthesis.harmony.voices = 2;",
      "  processor.setConfig(twoVoiceConfig);",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 2);",
      "",
      "  processor.setConfig(constructConfig);",
      "  assert.equal(processor.config().resynthesis.correction.humanize, 0);",
      "  assert.equal(processor.config().resynthesis.correction.flexTune, 0);",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 4);",
      "",
      "  const oneVoiceConfig = rootModule.defaultConfig();",
      "  oneVoiceConfig.resynthesis.harmony.voices = 1;",
      "  processor.setConfig(oneVoiceConfig);",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 1);",
      "",
      "  const tunedCorrectionConfig = processor.config();",
      "  tunedCorrectionConfig.resynthesis.correction.humanize = 0.5;",
      "  tunedCorrectionConfig.resynthesis.correction.flexTune = 0.25;",
      "  processor.setConfig(tunedCorrectionConfig);",
      "  assert.equal(processor.config().resynthesis.correction.humanize, 0.5);",
      "  assert.equal(processor.config().resynthesis.correction.flexTune, 0.25);",
      "",
      "  const omittedCorrectionConfig = processor.config();",
      "  delete omittedCorrectionConfig.resynthesis.correction.humanize;",
      "  delete omittedCorrectionConfig.resynthesis.correction.flexTune;",
      "  processor.setConfig(omittedCorrectionConfig);",
      "  assert.equal(processor.config().resynthesis.correction.humanize, 0);",
      "  assert.equal(processor.config().resynthesis.correction.flexTune, 0);",
      "",
      "  const mono = new Int16Array(128);",
      "  const eventResynthesis = processor.config().resynthesis;",
      "  eventResynthesis.correction.humanize = undefined;",
      "  eventResynthesis.correction.flexTune = undefined;",
      "  eventResynthesis.harmony.voices = undefined;",
      "  processor.processMono(mono, [",
      '    { offset: 0, event: { type: "resynthesis", value: eventResynthesis } },',
      "  ]);",
      "  assert.equal(processor.config().resynthesis.correction.humanize, 0);",
      "  assert.equal(processor.config().resynthesis.correction.flexTune, 0);",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 4);",
      "",
      "  processor.setConfig(twoVoiceConfig);",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 2);",
      "",
      "  const input = new Int16Array(128);",
      "  const left = new Int16Array(128);",
      "  const right = new Int16Array(128);",
      "  processor.process(input, left, right, [",
      "    {",
      "      offset: 0,",
      "      event: {",
      '        type: "configure",',
      "        value: constructConfig,",
      "      },",
      "    },",
      "  ]);",
      "  assert.equal(processor.config().resynthesis.correction.humanize, 0);",
      "  assert.equal(processor.config().resynthesis.correction.flexTune, 0);",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 4);",
      "} finally {",
      "  processor.free();",
      "}",
      "",
      "const config = rootModule.defaultConfig();",
      "config.resynthesis.formant.enabled = true;",
      "config.reverb.enabled = true;",
      "const pcmProcessor = new rootModule.Tunelet(48000, 128, config);",
      "const copiedF32Processor = new rootModule.Tunelet(48000, 128, config);",
      "const f32Processor = new rootModule.Tunelet(48000, 128, config);",
      "const views = new rootModule.TuneletAudioViews(wasmExports.memory, f32Processor);",
      "try {",
      "  assert.equal(views.maxBlockFrames, 128);",
      "  assert.equal(views.disposed, false);",
      "  const initPtrs = [",
      "    f32Processor.inputF32Ptr,",
      "    f32Processor.leftF32Ptr,",
      "    f32Processor.rightF32Ptr,",
      "  ];",
      "  assert.equal(initPtrs[0] % 4, 0);",
      "  assert.equal(initPtrs[1] % 4, 0);",
      "  assert.equal(initPtrs[2] % 4, 0);",
      "  assert.equal(views.input.length, 128);",
      "  assert.equal(views.left.length, 128);",
      "  assert.equal(views.right.length, 128);",
      "  assert.equal(views.input, views.input);",
      "",
      "  const pcmInput = new Int16Array(128);",
      "  const pcmLeft = new Int16Array(128);",
      "  const pcmRight = new Int16Array(128);",
      "  const inputF32 = new Float32Array(128);",
      "  const leftF32 = new Float32Array(128);",
      "  const rightF32 = new Float32Array(128);",
      "  for (let block = 0; block < 20; block++) {",
      "    const f32In = views.input;",
      "    for (let i = 0; i < 128; i++) {",
      "      const sample = Math.fround(0.6 * Math.sin(((block * 128 + i) * 2 * Math.PI * 220) / 48000));",
      "      f32In[i] = sample;",
      "      inputF32[i] = sample;",
      "      pcmInput[i] = Math.round(Math.max(-1, Math.min(1, sample)) * 32767);",
      "    }",
      "    pcmProcessor.process(pcmInput, pcmLeft, pcmRight);",
      "    copiedF32Processor.processF32(inputF32, leftF32, rightF32);",
      "    f32Processor.processF32InPlace(128);",
      "    const f32Left = views.left;",
      "    const f32Right = views.right;",
      "    for (let i = 0; i < 128; i++) {",
      "      const expectedL = Math.fround(pcmLeft[i] / 32768);",
      "      const expectedR = Math.fround(pcmRight[i] / 32768);",
      "      assert.equal(leftF32[i], expectedL);",
      "      assert.equal(rightF32[i], expectedR);",
      "      assert.equal(f32Left[i], expectedL);",
      "      assert.equal(f32Right[i], expectedR);",
      "    }",
      "  }",
      "",
      "  const beforeBuffer = wasmExports.memory.buffer;",
      "  const standby = new rootModule.Tunelet(48000, 2048, config);",
      "  try {",
      "    const envIn = new Float32Array(262144);",
      "    const envOut = new Float32Array(262144);",
      "    f32Processor.formantEnvelope(12000, envIn, envOut);",
      "  } finally {",
      "    standby.free();",
      "  }",
      "  assert.notEqual(wasmExports.memory.buffer, beforeBuffer);",
      "",
      "  const reboundInput = views.input;",
      "  reboundInput.fill(0.25, 0, 64);",
      "  inputF32.fill(0.25, 0, 64);",
      "  const gainEvents = [{ offset: 0, event: { type: \"outputGain\", value: 0.75 } }];",
      "  f32Processor.processF32InPlace(64, gainEvents);",
      "  copiedF32Processor.processF32(",
      "    inputF32.subarray(0, 64),",
      "    leftF32.subarray(0, 64),",
      "    rightF32.subarray(0, 64),",
      "    gainEvents,",
      "  );",
      "  pcmInput.fill(Math.round(Math.fround(0.25) * 32767), 0, 64);",
      "  pcmProcessor.process(",
      "    pcmInput.subarray(0, 64),",
      "    pcmLeft.subarray(0, 64),",
      "    pcmRight.subarray(0, 64),",
      "    gainEvents,",
      "  );",
      "  for (let i = 0; i < 64; i++) {",
      "    assert.equal(views.left[i], Math.fround(pcmLeft[i] / 32768));",
      "    assert.equal(views.right[i], Math.fround(pcmRight[i] / 32768));",
      "    assert.equal(leftF32[i], views.left[i]);",
      "    assert.equal(rightF32[i], views.right[i]);",
      "  }",
      "  assert.deepEqual(",
      "    [f32Processor.inputF32Ptr, f32Processor.leftF32Ptr, f32Processor.rightF32Ptr],",
      "    initPtrs,",
      "  );",
      "",
      "  const framesBeforeError = f32Processor.feedback().processedFrames;",
      "  const savedLeft0 = views.left[0];",
      "  assert.throws(() => f32Processor.processF32InPlace(129));",
      "  assert.throws(() => f32Processor.processF32InPlace(-1));",
      "  assert.throws(() => f32Processor.processF32InPlace(1.5));",
      "  assert.throws(() =>",
      "    f32Processor.processF32InPlace(64, [{ offset: 64, event: { type: \"reset\" } }]),",
      "  );",
      "  assert.equal(f32Processor.feedback().processedFrames, framesBeforeError);",
      "  assert.equal(views.left[0], savedLeft0);",
      "",
      "  pcmProcessor.beginDrain();",
      "  copiedF32Processor.beginDrain();",
      "  f32Processor.beginDrain();",
      "  const pcmCount = pcmProcessor.drain(pcmLeft, pcmRight);",
      "  const copiedCount = copiedF32Processor.drainF32(leftF32, rightF32);",
      "  const f32Count = f32Processor.drainF32InPlace(128);",
      "  assert.equal(copiedCount, pcmCount);",
      "  assert.equal(f32Count, pcmCount);",
      "  for (let i = 0; i < 128; i++) {",
      "    assert.equal(views.left[i], Math.fround(pcmLeft[i] / 32768));",
      "    assert.equal(views.right[i], Math.fround(pcmRight[i] / 32768));",
      "    assert.equal(leftF32[i], views.left[i]);",
      "    assert.equal(rightF32[i], views.right[i]);",
      "  }",
      "",
      "  const monoPcm = new rootModule.Tunelet(48000, 64, config);",
      "  const monoCopiedF32 = new rootModule.Tunelet(48000, 64, config);",
      "  const monoF32 = new rootModule.Tunelet(48000, 64, config);",
      "  const monoViews = new rootModule.TuneletAudioViews(wasmExports.memory, monoF32);",
      "  try {",
      "    const monoBuf = new Int16Array(64);",
      "    const monoCopiedBuf = new Float32Array(64);",
      "    for (let i = 0; i < 64; i++) {",
      "      monoViews.input[i] = 0.5;",
      "      monoCopiedBuf[i] = 0.5;",
      "      monoBuf[i] = Math.round(0.5 * 32767);",
      "    }",
      "    monoPcm.processMono(monoBuf);",
      "    monoCopiedF32.processMonoF32(monoCopiedBuf);",
      "    monoF32.processMonoF32InPlace(64);",
      "    for (let i = 0; i < 64; i++) {",
      "      const expected = Math.fround(monoBuf[i] / 32768);",
      "      assert.equal(monoCopiedBuf[i], expected);",
      "      assert.equal(monoViews.input[i], expected);",
      "    }",
      "    monoPcm.beginDrain();",
      "    monoCopiedF32.beginDrain();",
      "    monoF32.beginDrain();",
      "    const expectedDrain = monoPcm.drainMono(monoBuf);",
      "    assert.equal(monoCopiedF32.drainMonoF32(monoCopiedBuf), expectedDrain);",
      "    assert.equal(monoF32.drainMonoF32InPlace(64), expectedDrain);",
      "    for (let i = 0; i < 64; i++) {",
      "      const expected = Math.fround(monoBuf[i] / 32768);",
      "      assert.equal(monoCopiedBuf[i], expected);",
      "      assert.equal(monoViews.input[i], expected);",
      "    }",
      "  } finally {",
      "    monoViews.dispose();",
      "    monoPcm.free();",
      "    monoCopiedF32.free();",
      "    monoF32.free();",
      "  }",
      "} finally {",
      "  views.dispose();",
      "  assert.equal(views.disposed, true);",
      "  assert.throws(() => views.input);",
      "  assert.throws(() => views.left);",
      "  assert.throws(() => views.right);",
      "  assert.throws(() => views.refresh());",
      "  pcmProcessor.free();",
      "  copiedF32Processor.free();",
      "  f32Processor.free();",
      "}",
      "",
      'assert.equal(typeof Symbol.dispose, "symbol");',
      "const disposableProcessor = new rootModule.Tunelet(48000, 128);",
      "const disposableViews = new rootModule.TuneletAudioViews(wasmExports.memory, disposableProcessor);",
      'assert.equal(typeof disposableViews[Symbol.dispose], "function");',
      "disposableViews[Symbol.dispose]();",
      "assert.equal(disposableViews.disposed, true);",
      'assert.equal(typeof disposableProcessor[Symbol.dispose], "function");',
      "disposableProcessor[Symbol.dispose]();",
    ],
    [packageName, installedDir],
  );

  runNodeCheck(
    consumerDir,
    "check-index-entry.mjs",
    [
      'import assert from "node:assert/strict";',
      'import { pathToFileURL } from "node:url";',
      "",
      "const installedDir = process.argv[2];",
      "const indexModule = await import(pathToFileURL(`${installedDir}/dist/index.js`).href);",
      "const wasmExports = await indexModule.loadTunelet();",
      "assert.ok(wasmExports);",
      "const processor = new indexModule.Tunelet(48000, 64);",
      "const views = new indexModule.TuneletAudioViews(wasmExports.memory, processor);",
      "assert.equal(views.maxBlockFrames, 64);",
      'assert.equal(typeof views[Symbol.dispose], "function");',
      "views[Symbol.dispose]();",
      "assert.equal(processor.sampleRate, 48000);",
      'assert.equal(typeof processor[Symbol.dispose], "function");',
      "processor[Symbol.dispose]();",
    ],
    [installedDir],
  );

  runNodeCheck(
    consumerDir,
    "check-index-file-string.mjs",
    [
      'import assert from "node:assert/strict";',
      'import { pathToFileURL } from "node:url";',
      "",
      "const packageName = process.argv[2];",
      "const installedDir = process.argv[3];",
      "const indexModule = await import(pathToFileURL(`${installedDir}/dist/index.js`).href);",
      'const absentUrl = new URL("./absent-tunelet.wasm", import.meta.url).href;',
      "await assert.rejects(() => indexModule.loadTunelet(absentUrl));",
      "const fileString = import.meta.resolve(`${packageName}/pkg/tunelet_bg.wasm`);",
      "await indexModule.loadTunelet(fileString);",
      "assert.equal(indexModule.defaultConfig().resynthesis.harmony.voices, 4);",
    ],
    [packageName, installedDir],
  );

  runNodeCheck(
    consumerDir,
    "check-file-string.mjs",
    [
      'import assert from "node:assert/strict";',
      "",
      "const packageName = process.argv[2];",
      "const rootModule = await import(packageName);",
      'const absentUrl = new URL("./absent-tunelet.wasm", import.meta.url).href;',
      "await assert.rejects(() => rootModule.loadTunelet(absentUrl));",
      "",
      "const fileString = import.meta.resolve(`${packageName}/pkg/tunelet_bg.wasm`);",
      'assert.match(fileString, /^file:/);',
      "await rootModule.loadTunelet(fileString);",
      "const processor = new rootModule.Tunelet(48000, 64);",
      "try {",
      "  assert.equal(processor.sampleRate, 48000);",
      "} finally {",
      "  processor.free();",
      "}",
    ],
    [packageName],
  );

  runNodeCheck(
    consumerDir,
    "check-cross-realm-url.mjs",
    [
      'import assert from "node:assert/strict";',
      "",
      "const packageName = process.argv[2];",
      "const rootModule = await import(packageName);",
      "const href = import.meta.resolve(`${packageName}/tunelet_bg.wasm`);",
      'const crossRealmUrl = { href, protocol: "file:" };',
      "assert.equal(crossRealmUrl instanceof URL, false);",
      "await rootModule.loadTunelet(crossRealmUrl);",
      "assert.equal(rootModule.defaultConfig().resynthesis.harmony.voices, 4);",
    ],
    [packageName],
  );

  runNodeCheck(
    consumerDir,
    "check-wasm-init.mjs",
    [
      'import assert from "node:assert/strict";',
      'import fs from "node:fs";',
      'import { fileURLToPath } from "node:url";',
      "",
      "const packageName = process.argv[2];",
      "const wasmBytes = fs.readFileSync(",
      "  fileURLToPath(import.meta.resolve(`${packageName}/pkg/tunelet_bg.wasm`)),",
      ");",
      "const wasmModule = await import(`${packageName}/wasm`);",
      "const exportsFromInit = await wasmModule.default({",
      "  module_or_path: wasmBytes,",
      "});",
      "assert.ok(exportsFromInit);",
      "const processor = new wasmModule.Tunelet(48000, 128);",
      "try {",
      "  assert.equal(processor.sampleRate, 48000);",
      "} finally {",
      "  processor.free();",
      "}",
    ],
    [packageName],
  );

  runNodeCheck(
    consumerDir,
    "check-init-sync.mjs",
    [
      'import assert from "node:assert/strict";',
      'import fs from "node:fs";',
      'import { fileURLToPath } from "node:url";',
      "",
      "const packageName = process.argv[2];",
      "const wasmBytes = fs.readFileSync(",
      "  fileURLToPath(import.meta.resolve(`${packageName}/tunelet_bg.wasm`)),",
      ");",
      "const { initSync, Tunelet } = await import(packageName);",
      "const exportsFromSync = initSync({ module: wasmBytes });",
      "assert.ok(exportsFromSync);",
      "const processor = new Tunelet(48000, 128);",
      "try {",
      "  assert.equal(processor.sampleRate, 48000);",
      "} finally {",
      "  processor.free();",
      "}",
    ],
    [packageName],
  );

  runNodeCheck(
    consumerDir,
    "check-wasm-init-sync.mjs",
    [
      'import assert from "node:assert/strict";',
      'import fs from "node:fs";',
      'import { fileURLToPath } from "node:url";',
      "",
      "const packageName = process.argv[2];",
      "const wasmBytes = fs.readFileSync(",
      "  fileURLToPath(import.meta.resolve(`${packageName}/pkg/tunelet_bg.wasm`)),",
      ");",
      "const wasmModule = await import(`${packageName}/wasm`);",
      "const exportsFromSync = wasmModule.initSync({ module: wasmBytes });",
      "assert.ok(exportsFromSync);",
      "const processor = new wasmModule.Tunelet(48000, 128);",
      "try {",
      "  assert.equal(processor.sampleRate, 48000);",
      "} finally {",
      "  processor.free();",
      "}",
    ],
    [packageName],
  );

  const relocatedDir = path.join(consumerDir, "relocated");
  fs.mkdirSync(path.join(relocatedDir, "dist"), { recursive: true });
  fs.mkdirSync(path.join(relocatedDir, "pkg"), { recursive: true });
  for (const file of ["node.js", "shared.js"]) {
    fs.copyFileSync(
      path.join(installedDir, "dist", file),
      path.join(relocatedDir, "dist", file),
    );
  }
  fs.copyFileSync(
    path.join(installedDir, "pkg", "tunelet.js"),
    path.join(relocatedDir, "pkg", "tunelet.js"),
  );

  const flatBundlePath = path.join(consumerDir, "flat-node-bundle.mjs");
  const wasmGlue = fs
    .readFileSync(path.join(installedDir, "pkg", "tunelet.js"), "utf8")
    .replace(
      "export { initSync, __wbg_init as default };",
      "const init = __wbg_init;\nexport { initSync };",
    );
  const sharedGlue = fs
    .readFileSync(path.join(installedDir, "dist", "shared.js"), "utf8")
    .replace(/^import init from "\.\.\/pkg\/tunelet\.js";\r?\n/m, "")
    .replace(/^export \{ init \};\r?\n/m, "")
    .replace(/^export function loadOnce\(/m, "function loadOnce(")
    .replace(/^export function isUrlLike\(/m, "function isUrlLike(");
  const nodeGlue = fs
    .readFileSync(path.join(installedDir, "dist", "node.js"), "utf8")
    .replace(
      /^import \{ init, isUrlLike, loadOnce, TuneletAudioViews \} from "\.\/shared\.js";\r?\n/m,
      "",
    )
    .replace(/^export \* from "\.\.\/pkg\/tunelet\.js";\r?\n/m, "")
    .replace(/^export \{ TuneletAudioViews \};\r?\n/m, "");
  fs.writeFileSync(
    flatBundlePath,
    `${wasmGlue}\n${sharedGlue}\n${nodeGlue}\n`,
    "utf8",
  );

  runNodeCheck(consumerDir, "check-flat-bundle.mjs", [
    'import assert from "node:assert/strict";',
    'import * as relocated from "./relocated/dist/node.js";',
    "",
    "await relocated.loadTunelet();",
    "assert.equal(relocated.defaultConfig().resynthesis.harmony.voices, 4);",
  ]);

  runNodeCheck(consumerDir, "check-inlined-flat-bundle.mjs", [
    'import assert from "node:assert/strict";',
    'import * as flatBundle from "./flat-node-bundle.mjs";',
    "",
    "const wasmExports = await flatBundle.loadTunelet();",
    "const processor = new flatBundle.Tunelet(48000, 64);",
    "const views = new flatBundle.TuneletAudioViews(wasmExports.memory, processor);",
    "try {",
    "  views.input.fill(0);",
    "  processor.processMonoF32InPlace(64);",
    "  const audio = new Int16Array(64);",
    "  processor.processMono(audio);",
    "} finally {",
    "  views.dispose();",
    "  processor.free();",
    "}",
  ]);

  const corruptDir = path.join(consumerDir, "corrupt");
  fs.mkdirSync(path.join(corruptDir, "dist"), { recursive: true });
  fs.mkdirSync(path.join(corruptDir, "pkg"), { recursive: true });
  for (const file of ["node.js", "shared.js"]) {
    fs.copyFileSync(
      path.join(installedDir, "dist", file),
      path.join(corruptDir, "dist", file),
    );
  }
  fs.copyFileSync(
    path.join(installedDir, "pkg", "tunelet.js"),
    path.join(corruptDir, "pkg", "tunelet.js"),
  );
  fs.writeFileSync(
    path.join(corruptDir, "pkg", "tunelet_bg.wasm"),
    Uint8Array.of(0, 97, 115, 109, 99),
  );

  runNodeCheck(consumerDir, "check-corrupt-wasm.mjs", [
    'import assert from "node:assert/strict";',
    'import * as corrupt from "./corrupt/dist/node.js";',
    "",
    "await assert.rejects(() => corrupt.loadTunelet(), WebAssembly.CompileError);",
  ]);
}

/** Extract JSDoc comment blocks from a file and strip their comment markers. */
function extractJsDocBlocks(source) {
  const normalizedSource = source.replace(/\r\n/g, "\n");
  return [...normalizedSource.matchAll(/\/\*\*[\s\S]*?\*\//g)].map((match) => {
    const block = match[0];
    if (!block.includes("\n")) {
      return block.slice(3, -2).trim();
    }
    return block
      .split("\n")
      .slice(1, -1)
      .map((line) => line.replace(/^[ \t]*\*(?: |$)/, ""))
      .join("\n");
  });
}

/** Extract the JSDoc block immediately before a named member or export. */
function extractMemberJsDoc(source, pattern, label) {
  const match = pattern.exec(source);
  assert.ok(match, `Expected JSDoc block for ${label}`);
  return extractJsDocBlocks(match[1])[0];
}

/** Check that packed JSDoc comments contain clean TypeScript documentation. */
function verifyNormalizedJsDoc(installedDir) {
  const filesToCheck = [
    path.join(installedDir, "dist", "browser.d.ts"),
    path.join(installedDir, "dist", "browser.js"),
    path.join(installedDir, "dist", "index.d.ts"),
    path.join(installedDir, "dist", "index.js"),
    path.join(installedDir, "dist", "node.d.ts"),
    path.join(installedDir, "dist", "node.js"),
    path.join(installedDir, "dist", "shared.d.ts"),
    path.join(installedDir, "dist", "shared.js"),
    path.join(installedDir, "pkg", "tunelet.d.ts"),
    path.join(installedDir, "pkg", "tunelet.js"),
  ];

  for (const filePath of filesToCheck) {
    const content = fs.readFileSync(filePath, "utf8");
    const blocks = extractJsDocBlocks(content);
    assert.ok(blocks.length > 0, `Expected JSDoc blocks in ${filePath}`);
    for (const text of blocks) {
      assert.doesNotMatch(
        text,
        /(?:^|\n)#+\s/,
        `Unexpected Markdown heading in ${filePath}`,
      );
      assert.ok(
        !text.includes("::"),
        `Unexpected Rust path separator in ${filePath}`,
      );
      assert.ok(
        !text.includes("..="),
        `Unexpected Rust inclusive range in ${filePath}`,
      );
      assert.doesNotMatch(
        text,
        /\b(?:None|NONE)\b/,
        `Unexpected Rust None literal in ${filePath}`,
      );
      assert.doesNotMatch(
        text,
        /\[`[^`]+`\]/,
        `Unexpected Rust intra-doc link in ${filePath}`,
      );
      assert.ok(
        !text.includes("`Default`"),
        `Unexpected Rust Default trait reference in ${filePath}`,
      );
      assert.doesNotMatch(
        text,
        /`(?:max_frequency_hz|min_frequency_hz|stereo_output|retune_ms|attack_ms|gain_db|ceiling_db)`/,
        `Unexpected snake_case config field in ${filePath}`,
      );
    }
  }

  const dtsPath = path.join(installedDir, "pkg", "tunelet.d.ts");
  const jsPath = path.join(installedDir, "pkg", "tunelet.js");
  const dts = fs.readFileSync(dtsPath, "utf8");
  const js = fs.readFileSync(jsPath, "utf8");
  const disposableDirective = '/// <reference lib="esnext.disposable" />\n';
  assert.ok(
    dts.startsWith(disposableDirective),
    `Expected leading esnext.disposable directive in ${dtsPath}`,
  );
  assert.equal(normalizeJsDoc(dts, dtsPath), dts);
  assert.equal(normalizeJsDoc(js, jsPath), js);

  const throwsCount = [...dts.matchAll(/\* @throws /g)].length;
  assert.ok(
    throwsCount >= 14,
    `Expected at least 14 @throws tags, found ${throwsCount}`,
  );

  const fallibleSignatures = [
    ["config", /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+config\(\): ProcessorConfig;/],
    ["controlActivity", /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+controlActivity\(\): ControlActivity;/],
    ["drainMono", /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+drainMono\(audio: Int16Array\): number;/],
    ["drain", /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+drain\(left: Int16Array, right: Int16Array\): number;/],
    ["feedback", /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+feedback\(\): Feedback;/],
    ["formantEnvelope", /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+formantEnvelope\([^)]*\): boolean;/],
    ["constructor", /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+constructor\([^)]*\);/],
    ["processMono", /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+processMono\([^)]*\): void;/],
    ["process", /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+process\([^)]*\): void;/],
    ["setConfig", /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+setConfig\(config: ProcessorConfig\): void;/],
    ["takeFeedback", /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+takeFeedback\(\): Feedback;/],
    ["takeTelemetry", /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+takeTelemetry\(\): Telemetry;/],
    ["telemetry", /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+telemetry\(\): Telemetry;/],
    ["defaultConfig", /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+export function defaultConfig\(\): ProcessorConfig;/],
  ];
  for (const [label, pattern] of fallibleSignatures) {
    const doc = extractMemberJsDoc(dts, pattern, label);
    assert.match(doc, /(?:^|\n)@throws /, `Expected @throws tag on ${label}`);
  }

  const controlActivityInterfaceMatch =
    /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+export interface ControlActivity \{([\s\S]*?)\n\}/.exec(
      dts,
    );
  assert.ok(controlActivityInterfaceMatch, "Expected ControlActivity interface");
  const controlActivityDoc = extractJsDocBlocks(
    controlActivityInterfaceMatch[1],
  )[0].replace(/\s+/g, " ");
  assert.ok(controlActivityDoc.includes("`ProcessorConfig`"));
  assert.ok(controlActivityDoc.includes("`Tunelet.controlActivity`"));
  assert.ok(
    controlActivityInterfaceMatch[2].includes("`ResynthesisConfig.choirVoices`"),
  );
  assert.ok(
    controlActivityInterfaceMatch[2].includes("`HarmonyConfig.source`"),
  );
  assert.ok(
    controlActivityInterfaceMatch[2].includes("`ReverbConfig.brightnessHz`"),
  );

  const latencyDoc = extractMemberJsDoc(
    dts,
    /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+readonly latencyFrames: number;/,
    "latencyFrames",
  ).replace(/\s+/g, " ");
  assert.ok(!latencyDoc.includes("latencyFrames"));
  assert.ok(latencyDoc.includes("dry path"));
  assert.ok(latencyDoc.includes("1,593 frames"));

  const beginDrainDoc = extractMemberJsDoc(
    dts,
    /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+beginDrain\(\): number;/,
    "beginDrain",
  ).replace(/\s+/g, " ");
  assert.ok(!beginDrainDoc.includes("beginDrain"));
  assert.ok(beginDrainDoc.includes("returns zero"));
  assert.ok(beginDrainDoc.includes("Completion resets"));

  const drainMonoDoc = extractMemberJsDoc(
    dts,
    /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+drainMono\(audio: Int16Array\): number;/,
    "drainMono",
  ).replace(/\s+/g, " ");
  assert.ok(!drainMonoDoc.includes("drainMono"));
  assert.ok(drainMonoDoc.includes("after the mono downmix"));
  assert.ok(drainMonoDoc.includes("Completion resets"));
}

/** Run one isolated Node.js child process while the HTTP server is active. */
function runNodeScript(scriptPath, args, cwd) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [scriptPath, ...args], {
      cwd,
      stdio: "inherit",
    });
    child.on("error", reject);
    child.on("exit", (code, signal) => {
      if (code === 0) {
        resolve();
        return;
      }
      reject(
        new Error(
          `Child process failed (${args.join(" ")}): code=${code} signal=${signal}`,
        ),
      );
    });
  });
}

/** Start a local HTTP server for `WebAssembly.instantiateStreaming`, MIME fallback and 404 checks. */
function startWasmServer(wasmBytes) {
  const requestCounts = new Map();
  const server = http.createServer((req, res) => {
    const route = req.url ?? "/";
    requestCounts.set(route, (requestCounts.get(route) ?? 0) + 1);

    if (route === "/tunelet.wasm") {
      res.writeHead(200, { "Content-Type": "application/wasm" });
      res.end(wasmBytes);
      return;
    }
    if (route === "/octet-stream.wasm") {
      res.writeHead(200, { "Content-Type": "application/octet-stream" });
      res.end(wasmBytes);
      return;
    }
    res.writeHead(404, { "Content-Type": "text/plain" });
    res.end("Not Found");
  });

  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      assert.ok(address !== null && typeof address === "object");
      resolve({
        baseUrl: `http://127.0.0.1:${address.port}`,
        requestCounts,
        close() {
          return new Promise((closeResolve, closeReject) => {
            server.closeAllConnections();
            server.close((error) => {
              if (error) {
                closeReject(error);
                return;
              }
              closeResolve();
            });
          });
        },
      });
    });
  });
}

/** Check the HTTP Wasm loaders in isolated Node.js child processes. */
async function verifyHttpLoaders(consumerDir, installedDir, packageName) {
  const wasmBytes = fs.readFileSync(
    path.join(installedDir, "pkg", "tunelet_bg.wasm"),
  );
  const checkHttpPath = path.join(consumerDir, "check-http.mjs");

  fs.writeFileSync(
    checkHttpPath,
    [
      'import assert from "node:assert/strict";',
      'import { pathToFileURL } from "node:url";',
      "",
      "const [packageName, installedDir, mode, baseUrl] = process.argv.slice(2);",
      "",
      "function verifyPcmBlock(moduleNamespace) {",
      "  const config = moduleNamespace.defaultConfig();",
      "  const processor = new moduleNamespace.Tunelet(48000, 128, config);",
      "  try {",
      "    const input = new Int16Array(128);",
      "    for (let i = 0; i < input.length; i += 1) {",
      "      input[i] = ((i * 517) % 16384) - 8192;",
      "    }",
      "    const left = new Int16Array(128);",
      "    const right = new Int16Array(128);",
      "    processor.process(input, left, right);",
      "    assert.equal(processor.feedback().processedFrames, 128n);",
      "    const mono = Int16Array.from(input);",
      "    processor.processMono(mono);",
      "    assert.equal(processor.feedback().processedFrames, 256n);",
      "  } finally {",
      "    processor.free();",
      "  }",
      "}",
      "",
      'if (mode === "http-404-retry") {',
      "  const rootModule = await import(packageName);",
      "  await assert.rejects(",
      "    () => rootModule.loadTunelet(`${baseUrl}/missing.wasm`),",
      "    /404/,",
      "  );",
      "  const wasmExports = await rootModule.loadTunelet(`${baseUrl}/tunelet.wasm`);",
      "  assert.ok(wasmExports);",
      "  const cachedExports = await rootModule.loadTunelet(`${baseUrl}/missing.wasm`);",
      "  assert.equal(cachedExports, wasmExports);",
      "  verifyPcmBlock(rootModule);",
      '} else if (mode === "http-response-wasm") {',
      "  const rootModule = await import(packageName);",
      "  const response = await fetch(`${baseUrl}/tunelet.wasm`);",
      "  assert.equal(response.ok, true);",
      '  assert.equal(response.headers.get("Content-Type"), "application/wasm");',
      "  const originalStreaming = WebAssembly.instantiateStreaming;",
      "  const originalInstantiate = WebAssembly.instantiate;",
      "  const originalWarn = console.warn;",
      "  let streamingCalls = 0;",
      "  let instantiateCalls = 0;",
      "  const warnings = [];",
      "  WebAssembly.instantiateStreaming = async (...args) => {",
      "    streamingCalls += 1;",
      "    return originalStreaming.apply(WebAssembly, args);",
      "  };",
      "  WebAssembly.instantiate = async (...args) => {",
      "    instantiateCalls += 1;",
      "    return originalInstantiate.apply(WebAssembly, args);",
      "  };",
      "  console.warn = (...args) => {",
      "    warnings.push(args);",
      "  };",
      "  try {",
      "    const wasmExports = await rootModule.loadTunelet(response);",
      "    assert.ok(wasmExports);",
      "  } finally {",
      "    WebAssembly.instantiateStreaming = originalStreaming;",
      "    WebAssembly.instantiate = originalInstantiate;",
      "    console.warn = originalWarn;",
      "  }",
      "  assert.equal(streamingCalls, 1);",
      "  assert.equal(instantiateCalls, 0);",
      "  assert.equal(warnings.length, 0);",
      "  verifyPcmBlock(rootModule);",
      '} else if (mode === "http-response-octet-stream") {',
      "  const rootModule = await import(packageName);",
      "  const response = await fetch(`${baseUrl}/octet-stream.wasm`);",
      "  assert.equal(response.ok, true);",
      '  assert.equal(response.headers.get("Content-Type"), "application/octet-stream");',
      "  const originalStreaming = WebAssembly.instantiateStreaming;",
      "  const originalInstantiate = WebAssembly.instantiate;",
      "  const originalWarn = console.warn;",
      "  let streamingCalls = 0;",
      "  let streamingRejected = false;",
      "  let instantiateCalls = 0;",
      "  const warnings = [];",
      "  WebAssembly.instantiateStreaming = async (...args) => {",
      "    streamingCalls += 1;",
      "    try {",
      "      return await originalStreaming.apply(WebAssembly, args);",
      "    } catch (error) {",
      "      streamingRejected = true;",
      "      throw error;",
      "    }",
      "  };",
      "  WebAssembly.instantiate = async (...args) => {",
      "    instantiateCalls += 1;",
      "    return originalInstantiate.apply(WebAssembly, args);",
      "  };",
      "  console.warn = (...args) => {",
      "    warnings.push(args);",
      "  };",
      "  try {",
      "    const wasmExports = await rootModule.loadTunelet(response);",
      "    assert.ok(wasmExports);",
      "  } finally {",
      "    WebAssembly.instantiateStreaming = originalStreaming;",
      "    WebAssembly.instantiate = originalInstantiate;",
      "    console.warn = originalWarn;",
      "  }",
      "  assert.equal(streamingCalls, 1);",
      "  assert.equal(streamingRejected, true);",
      "  assert.equal(instantiateCalls, 1);",
      "  assert.equal(warnings.length, 1);",
      "  assert.match(String(warnings[0][0]), /application\\/wasm/);",
      "  verifyPcmBlock(rootModule);",
      '} else if (mode === "browser-entry-http") {',
      "  const browserModule = await import(pathToFileURL(`${installedDir}/dist/browser.js`).href);",
      "  const crossRealmHttpUrl = { href: `${baseUrl}/tunelet.wasm`, protocol: \"http:\" };",
      "  assert.equal(crossRealmHttpUrl instanceof URL, false);",
      "  const wasmExports = await browserModule.loadTunelet(crossRealmHttpUrl);",
      "  assert.ok(wasmExports);",
      "  verifyPcmBlock(browserModule);",
      '} else if (mode === "index-entry-http") {',
      "  const indexModule = await import(pathToFileURL(`${installedDir}/dist/index.js`).href);",
      "  const wasmExports = await indexModule.loadTunelet(`${baseUrl}/tunelet.wasm`);",
      "  assert.ok(wasmExports);",
      "  verifyPcmBlock(indexModule);",
      "} else {",
      "  throw new Error(`Unknown HTTP loader mode: ${mode}`);",
      "}",
      "",
    ].join("\n"),
  );

  const server = await startWasmServer(wasmBytes);
  try {
    const modes = [
      "http-404-retry",
      "http-response-wasm",
      "http-response-octet-stream",
      "browser-entry-http",
      "index-entry-http",
    ];
    for (const mode of modes) {
      await runNodeScript(
        checkHttpPath,
        [packageName, installedDir, mode, server.baseUrl],
        consumerDir,
      );
    }
    assert.equal(server.requestCounts.get("/missing.wasm"), 1);
    assert.equal(server.requestCounts.get("/tunelet.wasm"), 4);
    assert.equal(server.requestCounts.get("/octet-stream.wasm"), 1);
  } finally {
    await server.close();
  }
}

/** Run `tsc` with a strict profile configuration and zero `skipLibCheck`. */
function runTscProfile(consumerDir, tscBin, profileName, compilerOptions, code) {
  const tsPath = path.join(consumerDir, `check-${profileName}.ts`);
  const configPath = path.join(consumerDir, `tsconfig.${profileName}.json`);
  fs.writeFileSync(tsPath, `${code.join("\n")}\n`, "utf8");
  fs.writeFileSync(
    configPath,
    JSON.stringify(
      {
        compilerOptions: {
          strict: true,
          exactOptionalPropertyTypes: true,
          skipLibCheck: false,
          noEmit: true,
          ...compilerOptions,
        },
        files: [tsPath],
      },
      null,
      2,
    ),
    "utf8",
  );
  execFileSync(process.execPath, [tscBin, "-p", configPath], {
    cwd: consumerDir,
    stdio: "inherit",
  });
}

/** Check strict TypeScript resolution across Node, ES2023, DOM and WebWorker. */
function verifyTypeDeclarations(consumerDir, installedDir, packageName) {
  const pkgLiteral = JSON.stringify(packageName);
  const wasmLiteral = JSON.stringify(`${packageName}/wasm`);
  const pkgWasmLiteral = JSON.stringify(`${packageName}/pkg/tunelet_bg.wasm`);
  const rootWasmLiteral = JSON.stringify(`${packageName}/tunelet_bg.wasm`);
  const directIndexLiteral = JSON.stringify(
    `./${path.posix.join(...path.relative(consumerDir, path.join(installedDir, "dist", "index.js")).split(path.sep))}`,
  );
  const tscBin = path.join(
    repoRoot,
    "node_modules",
    "typescript",
    "bin",
    "tsc",
  );
  const typeRoots = [path.join(repoRoot, "node_modules", "@types")];

  const sharedAssertions = [
    "import loadTunelet, {",
    "  defaultConfig,",
    "  initSync as rootInitSync,",
    "  loadTunelet as namedLoadTunelet,",
    "  Tunelet,",
    "  TuneletAudioViews,",
    "  type ControlActivity,",
    "  type HarmonyConfig,",
    "  type ProcessorConfig,",
    "  type ResynthesisConfig,",
    "  type SyncInitInput as RootSyncInitInput,",
    `} from ${pkgLiteral};`,
    `import indexDefaultLoad, { loadTunelet as indexLoadTunelet, Tunelet as IndexTunelet, TuneletAudioViews as IndexTuneletAudioViews } from ${directIndexLiteral};`,
    "import init, {",
    "  initSync as wasmInitSync,",
    "  type InitInput,",
    "  type InitOutput,",
    "  type SyncInitInput,",
    `} from ${wasmLiteral};`,
    "",
    "export async function verifyShared(",
    "  source?: InitInput,",
    "  syncSource?: SyncInitInput & RootSyncInitInput,",
    "  optionalVoices?: number,",
    "): Promise<InitOutput> {",
    "  const exportsFromRoot: InitOutput = await loadTunelet(source);",
    "  const exportsFromNamed: InitOutput = await namedLoadTunelet(source);",
    "  const exportsFromIndex: InitOutput = await indexLoadTunelet(source);",
    "  const exportsFromIndexDefault: InitOutput = await indexDefaultLoad(source);",
    "  const exportsFromWasm: InitOutput =",
    "    source === undefined",
    "      ? await init()",
    "      : await init({ module_or_path: source });",
    "  const baseConfig: ProcessorConfig = defaultConfig();",
    "  const harmony: HarmonyConfig = {",
    "    ...baseConfig.resynthesis.harmony,",
    "    voices: optionalVoices,",
    "  };",
    "  const config: ProcessorConfig = {",
    "    ...baseConfig,",
    "    resynthesis: {",
    "      ...baseConfig.resynthesis,",
    "      correction: {",
    "        ...baseConfig.resynthesis.correction,",
    "        humanize: optionalVoices,",
    "        flexTune: optionalVoices,",
    "      },",
    "      harmony,",
    "    },",
    "  };",
    "  config.resynthesis.harmony.voices = undefined;",
    "  {",
    "    using disposableProcessor = new Tunelet(48000, 128, config);",
    "    using disposableViews = new TuneletAudioViews(",
    "      exportsFromRoot.memory,",
    "      disposableProcessor,",
    "    );",
    "    const activity: ControlActivity = disposableProcessor.controlActivity();",
    "    void activity;",
    "    disposableViews.input.fill(0);",
    "    disposableProcessor.reset();",
    "  }",
    "  {",
    "    using disposableIndexProcessor = new IndexTunelet(48000, 128, config);",
    "    using disposableIndexViews = new IndexTuneletAudioViews(",
    "      exportsFromIndex.memory,",
    "      disposableIndexProcessor,",
    "    );",
    "    disposableIndexViews.input.fill(0);",
    "    disposableIndexProcessor.reset();",
    "  }",
    "  const processor = new Tunelet(48000, 128, config);",
    "  processor.setConfig(config);",
    "  const views = new TuneletAudioViews(exportsFromRoot.memory, processor);",
    "  const ptrs: [number, number, number] = [",
    "    processor.inputF32Ptr,",
    "    processor.leftF32Ptr,",
    "    processor.rightF32Ptr,",
    "  ];",
    "  views.input.fill(0);",
    "  processor.processF32InPlace(128);",
    "  const stereoOut: [Float32Array, Float32Array] = [views.left, views.right];",
    "  processor.processMonoF32InPlace(128);",
    "  processor.beginDrain();",
    "  const stereoTail: number = processor.drainF32InPlace(128);",
    "  const monoTail: number = processor.drainMonoF32InPlace(128);",
    "  views.refresh();",
    "  const resynthesisUpdate: ResynthesisConfig = {",
    "    ...config.resynthesis,",
    "    correction: {",
    "      enabled: config.resynthesis.correction.enabled,",
    "      notes: config.resynthesis.correction.notes,",
    "      retuneMs: config.resynthesis.correction.retuneMs,",
    "    },",
    "    harmony: {",
    "      ...config.resynthesis.harmony,",
    "      voices: optionalVoices,",
    "    },",
    "  };",
    "  const input = new Int16Array(128);",
    "  const left = new Int16Array(128);",
    "  const right = new Int16Array(128);",
    "  processor.processMono(input, [",
    '    { offset: 0, event: { type: "resynthesis", value: resynthesisUpdate } },',
    "  ]);",
    "  processor.process(input, left, right);",
    "  processor.beginDrain();",
    "  const drained: number = processor.drain(left, right);",
    "  const drainedMono: number = processor.drainMono(input);",
    "  const inputF32 = new Float32Array(128);",
    "  const leftF32 = new Float32Array(128);",
    "  const rightF32 = new Float32Array(128);",
    "  processor.processF32(inputF32, leftF32, rightF32);",
    "  processor.processF32(inputF32, leftF32, rightF32, [",
    '    { offset: 0, event: { type: "outputGain", value: 1 } },',
    "  ]);",
    "  processor.processMonoF32(inputF32);",
    "  processor.beginDrain();",
    "  const drainedF32: number = processor.drainF32(leftF32, rightF32);",
    "  const drainedMonoF32: number = processor.drainMonoF32(inputF32);",
    "  views.dispose();",
    "  processor.free();",
    "  if (syncSource !== undefined) {",
    "    const syncExportsFromRoot: InitOutput = rootInitSync({",
    "      module: syncSource,",
    "    });",
    "    const syncExportsFromWasm: InitOutput = wasmInitSync({",
    "      module: syncSource,",
    "    });",
    "    void syncExportsFromRoot;",
    "    void syncExportsFromWasm;",
    "  }",
    "  const fixedBuffer: ArrayBuffer = exportsFromRoot.memory.toFixedLengthBuffer();",
    "  const resizableBuffer: ArrayBuffer = exportsFromRoot.memory.toResizableBuffer();",
    "  void ptrs;",
    "  void stereoOut;",
    "  void stereoTail;",
    "  void monoTail;",
    "  void drained;",
    "  void drainedMono;",
    "  void drainedF32;",
    "  void drainedMonoF32;",
    "  void fixedBuffer;",
    "  void resizableBuffer;",
    "  void exportsFromNamed;",
    "  void exportsFromIndex;",
    "  void exportsFromIndexDefault;",
    "  void exportsFromWasm;",
    "  return exportsFromRoot;",
    "}",
  ];

  runTscProfile(
    consumerDir,
    tscBin,
    "node",
    {
      target: "ES2022",
      module: "NodeNext",
      moduleResolution: "NodeNext",
      lib: ["ES2022"],
      typeRoots,
      types: ["node"],
    },
    [
      'import fs from "node:fs/promises";',
      ...sharedAssertions,
      "",
      "export async function verifyNodeInputs(",
      "  req: Request,",
      "  res: Response,",
      "  url: URL,",
      "  voices?: number,",
      "): Promise<void> {",
      `  const pkgWasmHref: string = import.meta.resolve(${pkgWasmLiteral});`,
      `  const rootWasmHref: string = import.meta.resolve(${rootWasmLiteral});`,
      "  await loadTunelet(pkgWasmHref);",
      "  await loadTunelet(rootWasmHref);",
      '  await loadTunelet("file:///tmp/tunelet_bg.wasm");',
      "  await loadTunelet(url);",
      "  await loadTunelet(req);",
      "  await loadTunelet(res);",
      '  await loadTunelet(fetch("https://example.com/tunelet_bg.wasm"));',
      "  const bytes = await fs.readFile(url);",
      "  await verifyShared(bytes, bytes, voices);",
      "  // @ts-expect-error window must not leak into Node.js scope",
      "  void window;",
      "}",
    ],
  );

  runTscProfile(
    consumerDir,
    tscBin,
    "es2023-bundler",
    {
      target: "ES2022",
      module: "ES2022",
      moduleResolution: "Bundler",
      lib: ["ES2023"],
      types: [],
    },
    [
      ...sharedAssertions,
      "",
      "export async function verifyEs2023Inputs(voices?: number): Promise<void> {",
      "  const bytes = new Uint8Array(8);",
      '  await loadTunelet("https://example.com/tunelet_bg.wasm");',
      '  await loadTunelet({ href: "file:///tunelet_bg.wasm", protocol: "file:" });',
      "  await loadTunelet(bytes);",
      "  await loadTunelet(bytes.buffer);",
      "  await verifyShared(bytes, bytes, voices);",
      "  // @ts-expect-error window must not leak into pure ES2023 scope",
      "  void window;",
      "}",
    ],
  );

  runTscProfile(
    consumerDir,
    tscBin,
    "es2023-nodenext",
    {
      target: "ES2022",
      module: "NodeNext",
      moduleResolution: "NodeNext",
      lib: ["ES2023"],
      types: [],
    },
    [
      ...sharedAssertions,
      "",
      "export async function verifyEs2023NodeNext(voices?: number): Promise<void> {",
      "  const bytes = new Uint8Array(16);",
      "  await verifyShared(bytes, bytes, voices);",
      "  // @ts-expect-error window must not leak into pure ES2023 scope",
      "  void window;",
      "}",
    ],
  );

  runTscProfile(
    consumerDir,
    tscBin,
    "dom",
    {
      target: "ES2022",
      module: "ES2022",
      moduleResolution: "Bundler",
      customConditions: ["browser"],
      lib: ["ES2022", "DOM"],
      types: [],
    },
    [
      ...sharedAssertions,
      "",
      "export async function verifyDomInputs(",
      "  reqInfo: RequestInfo,",
      "  req: Request,",
      "  res: Response,",
      "  bufferSource: BufferSource,",
      "  compiled: WebAssembly.Module,",
      "  wasmMemory: WebAssembly.Memory,",
      "  voices?: number,",
      "): Promise<WebAssembly.Memory> {",
      "  await loadTunelet(reqInfo);",
      "  await loadTunelet(req);",
      "  await loadTunelet(res);",
      "  await loadTunelet(bufferSource);",
      '  await loadTunelet(new URL("./tunelet_bg.wasm", import.meta.url));',
      '  await loadTunelet(fetch("https://example.com/tunelet_bg.wasm"));',
      "  const out = await verifyShared(compiled, compiled, voices);",
      "  const domProc = new Tunelet(48000, 128);",
      "  const domViews = new TuneletAudioViews(wasmMemory, domProc);",
      "  domViews.dispose();",
      "  domProc.free();",
      "  const table: WebAssembly.Table = out.__wbindgen_externrefs;",
      "  void table;",
      "  void window;",
      "  return out.memory;",
      "}",
    ],
  );

  runTscProfile(
    consumerDir,
    tscBin,
    "webworker",
    {
      target: "ES2022",
      module: "ES2022",
      moduleResolution: "Bundler",
      lib: ["ES2022", "WebWorker"],
      types: [],
    },
    [
      ...sharedAssertions,
      "",
      "export async function verifyWorkerInputs(",
      "  reqInfo: RequestInfo,",
      "  req: Request,",
      "  res: Response,",
      "  bufferSource: BufferSource,",
      "  compiled: WebAssembly.Module,",
      "  wasmMemory: WebAssembly.Memory,",
      "  voices?: number,",
      "): Promise<WebAssembly.Memory> {",
      "  await loadTunelet(reqInfo);",
      "  await loadTunelet(req);",
      "  await loadTunelet(res);",
      "  await loadTunelet(bufferSource);",
      '  await loadTunelet(new URL("./tunelet_bg.wasm", self.location.href));',
      '  await loadTunelet(fetch("https://example.com/tunelet_bg.wasm"));',
      "  const out = await verifyShared(compiled, compiled, voices);",
      "  const workerProc = new Tunelet(48000, 128);",
      "  const workerViews = new TuneletAudioViews(wasmMemory, workerProc);",
      "  workerViews.dispose();",
      "  workerProc.free();",
      "  const table: WebAssembly.Table = out.__wbindgen_externrefs;",
      "  void table;",
      "  void self;",
      "  // @ts-expect-error window must not leak into WebWorker scope",
      "  void window;",
      "  return out.memory;",
      "}",
    ],
  );
}

/** Check the archive list, runtime imports, HTTP loaders, types and JSDoc. */
async function verifyPackedArchive(workDir, relativeArchivePath, packageName) {
  const consumerDir = path.join(workDir, "consumer");
  fs.mkdirSync(consumerDir);
  verifyArchiveEntries(workDir, relativeArchivePath);
  const installedDir = installArchive(
    workDir,
    relativeArchivePath,
    consumerDir,
    packageName,
  );
  verifyInstalledFiles(installedDir);
  verifyRuntimeImport(consumerDir, installedDir, packageName);
  await verifyHttpLoaders(consumerDir, installedDir, packageName);
  verifyTypeDeclarations(consumerDir, installedDir, packageName);
  verifyNormalizedJsDoc(installedDir);
}

/** Copy both verified archives and write `SHA256SUMS.txt` in `outDir`. */
function writeVerifiedOutputs(outDir, archives) {
  fs.mkdirSync(outDir, { recursive: true });
  const checksumLines = [];
  for (const { fileName, sourcePath } of archives) {
    const destinationPath = path.join(outDir, fileName);
    fs.copyFileSync(sourcePath, destinationPath);
    const digest = createHash("sha256")
      .update(fs.readFileSync(destinationPath))
      .digest("hex");
    checksumLines.push(`${digest}  ${fileName}`);
  }
  fs.writeFileSync(
    path.join(outDir, "SHA256SUMS.txt"),
    `${checksumLines.join("\n")}\n`,
  );
}

const { ignoreScripts, outDir } = parseCliOptions(process.argv.slice(2));
const manifest = readPackageManifest();
if (ignoreScripts) {
  verifyPrebuiltFiles();
}
const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "tunelet-verify-pack-"));

try {
  const unscopedWorkDir = path.join(tempDir, "unscoped");
  const unscopedPackDir = path.join(unscopedWorkDir, "pack");
  fs.mkdirSync(unscopedPackDir, { recursive: true });
  const unscopedFileName = expectedArchiveFileName(
    UNSCOPED_PACKAGE_NAME,
    manifest.version,
  );
  const unscopedRelativeArchivePath = packArchive(
    repoRoot,
    unscopedPackDir,
    unscopedFileName,
    { ignoreScripts },
  );
  await verifyPackedArchive(
    unscopedWorkDir,
    unscopedRelativeArchivePath,
    UNSCOPED_PACKAGE_NAME,
  );

  const scopedWorkDir = path.join(tempDir, "scoped");
  const scopedStageDir = path.join(scopedWorkDir, "stage");
  const scopedPackDir = path.join(scopedWorkDir, "pack");
  fs.mkdirSync(scopedPackDir, { recursive: true });
  stageScopedPackage(scopedStageDir, manifest, SCOPED_PACKAGE_NAME);
  const scopedFileName = expectedArchiveFileName(
    SCOPED_PACKAGE_NAME,
    manifest.version,
  );
  const scopedRelativeArchivePath = packArchive(
    scopedStageDir,
    scopedPackDir,
    scopedFileName,
    { ignoreScripts: true },
  );
  await verifyPackedArchive(
    scopedWorkDir,
    scopedRelativeArchivePath,
    SCOPED_PACKAGE_NAME,
  );

  if (outDir !== undefined) {
    writeVerifiedOutputs(outDir, [
      {
        fileName: unscopedFileName,
        sourcePath: path.join(unscopedWorkDir, unscopedRelativeArchivePath),
      },
      {
        fileName: scopedFileName,
        sourcePath: path.join(scopedWorkDir, scopedRelativeArchivePath),
      },
    ]);
  }
} finally {
  fs.rmSync(tempDir, { recursive: true, force: true });
}
