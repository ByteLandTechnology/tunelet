/**
 * This module serves an unpacked package over HTTP and checks browser and
 * Web Worker loaders in headless Chromium through the DevTools Protocol.
 */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";

const CDP_TIMEOUT_MS = 15_000;

/** Check whether a path points to an executable file. */
function isExecutableFile(candidatePath) {
  try {
    const stat = fs.statSync(candidatePath);
    if (!stat.isFile()) {
      return false;
    }
    if (process.platform !== "win32") {
      fs.accessSync(candidatePath, fs.constants.X_OK);
    }
    return true;
  } catch {
    return false;
  }
}

/** Find a local Chrome or Chromium binary from CHROME_BIN, PATH or standard paths. */
export function findChromiumBinary() {
  const configured = process.env.CHROME_BIN?.trim();
  if (configured) {
    assert.ok(
      isExecutableFile(configured),
      `CHROME_BIN does not point to an executable file: ${configured}`,
    );
    return configured;
  }

  const binaryNames =
    process.platform === "win32"
      ? ["chrome.exe", "chromium.exe"]
      : [
          "google-chrome",
          "google-chrome-stable",
          "chrome",
          "chromium",
          "chromium-browser",
        ];

  const pathDirs = (process.env.PATH ?? "")
    .split(path.delimiter)
    .filter((entry) => entry.length > 0);
  for (const dir of pathDirs) {
    for (const name of binaryNames) {
      const candidate = path.join(dir, name);
      if (isExecutableFile(candidate)) {
        return candidate;
      }
    }
  }

  const fallbackPaths =
    process.platform === "darwin"
      ? [
          "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
          "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ]
      : process.platform === "win32"
        ? [
            path.join(
              process.env.ProgramFiles ?? "C:\\Program Files",
              "Google",
              "Chrome",
              "Application",
              "chrome.exe",
            ),
            path.join(
              process.env["ProgramFiles(x86)"] ?? "C:\\Program Files (x86)",
              "Google",
              "Chrome",
              "Application",
              "chrome.exe",
            ),
          ]
        : [
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/snap/bin/chromium",
          ];

  for (const candidate of fallbackPaths) {
    if (isExecutableFile(candidate)) {
      return candidate;
    }
  }

  throw new Error(
    "Headless Chromium was not found. Install Chrome or Chromium, or set CHROME_BIN.",
  );
}

/** Build the shared JavaScript helper that processes deterministic PCM blocks. */
function buildPcmHelperSource() {
  return `
function runDeterministicPcm(TuneletClass, defaultConfigFn) {
  const blockFrames = 128;
  const blockCount = 20;
  const period = 218;
  const config = defaultConfigFn();
  const processor = new TuneletClass(48000, blockFrames, config);
  let hash = 0x811c9dc5;
  let nonZeroSamples = 0;
  let latencyFrames = 0;
  let processedFrames = "0";
  try {
    latencyFrames = processor.latencyFrames;
    const input = new Int16Array(blockFrames);
    const left = new Int16Array(blockFrames);
    const right = new Int16Array(blockFrames);
    for (let block = 0; block < blockCount; block += 1) {
      const baseFrame = block * blockFrames;
      for (let i = 0; i < blockFrames; i += 1) {
        const phase = (baseFrame + i) % period;
        input[i] = Math.trunc((phase * 24000) / period) - 12000;
      }
      processor.process(input, left, right);
      for (let i = 0; i < blockFrames; i += 1) {
        if (left[i] !== 0) {
          nonZeroSamples += 1;
        }
        if (right[i] !== 0) {
          nonZeroSamples += 1;
        }
        hash = Math.imul(hash ^ (left[i] & 0xffff), 0x01000193) >>> 0;
        hash = Math.imul(hash ^ (right[i] & 0xffff), 0x01000193) >>> 0;
      }
    }
    processedFrames = String(processor.feedback().processedFrames);
  } finally {
    processor[Symbol.dispose]();
  }
  return { hash, nonZeroSamples, latencyFrames, processedFrames };
}
`;
}

/** Build the HTML page with an import map for the active package name. */
function buildImportMapPage(packageName) {
  const importMap = JSON.stringify(
    {
      imports: {
        [packageName]: "/dist/index.js",
        [`${packageName}/wasm`]: "/pkg/tunelet.js",
      },
    },
    null,
    2,
  );
  const pkgLiteral = JSON.stringify(packageName);
  const wasmLiteral = JSON.stringify(`${packageName}/wasm`);

  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <title>tunelet browser verification</title>
    <script type="importmap">
${importMap}
    </script>
  </head>
  <body>
    <main>
      <h1 id="heading">tunelet browser loader check</h1>
      <pre id="status">idle</pre>
    </main>
    <script type="module">
${buildPcmHelperSource()}

window.__runPrimaryChecks = async function runPrimaryChecks() {
  const statusNode = document.getElementById("status");
  statusNode.textContent = "running primary checks";
  const rootModule = await import(${pkgLiteral});
  const wasmSubpath = await import(${wasmLiteral});
  if (typeof rootModule.loadTunelet !== "function") {
    throw new Error("loadTunelet export was not a function");
  }
  if (typeof wasmSubpath.initSync !== "function") {
    throw new Error("subpath initSync export was not a function");
  }

  let streamCalls = 0;
  let bufferCalls = 0;
  const originalStream = WebAssembly.instantiateStreaming.bind(WebAssembly);
  const originalInstantiate = WebAssembly.instantiate.bind(WebAssembly);
  WebAssembly.instantiateStreaming = async function wrappedStream(...args) {
    streamCalls += 1;
    return originalStream(...args);
  };
  WebAssembly.instantiate = async function wrappedInstantiate(...args) {
    bufferCalls += 1;
    return originalInstantiate(...args);
  };

  let wasmExports;
  let notFoundMessage = "";
  try {
    let rejected = false;
    try {
      await rootModule.loadTunelet(new URL("/wasm/not-found.wasm", window.location.href));
    } catch (error) {
      rejected = true;
      notFoundMessage = error instanceof Error ? error.message : String(error);
    }
    if (!rejected || !notFoundMessage.includes("404")) {
      throw new Error("Expected HTTP 404 rejection, got: " + notFoundMessage);
    }

    wasmExports = await rootModule.loadTunelet();
    const cachedExports = await rootModule.loadTunelet(
      new URL("/wasm/not-found.wasm", window.location.href),
    );
    if (cachedExports !== wasmExports) {
      throw new Error("Second loadTunelet() call did not return the cached exports");
    }
  } finally {
    WebAssembly.instantiateStreaming = originalStream;
    WebAssembly.instantiate = originalInstantiate;
  }

  const mainPcm = runDeterministicPcm(rootModule.Tunelet, rootModule.defaultConfig);

  const workerResult = await new Promise((resolve, reject) => {
    const worker = new Worker("/worker.mjs", { type: "module" });
    const timer = setTimeout(() => {
      worker.terminate();
      reject(new Error("Web Worker timed out"));
    }, 10000);
    worker.addEventListener("message", (event) => {
      clearTimeout(timer);
      worker.terminate();
      if (event.data && event.data.ok) {
        resolve(event.data);
      } else {
        reject(new Error(event.data?.error ?? "Web Worker failed"));
      }
    });
    worker.addEventListener("error", (event) => {
      clearTimeout(timer);
      worker.terminate();
      reject(new Error(event.message || "Web Worker error event"));
    });
  });

  const summary = {
    packageName: ${pkgLiteral},
    notFoundMessage,
    streamCalls,
    bufferCalls,
    mainPcm,
    workerResult,
  };
  statusNode.textContent = JSON.stringify(summary, null, 2);
  return summary;
};

window.__runMimeFallbackCheck = async function runMimeFallbackCheck() {
  const statusNode = document.getElementById("status");
  statusNode.textContent = "running octet-stream fallback check";
  const rootModule = await import(${pkgLiteral});

  let streamCalls = 0;
  let bufferCalls = 0;
  const originalStream = WebAssembly.instantiateStreaming.bind(WebAssembly);
  const originalInstantiate = WebAssembly.instantiate.bind(WebAssembly);
  WebAssembly.instantiateStreaming = async function wrappedStream(...args) {
    streamCalls += 1;
    return originalStream(...args);
  };
  WebAssembly.instantiate = async function wrappedInstantiate(...args) {
    bufferCalls += 1;
    return originalInstantiate(...args);
  };

  try {
    await rootModule.loadTunelet(new URL("/wasm/octet-stream.wasm", window.location.href));
  } finally {
    WebAssembly.instantiateStreaming = originalStream;
    WebAssembly.instantiate = originalInstantiate;
  }

  const fallbackPcm = runDeterministicPcm(rootModule.Tunelet, rootModule.defaultConfig);
  const summary = {
    packageName: ${pkgLiteral},
    streamCalls,
    bufferCalls,
    fallbackPcm,
  };
  statusNode.textContent = JSON.stringify(summary, null, 2);
  return summary;
};

window.__fixtureReady = true;
    </script>
  </body>
</html>
`;
}

/** Build the module script that runs inside a dedicated Web Worker. */
function buildWorkerScript() {
  return `${buildPcmHelperSource()}

async function runWorker() {
  if (typeof window !== "undefined") {
    throw new Error("window must be undefined inside a Web Worker");
  }
  if (typeof document !== "undefined") {
    throw new Error("document must be undefined inside a Web Worker");
  }
  if (typeof DedicatedWorkerGlobalScope !== "function" || !(self instanceof DedicatedWorkerGlobalScope)) {
    throw new Error("self must be a DedicatedWorkerGlobalScope instance");
  }

  const rootModule = await import("/dist/index.js");

  let rejected = false;
  let notFoundMessage = "";
  try {
    await rootModule.loadTunelet(new URL("/wasm/not-found.wasm", self.location.href));
  } catch (error) {
    rejected = true;
    notFoundMessage = error instanceof Error ? error.message : String(error);
  }
  if (!rejected || !notFoundMessage.includes("404")) {
    throw new Error("Worker HTTP 404 check failed: " + notFoundMessage);
  }

  await rootModule.loadTunelet();
  const pcm = runDeterministicPcm(rootModule.Tunelet, rootModule.defaultConfig);
  self.postMessage({ ok: true, notFoundMessage, pcm });
}

runWorker().catch((error) => {
  self.postMessage({
    ok: false,
    error: error instanceof Error ? (error.stack ?? error.message) : String(error),
  });
});
`;
}

/** Start a local HTTP server for the unpacked package files. */
async function startFixtureServer(installedDir, packageName) {
  const distIndexJs = fs.readFileSync(path.join(installedDir, "dist", "index.js"));
  const pkgTuneletJs = fs.readFileSync(
    path.join(installedDir, "pkg", "tunelet.js"),
  );
  const pkgTuneletWasm = fs.readFileSync(
    path.join(installedDir, "pkg", "tunelet_bg.wasm"),
  );
  const htmlBody = Buffer.from(buildImportMapPage(packageName), "utf8");
  const workerBody = Buffer.from(buildWorkerScript(), "utf8");

  const hits = {
    page: 0,
    distJs: 0,
    pkgJs: 0,
    pkgWasm: 0,
    octetStreamWasm: 0,
    notFoundWasm: 0,
    workerJs: 0,
  };

  const server = http.createServer((req, res) => {
    const requestUrl = new URL(req.url ?? "/", "http://127.0.0.1");
    const pathname = requestUrl.pathname;

    res.setHeader("Cache-Control", "no-store");

    if (pathname === "/" || pathname === "/index.html") {
      hits.page += 1;
      res.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
      res.end(htmlBody);
      return;
    }

    if (pathname === "/dist/index.js") {
      hits.distJs += 1;
      res.writeHead(200, { "Content-Type": "text/javascript; charset=utf-8" });
      res.end(distIndexJs);
      return;
    }

    if (pathname === "/pkg/tunelet.js") {
      hits.pkgJs += 1;
      res.writeHead(200, { "Content-Type": "text/javascript; charset=utf-8" });
      res.end(pkgTuneletJs);
      return;
    }

    if (pathname === "/pkg/tunelet_bg.wasm") {
      hits.pkgWasm += 1;
      res.writeHead(200, { "Content-Type": "application/wasm" });
      res.end(pkgTuneletWasm);
      return;
    }

    if (pathname === "/wasm/octet-stream.wasm") {
      hits.octetStreamWasm += 1;
      res.writeHead(200, { "Content-Type": "application/octet-stream" });
      res.end(pkgTuneletWasm);
      return;
    }

    if (pathname === "/worker.mjs") {
      hits.workerJs += 1;
      res.writeHead(200, { "Content-Type": "text/javascript; charset=utf-8" });
      res.end(workerBody);
      return;
    }

    if (pathname === "/wasm/not-found.wasm") {
      hits.notFoundWasm += 1;
    }
    res.writeHead(404, { "Content-Type": "text/plain; charset=utf-8" });
    res.end("Not Found");
  });

  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });

  const address = server.address();
  assert.ok(
    address && typeof address === "object",
    "Expected an IPv4 server address",
  );

  return {
    origin: `http://127.0.0.1:${address.port}`,
    hits,
    async close() {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

/** Start headless Chromium and return a Chrome DevTools Protocol client. */
async function launchChromiumCdp(chromiumBin) {
  const userDataDir = fs.mkdtempSync(
    path.join(os.tmpdir(), "tunelet-chromium-"),
  );
  const child = spawn(
    chromiumBin,
    [
      "--headless",
      "--no-sandbox",
      "--disable-gpu",
      "--disable-dev-shm-usage",
      "--disable-extensions",
      "--disable-background-networking",
      "--no-first-run",
      "--no-default-browser-check",
      `--user-data-dir=${userDataDir}`,
      "--remote-debugging-port=0",
      "about:blank",
    ],
    { stdio: ["ignore", "ignore", "pipe"] },
  );

  let stderrText = "";
  let exited = false;
  const exitPromise = new Promise((resolve) => {
    child.once("exit", () => {
      exited = true;
      resolve();
    });
  });

  let ws;
  try {
    const wsUrl = await new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        reject(
          new Error(
            `Timed out before Chromium printed a DevTools URL.\n${stderrText}`,
          ),
        );
      }, CDP_TIMEOUT_MS);

      child.stderr.on("data", (chunk) => {
        stderrText += chunk.toString("utf8");
        const match = stderrText.match(/DevTools listening on (ws:\/\/[^\s]+)/);
        if (match) {
          clearTimeout(timer);
          resolve(match[1]);
        }
      });

      child.once("error", (error) => {
        clearTimeout(timer);
        reject(error);
      });

      child.once("exit", (code) => {
        clearTimeout(timer);
        reject(
          new Error(
            `Chromium exited before CDP startup with code ${code}.\n${stderrText}`,
          ),
        );
      });
    });

    ws = new WebSocket(wsUrl);
    await new Promise((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error("Timed out on CDP WebSocket open")),
        CDP_TIMEOUT_MS,
      );
      ws.addEventListener(
        "open",
        () => {
          clearTimeout(timer);
          resolve();
        },
        { once: true },
      );
      ws.addEventListener(
        "error",
        () => {
          clearTimeout(timer);
          reject(new Error("Failed to open CDP WebSocket"));
        },
        { once: true },
      );
    });
  } catch (error) {
    if (!exited) {
      child.kill("SIGKILL");
      await exitPromise;
    }
    fs.rmSync(userDataDir, {
      recursive: true,
      force: true,
      maxRetries: 10,
      retryDelay: 100,
    });
    throw error;
  }

  let nextId = 1;
  const pending = new Map();
  const listeners = new Set();

  ws.addEventListener("message", (event) => {
    const payload = JSON.parse(String(event.data));
    if (typeof payload.id === "number") {
      const entry = pending.get(payload.id);
      if (entry) {
        pending.delete(payload.id);
        clearTimeout(entry.timer);
        if (payload.error) {
          entry.reject(
            new Error(
              `CDP ${entry.method} failed: ${JSON.stringify(payload.error)}`,
            ),
          );
        } else {
          entry.resolve(payload.result);
        }
      }
      return;
    }
    for (const listener of listeners) {
      listener(payload);
    }
  });

  ws.addEventListener("close", () => {
    for (const [, entry] of pending) {
      clearTimeout(entry.timer);
      entry.reject(new Error(`CDP socket closed before ${entry.method} ended`));
    }
    pending.clear();
  });

  function send(method, params = {}, sessionId = undefined) {
    const id = nextId;
    nextId += 1;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`CDP ${method} timed out after ${CDP_TIMEOUT_MS}ms`));
      }, CDP_TIMEOUT_MS);
      pending.set(id, { method, timer, resolve, reject });
      const message = sessionId
        ? { id, sessionId, method, params }
        : { id, method, params };
      ws.send(JSON.stringify(message));
    });
  }

  function onEvent(handler) {
    listeners.add(handler);
    return () => listeners.delete(handler);
  }

  async function close() {
    try {
      if (ws.readyState === WebSocket.OPEN) {
        await send("Browser.close").catch(() => {});
        ws.close();
      }
    } finally {
      if (!exited) {
        const killTimer = setTimeout(() => {
          if (!exited) {
            child.kill("SIGKILL");
          }
        }, 3_000);
        await exitPromise;
        clearTimeout(killTimer);
      }
      fs.rmSync(userDataDir, {
        recursive: true,
        force: true,
        maxRetries: 10,
        retryDelay: 100,
      });
    }
  }

  return { send, onEvent, close };
}

