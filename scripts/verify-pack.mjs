/**
 * This script builds the package one time and packs `tunelet` and
 * `@bytelandtechnology/tunelet`. For each archive, it checks the eight files,
 * `loadTunelet()`, the HTTP Wasm loaders, the `NodeNext` TypeScript
 * declarations and the JSDoc comments.
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

const EXPECTED_ARCHIVE_PATHS = [
  "package/LICENSE",
  "package/README.md",
  "package/dist/index.d.ts",
  "package/dist/index.js",
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

/** Check that the archive contains only the eight expected files. */
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

  fs.writeFileSync(
    path.join(consumerDir, "package.json"),
    JSON.stringify({
      name: "tunelet-pack-consumer",
      private: true,
      type: "module",
    }),
  );
}

/** Import the installed package in Node.js and check Wasm, PCM and Float32 paths. */
function verifyRuntimeImport(consumerDir, packageName) {
  const checkDefaultPath = path.join(consumerDir, "check-default.mjs");
  fs.writeFileSync(
    checkDefaultPath,
    [
      'import assert from "node:assert/strict";',
      'import fs from "node:fs";',
      'import { createRequire } from "node:module";',
      'import { fileURLToPath } from "node:url";',
      "",
      "const packageName = process.argv[2];",
      "const require = createRequire(import.meta.url);",
      "const pkgWasmSubpath = `${packageName}/pkg/tunelet_bg.wasm`;",
      "const rootWasmSubpath = `${packageName}/tunelet_bg.wasm`;",
      "const resolvedPkgWasm = import.meta.resolve(pkgWasmSubpath);",
      "const resolvedRootWasm = import.meta.resolve(rootWasmSubpath);",
      'assert.match(resolvedPkgWasm, /^file:/);',
      "assert.equal(resolvedPkgWasm, resolvedRootWasm);",
      "const wasmFilePath = fileURLToPath(resolvedPkgWasm);",
      "assert.ok(fs.statSync(wasmFilePath).isFile());",
      "assert.equal(require.resolve(pkgWasmSubpath), wasmFilePath);",
      "assert.equal(require.resolve(rootWasmSubpath), wasmFilePath);",
      "",
      "const rootModule = await import(packageName);",
      "const wasmModule = await import(`${packageName}/wasm`);",
      "",
      'assert.equal(typeof rootModule.loadTunelet, "function");',
      "assert.equal(rootModule.default, rootModule.loadTunelet);",
      'assert.equal(typeof rootModule.initSync, "function");',
      'assert.equal(typeof rootModule.Tunelet, "function");',
      'assert.equal(typeof rootModule.TuneletAudioViews, "function");',
      'assert.equal(typeof rootModule.defaultConfig, "function");',
      'assert.equal(typeof wasmModule.default, "function");',
      'assert.equal(typeof wasmModule.initSync, "function");',
      "",
      "const wasmExports = await rootModule.loadTunelet();",
      "assert.ok(wasmExports);",
      "assert.ok(wasmExports.memory instanceof WebAssembly.Memory);",
      "",
      "const initialConfig = rootModule.defaultConfig();",
      "initialConfig.resynthesis.correction.humanize = undefined;",
      "initialConfig.resynthesis.correction.flexTune = undefined;",
      "initialConfig.resynthesis.harmony.voices = undefined;",
      "const processor = new rootModule.Tunelet(48000, 128, initialConfig);",
      "try {",
      "  assert.equal(processor.config().resynthesis.correction.humanize, 0);",
      "  assert.equal(processor.config().resynthesis.correction.flexTune, 0);",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 4);",
      "  assert.equal(typeof processor.controlActivity().choirVoices, \"boolean\");",
      "  const initialTelemetry = processor.telemetry();",
      "  assert.ok(initialTelemetry.voiceRatios instanceof Float32Array);",
      "  assert.ok(initialTelemetry.voiceGains instanceof Float32Array);",
      "  assert.equal(initialTelemetry.voiceRatios.length, initialTelemetry.voiceCount);",
      "  assert.equal(initialTelemetry.voiced, false);",
      "  assert.equal(initialTelemetry.targetMidiNote, undefined);",
      "",
      "  const twoVoiceConfig = processor.config();",
      "  twoVoiceConfig.resynthesis.harmony.voices = 2;",
      "  processor.setConfig(twoVoiceConfig);",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 2);",
      "",
      "  const undefinedVoiceConfig = processor.config();",
      "  undefinedVoiceConfig.resynthesis.harmony.voices = undefined;",
      "  processor.setConfig(undefinedVoiceConfig);",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 4);",
      "",
      "  const oneVoiceConfig = processor.config();",
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
      "  const input = new Int16Array(128);",
      "  const left = new Int16Array(128);",
      "  const right = new Int16Array(128);",
      "  const midi = new Uint32Array([0, (0 << 24) | (0x90 << 16) | (60 << 8) | 100]);",
      '  processor.process(input, left, right, [',
      '    { offset: 0, event: { type: "parameter", value: { type: "dryMix", value: 0.25 } } },',
      '    { offset: 0, event: { type: "parameter", value: { type: "humanize", value: 0.5 } } },',
      '    { offset: 0, event: { type: "parameter", value: { type: "flexTune", value: 0.25 } } },',
      "  ]);",
      "  assert.equal(processor.config().resynthesis.dryMix, 0.25);",
      "  assert.equal(processor.config().resynthesis.correction.humanize, 0.5);",
      "  assert.equal(processor.config().resynthesis.correction.flexTune, 0.25);",
      "  processor.processWithMidi(input, left, right, midi);",
      "  processor.processMonoWithMidi(input, midi);",
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
      "",
    ].join("\n"),
  );

  execFileSync(process.execPath, [checkDefaultPath, packageName], {
    cwd: consumerDir,
    stdio: "inherit",
  });

  const checkFileStringPath = path.join(consumerDir, "check-file-string.mjs");
  fs.writeFileSync(
    checkFileStringPath,
    [
      'import assert from "node:assert/strict";',
      "",
      "const packageName = process.argv[2];",
      "const wasmHref = import.meta.resolve(`${packageName}/pkg/tunelet_bg.wasm`);",
      'assert.equal(typeof wasmHref, "string");',
      'assert.match(wasmHref, /^file:/);',
      "const { loadTunelet, Tunelet } = await import(packageName);",
      "const exportsFromString = await loadTunelet(wasmHref);",
      "assert.ok(exportsFromString);",
      "const processor = new Tunelet(48000, 128);",
      "try {",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 4);",
      "} finally {",
      "  processor.free();",
      "}",
      "",
    ].join("\n"),
  );

  execFileSync(process.execPath, [checkFileStringPath, packageName], {
    cwd: consumerDir,
    stdio: "inherit",
  });

  const checkCrossRealmPath = path.join(consumerDir, "check-cross-realm.mjs");
  fs.writeFileSync(
    checkCrossRealmPath,
    [
      'import assert from "node:assert/strict";',
      "",
      "const packageName = process.argv[2];",
      "const wasmHref = import.meta.resolve(`${packageName}/tunelet_bg.wasm`);",
      'const crossRealmUrl = { href: wasmHref, protocol: "file:" };',
      "assert.equal(crossRealmUrl instanceof URL, false);",
      "const { loadTunelet, Tunelet } = await import(packageName);",
      "const exportsFromObject = await loadTunelet(crossRealmUrl);",
      "assert.ok(exportsFromObject);",
      "const processor = new Tunelet(48000, 128);",
      "try {",
      "  assert.equal(processor.sampleRate, 48000);",
      "} finally {",
      "  processor.free();",
      "}",
      "",
    ].join("\n"),
  );

  execFileSync(process.execPath, [checkCrossRealmPath, packageName], {
    cwd: consumerDir,
    stdio: "inherit",
  });

  const checkWasmInitPath = path.join(consumerDir, "check-wasm-init.mjs");
  fs.writeFileSync(
    checkWasmInitPath,
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
      "",
    ].join("\n"),
  );

  execFileSync(process.execPath, [checkWasmInitPath, packageName], {
    cwd: consumerDir,
    stdio: "inherit",
  });

  const checkInitSyncPath = path.join(consumerDir, "check-init-sync.mjs");
  fs.writeFileSync(
    checkInitSyncPath,
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
      "",
    ].join("\n"),
  );

  execFileSync(process.execPath, [checkInitSyncPath, packageName], {
    cwd: consumerDir,
    stdio: "inherit",
  });

  const checkWasmInitSyncPath = path.join(
    consumerDir,
    "check-wasm-init-sync.mjs",
  );
  fs.writeFileSync(
    checkWasmInitSyncPath,
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
      "",
    ].join("\n"),
  );

  execFileSync(process.execPath, [checkWasmInitSyncPath, packageName], {
    cwd: consumerDir,
    stdio: "inherit",
  });
}

