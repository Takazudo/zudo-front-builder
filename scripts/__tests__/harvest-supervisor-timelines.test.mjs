// Tests for scripts/harvest-supervisor-timelines.mjs.
//
// Drives the real CLI (runCli) against a stub `gh` shell script passed via
// --gh, so the test exercises the actual subprocess/argv contract rather
// than a JS-level mock of gh. The stub (shared with
// tests/unit/run-supervisor-watch.sh, #2930 -- see
// scripts/__tests__/fixtures/gh-stub.sh) dispatches on argv and reads its
// fixture data from files under a per-test tmp directory, whose path it
// learns via the GH_STUB_FIXTURES_DIR env var (inherited by the child
// process the same way a real `gh` invocation would inherit the shell's
// environment).
//
// Two fixtures with two different owners (#2931's addendum): the record line
// is zfb's own contract, so it comes from the hand-authored corpus
// (fixtures/supervisor-timeline-samples.txt); the job-log envelope around it
// is GitHub's, so it comes from a real captured REST body
// (fixtures/rest-job-log-capture.log) and is never hand-written here.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { cleanupFixtures, makeRun, readCalls, setupFixtures } from "./fixtures/gh-fixture-tree.mjs";
import { TIMELINE_SAMPLES } from "./fixtures/load-timeline-samples.mjs";
import { REST_JOB_LOG_CAPTURE, REST_LOG_PREFIX } from "./fixtures/load-rest-job-log-capture.mjs";
import { parseTimelineLine, parseTimelines } from "../supervisor-timeline-summary.mjs";
import * as harvester from "../harvest-supervisor-timelines.mjs";
import {
  DEFAULT_RETRY_DELAY_MS,
  DEFAULT_WINDOW_DAYS,
  EXIT_NO_RECORDS,
  EXIT_NO_RUNS,
  EXIT_OK,
  EXIT_PARTIAL,
  EXIT_USAGE,
  JOBS_PER_PAGE,
  buildJobLogArgs,
  buildRunJobsArgs,
  buildRunListArgs,
  parseCliArgs,
  resolveDefaultRetryDelayMs,
  runCli,
} from "../harvest-supervisor-timelines.mjs";

// Not invented: sliced off a record line in the captured REST body, so every
// synthetic log below is wrapped in bytes GitHub actually emitted. Since
// #2931 that is the runner's `<ISO timestamp>Z ` plus `pnpm -r`'s `. test: `
// label -- the `<job>\t<step>\t` prefix belonged to `gh run view --log`,
// the porcelain this harvester no longer calls.
const LOG_PREFIX = REST_LOG_PREFIX;

const UP_UP2_RECORD_LINE = `${LOG_PREFIX}${TIMELINE_SAMPLES.UP_UP2_LINE}`;

// A vitest code frame quoting the tag inside a string literal -- must be
// ignored, not parsed as a record (the summarizer's own TAG_PATTERN guard).
const CODE_FRAME_LINE = `${LOG_PREFIX}  913|       expect(timelineLines[0]).toContain("[supervisor-timeline] case=hidden");`;

// An R-A candidate: the harvester's per-run `failedRecords=<m>` counts
// records shaped like this one.
const FAILED_RECORD_LINE = `${LOG_PREFIX}${TIMELINE_SAMPLES.FAILED_LINE}`;

const JOB_LOG_WITH_RECORD = [
  `${LOG_PREFIX}##[section]Starting: Run tests`,
  UP_UP2_RECORD_LINE,
  CODE_FRAME_LINE,
  `${LOG_PREFIX}PASS scripts/__tests__/docs-dev-supervisor.test.mjs`,
].join("\n");

const JOB_LOG_NO_RECORDS = [
  `${LOG_PREFIX}##[section]Starting: Run tests`,
  `${LOG_PREFIX}PASS some-other.test.mjs`,
].join("\n");

// Doubles as runCli's options bag: `retryDelayMs: 0` keeps the retry path
// instant (every persistent-failure case below goes through it), and a
// pinned `now` keeps the default window deterministic in cases that are not
// about it.
function sink() {
  const outWrites = [];
  const errWrites = [];
  return {
    stdout: { write: (chunk) => outWrites.push(chunk) },
    stderr: { write: (chunk) => errWrites.push(chunk) },
    retryDelayMs: 0,
    now: new Date("2026-10-01T12:00:00Z"),
    out: () => outWrites.join(""),
    err: () => errWrites.join(""),
    outWriteCount: () => outWrites.length,
  };
}

