import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const DOCS_PACKAGE_PATH = join(REPO_ROOT, "docs", "package.json");
const RUN_PARALLEL_PATH = join(
  REPO_ROOT,
  "docs",
  "node_modules",
  "@takazudo",
  "zudo-doc",
  "bin",
  "run-parallel.mjs",
);
const PROCESS_TIMEOUT_MS = 10_000;
const POLL_INTERVAL_MS = 20;

const docsPackage = JSON.parse(readFileSync(DOCS_PACKAGE_PATH, "utf8"));
const runParallelAvailable = existsSync(RUN_PARALLEL_PATH);
const legacySupervisorName = ["npm", "run", "all2"].join("-");

function nodeScript(source) {
  const shellSafeSource = source.replaceAll("'", "'\\\"'\\\"'");
  return `node -e '${shellSafeSource}'`;
}

function serverScript(label, markerPath) {
  return [
    'const http = require("node:http");',
    'const fs = require("node:fs");',
    "const server = http.createServer();",
    `server.listen(0, () => { const { port } = server.address(); ${markerPath ? `fs.writeFileSync(${JSON.stringify(markerPath)}, String(port));` : ""} console.log("UP ${label}", port, "pid=" + process.pid); });`,
    "setInterval(() => {}, 1000);",
  ].join(" ");
}

function waitingFailureScript(markerPath) {
  return [
    'const fs = require("node:fs");',
    `const marker = ${JSON.stringify(markerPath)};`,
    "const check = () => { if (fs.existsSync(marker)) process.exit(3); };",
    `const watcher = fs.watch(${JSON.stringify(dirname(markerPath))}, check);`,
    "check();",
    'watcher.on("error", () => process.exit(1));',
  ].join(" ");
}

function createFixture() {
  const directory = mkdtempSync(join(tmpdir(), "zfb-run-parallel-"));
  const markerPath = join(directory, "up.ready");
  writeFileSync(
    join(directory, "package.json"),
    JSON.stringify(
      {
        name: "run-parallel-fixture",
        private: true,
        scripts: {
          up: nodeScript(serverScript("up", markerPath)),
          up2: nodeScript(serverScript("up2")),
          boom: nodeScript(waitingFailureScript(markerPath)),
        },
      },
      null,
      2,
    ),
  );
  return { directory, markerPath };
}

function lineCollector(stream) {
  let remainder = "";
  const lines = [];
  const waiters = [];
  stream.setEncoding("utf8");
  stream.on("data", (chunk) => {
    remainder += chunk;
    const parts = remainder.split(/\r?\n/);
    remainder = parts.pop();
    for (const line of parts) {
      lines.push(line);
      for (let index = waiters.length - 1; index >= 0; index -= 1) {
        const waiter = waiters[index];
        if (!waiter.predicate(line)) continue;
        waiters.splice(index, 1);
        clearTimeout(waiter.timer);
        waiter.resolve(line);
      }
    }
  });

  return {
    all() {
      return [...lines, ...(remainder ? [remainder] : [])].join("\n");
    },
    waitFor(predicate, timeoutMs = PROCESS_TIMEOUT_MS) {
      const existing = lines.find(predicate);
      if (existing !== undefined) return Promise.resolve(existing);
      return new Promise((resolvePromise, reject) => {
        const timer = setTimeout(() => {
          const index = waiters.findIndex((waiter) => waiter.resolve === resolvePromise);
          if (index !== -1) waiters.splice(index, 1);
          reject(new Error(`Timed out waiting for child output after ${timeoutMs}ms`));
        }, timeoutMs);
        waiters.push({ predicate, reject, resolve: resolvePromise, timer });
      });
    },
  };
}

