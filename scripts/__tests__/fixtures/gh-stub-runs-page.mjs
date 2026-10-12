// scripts/__tests__/fixtures/gh-stub-runs-page.mjs
//
// The list-workflow-runs half of gh-stub.sh (#3230): serves
// `gh api repos/{owner}/{repo}/actions/workflows/health.yml/runs?created=<a>..<b>&...&per_page=<n>&page=<p>`
// from $GH_STUB_FIXTURES_DIR/run-list.json the way GitHub does -- filtered to
// the inclusive `created` range (and `branch`, when given), newest first,
// paged by `per_page`, with the REST field names and a `total_count`. It
// lives in node rather than POSIX sh because date-range filtering and JSON
// paging are not shell work.
//
// Fixture files (all optional except run-list.json, which holds runs in the
// shape `makeRun` builds -- the names `gh run list --json` used):
//   run-list.ceiling      a number N: like GitHub's 1,000-result ceiling per
//                         search, only the newest N matching runs are ever
//                         served, while total_count still reports them all
//   run-list.total-count  a number that replaces the real total_count
//   runs-page-<p>.fail          page <p> of any partition fails every time
//   runs-page-<p>.malformed     page <p> answers 200 with a non-runs body
//   runs-page-<p>.rate-limited  page <p> fails with gh's rate-limit message
//   runs-page-<p>.repeat        page <p> re-serves page <p - 1>'s rows
//                               (pagination that stops progressing)
//   runs-page-<p>.total-shift   page <p> reports total_count + 1 every time
//   runs-page-<p>.total-shift-once
//                               page <p> reports total_count + 1 once, and
//                               the marker is consumed (a run landing or
//                               vanishing mid-walk, then settling)

import { existsSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";

const dir = process.env.GH_STUB_FIXTURES_DIR;
const path = process.argv[2];
const params = new URLSearchParams(path.slice(path.indexOf("?") + 1));
const page = Number(params.get("page") ?? "1");
const perPage = Number(params.get("per_page") ?? "30");
const [from, to] = (params.get("created") ?? "").split("..").map((value) => Date.parse(value));
const branch = params.get("branch");

const marker = (name) => existsSync(join(dir, name));
const numberFile = (name) =>
  marker(name) ? Number(readFileSync(join(dir, name), "utf8").trim()) : undefined;

if (marker(`runs-page-${page}.fail`)) {
  process.stderr.write(`gh-stub: simulated failure fetching runs page ${page}\n`);
  process.exit(1);
}
if (marker(`runs-page-${page}.rate-limited`)) {
  process.stdout.write(
    '{"message":"API rate limit exceeded for installation ID 1.","documentation_url":"https://docs.github.com/rest/overview/rate-limits-for-the-rest-api","status":"403"}',
  );
  process.stderr.write("gh: API rate limit exceeded for installation ID 1. (HTTP 403)\n");
  process.exit(1);
}
if (marker(`runs-page-${page}.malformed`)) {
  process.stdout.write('{"unexpected":true}');
  process.exit(0);
}

const matching = JSON.parse(readFileSync(join(dir, "run-list.json"), "utf8"))
  .filter((run) => {
    const created = Date.parse(run.createdAt);
    return created >= from && created <= to && (!branch || run.headBranch === branch);
  })
  .sort(
    (left, right) =>
      Date.parse(right.createdAt) - Date.parse(left.createdAt) ||
      right.databaseId - left.databaseId,
  );
const servable = matching.slice(0, numberFile("run-list.ceiling") ?? matching.length);
let totalShift = marker(`runs-page-${page}.total-shift`) ? 1 : 0;
if (marker(`runs-page-${page}.total-shift-once`)) {
  rmSync(join(dir, `runs-page-${page}.total-shift-once`));
  totalShift = 1;
}
const servedPage = marker(`runs-page-${page}.repeat`) ? page - 1 : page;
const rows = servable.slice((servedPage - 1) * perPage, servedPage * perPage);

process.stdout.write(
  JSON.stringify({
    total_count: (numberFile("run-list.total-count") ?? matching.length) + totalShift,
    workflow_runs: rows.map((run) => ({
      id: run.databaseId,
      run_attempt: run.attempt,
      event: run.event,
      head_branch: run.headBranch,
      head_sha: run.headSha,
      created_at: run.createdAt,
      conclusion: run.conclusion,
      status: run.status,
    })),
  }),
);
