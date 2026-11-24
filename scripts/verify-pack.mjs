/**
 * This script packs the npm archive, checks its twelve files, verifies
 * bidirectional parity with `pkg/tunelet.d.ts`, runs `loadTunelet()` and
 * bundler checks, and compiles four consumer TypeScript profiles.
 */
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";

const EXPECTED_ARCHIVE_PATHS = [
  "package/LICENSE",
  "package/README.md",
  "package/dist/index.d.ts",
  "package/dist/index.js",
  "package/dist/types.d.ts",
  "package/dist/types.js",
  "package/dist/wasm.d.ts",
  "package/dist/wasm.js",
  "package/package.json",
  "package/pkg/tunelet.d.ts",
  "package/pkg/tunelet.js",
  "package/pkg/tunelet_bg.wasm",
];

const FORBIDDEN_DECLARATION_TOKENS = [
  "../pkg/tunelet.js",
  "../pkg/tunelet.d.ts",
  "Symbol.dispose",
  "RequestInfo",
  "BufferSource",
  "WebAssembly",
  "__wbg_",
  "__wbindgen_",
  "declare global",
  "/// <reference",
];

const PACKAGE_NAME_PATTERN =
  /^(?:@[a-z0-9-~][a-z0-9-._~]*\/)?[a-z0-9-~][a-z0-9-._~]*$/;

const repoRoot = fileURLToPath(new URL("..", import.meta.url));
const tscBin = path.join(repoRoot, "node_modules", "typescript", "bin", "tsc");

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

/** Convert a filesystem path to a POSIX relative module specifier. */
function toRelativeModuleSpecifier(fromDir, targetPath) {
  const relativePath = path
    .relative(fromDir, targetPath)
    .split(path.sep)
    .join("/");
  return relativePath.startsWith(".") ? relativePath : `./${relativePath}`;
}

/** Run `tsc -p <tsconfigPath>` and fail on any compiler diagnostic. */
function runTscProject(tsconfigPath, cwd) {
  execFileSync(process.execPath, [tscBin, "-p", tsconfigPath], {
    cwd,
    stdio: "inherit",
  });
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

/** Check that the archive contains only the twelve expected files. */
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

  return installedDir;
}

/** Check that public `dist/*.d.ts` files expose no generated or host leaks. */
function verifyDeclarationPortability(installedDir) {
  for (const relativeFile of [
    "dist/index.d.ts",
    "dist/wasm.d.ts",
    "dist/types.d.ts",
  ]) {
    const content = fs.readFileSync(
      path.join(installedDir, relativeFile),
      "utf8",
    );
    for (const token of FORBIDDEN_DECLARATION_TOKENS) {
      assert.equal(
        content.includes(token),
        false,
        `Unexpected token "${token}" in ${relativeFile}`,
      );
    }
  }
}

