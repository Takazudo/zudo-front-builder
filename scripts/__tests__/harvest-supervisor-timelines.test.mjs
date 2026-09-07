// Tests for scripts/harvest-supervisor-timelines.mjs.
//
// Drives the real CLI (runCli) against a stub `gh` shell script passed via
// --gh, so the test exercises the actual subprocess/argv contract rather
// than a JS-level mock of gh. The stub dispatches on argv and reads its
// fixture data from files under a per-test tmp directory, whose path it
// learns via the HARVEST_TEST_FIXTURES_DIR env var (inherited by the child
// process the same way a real `gh` invocation would inherit the shell's
// environment).

import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { parseTimelineLine } from "../supervisor-timeline-summary.mjs";
import {
  DEFAULT_WINDOW_DAYS,
  EXIT_NO_RECORDS,
  EXIT_NO_RUNS,
  EXIT_OK,
  EXIT_PARTIAL,
  EXIT_USAGE,
  IDENTITY_CONTRACT_EPOCH,
  buildRunListArgs,
  parseCliArgs,
  runCli,
} from "../harvest-supervisor-timelines.mjs";

// `<name>.fail` fails every time; `<name>.fail-once` is consumed by the first
// failure, so the harvester's single retry then sees the real fixture.
const GH_STUB = `#!/usr/bin/env bash
set -euo pipefail
FIXDIR="\${HARVEST_TEST_FIXTURES_DIR:?HARVEST_TEST_FIXTURES_DIR not set}"
printf '%s\\n' "$*" >> "$FIXDIR/calls.log"

if [ "$1" = "run" ] && [ "$2" = "list" ]; then
  if [ -f "$FIXDIR/run-list.fail" ]; then
    echo "gh-stub: simulated run list failure" >&2
    exit 1
  fi
  if [ -f "$FIXDIR/run-list.fail-once" ]; then
    rm "$FIXDIR/run-list.fail-once"
    echo "gh-stub: simulated transient run list failure" >&2
    exit 1
  fi
  cat "$FIXDIR/run-list.json"
  exit 0
fi

if [ "$1" = "run" ] && [ "$2" = "view" ]; then
  if [ "$3" = "--job" ]; then
    jobId="$4"
    if [ -f "$FIXDIR/job-$jobId.fail" ]; then
      echo "gh-stub: simulated failure fetching job $jobId" >&2
      exit 1
    fi
    if [ -f "$FIXDIR/job-$jobId.fail-once" ]; then
      rm "$FIXDIR/job-$jobId.fail-once"
      echo "gh-stub: simulated transient failure fetching job $jobId" >&2
      exit 1
    fi
    cat "$FIXDIR/job-$jobId.log"
    exit 0
  else
    runId="$3"
    cat "$FIXDIR/jobs-$runId.json"
    exit 0
  fi
fi

echo "gh-stub: unhandled invocation: $*" >&2
exit 1
`;

// Mirrors the real job-log shape quoted in the issue: gh's --log output
// prefixes every line with "<job>\t<step>\t<timestamp> " ahead of whatever
// the step actually printed.
const LOG_PREFIX = "health\tUNKNOWN STEP\t2026-09-06T23:06:25.83Z ";

const UP_UP2_RECORD_LINE =
  `${LOG_PREFIX}. test: [supervisor-timeline] case=up+up2 outcome=ok total=357 runner=pnpm ` +
  "zudoDoc=5.15.0 runParallel=sha256:646f90cc300185cb fixtureShape=sha256:2d146d48587c00f5 " +
  "env=sha256:ed62f5285936a0ca first-stdout-byte=233 first-up-line=292";

// A vitest code frame quoting the tag inside a string literal -- must be
// ignored, not parsed as a record (the summarizer's own TAG_PATTERN guard).
const CODE_FRAME_LINE = `${LOG_PREFIX}  913|       expect(timelineLines[0]).toContain("[supervisor-timeline] case=hidden");`;

// An R-A candidate: the harvester's per-run `failed=<m>` counts records
// shaped like this one.
const FAILED_RECORD_LINE =
  `${LOG_PREFIX}. test: [supervisor-timeline] case=up+boom outcome=failed total=10041 runner=pnpm ` +
  "zudoDoc=5.15.0 runParallel=sha256:646f90cc300185cb fixtureShape=sha256:2d146d48587c00f5 " +
  "env=sha256:ed62f5285936a0ca first-stdout-byte=233";

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

