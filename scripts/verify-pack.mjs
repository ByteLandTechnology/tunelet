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
      "  processor.process(input, left, right);",
      "} finally {",
      "  processor.free();",
      "}",
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
    "  syncSource: SyncInitInput,",
    "  optionalVoices: number | undefined,",
    "): Promise<InitOutput> {",
    "  const exportsFromRoot: InitOutput = await loadTunelet(source);",
    "  const exportsFromNamed: InitOutput = await namedLoadTunelet(source);",
    "  const exportsFromWasm: InitOutput = await init(source);",
    "  const syncFromRoot: InitOutput = rootInitSync({ module: syncSource });",
    "  const syncFromWasm: InitOutput = wasmInitSync(syncSource);",
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
    "  processor.processMono(input, [",
    '    { offset: 0, event: { type: "resynthesis", value: resynthesisUpdate } },',
    "  ]);",
    "  processor.process(input, left, right);",
    "  processor.free();",
    "  void exportsFromNamed;",
    "  void exportsFromWasm;",
    "  void syncFromRoot;",
    "  void syncFromWasm;",
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
