#!/usr/bin/env node
import { execFile } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";

import { parseTimelineLine } from "./supervisor-timeline-summary.mjs";

/**
 * Re-harvests the ubuntu `[supervisor-timeline]` population that
 * `health.yml` has been sampling since #2902 / PR #2906
 * (`ZFB_SUPERVISOR_TIMELINE: "1"` on its `pnpm test:workspace` step). Today
 * those lines only exist scattered inside individual CI job logs; this
 * script re-fetches them on demand via `gh` and re-parses them with the
 * summarizer's own `parseTimelineLine`, so the population is
 * re-aggregatable in one command:
 *
 *   node scripts/harvest-supervisor-timelines.mjs \
 *     | node scripts/supervisor-timeline-summary.mjs --strict --allow-drift env
 *
 * This adds aggregation, not storage: nothing is persisted between runs
 * (see `--save-dir` below for an opt-in exception used for debugging).
 *
 * Per run, only the `health` job's log is fetched, never the whole run's.
 * On 2026-09-07 the whole-run `gh run view <id> --log` for a real run
 * returned 18,615 lines with zero vitest output (the `health` job's log was
 * missing from the combined stream), while `gh run view --job <jobId> --log`
 * for that same run's `health` job returned 10,014 lines including the
 * records. Fetching the whole run is not merely wasteful here, it silently
 * loses the data.
 *
 * Output contract
 * ----------------
 * stdout carries only the original `[supervisor-timeline]` line text (one
 * per record, in the order runs were enumerated) — exactly what
 * `parseTimelineLine` accepted, so it round-trips through the summarizer
 * unchanged. All output is buffered and written in a single stdout write
 * at the very end, so a late failure can never leave a plausible-looking
 * partial dataset on stdout for a downstream `pipefail` consumer to trust.
 *
 * stderr carries a per-run manifest line plus one final summary line:
 *
 *   run=<id> attempt=<n> branch=<b> sha=<sha8> created=<iso> conclusion=<c> job=<jobId|none> lines=<n>
 *   run=<id> attempt=<n> branch=<b> sha=<sha8> created=<iso> conclusion=<c> job=<jobId|none> skipped=<reason>
 *   run=<id> attempt=<n> branch=<b> sha=<sha8> created=<iso> conclusion=<c> job=<jobId|none> error=<reason>
 *   runs=<enumerated> harvested=<k> failed=<f> records=<r>
 *
 * A run whose `status` is not yet `completed` (no complete log to fetch)
 * and a completed run with no `health` job (skipped/cancelled) are both
 * reported with `skipped=<reason>` and count toward neither `harvested` nor
 * `failed`.
 *
 * Exit codes (mirrors the summarizer's contract style: distinct codes for
 * distinct situations, never conflating "nothing went wrong" with "there is
 * no data"):
 *
 *   0  every attempted per-job fetch succeeded and at least one record was
 *      extracted
 *   1  no per-job fetch failed, but zero records were extracted
 *   3  at least one per-job fetch failed (partial harvest) — distinct from
 *      0/1 so `set -o pipefail` notices, regardless of how many records
 *      were still extracted from the runs that did succeed
 *  64  a usage error (bad flag, `gh run list` itself failed or returned
 *      unparsable JSON, or `--save-dir` could not be created)
 */

const execFileAsync = promisify(execFile);

export const HEALTH_WORKFLOW = "health.yml";
export const HEALTH_JOB_NAME = "health";
export const DEFAULT_SINCE = "2026-09-06T22:00:00Z";
export const DEFAULT_LIMIT = 200;
export const DEFAULT_GH = "gh";

export const EXIT_OK = 0;
export const EXIT_NO_RECORDS = 1;
export const EXIT_PARTIAL = 3;
export const EXIT_USAGE = 64;

const RUN_LIST_JSON_FIELDS =
  "databaseId,headBranch,headSha,conclusion,status,createdAt,event,attempt,url";

