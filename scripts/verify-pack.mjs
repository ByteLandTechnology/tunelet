/**
 * Pack the npm archive and check its files, types and runtime loaders.
 */
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { verifyBrowserLoaders } from "./verify-browser.mjs";

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

const SCOPED_PACKAGE_NAME = "@bytelandtechnology/tunelet";

const repoRoot = fileURLToPath(new URL("..", import.meta.url));
const tscBin = path.join(repoRoot, "node_modules", "typescript", "bin", "tsc");
const nodeTypesRoot = path.join(repoRoot, "node_modules", "@types");

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
  manifestPackageName,
  targetPackageName,
) {
  fs.mkdirSync(consumerDir, { recursive: true });
  const extractDir = fs.mkdtempSync(path.join(tempDir, "extract-"));
  execFileSync(
    "tar",
    ["-xzf", relativeArchivePath, "-C", path.basename(extractDir)],
    { cwd: tempDir },
  );

  const installedDir = path.join(
    consumerDir,
    "node_modules",
    ...targetPackageName.split("/"),
  );
  fs.mkdirSync(path.dirname(installedDir), { recursive: true });
  fs.renameSync(path.join(extractDir, "package"), installedDir);
  fs.rmSync(extractDir, { recursive: true, force: true });

  for (const archivePath of EXPECTED_ARCHIVE_PATHS) {
    const relativeFile = archivePath.slice("package/".length);
    const fullPath = path.join(installedDir, relativeFile);
    const stat = fs.statSync(fullPath);
    assert.ok(stat.isFile(), `Expected file at ${relativeFile}`);
    assert.ok(stat.size > 0, `Expected non-empty file at ${relativeFile}`);
  }

  const packedMetadataPath = path.join(installedDir, "package.json");
  const packedMetadata = JSON.parse(
    fs.readFileSync(packedMetadataPath, "utf8"),
  );
  assert.equal(packedMetadata.name, manifestPackageName);
  if (targetPackageName !== manifestPackageName) {
    packedMetadata.name = targetPackageName;
    fs.writeFileSync(
      packedMetadataPath,
      `${JSON.stringify(packedMetadata, null, 2)}\n`,
    );
  }

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

/** Run `tsc --noEmit --strict` on a fixture without `skipLibCheck`. */
function runStrictTypecheck(fixtureDir, compilerOptions, checkLines) {
  assert.equal(
    "skipLibCheck" in compilerOptions,
    false,
    "Fixture compilerOptions must never set skipLibCheck",
  );
  assert.equal(
    compilerOptions.strict,
    true,
    "Fixture compilerOptions must enforce strict: true",
  );

  const tsconfigPath = path.join(fixtureDir, "tsconfig.json");
  fs.writeFileSync(
    tsconfigPath,
    JSON.stringify(
      {
        compilerOptions,
        include: ["check.ts"],
      },
      null,
      2,
    ),
  );
  fs.writeFileSync(path.join(fixtureDir, "check.ts"), checkLines.join("\n"));

  execFileSync(
    process.execPath,
    [tscBin, "--project", tsconfigPath, "--noEmit", "--strict"],
    { cwd: fixtureDir, stdio: "inherit" },
  );
}

/** Check the browser TypeScript fixture with DOM and without Node.js globals. */
function verifyBrowserFixture(fixtureDir, packageName) {
  const pkgLiteral = JSON.stringify(packageName);
  const wasmLiteral = JSON.stringify(`${packageName}/wasm`);
  runStrictTypecheck(
    fixtureDir,
    {
      target: "ES2020",
      lib: ["ES2020", "DOM"],
      module: "ESNext",
      moduleResolution: "Bundler",
      types: [],
      strict: true,
      noEmit: true,
    },
    [
      "import loadTunelet, {",
      "  defaultConfig,",
      "  initSync as rootInitSync,",
      "  loadTunelet as namedLoadTunelet,",
      "  Tunelet,",
      "  type ProcessorConfig,",
      `} from ${pkgLiteral};`,
      "import init, {",
      "  initSync as wasmInitSync,",
      "  type InitInput,",
      "  type InitOutput,",
      "  type SyncInitInput,",
      `} from ${wasmLiteral};`,
      "",
      "// @ts-expect-error Node.js globals must not leak into a browser project",
      "void process;",
      "",
      "export async function verifyBrowserTypes(",
      "  requestInfo: RequestInfo,",
      "  bufferSource: BufferSource,",
      "  response: Response,",
      "  wasmModule: WebAssembly.Module,",
      "  source?: InitInput,",
      "): Promise<void> {",
      "  const exportsDefault: InitOutput = await loadTunelet();",
      "  const exportsNamed: InitOutput = await namedLoadTunelet(source);",
      '  const exportsUrl: InitOutput = await loadTunelet(new URL("/pkg/tunelet_bg.wasm", window.location.href));',
      "  const exportsRequest: InitOutput = await loadTunelet(requestInfo);",
      "  const exportsBuffer: InitOutput = await loadTunelet(bufferSource);",
      "  const exportsResponse: InitOutput = await loadTunelet(response);",
      "  const exportsModulePromise: InitOutput = await loadTunelet(Promise.resolve(wasmModule));",
      "  const exportsWasm: InitOutput = await init({ module_or_path: new Uint8Array(8) });",
      "  const syncInput: SyncInitInput = bufferSource;",
      "  const syncRoot: InitOutput = rootInitSync({ module: syncInput });",
      "  const syncWasm: InitOutput = wasmInitSync({ module: wasmModule });",
      "  const config: ProcessorConfig = defaultConfig();",
      "  const processor = new Tunelet(48000, 128, config);",
      "  const input = new Int16Array(128);",
      "  const left = new Int16Array(128);",
      "  const right = new Int16Array(128);",
      "  processor.process(input, left, right);",
      "  processor[Symbol.dispose]();",
      "  processor.free();",
      "  void exportsDefault;",
      "  void exportsNamed;",
      "  void exportsUrl;",
      "  void exportsRequest;",
      "  void exportsBuffer;",
      "  void exportsResponse;",
      "  void exportsModulePromise;",
      "  void exportsWasm;",
      "  void syncRoot;",
      "  void syncWasm;",
      "}",
      "",
    ],
  );
}

/** Check the Web Worker TypeScript fixture without DOM or Node.js globals. */
function verifyWebWorkerFixture(fixtureDir, packageName) {
  const pkgLiteral = JSON.stringify(packageName);
  const wasmLiteral = JSON.stringify(`${packageName}/wasm`);
  runStrictTypecheck(
    fixtureDir,
    {
      target: "ES2020",
      lib: ["ES2020", "WebWorker"],
      module: "ESNext",
      moduleResolution: "Bundler",
      types: [],
      strict: true,
      noEmit: true,
    },
    [
      "import loadTunelet, {",
      "  defaultConfig,",
      "  initSync as rootInitSync,",
      "  Tunelet,",
      "  type ProcessorConfig,",
      `} from ${pkgLiteral};`,
      "import init, {",
      "  initSync as wasmInitSync,",
      "  type InitInput,",
      "  type InitOutput,",
      "  type SyncInitInput,",
      `} from ${wasmLiteral};`,
      "",
      "// @ts-expect-error DOM window must not exist in a Web Worker project",
      "void window;",
      "// @ts-expect-error DOM document must not exist in a Web Worker project",
      "void document;",
      "// @ts-expect-error Node.js globals must not leak into a Web Worker project",
      "void process;",
      "",
      "export async function verifyWorkerTypes(",
      "  requestInfo: RequestInfo,",
      "  bufferSource: BufferSource,",
      "  wasmModule: WebAssembly.Module,",
      "  source?: InitInput,",
      "): Promise<void> {",
      "  const exportsDefault: InitOutput = await loadTunelet();",
      "  const exportsSource: InitOutput = await loadTunelet(source);",
      '  const exportsUrl: InitOutput = await loadTunelet(new URL("/pkg/tunelet_bg.wasm", self.location.href));',
      "  const exportsRequest: InitOutput = await loadTunelet(requestInfo);",
      "  const exportsModulePromise: InitOutput = await loadTunelet(Promise.resolve(wasmModule));",
      "  const exportsWasm: InitOutput = await init({ module_or_path: bufferSource });",
      "  const syncInput: SyncInitInput = new Uint8Array(8);",
      "  const syncRoot: InitOutput = rootInitSync({ module: syncInput });",
      "  const syncWasm: InitOutput = wasmInitSync({ module: wasmModule });",
      "  const config: ProcessorConfig = defaultConfig();",
      "  const processor = new Tunelet(48000, 128, config);",
      "  const audio = new Int16Array(128);",
      "  processor.processMono(audio);",
      "  processor[Symbol.dispose]();",
      "  void exportsDefault;",
      "  void exportsSource;",
      "  void exportsUrl;",
      "  void exportsRequest;",
      "  void exportsModulePromise;",
      "  void exportsWasm;",
      "  void syncRoot;",
      "  void syncWasm;",
      "}",
      "",
    ],
  );
}

/** Check the strict NodeNext TypeScript fixture without DOM globals. */
function verifyNodeNextFixture(fixtureDir, packageName) {
  const pkgLiteral = JSON.stringify(packageName);
  const wasmLiteral = JSON.stringify(`${packageName}/wasm`);
  runStrictTypecheck(
    fixtureDir,
    {
      target: "ES2020",
      lib: ["ES2020"],
      module: "NodeNext",
      moduleResolution: "NodeNext",
      typeRoots: [nodeTypesRoot],
      types: ["node"],
      strict: true,
      noEmit: true,
    },
    [
      'import fs from "node:fs/promises";',
      'import { pathToFileURL } from "node:url";',
      "import loadTunelet, {",
      "  defaultConfig,",
      "  initSync as rootInitSync,",
      "  loadTunelet as namedLoadTunelet,",
      "  Tunelet,",
      "  type ProcessorConfig,",
      `} from ${pkgLiteral};`,
      "import init, {",
      "  initSync as wasmInitSync,",
      "  type InitInput,",
      "  type InitOutput,",
      "  type SyncInitInput,",
      `} from ${wasmLiteral};`,
      "",
      "// @ts-expect-error DOM window must not leak into a NodeNext project",
      "void window;",
      "// @ts-expect-error DOM document must not leak into a NodeNext project",
      "void document;",
      "",
      "export async function verifyNodeTypes(",
      "  modulePromise: Promise<WebAssembly.Module>,",
      "  wasmModule: WebAssembly.Module,",
      "  response: Response,",
      "  request: Request,",
      "  source?: InitInput,",
      "): Promise<void> {",
      '  const fileUrl: URL = pathToFileURL("./pkg/tunelet_bg.wasm");',
      '  const fileBuffer = await fs.readFile("./pkg/tunelet_bg.wasm");',
      "  const bytes = new Uint8Array(fileBuffer);",
      "  const exportsDefault: InitOutput = await loadTunelet();",
      "  const exportsNamed: InitOutput = await namedLoadTunelet(source);",
      "  const exportsFileUrl: InitOutput = await loadTunelet(fileUrl);",
      "  const exportsBytes: InitOutput = await loadTunelet(bytes);",
      "  const exportsBuffer: InitOutput = await loadTunelet(fileBuffer);",
      "  const exportsModulePromise: InitOutput = await loadTunelet(modulePromise);",
      "  const exportsResponse: InitOutput = await loadTunelet(response);",
      "  const exportsRequest: InitOutput = await loadTunelet(request);",
      "  const exportsWasm: InitOutput = await init({ module_or_path: bytes });",
      "  const syncInput: SyncInitInput = bytes;",
      "  const syncRoot: InitOutput = rootInitSync({ module: syncInput });",
      "  const syncWasm: InitOutput = wasmInitSync({ module: wasmModule });",
      "  const config: ProcessorConfig = defaultConfig();",
      "  const processor = new Tunelet(48000, 128, config);",
      "  const input = new Int16Array(128);",
      "  const left = new Int16Array(128);",
      "  const right = new Int16Array(128);",
      "  processor.process(input, left, right);",
      "  processor[Symbol.dispose]();",
      "  void exportsDefault;",
      "  void exportsNamed;",
      "  void exportsFileUrl;",
      "  void exportsBytes;",
      "  void exportsBuffer;",
      "  void exportsModulePromise;",
      "  void exportsResponse;",
      "  void exportsRequest;",
      "  void exportsWasm;",
      "  void syncRoot;",
      "  void syncWasm;",
      "}",
      "",
    ],
  );
}

/** Run isolated Node.js checks for file:, Uint8Array, Module and initSync. */
function verifyNodeRuntime(consumerDir, installedDir, packageName) {
  const checkPath = path.join(consumerDir, "check.mjs");
  const wasmPath = path.join(installedDir, "pkg", "tunelet_bg.wasm");

  fs.writeFileSync(
    checkPath,
    [
      'import assert from "node:assert/strict";',
      'import fs from "node:fs";',
      'import { pathToFileURL } from "node:url";',
      "",
      "const [packageName, wasmPath, mode] = process.argv.slice(2);",
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
      "if (mode === 'retry-and-default') {",
      "  await assert.rejects(",
      '    rootModule.loadTunelet(new URL("./absent.wasm", import.meta.url)),',
      "    { code: 'ENOENT' },",
      "  );",
      "  const firstExports = await rootModule.loadTunelet();",
      "  assert.ok(firstExports);",
      "  const cachedExports = await rootModule.loadTunelet(",
      '    new URL("./absent.wasm", import.meta.url),',
      "  );",
      "  assert.equal(cachedExports, firstExports);",
      "} else if (mode === 'file-url') {",
      "  const exportsFromUrl = await rootModule.loadTunelet(pathToFileURL(wasmPath));",
      "  assert.ok(exportsFromUrl);",
      "} else if (mode === 'uint8array') {",
      "  const bytes = new Uint8Array(fs.readFileSync(wasmPath));",
      "  const exportsFromBytes = await rootModule.loadTunelet(bytes);",
      "  assert.ok(exportsFromBytes);",
      "} else if (mode === 'module-promise') {",
      "  const bytes = new Uint8Array(fs.readFileSync(wasmPath));",
      "  const modulePromise = WebAssembly.compile(bytes);",
      "  assert.ok(modulePromise instanceof Promise);",
      "  const exportsFromModule = await rootModule.loadTunelet(modulePromise);",
      "  assert.ok(exportsFromModule);",
      "} else if (mode === 'init-sync-root') {",
      "  const bytes = new Uint8Array(fs.readFileSync(wasmPath));",
      "  const syncExports = rootModule.initSync({ module: bytes });",
      "  assert.ok(syncExports);",
      "  assert.equal(rootModule.initSync({ module: bytes }), syncExports);",
      "} else if (mode === 'init-sync-wasm') {",
      "  const compiled = new WebAssembly.Module(fs.readFileSync(wasmPath));",
      "  const syncExports = wasmModule.initSync({ module: compiled });",
      "  assert.ok(syncExports);",
      "} else {",
      "  throw new Error(`Unknown runtime check mode: ${mode}`);",
      "}",
      "",
      "const blockFrames = 128;",
      "const blockCount = 20;",
      "const period = 218;",
      "const config = rootModule.defaultConfig();",
      "const processor = new rootModule.Tunelet(48000, blockFrames, config);",
      "let hash = 0x811c9dc5;",
      "let nonZeroSamples = 0;",
      "let latencyFrames = 0;",
      'let processedFrames = "0";',
      "try {",
      "  latencyFrames = processor.latencyFrames;",
      "  const input = new Int16Array(blockFrames);",
      "  const left = new Int16Array(blockFrames);",
      "  const right = new Int16Array(blockFrames);",
      "  for (let block = 0; block < blockCount; block += 1) {",
      "    const baseFrame = block * blockFrames;",
      "    for (let i = 0; i < blockFrames; i += 1) {",
      "      const phase = (baseFrame + i) % period;",
      "      input[i] = Math.trunc((phase * 24000) / period) - 12000;",
      "    }",
      "    processor.process(input, left, right);",
      "    for (let i = 0; i < blockFrames; i += 1) {",
      "      if (left[i] !== 0) {",
      "        nonZeroSamples += 1;",
      "      }",
      "      if (right[i] !== 0) {",
      "        nonZeroSamples += 1;",
      "      }",
      "      hash = Math.imul(hash ^ (left[i] & 0xffff), 0x01000193) >>> 0;",
      "      hash = Math.imul(hash ^ (right[i] & 0xffff), 0x01000193) >>> 0;",
      "    }",
      "  }",
      "  processedFrames = String(processor.feedback().processedFrames);",
      "} finally {",
      "  processor[Symbol.dispose]();",
      "}",
      "",
      "process.stdout.write(JSON.stringify({ hash, nonZeroSamples, latencyFrames, processedFrames }));",
      "",
    ].join("\n"),
  );

  const modes = [
    "retry-and-default",
    "file-url",
    "uint8array",
    "module-promise",
    "init-sync-root",
    "init-sync-wasm",
  ];

  let referencePcm;
  for (const mode of modes) {
    const rawOutput = execFileSync(
      process.execPath,
      [checkPath, packageName, wasmPath, mode],
      { cwd: consumerDir, encoding: "utf8" },
    );
    const result = JSON.parse(rawOutput);
    assert.equal(result.latencyFrames, 1593);
    assert.equal(result.processedFrames, "2560");
    assert.ok(
      result.nonZeroSamples > 0,
      `Expected non-zero PCM output in Node.js mode ${mode}`,
    );
    if (referencePcm === undefined) {
      referencePcm = result;
    } else {
      assert.deepEqual(
        result,
        referencePcm,
        `Expected deterministic PCM output in Node.js mode ${mode}`,
      );
    }
  }

  return referencePcm;
}

const manifestPackageName = readPackageName();
const packageNames = Array.from(
  new Set([manifestPackageName, "tunelet", SCOPED_PACKAGE_NAME]),
);
const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "tunelet-verify-pack-"));

