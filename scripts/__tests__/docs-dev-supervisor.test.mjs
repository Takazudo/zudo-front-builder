import { execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const DOCS_PACKAGE_PATH = join(REPO_ROOT, "docs", "package.json");
const ZUDO_DOC_PACKAGE_PATH = join(
  REPO_ROOT,
  "docs",
  "node_modules",
  "@takazudo",
  "zudo-doc",
  "package.json",
);
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

// Deliberately short, and deliberately NOT PROCESS_TIMEOUT_MS: the pre-UP
// regression below waits for a line that is designed never to arrive, so this is
// the budget for proving a negative rather than the budget for a wait that is
// expected to succeed.
const PRE_UP_FAILURE_TIMEOUT_MS = 250;

/**
 * Narrow predicate for the ONE deliberate throw the pre-UP regression test
 * (#2894) induces: the "hidden" case's wait for a "UP hidden ..." line that is
 * designed never to arrive. Matched on the exact wait label and timeout
 * message -- never a blanket "the hidden case threw" -- so a genuine
 * regression in that test (a bad assertion, a setup failure) still reports
 * outcome=failed instead of re-poisoning the field this fix exists to clean
 * up (#2904).
 */
function isExpectedHiddenTimeout(error) {
  return (
    error instanceof Error &&
    error.message.includes("Timed out waiting for child output") &&
    error.message.includes("UP hidden <port> pid=<pid>")
  );
}

// Cleanup re-collects the descendant tree this many times. A mid-tier process can
// exit and reparent its descendants between passes, so one pass is not enough;
// more than a handful is gold-plating a teardown path that the product itself
// ships as a single pass.
const CLEANUP_TREE_PASSES = 3;

// How long the pre-UP regression polls for the reaped tree to actually disappear.
// Bottom-up SIGKILL leaves each intermediate process a zombie until its own
// parent dies and init reaps it, so "gone" is reached by polling, not instantly.
const REAP_CONFIRM_TIMEOUT_MS = 5_000;

// Evidence budgets. A wait failure has to stay readable in a CI log, so every
// captured blob is tail-truncated with an explicit "N chars omitted" marker
// rather than silently cut.
const EVIDENCE_STREAM_CHARS = 1_500;
const EVIDENCE_FIXTURE_CHARS = 1_400;
const EVIDENCE_ENV_VALUE_CHARS = 120;

// Environment entries that actually steer this spawn: the two run-parallel reads
// to pick a package manager, plus the ones that change how node/pnpm start up or
// which vitest worker we are sharing the machine with. `npm_execpath` is always
// <unset> here by construction (spawnSupervisor deletes it); the ambient value
// vitest itself was launched with is reported separately on its own line.
const REPORTED_ENV_KEYS = [
  "npm_execpath",
  "npm_config_user_agent",
  "NODE_OPTIONS",
  "NODE_ENV",
  "CI",
  "PNPM_HOME",
  "TMPDIR",
  "VITEST_POOL_ID",
  "VITEST_WORKER_ID",
  "TINYPOOL_WORKER_ID",
];

// Vitest reassigns these per run purely from worker scheduling -- a single-file
// run and a full-suite run of the same commit differ in nothing else. Folding
// them into the env digest would make every baseline-vs-load comparison report
// input drift that is not there, so they are reported by name and excluded from
// the comparator.
const VOLATILE_ENV_KEYS = new Set(["TINYPOOL_WORKER_ID", "VITEST_POOL_ID", "VITEST_WORKER_ID"]);

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

/**
 * A leaf that announces its pid ONLY by writing it to a side-channel file, and
 * never prints an `UP` line. Cleanup therefore cannot learn about it from stdout
 * -- which is precisely the descendant #2894 leaked when an inner wait timed out
 * before `UP`.
 */
function hiddenLeafScript(pidPath) {
  return [
    'const fs = require("node:fs");',
    `fs.writeFileSync(${JSON.stringify(pidPath)}, String(process.pid));`,
    "setInterval(() => {}, 1000);",
  ].join(" ");
}

function createFixture() {
  const directory = mkdtempSync(join(tmpdir(), "zfb-run-parallel-"));
  const markerPath = join(directory, "up.ready");
  const hiddenPidPath = join(directory, "hidden.pid");
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
          hidden: nodeScript(hiddenLeafScript(hiddenPidPath)),
        },
      },
      null,
      2,
    ),
  );
  return { directory, hiddenPidPath, markerPath };
}