/** Check strict TypeScript consumer profiles without ambient leakage. */
function verifyTypeDeclarations(consumerDir, packageName) {
  const pkgLiteral = JSON.stringify(packageName);
  const wasmLiteral = JSON.stringify(`${packageName}/wasm`);
  const pkgWasmLiteral = JSON.stringify(`${packageName}/pkg/tunelet_bg.wasm`);
  const rootWasmLiteral = JSON.stringify(`${packageName}/tunelet_bg.wasm`);
  const typeRoots = [path.join(repoRoot, "node_modules", "@types")];

  const sharedImportsAndHarmonyChecks = [
    "import loadTunelet, {",
    "  defaultConfig,",
    "  initSync as rootInitSync,",
    "  loadTunelet as namedLoadTunelet,",
    "  Tunelet,",
    "  TuneletAudioViews,",
    "  type ControlActivity,",
    "  type ControlEvent,",
    "  type ParameterControl,",
    "  type ProcessorConfig,",
    "  type ResynthesisConfig,",
    "  type SyncInitInput as RootSyncInitInput,",
    "  type Telemetry,",
    `} from ${pkgLiteral};`,
    "import init, {",
    "  initSync as wasmInitSync,",
    "  type InitInput,",
    "  type InitOutput,",
    "  type ParameterControl as WasmParameterControl,",
    "  type SyncInitInput,",
    `} from ${wasmLiteral};`,
    "",
    "async function verifySharedApi(",
    "  source: InitInput | undefined,",
    "  syncSource: SyncInitInput & RootSyncInitInput,",
    "  optionalVoices: number | undefined,",
    "): Promise<InitOutput> {",
    "  const exportsFromRoot: InitOutput = await loadTunelet(source);",
    "  const exportsFromNamed: InitOutput = await namedLoadTunelet(source);",
    "  const exportsFromWasm: InitOutput =",
    "    source === undefined",
    "      ? await init()",
    "      : await init({ module_or_path: source });",
    "  const syncExportsFromRoot: InitOutput = rootInitSync({",
    "    module: syncSource,",
    "  });",
    "  const syncExportsFromWasm: InitOutput = wasmInitSync({",
    "    module: syncSource,",
    "  });",
    "  const baseConfig: ProcessorConfig = defaultConfig();",
    "  const configWithUndefined: ProcessorConfig = {",
    "    ...baseConfig,",
    "    resynthesis: {",
    "      ...baseConfig.resynthesis,",
    "      correction: {",
    "        ...baseConfig.resynthesis.correction,",
    "        humanize: optionalVoices,",
    "        flexTune: optionalVoices,",
    "      },",
    "      harmony: {",
    "        ...baseConfig.resynthesis.harmony,",
    "        voices: optionalVoices,",
    "      },",
    "    },",
    "  };",
    "  {",
    "    using disposableProcessor = new Tunelet(",
    "      48000,",
    "      128,",
    "      configWithUndefined,",
    "    );",
    "    using disposableViews = new TuneletAudioViews(",
    "      exportsFromRoot.memory,",
    "      disposableProcessor,",
    "    );",
    "    disposableViews.input.fill(0);",
    "    disposableProcessor.reset();",
    "  }",
    "  const processor = new Tunelet(48000, 128, configWithUndefined);",
    "  processor.setConfig(configWithUndefined);",
    "  const activity: ControlActivity = processor.controlActivity();",
    "  const telemetry: Telemetry = processor.telemetry();",
    "  const voiceSlices: [Float32Array, Float32Array] = [",
    "    telemetry.voiceRatios,",
    "    telemetry.voiceGains,",
    "  ];",
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
    "    ...configWithUndefined.resynthesis,",
    "    correction: {",
    "      enabled: configWithUndefined.resynthesis.correction.enabled,",
    "      notes: configWithUndefined.resynthesis.correction.notes,",
    "      retuneMs: configWithUndefined.resynthesis.correction.retuneMs,",
    "    },",
    "    harmony: {",
    "      ...configWithUndefined.resynthesis.harmony,",
    "      voices: undefined,",
    "    },",
    "  };",
    "  const input = new Int16Array(128);",
    "  const left = new Int16Array(128);",
    "  const right = new Int16Array(128);",
    "  const midi = new Uint32Array([0, (0 << 24) | (0x90 << 16) | (60 << 8) | 100]);",
    "  processor.processMono(input, [",
    '    { offset: 0, event: { type: "resynthesis", value: resynthesisUpdate } },',
    "  ]);",
    '  const dryMixParameter: ParameterControl & WasmParameterControl = { type: "dryMix", value: 0.25 };',
    '  const humanizeParameter: ParameterControl & WasmParameterControl = { type: "humanize", value: 0.5 };',
    '  const flexTuneParameter: ParameterControl & WasmParameterControl = { type: "flexTune", value: 0.25 };',
    '  const parameterEvent: ControlEvent = { type: "parameter", value: dryMixParameter };',
    "  processor.process(input, left, right, [",
    "    { offset: 0, event: parameterEvent },",
    '    { offset: 0, event: { type: "parameter", value: humanizeParameter } },',
    '    { offset: 0, event: { type: "parameter", value: flexTuneParameter } },',
    "  ]);",
    "  processor.processWithMidi(input, left, right, midi);",
    "  processor.processMonoWithMidi(input, midi);",
    "  processor.beginDrain();",
    "  const drained: number = processor.drain(left, right);",
    "  const drainedMono: number = processor.drainMono(input);",
    "  const inputF32 = new Float32Array(128);",
    "  const leftF32 = new Float32Array(128);",
    "  const rightF32 = new Float32Array(128);",
    "  processor.processF32(inputF32, leftF32, rightF32);",
    "  processor.processF32(inputF32, leftF32, rightF32, [",
    '    { offset: 0, event: { type: "outputGain", value: 1 } },',
    "    { offset: 0, event: parameterEvent },",
    "  ]);",
    "  processor.processMonoF32(inputF32);",
    "  processor.beginDrain();",
    "  const drainedF32: number = processor.drainF32(leftF32, rightF32);",
    "  const drainedMonoF32: number = processor.drainMonoF32(inputF32);",
    "  views.dispose();",
    "  processor.free();",
    "  void activity;",
    "  void voiceSlices;",
    "  void ptrs;",
    "  void stereoOut;",
    "  void stereoTail;",
    "  void monoTail;",
    "  void drained;",
    "  void drainedMono;",
    "  void drainedF32;",
    "  void drainedMonoF32;",
    "  void exportsFromNamed;",
    "  void exportsFromWasm;",
    "  void syncExportsFromRoot;",
    "  void syncExportsFromWasm;",
    "  return exportsFromRoot;",
    "}",
  ];

  const profiles = [
    {
      name: "node",
      compilerOptions: {
        target: "ES2022",
        module: "NodeNext",
        moduleResolution: "NodeNext",
        lib: ["ES2022"],
        typeRoots,
        types: ["node"],
        strict: true,
        exactOptionalPropertyTypes: true,
        skipLibCheck: false,
      },
      lines: [
        'import fs from "node:fs/promises";',
        ...sharedImportsAndHarmonyChecks,
        "",
        "export async function verifyNodeProfile(",
        "  req: Request,",
        "  res: Response,",
        "  url: URL,",
        "  voices?: number,",
        "): Promise<void> {",
        `  const pkgWasmHref: string = import.meta.resolve(${pkgWasmLiteral});`,
        `  const rootWasmHref: string = import.meta.resolve(${rootWasmLiteral});`,
        "  await loadTunelet(pkgWasmHref);",
        "  await loadTunelet(rootWasmHref);",
        "  await loadTunelet(url);",
        "  await loadTunelet(req);",
        "  await loadTunelet(res);",
        '  await loadTunelet(fetch("https://example.com/tunelet_bg.wasm"));',
        "  const bytes = await fs.readFile(url);",
        "  await verifySharedApi(bytes, bytes, voices);",
        "  // @ts-expect-error window must not leak into Node.js scope",
        "  void window;",
        "}",
        "",
      ],
    },
    {
      name: "es2023",
      compilerOptions: {
        target: "ES2022",
        module: "NodeNext",
        moduleResolution: "NodeNext",
        lib: ["ES2023"],
        types: [],
        strict: true,
        exactOptionalPropertyTypes: true,
        skipLibCheck: false,
      },
      lines: [
        ...sharedImportsAndHarmonyChecks,
        "",
        "export async function verifyEs2023Profile(voices?: number): Promise<void> {",
        "  const bytes = new Uint8Array(16);",
        "  await loadTunelet(bytes.buffer);",
        '  await loadTunelet({ href: "file:///tunelet_bg.wasm", protocol: "file:" });',
        "  await verifySharedApi(bytes, bytes, voices);",
        "  // @ts-expect-error window must not leak into pure ES2023 scope",
        "  void window;",
        "}",
        "",
      ],
    },
    {
      name: "dom",
      compilerOptions: {
        target: "ES2022",
        module: "NodeNext",
        moduleResolution: "NodeNext",
        lib: ["ES2022", "DOM"],
        types: [],
        strict: true,
        exactOptionalPropertyTypes: true,
        skipLibCheck: false,
      },
      lines: [
        ...sharedImportsAndHarmonyChecks,
        "",
        "export async function verifyDomProfile(",
        "  reqInfo: RequestInfo,",
        "  req: Request,",
        "  res: Response,",
        "  url: URL,",
        "  bufferSource: BufferSource,",
        "  wasmModule: WebAssembly.Module,",
        "  wasmMemory: WebAssembly.Memory,",
        "  voices?: number,",
        "): Promise<WebAssembly.Memory> {",
        "  await loadTunelet(reqInfo);",
        "  await loadTunelet(req);",
        "  await loadTunelet(res);",
        '  await loadTunelet(fetch("/tunelet_bg.wasm"));',
        "  await loadTunelet(url);",
        "  await loadTunelet(bufferSource);",
        "  const out = await verifySharedApi(wasmModule, wasmModule, voices);",
        "  const domProc = new Tunelet(48000, 128);",
        "  const domViews = new TuneletAudioViews(wasmMemory, domProc);",
        "  domViews.dispose();",
        "  domProc.free();",
        "  const table: WebAssembly.Table = out.__wbindgen_externrefs;",
        "  void table;",
        "  void window;",
        "  return out.memory;",
        "}",
        "",
      ],
    },
    {
      name: "webworker",
      compilerOptions: {
        target: "ES2022",
        module: "NodeNext",
        moduleResolution: "NodeNext",
        lib: ["ES2022", "WebWorker"],
        types: [],
        strict: true,
        exactOptionalPropertyTypes: true,
        skipLibCheck: false,
      },
      lines: [
        ...sharedImportsAndHarmonyChecks,
        "",
        "export async function verifyWebWorkerProfile(",
        "  reqInfo: RequestInfo,",
        "  res: Response,",
        "  url: URL,",
        "  bufferSource: BufferSource,",
        "  wasmModule: WebAssembly.Module,",
        "  wasmMemory: WebAssembly.Memory,",
        "  voices?: number,",
        "): Promise<WebAssembly.Memory> {",
        "  await loadTunelet(reqInfo);",
        "  await loadTunelet(res);",
        '  await loadTunelet(fetch("/tunelet_bg.wasm"));',
        "  await loadTunelet(url);",
        "  await loadTunelet(bufferSource);",
        "  const out = await verifySharedApi(wasmModule, wasmModule, voices);",
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
        "",
      ],
    },
  ];

  const tscBin = path.join(
    repoRoot,
    "node_modules",
    "typescript",
    "bin",
    "tsc",
  );

  for (const profile of profiles) {
    const sourceFile = `check-${profile.name}.ts`;
    const sourcePath = path.join(consumerDir, sourceFile);
    const configPath = path.join(consumerDir, `tsconfig.${profile.name}.json`);
    fs.writeFileSync(sourcePath, profile.lines.join("\n"));
    fs.writeFileSync(
      configPath,
      JSON.stringify({
        compilerOptions: profile.compilerOptions,
        files: [sourceFile],
      }),
    );
    execFileSync(
      process.execPath,
      [
        tscBin,
        "--noEmit",
        "--strict",
        "--exactOptionalPropertyTypes",
        "-p",
        configPath,
      ],
      { cwd: consumerDir, stdio: "inherit" },
    );
  }
}

