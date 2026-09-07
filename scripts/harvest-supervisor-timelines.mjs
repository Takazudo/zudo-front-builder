#!/usr/bin/env node
import { execFile } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";

import { parseTimelines } from "./supervisor-timeline-summary.mjs";

/**
 * Re-harvests the ubuntu `[supervisor-timeline]` population that
 * `health.yml` has been sampling since #2902 / PR #2906
 * (`ZFB_SUPERVISOR_TIMELINE: "1"` on its `pnpm test:workspace` step). Today
 * those lines only exist scattered inside individual CI job logs; this
 * script re-fetches them on demand via `gh` and re-parses them with the
 * summarizer's own `parseTimelines`, so the population is re-aggregatable
 * in one command:
 *
 *   node scripts/harvest-supervisor-timelines.mjs \
 *     | node scripts/supervisor-timeline-summary.mjs --strict --allow-drift env
 *
 * This adds aggregation, not storage: nothing is persisted between runs
 * (see `--save-dir` below for an opt-in exception used for debugging).
 *
 * Per run, only the `health` job's log lines are read, never the whole
 * run's. On 2026-09-07 the whole-run `gh run view <id> --log` for a real run
 * returned 18,615 lines with zero vitest output (the `health` job's log was
 * missing from the combined stream), while `gh run view --job <jobId> --log`
 * for that same run's `health` job returned 10,014 lines including the
 * records. Fetching the whole run is not merely wasteful here, it silently
 * loses the data. (On the wire gh still downloads the run's log archive
 * for `--job` and keeps it in its own cache directory; the per-job form is
 * about which lines come back, not about bytes transferred.)
 *
 * Runs are harvested with bounded concurrency (`CONCURRENCY` below): each
 * run costs two `gh` invocations of roughly 1.5-3.5 s (measured
 * 2026-09-07), so a strictly sequential 200-run default harvest would take
 * 10-17 minutes.
 *
 * Output contract
 * ----------------
 * stdout carries only the original `[supervisor-timeline]` line text (one
 * per record, in the order runs were enumerated) — exactly what
 * `parseTimelines` accepted, so it round-trips through the summarizer
 * unchanged. All output is buffered and written in a single stdout write
 * at the very end, so a crash mid-harvest can never leave a truncated,
 * plausible-looking dataset on stdout. A *partial* harvest (exit 3, below)
 * does write the records from the runs that succeeded — the exit code is
 * what says the population is incomplete.
 *
 * stderr carries a per-run manifest line (emitted as each run completes,
 * so not necessarily in enumeration order) plus one final summary line:
 *
 *   run=<id> attempt=<n> event=<e> branch=<b> sha=<sha8> created=<iso> conclusion=<c> job=<jobId|none> lines=<n>
 *   run=<id> attempt=<n> event=<e> branch=<b> sha=<sha8> created=<iso> conclusion=<c> job=<jobId|none> skipped=<reason>
 *   run=<id> attempt=<n> event=<e> branch=<b> sha=<sha8> created=<iso> conclusion=<c> job=<jobId|none> error=<reason>
 *   runs=<enumerated> harvested=<k> failed=<f> records=<r>
 *
 * Skipped (counts toward neither `harvested` nor `failed`):
 *   - a run whose `status` is not yet `completed` (no complete log to fetch);
 *   - a completed run with no `health` job at all;
 *   - a `health` job that never started (cancelled while still queued by
 *     `cancel-in-progress`, or skipped) — GitHub has no log for it and
 *     `gh run view --job <id> --log` fails with `log not found`, which must
 *     not read as a partial harvest. A job that started and was then
 *     cancelled does have a (partial) log and is harvested normally; its
 *     run-level `conclusion=cancelled` stays visible in the manifest.
 *
 * When `gh run list` returns exactly `--limit` runs the window is capped,
 * not complete: a `notice:` line says so, because the manifest's
 * `runs=<n>` alone cannot distinguish "all runs since --since" from "the
 * newest --limit of them".
 *
 * Exit codes (mirrors the summarizer's contract style: distinct codes for
 * distinct situations, never conflating "nothing went wrong" with "there is
 * no data"):
 *
 *   0  every attempted run succeeded and at least one record was extracted
 *   1  no run failed, but zero records were extracted
 *   3  at least one run failed (its log could not be fetched, saved, or
 *      parsed as `[supervisor-timeline]` records) — a partial harvest,
 *      distinct from 0/1 regardless of how many records the other runs
 *      still yielded. A failed run contributes no records at all.
 *  64  a usage error (bad flag, `gh run list` itself failed or returned
 *      unparsable JSON, or `--save-dir` could not be created)
 *
 * In the documented pipeline the shell reports the summarizer's exit code
 * (`$?`), and even under `set -o pipefail` it is the *rightmost* non-zero
 * code that wins — so a harvester 64 (say, an expired gh token) reaches the
 * shell as the summarizer's 1 ("no data"). Read `PIPESTATUS[0]` (bash) or
 * `pipestatus[1]` (zsh), or harvest into a file first, whenever the
 * harvester's own code matters.
 */

