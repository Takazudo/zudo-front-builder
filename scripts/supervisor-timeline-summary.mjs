#!/usr/bin/env node
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

import { parseEnvIdentity } from "./supervisor-env-identity.mjs";

/**
 * Parses `[supervisor-timeline]` lines — emitted by `timelineLine()` in
 * `scripts/__tests__/docs-dev-supervisor.test.mjs` behind
 * `ZFB_SUPERVISOR_TIMELINE=1` — and reports the pre-UP / package-manager-startup
 * / server-listen / whole-case distributions for a chosen case. It exists so
 * the "settle-it protocol" locked in #2887's decision comment
 * (https://github.com/Takazudo/zudo-front-builder/issues/2887#issuecomment-5559910016)
 * has a real, tested analysis step instead of the silent no-op #2902 found
 * (`summarize.mjs` reads a different harness's `results/*.jsonl` and, handed
 * a `timelines.txt`, prints nothing and exits 0).
 *
 * The three pre-registered rules from that comment, summarized (read the
 * comment for the full text — this is not a substitute):
 *
 *   R-A — any `outcome=failed` line is a real failure needing triage: read
 *         its diagnostic block and classify it (startup starvation, a
 *         stalled boot stage, or a fixture defect) rather than guessing.
 *   R-B — no failure, but the max pre-UP across sampled runs reaches
 *         `--threshold x --budget-ms` (default 75% of 10s): the budget is
 *         measured-too-tight and should be recomputed from the observed
 *         distribution, not bumped by feel.
 *   R-C — no failure and pre-UP stays comfortably under budget: nothing to
 *         do; the next occurrence — local or CI — is the experiment.
 *         `.github/workflows/supervisor-watch.yml` runs `harvest →
 *         summarize --strict` weekly and files/closes the deduped
 *         `[exam-failure] Supervisor watch is failing` issue — the
 *         automated form of "the next occurrence is the experiment"; a
 *         human still classifies an R-A hit from the diagnostic block in
 *         the run's job log (saved in the workflow artifact), and the
 *         harvester manifest's `failedRecords=` field names the run.
 *
 * `expected-failure` (the `hidden` case's deliberately induced pre-UP
 * timeout) is a distinct outcome from `failed` by construction, precisely so
 * it can never satisfy R-A on its own.
 *
 * Exit codes are the whole point of this tool (see #2902/#2903): 0 means at
 * least one line was parsed and analysed, 1 means nothing matched (the exact
 * failure mode this replaces), 2 is a --strict finding, and 64 is a usage or
 * parse error that must never be confused with "no data".
 *
 * Env identity contract versions and cohorts (#2933)
 * ---------------------------------------------------
 * The `env` identity field is `v<N>:sha256:…` — the steering-only digest
 * from `scripts/supervisor-env-identity.mjs` (its header owns the token
 * grammar and the key list), prefixed with the version of the contract that
 * produced it. An unversioned `env=sha256:…` record predates the prefix and
 * is **v1**, the same contract v1 now labels explicitly: the parser
 * canonicalises it, so a legacy `sha256:X` and a fresh `v1:sha256:X` are one
 * identity, not drift. Every record's version is `record.envVersion`.
 *
 * Which digest a run emits is decided by the emitting commit, never by the
 * run's date, so a population is split by *version*, not by time. The
 * selected case's records are grouped into one cohort per version and the
 * rules apply as follows — exactly this, because "compare within a version"
 * alone would still let incompatible records be pooled:
 *
 *   - R-A stays GLOBAL. `outcome=failed` is a finding under any contract,
 *     inside or outside `--case`, so it is counted before any split.
 *   - Identity drift is evaluated WITHIN each cohort, over every
 *     `IDENTITY_FIELDS` entry. A field that differs only *between* cohorts
 *     is visible in each cohort's `identity` lines but is not drift: no
 *     distribution mixes those records, so nothing is corrupted. (Accepted
 *     cost: a change to another identity field that lands in the same week
 *     as a contract bump is not a strict finding that week.)
 *   - R-B and the four timing distributions are computed PER cohort, never
 *     over mixed versions. Each cohort prints its own block and verdict.
 *   - `--strict` fails when ANY cohort trips R-B or carries drift that is
 *     not allow-listed.
 *   - A version split is reported prominently (`!!! ENV IDENTITY CONTRACT
 *     VERSION SPLIT !!!`) but is NOT, by itself, a `--strict` finding: it is
 *     the expected shape of the week a `STEERING_ENV_KEYS` change merges, and
 *     it heals as the old cohort rolls out of the harvest window. So a
 *     contract bump needs no `--allow-drift`, no `--branch main`, and no
 *     date epoch.
 *
 * `--allow-drift <field>[,<field>]` (#2908) lists identity fields whose drift
 * is still reported but never trips `--strict`. It is the escape hatch for
 * a *deliberately* mixed population: an intentional A/B, or the weekly
 * watch's all-branch pass (heterogeneous by design, so it allow-lists every
 * `IDENTITY_FIELDS` entry and judges identity drift on `main` only — see the
 * R-C bullet above). `--allow-drift env` has one meaning, and it is within
 * a cohort: two different steering digests under the SAME contract version
 * — a real steering-input change such as a node patch bump, a runner-image
 * change to `PNPM_HOME`/`TMPDIR`, or a new `NODE_OPTIONS` — is reported but
 * does not trip `--strict`. It says nothing across cohorts, because
 * cross-version digests are never compared in the first place; it is not
 * the remedy for a contract change and the documented harvester-to-
 * summarizer pipeline does not pass it. Field names are validated against
 * `IDENTITY_FIELDS`; an unknown name or an empty list is a usage error (64),
 * not a silently-ignored no-op, and a repeated flag accumulates rather than
 * replacing the earlier list. The drift report still prints allow-listed
 * fields, marked as such.
 */

