import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import {
  cleanupFixtures,
  jobLog,
  jobLogNoRecords,
  jobsJson,
  makeRun,
  setupFixtures,
} from "./fixtures/gh-fixture-tree.mjs";
import { classifyPassA, classifyPassB, deriveVerdict, runCli } from "../supervisor-watch.mjs";

const FIXTURE_ENV = "GH_STUB_FIXTURES_DIR";
const ENV_A = "sha256:ed62f5285936a0ca";
const ENV_B = "sha256:173aae2cffffffff";
const NOW = new Date("2026-09-08T00:00:00Z");

function healthyLog(env = ENV_A) {
  return jobLog({ outcome: "ok", firstUpLine: 430, total: 540, env });
}

function sink() {
  const stdout = [];
  const stderr = [];
  return {
    stdout: { write: (chunk) => stdout.push(chunk.toString()) },
    stderr: { write: (chunk) => stderr.push(chunk.toString()) },
    out: () => stdout.join(""),
    err: () => stderr.join(""),
  };
}

async function withFixtureEnvironment(dir, operation) {
  const hadValue = Object.hasOwn(process.env, FIXTURE_ENV);
  const previous = process.env[FIXTURE_ENV];
  process.env[FIXTURE_ENV] = dir;
  try {
    return await operation();
  } finally {
    if (hadValue) process.env[FIXTURE_ENV] = previous;
    else delete process.env[FIXTURE_ENV];
  }
}

async function runScenario(fixtureOptions) {
  const { dir, stubPath } = setupFixtures(fixtureOptions);
  const outDir = join(dir, "out");
  const githubOutput = join(dir, "gh-output.txt");
  const streams = sink();
  const env = {
    ...process.env,
    WATCH_OUT_DIR: outDir,
    WATCH_RETRY_DELAY_MS: "0",
    GITHUB_OUTPUT: githubOutput,
    GITHUB_STEP_SUMMARY: join(dir, "gh-step-summary.md"),
  };

  const exitCode = await withFixtureEnvironment(dir, () =>
    runCli(["--gh", stubPath], { ...streams, env, now: NOW }),
  );

  return { dir, exitCode, githubOutput, outDir, ...streams };
}

afterEach(() => {
  cleanupFixtures();
});

const verdictScenarios = [
  {
    label: "green: healthy population on both branches",
    exitCode: 0,
    verdict: "green",
    detail: "all=ok:0/0 main=ok:1/0",
    fixtures: () => ({
      runs: [
        makeRun({ databaseId: 1001, headBranch: "main" }),
        makeRun({ databaseId: 1002, headBranch: "feat/x" }),
      ],
      jobsById: { 1001: jobsJson(5001), 1002: jobsJson(5002) },
      logsByJobId: { 5001: healthyLog(), 5002: healthyLog() },
    }),
  },
  {
    label: "no-data: gh run list returns no runs",
    exitCode: 1,
    verdict: "no-data",
    fixtures: () => ({ runs: [] }),
  },
  {
    label: "no-data: the only run is still in progress",
    exitCode: 1,
    verdict: "no-data",
    fixtures: () => ({
      runs: [makeRun({ databaseId: 1001, status: "in_progress" })],
    }),
  },
  {
    label: "green: main health red before the test step, PR healthy",
    exitCode: 0,
    verdict: "green",
    fixtures: () => ({
      runs: [
        makeRun({ databaseId: 1001, headBranch: "main" }),
        makeRun({ databaseId: 1002, headBranch: "feat/x" }),
      ],
      jobsById: {
        1001: jobsJson(5001, { conclusion: "failure" }),
        1002: jobsJson(5002),
      },
      logsByJobId: { 5001: jobLogNoRecords(), 5002: healthyLog() },
    }),
  },
  {
    label: "red R-A: outcome=failed on a PR branch",
    exitCode: 2,
    verdict: "red",
    fixtures: () => ({
      runs: [
        makeRun({ databaseId: 1001, headBranch: "main" }),
        makeRun({ databaseId: 1002, headBranch: "feat/x" }),
      ],
      jobsById: { 1001: jobsJson(5001), 1002: jobsJson(5002) },
      logsByJobId: {
        5001: healthyLog(),
        5002: jobLog({ outcome: "failed", firstUpLine: 430, total: 540, env: ENV_A }),
      },
    }),
  },
  {
    label: "red R-B: pre-UP 7600ms >= 7500ms boundary",
    exitCode: 2,
    verdict: "red",
    fixtures: () => ({
      runs: [makeRun({ databaseId: 1001 })],
      jobsById: { 1001: jobsJson(5001) },
      logsByJobId: {
        5001: jobLog({ outcome: "ok", firstUpLine: 7600, total: 7900, env: ENV_A }),
      },
    }),
  },
  {
    label: "red infra: gh run list fails",
    exitCode: 2,
    verdict: "red",
    fixtures: () => ({
      runs: [makeRun({ databaseId: 1001 })],
      failRunList: true,
    }),
  },
  {
    label: "red telemetry-vanished: green health job, no records",
    exitCode: 2,
    verdict: "red",
    fixtures: () => ({
      runs: [makeRun({ databaseId: 1001 })],
      jobsById: { 1001: jobsJson(5001) },
      logsByJobId: { 5001: jobLogNoRecords() },
    }),
  },
  {
    label: "red silent-on-main: one green main job with zero records",
    exitCode: 2,
    verdict: "red",
    fixtures: () => ({
      runs: [makeRun({ databaseId: 1001 }), makeRun({ databaseId: 1003 })],
      jobsById: { 1001: jobsJson(5001), 1003: jobsJson(5003) },
      logsByJobId: { 5001: healthyLog(), 5003: jobLogNoRecords() },
    }),
  },
  {
    label: "green: silent green job on a PR branch only",
    exitCode: 0,
    verdict: "green",
    fixtures: () => ({
      runs: [
        makeRun({ databaseId: 1001, headBranch: "main" }),
        makeRun({ databaseId: 1002, headBranch: "feat/pre-emitter" }),
      ],
      jobsById: { 1001: jobsJson(5001), 1002: jobsJson(5002) },
      logsByJobId: { 5001: healthyLog(), 5002: jobLogNoRecords() },
    }),
  },
  {
    label: "green: pull_request run from a fork branch named main is not trunk",
    exitCode: 0,
    verdict: "green",
    fixtures: () => ({
      runs: [
        makeRun({ databaseId: 1001, headBranch: "main" }),
        makeRun({ databaseId: 1004, headBranch: "main", event: "pull_request" }),
      ],
      jobsById: { 1001: jobsJson(5001), 1004: jobsJson(5004) },
      logsByJobId: { 5001: healthyLog(), 5004: healthyLog(ENV_B) },
    }),
  },
  {
    label: "red partial: one job log unfetchable (harvester exit 3)",
    exitCode: 2,
    verdict: "red",
    fixtures: () => ({
      runs: [
        makeRun({ databaseId: 1001, headBranch: "main" }),
        makeRun({ databaseId: 1002, headBranch: "feat/x" }),
      ],
      jobsById: { 1001: jobsJson(5001), 1002: jobsJson(5002) },
      logsByJobId: { 5001: healthyLog() },
      failingJobIds: [5002],
    }),
  },
  {
    label: "green: env drift confined to a PR branch",
    exitCode: 0,
    verdict: "green",
    fixtures: () => ({
      runs: [
        makeRun({ databaseId: 1001, headBranch: "main" }),
        makeRun({ databaseId: 1002, headBranch: "feat/x" }),
      ],
      jobsById: { 1001: jobsJson(5001), 1002: jobsJson(5002) },
      logsByJobId: { 5001: healthyLog(), 5002: healthyLog(ENV_B) },
    }),
  },
  {
    label: "red: env drift between two main runs",
    exitCode: 2,
    verdict: "red",
    fixtures: () => ({
      runs: [makeRun({ databaseId: 1001 }), makeRun({ databaseId: 1003 })],
      jobsById: { 1001: jobsJson(5001), 1003: jobsJson(5003) },
      logsByJobId: { 5001: healthyLog(), 5003: healthyLog(ENV_B) },
    }),
  },
];