const execFileAsync = promisify(execFile);

const HEALTH_WORKFLOW = "health.yml";
const HEALTH_JOB_NAME = "health";
const DEFAULT_SINCE = "2026-09-06T22:00:00Z";
const DEFAULT_LIMIT = 200;
const DEFAULT_GH = "gh";
// Bounded so a full default harvest is minutes, not a quarter hour, while
// staying far below GitHub's secondary rate limit (~5 GETs per run).
const CONCURRENCY = 4;

export const EXIT_OK = 0;
export const EXIT_NO_RECORDS = 1;
export const EXIT_PARTIAL = 3;
export const EXIT_USAGE = 64;

const RUN_LIST_JSON_FIELDS =
  "databaseId,headBranch,headSha,conclusion,status,createdAt,event,attempt";

// gh output can run into the tens of thousands of lines for a single job log
// (10,014 lines was the real measurement on 2026-09-07); give execFile a
// generous buffer so a legitimate log is never truncated into a JSON parse
// or line-parse failure.
const MAX_BUFFER = 200 * 1024 * 1024;

const STRING_FLAGS = {
  "--since": "since",
  "--branch": "branch",
  "--save-dir": "saveDir",
  "--gh": "gh",
};

function takeFlagValue(argv, index, flag) {
  const value = argv[index + 1];
  if (value === undefined || value === "" || value.startsWith("--")) {
    throw new Error(`${flag} requires a value`);
  }
  return value;
}

export function parseCliArgs(argv) {
  const options = {
    since: DEFAULT_SINCE,
    limit: DEFAULT_LIMIT,
    branch: undefined,
    saveDir: undefined,
    gh: DEFAULT_GH,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (Object.hasOwn(STRING_FLAGS, arg)) {
      options[STRING_FLAGS[arg]] = takeFlagValue(argv, index, arg);
      index += 1;
    } else if (arg === "--limit") {
      const value = argv[index + 1];
      const parsed = Number(value);
      if (!Number.isInteger(parsed) || parsed <= 0) {
        throw new Error(`--limit requires a positive integer, got: ${value}`);
      }
      options.limit = parsed;
      index += 1;
    } else if (arg.startsWith("--")) {
      throw new Error(`unknown flag: ${arg}`);
    } else {
      throw new Error(`unexpected positional argument: ${arg}`);
    }
  }
  return options;
}

export function buildRunListArgs({ since, limit, branch }) {
  const args = [
    "run",
    "list",
    `--workflow=${HEALTH_WORKFLOW}`,
    "--created",
    `>=${since}`,
    "--limit",
    String(limit),
    "--json",
    RUN_LIST_JSON_FIELDS,
  ];
  if (branch) args.push("--branch", branch);
  return args;
}

async function runGh(gh, args) {
  const { stdout } = await execFileAsync(gh, args, {
    encoding: "utf8",
    maxBuffer: MAX_BUFFER,
  });
  return stdout;
}

/** Resolves a run's `health` job, or `null` if the run has none. */
async function findHealthJob(gh, runId) {
  const stdout = await runGh(gh, ["run", "view", String(runId), "--json", "jobs"]);
  const parsed = JSON.parse(stdout);
  return (parsed.jobs ?? []).find((candidate) => candidate.name === HEALTH_JOB_NAME) ?? null;
}

// A job that never ran has no log to fetch: GitHub reports it with
// `conclusion: skipped`, or (cancelled while queued) with an empty `steps`
// array — a job that started always carries at least its "Set up job" step.
function jobNeverStarted(job) {
  return job.conclusion === "skipped" || (Array.isArray(job.steps) && job.steps.length === 0);
}

async function fetchJobLog(gh, jobId) {
  return runGh(gh, ["run", "view", "--job", String(jobId), "--log"]);
}

// `parseTimelines` hands back lines sliced out of the whole job log, and V8
// keeps a sliced string's parent alive for as long as the slice is — so
// holding ~3 short records per run would pin every ~1 MB log until the
// final join. Copying through a Buffer yields a flat string that does not.
function detachFromParentString(text) {
  return Buffer.from(text, "utf8").toString("utf8");
}

function manifestBase(run) {
  const shortSha = String(run.headSha ?? "").slice(0, 8);
  return `run=${run.databaseId} attempt=${run.attempt} event=${run.event} branch=${run.headBranch} sha=${shortSha} created=${run.createdAt} conclusion=${run.conclusion ?? "null"}`;
}