afterEach(() => {
  delete process.env.GH_STUB_FIXTURES_DIR;
  cleanupFixtures();
});

describe("buildRunListArgs", () => {
  it("includes --workflow=health.yml, --created, --limit, and --json", () => {
    const args = buildRunListArgs({ since: "2026-01-01T00:00:00Z", limit: 50 });
    expect(args).toContain("--workflow=health.yml");
    const createdIndex = args.indexOf("--created");
    expect(createdIndex).toBeGreaterThan(-1);
    expect(args[createdIndex + 1]).toBe(">=2026-01-01T00:00:00Z");
    const limitIndex = args.indexOf("--limit");
    expect(args[limitIndex + 1]).toBe("50");
  });

  it("adds --branch only when given", () => {
    expect(buildRunListArgs({ since: "x", limit: 1 })).not.toContain("--branch");
    const args = buildRunListArgs({ since: "x", limit: 1, branch: "main" });
    expect(args).toContain("--branch");
    expect(args[args.indexOf("--branch") + 1]).toBe("main");
  });
});

describe("REST argv builders", () => {
  it("resolves owner/repo with gh's own placeholders, spending no extra call", () => {
    // gh expands {owner}/{repo} from the current repository (or GH_REPO)
    // locally -- verified 2026-09-07 with GH_DEBUG=api: one GET on the wire,
    // none to resolve the names. Anything that had to be looked up first
    // would put a third metered GET back into every run.
    expect(buildRunJobsArgs(42)).toEqual([
      "api",
      "repos/{owner}/{repo}/actions/runs/42/jobs?per_page=100",
    ]);
    expect(buildJobLogArgs(7)).toEqual(["api", "repos/{owner}/{repo}/actions/jobs/7/logs"]);
  });

  it("pins the jobs page size at the maximum, since the endpoint defaults to 30", () => {
    // A health.yml run already carries 10 jobs; the default page size leaves
    // no headroom, and the failure would be a silently missing health job.
    expect(JOBS_PER_PAGE).toBe(100);
    expect(buildRunJobsArgs(42, 2)).toEqual([
      "api",
      "repos/{owner}/{repo}/actions/runs/42/jobs?per_page=100&page=2",
    ]);
  });

  it("passes nothing that would stop gh following the log endpoint's 302", () => {
    // The job-logs endpoint answers 302 to a plaintext blob on a separate
    // (unmetered) host, and gh follows it and prints the blob -- verified
    // live 2026-09-07 with GH_DEBUG=api. A flag like `--include`, `-i` or
    // `-X` would surface the redirect (or change the method) instead, and
    // the harvester would then parse response headers as a job log, so the
    // argv must stay a bare path.
    expect(buildJobLogArgs(7).filter((arg) => arg.startsWith("-"))).toEqual([]);
    expect(buildRunJobsArgs(42).filter((arg) => arg.startsWith("-"))).toEqual([]);
  });
});