/** Check bidirectional assignability between `src/types.ts` and `pkg/tunelet.d.ts`. */
function verifyTypeParity(tempDir) {
  const parityDir = path.join(tempDir, "parity");
  fs.mkdirSync(parityDir);

  fs.writeFileSync(
    path.join(parityDir, "package.json"),
    JSON.stringify({
      name: "tunelet-type-parity",
      private: true,
      type: "module",
    }),
  );

  const srcTypesSpec = toRelativeModuleSpecifier(
    parityDir,
    path.join(repoRoot, "src", "types.ts"),
  );
  const srcWasmSpec = toRelativeModuleSpecifier(
    parityDir,
    path.join(repoRoot, "src", "wasm.ts"),
  );
  const pkgSpec = toRelativeModuleSpecifier(
    parityDir,
    path.join(repoRoot, "pkg", "tunelet.js"),
  );

  const parityTsPath = path.join(parityDir, "check-parity.ts");
  fs.writeFileSync(
    parityTsPath,
    [
      `import type * as Curated from ${JSON.stringify(srcTypesSpec)};`,
      `import { defaultConfig as curatedDefaultConfig } from ${JSON.stringify(srcWasmSpec)};`,
      `import type * as Pkg from ${JSON.stringify(pkgSpec)};`,
      `import { defaultConfig as pkgDefaultConfig } from ${JSON.stringify(pkgSpec)};`,
      "",
      "type AssertTrue<T extends true> = T;",
      "type IsAssignable<From, To> = [From] extends [To] ? true : false;",
      "type RequiredKeys<T> = {",
      "  [K in keyof T]-?: {} extends Pick<T, K> ? never : K;",
      "}[keyof T];",
      "type ExactObjectParity<A, B> = [",
      "  IsAssignable<A, B>,",
      "  IsAssignable<B, A>,",
      "  IsAssignable<Exclude<keyof A, keyof B>, never>,",
      "  IsAssignable<Exclude<keyof B, keyof A>, never>,",
      "  IsAssignable<Exclude<RequiredKeys<A>, RequiredKeys<B>>, never>,",
      "  IsAssignable<Exclude<RequiredKeys<B>, RequiredKeys<A>>, never>,",
      "] extends [true, true, true, true, true, true]",
      "  ? true",
      "  : false;",
      "type ExactTypeParity<A, B> = [",
      "  IsAssignable<A, B>,",
      "  IsAssignable<B, A>,",
      "] extends [true, true]",
      "  ? true",
      "  : false;",
      "",
      "export type ParityChecks = [",
      "  AssertTrue<ExactObjectParity<Curated.ProcessorConfig, Pkg.ProcessorConfig>>,",
      "  AssertTrue<ExactObjectParity<Curated.PunchConfig, Pkg.PunchConfig>>,",
      "  AssertTrue<ExactObjectParity<Curated.WarmConfig, Pkg.WarmConfig>>,",
      "  AssertTrue<ExactObjectParity<Curated.ResynthesisConfig, Pkg.ResynthesisConfig>>,",
      "  AssertTrue<ExactObjectParity<Curated.PitchConfig, Pkg.PitchConfig>>,",
      "  AssertTrue<ExactObjectParity<Curated.CorrectionConfig, Pkg.CorrectionConfig>>,",
      "  AssertTrue<ExactObjectParity<Curated.FormantConfig, Pkg.FormantConfig>>,",
      "  AssertTrue<ExactObjectParity<Curated.HarmonyConfig, Pkg.HarmonyConfig>>,",
      "  AssertTrue<ExactObjectParity<Curated.AlienConfig, Pkg.AlienConfig>>,",
      "  AssertTrue<ExactObjectParity<Curated.ReverbConfig, Pkg.ReverbConfig>>,",
      "  AssertTrue<ExactObjectParity<Curated.Feedback, Pkg.Feedback>>,",
      "  AssertTrue<ExactObjectParity<Curated.Telemetry, Pkg.Telemetry>>,",
      "  AssertTrue<ExactObjectParity<Curated.TimedEvent, Pkg.TimedEvent>>,",
      "  AssertTrue<ExactTypeParity<Curated.ControlEvent, Pkg.ControlEvent>>,",
      "  AssertTrue<ExactTypeParity<Curated.MidiEvent, Pkg.MidiEvent>>,",
      "  AssertTrue<ExactTypeParity<Curated.BlockEvents, Pkg.BlockEvents>>,",
      "  AssertTrue<ExactTypeParity<Curated.NoteMask, Pkg.NoteMask>>,",
      "  AssertTrue<ExactTypeParity<Curated.MultiVoiceMode, Pkg.MultiVoiceMode>>,",
      "  AssertTrue<ExactTypeParity<Curated.PitchClass, Pkg.PitchClass>>,",
      "  AssertTrue<ExactTypeParity<Curated.CorrectionScale, Pkg.CorrectionScale>>,",
      "  AssertTrue<ExactTypeParity<Curated.HarmonySource, Pkg.HarmonySource>>,",
      "  AssertTrue<ExactTypeParity<Curated.HarmonyScale, Pkg.HarmonyScale>>,",
      "  AssertTrue<ExactTypeParity<Curated.ChordName, Pkg.ChordName>>,",
      "  AssertTrue<ExactTypeParity<Curated.ChordDegree, Pkg.ChordDegree>>,",
      "  AssertTrue<ExactTypeParity<Curated.Inversion, Pkg.Inversion>>,",
      "  AssertTrue<ExactTypeParity<Curated.ReverbMode, Pkg.ReverbMode>>,",
      "  AssertTrue<ExactObjectParity<Curated.Tunelet, Omit<Pkg.Tunelet, typeof Symbol.dispose>>>,",
      "  AssertTrue<ExactTypeParity<ConstructorParameters<Curated.TuneletConstructor>, ConstructorParameters<typeof Pkg.Tunelet>>>,",
      "  AssertTrue<ExactTypeParity<typeof curatedDefaultConfig, typeof pkgDefaultConfig>>,",
      "  AssertTrue<IsAssignable<Pkg.InitInput, Curated.WasmInitInput>>,",
      "  AssertTrue<IsAssignable<Pkg.SyncInitInput, Curated.WasmSyncInitInput>>,",
      "  AssertTrue<IsAssignable<Pkg.InitOutput, Curated.WasmInitOutput>>,",
      "];",
      "",
    ].join("\n"),
  );

  const tsconfigPath = path.join(parityDir, "tsconfig.json");
  fs.writeFileSync(
    tsconfigPath,
    JSON.stringify({
      compilerOptions: {
        target: "ES2020",
        lib: ["ES2020", "ESNext.Disposable", "DOM"],
        module: "NodeNext",
        moduleResolution: "NodeNext",
        allowImportingTsExtensions: true,
        types: [],
        strict: true,
        exactOptionalPropertyTypes: true,
        skipLibCheck: false,
        noEmit: true,
      },
      files: [parityTsPath],
    }),
  );

  runTscProject(tsconfigPath, parityDir);
}