// Not anchored to line-start: `pnpm -r`'s parallel reporter prefixes every
// line with a package label (e.g. ". test: [supervisor-timeline] ...") when
// the emitting test lives at the workspace root, which is exactly how the
// documented pipeline (`pnpm test:workspace` piped through `grep -h`) is
// run in practice. An anchored pattern silently drops every real captured
// line -- the exact "join" defect #2902 exists to fix (verified live: see
// #2905's confirm pass).
// The prefix before the tag may not contain a quote character. `pnpm -r`'s
// reporter prefix (". test: ") has none, but a vitest code frame quoting the
// tag always does -- and the emitting test file now contains such a literal,
// so vitest prints exactly that frame when a supervisor test fails. Matching
// it lets a fragment like `case=hidden");` clear the key=value guard below
// and then throw on the missing `outcome`, taking the whole analysis down
// with exit 64 and discarding the genuine samples beside it -- in the R-A
// case (a failure happened), which is when the tool is needed most.
export const TAG_PATTERN = /^[^"'`]*\[supervisor-timeline\]\s+(\S.*)$/;

// A real emission's first token is always `case=<label>`, so requiring a
// key=value shape there is what separates an actual record from a line that
// merely *quotes* the tag. Two such lines occur in the documented pipeline
// itself: a vitest code frame of the emitting test file (which contains the
// literal `"[supervisor-timeline]"`), and this tool's own
// `!!! NO [supervisor-timeline] LINES FOUND !!!` / `parsed N
// [supervisor-timeline] line(s)` output when a whole CI job log is grepped.
// Without this guard each of those throws and takes the entire analysis down
// with exit 64, discarding the genuine samples sitting beside them -- and it
// does so precisely in the R-A case (a failure happened) where the tool is
// needed most.
const FIRST_TOKEN_PATTERN = /^[A-Za-z][\w.-]*=/;

export const IDENTITY_FIELDS = ["runner", "zudoDoc", "runParallel", "fixtureShape", "env"];

export const OUTCOMES = ["ok", "expected-failure", "failed"];

export const EXIT_OK = 0;
export const EXIT_NO_SAMPLES = 1;
export const EXIT_STRICT = 2;
export const EXIT_USAGE = 64;

export const DEFAULT_CASE = "up+boom";
// Kept in sync with PROCESS_TIMEOUT_MS in scripts/__tests__/docs-dev-supervisor.test.mjs
// by the drift-guard test in this script's own test file — see that test for why.
export const DEFAULT_BUDGET_MS = 10_000;
export const DEFAULT_THRESHOLD = 0.75;

/** The locked quantile definition — must match byte-for-byte so figures stay
 * comparable to the numbers quoted in the #2887 decision comment. */
export function quantile(sorted, p) {
  return sorted.length
    ? sorted[Math.min(sorted.length - 1, Math.floor(p * (sorted.length - 1) + 0.5))]
    : NaN;
}

export function distributionStats(values) {
  const sorted = [...values].sort((a, b) => a - b);
  return {
    n: sorted.length,
    min: sorted.length ? sorted[0] : NaN,
    p50: quantile(sorted, 0.5),
    p90: quantile(sorted, 0.9),
    p99: quantile(sorted, 0.99),
    max: sorted.length ? sorted[sorted.length - 1] : NaN,
  };
}

function toMs(value, key, line) {
  if (!/^-?\d+$/.test(value)) {
    throw new Error(
      `malformed [supervisor-timeline] line (non-integer ms for "${key}=${value}"): ${line}`,
    );
  }
  return Number(value);
}

/**
 * Parses one line. Returns `null` for a line that does not carry the
 * `[supervisor-timeline]` tag, or that carries it only as quoted prose rather
 * than as a record (ignored, not an error — the expected usage pipes a whole
 * vitest log through this tool). Throws when the line really is a record
 * (tag + a leading `key=value` token) but the content does not parse, since
 * that must surface as a usage error (exit 64), never as "no data" (exit 1).
 */
export function parseTimelineLine(line) {
  const match = TAG_PATTERN.exec(line);
  if (!match) return null;

  const tokens = match[1].split(/\s+/).filter(Boolean);
  if (tokens.length === 0 || !FIRST_TOKEN_PATTERN.test(tokens[0])) return null;

  const fields = {};
  const marks = {};
  for (const token of tokens) {
    const eq = token.indexOf("=");
    if (eq <= 0) {
      throw new Error(
        `malformed [supervisor-timeline] line (bad key=value token "${token}"): ${line}`,
      );
    }
    const key = token.slice(0, eq);
    const value = token.slice(eq + 1);
    if (key === "case" || key === "outcome" || IDENTITY_FIELDS.includes(key)) {
      fields[key] = value;
    } else if (key === "total") {
      fields.total = toMs(value, key, line);
    } else {
      marks[key] = toMs(value, key, line);
    }
  }

  for (const required of ["case", "outcome", "total", ...IDENTITY_FIELDS]) {
    if (fields[required] === undefined) {
      throw new Error(`malformed [supervisor-timeline] line (missing "${required}"): ${line}`);
    }
  }
  if (!OUTCOMES.includes(fields.outcome)) {
    throw new Error(
      `malformed [supervisor-timeline] line (unknown outcome "${fields.outcome}"): ${line}`,
    );
  }

  // Canonicalised: a legacy unversioned token and its explicit-v1 form must
  // compare equal in the drift check (see the header), so the record carries
  // `v<N>:…` whatever the wire said. `raw` keeps the original line.
  const envIdentity = parseEnvIdentity(fields.env);
  const identity = Object.fromEntries(IDENTITY_FIELDS.map((field) => [field, fields[field]]));
  identity.env = envIdentity.token;

  return {
    case: fields.case,
    outcome: fields.outcome,
    total: fields.total,
    identity,
    envVersion: envIdentity.version,
    marks,
    raw: line,
  };
}

/** Parses a whole blob (a raw log, or several files joined by newlines).
 * Non-tagged lines (ordinary vitest output) are silently skipped. */
export function parseTimelines(text) {
  const records = [];
  for (const line of text.split(/\r?\n/)) {
    const record = parseTimelineLine(line);
    if (record) records.push(record);
  }
  return records;
}

export function outcomeCounts(records) {
  const counts = { ok: 0, "expected-failure": 0, failed: 0 };
  for (const record of records) counts[record.outcome] += 1;
  return counts;
}

/** The report's four distributions for one case's records. */
export function summarizeCase(caseRecords) {
  const preUpValues = caseRecords
    .filter((record) => record.marks["first-up-line"] !== undefined)
    .map((record) => record.marks["first-up-line"]);
  const pkgStartupValues = caseRecords
    .filter((record) => record.marks["first-stdout-byte"] !== undefined)
    .map((record) => record.marks["first-stdout-byte"]);
  const serverListenValues = caseRecords
    .filter(
      (record) =>
        record.marks["first-up-line"] !== undefined &&
        record.marks["first-stdout-byte"] !== undefined,
    )
    .map((record) => record.marks["first-up-line"] - record.marks["first-stdout-byte"]);
  const wholeCaseValues = caseRecords.map((record) => record.total);

  return {
    preUp: distributionStats(preUpValues),
    pkgStartup: distributionStats(pkgStartupValues),
    serverListen: distributionStats(serverListenValues),
    wholeCase: distributionStats(wholeCaseValues),
  };
}

/** Distinct values per identity field, scoped to the records being reported
 * on — the population whose quantiles the drift check is meant to protect. */
export function identityDrift(caseRecords) {
  const distinct = {};
  for (const field of IDENTITY_FIELDS) {
    distinct[field] = [...new Set(caseRecords.map((record) => record.identity[field]))];
  }
  const driftedFields = IDENTITY_FIELDS.filter((field) => distinct[field].length > 1);
  return { distinct, driftedFields, hasDrift: driftedFields.length > 0 };
}

export function evaluateRB(preUpStats, budgetMs, threshold) {
  const boundary = threshold * budgetMs;
  const tripped = Number.isFinite(preUpStats.max) && preUpStats.max >= boundary;
  return { boundary, max: preUpStats.max, tripped };
}

/** One case's records grouped by env contract version, ascending — the
 * populations the distributions, drift check, and R-B are each scoped to. */
export function envVersionCohorts(caseRecords) {
  const byVersion = new Map();
  for (const record of caseRecords) {
    if (!byVersion.has(record.envVersion)) byVersion.set(record.envVersion, []);
    byVersion.get(record.envVersion).push(record);
  }
  return [...byVersion.entries()]
    .sort(([a], [b]) => a - b)
    .map(([version, records]) => ({ version, records }));
}

/** Everything the report and `--strict` need for one case: a distribution
 * summary, drift verdict, and R-B verdict per cohort, never pooled. */
export function analyzeCase(caseRecords, { budgetMs, threshold }) {
  const cohorts = envVersionCohorts(caseRecords).map(({ version, records }) => {
    const summary = summarizeCase(records);
    return {
      version,
      records,
      summary,
      drift: identityDrift(records),
      rb: evaluateRB(summary.preUp, budgetMs, threshold),
    };
  });
  return { cohorts, split: cohorts.length > 1 };
}

function fmt(value) {
  return Number.isFinite(value) ? `${value}ms` : "n/a";
}

function formatDistribution(label, stats) {
  return `  ${label}: n=${stats.n} min=${fmt(stats.min)} p50=${fmt(stats.p50)} p90=${fmt(stats.p90)} p99=${fmt(stats.p99)} max=${fmt(stats.max)}`;
}

/** The drifted fields that are not on the `--allow-drift` list — the ones
 * `--strict` acts on. */
function strictRelevantDrift(drift, allowDrift) {
  return drift.driftedFields.filter((field) => !allowDrift.includes(field));
}

/** True when `--strict` has something to act on in this cohort. */
function cohortTripsStrict(cohort, allowDrift) {
  return strictRelevantDrift(cohort.drift, allowDrift).length > 0 || cohort.rb.tripped;
}

function formatCohortSizes(cohorts) {
  return cohorts.map((cohort) => `v${cohort.version} (n=${cohort.records.length})`).join(", ");
}

function formatVersionSplit(cohorts) {
  return [
    "!!! ENV IDENTITY CONTRACT VERSION SPLIT !!!",
    `The sampled population mixes env contract versions: ${formatCohortSizes(cohorts)}.`,
    "Distributions, identity drift, and R-B are judged per cohort below and never pooled",
    "across versions; the split itself is not a --strict finding (see the doc comment).",
  ].join("\n");
}

function formatDriftWarning(drift, allowDrift) {
  const lines = [
    "!!! INPUT DRIFT DETECTED !!!",
    "The sampled distribution mixes more than one value for:",
  ];
  for (const field of drift.driftedFields) {
    const suffix = allowDrift.includes(field) ? " (allowed by --allow-drift)" : "";
    lines.push(`  - ${field}: ${drift.distinct[field].join(", ")}${suffix}`);
  }
  lines.push("Quantiles above may not be meaningful across mixed inputs — see #2902/#2903.");
  if (strictRelevantDrift(drift, allowDrift).length === 0) {
    lines.push("Every drifted field is allow-listed; --strict does not trip on this drift.");
  }
  return lines.join("\n");
}

export function parseCliArgs(argv) {
  const options = {
    caseLabel: DEFAULT_CASE,
    budgetMs: DEFAULT_BUDGET_MS,
    threshold: DEFAULT_THRESHOLD,
    strict: false,
    allowDrift: [],
    files: [],
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--case") {
      const value = argv[index + 1];
      if (value === undefined || value.startsWith("--")) throw new Error("--case requires a value");
      options.caseLabel = value;
      index += 1;
    } else if (arg === "--allow-drift") {
      const value = argv[index + 1];
      if (value === undefined || value.startsWith("--")) {
        throw new Error("--allow-drift requires a value");
      }
      const fields = value
        .split(",")
        .map((field) => field.trim())
        .filter(Boolean);
      if (fields.length === 0) {
        throw new Error(`--allow-drift requires at least one field name, got: "${value}"`);
      }
      for (const field of fields) {
        if (!IDENTITY_FIELDS.includes(field)) {
          throw new Error(`unknown field for --allow-drift: "${field}"`);
        }
        if (!options.allowDrift.includes(field)) options.allowDrift.push(field);
      }
      index += 1;
    } else if (arg === "--budget-ms") {
      const value = argv[index + 1];
      const parsed = value === undefined ? NaN : Number(value);
      if (!Number.isFinite(parsed) || parsed <= 0) {
        throw new Error(`--budget-ms requires a positive number, got: ${value}`);
      }
      options.budgetMs = parsed;
      index += 1;
    } else if (arg === "--threshold") {
      const value = argv[index + 1];
      const parsed = value === undefined ? NaN : Number(value);
      if (!Number.isFinite(parsed) || parsed <= 0) {
        throw new Error(`--threshold requires a positive number, got: ${value}`);
      }
      options.threshold = parsed;
      index += 1;
    } else if (arg === "--strict") {
      options.strict = true;
    } else if (arg.startsWith("--")) {
      throw new Error(`unknown flag: ${arg}`);
    } else {
      options.files.push(arg);
    }
  }
  return options;
}