describe("parseCliArgs", () => {
  it("applies documented defaults with no flags: --since is a purely rolling window", () => {
    // 8 days, not 7: a run in progress at one Sunday harvest must still be
    // enumerated by the next one, so consecutive windows overlap by a day.
    expect(DEFAULT_WINDOW_DAYS).toBe(8);
    const options = parseCliArgs([], { now: new Date("2026-10-01T12:00:00Z") });
    expect(options.since).toBe("2026-09-23T12:00:00Z");
    expect(options.limit).toBe(200);
    expect(options.branch).toBeUndefined();
    expect(options.saveDir).toBeUndefined();
    expect(options.gh).toBe("gh");
  });

  it("has no identity epoch: the default window reaches back across the #2913 contract change (#2933)", () => {
    // The retired IDENTITY_CONTRACT_EPOCH ("2026-09-08T00:00:00Z") floored
    // this default; a contract change is now a version on the record itself,
    // so a "now" two days past that instant gets the full window, not a
    // floor -- and the constant is gone, not merely unused.
    const options = parseCliArgs([], { now: new Date("2026-09-10T00:00:00Z") });
    expect(options.since).toBe("2026-09-02T00:00:00Z");
    expect(Object.keys(harvester)).not.toContain("IDENTITY_CONTRACT_EPOCH");
  });

  it("an explicit --since overrides the computed default", () => {
    const options = parseCliArgs(["--since", "2020-01-01T00:00:00Z"], {
      now: new Date("2026-10-01T12:00:00Z"),
    });
    expect(options.since).toBe("2020-01-01T00:00:00Z");
  });

  it("uses the real clock when no now is injected", () => {
    const before = Date.now();
    const options = parseCliArgs([]);
    const windowMs = DEFAULT_WINDOW_DAYS * 24 * 60 * 60 * 1000;
    // `since` is trimmed to whole seconds, so allow that plus the call's own
    // duration.
    expect(Date.parse(options.since)).toBeGreaterThanOrEqual(before - windowMs - 1000);
    expect(Date.parse(options.since)).toBeLessThanOrEqual(Date.now() - windowMs);
  });

  it("parses all flags", () => {
    const options = parseCliArgs([
      "--since",
      "2026-01-01T00:00:00Z",
      "--limit",
      "5",
      "--branch",
      "main",
      "--save-dir",
      "/tmp/out",
      "--gh",
      "/usr/local/bin/gh",
    ]);
    expect(options).toEqual({
      since: "2026-01-01T00:00:00Z",
      limit: 5,
      branch: "main",
      saveDir: "/tmp/out",
      gh: "/usr/local/bin/gh",
    });
  });

  it("throws on an unknown flag", () => {
    expect(() => parseCliArgs(["--nope"])).toThrow(/unknown flag: --nope/);
  });

  it("throws when --limit is not a positive integer", () => {
    expect(() => parseCliArgs(["--limit", "abc"])).toThrow(/--limit requires a positive integer/);
    expect(() => parseCliArgs(["--limit", "0"])).toThrow(/--limit requires a positive integer/);
    expect(() => parseCliArgs(["--limit", "-1"])).toThrow(/--limit requires a positive integer/);
  });

  it("throws on an unexpected positional argument", () => {
    expect(() => parseCliArgs(["extra.log"])).toThrow(/unexpected positional argument/);
  });

  it("rejects an empty value for a string flag instead of silently dropping the flag", () => {
    expect(() => parseCliArgs(["--branch", ""])).toThrow(/--branch requires a value/);
    expect(() => parseCliArgs(["--save-dir", ""])).toThrow(/--save-dir requires a value/);
    expect(() => parseCliArgs(["--since", "--limit", "5"])).toThrow(/--since requires a value/);
  });
});

describe("resolveDefaultRetryDelayMs (#2932)", () => {
  const ENV_KEY = "HARVEST_RETRY_DELAY_MS";
  const original = process.env[ENV_KEY];

  afterEach(() => {
    if (original === undefined) delete process.env[ENV_KEY];
    else process.env[ENV_KEY] = original;
  });

  it("defaults to 2000 ms when the env var is unset, empty, or not a finite non-negative number", () => {
    delete process.env[ENV_KEY];
    expect(resolveDefaultRetryDelayMs()).toBe(2000);
    expect(DEFAULT_RETRY_DELAY_MS).toBe(2000);

    process.env[ENV_KEY] = "";
    expect(resolveDefaultRetryDelayMs()).toBe(DEFAULT_RETRY_DELAY_MS);

    process.env[ENV_KEY] = "not-a-number";
    expect(resolveDefaultRetryDelayMs()).toBe(DEFAULT_RETRY_DELAY_MS);

    process.env[ENV_KEY] = "-5";
    expect(resolveDefaultRetryDelayMs()).toBe(DEFAULT_RETRY_DELAY_MS);
  });

  it("honors a valid override, which is how the sh unit test avoids two real 2 s sleeps", () => {
    process.env[ENV_KEY] = "0";
    expect(resolveDefaultRetryDelayMs()).toBe(0);

    process.env[ENV_KEY] = "150";
    expect(resolveDefaultRetryDelayMs()).toBe(150);
  });
});