/** Check runtime imports, `{ voices: undefined }`, `.wasm` subpaths and `file:` URLs. */
function verifyRuntimeImport(consumerDir, installedDir, packageName) {
  const expectedWasmUrl = pathToFileURL(
    path.join(installedDir, "pkg", "tunelet_bg.wasm"),
  ).href;

  const checkPath = path.join(consumerDir, "check.mjs");
  fs.writeFileSync(
    checkPath,
    [
      'import assert from "node:assert/strict";',
      "",
      "const [packageName, expectedWasmUrl] = process.argv.slice(2);",
      "const pkgWasmUrl = import.meta.resolve(`${packageName}/pkg/tunelet_bg.wasm`);",
      "const shortWasmUrl = import.meta.resolve(`${packageName}/tunelet_bg.wasm`);",
      "assert.equal(pkgWasmUrl, expectedWasmUrl);",
      "assert.equal(shortWasmUrl, expectedWasmUrl);",
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
      "assert.equal(wasmModule.Tunelet, rootModule.Tunelet);",
      "assert.equal(wasmModule.defaultConfig, rootModule.defaultConfig);",
      "",
      "const wasmExports = await rootModule.loadTunelet();",
      "assert.ok(wasmExports);",
      "assert.ok(wasmExports.memory);",
      "",
      "function withHarmonyVoices(config, voices) {",
      "  return {",
      "    ...config,",
      "    resynthesis: {",
      "      ...config.resynthesis,",
      "      harmony: {",
      "        ...config.resynthesis.harmony,",
      "        voices,",
      "      },",
      "    },",
      "  };",
      "}",
      "",
      "const baseConfig = rootModule.defaultConfig();",
      "const processor = new rootModule.Tunelet(",
      "  48000,",
      "  128,",
      "  withHarmonyVoices(baseConfig, undefined),",
      ");",
      "try {",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 4);",
      "",
      "  processor.setConfig(withHarmonyVoices(processor.config(), 1));",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 1);",
      "",
      "  processor.setConfig(withHarmonyVoices(processor.config(), undefined));",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 4);",
      "",
      "  const input = new Int16Array(128);",
      "  const left = new Int16Array(128);",
      "  const right = new Int16Array(128);",
      "  processor.process(input, left, right);",
      "",
      "  processor.setConfig(withHarmonyVoices(processor.config(), 2));",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 2);",
      "  const mono = new Int16Array(128);",
      "  processor.processMono(mono, [",
      "    {",
      "      offset: 0,",
      "      event: {",
      '        type: "resynthesis",',
      "        value: withHarmonyVoices(processor.config(), undefined).resynthesis,",
      "      },",
      "    },",
      "  ]);",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 4);",
      "",
      "  processor.setConfig(withHarmonyVoices(processor.config(), 3));",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 3);",
      "  processor.processMono(mono, [",
      "    {",
      "      offset: 0,",
      "      event: {",
      '        type: "configure",',
      "        value: withHarmonyVoices(processor.config(), undefined),",
      "      },",
      "    },",
      "  ]);",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 4);",
      "} finally {",
      "  processor.free();",
      "}",
      "",
    ].join("\n"),
  );

  execFileSync(process.execPath, [checkPath, packageName, expectedWasmUrl], {
    cwd: consumerDir,
    stdio: "inherit",
  });

  const fileUrlCheckPath = path.join(consumerDir, "check-file-source.mjs");
  fs.writeFileSync(
    fileUrlCheckPath,
    [
      'import assert from "node:assert/strict";',
      'import vm from "node:vm";',
      "",
      "const [packageName, mode] = process.argv.slice(2);",
      "const { loadTunelet, Tunelet } = await import(packageName);",
      "",
      "let source;",
      'if (mode === "pkg-subpath-string") {',
      "  source = import.meta.resolve(`${packageName}/pkg/tunelet_bg.wasm`);",
      '  assert.equal(typeof source, "string");',
      '  assert.equal(source.startsWith("file:"), true);',
      '} else if (mode === "short-subpath-string") {',
      "  source = import.meta.resolve(`${packageName}/tunelet_bg.wasm`);",
      '  assert.equal(typeof source, "string");',
      '  assert.equal(source.startsWith("file:"), true);',
      '} else if (mode === "cross-realm-url") {',
      "  const href = import.meta.resolve(`${packageName}/pkg/tunelet_bg.wasm`);",
      '  source = vm.runInNewContext(\'({ protocol: "file:", href })\', { href });',
      "  assert.equal(source instanceof URL, false);",
      "} else {",
      "  throw new Error(`Unknown mode: ${mode}`);",
      "}",
      "",
      "const exportsOut = await loadTunelet(source);",
      "assert.ok(exportsOut.memory);",
      "const processor = new Tunelet(48000, 64);",
      "try {",
      "  const mono = new Int16Array(64);",
      "  processor.processMono(mono);",
      "} finally {",
      "  processor.free();",
      "}",
      "",
    ].join("\n"),
  );

  for (const mode of [
    "pkg-subpath-string",
    "short-subpath-string",
    "cross-realm-url",
  ]) {
    execFileSync(process.execPath, [fileUrlCheckPath, packageName, mode], {
      cwd: consumerDir,
      stdio: "inherit",
    });
  }
}