describe("supervisor-watch verdict scenarios ported from the shell harness", () => {
  it.each(verdictScenarios)("$label", async ({ detail, exitCode, fixtures, verdict }) => {
    const result = await runScenario(fixtures());

    expect(result.exitCode).toBe(exitCode);
    expect(result.out().trimEnd().split("\n").at(-1)).toMatch(`verdict=${verdict} `);
    await expect(readFile(join(result.outDir, "all", "summary.txt"), "utf8")).resolves.toEqual(
      expect.any(String),
    );
    await expect(readFile(join(result.outDir, "main", "summary.txt"), "utf8")).resolves.toEqual(
      expect.any(String),
    );

    if (detail) {
      await expect(readFile(result.githubOutput, "utf8")).resolves.toContain(`detail=${detail}\n`);
    }
  });
});

describe("status precedence", () => {
  it.each([
    { hrc: 0, src: 0, expected: "ok" },
    { hrc: 0, src: 1, expected: "red" },
    { hrc: 1, src: 0, expected: "red" },
    { hrc: 4, src: 0, expected: "empty" },
    { hrc: 4, src: 1, expected: "empty" },
    { hrc: 4, src: 2, expected: "empty" },
    { hrc: 4, src: 64, expected: "empty" },
    { hrc: 64, src: 0, expected: "red" },
  ])("classifyPassA($hrc, $src) is $expected", ({ expected, hrc, src }) => {
    expect(classifyPassA(hrc, src)).toBe(expected);
  });

  it.each([
    { runCount: 0, silentCount: 0, src: 1, expected: "empty" },
    { runCount: 1, silentCount: 1, src: 0, expected: "silent" },
    { runCount: 1, silentCount: 1, src: 1, expected: "silent" },
    { runCount: 1, silentCount: 1, src: 2, expected: "silent" },
    { runCount: 1, silentCount: 1, src: 64, expected: "silent" },
    { runCount: 1, silentCount: 0, src: 0, expected: "ok" },
    { runCount: 1, silentCount: 0, src: 1, expected: "red" },
  ])(
    "classifyPassB($runCount runs, $silentCount silent, src=$src) is $expected",
    ({ expected, runCount, silentCount, src }) => {
      expect(classifyPassB({ runCount, silentCount, src })).toBe(expected);
    },
  );

  it.each([
    { statusA: "red", statusB: "ok", verdict: "red", exitCode: 2 },
    { statusA: "red", statusB: "empty", verdict: "red", exitCode: 2 },
    { statusA: "ok", statusB: "red", verdict: "red", exitCode: 2 },
    { statusA: "empty", statusB: "red", verdict: "red", exitCode: 2 },
    { statusA: "empty", statusB: "silent", verdict: "red", exitCode: 2 },
    { statusA: "empty", statusB: "ok", verdict: "no-data", exitCode: 1 },
    { statusA: "ok", statusB: "empty", verdict: "green", exitCode: 0 },
    { statusA: "ok", statusB: "ok", verdict: "green", exitCode: 0 },
  ])(
    "deriveVerdict($statusA, $statusB) is $verdict/$exitCode",
    ({ exitCode, statusA, statusB, verdict }) => {
      expect(deriveVerdict(statusA, statusB)).toEqual({ verdict, exitCode });
    },
  );
});