/** Check the archive list, runtime imports and TypeScript declarations. */
async function verifyPackedArchive(workDir, relativeArchivePath, packageName) {
  const consumerDir = path.join(workDir, "consumer");
  fs.mkdirSync(consumerDir);
  verifyArchiveEntries(workDir, relativeArchivePath);
  installArchive(workDir, relativeArchivePath, consumerDir, packageName);
  verifyRuntimeImport(consumerDir, packageName);
  await verifyHttpLoaders(consumerDir, packageName);
  verifyTypeDeclarations(consumerDir, packageName);
  verifyNormalizedJsDoc(consumerDir, packageName);
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
function verifyNormalizedJsDoc(consumerDir, packageName) {
  const installedDir = path.join(
    consumerDir,
    "node_modules",
    ...packageName.split("/"),
  );
  const filesToCheck = [
    path.join(installedDir, "dist", "index.d.ts"),
    path.join(installedDir, "dist", "index.js"),
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
    ["processMonoWithMidi", /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+processMonoWithMidi\([^)]*\): void;/],
    ["processMono", /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+processMono\([^)]*\): void;/],
    ["processWithMidi", /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+processWithMidi\([^)]*\): void;/],
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

  const telemetryInterfaceMatch =
    /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+export interface Telemetry \{([\s\S]*?)\n\}/.exec(
      dts,
    );
  assert.ok(telemetryInterfaceMatch, "Expected Telemetry interface");
  const telemetryDoc = extractJsDocBlocks(
    telemetryInterfaceMatch[1],
  )[0].replace(/\s+/g, " ");
  assert.ok(telemetryDoc.includes("`Tunelet.telemetry`"));
  assert.ok(telemetryDoc.includes("`Tunelet.takeTelemetry`"));
  assert.ok(!telemetryDoc.includes("`Default`"));
  assert.ok(telemetryInterfaceMatch[2].includes("`Feedback.resampleRatio`"));
  assert.ok(telemetryInterfaceMatch[2].includes("`Feedback.periodSamples`"));
  assert.ok(telemetryInterfaceMatch[2].includes("`Feedback.frequencyHz`"));
  assert.ok(telemetryInterfaceMatch[2].includes("`Feedback.targetMidiNote`"));
  assert.ok(telemetryInterfaceMatch[2].includes("`periodSamples`"));
  assert.ok(telemetryInterfaceMatch[2].includes("voiceRatios: Float32Array;"));
  assert.ok(telemetryInterfaceMatch[2].includes("voiceGains: Float32Array;"));

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
async function verifyHttpLoaders(consumerDir, packageName) {
  const installedDir = path.join(
    consumerDir,
    "node_modules",
    ...packageName.split("/"),
  );
  const wasmBytes = fs.readFileSync(
    path.join(installedDir, "pkg", "tunelet_bg.wasm"),
  );
  const checkHttpPath = path.join(consumerDir, "check-http.mjs");

  fs.writeFileSync(
    checkHttpPath,
    [
      'import assert from "node:assert/strict";',
      "",
      "const [packageName, mode, baseUrl] = process.argv.slice(2);",
      "const rootModule = await import(packageName);",
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
    ];
    for (const mode of modes) {
      await runNodeScript(
        checkHttpPath,
        [packageName, mode, server.baseUrl],
        consumerDir,
      );
    }
    assert.equal(server.requestCounts.get("/missing.wasm"), 1);
    assert.equal(server.requestCounts.get("/tunelet.wasm"), 2);
    assert.equal(server.requestCounts.get("/octet-stream.wasm"), 1);
  } finally {
    await server.close();
  }
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