function spawnSupervisor(directory, scripts) {
  const env = { ...process.env };
  delete env.npm_execpath;
  // Make the no-npm_execpath fallback deterministic even when Vitest itself was
  // launched through npm, pnpm, yarn, or bun.
  env.npm_config_user_agent = `pnpm/11.3.0 node/${process.versions.node} ${process.platform}/${process.arch}`;
  const child = spawn(process.execPath, [RUN_PARALLEL_PATH, ...scripts], {
    cwd: directory,
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const stdout = lineCollector(child.stdout);
  const stderr = lineCollector(child.stderr);
  const close = new Promise((resolvePromise, reject) => {
    child.once("error", reject);
    child.once("close", (code, signal) => resolvePromise({ code, signal }));
  });
  return { child, close, stderr, stdout };
}

async function waitForExit(close, timeoutMs = PROCESS_TIMEOUT_MS) {
  let timer;
  try {
    return await Promise.race([
      close,
      new Promise((_, reject) => {
        timer = setTimeout(
          () => reject(new Error(`Timed out waiting for supervisor after ${timeoutMs}ms`)),
          timeoutMs,
        );
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

function processIsAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code !== "ESRCH";
  }
}

async function waitUntil(predicate, timeoutMs = PROCESS_TIMEOUT_MS) {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() >= deadline) return false;
    await new Promise((resolvePromise) => setTimeout(resolvePromise, POLL_INTERVAL_MS));
  }
  return true;
}

function pidFromUpLine(line) {
  const match = line.match(/\bpid=(\d+)$/);
  if (!match) throw new Error(`UP line did not include a pid: ${line}`);
  return Number(match[1]);
}

function portFromUpLine(line) {
  const match = line.match(/^UP \S+ (\d+) pid=\d+$/);
  if (!match) throw new Error(`UP line did not include an ephemeral port: ${line}`);
  return Number(match[1]);
}

async function terminateForCleanup(supervisor, childPids) {
  if (supervisor.child.exitCode === null && supervisor.child.signalCode === null) {
    supervisor.child.kill("SIGKILL");
  }
  for (const pid of childPids) {
    if (processIsAlive(pid)) {
      try {
        process.kill(pid, "SIGKILL");
      } catch (error) {
        if (error.code !== "ESRCH") throw error;
      }
    }
  }
  try {
    await waitForExit(supervisor.close, 1_000);
  } catch {
    // The test's assertions report the original timeout/failure. Cleanup is best effort.
  }
}

// vitest 2.1.9's context.skip() ignores its reason argument, so the reasons live
// here: without docs/node_modules there is no run-parallel binary to spawn (run
// pnpm install), and the process-signal assertions only hold on macOS/Linux.
const supervisorRunnable = runParallelAvailable && process.platform !== "win32";
if (process.env.CI && !runParallelAvailable) {
  throw new Error(
    `Missing ${RUN_PARALLEL_PATH}; CI must install the docs workspace before running this test`,
  );
}

async function withSupervisor(scripts, run) {
  const fixture = createFixture();
  const supervisor = spawnSupervisor(fixture.directory, scripts);
  const childPids = [];
  try {
    await run(supervisor, childPids);
  } finally {
    await terminateForCleanup(supervisor, childPids);
    rmSync(fixture.directory, { recursive: true, force: true });
  }
}

describe("docs dev supervisor", () => {
  it("uses zudo-doc run-parallel for both docs dev scripts", () => {
    expect(docsPackage.scripts.dev).toMatch(/^run-parallel /);
    expect(docsPackage.scripts["dev:network"]).toMatch(/^run-parallel /);
    expect(docsPackage.devDependencies).not.toHaveProperty(legacySupervisorName);
  });

  // Worst case is 41s of chained PROCESS_TIMEOUT_MS waits (SIGINT test) plus the
  // 1s cleanup wait; 41s / 0.75 ~= 55s -> 60s so the inner waits fail first and
  // name their phase instead of vitest's bare 5s "Test timed out" (#2874, #2869).
  // Scoped to this describe: every other root suite keeps the 5s hang guardrail.
  describe.skipIf(!supervisorRunnable)("supervisor process behaviour", { timeout: 60_000 }, () => {
    it("aborts every sibling when a task exits non-zero", async () => {
      await withSupervisor(["up", "boom"], async (supervisor, childPids) => {
        const upLine = await supervisor.stdout.waitFor((line) => line.startsWith("UP up "));
        childPids.push(pidFromUpLine(upLine));
        expect(portFromUpLine(upLine)).toBeGreaterThan(0);
        const failure = await waitForExit(supervisor.close);
        expect(failure.code).toBe(3);
        expect(failure.signal).toBeNull();
        expect(supervisor.stderr.all()).toContain('ERROR: "boom" exited with 3.');
        expect(await waitUntil(() => !processIsAlive(childPids[0]))).toBe(true);
      });
    });

    it("forwards a supervisor-only SIGINT to the whole process tree", async () => {
      await withSupervisor(["up", "up2"], async (supervisor, childPids) => {
        const upLine = await supervisor.stdout.waitFor((line) => line.startsWith("UP up "));
        const up2Line = await supervisor.stdout.waitFor((line) => line.startsWith("UP up2 "));
        childPids.push(pidFromUpLine(upLine), pidFromUpLine(up2Line));
        expect(portFromUpLine(upLine)).toBeGreaterThan(0);
        expect(portFromUpLine(up2Line)).toBeGreaterThan(0);

        expect(supervisor.child.kill("SIGINT")).toBe(true);
        const signalExit = await waitForExit(supervisor.close);
        expect(signalExit.code).toBe(130);
        expect(signalExit.signal).toBeNull();
        expect(await waitUntil(() => childPids.every((pid) => !processIsAlive(pid)))).toBe(true);
      });
    });
  });
});