/** Open an isolated page target, run an async expression and close the target. */
async function evaluateInFreshTarget(cdp, pageUrl, expression) {
  const { targetId } = await cdp.send("Target.createTarget", {
    url: "about:blank",
  });

  const warnings = [];
  const exceptions = [];
  let sessionId;
  let unsubscribe = () => {};

  try {
    const attached = await cdp.send("Target.attachToTarget", {
      targetId,
      flatten: true,
    });
    sessionId = attached.sessionId;

    const loadFired = new Promise((resolve) => {
      const removeLoadListener = cdp.onEvent((msg) => {
        if (
          msg.sessionId === sessionId &&
          msg.method === "Page.loadEventFired"
        ) {
          removeLoadListener();
          resolve();
        }
      });
    });

    unsubscribe = cdp.onEvent((msg) => {
      if (msg.sessionId !== sessionId) {
        return;
      }
      if (msg.method === "Runtime.consoleAPICalled") {
        const text = (msg.params?.args ?? [])
          .map((arg) => arg.value ?? arg.description ?? "")
          .join(" ");
        if (msg.params?.type === "warning") {
          warnings.push(text);
        }
      } else if (msg.method === "Runtime.exceptionThrown") {
        const details = msg.params?.exceptionDetails;
        exceptions.push(
          details?.exception?.description ?? details?.text ?? "Page exception",
        );
      }
    });

    await cdp.send("Page.enable", {}, sessionId);
    await cdp.send("Runtime.enable", {}, sessionId);
    await cdp.send("Page.navigate", { url: pageUrl }, sessionId);
    await loadFired;

    const readyCheck = await cdp.send(
      "Runtime.evaluate",
      {
        expression: `new Promise((resolve, reject) => {
          const deadline = Date.now() + 5000;
          function poll() {
            if (window.__fixtureReady === true) {
              resolve(true);
              return;
            }
            if (Date.now() > deadline) {
              reject(new Error("Import map module script did not initialize"));
              return;
            }
            setTimeout(poll, 10);
          }
          poll();
        })`,
        awaitPromise: true,
        returnByValue: true,
      },
      sessionId,
    );
    if (readyCheck.exceptionDetails) {
      throw new Error(
        readyCheck.exceptionDetails.exception?.description ??
          readyCheck.exceptionDetails.text,
      );
    }

    const evaluation = await cdp.send(
      "Runtime.evaluate",
      {
        expression,
        awaitPromise: true,
        returnByValue: true,
      },
      sessionId,
    );
    if (evaluation.exceptionDetails) {
      throw new Error(
        evaluation.exceptionDetails.exception?.description ??
          evaluation.exceptionDetails.text,
      );
    }
    assert.deepEqual(
      exceptions,
      [],
      `Uncaught page exceptions: ${exceptions.join("\n")}`,
    );

    return { value: evaluation.result?.value, warnings };
  } finally {
    unsubscribe();
    await cdp.send("Target.closeTarget", { targetId }).catch(() => {});
  }
}