/** Check that `esbuild` resolves `.`, `./wasm`, and both `.wasm` subpaths. */
async function verifyBundlerResolution(consumerDir, installedDir, packageName) {
  const bundleDir = path.join(consumerDir, "bundle");
  const outDir = path.join(bundleDir, "out");
  fs.mkdirSync(bundleDir);

  const entryPath = path.join(bundleDir, "entry.js");
  fs.writeFileSync(
    entryPath,
    [
      `import loadTunelet, { Tunelet, defaultConfig } from ${JSON.stringify(packageName)};`,
      `import initWasm, { initSync } from ${JSON.stringify(`${packageName}/wasm`)};`,
      `import pkgWasmAsset from ${JSON.stringify(`${packageName}/pkg/tunelet_bg.wasm`)};`,
      `import shortWasmAsset from ${JSON.stringify(`${packageName}/tunelet_bg.wasm`)};`,
      "",
      "export { loadTunelet, Tunelet, defaultConfig, initWasm, initSync, pkgWasmAsset, shortWasmAsset };",
      "",
    ].join("\n"),
  );

  const result = await esbuild.build({
    absWorkingDir: consumerDir,
    entryPoints: [entryPath],
    bundle: true,
    format: "esm",
    platform: "browser",
    outdir: outDir,
    loader: { ".wasm": "file" },
    metafile: true,
    logLevel: "silent",
  });

  const expectedWasmReal = fs.realpathSync(
    path.join(installedDir, "pkg", "tunelet_bg.wasm"),
  );
  const entryImports = result.metafile.inputs["bundle/entry.js"].imports.map(
    (entry) => fs.realpathSync(path.resolve(consumerDir, entry.path)),
  );
  const wasmImportCount = entryImports.filter(
    (resolvedPath) => resolvedPath === expectedWasmReal,
  ).length;
  assert.equal(
    wasmImportCount,
    2,
    "Expected esbuild to resolve both .wasm subpaths to pkg/tunelet_bg.wasm",
  );

  const emittedFiles = fs.readdirSync(outDir);
  assert.ok(
    emittedFiles.some((file) => file.endsWith(".wasm")),
    "Expected esbuild to emit a .wasm asset",
  );
  assert.ok(
    emittedFiles.includes("entry.js"),
    "Expected esbuild to emit entry.js",
  );
}

