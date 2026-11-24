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

const CHOCOLATEY_PACKAGES = [
  "npm",
  "nodejs",
  "nodejs-lts",
  "nodejs.commandline",
  "nodejs.install",
];

const repoRoot = fileURLToPath(new URL("..", import.meta.url));

/** Return true if `filePath` is a regular file on disk. */
function isRegularFile(filePath) {
  try {
    return fs.statSync(filePath).isFile();
  } catch {
    return false;
  }
}

/** Parse the CLI flags for `verify-pack.mjs`. */
function parsePackOptions(argv) {
  let ignoreScripts = false;
  for (const arg of argv) {
    if (arg === "--ignore-scripts") {
      ignoreScripts = true;
    } else {
      throw new Error(`Unknown option: ${arg}`);
    }
  }
  return { ignoreScripts };
}

/** Check that all archive source files exist before an `--ignore-scripts` pack. */
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

/** Check standard and split relative paths from `dir` for `npm-cli.js`. */
function findNpmCliInDir(dir) {
  const candidates = [
    path.join(dir, "node_modules", "npm", "bin", "npm-cli.js"),
    path.resolve(dir, "..", "lib", "node_modules", "npm", "bin", "npm-cli.js"),
    path.resolve(dir, "..", "node_modules", "npm", "bin", "npm-cli.js"),
    path.join(dir, "npm-cli.js"),
    path.join(dir, "bin", "npm-cli.js"),
    path.resolve(dir, "..", "bin", "npm-cli.js"),
  ];
  for (const candidate of candidates) {
    if (isRegularFile(candidate)) {
      return candidate;
    }
  }
  return undefined;
}

/** Check the tool directories of Chocolatey packages in `libDir` for `npm-cli.js`. */
function findNpmCliInChocolateyLib(libDir) {
  for (const pkg of CHOCOLATEY_PACKAGES) {
    const toolsDir = path.join(libDir, pkg, "tools");
    const direct = findNpmCliInDir(toolsDir);
    if (direct) {
      return direct;
    }
    try {
      for (const entry of fs.readdirSync(toolsDir, { withFileTypes: true })) {
        if (entry.isDirectory() && entry.name !== "node_modules") {
          const nested = findNpmCliInDir(path.join(toolsDir, entry.name));
          if (nested) {
            return nested;
          }
        }
      }
    } catch {
      // Ignore absent Chocolatey tool directories.
    }
  }
  return undefined;
}

/** Check Volta image directories relative to `nodeDir` for `npm`. */
function findNpmInVoltaImage(nodeDir) {
  const leafName = path.basename(nodeDir).toLowerCase();
  const baseNodeDir =
    leafName === "bin" || leafName === "x64" || leafName === "arm64"
      ? path.dirname(nodeDir)
      : nodeDir;
  const nodeTypeDir = path.dirname(baseNodeDir);
  const imageDir = path.dirname(nodeTypeDir);
  if (
    path.basename(nodeTypeDir).toLowerCase() !== "node" ||
    path.basename(imageDir).toLowerCase() !== "image"
  ) {
    return undefined;
  }

  for (const imageKind of ["npm", "bundled-npm"]) {
    const kindDir = path.join(imageDir, imageKind);
    try {
      const versions = fs
        .readdirSync(kindDir, { withFileTypes: true })
        .filter((entry) => entry.isDirectory())
        .map((entry) => entry.name)
        .sort((a, b) => b.localeCompare(a, undefined, { numeric: true }));
      for (const version of versions) {
        const cliCandidate = findNpmCliInDir(path.join(kindDir, version));
        if (cliCandidate) {
          return { command: process.execPath, prefixArgs: [cliCandidate] };
        }
      }
    } catch {
      // Ignore absent Volta image directories.
    }
  }

  const voltaRoot = path.resolve(imageDir, "..", "..");
  for (const exeCandidate of [
    path.join(voltaRoot, "bin", "npm.exe"),
    path.join(voltaRoot, "npm.exe"),
  ]) {
    if (isRegularFile(exeCandidate)) {
      return { command: exeCandidate, prefixArgs: [] };
    }
  }
  return undefined;
}

/** Check one candidate directory `dir` for `npm-cli.js` or `npm.exe`. */
function findWindowsNpmInDir(dir) {
  const cliPath = findNpmCliInDir(dir);
  if (cliPath) {
    return { command: process.execPath, prefixArgs: [cliPath] };
  }

  const dirName = path.basename(dir).toLowerCase();
  const parentName = path.basename(path.dirname(dir)).toLowerCase();
  if (dirName === "bin") {
    const chocoCli = findNpmCliInChocolateyLib(path.resolve(dir, "..", "lib"));
    if (chocoCli) {
      return { command: process.execPath, prefixArgs: [chocoCli] };
    }
  } else if (dirName === "tools") {
    const chocoCli = findNpmCliInChocolateyLib(path.resolve(dir, "..", ".."));
    if (chocoCli) {
      return { command: process.execPath, prefixArgs: [chocoCli] };
    }
  } else if (parentName === "tools") {
    const chocoCli = findNpmCliInChocolateyLib(
      path.resolve(dir, "..", "..", ".."),
    );
    if (chocoCli) {
      return { command: process.execPath, prefixArgs: [chocoCli] };
    }
  }

  const npmExePath = path.join(dir, "npm.exe");
  if (
    isRegularFile(npmExePath) &&
    path.resolve(npmExePath) !== path.resolve(process.execPath)
  ) {
    return { command: npmExePath, prefixArgs: [] };
  }
  return undefined;
}