// gh output can run into the tens of thousands of lines for a single job log
// (10,014 lines was the real measurement on 2026-09-07); give execFile a
// generous buffer so a legitimate log is never truncated into a JSON parse
// or line-parse failure.
const MAX_BUFFER = 200 * 1024 * 1024;

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
    if (arg === "--since") {
      const value = argv[index + 1];
      if (value === undefined || value.startsWith("--")) {
        throw new Error("--since requires a value");
      }
      options.since = value;
      index += 1;
    } else if (arg === "--limit") {
      const value = argv[index + 1];
      const parsed = value === undefined ? NaN : Number(value);
      if (!Number.isInteger(parsed) || parsed <= 0) {
        throw new Error(`--limit requires a positive integer, got: ${value}`);
      }
      options.limit = parsed;
      index += 1;
    } else if (arg === "--branch") {
      const value = argv[index + 1];
      if (value === undefined || value.startsWith("--")) {
        throw new Error("--branch requires a value");
      }
      options.branch = value;
      index += 1;
    } else if (arg === "--save-dir") {
      const value = argv[index + 1];
      if (value === undefined || value.startsWith("--")) {
        throw new Error("--save-dir requires a value");
      }
      options.saveDir = value;
      index += 1;
    } else if (arg === "--gh") {
      const value = argv[index + 1];
      if (value === undefined || value.startsWith("--")) {
        throw new Error("--gh requires a value");
      }
      options.gh = value;
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

/** Resolves the `health` job's id for a run, or `null` if the run has none
 * (e.g. it was skipped or cancelled before that job started). */
async function findHealthJobId(gh, runId) {
  const stdout = await runGh(gh, ["run", "view", String(runId), "--json", "jobs"]);
  const parsed = JSON.parse(stdout);
  const job = (parsed.jobs ?? []).find((candidate) => candidate.name === HEALTH_JOB_NAME);
  return job ? job.databaseId : null;
}

async function fetchJobLog(gh, jobId) {
  return runGh(gh, ["run", "view", "--job", String(jobId), "--log"]);
}

function manifestBase(run) {
  const shortSha = String(run.headSha ?? "").slice(0, 8);
  return `run=${run.databaseId} attempt=${run.attempt} branch=${run.headBranch} sha=${shortSha} created=${run.createdAt} conclusion=${run.conclusion ?? "null"}`;
}

// Node's execFile error messages embed the subprocess's own stderr (often
// multi-line, e.g. "Command failed: gh ...\n<gh's own error output>"). The
// manifest is documented as one line per run, so collapse it to a single
// line rather than letting one failure fragment the stderr stream a
// downstream parser expects to read line-by-line.
function flattenErrorMessage(error) {
  return String(error.message ?? error).replace(/\s*\r?\n\s*/g, " | ");
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

  const outputLines = [];
  let harvested = 0;
  let failed = 0;

  for (const run of runs) {
    const base = manifestBase(run);

    if (run.status !== "completed") {
      stderr.write(`${base} job=none skipped=status:${run.status}\n`);
      continue;
    }

    let jobId = null;
    try {
      jobId = await findHealthJobId(options.gh, run.databaseId);
      if (jobId === null) {
        stderr.write(`${base} job=none skipped=no-health-job\n`);
        continue;
      }

      const log = await fetchJobLog(options.gh, jobId);

      if (options.saveDir) {
        writeFileSync(join(options.saveDir, `run-${run.databaseId}-job-${jobId}.log`), log);
      }

      let lineCount = 0;
      for (const line of log.split(/\r?\n/)) {
        const record = parseTimelineLine(line);
        if (record) {
          outputLines.push(record.raw);
          lineCount += 1;
        }
      }

      harvested += 1;
      stderr.write(`${base} job=${jobId} lines=${lineCount}\n`);
    } catch (error) {
      failed += 1;
      stderr.write(`${base} job=${jobId ?? "none"} error=${flattenErrorMessage(error)}\n`);
    }
  }

  stdout.write(outputLines.length > 0 ? `${outputLines.join("\n")}\n` : "");

  stderr.write(
    `runs=${runs.length} harvested=${harvested} failed=${failed} records=${outputLines.length}\n`,
  );

  if (failed > 0) return EXIT_PARTIAL;
  if (outputLines.length === 0) return EXIT_NO_RECORDS;
  return EXIT_OK;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = await runCli(process.argv.slice(2));
}
