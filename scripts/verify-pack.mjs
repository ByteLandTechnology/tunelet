/**
 * This script packs the npm archive, checks its twelve files, runs runtime
 * Node.js checks and checks four strict TypeScript profiles.
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
  "package/dist/browser.d.ts",
  "package/dist/browser.js",
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

/** Remove comments from JavaScript or TypeScript source text. */
function stripComments(source) {
  return source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/.*$/gm, "");
}

/** Check browser bundle isolation and declaration parity on packed files. */
function verifyInstalledFiles(installedDir) {
  const browserJs = fs.readFileSync(
    path.join(installedDir, "dist", "browser.js"),
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

  assert.equal(
    stripComments(nodeDts).trim(),
    stripComments(browserDts).trim(),
    "dist/node.d.ts and dist/browser.d.ts must expose identical signatures",
  );
  assert.match(wasmDts, /voices\?: number \| undefined;/);

  for (const [label, content] of [
    ["dist/node.d.ts", nodeDts],
    ["dist/browser.d.ts", browserDts],
    ["dist/shared.d.ts", sharedDts],
    ["pkg/tunelet.d.ts", wasmDts],
  ]) {
    const code = stripComments(content);
    for (const forbidden of [
      /Symbol\.dispose/,
      /\bWebAssembly\b/,
      /\bRequestInfo\b/,
      /\bBufferSource\b/,
      /\bdeclare\s+global\b/,
      /\/\/\/\s*<reference\b/,
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

/** Check Node.js exports, `voices: undefined`, `file:` strings and bundles. */
function verifyRuntimeImport(consumerDir, installedDir, packageName) {
  runNodeCheck(
    consumerDir,
    "check-default.mjs",
    [
      'import assert from "node:assert/strict";',
      'import { createRequire } from "node:module";',
      'import { pathToFileURL } from "node:url";',
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
      'assert.match(pkgWasmResolved, /\\/pkg\\/tunelet_bg\\.wasm$/);',
      "assert.equal(shortWasmResolved, pkgWasmResolved);",
      "assert.equal(",
      "  require.resolve(`${packageName}/pkg/tunelet_bg.wasm`),",
      "  require.resolve(`${packageName}/tunelet_bg.wasm`),",
      ");",
      "",
      "const rootModule = await import(packageName);",
      "const browserModule = await import(pathToFileURL(`${installedDir}/dist/browser.js`).href);",
      "const wasmModule = await import(`${packageName}/wasm`);",
      "",
      "assert.deepEqual(Object.keys(rootModule).sort(), Object.keys(browserModule).sort());",
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
      "const baseConfig = rootModule.defaultConfig();",
      "assert.equal(baseConfig.resynthesis.harmony.voices, 4);",
      "",
      "const constructConfig = {",
      "  ...baseConfig,",
      "  resynthesis: {",
      "    ...baseConfig.resynthesis,",
      "    harmony: {",
      "      ...baseConfig.resynthesis.harmony,",
      "      voices: undefined,",
      "    },",
      "  },",
      "};",
      "const processor = new rootModule.Tunelet(48000, 128, constructConfig);",
      "try {",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 4);",
      "",
      "  const twoVoiceConfig = rootModule.defaultConfig();",
      "  twoVoiceConfig.resynthesis.harmony.voices = 2;",
      "  processor.setConfig(twoVoiceConfig);",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 2);",
      "",
      "  processor.setConfig(constructConfig);",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 4);",
      "",
      "  const oneVoiceConfig = rootModule.defaultConfig();",
      "  oneVoiceConfig.resynthesis.harmony.voices = 1;",
      "  processor.setConfig(oneVoiceConfig);",
      "  assert.equal(processor.config().resynthesis.harmony.voices, 1);",
      "",
      "  const mono = new Int16Array(128);",
      "  processor.processMono(mono, [",
      "    {",
      "      offset: 0,",
      "      event: {",
      '        type: "resynthesis",',
      "        value: {",
      "          ...baseConfig.resynthesis,",
      "          harmony: {",
      "            ...baseConfig.resynthesis.harmony,",
      "            voices: undefined,",
      "          },",
      "        },",
      "      },",
      "    },",
      "  ]);",
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
      "  assert.equal(processor.config().resynthesis.harmony.voices, 4);",
      "} finally {",
      "  processor.free();",
      "}",
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
    .replace(/^export function loadOnce\(/m, "function loadOnce(");
  const nodeGlue = fs
    .readFileSync(path.join(installedDir, "dist", "node.js"), "utf8")
    .replace(/^import \{ init, loadOnce \} from "\.\/shared\.js";\r?\n/m, "")
    .replace(/^export \* from "\.\.\/pkg\/tunelet\.js";\r?\n/m, "");
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
    "await flatBundle.loadTunelet();",
    "const processor = new flatBundle.Tunelet(48000, 64);",
    "try {",
    "  const audio = new Int16Array(64);",
    "  processor.processMono(audio);",
    "} finally {",
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
function verifyTypeDeclarations(consumerDir, packageName) {
  const pkgLiteral = JSON.stringify(packageName);
  const wasmLiteral = JSON.stringify(`${packageName}/wasm`);
  const tscBin = path.join(
    repoRoot,
    "node_modules",
    "typescript",
    "bin",
    "tsc",
  );
  const nodeTypesPath = path.join(repoRoot, "node_modules", "@types", "node");

  const sharedAssertions = [
    "import loadTunelet, {",
    "  defaultConfig,",
    "  initSync as rootInitSync,",
    "  loadTunelet as namedLoadTunelet,",
    "  Tunelet,",
    "  type HarmonyConfig,",
    "  type ProcessorConfig,",
    `} from ${pkgLiteral};`,
    "import init, {",
    "  initSync as wasmInitSync,",
    "  type InitInput,",
    "  type InitOutput,",
    `} from ${wasmLiteral};`,
    "",
    "export async function verifyShared(",
    "  source?: InitInput,",
    "  optionalVoices?: number,",
    "): Promise<void> {",
    "  const exportsFromRoot: InitOutput = await loadTunelet(source);",
    "  const exportsFromNamed: InitOutput = await namedLoadTunelet(source);",
    "  const exportsFromWasm: InitOutput = await init(source);",
    "  const config: ProcessorConfig = defaultConfig();",
    "  const harmony: HarmonyConfig = {",
    "    ...config.resynthesis.harmony,",
    "    voices: optionalVoices,",
    "  };",
    "  config.resynthesis.harmony = harmony;",
    "  config.resynthesis.harmony.voices = undefined;",
    "  const processor = new Tunelet(48000, 128, config);",
    "  const input = new Int16Array(128);",
    "  const left = new Int16Array(128);",
    "  const right = new Int16Array(128);",
    "  processor.setConfig(config);",
    "  processor.process(input, left, right);",
    "  processor.processMono(input, [",
    "    {",
    "      offset: 0,",
    "      event: {",
    '        type: "resynthesis",',
    "        value: {",
    "          ...config.resynthesis,",
    "          harmony: {",
    "            ...config.resynthesis.harmony,",
    "            voices: optionalVoices,",
    "          },",
    "        },",
    "      },",
    "    },",
    "  ]);",
    "  processor.free();",
    "  void rootInitSync(new Uint8Array(8));",
    "  void wasmInitSync({ module: new ArrayBuffer(8) });",
    "  void exportsFromRoot.memory.buffer;",
    "  void exportsFromNamed;",
    "  void exportsFromWasm;",
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
      types: [nodeTypesPath],
    },
    [
      ...sharedAssertions,
      "",
      "export async function verifyNodeInputs(): Promise<void> {",
      '  await loadTunelet("file:///tmp/tunelet_bg.wasm");',
      '  await loadTunelet(new URL("file:///tmp/tunelet_bg.wasm"));',
      "  await loadTunelet(Buffer.alloc(8));",
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
      "export async function verifyEs2023Inputs(): Promise<void> {",
      '  await loadTunelet("https://example.com/tunelet_bg.wasm");',
      "  await loadTunelet(new Uint8Array(8));",
      "  await loadTunelet(new ArrayBuffer(8));",
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
    sharedAssertions,
  );

  runTscProfile(
    consumerDir,
    tscBin,
    "dom",
    {
      target: "ES2022",
      module: "ES2022",
      moduleResolution: "Bundler",
      lib: ["ES2022", "DOM"],
      types: [],
    },
    [
      ...sharedAssertions,
      "",
      "export async function verifyDomInputs(",
      "  compiled: WebAssembly.Module,",
      "): Promise<void> {",
      '  await loadTunelet(new URL("./tunelet_bg.wasm", import.meta.url));',
      '  await loadTunelet(new Request("https://example.com/tunelet_bg.wasm"));',
      '  await loadTunelet(fetch("https://example.com/tunelet_bg.wasm"));',
      "  await loadTunelet(compiled);",
      "  void rootInitSync({ module: compiled });",
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
      "  compiled: WebAssembly.Module,",
      "): Promise<void> {",
      '  await loadTunelet(new URL("./tunelet_bg.wasm", self.location.href));',
      '  await loadTunelet(new Request("https://example.com/tunelet_bg.wasm"));',
      '  await loadTunelet(fetch("https://example.com/tunelet_bg.wasm"));',
      "  await loadTunelet(compiled);",
      "  void rootInitSync({ module: compiled });",
      "}",
    ],
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
  const installedDir = installArchive(
    tempDir,
    relativeArchivePath,
    consumerDir,
    packageName,
  );
  verifyInstalledFiles(installedDir);
  verifyRuntimeImport(consumerDir, installedDir, packageName);
  verifyTypeDeclarations(consumerDir, packageName);
} finally {
  fs.rmSync(tempDir, { recursive: true, force: true });
}
