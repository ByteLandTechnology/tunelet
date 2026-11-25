/**
 * This script builds the package one time and packs `tunelet` and
 * `@bytelandtechnology/tunelet`. For each archive, it checks the eight files,
 * `loadTunelet()`, the HTTP Wasm loaders and the `NodeNext` TypeScript
 * declarations.
 *
 * If you give `--out-dir <dir>`, the script writes both `.tgz` archives and
 * `SHA256SUMS.txt` to `<dir>` after all checks succeed.
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

/** Read the optional `--out-dir` flag from `process.argv`. */
function parseCliOptions(argv) {
  const { values, positionals } = parseArgs({
    args: argv,
    options: {
      "out-dir": { type: "string" },
    },
    strict: true,
  });
  assert.equal(
    positionals.length,
    0,
    `Unexpected positional arguments: ${positionals.join(" ")}`,
  );
  const rawOutDir = values["out-dir"];
  if (rawOutDir === undefined) {
    return { outDir: undefined };
  }
  assert.ok(
    rawOutDir.trim().length > 0 && !rawOutDir.startsWith("-"),
    "--out-dir requires a directory path",
  );
  return { outDir: path.resolve(process.cwd(), rawOutDir) };
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

/** Import the installed package in Node.js and check Wasm paths and configs. */
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
      'assert.equal(typeof rootModule.defaultConfig, "function");',
      'assert.equal(typeof wasmModule.default, "function");',
      'assert.equal(typeof wasmModule.initSync, "function");',
      "",
      "const wasmExports = await rootModule.loadTunelet();",
      "assert.ok(wasmExports);",
      "",
      "const initialConfig = rootModule.defaultConfig();",
      "initialConfig.resynthesis.harmony.voices = undefined;",
      "const processor = new rootModule.Tunelet(48000, 128, initialConfig);",
      "try {",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 4);",
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
      "  const mono = new Int16Array(128);",
      "  const eventResynthesis = processor.config().resynthesis;",
      "  eventResynthesis.harmony.voices = undefined;",
      "  processor.processMono(mono, [",
      '    { offset: 0, event: { type: "resynthesis", value: eventResynthesis } },',
      "  ]);",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 4);",
      "",
      "  const input = new Int16Array(128);",
      "  const left = new Int16Array(128);",
      "  const right = new Int16Array(128);",
      "  const midi = new Uint32Array([0, (0 << 24) | (0x90 << 16) | (60 << 8) | 100]);",
      "  processor.process(input, left, right);",
      "  processor.processWithMidi(input, left, right, midi);",
      "  processor.processMonoWithMidi(input, midi);",
      "} finally {",
      "  processor.free();",
      "}",
      "",
      'assert.equal(typeof Symbol.dispose, "symbol");',
      "const disposableProcessor = new rootModule.Tunelet(48000, 128);",
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
    "  type ProcessorConfig,",
    "  type ResynthesisConfig,",
    "  type SyncInitInput as RootSyncInitInput,",
    `} from ${pkgLiteral};`,
    "import init, {",
    "  initSync as wasmInitSync,",
    "  type InitInput,",
    "  type InitOutput,",
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
    "    disposableProcessor.reset();",
    "  }",
    "  const processor = new Tunelet(48000, 128, configWithUndefined);",
    "  processor.setConfig(configWithUndefined);",
    "  const resynthesisUpdate: ResynthesisConfig = {",
    "    ...configWithUndefined.resynthesis,",
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
    "  processor.process(input, left, right);",
    "  processor.processWithMidi(input, left, right, midi);",
    "  processor.processMonoWithMidi(input, midi);",
    "  processor.free();",
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
        "  voices?: number,",
        "): Promise<WebAssembly.Memory> {",
        "  await loadTunelet(reqInfo);",
        "  await loadTunelet(req);",
        "  await loadTunelet(res);",
        '  await loadTunelet(fetch("/tunelet_bg.wasm"));',
        "  await loadTunelet(url);",
        "  await loadTunelet(bufferSource);",
        "  const out = await verifySharedApi(wasmModule, wasmModule, voices);",
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
        "  voices?: number,",
        "): Promise<WebAssembly.Memory> {",
        "  await loadTunelet(reqInfo);",
        "  await loadTunelet(res);",
        '  await loadTunelet(fetch("/tunelet_bg.wasm"));',
        "  await loadTunelet(url);",
        "  await loadTunelet(bufferSource);",
        "  const out = await verifySharedApi(wasmModule, wasmModule, voices);",
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

const { outDir } = parseCliOptions(process.argv.slice(2));
const manifest = readPackageManifest();
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