function digestOf(text) {
  return `sha256:${createHash("sha256").update(text).digest("hex").slice(0, 16)}`;
}

function truncateTail(text, limit) {
  if (!text) return "(empty)";
  if (text.length <= limit) return text;
  return `...(${text.length - limit} earlier chars omitted)...\n${text.slice(-limit)}`;
}

function truncateHead(text, limit) {
  if (!text) return "(empty)";
  if (text.length <= limit) return text;
  return `${text.slice(0, limit)}\n...(${text.length - limit} later chars omitted)...`;
}

/** Newline-free variant, for values spliced into a single-line field. */
function truncateInline(text, limit) {
  if (!text) return "(empty)";
  if (text.length <= limit) return text;
  return `${text.slice(0, limit)}...(${text.length - limit} more chars)`;
}

function indent(text, prefix = "    ") {
  return text
    .split("\n")
    .map((line) => `${prefix}${line}`)
    .join("\n");
}

/**
 * Mirror of run-parallel.mjs's own package-manager resolution, so the runner it
 * WILL pick is part of the captured input rather than something only inferable
 * from the outcome. Mirrored rather than imported: run-parallel is a shipped
 * artifact of @takazudo/zudo-doc with no exported module entry point, and a
 * drift between the two shows up as a runner line that disagrees with the
 * observed child output.
 */
function resolveRunnerForEnv(env) {
  const execpath = env.npm_execpath;
  if (execpath) {
    return { command: execpath, viaNode: /\.(c|m)?js$/.test(execpath) };
  }
  const agent = env.npm_config_user_agent ?? "";
  const command = ["pnpm", "yarn", "bun"].find((pm) => agent.startsWith(pm)) ?? "npm";
  return { command, viaNode: false };
}

function resolveOnPath(command, pathValue) {
  if (command.includes("/")) return command;
  for (const directory of (pathValue ?? "").split(":")) {
    if (!directory) continue;
    const candidate = join(directory, command);
    if (existsSync(candidate)) return candidate;
  }
  return null;
}

function envSlice(env) {
  const comparedKeys = Object.keys(env)
    .filter((key) => !VOLATILE_ENV_KEYS.has(key))
    .sort();
  const serialized = comparedKeys.map((key) => `${key}=${env[key]}`).join("\n");
  const reported = REPORTED_ENV_KEYS.map((key) => {
    const value = env[key];
    if (value === undefined) return `${key}=<unset>`;
    return `${key}=${truncateInline(value, EVIDENCE_ENV_VALUE_CHARS)}`;
  });
  const skipped = Object.keys(env).length - comparedKeys.length;
  return {
    digest: `${digestOf(serialized)} over ${comparedKeys.length} vars (${skipped} volatile excluded)`,
    reported,
  };
}

/**
 * Capture everything Rule 1 compares -- argv, cwd, resolved binaries, package
 * versions, environment, fixture contents -- BEFORE the spawn. A run whose child
 * never starts still has to produce this, because "no child appeared" is an
 * outcome, not evidence that the input differed.
 */
