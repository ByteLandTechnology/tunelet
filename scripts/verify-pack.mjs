/**
 * This script packs the npm archive, checks its eight files, runs
 * `loadTunelet()` and checks the TypeScript declarations.
 */
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

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

/** Read and check the current package name so scoped names also work. */
function readPackageName() {
  const packageJsonPath = path.join(repoRoot, "package.json");
  const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, "utf8"));
  const packageName = packageJson.name;
  assert.equal(
    typeof packageName,
    "string",
    "package.json name must be a string",
  );
  assert.match(
    packageName,
    PACKAGE_NAME_PATTERN,
    `Invalid package name: ${packageName}`,
  );
  return packageName;
}

/** Run `npm pack` without a shell and return the relative archive path. */
function packArchive(packDir) {
  const npmExecPath = process.env.npm_execpath;
  if (npmExecPath) {
    execFileSync(
      process.execPath,
      [npmExecPath, "pack", "--pack-destination", packDir],
      { cwd: repoRoot, stdio: "inherit" },
    );
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
    execFileSync(
      process.execPath,
      [npmCliPath, "pack", "--pack-destination", packDir],
      { cwd: repoRoot, stdio: "inherit" },
    );
  } else {
    execFileSync("npm", ["pack", "--pack-destination", packDir], {
      cwd: repoRoot,
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

/** Import the installed package in Node.js and verify PCM and Float32 paths. */
function verifyRuntimeImport(consumerDir, packageName) {
  const checkPath = path.join(consumerDir, "check.mjs");
  fs.writeFileSync(
    checkPath,
    [
      'import assert from "node:assert/strict";',
      "",
      "const packageName = process.argv[2];",
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
      "const config = rootModule.defaultConfig();",
      "config.resynthesis.formant.enabled = true;",
      "config.reverb.enabled = true;",
      "const pcmProcessor = new rootModule.Tunelet(48000, 128, config);",
      "const f32Processor = new rootModule.Tunelet(48000, 128, config);",
      "const views = new rootModule.TuneletAudioViews(wasmExports.memory, f32Processor);",
      "try {",
      "  assert.equal(views.maxBlockFrames, 128);",
      "  assert.equal(views.disposed, false);",
      "  assert.equal(f32Processor.inputF32Ptr % 4, 0);",
      "  assert.equal(f32Processor.leftF32Ptr % 4, 0);",
      "  assert.equal(f32Processor.rightF32Ptr % 4, 0);",
      "  assert.equal(views.input.length, 128);",
      "  assert.equal(views.left.length, 128);",
      "  assert.equal(views.right.length, 128);",
      "  assert.equal(views.input, views.input);",
      "",
      "  const pcmInput = new Int16Array(128);",
      "  const pcmLeft = new Int16Array(128);",
      "  const pcmRight = new Int16Array(128);",
      "  for (let block = 0; block < 20; block++) {",
      "    const f32In = views.input;",
      "    for (let i = 0; i < 128; i++) {",
      "      const sample = Math.fround(0.6 * Math.sin(((block * 128 + i) * 2 * Math.PI * 220) / 48000));",
      "      f32In[i] = sample;",
      "      pcmInput[i] = Math.round(Math.max(-1, Math.min(1, sample)) * 32767);",
      "    }",
      "    pcmProcessor.process(pcmInput, pcmLeft, pcmRight);",
      "    f32Processor.processF32InPlace(128);",
      "    const f32Left = views.left;",
      "    const f32Right = views.right;",
      "    for (let i = 0; i < 128; i++) {",
      "      assert.equal(f32Left[i], Math.fround(pcmLeft[i] / 32768));",
      "      assert.equal(f32Right[i], Math.fround(pcmRight[i] / 32768));",
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
      "  f32Processor.processF32InPlace(64, [{ offset: 0, event: { type: \"outputGain\", value: 0.75 } }]);",
      "  pcmInput.fill(Math.round(Math.fround(0.25) * 32767), 0, 64);",
      "  pcmProcessor.process(",
      "    pcmInput.subarray(0, 64),",
      "    pcmLeft.subarray(0, 64),",
      "    pcmRight.subarray(0, 64),",
      "    [{ offset: 0, event: { type: \"outputGain\", value: 0.75 } }],",
      "  );",
      "  for (let i = 0; i < 64; i++) {",
      "    assert.equal(views.left[i], Math.fround(pcmLeft[i] / 32768));",
      "    assert.equal(views.right[i], Math.fround(pcmRight[i] / 32768));",
      "  }",
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
      "  f32Processor.beginDrain();",
      "  const pcmCount = pcmProcessor.drain(pcmLeft, pcmRight);",
      "  const f32Count = f32Processor.drainF32InPlace(128);",
      "  assert.equal(f32Count, pcmCount);",
      "  for (let i = 0; i < 128; i++) {",
      "    assert.equal(views.left[i], Math.fround(pcmLeft[i] / 32768));",
      "    assert.equal(views.right[i], Math.fround(pcmRight[i] / 32768));",
      "  }",
      "",
      "  const monoPcm = new rootModule.Tunelet(48000, 64, config);",
      "  const monoF32 = new rootModule.Tunelet(48000, 64, config);",
      "  const monoViews = new rootModule.TuneletAudioViews(wasmExports.memory, monoF32);",
      "  try {",
      "    const monoBuf = new Int16Array(64);",
      "    for (let i = 0; i < 64; i++) {",
      "      monoViews.input[i] = 0.5;",
      "      monoBuf[i] = Math.round(0.5 * 32767);",
      "    }",
      "    monoPcm.processMono(monoBuf);",
      "    monoF32.processMonoF32InPlace(64);",
      "    for (let i = 0; i < 64; i++) {",
      "      assert.equal(monoViews.input[i], Math.fround(monoBuf[i] / 32768));",
      "    }",
      "    monoPcm.beginDrain();",
      "    monoF32.beginDrain();",
      "    assert.equal(monoF32.drainMonoF32InPlace(64), monoPcm.drainMono(monoBuf));",
      "    for (let i = 0; i < 64; i++) {",
      "      assert.equal(monoViews.input[i], Math.fround(monoBuf[i] / 32768));",
      "    }",
      "  } finally {",
      "    monoViews.dispose();",
      "    monoPcm.free();",
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
      "  f32Processor.free();",
      "}",
      "",
    ].join("\n"),
  );

  execFileSync(process.execPath, [checkPath, packageName], {
    cwd: consumerDir,
    stdio: "inherit",
  });
}

/** Check TypeScript resolution for the root entry and the `./wasm` entry. */
function verifyTypeDeclarations(consumerDir, packageName) {
  const checkTsPath = path.join(consumerDir, "check.ts");
  const pkgLiteral = JSON.stringify(packageName);
  const wasmLiteral = JSON.stringify(`${packageName}/wasm`);
  fs.writeFileSync(
    checkTsPath,
    [
      "import loadTunelet, {",
      "  defaultConfig,",
      "  initSync as rootInitSync,",
      "  loadTunelet as namedLoadTunelet,",
      "  Tunelet,",
      "  TuneletAudioViews,",
      "  type ProcessorConfig,",
      `} from ${pkgLiteral};`,
      "import init, {",
      "  initSync as wasmInitSync,",
      "  type InitInput,",
      "  type InitOutput,",
      `} from ${wasmLiteral};`,
      "",
      "export async function verifyTypes(source?: InitInput): Promise<void> {",
      "  const exportsFromRoot: InitOutput = await loadTunelet(source);",
      "  const exportsFromNamed: InitOutput = await namedLoadTunelet(source);",
      "  const exportsFromWasm: InitOutput = await init(source);",
      "  const config: ProcessorConfig = defaultConfig();",
      "  const processor = new Tunelet(48000, 128, config);",
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
      "  const input = new Int16Array(128);",
      "  const left = new Int16Array(128);",
      "  const right = new Int16Array(128);",
      "  processor.process(input, left, right);",
      "  views.dispose();",
      "  processor.free();",
      "  void ptrs;",
      "  void stereoOut;",
      "  void stereoTail;",
      "  void monoTail;",
      "  void rootInitSync;",
      "  void wasmInitSync;",
      "  void exportsFromRoot;",
      "  void exportsFromNamed;",
      "  void exportsFromWasm;",
      "}",
      "",
    ].join("\n"),
  );

  const tscBin = path.join(
    repoRoot,
    "node_modules",
    "typescript",
    "bin",
    "tsc",
  );
  execFileSync(
    process.execPath,
    [
      tscBin,
      "--noEmit",
      "--module",
      "NodeNext",
      "--moduleResolution",
      "NodeNext",
      "--target",
      "ES2020",
      "--lib",
      "ES2020,ESNext.Disposable,DOM",
      "--strict",
      checkTsPath,
    ],
    { cwd: consumerDir, stdio: "inherit" },
  );
}

const packageName = readPackageName();
const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "tunelet-verify-pack-"));

try {
  const packDir = path.join(tempDir, "pack");
  const consumerDir = path.join(tempDir, "consumer");
  fs.mkdirSync(packDir);
  fs.mkdirSync(consumerDir);

  const relativeArchivePath = packArchive(packDir);
  verifyArchiveEntries(tempDir, relativeArchivePath);
  installArchive(tempDir, relativeArchivePath, consumerDir, packageName);
  verifyRuntimeImport(consumerDir, packageName);
  verifyTypeDeclarations(consumerDir, packageName);
} finally {
  fs.rmSync(tempDir, { recursive: true, force: true });
}
