#!/usr/bin/env node
// verify-source-ci.mjs — gate npm publication on the release SOURCE commit's
// required CI (#4690). Read-only: it lists check runs and commit statuses for
// exactly one SHA and never re-runs, dispatches or mutates anything.
//
//   node scripts/release/verify-source-ci.mjs --sha <40-hex> [--json] [--repo owner/name]
//                                             [--require-pr-only]
//
// Exit 0 passed, 1 not passed (pending, cancelled, skipped, failed, missing),
// 2 usage or read error. Only GitHub Actions check runs (app github-actions)
// count, and each only when its check suite's workflow run is the context's
// own workflow file (REQUIRED_CONTEXTS[].workflow); commit statuses never
// satisfy a required context (see ACTIONS_APP).
//
// REQUIRED_CONTEXTS mirrors the `main` ruleset in scripts/apply-main-ruleset.sh
// (a drift test keeps the two lists equal). Each context names the workflow
// that produces it. Two of those workflows run on `pull_request` only, so a
// commit that reached `main` by a push (the release version bump) or as a merge
// commit never carries their check runs: they are reported as
// `not-applicable-pr-only`, never counted as passed, and only gate when
// --require-pr-only is given. A drift test reads each workflow's `on:` block so
// this classification cannot silently go stale.
import { execGh, isMain, parseJsonLines, repoFromEnv } from "./gh.mjs";

export const REQUIRED_CONTEXTS = Object.freeze([
  { context: "health", workflow: "health.yml", prOnly: false },
  { context: "build (no-v8)", workflow: "health.yml", prOnly: false },
  {
    context: "Build binary (x86_64-unknown-linux-gnu)",
    workflow: "node-free-smoke.yml",
    prOnly: false,
  },
  {
    context: "Build binary (aarch64-unknown-linux-gnu)",
    workflow: "node-free-smoke.yml",
    prOnly: false,
  },
  { context: "Smoke amd64 (local mode)", workflow: "node-free-smoke.yml", prOnly: false },
  { context: "Smoke amd64 TS-config (local mode)", workflow: "node-free-smoke.yml", prOnly: false },
  { context: "Smoke arm64 (local mode)", workflow: "node-free-smoke.yml", prOnly: false },
  { context: "Smoke arm64 TS-config (local mode)", workflow: "node-free-smoke.yml", prOnly: false },
  {
    context: "Scaffold E2E (packed tarballs, pre-publish)",
    workflow: "node-free-smoke.yml",
    prOnly: false,
  },
  { context: "pnpm audit (prod)", workflow: "pr-checks.yml", prOnly: true },
  { context: "Docs gate", workflow: "docs-checks.yml", prOnly: true },
]);

// Only `success` passes. The ruleset reads a job-level skip as success, but that
// exists for docs-only PRs (#4372); a push to main runs every required job, so a
// skipped required job on a release source commit is not evidence of passing.
const PASSING_CONCLUSIONS = new Set(["success"]);

// Every required context is a GitHub Actions job, so only check runs created by
// the GitHub Actions app count. Commit statuses are NEVER accepted: any token
// with `statuses: write` can post a status under any context name, and no
// required context is produced as a status, so there is no expected creator to
// pin. A status named like a required context is reported as ignored evidence.
export const ACTIONS_APP = "github-actions";

const SHA_RE = /^[0-9a-f]{40}$/;

// Required contexts are produced by push / pull_request / merge_group runs. The
// same workflow file also runs on workflow_run (the post-release installer smoke)
// and workflow_dispatch, where these jobs report `skipped`; such a later suite must
// not mask the push run's result (v6.1.2 source 38db86d58, run 43401576405).
export const SOURCE_CI_EVENTS = new Set(["push", "pull_request", "merge_group"]);
const countsAsSourceCi = (run) =>
  run.workflowEvent === undefined || SOURCE_CI_EVENTS.has(run.workflowEvent);

/**
 * Pure: classify one required context from every check run and commit status
 * that carries its name on the SHA. The effective record is the newest check
 * run (highest id; a re-run replaces the earlier one for branch protection), or
 * the commit status when no check run exists.
 */
export const workflowPathOf = (spec) => `.github/workflows/${spec.workflow}`;

export function classifyContext(spec, checkRuns, statuses) {
  const named = checkRuns.filter((run) => run.name === spec.context);
  // Pinned to the producing workflow file: a job of the same name in another
  // workflow (or an unresolvable check suite) never satisfies the context.
  const runs = named
    .filter(
      (run) =>
        run.app === ACTIONS_APP &&
        run.workflowPath === workflowPathOf(spec) &&
        countsAsSourceCi(run),
    )
    .sort((a, b) => Number(a.id) - Number(b.id));
  const status = statuses.find((entry) => entry.context === spec.context);
  const base = { context: spec.context, workflow: spec.workflow, prOnly: spec.prOnly };
  const ignored = {};
  if (named.length > runs.length) {
    ignored.ignoredCheckRuns = named
      .filter((run) => !runs.includes(run))
      .map((run) => ({
        id: run.id,
        app: run.app ?? null,
        workflowPath: run.workflowPath ?? null,
        workflowEvent: run.workflowEvent ?? null,
        conclusion: run.conclusion ?? null,
      }));
  }
  if (status) ignored.ignoredStatus = { state: status.state, creator: status.creator ?? null };
  if (runs.length > 0) {
    const effective = runs.at(-1);
    const evidence = {
      checkRunId: effective.id,
      status: effective.status,
      conclusion: effective.conclusion ?? null,
      url: effective.html_url ?? null,
      history: runs.map((run) => ({
        id: run.id,
        status: run.status,
        conclusion: run.conclusion ?? null,
      })),
      ...ignored,
    };
    if (effective.status !== "completed") return { ...base, state: "pending", ...evidence };
    if (PASSING_CONCLUSIONS.has(effective.conclusion)) {
      return { ...base, state: "passed", ...evidence };
    }
    if (effective.conclusion === "cancelled") return { ...base, state: "cancelled", ...evidence };
    if (effective.conclusion === "skipped") return { ...base, state: "skipped", ...evidence };
    return { ...base, state: "failed", ...evidence };
  }
  return { ...base, state: spec.prOnly ? "not-applicable-pr-only" : "missing", ...ignored };
}