/** Check a list of directories for `npm` and skip duplicates. */
function searchWindowsNpmDirs(dirs, seen) {
  for (const dir of dirs) {
    const key = path.normalize(dir).toLowerCase();
    if (seen.has(key)) {
      continue;
    }
    seen.add(key);
    const resolved = findWindowsNpmInDir(dir);
    if (resolved) {
      return resolved;
    }
  }
  return undefined;
}

/** Locate `npm-cli.js` or `npm.exe` across MSI, Chocolatey, MSYS2 and Volta. */
function resolveWindowsNpm() {
  const nodeDirs = [path.dirname(process.execPath)];
  try {
    const realNodeDir = path.dirname(fs.realpathSync(process.execPath));
    if (!nodeDirs.includes(realNodeDir)) {
      nodeDirs.push(realNodeDir);
    }
  } catch {
    // If realpath fails, keep the first directory.
  }

  const rawPath = process.env.PATH ?? process.env.Path ?? "";
  const pathDelimiter = rawPath.includes(";") ? ";" : path.delimiter;
  const pathDirs = rawPath
    .split(pathDelimiter)
    .map((entry) => entry.trim().replace(/^"+|"+$/g, ""))
    .filter((entry) => entry.length > 0);

  const seen = new Set();
  const fromNodeOrPath = searchWindowsNpmDirs([...nodeDirs, ...pathDirs], seen);
  if (fromNodeOrPath) {
    return fromNodeOrPath;
  }

  for (const nodeDir of nodeDirs) {
    const voltaMatch = findNpmInVoltaImage(nodeDir);
    if (voltaMatch) {
      return voltaMatch;
    }
  }

  const envDirs = [];
  if (process.env.VOLTA_HOME) {
    envDirs.push(path.join(process.env.VOLTA_HOME, "bin"));
    envDirs.push(process.env.VOLTA_HOME);
  }
  if (process.env.LOCALAPPDATA) {
    envDirs.push(path.join(process.env.LOCALAPPDATA, "Volta", "bin"));
  }
  if (process.env.ChocolateyInstall) {
    envDirs.push(path.join(process.env.ChocolateyInstall, "bin"));
  }
  if (process.env.ProgramData) {
    envDirs.push(path.join(process.env.ProgramData, "chocolatey", "bin"));
  }
  if (process.env.APPDATA) {
    envDirs.push(path.join(process.env.APPDATA, "npm"));
  }
  if (process.env.ProgramFiles) {
    envDirs.push(path.join(process.env.ProgramFiles, "Volta"));
    envDirs.push(path.join(process.env.ProgramFiles, "nodejs"));
  }

  const fromEnvDirs = searchWindowsNpmDirs(envDirs, seen);
  if (fromEnvDirs) {
    return fromEnvDirs;
  }

  for (const chocoRoot of [
    process.env.ChocolateyInstall,
    process.env.ProgramData
      ? path.join(process.env.ProgramData, "chocolatey")
      : undefined,
  ]) {
    if (chocoRoot) {
      const chocoCli = findNpmCliInChocolateyLib(path.join(chocoRoot, "lib"));
      if (chocoCli) {
        return { command: process.execPath, prefixArgs: [chocoCli] };
      }
    }
  }

  throw new Error("Cannot find npm-cli.js or npm.exe on Windows.");
}

/** Run `npm pack` without a shell and return the relative archive path. */
function packArchive(packDir, { ignoreScripts }) {
  if (ignoreScripts) {
    verifyPrebuiltFiles();
  }

  const packArgs = ignoreScripts
    ? ["pack", "--ignore-scripts", "--pack-destination", packDir]
    : ["pack", "--pack-destination", packDir];

  const npmExecPath = process.env.npm_execpath;
  if (npmExecPath) {
    if (npmExecPath.toLowerCase().endsWith(".exe")) {
      execFileSync(npmExecPath, packArgs, { cwd: repoRoot, stdio: "inherit" });
    } else {
      execFileSync(process.execPath, [npmExecPath, ...packArgs], {
        cwd: repoRoot,
        stdio: "inherit",
      });
    }
  } else if (process.platform === "win32") {
    const { command, prefixArgs } = resolveWindowsNpm();
    execFileSync(command, [...prefixArgs, ...packArgs], {
      cwd: repoRoot,
      stdio: "inherit",
    });
  } else {
    execFileSync("npm", packArgs, {
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

const packOptions = parsePackOptions(process.argv.slice(2));
const packageName = readPackageName();
const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "tunelet-verify-pack-"));

try {
  const packDir = path.join(tempDir, "pack");
  const consumerDir = path.join(tempDir, "consumer");
  fs.mkdirSync(packDir);
  fs.mkdirSync(consumerDir);

  const relativeArchivePath = packArchive(packDir, packOptions);
  verifyArchiveEntries(tempDir, relativeArchivePath);
  installArchive(tempDir, relativeArchivePath, consumerDir, packageName);
  verifyRuntimeImport(consumerDir, packageName);
  verifyTypeDeclarations(consumerDir, packageName);
} finally {
  fs.rmSync(tempDir, { recursive: true, force: true });
}