// Node's execFile error messages embed the subprocess's own stderr (often
// multi-line, e.g. "Command failed: gh ...\n<gh's own error output>\n"). The
// manifest is documented as one line per run, so collapse it to a single
// line rather than letting one failure fragment the stderr stream a
// downstream parser expects to read line-by-line.
function flattenErrorMessage(error) {
  return String(error.message ?? error)
    .trim()
    .replace(/\s*\r?\n\s*/g, " | ");
}

async function harvestRun(run, options, stderr) {
  const base = manifestBase(run);
  const skipped = { kind: "skipped", rawLines: [] };

  if (run.status !== "completed") {
    stderr.write(`${base} job=none skipped=status:${run.status}\n`);
    return skipped;
  }

  let jobId = null;
  try {
    const job = await findHealthJob(options.gh, run.databaseId);
    if (job === null) {
      stderr.write(`${base} job=none skipped=no-health-job\n`);
      return skipped;
    }
    jobId = job.databaseId;
    if (jobNeverStarted(job)) {
      stderr.write(`${base} job=${jobId} skipped=health-job-never-started\n`);
      return skipped;
    }

    const log = await fetchJobLog(options.gh, jobId);

    if (options.saveDir) {
      writeFileSync(join(options.saveDir, `run-${run.databaseId}-job-${jobId}.log`), log);
    }

    const rawLines = parseTimelines(log).map((record) => detachFromParentString(record.raw));
    stderr.write(`${base} job=${jobId} lines=${rawLines.length}\n`);
    return { kind: "harvested", rawLines };
  } catch (error) {
    stderr.write(`${base} job=${jobId ?? "none"} error=${flattenErrorMessage(error)}\n`);
    return { kind: "failed", rawLines: [] };
  }
}

/** Runs `worker` over `items` with at most `limit` in flight; results keep
 * the items' order regardless of completion order. */
async function mapWithConcurrency(items, limit, worker) {
  const results = new Array(items.length);
  let next = 0;
  async function lane() {
    while (next < items.length) {
      const index = next;
      next += 1;
      results[index] = await worker(items[index]);
    }
  }
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, lane));
  return results;
}

/**
 * Runs the CLI end to end and returns the exit code — never calls
 * `process.exit` itself, so tests can drive it with a stub `gh` script and
 * fake output streams.
 */
export async function runCli(argv, { stdout = process.stdout, stderr = process.stderr } = {}) {
  let options;
  try {
    options = parseCliArgs(argv);
  } catch (error) {
    stderr.write(`usage error: ${error.message}\n`);
    return EXIT_USAGE;
  }

  let runs;
  try {
    const raw = await runGh(options.gh, buildRunListArgs(options));
    runs = JSON.parse(raw);
    if (!Array.isArray(runs)) {
      throw new Error("expected a JSON array from `gh run list`");
    }
  } catch (error) {
    stderr.write(`usage error: gh run list failed: ${flattenErrorMessage(error)}\n`);
    return EXIT_USAGE;
  }

  if (runs.length >= options.limit) {
    stderr.write(
      `notice: gh run list returned ${runs.length} run(s), the --limit cap; older runs since ${options.since} may be missing (raise --limit)\n`,
    );
  }

  if (options.saveDir) {
    try {
      mkdirSync(options.saveDir, { recursive: true });
    } catch (error) {
      stderr.write(
        `usage error: cannot create --save-dir "${options.saveDir}": ${error.message}\n`,
      );
      return EXIT_USAGE;
    }
  }

  const results = await mapWithConcurrency(runs, CONCURRENCY, (run) =>
    harvestRun(run, options, stderr),
  );
  const outputLines = results.flatMap((result) => result.rawLines);
  const harvested = results.filter((result) => result.kind === "harvested").length;
  const failed = results.filter((result) => result.kind === "failed").length;

  if (outputLines.length > 0) stdout.write(`${outputLines.join("\n")}\n`);

  stderr.write(
    `runs=${runs.length} harvested=${harvested} failed=${failed} records=${outputLines.length}\n`,
  );

  if (failed > 0) return EXIT_PARTIAL;
  if (outputLines.length === 0) return EXIT_NO_RECORDS;
  return EXIT_OK;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  // A consumer that exits before reading stdin (`| head`, or the summarizer
  // rejecting its own flags) turns the final stdout write into an EPIPE
  // 'error' event; unhandled, Node would replace the harvester's exit code
  // with a crash trace. The consumer's own exit code already carries the story.
  process.stdout.on("error", (error) => {
    if (error.code !== "EPIPE") throw error;
  });
  process.exitCode = await runCli(process.argv.slice(2));
}