describe("runCli", () => {
  it("exit 0: harvests a completed run's health job log and emits round-trippable records", async () => {
    const runs = [makeRun({ databaseId: 1001 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: {
        1001: [
          { id: 5001, name: "health" },
          { id: 5002, name: "build" },
        ],
      },
      logsByJobId: { 5001: JOB_LOG_WITH_RECORD },
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--since", "2026-09-06T22:00:00Z", "--gh", stubPath], s);

    expect(code).toBe(EXIT_OK);
    expect(s.outWriteCount()).toBe(1); // buffered: a single stdout write
    const emittedLines = s.out().trimEnd().split("\n");
    expect(emittedLines).toHaveLength(1);
    const record = parseTimelineLine(emittedLines[0]);
    expect(record).not.toBeNull();
    expect(record.case).toBe("up+up2");
    expect(record.outcome).toBe("ok");

    expect(s.err()).toMatch(/run=1001 .* job=5001 lines=1/);
    expect(s.err()).toMatch(/run=1001 .* job=5001 lines=1 failedRecords=0/);
    expect(s.err()).toMatch(/runs=1 harvested=1 failed=0 records=1/);

    const calls = readCalls(dir);
    expect(
      calls.some((line) => line.startsWith("run list") && line.includes("--workflow=health.yml")),
    ).toBe(true);
    expect(calls.some((line) => line.includes("--created >=2026-09-06T22:00:00Z"))).toBe(true);
    // Exactly the two REST calls, one metered GET each -- and never the
    // `gh run view` porcelain, whose per-run fan-out (~7 GETs, measured
    // 2026-09-07) is what #2931 removed.
    expect(calls).toContain("api repos/{owner}/{repo}/actions/runs/1001/jobs?per_page=100");
    expect(calls).toContain("api repos/{owner}/{repo}/actions/jobs/5001/logs");
    expect(calls.filter((line) => line.startsWith("api "))).toHaveLength(2);
    expect(calls.some((line) => line.startsWith("run view"))).toBe(false);
  });

  it("--save-dir writes each fetched job log to <dir>/run-<id>-job-<jobId>.log", async () => {
    const runs = [makeRun({ databaseId: 1001 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 1001: [{ id: 5001, name: "health" }] },
      logsByJobId: { 5001: JOB_LOG_WITH_RECORD },
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;
    const saveDir = join(dir, "saved");

    const s = sink();
    const code = await runCli(["--gh", stubPath, "--save-dir", saveDir], s);

    expect(code).toBe(EXIT_OK);
    const saved = readFileSync(join(saveDir, "run-1001-job-5001.log"), "utf8");
    expect(saved).toBe(JOB_LOG_WITH_RECORD);
  });

  it("manifest's failedRecords=<m> counts parsed records with outcome=failed, an R-A candidate", async () => {
    const jobLog = [UP_UP2_RECORD_LINE, FAILED_RECORD_LINE].join("\n");
    const runs = [makeRun({ databaseId: 1009 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 1009: [{ id: 5009, name: "health" }] },
      logsByJobId: { 5009: jobLog },
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_OK);
    // A run-level manifest failure count is untouched by a record-level one:
    // the run itself harvested successfully, it just carries an R-A record.
    expect(s.err()).toMatch(/run=1009 .* job=5009 lines=2 failedRecords=1/);
    expect(s.err()).toMatch(/runs=1 harvested=1 failed=0 records=2/);
  });

  it("exit 1: a green health job whose log carries zero records is the emitter going silent", async () => {
    const runs = [makeRun({ databaseId: 2001 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 2001: [{ id: 6001, name: "health", conclusion: "success" }] },
      logsByJobId: { 6001: JOB_LOG_NO_RECORDS },
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_NO_RECORDS);
    expect(s.out()).toBe("");
    expect(s.err()).toMatch(/run=2001 .* job=6001 lines=0 failedRecords=0/);
    expect(s.err()).toMatch(/runs=1 harvested=1 failed=0 records=0/);
  });

  it("skips a health job that went red before the test step (no records, conclusion != success)", async () => {
    const runs = [
      makeRun({ databaseId: 2002, conclusion: "failure" }),
      makeRun({ databaseId: 2003, conclusion: "cancelled" }),
    ];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: {
        2002: [{ id: 6002, name: "health", conclusion: "failure" }],
        2003: [{ id: 6003, name: "health", conclusion: "cancelled" }],
      },
      logsByJobId: { 6002: JOB_LOG_NO_RECORDS, 6003: JOB_LOG_NO_RECORDS },
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    // A red-at-clippy health job has a log and no records; that is not the
    // emitter vanishing, so it is neither harvested nor a no-records exit.
    expect(code).toBe(EXIT_NO_RUNS);
    expect(s.err()).toMatch(/run=2002 .* job=6002 skipped=no-records-health-failure/);
    expect(s.err()).toMatch(/run=2003 .* job=6003 skipped=no-records-health-cancelled/);
    expect(s.err()).toMatch(/runs=2 harvested=0 failed=0 records=0/);
  });

  it("a red health job that still emitted records (a supervisor failure) is harvested, not skipped", async () => {
    const runs = [makeRun({ databaseId: 2004, conclusion: "failure" })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 2004: [{ id: 6004, name: "health", conclusion: "failure" }] },
      logsByJobId: { 6004: FAILED_RECORD_LINE },
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_OK);
    expect(s.err()).toMatch(/run=2004 .* job=6004 lines=1 failedRecords=1/);
  });

  it("exit 4: an empty run list is a quiet window, distinct from a silent emitter", async () => {
    const { dir, stubPath } = setupFixtures({ runs: [] });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_NO_RUNS);
    expect(s.out()).toBe("");
    expect(s.err()).toMatch(/runs=0 harvested=0 failed=0 records=0/);
  });

  it("prints the effective window first, and warns when --since lies in the future", async () => {
    const { dir, stubPath } = setupFixtures({ runs: [] });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    // An explicit --since ahead of "now" enumerates nothing and must say so
    // rather than read as a quiet week.
    const early = sink();
    await runCli(["--gh", stubPath, "--since", "2026-09-08T00:00:00Z"], {
      ...early,
      now: new Date("2026-09-07T00:00:00Z"),
    });
    const earlyLines = early.err().trimEnd().split("\n");
    expect(earlyLines[0]).toBe("window: since=2026-09-08T00:00:00Z limit=200 branch=all");
    expect(earlyLines[1]).toMatch(/^warning: --since 2026-09-08T00:00:00Z lies in the future/);

    const later = sink();
    await runCli(["--gh", stubPath, "--branch", "main", "--limit", "7"], {
      ...later,
      now: new Date("2026-10-01T12:00:00Z"),
    });
    expect(later.err()).toMatch(/^window: since=2026-09-23T12:00:00Z limit=7 branch=main$/m);
    expect(later.err()).not.toMatch(/warning:/);
  });

  it("retries a transient gh failure once before giving up on the run", async () => {
    const runs = [makeRun({ databaseId: 1011 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 1011: [{ id: 5011, name: "health" }] },
      logsByJobId: { 5011: JOB_LOG_WITH_RECORD },
      transientlyFailingJobIds: [5011],
      failRunListOnce: true,
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_OK);
    expect(s.err()).toMatch(/run=1011 .* job=5011 lines=1 failedRecords=0/);
    const calls = readCalls(dir);
    expect(calls.filter((line) => line.startsWith("run list"))).toHaveLength(2);
    expect(
      calls.filter((line) => line === "api repos/{owner}/{repo}/actions/jobs/5011/logs"),
    ).toHaveLength(2);
  });

  it("wires HARVEST_RETRY_DELAY_MS through to the real retry pause, not just the sink's override (#2932)", async () => {
    // Every other test pins retryDelayMs via sink() so the pause never
    // reaches this suite's timeout budget. This one instead drives the
    // production default-parameter path -- retryDelayMs omitted from the
    // options bag entirely -- so a regression that stops reading the env
    // var (e.g. an accidental revert to the bare DEFAULT_RETRY_DELAY_MS
    // default) would show up as a ~2 s slowdown here, not silently.
    const runs = [makeRun({ databaseId: 1019 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 1019: [{ id: 5019, name: "health" }] },
      logsByJobId: { 5019: JOB_LOG_WITH_RECORD },
      transientlyFailingJobIds: [5019],
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;
    process.env.HARVEST_RETRY_DELAY_MS = "0";
    try {
      const s = sink();
      delete s.retryDelayMs; // force the default-parameter path, not the sink's override

      const started = Date.now();
      const code = await runCli(["--gh", stubPath], s);
      const elapsedMs = Date.now() - started;

      expect(code).toBe(EXIT_OK);
      expect(s.err()).toMatch(/run=1019 .* job=5019 lines=1 failedRecords=0/);
      // A generous ceiling well under the real 2000 ms default -- this
      // guards against the env var being ignored, not against ordinary
      // subprocess jitter.
      expect(elapsedMs).toBeLessThan(1500);
    } finally {
      delete process.env.HARVEST_RETRY_DELAY_MS;
    }
  });

  it("skips a run whose status is not completed, and a completed run with no health job", async () => {
    const runs = [
      makeRun({ databaseId: 1002, status: "in_progress" }),
      makeRun({ databaseId: 1003, status: "completed" }),
    ];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 1003: [{ id: 5010, name: "docs" }] },
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_NO_RUNS);
    expect(s.err()).toMatch(/run=1002 .* job=none skipped=status:in_progress/);
    expect(s.err()).toMatch(/run=1003 .* job=none skipped=no-health-job/);
    expect(s.err()).toMatch(/runs=2 harvested=0 failed=0 records=0/);

    // Neither skip reason should have triggered a job-log fetch.
    const calls = readCalls(dir);
    expect(calls.some((line) => line.includes("/actions/jobs/"))).toBe(false);
    // The in-progress run must never even have its jobs resolved.
    expect(calls.some((line) => line.includes("/actions/runs/1002/jobs"))).toBe(false);
  });

  it("exit 3: a failing per-job log fetch is partial, even though another run harvested a record", async () => {
    const runs = [makeRun({ databaseId: 1001 }), makeRun({ databaseId: 1004 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: {
        1001: [{ id: 5001, name: "health" }],
        1004: [{ id: 5004, name: "health" }],
      },
      logsByJobId: { 5001: JOB_LOG_WITH_RECORD },
      failingJobIds: [5004],
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_PARTIAL);
    // Buffered stdout still carries the good record from the run that succeeded --
    // a partial harvest is not the same as a corrupted one.
    expect(s.outWriteCount()).toBe(1);
    expect(s.out().trimEnd().split("\n")).toHaveLength(1);
    expect(s.err()).toMatch(/run=1004 .* job=5004 error=/);
    expect(s.err()).toMatch(/runs=2 harvested=1 failed=1 records=1/);

    // The manifest is documented as one line per run (plus the leading
    // window line and the final summary): a gh subprocess error (Node's
    // execFile embeds the child's own stderr, often multi-line) must never
    // fragment the run=1004 manifest entry across physical lines.
    const errLines = s.err().trimEnd().split("\n");
    expect(errLines).toHaveLength(runs.length + 2);
    const run1004Line = errLines.find((line) => line.startsWith("run=1004"));
    expect(run1004Line).toMatch(
      /^run=1004 .*error=.*\| gh-stub: simulated failure fetching job 5004$/,
    );
  });

  it("exit 3: a log with a malformed record is a failed run that contributes no records", async () => {
    const malformedLog = [
      UP_UP2_RECORD_LINE,
      `${LOG_PREFIX}[supervisor-timeline] case=up+boom outcome=ok total=oops`,
    ].join("\n");
    const runs = [makeRun({ databaseId: 1005 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 1005: [{ id: 5005, name: "health" }] },
      logsByJobId: { 5005: malformedLog },
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_PARTIAL);
    // The well-formed record ahead of the malformed line must not leak into
    // stdout while the manifest reports the run as failed.
    expect(s.out()).toBe("");
    expect(s.err()).toMatch(/run=1005 .* job=5005 error=malformed \[supervisor-timeline\] line/);
    expect(s.err()).toMatch(/runs=1 harvested=0 failed=1 records=0/);
  });

  it("skips a health job that never started instead of counting its missing log as a failure", async () => {
    const runs = [
      makeRun({ databaseId: 1006, conclusion: "cancelled" }),
      makeRun({ databaseId: 1007, conclusion: "skipped" }),
    ];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: {
        1006: [{ id: 5006, name: "health", conclusion: "cancelled", steps: [] }],
        1007: [{ id: 5007, name: "health", conclusion: "skipped", steps: [] }],
      },
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_NO_RUNS);
    expect(s.err()).toMatch(/run=1006 .* job=5006 skipped=health-job-never-started/);
    expect(s.err()).toMatch(/run=1007 .* job=5007 skipped=health-job-never-started/);
    expect(s.err()).toMatch(/runs=2 harvested=0 failed=0 records=0/);
    expect(readCalls(dir).some((line) => line.includes("/actions/jobs/"))).toBe(false);
  });

  it("warns when gh run list returned exactly --limit runs, since the window may be capped", async () => {
    const runs = [makeRun({ databaseId: 1001 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 1001: [{ id: 5001, name: "health" }] },
      logsByJobId: { 5001: JOB_LOG_WITH_RECORD },
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    const capped = sink();
    expect(await runCli(["--gh", stubPath, "--limit", "1"], capped)).toBe(EXIT_OK);
    expect(capped.err()).toMatch(/^notice: gh run list returned 1 run\(s\), the --limit cap/m);

    const uncapped = sink();
    expect(await runCli(["--gh", stubPath, "--limit", "2"], uncapped)).toBe(EXIT_OK);
    expect(uncapped.err()).not.toMatch(/notice:/);
  });

  it("exit 64: gh run list itself failing is a usage error, not partial or no-records", async () => {
    const { dir, stubPath } = setupFixtures({ runs: [], failRunList: true });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_USAGE);
    expect(s.err()).toMatch(/usage error: gh run list failed/);
    expect(s.outWriteCount()).toBe(0); // nothing buffered yet -- fails before any run is processed
  });

  it("parses a non-zero record count out of the REAL captured REST body", async () => {
    // The addendum's positive assertion. A hand-guessed envelope would leave
    // parseTimelines finding nothing: every run would report lines=0, the
    // watch would file them all as silent runs, and the weekly verdict would
    // be a confident `no-data` -- which is excluded from issue filing, so the
    // lane would go dark and say nothing. Asserting "no error" cannot catch
    // that; only asserting records > 0 against real bytes can.
    const runs = [makeRun({ databaseId: 1012 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 1012: [{ id: 5012, name: "health", conclusion: "success" }] },
      logsByJobId: { 5012: REST_JOB_LOG_CAPTURE },
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_OK);
    // An exact count, not "> 0": an empty stdout would still split into one
    // (empty) element, which is exactly the vacuous pass this test exists to
    // rule out. The trimmed capture carries three records.
    const emitted = s.out().trimEnd().split("\n");
    expect(emitted).toHaveLength(3);
    expect(s.err()).toMatch(/run=1012 .* job=5012 lines=3 failedRecords=0/);

    // Semantically identical to what the summarizer gets from the capture
    // itself -- the acceptance bar is the parsed records, not the raw bytes
    // (the REST body carries no job/step prefix, so `raw` legitimately
    // differs from what the old porcelain produced).
    const fields = (records) =>
      records.map((record) => ({
        case: record.case,
        outcome: record.outcome,
        total: record.total,
        identity: record.identity,
        marks: record.marks,
      }));
    expect(fields(emitted.map((line) => parseTimelineLine(line)))).toEqual(
      fields(parseTimelines(REST_JOB_LOG_CAPTURE)),
    );
  });

  it("the captured body's own quirks (BOM, timestamp prefix) survive --save-dir verbatim", async () => {
    const runs = [makeRun({ databaseId: 1013 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 1013: [{ id: 5013, name: "health" }] },
      logsByJobId: { 5013: REST_JOB_LOG_CAPTURE },
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;
    const saveDir = join(dir, "saved");

    const s = sink();
    expect(await runCli(["--gh", stubPath, "--save-dir", saveDir], s)).toBe(EXIT_OK);

    // Pass B of the weekly watch re-summarizes these files, so what lands on
    // disk must be the endpoint's bytes untouched -- BOM included.
    const saved = readFileSync(join(saveDir, "run-1013-job-5013.log"), "utf8");
    expect(saved).toBe(REST_JOB_LOG_CAPTURE);
    expect(saved.charCodeAt(0)).toBe(0xfeff);
    expect(parseTimelines(saved).length).toBeGreaterThan(0);
  });

  it("walks a second jobs page only when health was not on the first", async () => {
    const runs = [makeRun({ databaseId: 1014 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 1014: [{ id: 5100, name: "build" }] },
      jobsTotalCountById: { 1014: 2 },
      extraJobPagesById: { 1014: { 2: [{ id: 5014, name: "health" }] } },
      logsByJobId: { 5014: JOB_LOG_WITH_RECORD },
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    const s = sink();
    expect(await runCli(["--gh", stubPath], s)).toBe(EXIT_OK);
    expect(s.err()).toMatch(/run=1014 .* job=5014 lines=1 failedRecords=0/);

    const calls = readCalls(dir);
    expect(calls).toContain("api repos/{owner}/{repo}/actions/runs/1014/jobs?per_page=100&page=2");
  });

  it("stops at one jobs page when total_count is covered, so the common run costs one GET", async () => {
    const runs = [makeRun({ databaseId: 1015 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 1015: [{ id: 5015, name: "docs" }] },
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    const s = sink();
    expect(await runCli(["--gh", stubPath], s)).toBe(EXIT_NO_RUNS);
    expect(s.err()).toMatch(/run=1015 .* job=none skipped=no-health-job/);
    expect(readCalls(dir).filter((line) => line.includes("/jobs?"))).toHaveLength(1);
  });

  it("exit 3: an API error envelope on the log endpoint is a failed run, never lines=0", async () => {
    // gh exits non-zero on a 4xx, so this shape only reaches the parser if
    // something between here and GitHub returns the error body with a
    // success status. Treating it as a log would report a silent emitter --
    // the one lie this lane must not tell.
    const runs = [makeRun({ databaseId: 1016 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 1016: [{ id: 5016, name: "health", conclusion: "success" }] },
      logsByJobId: {
        5016: JSON.stringify({ message: "Not Found", status: "404" }),
      },
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_PARTIAL);
    expect(s.err()).toMatch(
      /run=1016 .* job=5016 error=job 5016 log fetch returned an API error envelope: Not Found/,
    );
    expect(s.err()).not.toMatch(/lines=0/);
  });

  it("a log that merely opens with a brace is still a log, not an error envelope", async () => {
    const runs = [makeRun({ databaseId: 1017 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 1017: [{ id: 5017, name: "health" }] },
      logsByJobId: { 5017: `{ not json after all\n${UP_UP2_RECORD_LINE}` },
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    const s = sink();
    expect(await runCli(["--gh", stubPath], s)).toBe(EXIT_OK);
    expect(s.err()).toMatch(/run=1017 .* job=5017 lines=1 failedRecords=0/);
  });

  it("exit 3: a missing/expired log 404s, and that stays an error= line rather than a crash", async () => {
    // A `health` job whose log GitHub no longer has (retention expired, or
    // the log was deleted) still has a started-looking job object, so the
    // harvester does fetch it. gh's real answer -- error body on stdout, a
    // "gh: Not Found (HTTP 404)" line on stderr, exit 1 -- must land as one
    // manifest line and a partial harvest, never as a thrown stack.
    const runs = [makeRun({ databaseId: 1018 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 1018: [{ id: 5018, name: "health", steps: [{ name: "Set up job" }] }] },
      notFoundJobIds: [5018],
    });
    process.env.GH_STUB_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_PARTIAL);
    expect(s.err()).toMatch(/run=1018 .* job=5018 error=.*gh: Not Found \(HTTP 404\)/);
    expect(s.err()).toMatch(/runs=1 harvested=0 failed=1 records=0/);
    // One line per run stays one line, even though gh printed a JSON body
    // and a message across two streams.
    expect(s.err().trimEnd().split("\n")).toHaveLength(3);
  });

  it("exit 64: an unknown flag never invokes gh at all", async () => {
    const s = sink();
    const code = await runCli(["--nope"], s);
    expect(code).toBe(EXIT_USAGE);
    expect(s.err()).toMatch(/usage error: unknown flag: --nope/);
  });
});
