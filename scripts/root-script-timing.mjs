#!/usr/bin/env node

import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { Worker } from "node:worker_threads";

const DEFAULT_RUNS = 5;
const MAX_RUNS = 20;
const MAX_CPU_WORKERS = 4;
const RUNNER_COMMAND = ["pnpm", "exec", "vp", "test", "run", "--project", "scripts"];

let activeChild;
let terminationTimer;
let interruptedSignal;

export function quantile(values, probability) {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  // Match the supervisor timeline convention: round(p * (n - 1)), capped at n - 1.
  return sorted[Math.min(sorted.length - 1, Math.floor(probability * (sorted.length - 1) + 0.5))];
}

function summarizeDurations(values) {
  const sorted = [...values].sort((a, b) => a - b);
  return {
    n: sorted.length,
    minMs: sorted.length ? sorted[0] : null,
    p50Ms: quantile(sorted, 0.5),
    p90Ms: quantile(sorted, 0.9),
    p99Ms: quantile(sorted, 0.99),
    maxMs: sorted.length ? sorted.at(-1) : null,
  };
}

export function collectPerTestDurations(report, runId) {
  if (!Array.isArray(report?.testResults)) {
    throw new Error("Vitest JSON is missing the testResults array");
  }

  const samples = [];
  for (const suite of report.testResults) {
    const assertions = Array.isArray(suite.assertionResults) ? suite.assertionResults : [];
    const file = suite.name ?? suite.file ?? "<unknown-file>";
    for (const assertion of assertions) {
      const durationMs = assertion.duration;
      if (typeof durationMs !== "number" || !Number.isFinite(durationMs) || durationMs < 0)
        continue;
      const fullName =
        assertion.fullName ||
        [...(assertion.ancestorTitles ?? []), assertion.title].filter(Boolean).join(" ");
      samples.push({
        runId,
        file,
        test: fullName || "<unknown-test>",
        testId: `${file} :: ${fullName || "<unknown-test>"}`,
        status: assertion.status ?? "unknown",
        durationMs,
      });
    }
  }
  if (samples.length === 0) {
    throw new Error("Vitest JSON contains no assertionResults with numeric duration values");
  }
  return samples;
}

export function summarizeRuns(runs) {
  const parsedReports = [];
  const testSamples = [];
  const analysisErrors = [];
  const runSummaries = runs.map((run) => {
    const base = {
      id: run.id,
      exitCode: run.exitCode,
      signal: run.signal,
      rawJson: run.rawJson,
      provenance: run.provenance,
      runnerCounts: run.report
        ? {
            total: run.report.numTotalTests ?? null,
            passed: run.report.numPassedTests ?? null,
            failed: run.report.numFailedTests ?? null,
            pending: run.report.numPendingTests ?? null,
            todo: run.report.numTodoTests ?? null,
            totalSuites: run.report.numTotalTestSuites ?? null,
            failedSuites: run.report.numFailedTestSuites ?? null,
          }
        : null,
    };
    if (!run.report)
      return {
        ...base,
        sampleCount: 0,
        distribution: summarizeDurations([]),
        testStatusCounts: {},
      };

    parsedReports.push(run.report);
    let samples;
    try {
      samples = collectPerTestDurations(run.report, run.id);
    } catch (error) {
      analysisErrors.push({ runId: run.id, message: error.message });
      return {
        ...base,
        sampleCount: 0,
        distribution: summarizeDurations([]),
        testStatusCounts: {},
        analysisError: error.message,
      };
    }
    testSamples.push(...samples);
    const testStatusCounts = {};
    for (const sample of samples) {
      testStatusCounts[sample.status] = (testStatusCounts[sample.status] ?? 0) + 1;
    }
    return {
      ...base,
      sampleCount: samples.length,
      distribution: summarizeDurations(samples.map((sample) => sample.durationMs)),
      testStatusCounts,
    };
  });

  const byTest = new Map();
  for (const sample of testSamples) {
    const rows = byTest.get(sample.testId) ?? [];
    rows.push(sample);
    byTest.set(sample.testId, rows);
  }
  const perTest = [...byTest.entries()]
    .map(([testId, rows]) => {
      const durations = rows.map((row) => row.durationMs);
      return {
        testId,
        file: rows[0].file,
        test: rows[0].test,
        sampleCount: rows.length,
        failedSamples: rows.filter((row) => row.status === "failed").length,
        distribution: summarizeDurations(durations),
      };
    })
    .sort(
      (a, b) => b.distribution.maxMs - a.distribution.maxMs || a.testId.localeCompare(b.testId),
    );

  return {
    runCount: runs.length,
    reportCount: parsedReports.length,
    analysisErrors,
    testSampleCount: testSamples.length,
    distributionAcrossPerTestObservations: summarizeDurations(
      testSamples.map((sample) => sample.durationMs),
    ),
    runs: runSummaries,
    perTest,
    slowestTestsByObservedMax: perTest.slice(0, 20),
  };
}

