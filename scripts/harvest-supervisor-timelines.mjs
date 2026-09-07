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
 *     | node scripts/supervisor-timeline-summary.mjs --strict
 *
 * This adds aggregation, not storage: nothing is persisted between runs.
 * `--save-dir` keeps the raw job logs it fetched; the weekly watch
 * (`.github/workflows/supervisor-watch.yml`) uses that to upload the logs an
 * R-A triage reads and to judge its `main`-only pass without a second fetch.
 *
 * `parseTimelines` already tolerates the `pnpm -r` package-label prefix and
 * vitest code frames quoting the tag (see the summarizer's `TAG_PATTERN`
 * comment) — keep that when touching this harvest path, since a job log is
 * exactly the kind of blob those guards exist for.
 *
 * Per run, only the `health` job's log lines are read, never the whole
 * run's. On 2026-09-07 the whole-run `gh run view <id> --log` for a real run
 * returned 18,615 lines with zero vitest output (the `health` job's log was
 * missing from the combined stream), while the single `health` job's own log
 * returned 10,014 lines including the records. Fetching the whole run is not
 * merely wasteful here, it silently loses the data.
 *
 * Both per-run fetches go through `gh api` at the REST endpoints rather than
 * through `gh run view` (#2931). The porcelain resolves the run and its
 * workflow again on every call, and its `--job … --log` form downloads the
 * *whole run's* log archive just to slice one job out of it: measured with
 * `GH_DEBUG=api` on 2026-09-07, `gh run view <id> --json jobs` cost 3
 * metered GETs and `gh run view --job <id> --log` cost 4 more plus the
 * archive download — ~7 per run, so a capped 200-run harvest sat near 1,400
 * against `GITHUB_TOKEN`'s 1,000-per-hour primary limit and would 403
 * mid-harvest, filing a tracking issue about its own rate limit. The two
 * REST calls below cost 1 metered GET each (the log endpoint's 302 to a
 * plaintext blob is a separate, unmetered host), so the same harvest is
 * ~400.
 *
 * `{owner}` / `{repo}` in those paths are expanded by gh from the current
 * repository (or `GH_REPO`) without an API call of its own — the same
 * resolution `gh run list` already relies on, so the harvester gains no new
 * way to be pointed at the wrong repo.
 *
 * The REST log body is NOT what `gh run view --job … --log` printed: the
 * porcelain prefixes every line with `<job>\t<step>\t`, the REST body has
 * only the runner's own `<ISO timestamp> ` prefix, and it opens with a UTF-8
 * BOM. `parseTimelines` sees through all of it (its `TAG_PATTERN` accepts
 * any quote-free prefix), but `record.raw` and the bytes `--save-dir` keeps
 * did change with #2931 — the contract is the parsed records, not the raw
 * line. `scripts/__tests__/fixtures/rest-job-log-capture.log` is a trimmed
 * real capture of that body; the suites build their synthetic logs from its
 * prefix rather than from a guess at it.
 *
 * Runs are harvested with bounded concurrency (`CONCURRENCY` below): each
 * run costs two `gh` invocations of roughly 1.5-3.5 s (measured 2026-09-07,
 * and re-measured across the REST migration at 3.4-3.9 s for the pair), so
 * a strictly sequential 200-run default harvest would take 10-17 minutes.
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
 * stderr carries one leading `window:` line (the effective `--since`,
 * `--limit`, and branch, since a reader cannot recover them from the rest),
 * a per-run manifest line (emitted as each run completes, so not
 * necessarily in enumeration order), and one final summary line:
 *
 *   window: since=<iso> limit=<n> branch=<b|all>
 *   run=<id> attempt=<n> event=<e> branch=<b> sha=<sha8> created=<iso> conclusion=<c> job=<jobId|none> lines=<n> failedRecords=<m>
 *   run=<id> attempt=<n> event=<e> branch=<b> sha=<sha8> created=<iso> conclusion=<c> job=<jobId|none> skipped=<reason>
 *   run=<id> attempt=<n> event=<e> branch=<b> sha=<sha8> created=<iso> conclusion=<c> job=<jobId|none> error=<reason>
 *   runs=<enumerated> harvested=<k> failed=<f> records=<r>
 *
 * The per-run `failedRecords=<m>` and the final summary's `failed=<f>` count
 * different things: the per-run one counts that run's own parsed records
 * whose `outcome=failed` (an R-A candidate inside an otherwise-successful
 * harvest), the summary one counts runs the harvester itself could not
 * fetch/save/parse. They no longer share a name (#2932), so no anchor is
 * needed to tell them apart. The weekly watch (#2915) greps the per-run
 * lines (`^run=… failedRecords=[1-9]`) to name the run whose job log holds
 * the R-A diagnostic block, without re-running the summarizer.
 *
 * Skipped (counts toward neither `harvested` nor `failed`):
 *   - a run whose `status` is not yet `completed` (no complete log to fetch);
 *   - a completed run with no `health` job at all;
 *   - a `health` job that never started (cancelled while still queued by
 *     `cancel-in-progress`, or skipped) — GitHub has no log for it and the
 *     job-logs endpoint answers 404, which must not read as a partial
 *     harvest;
 *   - a `health` job whose conclusion is not `success` and whose log carries
 *     no records (`skipped=no-records-health-<conclusion>`): it went red or
 *     was cancelled before the `pnpm test:workspace` step, so the missing
 *     records say nothing about the emitter. A job that started, emitted,
 *     and was then cancelled is harvested normally.
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
 *   1  at least one run was harvested, yet zero records were extracted —
 *      a green `health` job with no `[supervisor-timeline]` lines, i.e. the
 *      emitter (or this parser) went silent
 *   3  at least one run failed (its log could not be fetched, saved, or
 *      parsed as `[supervisor-timeline]` records) — a partial harvest,
 *      distinct from 0/1 regardless of how many records the other runs
 *      still yielded. A failed run contributes no records at all.
 *   4  nothing was harvestable: zero runs enumerated, or every enumerated
 *      run was skipped (see above). A quiet window, not a silent emitter.
 *  64  a usage error (bad flag, `gh run list` itself failed or returned
 *      unparsable JSON, or `--save-dir` could not be created)
 *
 * Every gh call is attempted twice with a short pause between, so one
 * transient API failure does not by itself demote a harvest to 3 (or 64).
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
// One weekly cadence plus a day of overlap: a run still in progress at
// harvest time is skipped this week, and without the overlap its `createdAt`
// would already sit behind next week's window start, so it would never be
// enumerated at all. The window is purely rolling: an `env=` identity
// contract change is carried as a version on the record itself (#2933) and
// the summarizer splits the population by that, so no date floor is needed
// here to keep the old and new contracts apart.
export const DEFAULT_WINDOW_DAYS = 8;
const DEFAULT_WINDOW_MS = DEFAULT_WINDOW_DAYS * 24 * 60 * 60 * 1000;
const DEFAULT_LIMIT = 200;
const DEFAULT_GH = "gh";
// Bounded so a full default harvest is minutes, not a quarter hour, while
// staying below GitHub's secondary rate limit. Since #2931 the two gh
// invocations per run are one metered GET each, so a capped 200-run harvest
// is ~400 GETs — comfortably inside the primary hourly limit at this
// concurrency.
const CONCURRENCY = 4;
// One retry with a short pause: a single transient 5xx or secondary-rate-limit
// 403 on one of hundreds of calls must not turn a complete population into a
// partial harvest (exit 3) that the weekly watch escalates.
const GH_ATTEMPTS = 2;
export const DEFAULT_RETRY_DELAY_MS = 2000;

// Test-ergonomics escape hatch (#2932): the harvester and supervisor-watch
// suites drive many real subprocesses and cannot afford genuine 2 s sleeps
// per invocation (a retryable gh failure x GH_ATTEMPTS - 1 pauses).
// Reachable only via this env var -- a production caller never sets it, so
// the default stays 2000 ms. An unset, empty, or non-numeric value falls
// back to the default rather than silently coercing to 0.
export function resolveDefaultRetryDelayMs() {
  const raw = process.env.HARVEST_RETRY_DELAY_MS;
  if (raw === undefined || raw === "") return DEFAULT_RETRY_DELAY_MS;
  const parsed = Number(raw);
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : DEFAULT_RETRY_DELAY_MS;
}

// ISO-8601 with seconds and a trailing `Z`, matching `--since`'s documented
// shape (`Date#toISOString` includes milliseconds, which this trims).
function toIsoSeconds(date) {
  return date.toISOString().replace(/\.\d{3}Z$/, "Z");
}

function defaultSince(now) {
  return toIsoSeconds(new Date(now.getTime() - DEFAULT_WINDOW_MS));
}

export const EXIT_OK = 0;
export const EXIT_NO_RECORDS = 1;
export const EXIT_PARTIAL = 3;
export const EXIT_NO_RUNS = 4;
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

export function parseCliArgs(argv, { now = new Date() } = {}) {
  const options = {
    since: defaultSince(now),
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

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function runGh({ gh, retryDelayMs }, args) {
  for (let attempt = 1; ; attempt += 1) {
    try {
      const { stdout } = await execFileAsync(gh, args, {
        encoding: "utf8",
        maxBuffer: MAX_BUFFER,
      });
      return stdout;
    } catch (error) {
      if (attempt >= GH_ATTEMPTS) throw error;
      await sleep(retryDelayMs);
    }
  }
}

// The run-jobs endpoint pages at 30 by default and a `health.yml` run
// already carries 10 jobs, so the page size is pinned at the maximum: one
// GET covers every realistic run. `gh api --paginate` is deliberately not
// used — on a wrapped-collection endpoint it concatenates one JSON object
// per page, which `JSON.parse` cannot read.
export const JOBS_PER_PAGE = 100;

/** `gh api` argv for one page of a run's jobs. */
export function buildRunJobsArgs(runId, page = 1) {
  const query = page > 1 ? `?per_page=${JOBS_PER_PAGE}&page=${page}` : `?per_page=${JOBS_PER_PAGE}`;
  return ["api", `repos/{owner}/{repo}/actions/runs/${runId}/jobs${query}`];
}

/** `gh api` argv for one job's log. */
export function buildJobLogArgs(jobId) {
  return ["api", `repos/{owner}/{repo}/actions/jobs/${jobId}/logs`];
}

/**
 * Resolves a run's `health` job, or `null` if the run has none.
 *
 * Walks further pages only when `health` was not on the one already fetched
 * and `total_count` says more jobs exist — so the >100-job case is supported
 * without every ordinary run paying for it. The empty-page guard is what
 * bounds the loop if `total_count` ever disagrees with the pages served.
 */
async function findHealthJob(ghOptions, runId) {
  let seen = 0;
  for (let page = 1; ; page += 1) {
    const stdout = await runGh(ghOptions, buildRunJobsArgs(runId, page));
    const parsed = JSON.parse(stdout);
    const jobs = parsed.jobs ?? [];
    const health = jobs.find((candidate) => candidate.name === HEALTH_JOB_NAME);
    if (health) return health;
    seen += jobs.length;
    if (jobs.length === 0 || seen >= (parsed.total_count ?? seen)) return null;
  }
}

// A job that never ran has no log to fetch: GitHub reports it with
// `conclusion: skipped`, or (cancelled while queued) with an empty `steps`
// array — a job that started always carries at least its "Set up job" step.
function jobNeverStarted(job) {
  return job.conclusion === "skipped" || (Array.isArray(job.steps) && job.steps.length === 0);
}

// `gh api` exits non-zero on an HTTP error, so a JSON error envelope
// arriving on stdout means the fetch "succeeded" while returning no log at
// all. Unguarded that lands as `lines=0`, which the weekly watch reads as
// the emitter going silent rather than as a broken fetch — the one failure
// shape this lane must never mistake for data.
function apiErrorMessage(body) {
  if (!body.slice(0, 64).trimStart().startsWith("{")) return null;
  let parsed;
  try {
    parsed = JSON.parse(body);
  } catch {
    return null;
  }
  return typeof parsed?.message === "string" ? parsed.message : null;
}

// One metered GET: the endpoint answers 302 to a plaintext blob on a
// separate host and gh follows it itself (verified live 2026-09-07 with
// `GH_DEBUG=api`: 302 Found -> actions-results blob, 200 OK). The body is
// therefore the log itself, verbatim, and must not be post-processed here.
async function fetchJobLog(ghOptions, jobId) {
  const body = await runGh(ghOptions, buildJobLogArgs(jobId));
  const message = apiErrorMessage(body);
  if (message !== null) {
    throw new Error(`job ${jobId} log fetch returned an API error envelope: ${message}`);
  }
  return body;
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
    const job = await findHealthJob(options.ghOptions, run.databaseId);
    if (job === null) {
      stderr.write(`${base} job=none skipped=no-health-job\n`);
      return skipped;
    }
    // REST names the job's numeric id `id`; only `gh run list --json`
    // (the run enumeration above) calls it `databaseId`.
    jobId = job.id;
    if (jobNeverStarted(job)) {
      stderr.write(`${base} job=${jobId} skipped=health-job-never-started\n`);
      return skipped;
    }

    const log = await fetchJobLog(options.ghOptions, jobId);

    if (options.saveDir) {
      writeFileSync(join(options.saveDir, `run-${run.databaseId}-job-${jobId}.log`), log);
    }

    const records = parseTimelines(log);
    // A `health` job that went red (or was cancelled) before its
    // `pnpm test:workspace` step has a log and no records, and that is not
    // the emitter vanishing — only a job that ran to a green conclusion with
    // zero records is. A supervisor failure itself still shows up as records
    // (`outcome=failed` is emitted on the way out), so it is never hidden here.
    if (records.length === 0 && job.conclusion !== "success") {
      stderr.write(`${base} job=${jobId} skipped=no-records-health-${job.conclusion ?? "null"}\n`);
      return skipped;
    }
    const rawLines = records.map((record) => detachFromParentString(record.raw));
    const failedRecords = records.filter((record) => record.outcome === "failed").length;
    stderr.write(`${base} job=${jobId} lines=${rawLines.length} failedRecords=${failedRecords}\n`);
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
export async function runCli(
  argv,
  {
    stdout = process.stdout,
    stderr = process.stderr,
    now = new Date(),
    retryDelayMs = resolveDefaultRetryDelayMs(),
  } = {},
) {
  let options;
  try {
    options = parseCliArgs(argv, { now });
  } catch (error) {
    stderr.write(`usage error: ${error.message}\n`);
    return EXIT_USAGE;
  }
  options.ghOptions = { gh: options.gh, retryDelayMs };

  // The window is the one input a reader of the manifest cannot otherwise
  // recover — and an explicit `--since` can lie in the future, which
  // enumerates nothing and would otherwise read as a quiet week.
  stderr.write(
    `window: since=${options.since} limit=${options.limit} branch=${options.branch ?? "all"}\n`,
  );
  if (Date.parse(options.since) > now.getTime()) {
    stderr.write(
      `warning: --since ${options.since} lies in the future; gh run list will enumerate nothing\n`,
    );
  }

  let runs;
  try {
    const raw = await runGh(options.ghOptions, buildRunListArgs(options));
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
  if (harvested === 0) return EXIT_NO_RUNS;
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