function captureSpawnInput(directory, scripts, env) {
  const fixtureSource = readFileSync(join(directory, "package.json"), "utf8");
  // The fixture lives in a fresh mkdtemp directory, so its raw bytes differ on
  // every run by design. Eliding that one path yields a digest that is stable
  // across runs and is therefore the comparator Rule 1 actually wants.
  const fixtureShape = fixtureSource.replaceAll(directory, "<FIXTURE_DIR>");
  const runner = resolveRunnerForEnv(env);
  let runParallel;
  try {
    const source = readFileSync(RUN_PARALLEL_PATH);
    runParallel = `${statSync(RUN_PARALLEL_PATH).size} bytes ${digestOf(source)}`;
  } catch (error) {
    runParallel = `unreadable (${error.code ?? error.message})`;
  }
  let zudoDocVersion;
  try {
    zudoDocVersion = JSON.parse(readFileSync(ZUDO_DOC_PACKAGE_PATH, "utf8")).version;
  } catch (error) {
    zudoDocVersion = `unreadable (${error.code ?? error.message})`;
  }
  return {
    ambientNpmExecpath: process.env.npm_execpath ?? "<unset>",
    argv: [process.execPath, RUN_PARALLEL_PATH, ...scripts],
    cwd: directory,
    env: envSlice(env),
    fixture: {
      digest: digestOf(fixtureSource),
      shapeDigest: digestOf(fixtureShape),
      source: fixtureSource,
    },
    node: process.version,
    runParallel,
    runner: {
      command: runner.command,
      resolved: resolveOnPath(runner.command, env.PATH) ?? "not found on PATH",
      viaNode: runner.viaNode,
    },
    zudoDocVersion,
  };
}

function formatInput(input) {
  return [
    "INPUT (captured before spawn)",
    `  argv: ${input.argv.join(" ")}`,
    `  cwd: ${input.cwd}`,
    `  node: ${input.node}`,
    `  runner run-parallel will resolve: ${input.runner.command} -> ${input.runner.resolved}${
      input.runner.viaNode ? " (run through node)" : ""
    }`,
    `  npm_execpath vitest itself was launched with: ${input.ambientNpmExecpath}`,
    `  @takazudo/zudo-doc: ${input.zudoDocVersion}`,
    `  run-parallel.mjs: ${input.runParallel}`,
    `  env: ${input.env.reported.join(" ")}`,
    `  env digest: ${input.env.digest}`,
    `  fixture package.json: ${input.fixture.digest} (path-independent shape ${input.fixture.shapeDigest})`,
    indent(truncateHead(input.fixture.source, EVIDENCE_FIXTURE_CHARS), "    "),
  ].join("\n");
}

/**
 * Phase clock plus evidence formatter shared by every inner wait. The phase
 * marks are what separate "the package manager never started" from "the server
 * never listened" from "run-parallel never tore the siblings down" -- the three
 * suspects a bare `Timed out waiting for child output` cannot tell apart.
 */