/** Run the HTTP 404 retry, default fetch, MIME fallback and worker checks. */
async function verifyPackageInChromium(cdp, installedDir, packageName) {
  const fixtureServer = await startFixtureServer(installedDir, packageName);
  try {
    const primary = await evaluateInFreshTarget(
      cdp,
      `${fixtureServer.origin}/index.html?mode=primary`,
      "window.__runPrimaryChecks()",
    );
    assert.ok(primary.value, "Expected primary browser check output");
    assert.equal(primary.value.packageName, packageName);
    assert.equal(
      primary.value.streamCalls,
      1,
      "Expected one WebAssembly.instantiateStreaming call on application/wasm",
    );
    assert.equal(
      primary.value.bufferCalls,
      0,
      "Expected zero WebAssembly.instantiate fallback calls on application/wasm",
    );
    assert.deepEqual(
      primary.warnings,
      [],
      `Unexpected warnings on application/wasm: ${primary.warnings.join("\n")}`,
    );

    const { mainPcm, workerResult } = primary.value;
    assert.equal(mainPcm.latencyFrames, 1593);
    assert.equal(mainPcm.processedFrames, "2560");
    assert.ok(
      mainPcm.nonZeroSamples > 0,
      "Expected non-zero PCM output on the main thread",
    );

    assert.equal(workerResult.ok, true);
    assert.deepEqual(
      workerResult.pcm,
      mainPcm,
      "Expected Web Worker PCM output to match the main thread",
    );

    const fallback = await evaluateInFreshTarget(
      cdp,
      `${fixtureServer.origin}/index.html?mode=octet-stream`,
      "window.__runMimeFallbackCheck()",
    );
    assert.ok(fallback.value, "Expected MIME fallback check output");
    assert.equal(
      fallback.value.streamCalls,
      1,
      "Expected WebAssembly.instantiateStreaming attempt on application/octet-stream",
    );
    assert.equal(
      fallback.value.bufferCalls,
      1,
      "Expected WebAssembly.instantiate fallback on application/octet-stream",
    );
    assert.ok(
      fallback.warnings.some((entry) => entry.includes("application/wasm")),
      "Expected MIME fallback warning on application/octet-stream",
    );
    assert.deepEqual(
      fallback.value.fallbackPcm,
      mainPcm,
      "Expected MIME fallback PCM output to match the main thread",
    );

    assert.equal(fixtureServer.hits.page, 2);
    assert.ok(fixtureServer.hits.distJs >= 2);
    assert.ok(fixtureServer.hits.pkgJs >= 2);
    assert.equal(
      fixtureServer.hits.pkgWasm,
      2,
      "Expected two application/wasm loads (main thread and Web Worker)",
    );
    assert.equal(
      fixtureServer.hits.octetStreamWasm,
      1,
      "Expected one application/octet-stream load",
    );
    assert.equal(
      fixtureServer.hits.notFoundWasm,
      2,
      "Expected two HTTP 404 probes (main thread and Web Worker)",
    );
    assert.equal(fixtureServer.hits.workerJs, 1);

    return mainPcm;
  } finally {
    await fixtureServer.close();
  }
}

/** Check all unpacked package variants in one headless Chromium instance. */
export async function verifyBrowserLoaders(targets) {
  const chromiumBin = findChromiumBinary();
  const cdp = await launchChromiumCdp(chromiumBin);
  try {
    let referencePcm;
    for (const { installedDir, packageName } of targets) {
      const pcm = await verifyPackageInChromium(cdp, installedDir, packageName);
      if (referencePcm === undefined) {
        referencePcm = pcm;
      } else {
        assert.deepEqual(
          pcm,
          referencePcm,
          `Expected identical PCM output for ${packageName}`,
        );
      }
    }
    return referencePcm;
  } finally {
    await cdp.close();
  }
}