function makeRun(overrides = {}) {
  return {
    databaseId: 1000,
    headBranch: "main",
    headSha: "0123456789abcdef0123456789abcdef01234567",
    conclusion: "success",
    status: "completed",
    createdAt: "2026-09-07T00:00:00Z",
    event: "push",
    attempt: 1,
    url: "https://github.com/Takazudo/zudo-front-builder/actions/runs/1000",
    ...overrides,
  };
}

let activeDirs = [];

function setupFixtures({
  runs,
  jobsById = {},
  logsByJobId = {},
  failingJobIds = [],
  transientlyFailingJobIds = [],
  failRunList = false,
  failRunListOnce = false,
}) {
  const dir = mkdtempSync(join(tmpdir(), "harvest-supervisor-timelines-test-"));
  activeDirs.push(dir);

  const stubPath = join(dir, "gh-stub.sh");
  writeFileSync(stubPath, GH_STUB);
  chmodSync(stubPath, 0o755);

  writeFileSync(join(dir, "run-list.json"), JSON.stringify(runs));
  if (failRunList) writeFileSync(join(dir, "run-list.fail"), "");
  if (failRunListOnce) writeFileSync(join(dir, "run-list.fail-once"), "");

  for (const [runId, jobs] of Object.entries(jobsById)) {
    writeFileSync(join(dir, `jobs-${runId}.json`), JSON.stringify({ jobs }));
  }
  for (const [jobId, log] of Object.entries(logsByJobId)) {
    writeFileSync(join(dir, `job-${jobId}.log`), log);
  }
  for (const jobId of failingJobIds) {
    writeFileSync(join(dir, `job-${jobId}.fail`), "");
  }
  for (const jobId of transientlyFailingJobIds) {
    writeFileSync(join(dir, `job-${jobId}.fail-once`), "");
  }

  return { dir, stubPath };
}

// Doubles as runCli's options bag: `retryDelayMs: 0` keeps the retry path
// instant (every persistent-failure case below goes through it), and a
// pinned `now` well past the identity epoch keeps the default window -- and
// the future-`--since` warning -- out of cases that are not about them.
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

function readCalls(dir) {
  try {
    return readFileSync(join(dir, "calls.log"), "utf8").trim().split("\n");
  } catch {
    return [];
  }
}