export async function withOwnedWorkers(count, createWorker, action) {
  const workers = [];
  let actionError;
  let result;
  let cleanupErrors;
  try {
    for (let index = 0; index < count; index += 1) {
      workers.push(await createWorker(index));
    }
    result = await action();
  } catch (error) {
    actionError = error;
  } finally {
    const outcomes = await Promise.allSettled(
      workers.map(async ({ worker, stopBuffer }) => {
        if (stopBuffer) {
          const stop = new Int32Array(stopBuffer);
          Atomics.store(stop, 0, 1);
          Atomics.notify(stop, 0);
        }
        await worker.terminate();
      }),
    );
    cleanupErrors = outcomes
      .filter((result) => result.status === "rejected")
      .map((result) => result.reason);
  }
  if (cleanupErrors.length > 0) {
    throw new AggregateError(
      actionError ? [actionError, ...cleanupErrors] : cleanupErrors,
      "Failed to clean up one or more root script timing workers",
    );
  }
  if (actionError) throw actionError;
  return result;
}

function createCpuWorkerSource() {
  return `
    const { parentPort, workerData } = require("node:worker_threads");
    const stop = new Int32Array(workerData.stopBuffer);
    let state = workerData.seed >>> 0;
    while (Atomics.load(stop, 0) === 0) {
      for (let index = 0; index < 10000; index += 1) {
        state ^= state << 13;
        state ^= state >>> 17;
        state ^= state << 5;
      }
    }
    parentPort.postMessage(state >>> 0);
  `;
}

async function createCpuWorker(index) {
  const stopBuffer = new SharedArrayBuffer(Int32Array.BYTES_PER_ELEMENT);
  const worker = new Worker(createCpuWorkerSource(), {
    eval: true,
    workerData: { stopBuffer, seed: index + 1 },
  });
  try {
    await once(worker, "online");
    return { worker, stopBuffer };
  } catch (error) {
    await worker.terminate().catch(() => {});
    throw error;
  }
}

function runMetadataCommand(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8" });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} failed (${result.status}): ${result.stderr}`);
  }
  return result.stdout.trim();
}

function getGitProvenance() {
  const runGit = (args, encoding = "utf8") => {
    const result = spawnSync("git", args, { encoding });
    if (result.error || result.status !== 0) {
      throw result.error ?? new Error(`git ${args.join(" ")} failed (${result.status})`);
    }
    return result.stdout;
  };
  const status = runGit(["status", "--porcelain", "--untracked-files=all"]).trimEnd();
  const diff = runGit(["diff", "--binary", "HEAD"], null);
  return {
    commit: runGit(["rev-parse", "HEAD"]).trim(),
    branch: runGit(["branch", "--show-current"]).trim(),
    dirty: status.length > 0,
    statusLines: status ? status.split("\n") : [],
    trackedDiffSha256: createHash("sha256").update(diff).digest("hex"),
  };
}

function captureHostProvenance(profile, cpuWorkers) {
  const cpus = os.cpus();
  const availableParallelism = os.availableParallelism?.() ?? cpus.length;
  return {
    capturedAt: new Date().toISOString(),
    platform: os.platform(),
    osType: os.type(),
    osRelease: os.release(),
    architecture: os.arch(),
    cpu: {
      model: cpus[0]?.model ?? "unknown",
      logicalCpuCount: cpus.length,
      availableParallelism,
      frequencyMhz: cpus[0]?.speed ?? null,
    },
    memoryBytes: { total: os.totalmem(), freeAtStart: os.freemem() },
    loadAverage: os.loadavg(),
    load:
      profile === "cpu"
        ? {
            kind: "bounded-node-worker-threads",
            workers: cpuWorkers,
            maximumAllowedWorkers: Math.min(MAX_CPU_WORKERS, Math.max(1, availableParallelism)),
            workerBehavior:
              "fixed-size integer loop; no files, child processes, or expanding allocation",
          }
        : { kind: "idle", workers: 0 },
    ci: {
      githubActions: process.env.GITHUB_ACTIONS === "true",
      runnerOs: process.env.RUNNER_OS ?? null,
      runnerName: process.env.RUNNER_NAME ?? null,
      imageOs: process.env.ImageOS ?? null,
      imageVersion: process.env.ImageVersion ?? null,
      workflow: process.env.GITHUB_WORKFLOW ?? null,
      job: process.env.GITHUB_JOB ?? null,
      runId: process.env.GITHUB_RUN_ID ?? null,
      runAttempt: process.env.GITHUB_RUN_ATTEMPT ?? null,
      ref: process.env.GITHUB_REF ?? null,
      sha: process.env.GITHUB_SHA ?? null,
    },
  };
}

