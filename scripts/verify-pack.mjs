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

/** Extract JSDoc comment blocks from a file and strip their comment markers. */
function extractJsDocBlocks(source) {
  return [...source.matchAll(/\/\*\*[\s\S]*?\*\//g)].map((match) => {
    const block = match[0];
    if (!block.includes("\n")) {
      return block.slice(3, -2).trim();
    }
    return block
      .split("\n")
      .slice(1, -1)
      .map((line) => line.replace(/^[ \t]*\*(?: |$)/, ""))
      .join("\n");
  });
}

/** Extract the JSDoc block immediately before a named member or export. */
function extractMemberJsDoc(source, pattern, label) {
  const match = pattern.exec(source);
  assert.ok(match, `Expected JSDoc block for ${label}`);
  return extractJsDocBlocks(match[1])[0];
}

/** Check that packed JSDoc comments contain clean TypeScript documentation. */
function verifyNormalizedJsDoc(consumerDir, packageName) {
  const installedDir = path.join(
    consumerDir,
    "node_modules",
    ...packageName.split("/"),
  );
  const filesToCheck = [
    path.join(installedDir, "dist", "index.d.ts"),
    path.join(installedDir, "dist", "index.js"),
    path.join(installedDir, "pkg", "tunelet.d.ts"),
    path.join(installedDir, "pkg", "tunelet.js"),
  ];

  for (const filePath of filesToCheck) {
    const content = fs.readFileSync(filePath, "utf8");
    const blocks = extractJsDocBlocks(content);
    assert.ok(blocks.length > 0, `Expected JSDoc blocks in ${filePath}`);
    for (const text of blocks) {
      assert.doesNotMatch(
        text,
        /(?:^|\n)#+\s/,
        `Unexpected Markdown heading in ${filePath}`,
      );
      assert.ok(
        !text.includes("::"),
        `Unexpected Rust path separator in ${filePath}`,
      );
      assert.ok(
        !text.includes("..="),
        `Unexpected Rust inclusive range in ${filePath}`,
      );
      assert.doesNotMatch(
        text,
        /\b(?:None|NONE)\b/,
        `Unexpected Rust None literal in ${filePath}`,
      );
      assert.doesNotMatch(
        text,
        /\[`[^`]+`\]/,
        `Unexpected Rust intra-doc link in ${filePath}`,
      );
      assert.ok(
        !text.includes("`Default`"),
        `Unexpected Rust Default trait reference in ${filePath}`,
      );
      assert.doesNotMatch(
        text,
        /`(?:max_frequency_hz|min_frequency_hz|stereo_output|retune_ms|attack_ms)`/,
        `Unexpected snake_case config field in ${filePath}`,
      );
    }
  }

  const dts = fs.readFileSync(
    path.join(installedDir, "pkg", "tunelet.d.ts"),
    "utf8",
  );
  const throwsCount = [...dts.matchAll(/\* @throws /g)].length;
  assert.equal(throwsCount, 13, `Expected 13 @throws tags, found ${throwsCount}`);

  const latencyDoc = extractMemberJsDoc(
    dts,
    /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+readonly latencyFrames: number;/,
    "latencyFrames",
  );
  assert.ok(!latencyDoc.includes("latencyFrames"));
  assert.ok(latencyDoc.includes("1,543 frames"));

  const beginDrainDoc = extractMemberJsDoc(
    dts,
    /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+beginDrain\(\): number;/,
    "beginDrain",
  );
  assert.ok(!beginDrainDoc.includes("beginDrain"));
  assert.ok(beginDrainDoc.includes("returns zero"));
  assert.ok(beginDrainDoc.includes("Completion resets"));

  const drainMonoDoc = extractMemberJsDoc(
    dts,
    /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)\s+drainMono\(audio: Int16Array\): number;/,
    "drainMono",
  );
  assert.ok(!drainMonoDoc.includes("drainMono"));
  assert.ok(drainMonoDoc.includes("mono downmix"));
  assert.ok(drainMonoDoc.includes("Completion resets"));
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
  verifyNormalizedJsDoc(consumerDir, packageName);
} finally {
  fs.rmSync(tempDir, { recursive: true, force: true });
}
