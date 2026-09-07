import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import {
  cleanupFixtures,
  jobLog,
  jobLogNoRecords,
  jobsJson,
  makeRun,
  readCalls,
  setupFixtures,
} from "./fixtures/gh-fixture-tree.mjs";
import { runCli } from "../supervisor-watch.mjs";

const ENV_DIGEST = "sha256:ed62f5285936a0ca";
const NOW = new Date("2026-09-08T12:00:00Z");
const SINCE = "2026-08-31T12:00:00Z";

function sink() {
  const writes = [];
  return {
    stream: { write: (chunk) => writes.push(String(chunk)) },
    text: () => writes.join(""),
  };
}

function readIfPresent(path) {
  return existsSync(path) ? readFileSync(path, "utf8") : "";
}

function population({ silentPr = false, oneRun = false } = {}) {
  const runs = [makeRun({ databaseId: 1001, headBranch: "main" })];
  const jobsById = { 1001: jobsJson(5001) };
  const logsByJobId = {
    5001: jobLog({ outcome: "ok", firstUpLine: 430, total: 540, env: ENV_DIGEST }),
  };

  if (!oneRun) {
    runs.push(makeRun({ databaseId: 1002, headBranch: "feat/pre-emitter" }));
    jobsById[1002] = jobsJson(5002);
    logsByJobId[5002] = silentPr
      ? jobLogNoRecords()
      : jobLog({ outcome: "ok", firstUpLine: 430, total: 540, env: ENV_DIGEST });
  }

  return setupFixtures({ runs, jobsById, logsByJobId });
}

async function runWatch(fixtures, { tar } = {}) {
  const outDir = join(fixtures.dir, "out");
  const githubOutput = join(fixtures.dir, "gh-output.txt");
  const stepSummary = join(fixtures.dir, "gh-step-summary.md");
  const stdout = sink();
  const stderr = sink();
  const hadFixtureDir = Object.hasOwn(process.env, "GH_STUB_FIXTURES_DIR");
  const previousFixtureDir = process.env.GH_STUB_FIXTURES_DIR;

  process.env.GH_STUB_FIXTURES_DIR = fixtures.dir;
  try {
    const exitCode = await runCli(
      ["--out-dir", outDir, "--since", SINCE, "--gh", fixtures.stubPath],
      {
        env: {
          ...process.env,
          GITHUB_OUTPUT: githubOutput,
          GITHUB_STEP_SUMMARY: stepSummary,
          WATCH_RETRY_DELAY_MS: "0",
        },
        stdout: stdout.stream,
        stderr: stderr.stream,
        now: NOW,
        tar,
      },
    );
    return {
      exitCode,
      outDir,
      githubOutput,
      stepSummary,
      stdout: stdout.text(),
      stderr: stderr.text(),
    };
  } finally {
    if (hadFixtureDir) process.env.GH_STUB_FIXTURES_DIR = previousFixtureDir;
    else delete process.env.GH_STUB_FIXTURES_DIR;
  }
}

afterEach(() => {
  cleanupFixtures();
});