function captureToolchain() {
  const pnpmVersion = runMetadataCommand("pnpm", ["--version"]);
  const vitePlusOutput = runMetadataCommand("pnpm", ["exec", "vp", "--version"]);
  const vitePlusVersion = vitePlusOutput.match(/^vp\s+v?([^\s]+)/m)?.[1] ?? null;
  const viteVersion = vitePlusOutput.match(/^\s+vite\s+v?([^\s]+)/m)?.[1] ?? null;
  const vitestVersion = vitePlusOutput.match(/^\s+vitest\s+v?([^\s]+)/m)?.[1] ?? null;
  return {
    node: process.version,
    pnpm: pnpmVersion,
    vitePlus: vitePlusVersion,
    vite: viteVersion,
    vitest: vitestVersion,
    vitePlusVersionOutput: vitePlusOutput,
  };
}

function sendSignalToOwnedChild(child, signal) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  try {
    if (process.platform !== "win32" && child.pid) process.kill(-child.pid, signal);
    else child.kill(signal);
  } catch (error) {
    if (error.code !== "ESRCH") throw error;
  }
}

function handleTerminationSignal(signal) {
  interruptedSignal ??= signal;
  if (!activeChild) return;
  sendSignalToOwnedChild(activeChild, "SIGTERM");
  clearTimeout(terminationTimer);
  terminationTimer = setTimeout(() => sendSignalToOwnedChild(activeChild, "SIGKILL"), 5000);
  terminationTimer.unref?.();
}

function runTestCommand(cwd, jsonPath) {
  if (interruptedSignal) return Promise.reject(new Error(`Interrupted by ${interruptedSignal}`));
  const args = [...RUNNER_COMMAND.slice(1), "--reporter=json", `--outputFile=${jsonPath}`];
  return new Promise((resolve, reject) => {
    const child = spawn(RUNNER_COMMAND[0], args, {
      cwd,
      detached: process.platform !== "win32",
      stdio: "inherit",
    });
    activeChild = child;
    child.once("error", (error) => {
      if (activeChild === child) activeChild = undefined;
      clearTimeout(terminationTimer);
      reject(error);
    });
    child.once("close", (code, signal) => {
      if (activeChild === child) activeChild = undefined;
      clearTimeout(terminationTimer);
      resolve({ code, signal, command: [RUNNER_COMMAND[0], ...args] });
    });
    if (interruptedSignal) handleTerminationSignal(interruptedSignal);
  });
}

function timestampId(profile) {
  return `${profile}-${new Date().toISOString().replaceAll(":", "-").replaceAll(".", "-")}`;
}

function parseArgs(argv) {
  const options = { profile: null, runs: DEFAULT_RUNS, outputDir: null, cpuWorkers: null };
  for (let index = 0; index < argv.length; index += 1) {
    const key = argv[index];
    if (key === "--help" || key === "-h") return { help: true };
    const value = argv[index + 1];
    if (!value || value.startsWith("--")) throw new Error(`Missing value for ${key}`);
    if (key === "--profile") options.profile = value;
    else if (key === "--runs") options.runs = Number(value);
    else if (key === "--output-dir") options.outputDir = value;
    else if (key === "--cpu-workers") options.cpuWorkers = Number(value);
    else throw new Error(`Unknown option: ${key}`);
    index += 1;
  }

  if (!new Set(["idle", "cpu"]).has(options.profile))
    throw new Error("--profile must be idle or cpu");
  if (!Number.isInteger(options.runs) || options.runs < 1 || options.runs > MAX_RUNS) {
    throw new Error(`--runs must be an integer from 1 to ${MAX_RUNS}`);
  }
  if (!options.outputDir)
    throw new Error("--output-dir is required; keep raw artifacts outside the source tree");
  const availableParallelism = os.availableParallelism?.() ?? os.cpus().length;
  const maxCpuWorkers = Math.min(MAX_CPU_WORKERS, Math.max(1, availableParallelism));
  if (options.profile === "idle" && options.cpuWorkers !== null) {
    throw new Error("--cpu-workers is only valid with --profile cpu");
  }
  if (options.profile === "cpu") {
    options.cpuWorkers ??= Math.min(
      MAX_CPU_WORKERS,
      Math.max(1, Math.floor(availableParallelism / 2)),
    );
    if (
      !Number.isInteger(options.cpuWorkers) ||
      options.cpuWorkers < 1 ||
      options.cpuWorkers > maxCpuWorkers
    ) {
      throw new Error(`--cpu-workers must be an integer from 1 to ${maxCpuWorkers} on this host`);
    }
  }
  return options;
}