async function readAllStdin(stream) {
  const chunks = [];
  for await (const chunk of stream) {
    chunks.push(typeof chunk === "string" ? chunk : chunk.toString("utf8"));
  }
  return chunks.join("");
}

async function collectInput(files, stdin) {
  if (files.length > 0) {
    return files
      .map((file) => {
        try {
          return readFileSync(file, "utf8");
        } catch (error) {
          throw new Error(`cannot read file "${file}": ${error.message}`);
        }
      })
      .join("\n");
  }
  if (typeof stdin === "string") return stdin;
  return readAllStdin(stdin ?? process.stdin);
}

/**
 * Runs the CLI end to end and returns the exit code — never calls
 * `process.exit` itself, so tests can drive it with fixture strings and fake
 * streams. Precedence (locked in #2902/#2903, tested exactly in this order):
 *
 *   1. usage/malformed              -> 64
 *   2. --strict and outcome=failed anywhere (even outside --case,
 *      under any env contract version)                           -> 2
 *   3. no samples for the selected --case (including empty input) -> 1
 *   4. --strict and, in ANY env-version cohort of the selected case,
 *      non-allow-listed INPUT drift or an R-B trip                -> 2
 *   5. otherwise (a version split alone lands here)               -> 0
 */
export async function runCli(
  argv,
  { stdin, stdout = process.stdout, stderr = process.stderr } = {},
) {
  let options;
  try {
    options = parseCliArgs(argv);
  } catch (error) {
    stderr.write(`usage error: ${error.message}\n`);
    return EXIT_USAGE;
  }

  let text;
  try {
    text = await collectInput(options.files, stdin);
  } catch (error) {
    stderr.write(`usage error: ${error.message}\n`);
    return EXIT_USAGE;
  }

  let records;
  try {
    records = parseTimelines(text);
  } catch (error) {
    stderr.write(`usage error: ${error.message}\n`);
    return EXIT_USAGE;
  }

  if (records.length === 0) {
    stderr.write(
      [
        "!!! NO [supervisor-timeline] LINES FOUND !!!",
        "No input matched the [supervisor-timeline] tag -- nothing was parsed or analysed.",
        "Pipe a vitest log captured with ZFB_SUPERVISOR_TIMELINE=1 set.",
        "",
      ].join("\n"),
    );
    return EXIT_NO_SAMPLES;
  }

  const counts = outcomeCounts(records);
  const anyFailed = counts.failed > 0;

  stdout.write(`parsed ${records.length} [supervisor-timeline] line(s)\n`);
  stdout.write(
    `outcomes: ok=${counts.ok} expected-failure=${counts["expected-failure"]} failed=${counts.failed}\n\n`,
  );

  if (anyFailed) {
    const failedCases = [
      ...new Set(
        records.filter((record) => record.outcome === "failed").map((record) => record.case),
      ),
    ];
    stderr.write(
      `!!! outcome=failed present (case(s): ${failedCases.join(", ")}) -- R-A: classify its diagnostic block !!!\n\n`,
    );
  }

  const caseRecords = records.filter((record) => record.case === options.caseLabel);

  let analysis = null;

  if (caseRecords.length === 0) {
    stdout.write(`no samples for case "${options.caseLabel}"\n`);
  } else {
    analysis = analyzeCase(caseRecords, options);

    if (analysis.split) {
      stdout.write(
        `case "${options.caseLabel}" (n=${caseRecords.length}) spans ${analysis.cohorts.length} env contract versions: ${formatCohortSizes(analysis.cohorts)}\n\n`,
      );
      stdout.write(`${formatVersionSplit(analysis.cohorts)}\n\n`);
    }

    for (const cohort of analysis.cohorts) {
      const { summary, drift, rb } = cohort;
      const tag = `[env contract v${cohort.version}]`;

      stdout.write(`case "${options.caseLabel}" (n=${cohort.records.length}) ${tag}:\n`);
      stdout.write(`${formatDistribution("pre-UP (spawn -> UP line)", summary.preUp)}\n`);
      stdout.write(
        `${formatDistribution("of which package-manager startup", summary.pkgStartup)}\n`,
      );
      stdout.write(
        `${formatDistribution("of which server listen after that", summary.serverListen)}\n`,
      );
      stdout.write(`${formatDistribution("whole case (total)", summary.wholeCase)}\n\n`);

      for (const field of IDENTITY_FIELDS) {
        stdout.write(`  identity ${field}: ${drift.distinct[field].join(", ")}\n`);
      }
      if (drift.hasDrift) {
        stdout.write(`\n${formatDriftWarning(drift, options.allowDrift)}\n`);
      }

      stdout.write(
        `\nR-B verdict ${tag}: max pre-UP=${fmt(rb.max)} vs threshold(${options.threshold}) x budget(${options.budgetMs}ms) = ${rb.boundary}ms -> ${rb.tripped ? "TRIPPED" : "ok"}\n\n`,
      );
    }
  }

  if (options.strict && anyFailed) return EXIT_STRICT;
  if (caseRecords.length === 0) return EXIT_NO_SAMPLES;
  if (
    options.strict &&
    analysis.cohorts.some((cohort) => cohortTripsStrict(cohort, options.allowDrift))
  ) {
    return EXIT_STRICT;
  }
  return EXIT_OK;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = await runCli(process.argv.slice(2));
}