function createDiagnostics(input) {
  const startedAt = performance.now();
  const marks = [];
  const childPids = [];
  let child = null;

  const elapsedMs = () => Math.round(performance.now() - startedAt);
  const mark = (phase) => {
    if (marks.some((entry) => entry.phase === phase)) return;
    marks.push({ atMs: elapsedMs(), phase });
  };

  const liveness = () => {
    const lines = [];
    if (!child) {
      lines.push("  supervisor: not spawned yet");
    } else if (child.pid === undefined) {
      lines.push("  supervisor: spawn failed, no pid was ever assigned");
    } else {
      lines.push(
        `  supervisor: pid=${child.pid} alive=${processIsAlive(child.pid)} exitCode=${child.exitCode} signalCode=${child.signalCode}`,
      );
    }
    if (childPids.length === 0) {
      // The child pid is only knowable from the UP line the server prints, so
      // before that phase there is genuinely nothing to report here. Say which
      // of the two reasons applies rather than asserting the stronger one.
      const upLine = marks.find((entry) => entry.phase === "first-up-line");
      lines.push(
        upLine
          ? `  children: not recorded yet -- an UP line arrived at ${upLine.atMs}ms but the test had not read its pid`
          : "  children: unknown -- no UP line has been observed, so no child pid exists to report",
      );
    } else {
      lines.push(
        `  children: ${childPids.map((pid) => `pid=${pid} alive=${processIsAlive(pid)}`).join(" ")}`,
      );
    }
    return lines.join("\n");
  };

  let readStreams = () => ({ stderr: "", stdout: "" });

  return {
    attach(spawned) {
      child = spawned;
    },
    /** Wired after spawn, once the line collectors that own the buffers exist. */
    attachStreams(reader) {
      readStreams = reader;
    },
    childPids,
    /** Bounded evidence block appended to every inner-wait rejection. */
    evidence(phase, timeoutMs) {
      const streams = readStreams();
      return [
        "--- supervisor diagnostics ---",
        `awaiting: ${phase}   elapsed: ${elapsedMs()}ms   deadline: ${timeoutMs}ms`,
        formatInput(input),
        "TIMELINE (ms from spawn)",
        marks.length === 0
          ? "  (no phase reached)"
          : marks.map((entry) => `  ${entry.phase}: ${entry.atMs}`).join("\n"),
        "LIVENESS",
        liveness(),
        `SUPERVISOR STDOUT (last ${EVIDENCE_STREAM_CHARS} chars)`,
        indent(truncateTail(streams.stdout, EVIDENCE_STREAM_CHARS)),
        `SUPERVISOR STDERR (last ${EVIDENCE_STREAM_CHARS} chars)`,
        indent(truncateTail(streams.stderr, EVIDENCE_STREAM_CHARS)),
        "--- end supervisor diagnostics ---",
      ].join("\n");
    },
    mark,
    /**
     * One machine-parseable line per supervisor run: the identity of the INPUT
     * alongside the phase timings, so a sampled distribution can be checked for
     * input drift instead of assuming there was none.
     */
    timelineLine(label, outcome) {
      const identity = [
        `runner=${input.runner.command}`,
        `zudoDoc=${input.zudoDocVersion}`,
        `runParallel=${input.runParallel.split(" ").pop()}`,
        `fixtureShape=${input.fixture.shapeDigest}`,
        `env=${input.env.digest.split(" ")[0]}`,
      ].join(" ");
      return `[supervisor-timeline] case=${label} outcome=${outcome} total=${elapsedMs()} ${identity} ${marks
        .map((entry) => `${entry.phase}=${entry.atMs}`)
        .join(" ")}`;
    },
  };
}

