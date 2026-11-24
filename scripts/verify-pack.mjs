/**
 * This script packs the npm archives, checks their eight files, runs all
 * Wasm loader paths in Node.js and checks consumer TypeScript declarations.
 */
import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
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

const UNSCOPED_PACKAGE_NAME = "tunelet";
const SCOPED_PACKAGE_NAME = "@bytelandtechnology/tunelet";

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

/** Run `npm pack` without a shell and return the archive file name. */
function packArchive(sourceDir, packDir, extraArgs = []) {
  const npmArgs = ["pack", ...extraArgs, "--pack-destination", packDir];
  const npmExecPath = process.env.npm_execpath;
  if (npmExecPath) {
    execFileSync(process.execPath, [npmExecPath, ...npmArgs], {
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
    execFileSync(process.execPath, [npmCliPath, ...npmArgs], {
      cwd: sourceDir,
      stdio: "inherit",
    });
  } else {
    execFileSync("npm", npmArgs, {
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
  return archives[0];
}

/** Copy the built package files and set an alternate package name. */
function stagePackageCopy(stageDir, packageName) {
  fs.mkdirSync(path.join(stageDir, "pkg"), { recursive: true });
  fs.cpSync(path.join(repoRoot, "dist"), path.join(stageDir, "dist"), {
    recursive: true,
  });
  for (const pkgFile of ["tunelet.js", "tunelet.d.ts", "tunelet_bg.wasm"]) {
    fs.copyFileSync(
      path.join(repoRoot, "pkg", pkgFile),
      path.join(stageDir, "pkg", pkgFile),
    );
  }
  for (const rootFile of ["LICENSE", "README.md"]) {
    fs.copyFileSync(
      path.join(repoRoot, rootFile),
      path.join(stageDir, rootFile),
    );
  }
  const packageJson = JSON.parse(
    fs.readFileSync(path.join(repoRoot, "package.json"), "utf8"),
  );
  packageJson.name = packageName;
  fs.writeFileSync(
    path.join(stageDir, "package.json"),
    `${JSON.stringify(packageJson, null, 2)}\n`,
  );
}

/** Check that the archive contains only the eight expected files. */
function verifyArchiveEntries(packDir, archiveFile) {
  const rawList = execFileSync("tar", ["-tzf", archiveFile], {
    cwd: packDir,
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
function installArchive(packDir, archiveFile, consumerDir, packageName) {
  const extractDir = path.join(consumerDir, "extract");
  fs.mkdirSync(extractDir, { recursive: true });
  execFileSync("tar", ["-xzf", path.join(packDir, archiveFile), "-C", extractDir]);

  const installedDir = path.join(
    consumerDir,
    "node_modules",
    ...packageName.split("/"),
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

/** Run one isolated Node.js child process while the HTTP server stays active. */
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

/** Execute all documented loader modes in isolated Node.js child processes. */
async function verifyRuntimeImport(consumerDir, installedDir, packageName) {
  const wasmPath = path.join(installedDir, "pkg", "tunelet_bg.wasm");
  const wasmBytes = fs.readFileSync(wasmPath);
  const checkPath = path.join(consumerDir, "check.mjs");

  fs.writeFileSync(
    checkPath,
    [
      'import assert from "node:assert/strict";',
      'import fs from "node:fs";',
      "",
      "const [packageName, mode, baseUrl, wasmPath] = process.argv.slice(2);",
      "const rootModule = await import(packageName);",
      "const wasmModule = await import(`${packageName}/wasm`);",
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
      '    if (typeof Symbol.dispose === "symbol" && typeof processor[Symbol.dispose] === "function") {',
      "      processor[Symbol.dispose]();",
      "    } else {",
      "      processor.free();",
      "    }",
      "  }",
      "}",
      "",
      'if (mode === "default-file") {',
      '  assert.equal(typeof rootModule.loadTunelet, "function");',
      "  assert.equal(rootModule.default, rootModule.loadTunelet);",
      '  assert.equal(typeof rootModule.initSync, "function");',
      '  assert.equal(typeof rootModule.Tunelet, "function");',
      '  assert.equal(typeof rootModule.defaultConfig, "function");',
      '  assert.equal(typeof wasmModule.default, "function");',
      '  assert.equal(typeof wasmModule.initSync, "function");',
      "  const wasmExports = await rootModule.loadTunelet();",
      "  assert.ok(wasmExports);",
      "  verifyPcmBlock(rootModule);",
      '} else if (mode === "http-404-retry") {',
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
      '} else if (mode === "uint8array") {',
      "  const bytes = Uint8Array.from(fs.readFileSync(wasmPath));",
      "  assert.equal(bytes.constructor, Uint8Array);",
      "  const wasmExports = await rootModule.loadTunelet(bytes);",
      "  assert.ok(wasmExports);",
      "  verifyPcmBlock(rootModule);",
      '} else if (mode === "module-promise") {',
      "  const bytes = Uint8Array.from(fs.readFileSync(wasmPath));",
      "  const modulePromise = WebAssembly.compile(bytes);",
      "  const wasmExports = await rootModule.loadTunelet(modulePromise);",
      "  assert.ok(wasmExports);",
      "  verifyPcmBlock(rootModule);",
      '} else if (mode === "init-sync-root") {',
      "  const bytes = Uint8Array.from(fs.readFileSync(wasmPath));",
      "  const wasmExports = rootModule.initSync({ module: bytes });",
      "  assert.ok(wasmExports);",
      "  verifyPcmBlock(rootModule);",
      '} else if (mode === "init-sync-wasm") {',
      "  const bytes = Uint8Array.from(fs.readFileSync(wasmPath));",
      "  const compiledModule = new WebAssembly.Module(bytes);",
      "  const wasmExports = wasmModule.initSync({ module: compiledModule });",
      "  assert.ok(wasmExports);",
      "  verifyPcmBlock(wasmModule);",
      "  verifyPcmBlock(rootModule);",
      "} else {",
      "  throw new Error(`Unknown runtime mode: ${mode}`);",
      "}",
      "",
    ].join("\n"),
  );

  const server = await startWasmServer(wasmBytes);
  try {
    const modes = [
      "default-file",
      "http-404-retry",
      "http-response-wasm",
      "http-response-octet-stream",
      "uint8array",
      "module-promise",
      "init-sync-root",
      "init-sync-wasm",
    ];
    for (const mode of modes) {
      await runNodeScript(
        checkPath,
        [packageName, mode, server.baseUrl, wasmPath],
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

/** Build the TypeScript source for one consumer profile. */
function buildConsumerCheckSource(packageName, profileScopeLines) {
  const pkgLiteral = JSON.stringify(packageName);
  const wasmLiteral = JSON.stringify(`${packageName}/wasm`);
  return [
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
    ...profileScopeLines,
    "",
    "export async function verifyTypes(",
    "  source: InitInput | undefined,",
    "  bytes: Uint8Array,",
    "  buffer: ArrayBuffer,",
    "  compiledModule: WebAssembly.Module,",
    "  modulePromise: Promise<WebAssembly.Module>,",
    "  syncSource: SyncInitInput,",
    "): Promise<InitOutput> {",
    "  const exportsFromRoot: InitOutput = await loadTunelet(source);",
    "  const exportsFromNamed: InitOutput = await namedLoadTunelet(source);",
    "  const exportsFromWasm: InitOutput = await init(source);",
    '  await loadTunelet("https://example.invalid/tunelet_bg.wasm");',
    '  await loadTunelet(new Request("https://example.invalid/tunelet_bg.wasm"));',
    '  await loadTunelet(new URL("https://example.invalid/tunelet_bg.wasm"));',
    '  await loadTunelet(await fetch("https://example.invalid/tunelet_bg.wasm"));',
    "  await loadTunelet(bytes);",
    "  await loadTunelet(buffer);",
    "  await loadTunelet(compiledModule);",
    "  await loadTunelet(modulePromise);",
    "  const syncFromRoot: InitOutput = rootInitSync({ module: syncSource });",
    "  const syncFromWasm: InitOutput = wasmInitSync({ module: bytes });",
    "  const syncFromModule: InitOutput = wasmInitSync({ module: compiledModule });",
    "  const config: ProcessorConfig = defaultConfig();",
    "  using disposableProcessor = new Tunelet(48000, 128, config);",
    "  const input = new Int16Array(128);",
    "  const left = new Int16Array(128);",
    "  const right = new Int16Array(128);",
    "  disposableProcessor.process(input, left, right);",
    "  const manualProcessor = new Tunelet(48000, 128, config);",
    "  manualProcessor[Symbol.dispose]();",
    "  manualProcessor.free();",
    "  void exportsFromNamed;",
    "  void exportsFromWasm;",
    "  void syncFromRoot;",
    "  void syncFromWasm;",
    "  void syncFromModule;",
    "  return exportsFromRoot;",
    "}",
    "",
  ].join("\n");
}

/** Check strict TypeScript resolution across browser, WebWorker and Node.js. */
function verifyTypeDeclarations(consumerDir, packageName) {
  const tscBin = path.join(
    repoRoot,
    "node_modules",
    "typescript",
    "bin",
    "tsc",
  );
  const nodeTypesRoot = path.join(repoRoot, "node_modules", "@types");

  const profiles = [
    {
      name: "browser",
      compilerOptions: {
        target: "ES2022",
        module: "ESNext",
        moduleResolution: "Bundler",
        lib: ["ES2022", "DOM"],
        types: [],
        strict: true,
        skipLibCheck: false,
        noEmit: true,
      },
      scopeLines: ["const browserWindow: Window = window;", "void browserWindow;"],
    },
    {
      name: "webworker",
      compilerOptions: {
        target: "ES2022",
        module: "ESNext",
        moduleResolution: "Bundler",
        lib: ["ES2022", "WebWorker"],
        types: [],
        strict: true,
        skipLibCheck: false,
        noEmit: true,
      },
      scopeLines: [
        "// @ts-expect-error window is absent in WebWorker scopes.",
        "void window;",
        "const workerScope: WorkerGlobalScope = self;",
        "void workerScope;",
      ],
    },
    {
      name: "node",
      compilerOptions: {
        target: "ES2022",
        module: "NodeNext",
        moduleResolution: "NodeNext",
        lib: ["ES2022"],
        types: ["node"],
        typeRoots: [nodeTypesRoot],
        strict: true,
        skipLibCheck: false,
        noEmit: true,
      },
      scopeLines: [
        "// @ts-expect-error window is absent in Node.js scopes.",
        "void window;",
        "const timer: NodeJS.Timeout = setTimeout(() => {}, 0);",
        "clearTimeout(timer);",
      ],
    },
  ];

  for (const profile of profiles) {
    const profileDir = path.join(consumerDir, "typecheck", profile.name);
    fs.mkdirSync(profileDir, { recursive: true });
    fs.writeFileSync(
      path.join(profileDir, "package.json"),
      JSON.stringify({
        name: `tunelet-typecheck-${profile.name}`,
        private: true,
        type: "module",
      }),
    );
    fs.writeFileSync(
      path.join(profileDir, "tsconfig.json"),
      `${JSON.stringify(
        {
          compilerOptions: profile.compilerOptions,
          include: ["check.ts"],
        },
        null,
        2,
      )}\n`,
    );
    fs.writeFileSync(
      path.join(profileDir, "check.ts"),
      buildConsumerCheckSource(packageName, profile.scopeLines),
    );

    execFileSync(process.execPath, [tscBin, "-p", profileDir], {
      cwd: profileDir,
      stdio: "inherit",
    });
  }
}

/** Verify archive files, Node.js loaders and consumer types for one archive. */
async function verifyPackedPackage(packDir, archiveFile, consumerDir, packageName) {
  verifyArchiveEntries(packDir, archiveFile);
  const installedDir = installArchive(
    packDir,
    archiveFile,
    consumerDir,
    packageName,
  );
  await verifyRuntimeImport(consumerDir, installedDir, packageName);
  verifyTypeDeclarations(consumerDir, packageName);
}

const primaryPackageName = readPackageName();
const alternatePackageName =
  primaryPackageName === SCOPED_PACKAGE_NAME
    ? UNSCOPED_PACKAGE_NAME
    : SCOPED_PACKAGE_NAME;

const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "tunelet-verify-pack-"));

try {
  const primaryPackDir = path.join(tempDir, "pack-primary");
  const primaryConsumerDir = path.join(tempDir, "consumer-primary");
  fs.mkdirSync(primaryPackDir);
  fs.mkdirSync(primaryConsumerDir);

  const primaryArchive = packArchive(repoRoot, primaryPackDir);
  await verifyPackedPackage(
    primaryPackDir,
    primaryArchive,
    primaryConsumerDir,
    primaryPackageName,
  );

  const stageDir = path.join(tempDir, "stage-alternate");
  const alternatePackDir = path.join(tempDir, "pack-alternate");
  const alternateConsumerDir = path.join(tempDir, "consumer-alternate");
  fs.mkdirSync(alternatePackDir);
  fs.mkdirSync(alternateConsumerDir);

  stagePackageCopy(stageDir, alternatePackageName);
  const alternateArchive = packArchive(stageDir, alternatePackDir, [
    "--ignore-scripts",
  ]);
  await verifyPackedPackage(
    alternatePackDir,
    alternateArchive,
    alternateConsumerDir,
    alternatePackageName,
  );
} finally {
  fs.rmSync(tempDir, { recursive: true, force: true });
}