function renderMarkdownSummary(summary) {
  const distribution = summary.timing.distributionAcrossPerTestObservations;
  const number = (value) => (value === null ? "—" : `${value.toFixed(2)} ms`);
  const lines = [
    `# Root scripts timing (${summary.profile})`,
    "",
    `Batch: \`${summary.batchId}\``,
    `Source: \`${summary.source.commit}\` (${summary.source.branch || "detached"})${summary.source.dirty ? "; dirty tree" : "; clean tree"}`,
    `Host: ${summary.host.platform} ${summary.host.osRelease} ${summary.host.architecture}; ${summary.host.cpu.availableParallelism} available / ${summary.host.cpu.logicalCpuCount} logical CPUs; ${(summary.host.memoryBytes.total / 1024 ** 3).toFixed(1)} GiB RAM`,
    `Toolchain: Node ${summary.toolchain.node}, pnpm ${summary.toolchain.pnpm}, Vite+ ${summary.toolchain.vitePlus}, Vite ${summary.toolchain.vite}, Vitest ${summary.toolchain.vitest}`,
    `Load: ${summary.host.load.kind}${summary.profile === "cpu" ? `, ${summary.host.load.workers} worker threads` : ""}`,
    "",
    "## Per-test observation distribution",
    "",
    `Across ${distribution.n} individual test observations from ${summary.timing.runCount} run(s): min ${number(distribution.minMs)}, p50 ${number(distribution.p50Ms)}, p90 ${number(distribution.p90Ms)}, p99 ${number(distribution.p99Ms)}, max ${number(distribution.maxMs)}. Quantiles select sorted index round(p * (n - 1)), capped at n - 1, matching the repository's locked timeline convention.`,
    "",
    "## Runs",
    "",
    "| Run | Exit | Timed tests | Passed / failed / pending / todo | p50 | p90 | p99 | Max | Raw JSON | Provenance |",
    "| --- | ---: | ---: | --- | ---: | ---: | ---: | ---: | --- | --- |",
    ...summary.timing.runs.map(
      (run) =>
        `| ${run.id} | ${run.exitCode ?? "error"} | ${run.sampleCount} | ${run.runnerCounts ? `${run.runnerCounts.passed ?? "?"} / ${run.runnerCounts.failed ?? "?"} / ${run.runnerCounts.pending ?? "?"} / ${run.runnerCounts.todo ?? "?"}` : "—"} | ${number(run.distribution.p50Ms)} | ${number(run.distribution.p90Ms)} | ${number(run.distribution.p99Ms)} | ${number(run.distribution.maxMs)} | \`${run.rawJson}\` | \`${run.provenance}\` |`,
    ),
    "",
    "## Slowest tests by observed maximum",
    "",
    "| Test | Samples | Failed | p50 | p90 | p99 | Max |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ...summary.timing.slowestTestsByObservedMax.map(
      (test) =>
        `| \`${test.testId}\` | ${test.sampleCount} | ${test.failedSamples} | ${number(test.distribution.p50Ms)} | ${number(test.distribution.p90Ms)} | ${number(test.distribution.p99Ms)} | ${number(test.distribution.maxMs)} |`,
    ),
    "",
    "This summary describes only this host and profile. It does not justify a timeout change without representative platform runs and healthy-host evidence.",
    "",
  ];
  return lines.join("\n");
}

function helpText() {
  return `Usage: node scripts/root-script-timing.mjs --profile <idle|cpu> --output-dir <path> [--runs 5] [--cpu-workers N]

Runs the root Vitest scripts project sequentially with JSON per-test timings.
CPU mode starts at most four fixed-memory Node worker threads and always stops
them in finally, including when the runner fails. Each invocation creates a
unique batch directory under --output-dir with raw JSON, provenance, and summary.
CPU mode defaults to half of available parallelism, capped at four workers;
--cpu-workers can select a lower count.

Examples:
  node scripts/root-script-timing.mjs --profile idle --runs 5 --output-dir "$RUNNER_TEMP/root-script-timing"
  node scripts/root-script-timing.mjs --profile cpu --runs 5 --output-dir "$RUNNER_TEMP/root-script-timing"
`;
}