/** Check four consumer `tsc --strict --exactOptionalPropertyTypes` profiles. */
function verifyTypeDeclarations(consumerDir, packageName) {
  const pkgLiteral = JSON.stringify(packageName);
  const wasmLiteral = JSON.stringify(`${packageName}/wasm`);
  const nodeTypesRoot = path.join(repoRoot, "node_modules", "@types");

  const sharedPortableSource = [
    "import loadTunelet, {",
    "  defaultConfig,",
    "  initSync as rootInitSync,",
    "  loadTunelet as namedLoadTunelet,",
    "  Tunelet,",
    "  type Feedback,",
    "  type HarmonyConfig,",
    "  type InitInput,",
    "  type InitOutput,",
    "  type ProcessorConfig,",
    "  type SyncInitInput,",
    "  type Telemetry,",
    "  type TimedEvent,",
    "  type WasmInitInput,",
    "  type WasmInitOutput,",
    "  type WasmSyncInitInput,",
    `} from ${pkgLiteral};`,
    "import init, {",
    "  initSync as wasmInitSync,",
    "  Tunelet as WasmTunelet,",
    "  type Tunelet as WasmTuneletType,",
    "  type WasmInitInput as SubpathInitInput,",
    "  type WasmInitOutput as SubpathInitOutput,",
    "} from " + wasmLiteral + ";",
    "",
    "export async function verifyPortable(",
    "  source?: WasmInitInput,",
    "  compatSource?: InitInput,",
    "  syncSource?: WasmSyncInitInput,",
    "  compatSync?: SyncInitInput,",
    "): Promise<void> {",
    "  const exportsFromRoot: WasmInitOutput = await loadTunelet(source);",
    "  const exportsFromNamed: InitOutput = await namedLoadTunelet(compatSource);",
    "  const exportsFromFileString: WasmInitOutput = await loadTunelet(",
    '    "file:///tmp/tunelet_bg.wasm",',
    "  );",
    "  const exportsFromUrlLike: WasmInitOutput = await loadTunelet({",
    '    href: "file:///tmp/tunelet_bg.wasm",',
    '    protocol: "file:",',
    "  });",
    "  const subpathInput: SubpathInitInput | undefined = source;",
    "  const exportsFromWasm: SubpathInitOutput = await init(subpathInput);",
    "  if (syncSource !== undefined) {",
    "    const syncOut: WasmInitOutput = rootInitSync({ module: syncSource });",
    "    void syncOut;",
    "  }",
    "  if (compatSync !== undefined) {",
    "    const syncOut2: InitOutput = wasmInitSync(compatSync);",
    "    void syncOut2;",
    "  }",
    "  const config: ProcessorConfig = defaultConfig();",
    "  const harmonyWithUndefined: HarmonyConfig = {",
    "    ...config.resynthesis.harmony,",
    "    voices: undefined,",
    "  };",
    "  const nextConfig: ProcessorConfig = {",
    "    ...config,",
    "    resynthesis: {",
    "      ...config.resynthesis,",
    "      harmony: harmonyWithUndefined,",
    "    },",
    "  };",
    "  const processor: Tunelet = new Tunelet(48000, 128, nextConfig);",
    "  const wasmProcessor: WasmTuneletType = new WasmTunelet(48000, 128, nextConfig);",
    "  const input = new Int16Array(128);",
    "  const left = new Int16Array(128);",
    "  const right = new Int16Array(128);",
    "  const events: TimedEvent[] = [",
    "    {",
    "      offset: 0,",
    "      event: {",
    '        type: "resynthesis",',
    "        value: nextConfig.resynthesis,",
    "      },",
    "    },",
    "  ];",
    "  processor.process(input, left, right, events);",
    "  processor.processMono(input, events);",
    "  const feedback: Feedback = processor.feedback();",
    "  const telemetry: Telemetry = processor.telemetry();",
    "  processor.free();",
    "  wasmProcessor.free();",
    "  void exportsFromRoot;",
    "  void exportsFromNamed;",
    "  void exportsFromFileString;",
    "  void exportsFromUrlLike;",
    "  void exportsFromWasm;",
    "  void feedback;",
    "  void telemetry;",
    "}",
    "",
  ].join("\n");

  const profiles = [
    {
      name: "node",
      compilerOptions: {
        target: "ES2022",
        lib: ["ES2023"],
        module: "NodeNext",
        moduleResolution: "NodeNext",
        typeRoots: [nodeTypesRoot],
        types: ["node"],
        strict: true,
        exactOptionalPropertyTypes: true,
        skipLibCheck: false,
        noEmit: true,
      },
      source: [
        sharedPortableSource,
        "export async function verifyNodeHost(): Promise<void> {",
        '  const fileUrl = new URL("file:///tmp/tunelet_bg.wasm");',
        "  await loadTunelet(fileUrl);",
        "  await loadTunelet(Buffer.alloc(8));",
        "  wasmInitSync({ module: Buffer.alloc(8) });",
        "}",
        "",
      ].join("\n"),
    },
    {
      name: "es2023",
      compilerOptions: {
        target: "ES2023",
        lib: ["ES2023"],
        module: "NodeNext",
        moduleResolution: "NodeNext",
        types: [],
        strict: true,
        exactOptionalPropertyTypes: true,
        skipLibCheck: false,
        noEmit: true,
      },
      source: [
        sharedPortableSource,
        "export async function verifyPureEs2023(): Promise<void> {",
        "  await loadTunelet(new Uint8Array(8));",
        "  wasmInitSync({ module: new ArrayBuffer(8) });",
        "}",
        "",
      ].join("\n"),
    },
    {
      name: "dom",
      compilerOptions: {
        target: "ES2022",
        lib: ["ES2022", "DOM"],
        module: "ES2022",
        moduleResolution: "Bundler",
        types: [],
        strict: true,
        exactOptionalPropertyTypes: true,
        skipLibCheck: false,
        noEmit: true,
      },
      source: [
        sharedPortableSource,
        "export async function verifyDomHost(",
        "  compiled: WebAssembly.Module,",
        "): Promise<void> {",
        '  await loadTunelet(fetch("/tunelet_bg.wasm"));',
        '  await loadTunelet(new Request("/tunelet_bg.wasm"));',
        '  await loadTunelet(new URL("/tunelet_bg.wasm", location.href));',
        "  await loadTunelet(compiled);",
        "  wasmInitSync({ module: compiled });",
        "}",
        "",
      ].join("\n"),
    },
    {
      name: "webworker",
      compilerOptions: {
        target: "ES2022",
        lib: ["ES2022", "WebWorker"],
        module: "ES2022",
        moduleResolution: "Bundler",
        types: [],
        strict: true,
        exactOptionalPropertyTypes: true,
        skipLibCheck: false,
        noEmit: true,
      },
      source: [
        sharedPortableSource,
        "export async function verifyWebWorkerHost(",
        "  compiled: WebAssembly.Module,",
        "): Promise<void> {",
        "  const out: SubpathInitOutput = wasmInitSync({ module: compiled });",
        '  await init(fetch("/tunelet_bg.wasm"));',
        "  const workletEngine: WasmTuneletType = new WasmTunelet(48000, 128);",
        "  workletEngine.free();",
        "  void out;",
        "}",
        "",
      ].join("\n"),
    },
  ];

  const profilesRoot = path.join(consumerDir, "profiles");
  fs.mkdirSync(profilesRoot);

  for (const profile of profiles) {
    const profileDir = path.join(profilesRoot, profile.name);
    fs.mkdirSync(profileDir);

    const checkTsPath = path.join(profileDir, "check.ts");
    const tsconfigPath = path.join(profileDir, "tsconfig.json");

    fs.writeFileSync(checkTsPath, profile.source);
    fs.writeFileSync(
      tsconfigPath,
      JSON.stringify({
        compilerOptions: profile.compilerOptions,
        files: [checkTsPath],
      }),
    );

    runTscProject(tsconfigPath, profileDir);
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
  const installedDir = installArchive(
    tempDir,
    relativeArchivePath,
    consumerDir,
    packageName,
  );
  verifyDeclarationPortability(installedDir);
  verifyTypeParity(tempDir);
  verifyRuntimeImport(consumerDir, installedDir, packageName);
  await verifyBundlerResolution(consumerDir, installedDir, packageName);
  verifyTypeDeclarations(consumerDir, packageName);
} finally {
  fs.rmSync(tempDir, { recursive: true, force: true });
}
