#!/usr/bin/env node
import { execFile } from "node:child_process";
import { appendFile, mkdir, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";

import { runCli as runHarvester } from "./harvest-supervisor-timelines.mjs";
import { IDENTITY_FIELDS, runCli as runSummarizer } from "./supervisor-timeline-summary.mjs";

const execFileAsync = promisify(execFile);

/**
 * The scheduled supervisor watch's two-pass orchestrator (issue #2918,
 * epic #2915).
 *
 * ONE HARVEST, TWO PASSES, because one population cannot answer both
 * questions. Pass A covers every branch, catching failed records and a
 * pre-UP budget that has become too tight. This population is deliberately
 * heterogeneous, so all identity fields are allow-listed for drift. Pass B
 * selects the trunk push runs from pass A's manifest and re-reads their saved
 * job logs with strict identity checking. It is not a second harvest: using
 * one enumeration avoids judging a run that finished between two harvests in
 * only one population, and halves the GitHub traffic.
 *
 * The harvester and summarizer are called separately, never piped together.
 * A pipeline would let the rightmost summarizer status hide a harvester
 * failure (for example, harvester 64 becoming summarizer 1 and looking like a
 * quiet week). Their independent statuses are therefore part of the verdict.
 *
 * NO-DATA IS NOT RED, BUT VANISHED TELEMETRY IS. Harvester 4 means nothing
 * was harvestable and is a quiet week; harvester 1 means harvested green
 * health jobs carried no records and is red. Pass B is stricter: one silent
 * green trunk job beside emitting jobs is also red, because the summarizer's
 * no-records result only fires when its whole input is silent.
 *
 * Exit codes are informational (the workflow keys on the verdict output):
 *   0 green, 1 no-data, 2 red.
 */

function parseOptions(argv, env) {
  const options = {
    outDir: env.WATCH_OUT_DIR || "./supervisor-watch-out",
    since: env.WATCH_SINCE || undefined,
    gh: env.WATCH_GH || undefined,
    mainBranch: env.WATCH_MAIN_BRANCH || "main",
  };
  const flags = {
    "--out-dir": "outDir",
    "--since": "since",
    "--gh": "gh",
    "--main-branch": "mainBranch",
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const key = flags[arg];
    if (!key) throw new Error(`unknown flag: ${arg}`);
    const value = argv[index + 1];
    if (value === undefined || value === "" || value.startsWith("--")) {
      throw new Error(`${arg} requires a value`);
    }
    options[key] = value;
    index += 1;
  }
  return options;
}

function stringSink(onWrite) {
  return {
    write(chunk) {
      onWrite(typeof chunk === "string" ? chunk : chunk.toString("utf8"));
      return true;
    },
  };
}

async function withHarvesterEnvironment(env, operation) {
  const key = "HARVEST_RETRY_DELAY_MS";
  const hadValue = Object.hasOwn(process.env, key);
  const previous = process.env[key];
  const raw = env.WATCH_RETRY_DELAY_MS;
  try {
    if (raw !== undefined && raw !== "") process.env[key] = raw;
    else delete process.env[key];
    return await operation();
  } finally {
    if (hadValue) process.env[key] = previous;
    else delete process.env[key];
  }
}

async function runTar(tar, args) {
  if (typeof tar === "function") {
    await tar(args);
    return;
  }
  await execFileAsync(tar || "tar", args);
}

function matchingLines(text, predicate) {
  return text
    .split(/\r?\n/)
    .filter((line) => line !== "" && predicate(line))
    .join("\n");
}

function withFinalNewline(text) {
  return text === "" ? "" : `${text}\n`;
}

export function classifyPassA(hrc, src) {
  if (hrc === 0 && src === 0) return "ok";
  if (hrc === 4) return "empty";
  return "red";
}

export function classifyPassB({ runCount, silentCount, src }) {
  if (runCount === 0) return "empty";
  if (silentCount > 0) return "silent";
  return src === 0 ? "ok" : "red";
}

export function deriveVerdict(statusA, statusB) {
  if (statusA === "red" || statusB === "red" || statusB === "silent") {
    return { verdict: "red", exitCode: 2 };
  }
  if (statusA === "empty") return { verdict: "no-data", exitCode: 1 };
  return { verdict: "green", exitCode: 0 };
}

function fileOrNone(text) {
  return text === "" ? "none\n" : text;
}

function buildStepSummary({
  verdict,
  detail,
  mainBranch,
  mainCount,
  failedRuns,
  silentRuns,
  harvestErrors,
  harvestNotices,
  manifest,
  allSummary,
  mainSummary,
}) {
  const finalManifestLine = manifest.trimEnd().split(/\r?\n/).at(-1) ?? "";
  return [
    `## Supervisor watch — ${verdict}\n\n`,
    `\`verdict=${verdict} ${detail}\`\n\n`,
    "### Runs with failed supervisor records (R-A: read the saved job log)\n\n",
    "```\n",
    fileOrNone(failedRuns),
    "```\n\n",
    `### Green health jobs with zero records (silent emitter; red when on ${mainBranch})\n\n`,
    "```\n",
    fileOrNone(silentRuns),
    "```\n\n",
    "### Runs the harvester could not fetch or parse\n\n",
    "```\n",
    fileOrNone(harvestErrors),
    "```\n\n",
    "### Harvest window and notices\n\n",
    "```\n",
    fileOrNone(harvestNotices),
    `${finalManifestLine}\n`,
    "```\n\n",
    "### Summary — all branches (identity drift allow-listed)\n\n",
    "```\n",
    allSummary,
    "```\n\n",
    `### Summary — ${mainBranch} only (strict identity, ${mainCount} run(s))\n\n`,
    "```\n",
    mainSummary,
    "```\n",
  ].join("");
}

/** Runs the watch end to end and returns its exit code without exiting. */
export async function runCli(
  argv,
  { env = process.env, stdout = process.stdout, stderr = process.stderr, now, tar } = {},
) {
  let options;
  try {
    options = parseOptions(argv, env);
  } catch (error) {
    stderr.write(`usage error: ${error.message}\n`);
    return 64;
  }

  const out = options.outDir;
  await Promise.all([
    mkdir(join(out, "all"), { recursive: true }),
    mkdir(join(out, "main"), { recursive: true }),
    mkdir(join(out, "job-logs"), { recursive: true }),
  ]);

  const harvestArgs = [];
  if (options.since) harvestArgs.push("--since", options.since);
  if (options.gh) harvestArgs.push("--gh", options.gh);
  harvestArgs.push("--save-dir", join(out, "job-logs"));

  stderr.write("==> pass A (all branches): harvesting\n");
  let timelines = "";
  let manifest = "";
  // Forward WATCH_RETRY_DELAY_MS as an unparsed string into the harvester's
  // environment. Its own resolver remains the one place that interprets the
  // value, so an invalid value retains the 2000 ms fallback instead of being
  // silently coerced here. Restore the process environment after the awaited
  // harvest so an injected env never leaks into another in-process call.
  const hrcA = await withHarvesterEnvironment(env, () =>
    runHarvester(harvestArgs, {
      stdout: stringSink((chunk) => {
        timelines += chunk;
      }),
      stderr: stringSink((chunk) => {
        manifest += chunk;
      }),
      now,
    }),
  );
  // The manifest is emitted on stderr, potentially in arbitrarily-sized
  // chunks. Parse only after the awaited harvester has completely settled.
  await Promise.all([
    writeFile(join(out, "all", "timelines.txt"), timelines),
    writeFile(join(out, "all", "manifest.txt"), manifest),
  ]);

  stderr.write("==> pass A (all branches): summarizing\n");
  let allSummary = "";
  const allSummarySink = stringSink((chunk) => {
    allSummary += chunk;
  });
  const srcA = await runSummarizer(["--strict", "--allow-drift", IDENTITY_FIELDS.join(",")], {
    stdin: timelines,
    stdout: allSummarySink,
    stderr: allSummarySink,
  });
  await writeFile(join(out, "all", "summary.txt"), allSummary);
  const statusA = classifyPassA(hrcA, srcA);
  stderr.write(`==> pass A (all branches): status=${statusA} hrc=${hrcA} src=${srcA}\n`);

  // Keep the anchored structural check as a regular expression, but select
  // event and branch tokens with fixed-string includes. In particular,
  // branch feat/a.b must never match feat/axb as a constructed RegExp would.
  const harvestedLine = /^run=[0-9]+ .* job=[0-9]+ lines=[0-9]+ /;
  const mainLines = manifest
    .split(/\r?\n/)
    .filter(
      (line) =>
        harvestedLine.test(line) &&
        line.includes(" event=push ") &&
        line.includes(` branch=${options.mainBranch} `),
    );
  const runsText = withFinalNewline(mainLines.join("\n"));
  await writeFile(join(out, "main", "runs.txt"), runsText);

  const mainLogs = [];
  let mainSilent = 0;
  for (const line of mainLines) {
    const runId = line.match(/^run=([0-9]+) /)?.[1];
    const jobId = line.match(/ job=([0-9]+) /)?.[1];
    mainLogs.push(join(out, "job-logs", `run-${runId}-job-${jobId}.log`));
    if (line.includes(" lines=0 ")) mainSilent += 1;
  }

  const mainCount = mainLogs.length;
  let srcB = "-";
  let mainSummary;
  if (mainCount === 0) {
    mainSummary = `no ${options.mainBranch} runs were harvested in this window (see all/manifest.txt)\n`;
  } else {
    stderr.write(
      `==> pass B (${options.mainBranch} only): summarizing ${mainCount} saved job log(s)\n`,
    );
    mainSummary = "";
    const mainSummarySink = stringSink((chunk) => {
      mainSummary += chunk;
    });
    srcB = await runSummarizer(["--strict", ...mainLogs], {
      stdout: mainSummarySink,
      stderr: mainSummarySink,
    });
  }
  await writeFile(join(out, "main", "summary.txt"), mainSummary);
  const statusB = classifyPassB({ runCount: mainCount, silentCount: mainSilent, src: srcB });
  stderr.write(
    `==> pass B (${options.mainBranch} only): status=${statusB} runs=${mainCount} silent=${mainSilent} src=${srcB}\n`,
  );

  // These files are provenance for triage. Their input order is the manifest
  // order, and the final runs= line is intentionally excluded from per-run
  // failure matching.
  const failedRuns = withFinalNewline(
    matchingLines(manifest, (line) => /^run=[0-9]+ .* failedRecords=[1-9]/.test(line)),
  );
  const harvestErrors = withFinalNewline(
    matchingLines(manifest, (line) => /^run=[0-9]+ .* error=/.test(line)),
  );
  const silentRuns = withFinalNewline(
    matchingLines(manifest, (line) => /^run=[0-9]+ .* lines=0 /.test(line)),
  );
  const harvestNotices = withFinalNewline(
    matchingLines(manifest, (line) => /^(window|notice|warning):/.test(line)),
  );
  await Promise.all([
    writeFile(join(out, "failed-runs.txt"), failedRuns),
    writeFile(join(out, "harvest-errors.txt"), harvestErrors),
    writeFile(join(out, "silent-runs.txt"), silentRuns),
    writeFile(join(out, "harvest-notices.txt"), harvestNotices),
  ]);

  const { verdict, exitCode } = deriveVerdict(statusA, statusB);
  const detail = `all=${statusA}:${hrcA}/${srcA} main=${statusB}:${mainCount}/${srcB}`;

  // The step summary precedes archiving so a broken archive cannot erase the
  // R-A triage pointers or the rendered diagnostics. Its append is awaited:
  // source position alone is not sequencing in async JavaScript.
  if (env.GITHUB_STEP_SUMMARY) {
    await appendFile(
      env.GITHUB_STEP_SUMMARY,
      buildStepSummary({
        verdict,
        detail,
        mainBranch: options.mainBranch,
        mainCount,
        failedRuns,
        silentRuns,
        harvestErrors,
        harvestNotices,
        manifest,
        allSummary,
        mainSummary,
      }),
    );
  }

  // Both passes are finished with job-logs/. Fold it into one artifact, but
  // delete the source directory only after archive creation AND verification.
  // On either failure remove the partial archive, keep the logs, and withhold
  // both GITHUB_OUTPUT and the final stdout verdict so the workflow's existing
  // "verdict was never written" alert fires.
  const archive = join(out, "job-logs.tar.gz");
  const tarRunner = tar ?? env.WATCH_TAR ?? "tar";
  stderr.write(`==> archiving ${join(out, "job-logs")}\n`);
  try {
    await runTar(tarRunner, ["-czf", archive, "-C", out, "job-logs"]);
    await runTar(tarRunner, ["-tzf", archive]);
  } catch {
    await rm(archive, { force: true });
    stderr.write(
      `!! archiving ${join(out, "job-logs")} failed; kept the plain directory, wrote no verdict\n`,
    );
    return 1;
  }
  await rm(join(out, "job-logs"), { recursive: true });
  stderr.write(`==> archived job logs to ${archive}, removed the plain directory\n`);

  if (env.GITHUB_OUTPUT) {
    await appendFile(env.GITHUB_OUTPUT, `verdict=${verdict}\ndetail=${detail}\n`);
  }
  stdout.write(`verdict=${verdict} ${detail}\n`);
  return exitCode;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = await runCli(process.argv.slice(2));
}
