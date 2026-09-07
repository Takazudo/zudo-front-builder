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
  EXIT_NO_RECORDS,
  EXIT_OK,
  EXIT_PARTIAL,
  EXIT_USAGE,
  buildRunListArgs,
  parseCliArgs,
  runCli,
} from "../harvest-supervisor-timelines.mjs";

const GH_STUB = `#!/usr/bin/env bash
set -euo pipefail
FIXDIR="\${HARVEST_TEST_FIXTURES_DIR:?HARVEST_TEST_FIXTURES_DIR not set}"
printf '%s\\n' "$*" >> "$FIXDIR/calls.log"

if [ "$1" = "run" ] && [ "$2" = "list" ]; then
  if [ -f "$FIXDIR/run-list.fail" ]; then
    echo "gh-stub: simulated run list failure" >&2
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
  failRunList = false,
}) {
  const dir = mkdtempSync(join(tmpdir(), "harvest-supervisor-timelines-test-"));
  activeDirs.push(dir);

  const stubPath = join(dir, "gh-stub.sh");
  writeFileSync(stubPath, GH_STUB);
  chmodSync(stubPath, 0o755);

  writeFileSync(join(dir, "run-list.json"), JSON.stringify(runs));
  if (failRunList) writeFileSync(join(dir, "run-list.fail"), "");

  for (const [runId, jobs] of Object.entries(jobsById)) {
    writeFileSync(join(dir, `jobs-${runId}.json`), JSON.stringify({ jobs }));
  }
  for (const [jobId, log] of Object.entries(logsByJobId)) {
    writeFileSync(join(dir, `job-${jobId}.log`), log);
  }
  for (const jobId of failingJobIds) {
    writeFileSync(join(dir, `job-${jobId}.fail`), "");
  }

  return { dir, stubPath };
}

function sink() {
  const outWrites = [];
  const errWrites = [];
  return {
    stdout: { write: (chunk) => outWrites.push(chunk) },
    stderr: { write: (chunk) => errWrites.push(chunk) },
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
  it("applies documented defaults with no flags", () => {
    const options = parseCliArgs([]);
    expect(options.since).toBe("2026-09-06T22:00:00Z");
    expect(options.limit).toBe(200);
    expect(options.branch).toBeUndefined();
    expect(options.saveDir).toBeUndefined();
    expect(options.gh).toBe("gh");
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

  it("exit 1: zero records extracted from an otherwise successful harvest", async () => {
    const runs = [makeRun({ databaseId: 2001 })];
    const { dir, stubPath } = setupFixtures({
      runs,
      jobsById: { 2001: [{ databaseId: 6001, name: "health" }] },
      logsByJobId: { 6001: JOB_LOG_NO_RECORDS },
    });
    process.env.HARVEST_TEST_FIXTURES_DIR = dir;

    const s = sink();
    const code = await runCli(["--gh", stubPath], s);

    expect(code).toBe(EXIT_NO_RECORDS);
    expect(s.out()).toBe("");
    expect(s.err()).toMatch(/runs=1 harvested=1 failed=0 records=0/);
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

    expect(code).toBe(EXIT_NO_RECORDS);
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

    // The manifest is documented as one line per run: a gh subprocess error
    // (Node's execFile embeds the child's own stderr, often multi-line)
    // must never fragment the run=1004 manifest entry across physical lines.
    const errLines = s.err().trimEnd().split("\n");
    const run1004Line = errLines.find((line) => line.startsWith("run=1004"));
    expect(run1004Line).toMatch(/^run=1004 .*error=.+$/);
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