async function runBatch(options) {
  const repoRoot = runMetadataCommand("git", ["rev-parse", "--show-toplevel"]);
  process.chdir(repoRoot);
  const source = getGitProvenance();
  const toolchain = captureToolchain();
  const batchId = timestampId(options.profile);
  const outputRoot = path.resolve(options.outputDir);
  const batchDir = path.resolve(options.outputDir, batchId);
  await mkdir(outputRoot, { recursive: true });
  await mkdir(batchDir, { recursive: false });
  const runs = [];
  const createWorker = options.profile === "cpu" ? createCpuWorker : undefined;

  for (let index = 1; index <= options.runs; index += 1) {
    if (interruptedSignal) break;
    const runId = `${options.profile}-${String(index).padStart(2, "0")}`;
    const jsonFile = `${runId}.vitest.json`;
    const rawJson = path.join(batchDir, jsonFile);
    const provenance = `${runId}.provenance.json`;
    const host = captureHostProvenance(options.profile, options.cpuWorkers ?? 0);
    const runStartedAt = new Date().toISOString();
    let result;
    let runError;
    try {
      result = await withOwnedWorkers(
        options.profile === "cpu" ? options.cpuWorkers : 0,
        createWorker,
        () => runTestCommand(repoRoot, rawJson),
      );
    } catch (error) {
      runError = error;
    }

    let report;
    let reportError;
    try {
      report = JSON.parse(await readFile(rawJson, "utf8"));
    } catch (error) {
      reportError = String(error.message ?? error);
      report = undefined;
    }
    const completedAt = new Date().toISOString();
    const provenanceData = {
      schemaVersion: 1,
      batchId,
      runId,
      profile: options.profile,
      runStartedAt,
      completedAt,
      elapsedWallMs: Date.parse(completedAt) - Date.parse(runStartedAt),
      command: result?.command ?? [
        RUNNER_COMMAND[0],
        ...RUNNER_COMMAND.slice(1),
        "--reporter=json",
        `--outputFile=${rawJson}`,
      ],
      exitCode: result?.code ?? null,
      signal: result?.signal ?? null,
      error:
        [runError ? String(runError.stack ?? runError) : null, reportError]
          .filter(Boolean)
          .join("\n") || null,
      host,
      source,
      toolchain,
      rawJson: jsonFile,
      rawJsonSha256: report
        ? createHash("sha256")
            .update(await readFile(rawJson))
            .digest("hex")
        : null,
    };
    await writeFile(
      path.join(batchDir, provenance),
      `${JSON.stringify(provenanceData, null, 2)}\n`,
    );
    runs.push({
      id: runId,
      exitCode: result?.code ?? (runError ? 1 : null),
      signal: result?.signal ?? null,
      rawJson: jsonFile,
      provenance,
      report,
    });
    if (runError || result?.code !== 0 || result?.signal || interruptedSignal) break;
  }

  const timing = summarizeRuns(runs);
  const summary = {
    schemaVersion: 1,
    batchId,
    profile: options.profile,
    requestedRuns: options.runs,
    generatedAt: new Date().toISOString(),
    source,
    toolchain,
    host: captureHostProvenance(options.profile, options.cpuWorkers ?? 0),
    timing,
    interruption: interruptedSignal ?? null,
  };
  await writeFile(path.join(batchDir, "summary.json"), `${JSON.stringify(summary, null, 2)}\n`);
  await writeFile(path.join(batchDir, "summary.md"), renderMarkdownSummary(summary));
  console.log(`Root scripts timing artifacts: ${batchDir}`);
  console.log(`Summary: ${path.join(batchDir, "summary.md")}`);
  if (
    interruptedSignal ||
    runs.some((run) => run.exitCode !== 0 || !run.report) ||
    timing.analysisErrors.length > 0
  )
    process.exitCode = 1;
}

async function main(argv = process.argv.slice(2)) {
  let options;
  try {
    options = parseArgs(argv);
    if (options.help) {
      console.log(helpText());
      return;
    }
  } catch (error) {
    console.error(error.message);
    console.error(helpText());
    process.exitCode = 2;
    return;
  }

  process.on("SIGINT", handleTerminationSignal);
  process.on("SIGTERM", handleTerminationSignal);
  try {
    await runBatch(options);
  } finally {
    process.off("SIGINT", handleTerminationSignal);
    process.off("SIGTERM", handleTerminationSignal);
    clearTimeout(terminationTimer);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await main();
}
