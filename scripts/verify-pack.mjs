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

/** Return true if `filePath` exists and is a regular file. */
function isRegularFile(filePath) {
  try {
    return fs.statSync(filePath).isFile();
  } catch {
    return false;
  }
}

/** Find `npm` on Windows across standard and split Node.js layouts. */
function resolveWin32Npm() {
  const nodeDir = path.dirname(process.execPath);
  const candidateDirs = [nodeDir];

  const rawPath = process.env.PATH ?? process.env.Path ?? "";
  for (const entry of rawPath.split(path.delimiter)) {
    const cleaned = entry.trim().replace(/^"+|"+$/g, "");
    if (cleaned.length > 0 && path.isAbsolute(cleaned)) {
      candidateDirs.push(cleaned);
    }
  }

  const voltaRoots = [];
  if (/[\\/]tools[\\/]image[\\/]node[\\/]/i.test(nodeDir)) {
    voltaRoots.push(
      path.resolve(nodeDir, "..", "..", "..", ".."),
      path.resolve(nodeDir, "..", "..", "..", "..", ".."),
    );
  }
  if (process.env.VOLTA_HOME && path.isAbsolute(process.env.VOLTA_HOME)) {
    voltaRoots.push(process.env.VOLTA_HOME);
  }
  if (process.env.LOCALAPPDATA && path.isAbsolute(process.env.LOCALAPPDATA)) {
    voltaRoots.push(path.join(process.env.LOCALAPPDATA, "Volta"));
    candidateDirs.push(
      path.join(process.env.LOCALAPPDATA, "Programs", "nodejs"),
    );
  }
  for (const voltaRoot of voltaRoots) {
    candidateDirs.push(path.join(voltaRoot, "bin"));
    const voltaNpmImageRoot = path.join(voltaRoot, "tools", "image", "npm");
    try {
      const versions = fs
        .readdirSync(voltaNpmImageRoot)
        .sort((a, b) => b.localeCompare(a, undefined, { numeric: true }));
      for (const version of versions) {
        candidateDirs.push(path.join(voltaNpmImageRoot, version, "bin"));
      }
    } catch {
      // Ignore absent Volta image directories.
    }
  }

  if (process.env.APPDATA && path.isAbsolute(process.env.APPDATA)) {
    candidateDirs.push(path.join(process.env.APPDATA, "npm"));
  }
  if (process.env.NVM_SYMLINK && path.isAbsolute(process.env.NVM_SYMLINK)) {
    candidateDirs.push(process.env.NVM_SYMLINK);
  }
  if (
    process.env.ChocolateyInstall &&
    path.isAbsolute(process.env.ChocolateyInstall)
  ) {
    const chocoRoot = process.env.ChocolateyInstall;
    candidateDirs.push(
      path.join(chocoRoot, "lib", "npm", "tools"),
      path.join(chocoRoot, "lib", "nodejs", "tools"),
      path.join(chocoRoot, "lib", "nodejs.commandline", "tools"),
      path.join(chocoRoot, "lib", "nodejs-lts", "tools"),
      path.join(chocoRoot, "bin"),
    );
  }
  for (const envName of ["ProgramFiles", "ProgramFiles(x86)"]) {
    const baseDir = process.env[envName];
    if (baseDir && path.isAbsolute(baseDir)) {
      candidateDirs.push(
        path.join(baseDir, "nodejs"),
        path.join(baseDir, "Volta"),
      );
    }
  }

  const seenDirs = new Set();
  for (const dir of candidateDirs) {
    const key = path.normalize(dir).toLowerCase();
    if (seenDirs.has(key)) {
      continue;
    }
    seenDirs.add(key);

    const cliCandidates = [
      path.join(dir, "node_modules", "npm", "bin", "npm-cli.js"),
      path.resolve(
        dir,
        "..",
        "lib",
        "node_modules",
        "npm",
        "bin",
        "npm-cli.js",
      ),
      path.resolve(dir, "..", "node_modules", "npm", "bin", "npm-cli.js"),
      path.join(dir, "npm-cli.js"),
      path.join(dir, "bin", "npm-cli.js"),
      path.resolve(
        dir,
        "..",
        "lib",
        "npm",
        "tools",
        "node_modules",
        "npm",
        "bin",
        "npm-cli.js",
      ),
      path.resolve(
        dir,
        "..",
        "npm",
        "tools",
        "node_modules",
        "npm",
        "bin",
        "npm-cli.js",
      ),
      path.resolve(
        dir,
        "..",
        "..",
        "npm",
        "tools",
        "node_modules",
        "npm",
        "bin",
        "npm-cli.js",
      ),
    ];
    for (const cliPath of cliCandidates) {
      if (isRegularFile(cliPath)) {
        return { command: process.execPath, prefixArgs: [cliPath] };
      }
    }

    const exePath = path.join(dir, "npm.exe");
    if (isRegularFile(exePath)) {
      return { command: exePath, prefixArgs: [] };
    }
  }

  throw new Error(
    `npm-cli.js or npm.exe was not found for ${process.execPath}`,
  );
}

/** Run `npm pack` without a shell and return the relative archive path. */
function packArchive(packDir) {
  const npmExecPath = process.env.npm_execpath;
  if (npmExecPath) {
    const isWin32Exe =
      process.platform === "win32" && /\.exe$/i.test(npmExecPath);
    const command = isWin32Exe ? npmExecPath : process.execPath;
    const prefixArgs = isWin32Exe ? [] : [npmExecPath];
    execFileSync(
      command,
      [...prefixArgs, "pack", "--pack-destination", packDir],
      { cwd: repoRoot, stdio: "inherit" },
    );
  } else if (process.platform === "win32") {
    const { command, prefixArgs } = resolveWin32Npm();
    execFileSync(
      command,
      [...prefixArgs, "pack", "--pack-destination", packDir],
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
