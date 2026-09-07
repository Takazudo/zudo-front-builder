// Shared builders for tests that drive the supervisor-watch `gh` stub.
//
// Timeline records come from zfb's hand-authored corpus, while the prefix and
// surrounding job-log envelope come from a captured GitHub REST response.
// Keeping those two owners here prevents each consumer from reconstructing
// either contract independently.

import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { REST_LOG_PREFIX } from "./load-rest-job-log-capture.mjs";
import { TIMELINE_SAMPLES } from "./load-timeline-samples.mjs";

export const GH_STUB_PATH = fileURLToPath(new URL("./gh-stub.sh", import.meta.url));

const activeDirs = new Set();

export function makeRun(overrides = {}) {
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

/** One page returned by the REST run-jobs endpoint. */
export function jobsJson(jobId, { conclusion = "success" } = {}) {
  return {
    total_count: 1,
    jobs: [
      {
        name: "health",
        id: jobId,
        conclusion,
        steps: [{ name: "Set up job" }],
      },
    ],
  };
}

/**
 * Build the canonical up+boom record used by the former shell harness.
 * Only the four scenario-specific tokens are changed; every other field
 * remains owned by the shared timeline corpus.
 */
export function timelineLine({ outcome, firstUpLine, total, env }) {
  const record = TIMELINE_SAMPLES.UP_BOOM_LINE.replace(/outcome=[^ ]+/, `outcome=${outcome}`)
    .replace(/first-up-line=[^ ]+/, `first-up-line=${firstUpLine}`)
    .replace(/total=[^ ]+/, `total=${total}`)
    .replace(/env=[^ ]+/, `env=${env}`);
  return `${REST_LOG_PREFIX}${record}`;
}

/** A captured REST job-log envelope containing one timeline record. */
export function jobLog(options) {
  return [
    `${REST_LOG_PREFIX}##[section]Starting: Run tests`,
    timelineLine(options),
    `${REST_LOG_PREFIX}PASS scripts/__tests__/docs-dev-supervisor.test.mjs`,
    "",
  ].join("\n");
}

/** A captured REST job-log envelope from a job that emitted no records. */
export function jobLogNoRecords() {
  return [
    `${REST_LOG_PREFIX}##[section]Starting: Run tests`,
    `${REST_LOG_PREFIX}PASS some-other.test.mjs`,
    "",
  ].join("\n");
}

function jobsPage(jobsOrPage, totalCount) {
  if (Array.isArray(jobsOrPage)) {
    return { total_count: totalCount ?? jobsOrPage.length, jobs: jobsOrPage };
  }
  return {
    ...jobsOrPage,
    total_count: totalCount ?? jobsOrPage.total_count ?? jobsOrPage.jobs.length,
  };
}

export function setupFixtures({
  runs,
  jobsById = {},
  jobsTotalCountById = {},
  extraJobPagesById = {},
  logsByJobId = {},
  failingJobIds = [],
  notFoundJobIds = [],
  transientlyFailingJobIds = [],
  failRunList = false,
  failRunListOnce = false,
}) {
  const dir = mkdtempSync(join(tmpdir(), "harvest-supervisor-timelines-test-"));
  activeDirs.add(dir);

  writeFileSync(join(dir, "run-list.json"), JSON.stringify(runs));
  if (failRunList) writeFileSync(join(dir, "run-list.fail"), "");
  if (failRunListOnce) writeFileSync(join(dir, "run-list.fail-once"), "");

  for (const [runId, jobsOrPage] of Object.entries(jobsById)) {
    writeFileSync(
      join(dir, `jobs-${runId}.json`),
      JSON.stringify(jobsPage(jobsOrPage, jobsTotalCountById[runId])),
    );
  }
  for (const [runId, pages] of Object.entries(extraJobPagesById)) {
    for (const [page, jobsOrPage] of Object.entries(pages)) {
      writeFileSync(
        join(dir, `jobs-${runId}-p${page}.json`),
        JSON.stringify(jobsPage(jobsOrPage, jobsTotalCountById[runId])),
      );
    }
  }
  for (const [jobId, log] of Object.entries(logsByJobId)) {
    writeFileSync(join(dir, `job-${jobId}.log`), log);
  }
  for (const jobId of failingJobIds) writeFileSync(join(dir, `job-${jobId}.fail`), "");
  for (const jobId of notFoundJobIds) writeFileSync(join(dir, `job-${jobId}.notfound`), "");
  for (const jobId of transientlyFailingJobIds) {
    writeFileSync(join(dir, `job-${jobId}.fail-once`), "");
  }

  return {
    dir,
    stubPath: GH_STUB_PATH,
    cleanup() {
      rmSync(dir, { recursive: true, force: true });
      activeDirs.delete(dir);
    },
  };
}

export function readCalls(dir) {
  try {
    return readFileSync(join(dir, "calls.log"), "utf8").trim().split("\n");
  } catch {
    return [];
  }
}

export function cleanupFixtures() {
  for (const dir of activeDirs) rmSync(dir, { recursive: true, force: true });
  activeDirs.clear();
}