/** Pure: the verdict over every required context. */
export function evaluate({
  sha,
  checkRuns,
  statuses,
  requirePrOnly = false,
  contexts = REQUIRED_CONTEXTS,
}) {
  const results = contexts.map((spec) => {
    const result = classifyContext(spec, checkRuns, statuses);
    if (requirePrOnly && result.state === "not-applicable-pr-only") {
      return { ...result, state: "missing" };
    }
    return result;
  });
  const blocking = results.filter(
    (result) => result.state !== "passed" && result.state !== "not-applicable-pr-only",
  );
  return {
    schema: "zfb-source-ci/v1",
    sha,
    passed: blocking.length === 0,
    requirePrOnly,
    results,
    blocking: blocking.map((result) => `${result.context}: ${result.state}`),
  };
}

export function readSourceCi({ sha, repo, gh = execGh }) {
  const checkRuns = parseJsonLines(
    gh([
      "api",
      "--paginate",
      `repos/${repo}/commits/${sha}/check-runs?per_page=100`,
      "--jq",
      ".check_runs[] | {id, name, status, conclusion, html_url, app: .app.slug, suite: .check_suite.id}",
    ]),
  );
  // Resolve each Actions check suite to its workflow run's file path, once.
  const paths = new Map();
  const events = new Map();
  for (const suite of new Set(checkRuns.filter((r) => r.app === ACTIONS_APP).map((r) => r.suite))) {
    if (suite === undefined || suite === null) continue;
    const runs = JSON.parse(
      gh(["api", `repos/${repo}/actions/runs?check_suite_id=${suite}&per_page=1`]),
    ).workflow_runs;
    const run = runs?.[0];
    paths.set(suite, run && run.head_sha === sha ? run.path : null);
    events.set(suite, run?.event ?? null);
  }
  for (const run of checkRuns) {
    run.workflowPath = paths.get(run.suite) ?? null;
    run.workflowEvent = events.get(run.suite) ?? null;
  }
  const combined = JSON.parse(gh(["api", `repos/${repo}/commits/${sha}/status`]));
  const statuses = (combined.statuses ?? []).map((entry) => ({
    context: entry.context,
    state: entry.state,
    creator: entry.creator?.login ?? null,
  }));
  return { checkRuns, statuses };
}

export function parseArgs(argv) {
  const opts = { json: false, requirePrOnly: false };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--json") opts.json = true;
    else if (arg === "--require-pr-only") opts.requirePrOnly = true;
    else if (arg === "--sha" || arg === "--repo") {
      const value = argv[++i];
      if (!value) return null;
      opts[arg.slice(2)] = value;
    } else return null;
  }
  if (!opts.sha || !SHA_RE.test(opts.sha)) return null;
  return opts;
}

export function render(verdict) {
  const lines = [`source CI for ${verdict.sha}:`];
  for (const result of verdict.results) {
    const where = result.checkRunId ? ` (check run ${result.checkRunId})` : "";
    lines.push(`  ${result.state.padEnd(24)} ${result.context}${where}`);
  }
  lines.push(
    verdict.passed
      ? "== PASS: every required context passed on this exact SHA (pr-only contexts are not counted as passed) =="
      : `== NOT PASSED: ${verdict.blocking.join("; ")} ==`,
  );
  return `${lines.join("\n")}\n`;
}

export function main(argv, { gh = execGh, env = process.env, out = process.stdout } = {}) {
  const opts = parseArgs(argv);
  if (!opts) {
    process.stderr.write(
      "usage: verify-source-ci.mjs --sha <40-hex> [--json] [--repo owner/name] [--require-pr-only]\n",
    );
    return 2;
  }
  let data;
  try {
    data = readSourceCi({ sha: opts.sha, repo: opts.repo ?? repoFromEnv(env), gh });
  } catch (error) {
    process.stderr.write(`verify-source-ci: ${error.message}\n`);
    return 2;
  }
  const verdict = evaluate({ sha: opts.sha, ...data, requirePrOnly: opts.requirePrOnly });
  out.write(opts.json ? `${JSON.stringify(verdict, null, 2)}\n` : render(verdict));
  return verdict.passed ? 0 : 1;
}

if (isMain(import.meta.url)) {
  process.exitCode = main(process.argv.slice(2));
}