describe("supervisor watch workflow handoff and artifacts", () => {
  it("writes workflow outputs, folds pass-A logs into one artifact, and reuses them for pass B", async () => {
    const fixtures = population({ silentPr: true });
    const previousFixtureDir = process.env.GH_STUB_FIXTURES_DIR;
    const result = await runWatch(fixtures);

    expect(process.env.GH_STUB_FIXTURES_DIR).toBe(previousFixtureDir);
    expect(result.exitCode).toBe(0);
    expect(result.stdout).toBe("verdict=green all=ok:0/0 main=ok:1/0\n");
    expect(readFileSync(result.githubOutput, "utf8")).toBe(
      "verdict=green\ndetail=all=ok:0/0 main=ok:1/0\n",
    );

    const summary = readFileSync(result.stepSummary, "utf8");
    expect(summary).toContain("## Supervisor watch — green\n");
    expect(summary).toContain("### Runs with failed supervisor records");
    expect(summary).toContain("### Green health jobs with zero records");
    expect(summary).toContain("### Runs the harvester could not fetch or parse");
    expect(summary).toContain("### Harvest window and notices");
    expect(summary).toContain("### Summary — all branches");
    expect(summary).toContain("### Summary — main only (strict identity, 1 run(s))");

    const archive = join(result.outDir, "job-logs.tar.gz");
    const members = execFileSync("tar", ["-tzf", archive], { encoding: "utf8" }).trim().split("\n");
    expect(members).toContain("job-logs/run-1001-job-5001.log");
    expect(members).toContain("job-logs/run-1002-job-5002.log");
    expect(existsSync(join(result.outDir, "job-logs"))).toBe(false);

    const mainRuns = readFileSync(join(result.outDir, "main", "runs.txt"), "utf8")
      .trim()
      .split("\n");
    expect(mainRuns).toHaveLength(1);
    expect(mainRuns[0]).toMatch(/^run=1001 /);
    expect(mainRuns[0]).not.toContain("run=1002");

    const calls = readCalls(fixtures.dir);
    expect(
      calls.filter((call) => call === "api repos/{owner}/{repo}/actions/jobs/5001/logs"),
    ).toHaveLength(1);
    expect(
      calls.filter((call) => call === "api repos/{owner}/{repo}/actions/jobs/5002/logs"),
    ).toHaveLength(1);
    expect(
      calls.filter(
        (call) => call === "api repos/{owner}/{repo}/actions/runs/1001/jobs?per_page=100",
      ),
    ).toHaveLength(1);
    expect(
      calls.filter(
        (call) => call === "api repos/{owner}/{repo}/actions/runs/1002/jobs?per_page=100",
      ),
    ).toHaveLength(1);
    expect(calls.filter((call) => call.startsWith("api "))).toHaveLength(4);
    expect(calls.filter((call) => call.startsWith("run view "))).toEqual([]);

    expect(readFileSync(join(result.outDir, "failed-runs.txt"), "utf8")).toBe("");
    expect(readFileSync(join(result.outDir, "harvest-notices.txt"), "utf8")).toContain(
      `window: since=${SINCE}`,
    );
    expect(readFileSync(join(result.outDir, "silent-runs.txt"), "utf8")).toMatch(
      /^run=1002 .* branch=feat\/pre-emitter .* lines=0 /m,
    );
  });
});

describe("supervisor watch archive failures", () => {
  it("withholds the verdict and preserves triage output when tar fails outright", async () => {
    const fixtures = population();
    const result = await runWatch(fixtures, {
      tar: async () => {
        throw new Error("simulated tar failure");
      },
    });

    expect(result.exitCode).not.toBe(0);
    expect(result.stdout).not.toContain("verdict=");
    expect(readIfPresent(result.githubOutput)).not.toContain("verdict=");
    expect(existsSync(join(result.outDir, "job-logs.tar.gz"))).toBe(false);
    expect(existsSync(join(result.outDir, "job-logs", "run-1001-job-5001.log"))).toBe(true);
    expect(existsSync(join(result.outDir, "job-logs", "run-1002-job-5002.log"))).toBe(true);
    expect(existsSync(join(result.outDir, "failed-runs.txt"))).toBe(true);
    expect(existsSync(join(result.outDir, "harvest-notices.txt"))).toBe(true);
    expect(readFileSync(result.stepSummary, "utf8")).toContain("## Supervisor watch — green");
    expect(result.stderr).toContain("failed; kept the plain directory, wrote no verdict");
  });

  it("removes a partly written archive while preserving its source log and handoffs", async () => {
    const fixtures = population({ oneRun: true });
    const result = await runWatch(fixtures, {
      tar: async (args) => {
        if (args[0] === "-czf") writeFileSync(args[1], "not a real gzip stream");
        throw new Error("simulated mid-write tar failure");
      },
    });

    expect(result.exitCode).not.toBe(0);
    expect(result.stdout).not.toContain("verdict=");
    expect(readIfPresent(result.githubOutput)).not.toContain("verdict=");
    expect(existsSync(join(result.outDir, "job-logs.tar.gz"))).toBe(false);
    expect(existsSync(join(result.outDir, "job-logs", "run-1001-job-5001.log"))).toBe(true);
    expect(existsSync(join(result.outDir, "failed-runs.txt"))).toBe(true);
    expect(existsSync(join(result.outDir, "harvest-notices.txt"))).toBe(true);
    expect(readFileSync(result.stepSummary, "utf8")).toContain("## Supervisor watch — green");
  });
});