try {
  const packDir = path.join(tempDir, "pack");
  fs.mkdirSync(packDir);

  const relativeArchivePath = packArchive(packDir);
  verifyArchiveEntries(tempDir, relativeArchivePath);

  const browserTargets = [];
  let expectedNodePcm;

  for (const [index, packageName] of packageNames.entries()) {
    const variantDir = path.join(tempDir, `variant-${index}`);
    const browserFixtureDir = path.join(variantDir, "browser");
    const workerFixtureDir = path.join(variantDir, "webworker");
    const nodeFixtureDir = path.join(variantDir, "node");

    const browserInstalledDir = installArchive(
      tempDir,
      relativeArchivePath,
      browserFixtureDir,
      manifestPackageName,
      packageName,
    );
    installArchive(
      tempDir,
      relativeArchivePath,
      workerFixtureDir,
      manifestPackageName,
      packageName,
    );
    const nodeInstalledDir = installArchive(
      tempDir,
      relativeArchivePath,
      nodeFixtureDir,
      manifestPackageName,
      packageName,
    );

    verifyBrowserFixture(browserFixtureDir, packageName);
    verifyWebWorkerFixture(workerFixtureDir, packageName);
    verifyNodeNextFixture(nodeFixtureDir, packageName);

    const nodePcm = verifyNodeRuntime(
      nodeFixtureDir,
      nodeInstalledDir,
      packageName,
    );
    if (expectedNodePcm === undefined) {
      expectedNodePcm = nodePcm;
    } else {
      assert.deepEqual(nodePcm, expectedNodePcm);
    }

    browserTargets.push({ installedDir: browserInstalledDir, packageName });
  }

  const browserPcm = await verifyBrowserLoaders(browserTargets);
  assert.deepEqual(
    browserPcm,
    expectedNodePcm,
    "Expected headless Chromium PCM output to match Node.js PCM output",
  );
} finally {
  fs.rmSync(tempDir, { recursive: true, force: true });
}