function lineCollector(stream, diagnostics, streamName) {
  let remainder = "";
  const lines = [];
  const waiters = [];
  stream.setEncoding("utf8");
  stream.on("data", (chunk) => {
    diagnostics.mark(`first-${streamName}-byte`);
    remainder += chunk;
    const parts = remainder.split(/\r?\n/);
    remainder = parts.pop();
    for (const line of parts) {
      lines.push(line);
      if (line.startsWith("UP ")) diagnostics.mark("first-up-line");
      if (line.startsWith("ERROR: ")) diagnostics.mark("supervisor-error-line");
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
    waitFor(predicate, label, timeoutMs = PROCESS_TIMEOUT_MS) {
      const existing = lines.find(predicate);
      if (existing !== undefined) return Promise.resolve(existing);
      return new Promise((resolvePromise, reject) => {
        const timer = setTimeout(() => {
          const index = waiters.findIndex((waiter) => waiter.resolve === resolvePromise);
          if (index !== -1) waiters.splice(index, 1);
          reject(
            new Error(
              `Timed out waiting for child output after ${timeoutMs}ms\n${diagnostics.evidence(`${streamName} line: ${label}`, timeoutMs)}`,
            ),
          );
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
  const diagnostics = createDiagnostics(captureSpawnInput(directory, scripts, env));
  const child = spawn(process.execPath, [RUN_PARALLEL_PATH, ...scripts], {
    cwd: directory,
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  diagnostics.attach(child);
  diagnostics.mark("supervisor-spawned");
  const stdout = lineCollector(child.stdout, diagnostics, "stdout");
  const stderr = lineCollector(child.stderr, diagnostics, "stderr");
  diagnostics.attachStreams(() => ({ stderr: stderr.all(), stdout: stdout.all() }));
  const close = new Promise((resolvePromise, reject) => {
    child.once("error", (error) => {
      diagnostics.mark("supervisor-spawn-error");
      reject(error);
    });
    child.once("close", (code, signal) => {
      diagnostics.mark("supervisor-closed");
      resolvePromise({ code, signal });
    });
  });
  return { child, close, diagnostics, stderr, stdout };
}

async function waitForExit(supervisor, timeoutMs = PROCESS_TIMEOUT_MS) {
  const { diagnostics } = supervisor;
  let timer;
  try {
    return await Promise.race([
      supervisor.close,
      new Promise((_, reject) => {
        timer = setTimeout(
          () =>
            reject(
              new Error(
                `Timed out waiting for supervisor after ${timeoutMs}ms\n${diagnostics.evidence("supervisor close", timeoutMs)}`,
              ),
            ),
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

/**
 * Poll every pid until `process.kill(pid, 0)` reports ESRCH, and return whatever
 * is still alive at the deadline. Polled rather than sampled once: killing a tree
 * bottom-up leaves each intermediate process a zombie until its own parent dies
 * and init reaps it, and a zombie still answers signal 0.
 */
async function waitForAllGone(pids, timeoutMs = REAP_CONFIRM_TIMEOUT_MS) {
  const deadline = Date.now() + timeoutMs;
  let alive = pids.filter(processIsAlive);
  while (alive.length > 0 && Date.now() < deadline) {
    await new Promise((resolvePromise) => setTimeout(resolvePromise, POLL_INTERVAL_MS));
    alive = alive.filter(processIsAlive);
  }
  return alive;
}

/**
 * Rejects with the same evidence block as the other two waits rather than
 * returning false: a bare `expected false to be true` names neither the phase
 * nor the process state that produced it.
 */
async function waitUntil(supervisor, label, predicate, timeoutMs = PROCESS_TIMEOUT_MS) {
  const { diagnostics } = supervisor;
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() >= deadline) {
      throw new Error(
        `Timed out waiting for ${label} after ${timeoutMs}ms\n${diagnostics.evidence(label, timeoutMs)}`,
      );
    }
    await new Promise((resolvePromise) => setTimeout(resolvePromise, POLL_INTERVAL_MS));
  }
  diagnostics.mark(label);
  return true;
}

function watchForMarker(diagnostics, markerPath) {
  const timer = setInterval(() => {
    if (!existsSync(markerPath)) return;
    diagnostics.mark("marker-file-created");
    clearInterval(timer);
  }, POLL_INTERVAL_MS);
  timer.unref();
  return () => clearInterval(timer);
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

/**
 * Parent pid -> child pids for every process on the box. Mirrors the enumeration
 * half of run-parallel.mjs's own `collectTree` -- `/proc` on Linux, `ps`
 * elsewhere -- and is mirrored rather than imported for the same reason
 * `resolveRunnerForEnv` is: run-parallel is a shipped artifact of
 * @takazudo/zudo-doc with no module entry point to import. Split out from the
 * walk so a cleanup pass over several roots forks `ps` once, not once per root.
 */
function readProcessTable() {
  const childrenByParent = new Map();
  const record = (pid, ppid) => {
    if (!Number.isInteger(pid) || !Number.isInteger(ppid)) return;
    // A self-parenting row can only come from a mis-parsed line, but `walk`
    // below recurses, so letting one through would blow the stack rather than
    // report anything useful.
    if (pid === ppid) return;
    const siblings = childrenByParent.get(ppid);
    if (siblings) siblings.push(pid);
    else childrenByParent.set(ppid, [pid]);
  };

  try {
    if (process.platform === "linux") {
      for (const entry of readdirSync("/proc")) {
        if (!/^\d+$/.test(entry)) continue;
        let stat;
        try {
          stat = readFileSync(`/proc/${entry}/stat`, "utf8");
        } catch {
          continue; // the process exited between readdir and read
        }
        // The comm field is parenthesised and may itself contain spaces or
        // parentheses, so split after the LAST ')' rather than on whitespace.
        const tail = stat.slice(stat.lastIndexOf(")") + 2).split(" ");
        record(Number(entry), Number(tail[1]));
      }
    } else {
      const out = execFileSync("ps", ["-Ao", "pid=,ppid="], { encoding: "utf8" });
      for (const line of out.split("\n")) {
        const [pid, ppid] = line.trim().split(/\s+/);
        record(Number(pid), Number(ppid));
      }
    }
  } catch {
    // Enumeration failed; callers fall back to signalling just their roots.
  }

  return childrenByParent;
}

/** A pid and all of its descendants, parents before children. */
function treeFrom(childrenByParent, rootPid) {
  const ordered = [];
  const walk = (pid) => {
    ordered.push(pid);
    for (const child of childrenByParent.get(pid) ?? []) walk(child);
  };
  walk(rootPid);
  return ordered;
}

function collectTree(rootPid) {
  return treeFrom(readProcessTable(), rootPid);
}

/**
 * Signal every descendant of every root, deepest first, recording each pid seen.
 *
 * Deepest-first is the same ordering run-parallel.mjs uses, for the same reason:
 * an intermediate `pnpm run x` must not be left briefly holding a still-live
 * grandchild it does not forward signals to.
 *
 * This NEVER throws. It runs from `withSupervisor`'s `finally`, after the catch
 * that rethrows the real failure, so a throw here would replace the original
 * diagnostic the evidence blocks exist to preserve.
 */
function killTree(rootPids, signal, seen) {
  const table = readProcessTable();
  const ordered = [];
  const queued = new Set();
  for (const root of rootPids) {
    for (const pid of treeFrom(table, root)) {
      if (queued.has(pid)) continue;
      // init and this very process are never ours to kill; a mis-parsed row is
      // the only way either could reach this list, and the cost of being wrong
      // is killing the test runner. Filtered here rather than at signal time so
      // neither can be recorded and then re-used as a root on the next pass.
      if (!Number.isInteger(pid) || pid <= 1 || pid === process.pid) continue;
      queued.add(pid);
      ordered.push(pid);
    }
  }
  for (const pid of ordered.reverse()) {
    seen.add(pid);
    try {
      process.kill(pid, signal);
    } catch (error) {
      // ESRCH just means it already exited, the common case in a tree that is
      // collapsing anyway. Anything else is worth a line, but must never mask
      // the original failure by throwing here.
      if (error.code !== "ESRCH") {
        process.stderr.write(
          `docs-dev-supervisor cleanup: could not signal pid ${pid}: ${error.code ?? error.message}\n`,
        );
      }
    }
  }
}

/**
 * Kill the supervisor AND every descendant it still owns.
 *
 * `childPids` alone is not enough: it is only ever populated by parsing an `UP`
 * line, so a wait that times out BEFORE `UP` leaves it empty and the
 * `pnpm run <task>` -> `node -e` tree survives the test (#2894). Survivors keep
 * the inherited stdout/stderr pipe open, so the supervisor's `close` never fires
 * and the `waitForExit` below always paid its full second.
 *
 * Bounded re-collect rather than one pass: a mid-tier process can exit and
 * reparent its descendants between passes, which would hide them from a tree
 * walked only once. Pids already seen are re-used as roots so a reparented
 * descendant is still reachable after its ancestor is gone.
 */
async function terminateForCleanup(supervisor, childPids) {
  const seen = new Set();
  for (let pass = 0; pass < CLEANUP_TREE_PASSES; pass += 1) {
    const roots = [];
    // Only walk from the supervisor while it is still ours. Once Node has reaped
    // it the pid can be recycled, and walking a recycled pid would signal some
    // unrelated tree.
    if (
      supervisor.child.pid !== undefined &&
      supervisor.child.exitCode === null &&
      supervisor.child.signalCode === null
    ) {
      roots.push(supervisor.child.pid);
    }
    for (const pid of new Set([...childPids, ...seen])) {
      if (processIsAlive(pid)) roots.push(pid);
    }
    if (roots.length === 0) break;
    killTree(roots, "SIGKILL", seen);
    if (pass + 1 < CLEANUP_TREE_PASSES) {
      await new Promise((resolvePromise) => setTimeout(resolvePromise, POLL_INTERVAL_MS));
    }
  }
  try {
    await waitForExit(supervisor, 1_000);
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

/**
 * @param {object} [options]
 * @param {(error: unknown) => boolean} [options.isExpectedFailure] Narrow
 *   predicate identifying the ONE deliberately induced throw a caller
 *   declared in advance -- never a blanket "this case threw". Any throw that
 *   does not match (including no predicate at all) reports outcome=failed,
 *   which is the only value #2887's rule R-A should ever match.
 */
async function withSupervisor(scripts, run, { isExpectedFailure } = {}) {
  const fixture = createFixture();
  const supervisor = spawnSupervisor(fixture.directory, scripts);
  const stopMarkerWatch = watchForMarker(supervisor.diagnostics, fixture.markerPath);
  const childPids = supervisor.diagnostics.childPids;
  let outcome = "ok";
  try {
    await run(supervisor, childPids, fixture);
  } catch (error) {
    outcome = isExpectedFailure?.(error) ? "expected-failure" : "failed";
    throw error;
  } finally {
    stopMarkerWatch();
    // Opt-in so the phase distribution can be sampled across many runs (the
    // matched baseline-vs-load experiment of #2889) without adding noise to an
    // ordinary green run.
    if (process.env.ZFB_SUPERVISOR_TIMELINE) {
      process.stderr.write(`${supervisor.diagnostics.timelineLine(scripts.join("+"), outcome)}\n`);
    }
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
        const upLine = await supervisor.stdout.waitFor(
          (line) => line.startsWith("UP up "),
          "UP up <port> pid=<pid>",
        );
        childPids.push(pidFromUpLine(upLine));
        expect(portFromUpLine(upLine)).toBeGreaterThan(0);
        const failure = await waitForExit(supervisor);
        expect(failure.code).toBe(3);
        expect(failure.signal).toBeNull();
        expect(supervisor.stderr.all()).toContain('ERROR: "boom" exited with 3.');
        await waitUntil(supervisor, "sibling-death", () => !processIsAlive(childPids[0]));
      });
    });

    it("forwards a supervisor-only SIGINT to the whole process tree", async () => {
      await withSupervisor(["up", "up2"], async (supervisor, childPids) => {
        const upLine = await supervisor.stdout.waitFor(
          (line) => line.startsWith("UP up "),
          "UP up <port> pid=<pid>",
        );
        const up2Line = await supervisor.stdout.waitFor(
          (line) => line.startsWith("UP up2 "),
          "UP up2 <port> pid=<pid>",
        );
        childPids.push(pidFromUpLine(upLine), pidFromUpLine(up2Line));
        expect(portFromUpLine(upLine)).toBeGreaterThan(0);
        expect(portFromUpLine(up2Line)).toBeGreaterThan(0);

        expect(supervisor.child.kill("SIGINT")).toBe(true);
        const signalExit = await waitForExit(supervisor);
        expect(signalExit.code).toBe(130);
        expect(signalExit.signal).toBeNull();
        await waitUntil(supervisor, "sibling-death", () =>
          childPids.every((pid) => !processIsAlive(pid)),
        );
      });
    });

    // Regression for #2894: cleanup used to SIGKILL the supervisor plus whatever
    // pids the UP line had disclosed. A wait that times out BEFORE any UP line
    // disclosed nothing, so the `pnpm run <task>` -> `node -e` tree outlived the
    // test -- still holding the inherited pipe, so the supervisor's `close`
    // never fired either. The `hidden` task reproduces that exactly: it
    // publishes its pid only through a side-channel file and never prints UP.
    it("reaps the whole descendant tree when cleanup runs before any UP line", async () => {
      const observed = { childPidsAtFailure: null, leafPid: null, tree: null };
      let surfaced = null;

      try {
        await withSupervisor(
          ["hidden"],
          async (supervisor, childPids, fixture) => {
            // Wait for a *parseable* pid rather than for the file to exist: the
            // leaf's write is not atomic, and an empty read would make this test
            // flaky instead of failing on the thing it guards. `> 0` is not
            // decoration -- `Number("")` is 0, not NaN, so an integer check alone
            // accepts the created-but-not-yet-written file, and pid 0 would then
            // be handed to `process.kill`, which reads it as "this process group".
            const readLeafPid = () => {
              try {
                return Number(readFileSync(fixture.hiddenPidPath, "utf8").trim());
              } catch {
                return Number.NaN;
              }
            };
            const leafPidIsReadable = () => {
              const pid = readLeafPid();
              return Number.isInteger(pid) && pid > 0;
            };
            await waitUntil(supervisor, "hidden-pid-file", leafPidIsReadable);
            observed.leafPid = readLeafPid();
            observed.tree = collectTree(supervisor.child.pid);
            observed.childPidsAtFailure = [...childPids];
            // `hidden` never prints an UP line, so this wait is guaranteed to be
            // the pre-UP failure #2887 hit.
            await supervisor.stdout.waitFor(
              (line) => line.startsWith("UP hidden "),
              "UP hidden <port> pid=<pid>",
              PRE_UP_FAILURE_TIMEOUT_MS,
            );
          },
          { isExpectedFailure: isExpectedHiddenTimeout },
        );
      } catch (error) {
        surfaced = error;
      }

      // The ORIGINAL diagnostic has to be what surfaces. Cleanup runs in
      // withSupervisor's finally, so a throw there would silently replace it.
      expect(surfaced).toBeInstanceOf(Error);
      expect(surfaced.message).toContain("Timed out waiting for child output");
      expect(surfaced.message).toContain("UP hidden <port> pid=<pid>");
      expect(surfaced.message).toContain("--- supervisor diagnostics ---");

      // The leaf really was invisible to the UP-line channel, and really was a
      // descendant rather than the supervisor itself.
      expect(observed.childPidsAtFailure).toEqual([]);
      expect(Number.isInteger(observed.leafPid) && observed.leafPid > 0).toBe(true);
      expect(observed.tree).toContain(observed.leafPid);
      expect(observed.tree.length).toBeGreaterThan(1);

      const survivors = await waitForAllGone([...new Set([...observed.tree, observed.leafPid])]);
      expect(survivors).toEqual([]);
    });

    // The narrowness guarantee for #2904: isExpectedHiddenTimeout must match
    // ONLY the specific pre-UP timeout above, never "the hidden case threw"
    // in general. An unrelated throw from inside the same ["hidden"] case has
    // to keep reporting outcome=failed -- otherwise the fix here would just
    // move the false positive from "always failed" to "always
    // expected-failure", re-poisoning the exact field rule R-A reads.
    it("reports outcome=failed, not expected-failure, for an unrelated throw in the hidden case", async () => {
      const timelineLines = [];
      const originalWrite = process.stderr.write.bind(process.stderr);
      const originalEnv = process.env.ZFB_SUPERVISOR_TIMELINE;
      process.env.ZFB_SUPERVISOR_TIMELINE = "1";
      process.stderr.write = (chunk, ...rest) => {
        const text = chunk.toString();
        if (text.startsWith("[supervisor-timeline]")) timelineLines.push(text.trim());
        return originalWrite(chunk, ...rest);
      };

      try {
        await expect(
          withSupervisor(
            ["hidden"],
            async () => {
              throw new Error("unrelated assertion failure, not the pre-UP timeout");
            },
            { isExpectedFailure: isExpectedHiddenTimeout },
          ),
        ).rejects.toThrow("unrelated assertion failure, not the pre-UP timeout");
      } finally {
        process.stderr.write = originalWrite;
        if (originalEnv === undefined) delete process.env.ZFB_SUPERVISOR_TIMELINE;
        else process.env.ZFB_SUPERVISOR_TIMELINE = originalEnv;
      }

      expect(timelineLines).toHaveLength(1);
      expect(timelineLines[0]).toContain("case=hidden");
      expect(timelineLines[0]).toContain("outcome=failed");
      expect(timelineLines[0]).not.toContain("outcome=expected-failure");
    });
  });
});
