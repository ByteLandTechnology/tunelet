/**
 * This script packs the npm archives, checks their eight files, runs
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

const DEFAULT_PACKAGE_NAMES = [
  "tunelet",
  "@bytelandtechnology/tunelet",
];

const IGNORED_STAGE_ENTRIES = new Set([".git", "node_modules", "target"]);

const PACKAGE_NAME_PATTERN =
  /^(?:@[a-z0-9-~][a-z0-9-._~]*\/)?[a-z0-9-~][a-z0-9-._~]*$/;

const repoRoot = fileURLToPath(new URL("..", import.meta.url));

/** Parse the command-line options for this script. */
function parseOptions(argv) {
  let prebuilt = false;
  for (const argument of argv) {
    if (argument === "--prebuilt") {
      prebuilt = true;
    } else {
      throw new Error(`Unknown option: ${argument}`);
    }
  }
  return { prebuilt };
}

/** Check that the package name has a valid npm format. */
function validatePackageName(packageName) {
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

/** Read `package.json` and check its package name. */
function readPackageJson() {
  const packageJsonPath = path.join(repoRoot, "package.json");
  const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, "utf8"));
  validatePackageName(packageJson.name);
  return packageJson;
}

/** Build a clean environment for `npm` in a directory. */
function createNpmEnv(cwd) {
  if (cwd === repoRoot) {
    return process.env;
  }
  const env = { ...process.env };
  delete env.INIT_CWD;
  for (const key of Object.keys(env)) {
    if (key.startsWith("npm_package_") || key.startsWith("npm_lifecycle_")) {
      delete env[key];
    }
  }
  return env;
}

/** Run `npm` without a shell in the target directory. */
function runNpm(args, cwd) {
  const env = createNpmEnv(cwd);
  const npmExecPath = process.env.npm_execpath;
  if (npmExecPath) {
    execFileSync(process.execPath, [npmExecPath, ...args], {
      cwd,
      env,
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
    execFileSync(process.execPath, [npmCliPath, ...args], {
      cwd,
      env,
      stdio: "inherit",
    });
  } else {
    execFileSync("npm", args, {
      cwd,
      env,
      stdio: "inherit",
    });
  }
}

/** Run `npm pack` without a shell and return the relative archive path. */
function packArchive(
  packDir,
  { cwd = repoRoot, ignoreScripts = false } = {},
) {
  const args = ["pack"];
  if (ignoreScripts) {
    args.push("--ignore-scripts");
  }
  args.push("--pack-destination", packDir);
  runNpm(args, cwd);

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

/** Copy the built package files and set a target name in `stageDir`. */
function stagePackageTree(stageDir, packageJson, packageName) {
  validatePackageName(packageName);
  fs.mkdirSync(stageDir, { recursive: true });
  for (const entry of fs.readdirSync(repoRoot)) {
    if (IGNORED_STAGE_ENTRIES.has(entry) || entry === "package.json") {
      continue;
    }
    fs.cpSync(path.join(repoRoot, entry), path.join(stageDir, entry), {
      recursive: true,
    });
  }
  fs.writeFileSync(
    path.join(stageDir, "package.json"),
    `${JSON.stringify({ ...packageJson, name: packageName }, null, 2)}\n`,
  );
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

/** Import the installed package in Node.js and process one audio block. */
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
      'assert.equal(typeof rootModule.defaultConfig, "function");',
      'assert.equal(typeof wasmModule.default, "function");',
      'assert.equal(typeof wasmModule.initSync, "function");',
      "",
      "const wasmExports = await rootModule.loadTunelet();",
      "assert.ok(wasmExports);",
      "",
      "const config = rootModule.defaultConfig();",
      "const processor = new rootModule.Tunelet(48000, 128, config);",
      "try {",
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
      "  const input = new Int16Array(128);",
      "  const left = new Int16Array(128);",
      "  const right = new Int16Array(128);",
      "  processor.process(input, left, right);",
      "  processor.free();",
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

/** Pack, install and check one archive for `packageName`. */
function verifyPackage(runDir, packageName, packOptions) {
  const packDir = path.join(runDir, "pack");
  const consumerDir = path.join(runDir, "consumer");
  fs.mkdirSync(packDir, { recursive: true });
  fs.mkdirSync(consumerDir, { recursive: true });

  const relativeArchivePath = packArchive(packDir, packOptions);
  verifyArchiveEntries(runDir, relativeArchivePath);
  installArchive(runDir, relativeArchivePath, consumerDir, packageName);
  verifyRuntimeImport(consumerDir, packageName);
  verifyTypeDeclarations(consumerDir, packageName);
}

const options = parseOptions(process.argv.slice(2));
const packageJson = readPackageJson();
const currentPackageName = packageJson.name;
const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "tunelet-verify-pack-"));

try {
  if (options.prebuilt) {
    verifyPackage(path.join(tempDir, "prebuilt"), currentPackageName, {
      cwd: repoRoot,
      ignoreScripts: true,
    });
  } else {
    const targetNames = DEFAULT_PACKAGE_NAMES.includes(currentPackageName)
      ? [
          currentPackageName,
          ...DEFAULT_PACKAGE_NAMES.filter(
            (name) => name !== currentPackageName,
          ),
        ]
      : [...DEFAULT_PACKAGE_NAMES];

    if (!DEFAULT_PACKAGE_NAMES.includes(currentPackageName)) {
      runNpm(["run", "build"], repoRoot);
    }

    for (const [index, targetName] of targetNames.entries()) {
      const runDir = path.join(tempDir, `check-${index}`);
      if (targetName === currentPackageName && index === 0) {
        verifyPackage(runDir, targetName, {
          cwd: repoRoot,
          ignoreScripts: false,
        });
      } else {
        const stageDir = path.join(runDir, "stage");
        stagePackageTree(stageDir, packageJson, targetName);
        verifyPackage(runDir, targetName, {
          cwd: stageDir,
          ignoreScripts: true,
        });
      }
    }
  }
} finally {
  fs.rmSync(tempDir, { recursive: true, force: true });
}
