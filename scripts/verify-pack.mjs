/**
 * This script builds the package one time and packs `tunelet` and
 * `@bytelandtechnology/tunelet`. For each archive, it checks the fourteen
 * files, runtime Node.js behavior and strict TypeScript profiles.
 *
 * If you give `--out-dir <dir>`, the script writes both `.tgz` archives and
 * `SHA256SUMS.txt` to `<dir>` after all checks succeed.
 */
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const UNSCOPED_PACKAGE_NAME = "tunelet";
const SCOPED_PACKAGE_NAME = "@bytelandtechnology/tunelet";

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
  assert.match(wasmDts, /voices\?: number \| undefined;/);
  assert.match(wasmDts, /toFixedLengthBuffer\(\): ArrayBuffer;/);
  assert.match(wasmDts, /toResizableBuffer\(\): ArrayBuffer;/);

  for (const [label, content] of [
    ["dist/index.d.ts", indexDts],
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
      /\bResponse\b/,
      /\bURL\b/,
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

/** Check Node.js exports, `dist/index.js`, `voices: undefined` and bundles. */
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
      "try {",
      "  assert.equal(processor.sampleRate, 48000);",
      "} finally {",
      "  processor.free();",
      "}",
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
  const nodeTypesPath = path.join(repoRoot, "node_modules", "@types", "node");

  const sharedAssertions = [
    "import loadTunelet, {",
    "  defaultConfig,",
    "  initSync as rootInitSync,",
    "  loadTunelet as namedLoadTunelet,",
    "  Tunelet,",
    "  type HarmonyConfig,",
    "  type ProcessorConfig,",
    "  type SyncInitInput as RootSyncInitInput,",
    `} from ${pkgLiteral};`,
    `import indexDefaultLoad, { loadTunelet as indexLoadTunelet } from ${directIndexLiteral};`,
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
      types: [nodeTypesPath],
    },
    [
      ...sharedAssertions,
      "",
      "export async function verifyNodeInputs(",
      "  req: Request,",
      "  res: Response,",
      "  url: URL,",
      "): Promise<void> {",
      `  const pkgWasmHref: string = import.meta.resolve(${pkgWasmLiteral});`,
      `  const rootWasmHref: string = import.meta.resolve(${rootWasmLiteral});`,
      "  await loadTunelet(pkgWasmHref);",
      "  await loadTunelet(rootWasmHref);",
      '  await loadTunelet("file:///tmp/tunelet_bg.wasm");',
      "  await loadTunelet(url);",
      "  await loadTunelet(req);",
      "  await loadTunelet(res);",
      "  await loadTunelet(Buffer.alloc(8));",
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
      "export async function verifyEs2023Inputs(): Promise<void> {",
      '  await loadTunelet("https://example.com/tunelet_bg.wasm");',
      '  await loadTunelet({ href: "file:///tunelet_bg.wasm", protocol: "file:" });',
      "  await loadTunelet(new Uint8Array(8));",
      "  await loadTunelet(new ArrayBuffer(8));",
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
      "export function verifyNoWindowInNodeNext(): void {",
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
      "): Promise<WebAssembly.Memory> {",
      "  await loadTunelet(reqInfo);",
      "  await loadTunelet(req);",
      "  await loadTunelet(res);",
      "  await loadTunelet(bufferSource);",
      '  await loadTunelet(new URL("./tunelet_bg.wasm", import.meta.url));',
      '  await loadTunelet(fetch("https://example.com/tunelet_bg.wasm"));',
      "  const out = await verifyShared(compiled, compiled);",
      "  const table: WebAssembly.Table = out.__wbindgen_externrefs;",
      "  void table;",
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
      "): Promise<WebAssembly.Memory> {",
      "  await loadTunelet(reqInfo);",
      "  await loadTunelet(req);",
      "  await loadTunelet(res);",
      "  await loadTunelet(bufferSource);",
      '  await loadTunelet(new URL("./tunelet_bg.wasm", self.location.href));',
      '  await loadTunelet(fetch("https://example.com/tunelet_bg.wasm"));',
      "  const out = await verifyShared(compiled, compiled);",
      "  const table: WebAssembly.Table = out.__wbindgen_externrefs;",
      "  void table;",
      "  // @ts-expect-error window must not leak into WebWorker scope",
      "  void window;",
      "  return out.memory;",
      "}",
    ],
  );
}

/** Check the archive list, runtime imports and TypeScript declarations. */
function verifyPackedArchive(workDir, relativeArchivePath, packageName) {
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
  verifyTypeDeclarations(consumerDir, installedDir, packageName);
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
  verifyPackedArchive(
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
  verifyPackedArchive(
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