afterEach(() => {
  delete process.env.HARVEST_TEST_FIXTURES_DIR;
  for (const dir of activeDirs) rmSync(dir, { recursive: true, force: true });
  activeDirs = [];
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

describe("parseCliArgs", () => {
  it("applies documented defaults with no flags: --since floors at the identity epoch", () => {
    // "now" sits within the window of the epoch, so the floor -- not the
    // rolling window -- decides the default.
    const options = parseCliArgs([], { now: new Date("2026-09-10T00:00:00Z") });
    expect(options.since).toBe(IDENTITY_CONTRACT_EPOCH);
    expect(options.limit).toBe(200);
    expect(options.branch).toBeUndefined();
    expect(options.saveDir).toBeUndefined();
    expect(options.gh).toBe("gh");
  });

  it("defaults --since to a rolling window (one weekly cadence plus a day of overlap) once past the epoch", () => {
    // 8 days, not 7: a run in progress at one Sunday harvest must still be
    // enumerated by the next one, so consecutive windows overlap by a day.
    expect(DEFAULT_WINDOW_DAYS).toBe(8);
    const options = parseCliArgs([], { now: new Date("2026-10-01T12:00:00Z") });
    expect(options.since).toBe("2026-09-23T12:00:00Z");
  });

  it("an explicit --since overrides the computed default", () => {
    const options = parseCliArgs(["--since", "2020-01-01T00:00:00Z"], {
      now: new Date("2026-10-01T12:00:00Z"),
    });
    expect(options.since).toBe("2020-01-01T00:00:00Z");
  });

  it("uses the real clock when no now is injected", () => {
    const options = parseCliArgs([]);
    // Whatever "now" really is, the default can never predate the epoch.
    expect(Date.parse(options.since)).toBeGreaterThanOrEqual(Date.parse(IDENTITY_CONTRACT_EPOCH));
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

describe("runCli", () => {
  it("exit 0: harvests a completed run's health job log and emits round-trippable records", async () => {
    const runs = [makeRun({ databaseId: 1001 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: {
        1001: [
          { databaseId: 5001, name: "health" },
          { databaseId: 5002, name: "build" },
        ],
      },
      logsByJobId: { 5001: JOB_LOG_WITH_RECORD },
    });
    process.env.HARVEST_TEST_FIXTURES_DIR = dir;

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
    expect(s.err()).toMatch(/run=1001 .* job=5001 lines=1 failed=0/);
    expect(s.err()).toMatch(/runs=1 harvested=1 failed=0 records=1/);

    const calls = readCalls(dir);
    expect(
      calls.some((line) => line.startsWith("run list") && line.includes("--workflow=health.yml")),
    ).toBe(true);
    expect(calls.some((line) => line.includes("--created >=2026-09-06T22:00:00Z"))).toBe(true);
    expect(calls.some((line) => line === "run view 1001 --json jobs")).toBe(true);
    expect(calls.some((line) => line === "run view --job 5001 --log")).toBe(true);
    // Per-job fetch only: never the whole run's log.
    expect(calls.some((line) => line.startsWith("run view 1001 --log"))).toBe(false);
  });

  it("--save-dir writes each fetched job log to <dir>/run-<id>-job-<jobId>.log", async () => {
    const runs = [makeRun({ databaseId: 1001 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 1001: [{ databaseId: 5001, name: "health" }] },
      logsByJobId: { 5001: JOB_LOG_WITH_RECORD },
    });
    process.env.HARVEST_TEST_FIXTURES_DIR = dir;
    const saveDir = join(dir, "saved");

    const s = sink();
    const code = await runCli(["--gh", stubPath, "--save-dir", saveDir], s);

    expect(code).toBe(EXIT_OK);
    const saved = readFileSync(join(saveDir, "run-1001-job-5001.log"), "utf8");
    expect(saved).toBe(JOB_LOG_WITH_RECORD);
  });

  it("manifest's failed=<m> counts parsed records with outcome=failed, an R-A candidate", async () => {
    const jobLog = [UP_UP2_RECORD_LINE, FAILED_RECORD_LINE].join("\n");
    const runs = [makeRun({ databaseId: 1009 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 1009: [{ databaseId: 5009, name: "health" }] },
      logsByJobId: { 5009: jobLog },
    });
    process.env.HARVEST_TEST_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_OK);
    // A run-level manifest failure count is untouched by a record-level one:
    // the run itself harvested successfully, it just carries an R-A record.
    expect(s.err()).toMatch(/run=1009 .* job=5009 lines=2 failed=1/);
    expect(s.err()).toMatch(/runs=1 harvested=1 failed=0 records=2/);
  });

  it("exit 1: a green health job whose log carries zero records is the emitter going silent", async () => {
    const runs = [makeRun({ databaseId: 2001 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 2001: [{ databaseId: 6001, name: "health", conclusion: "success" }] },
      logsByJobId: { 6001: JOB_LOG_NO_RECORDS },
    });
    process.env.HARVEST_TEST_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_NO_RECORDS);
    expect(s.out()).toBe("");
    expect(s.err()).toMatch(/run=2001 .* job=6001 lines=0 failed=0/);
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
        2002: [{ databaseId: 6002, name: "health", conclusion: "failure" }],
        2003: [{ databaseId: 6003, name: "health", conclusion: "cancelled" }],
      },
      logsByJobId: { 6002: JOB_LOG_NO_RECORDS, 6003: JOB_LOG_NO_RECORDS },
    });
    process.env.HARVEST_TEST_FIXTURES_DIR = dir;

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
      jobsById: { 2004: [{ databaseId: 6004, name: "health", conclusion: "failure" }] },
      logsByJobId: { 6004: FAILED_RECORD_LINE },
    });
    process.env.HARVEST_TEST_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_OK);
    expect(s.err()).toMatch(/run=2004 .* job=6004 lines=1 failed=1/);
  });

  it("exit 4: an empty run list is a quiet window, distinct from a silent emitter", async () => {
    const { dir, stubPath } = setupFixtures({ runs: [] });
    process.env.HARVEST_TEST_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_NO_RUNS);
    expect(s.out()).toBe("");
    expect(s.err()).toMatch(/runs=0 harvested=0 failed=0 records=0/);
  });

  it("prints the effective window first, and warns when --since lies in the future", async () => {
    const { dir, stubPath } = setupFixtures({ runs: [] });
    process.env.HARVEST_TEST_FIXTURES_DIR = dir;

    // A pre-epoch "now": the default --since is floored at the epoch and so
    // sits in the future, which enumerates nothing and must say so.
    const early = sink();
    await runCli(["--gh", stubPath], { ...early, now: new Date("2026-09-07T00:00:00Z") });
    const earlyLines = early.err().trimEnd().split("\n");
    expect(earlyLines[0]).toBe(`window: since=${IDENTITY_CONTRACT_EPOCH} limit=200 branch=all`);
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
      jobsById: { 1011: [{ databaseId: 5011, name: "health" }] },
      logsByJobId: { 5011: JOB_LOG_WITH_RECORD },
      transientlyFailingJobIds: [5011],
      failRunListOnce: true,
    });
    process.env.HARVEST_TEST_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_OK);
    expect(s.err()).toMatch(/run=1011 .* job=5011 lines=1 failed=0/);
    const calls = readCalls(dir);
    expect(calls.filter((line) => line.startsWith("run list"))).toHaveLength(2);
    expect(calls.filter((line) => line === "run view --job 5011 --log")).toHaveLength(2);
  });

  it("skips a run whose status is not completed, and a completed run with no health job", async () => {
    const runs = [
      makeRun({ databaseId: 1002, status: "in_progress" }),
      makeRun({ databaseId: 1003, status: "completed" }),
    ];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 1003: [{ databaseId: 5010, name: "docs" }] },
    });
    process.env.HARVEST_TEST_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_NO_RUNS);
    expect(s.err()).toMatch(/run=1002 .* job=none skipped=status:in_progress/);
    expect(s.err()).toMatch(/run=1003 .* job=none skipped=no-health-job/);
    expect(s.err()).toMatch(/runs=2 harvested=0 failed=0 records=0/);

    // Neither skip reason should have triggered a --job log fetch.
    const calls = readCalls(dir);
    expect(calls.some((line) => line.startsWith("run view --job"))).toBe(false);
    // The in-progress run must never even have its jobs resolved.
    expect(calls.some((line) => line === "run view 1002 --json jobs")).toBe(false);
  });

  it("exit 3: a failing per-job log fetch is partial, even though another run harvested a record", async () => {
    const runs = [makeRun({ databaseId: 1001 }), makeRun({ databaseId: 1004 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: {
        1001: [{ databaseId: 5001, name: "health" }],
        1004: [{ databaseId: 5004, name: "health" }],
      },
      logsByJobId: { 5001: JOB_LOG_WITH_RECORD },
      failingJobIds: [5004],
    });
    process.env.HARVEST_TEST_FIXTURES_DIR = dir;

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
      `${LOG_PREFIX}. test: [supervisor-timeline] case=up+boom outcome=ok total=oops`,
    ].join("\n");
    const runs = [makeRun({ databaseId: 1005 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 1005: [{ databaseId: 5005, name: "health" }] },
      logsByJobId: { 5005: malformedLog },
    });
    process.env.HARVEST_TEST_FIXTURES_DIR = dir;

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
        1006: [{ databaseId: 5006, name: "health", conclusion: "cancelled", steps: [] }],
        1007: [{ databaseId: 5007, name: "health", conclusion: "skipped", steps: [] }],
      },
    });
    process.env.HARVEST_TEST_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_NO_RUNS);
    expect(s.err()).toMatch(/run=1006 .* job=5006 skipped=health-job-never-started/);
    expect(s.err()).toMatch(/run=1007 .* job=5007 skipped=health-job-never-started/);
    expect(s.err()).toMatch(/runs=2 harvested=0 failed=0 records=0/);
    expect(readCalls(dir).some((line) => line.startsWith("run view --job"))).toBe(false);
  });

  it("warns when gh run list returned exactly --limit runs, since the window may be capped", async () => {
    const runs = [makeRun({ databaseId: 1001 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 1001: [{ databaseId: 5001, name: "health" }] },
      logsByJobId: { 5001: JOB_LOG_WITH_RECORD },
    });
    process.env.HARVEST_TEST_FIXTURES_DIR = dir;

    const capped = sink();
    expect(await runCli(["--gh", stubPath, "--limit", "1"], capped)).toBe(EXIT_OK);
    expect(capped.err()).toMatch(/^notice: gh run list returned 1 run\(s\), the --limit cap/m);

    const uncapped = sink();
    expect(await runCli(["--gh", stubPath, "--limit", "2"], uncapped)).toBe(EXIT_OK);
    expect(uncapped.err()).not.toMatch(/notice:/);
  });

  it("exit 64: gh run list itself failing is a usage error, not partial or no-records", async () => {
    const { dir, stubPath } = setupFixtures({ runs: [], failRunList: true });
    process.env.HARVEST_TEST_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_USAGE);
    expect(s.err()).toMatch(/usage error: gh run list failed/);
    expect(s.outWriteCount()).toBe(0); // nothing buffered yet -- fails before any run is processed
  });

  it("exit 64: an unknown flag never invokes gh at all", async () => {
    const s = sink();
    const code = await runCli(["--nope"], s);
    expect(code).toBe(EXIT_USAGE);
    expect(s.err()).toMatch(/usage error: unknown flag: --nope/);
  });
});
