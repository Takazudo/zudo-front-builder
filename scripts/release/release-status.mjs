#!/usr/bin/env node
// release-status.mjs — read-only status of one release and ONE recommended next
// action (#4690). It never dispatches, uploads, publishes or moves a dist-tag.
//
//   node scripts/release/release-status.mjs --tag vX.Y.Z [--json]
//        [--probe-capabilities] [--repo owner/name] [--repo-root DIR]
//
// Reports: the source (tag → commit/tree, or for a fresh draft without a tag
// ref, its 40-hex targetCommitish; `source.kind` says which); the Release (exists/draft/published,
// targetCommitish); expected Release assets (3 CI archive pairs, the 10 Darwin
// inputs, the MD/WASM digest manifest) as present/missing/conflicting; the
// source commit's required CI (verify-source-ci); Darwin admission (the real
// verifier on the downloaded inputs, in a temp dir); per-package npm state
// (npm-package-state); npm dist-tags; and the latest release.yml runs for the
// tag. The recommended action is one of:
//   prepare-darwin | publish-draft | dispatch-recovery | dispatch-finalize |
//   blocked:<reason> | complete
// --probe-capabilities adds `gh auth status`, the caller's repo permissions and
// whether Actions workflows are readable.
//
// Darwin admission reads git objects for the source commit from --repo-root
// (default: cwd); a checkout that lacks the commit reports admission "unknown".
//
// Exit 0 when the report was produced (any action), 2 usage error.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { admitDarwinArtifacts, sourceInputs } from "../admit-darwin-artifacts.mjs";
import { PUBLISHED_PACKAGES } from "../retire-next-dist-tag.mjs";
import { GhError, execGh, isMain, repoFromEnv } from "./gh.mjs";
import { DEFAULT_REGISTRY, collectStates, defaultRequest } from "./npm-package-state.mjs";
import { assetInventory, expectedReleaseAssets } from "./release-assets-contract.mjs";
import { evaluate as evaluateSourceCi, readSourceCi } from "./verify-source-ci.mjs";

const PRERELEASE_RE = /-(next|beta|rc)\.\d+$/;

function compareSemver(a, b) {
  const parse = (v) => {
    const [core, pre] = String(v).split("-");
    return { nums: core.split(".").map(Number), pre: pre ?? null };
  };
  const x = parse(a);
  const y = parse(b);
  for (let i = 0; i < 3; i++) if (x.nums[i] !== y.nums[i]) return x.nums[i] < y.nums[i] ? -1 : 1;
  if (x.pre === y.pre) return 0;
  if (x.pre === null) return 1;
  if (y.pre === null) return -1;
  const [xl, xn] = x.pre.split(".");
  const [yl, yn] = y.pre.split(".");
  if (xl !== yl) return xl < yl ? -1 : 1;
  return Number(xn) < Number(yn) ? -1 : Number(xn) > Number(yn) ? 1 : 0;
}

/**
 * Pure: do the dist-tags already satisfy what finalization would leave behind?
 * Stable: `latest` is this version or higher, and `next` is absent or strictly
 * ahead of `latest`. Prerelease: `next` is this version or higher, and `latest`
 * equals it while the registry is still in the prerelease phase (#481).
 */
export function distTagsSettled(version, tags) {
  const problems = [];
  for (const [name, entry] of Object.entries(tags)) {
    if (entry.error) {
      problems.push(`${name}: dist-tags unreadable`);
      continue;
    }
    const { latest, next } = entry.tags;
    if (PRERELEASE_RE.test(version)) {
      if (!next || compareSemver(next, version) < 0) problems.push(`${name}: next=${next ?? "∅"}`);
      if ((!latest || latest.includes("-")) && latest !== version)
        problems.push(`${name}: latest=${latest ?? "∅"} (prerelease phase)`);
    } else {
      if (!latest || compareSemver(latest, version) < 0)
        problems.push(`${name}: latest=${latest ?? "∅"}`);
      if (next && latest && compareSemver(next, latest) <= 0)
        problems.push(`${name}: stale next=${next}`);
    }
  }
  return { settled: problems.length === 0, problems };
}

/**
 * Pure: the commit this release builds from. A published Release always has a
 * tag ref; a fresh DRAFT has none until it is published, so its 40-hex
 * targetCommitish stands in (the procedure creates drafts with
 * `--target <SOURCE_SHA>`; release.yml re-checks the tag at publish time).
 * Returns { kind: "tag" | "draft-target", commit } or { kind: null, problem }.
 */
export function resolveSource({ tagInfo, release }) {
  if (tagInfo?.commit) return { kind: "tag", commit: tagInfo.commit, tree: tagInfo.tree ?? null };
  if (!release) return { kind: null, problem: "release-missing" };
  if (!release.isDraft) return { kind: null, problem: "tag-missing" };
  if (/^[0-9a-f]{40}$/.test(release.targetCommitish ?? "")) {
    return { kind: "draft-target", commit: release.targetCommitish, tree: null };
  }
  return { kind: null, problem: "release-target-not-sha" };
}

/** Pure: the single recommended next action. */
export function recommendNextAction(s) {
  if (!s.source?.commit) return `blocked:${s.source?.problem ?? "tag-missing"}`;
  if (!s.release) return "blocked:release-missing";
  if (s.release.targetMismatch) return "blocked:release-target-mismatch";
  if (s.runs?.some((run) => run.status !== "completed")) return "blocked:release-run-in-progress";
  if (s.assets.conflicting.length) return "blocked:release-assets-conflicting";
  if (!s.sourceCi) return "blocked:source-ci-unknown";
  if (!s.sourceCi.passed) {
    const states = new Set(s.sourceCi.results.map((result) => result.state));
    if (states.has("failed") || states.has("cancelled")) return "blocked:source-ci-failed";
    if (states.has("pending")) return "blocked:source-ci-pending";
    return "blocked:source-ci-missing";
  }
  const darwinMissing = s.assets.darwin.missing.length > 0;
  const admitted = s.admission?.admitted === true;
  if (s.release.isDraft) {
    if (darwinMissing || s.admission?.admitted === false) return "prepare-darwin";
    if (!admitted) return "blocked:darwin-admission-unknown";
    return "publish-draft";
  }
  // Published: build-darwin-local.sh uploads only to a draft, so missing or
  // rejected Darwin inputs on a published Release need a human decision.
  if (darwinMissing) return "blocked:darwin-inputs-missing-on-published-release";
  if (s.admission?.admitted === false) return "blocked:darwin-inputs-rejected";
  if (!admitted) return "blocked:darwin-admission-unknown";
  const npm = s.npm.summary;
  if (npm.uncertain.length) return "blocked:npm-uncertain";
  if (npm["present-unverified"].length) return "blocked:npm-unverified";
  if (npm.absent.length || s.assets.ci.missing.length) return "dispatch-recovery";
  if (s.assets.mdWasm.missing.length || !s.distTags.settled) return "dispatch-finalize";
  return "complete";
}

function resolveTag({ tag, repo, gh }) {
  let ref;
  try {
    ref = JSON.parse(gh(["api", `repos/${repo}/git/ref/tags/${tag}`]));
  } catch (error) {
    if (error instanceof GhError && error.notFound) return null;
    throw error;
  }
  let object = ref.object;
  for (let depth = 0; object?.type === "tag" && depth < 5; depth++) {
    object = JSON.parse(gh(["api", `repos/${repo}/git/tags/${object.sha}`])).object;
  }
  if (object?.type !== "commit") throw new Error(`tag ${tag} does not peel to a commit`);
  const commit = JSON.parse(gh(["api", `repos/${repo}/git/commits/${object.sha}`]));
  return { commit: object.sha, tree: commit.tree?.sha ?? null };
}

function readRelease({ tag, repo, gh }) {
  try {
    return JSON.parse(
      gh([
        "release",
        "view",
        tag,
        "--repo",
        repo,
        "--json",
        "tagName,isDraft,isPrerelease,publishedAt,targetCommitish,url,assets",
      ]),
    );
  } catch (error) {
    if (error instanceof GhError && error.notFound) return null;
    throw error;
  }
}

function admitFromRelease({ tag, version, repo, gh, tagInfo, repoRoot, darwinNames }) {
  const dir = mkdtempSync(join(tmpdir(), "zfb-release-status-darwin-"));
  try {
    gh([
      "release",
      "download",
      tag,
      "--repo",
      repo,
      "--dir",
      dir,
      ...darwinNames.flatMap((name) => ["--pattern", name]),
    ]);
    const source = sourceInputs(repoRoot, tagInfo.commit, tagInfo.tree);
    if (source.errors?.length) return { admitted: null, errors: source.errors };
    return admitDarwinArtifacts({
      dir,
      version,
      sourceCommit: tagInfo.commit,
      sourceTree: tagInfo.tree,
      inputs: source.inputs,
    });
  } catch (error) {
    return { admitted: null, errors: [error.message] };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

async function readDistTags({ request, registry }) {
  const tags = {};
  for (const name of PUBLISHED_PACKAGES) {
    const path = name.startsWith("@") ? `@${encodeURIComponent(name.slice(1))}` : name;
    try {
      const response = await request(`${registry}/-/package/${path}/dist-tags`);
      if (response.status !== 200) throw new Error(`HTTP ${response.status}`);
      tags[name] = { tags: JSON.parse(response.body.toString("utf8")) };
    } catch (error) {
      tags[name] = { error: error.message };
    }
  }
  return tags;
}

/**
 * Pure: is this release.yml run for exactly `tag`? release.yml's run-name is
 * `Release <tag>[ (finalize only)][ (dry run)]`, so main-based dispatches match
 * by title; release:published runs also carry headBranch = tag. Exact, never a
 * substring (v6.1.1 must not match v6.1.10 or v6.1.1-next.1).
 */
export function runMatchesTag(run, tag) {
  if (run.headBranch === tag) return true;
  const escaped = tag.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`^Release ${escaped}( \\(finalize only\\))?( \\(dry run\\))?$`).test(
    String(run.displayTitle ?? ""),
  );
}

function readRuns({ tag, repo, gh }) {
  const runs = JSON.parse(
    gh([
      "run",
      "list",
      "--repo",
      repo,
      "--workflow",
      "release.yml",
      "--limit",
      "50",
      "--json",
      "databaseId,attempt,status,conclusion,event,headBranch,headSha,displayTitle,createdAt,url",
    ]),
  );
  return runs.filter((run) => runMatchesTag(run, tag)).slice(0, 5);
}

function probeCapabilities({ repo, gh }) {
  const probe = (fn) => {
    try {
      return { ok: true, value: fn() };
    } catch (error) {
      return { ok: false, error: error.message };
    }
  };
  return {
    ghAuth: probe(() => gh(["auth", "status"]).trim().split("\n").slice(0, 3).join(" | ")),
    repoPermissions: probe(() => JSON.parse(gh(["api", `repos/${repo}`, "--jq", ".permissions"]))),
    actionsReadable: probe(
      () => JSON.parse(gh(["api", `repos/${repo}/actions/workflows/release.yml`])).state,
    ),
  };
}

export async function collectStatus({
  tag,
  repo,
  gh = execGh,
  request = defaultRequest,
  registry = DEFAULT_REGISTRY,
  repoRoot = process.cwd(),
  probe = false,
  absentReads = 3,
  delayMs = 2000,
}) {
  const version = tag.slice(1);
  const status = { schema: "zfb-release-status/v1", tag, version, repo };
  status.tag = { name: tag, ...(resolveTag({ tag, repo, gh }) ?? {}) };
  const release = readRelease({ tag, repo, gh });
  status.source = resolveSource({ tagInfo: status.tag, release });
  if (status.source.kind === "draft-target") {
    status.source.tree =
      JSON.parse(gh(["api", `repos/${repo}/git/commits/${status.source.commit}`])).tree?.sha ??
      null;
  }
  const sourceCommit = status.source.commit ?? null;
  status.release = release
    ? {
        exists: true,
        isDraft: release.isDraft,
        isPrerelease: release.isPrerelease,
        publishedAt: release.publishedAt || null,
        targetCommitish: release.targetCommitish,
        url: release.url,
        targetMismatch:
          status.source.kind === "tag" &&
          /^[0-9a-f]{40}$/.test(release.targetCommitish ?? "") &&
          release.targetCommitish !== status.tag.commit,
      }
    : null;
  status.assets = assetInventory(version, release?.assets ?? []);
  if (sourceCommit) {
    try {
      status.sourceCi = evaluateSourceCi({
        sha: sourceCommit,
        ...readSourceCi({ sha: sourceCommit, repo, gh }),
      });
    } catch (error) {
      status.sourceCi = null;
      status.sourceCiError = error.message;
    }
  }
  status.admission = null;
  if (release && sourceCommit && status.source.tree && status.assets.darwin.missing.length === 0) {
    status.admission = admitFromRelease({
      tag,
      version,
      repo,
      gh,
      tagInfo: status.source,
      repoRoot,
      darwinNames: expectedReleaseAssets(version).darwin,
    });
  }
  status.npm = sourceCommit
    ? await collectStates({
        version,
        sourceCommit,
        verdict: status.admission?.admitted ? status.admission : null,
        registry,
        request,
        absentReads,
        delayMs,
      })
    : null;
  const tags = await readDistTags({ request, registry });
  status.distTags = { packages: tags, ...distTagsSettled(version, tags) };
  try {
    status.runs = readRuns({ tag, repo, gh });
  } catch (error) {
    status.runs = [];
    status.runsError = error.message;
  }
  if (probe) status.capabilities = probeCapabilities({ repo, gh });
  status.recommendedAction = recommendNextAction(status);
  return status;
}

export function render(status) {
  const out = [];
  out.push(
    `release ${status.tag.name}: source ${status.source.kind ?? `none (${status.source.problem})`} commit ${status.source.commit ?? "∅"} tree ${status.source.tree ?? "∅"}`,
  );
  out.push(
    status.release
      ? `  Release: ${status.release.isDraft ? "draft" : "published"} target=${status.release.targetCommitish} ${status.release.url}`
      : "  Release: missing",
  );
  for (const group of ["ci", "darwin", "mdWasm"]) {
    const g = status.assets[group];
    out.push(`  assets ${group}: ${g.present.length} present, missing [${g.missing.join(", ")}]`);
  }
  if (status.assets.conflicting.length)
    out.push(`  assets conflicting: ${status.assets.conflicting.join(", ")}`);
  out.push(
    status.sourceCi
      ? `  source CI: ${status.sourceCi.passed ? "passed" : `NOT passed (${status.sourceCi.blocking.join("; ")})`}`
      : `  source CI: unknown ${status.sourceCiError ?? ""}`,
  );
  out.push(
    `  Darwin admission: ${status.admission ? (status.admission.admitted === true ? "admitted" : status.admission.admitted === false ? `rejected (${status.admission.errors.slice(0, 3).join("; ")})` : `unknown (${status.admission.errors.join("; ")})`) : "not evaluated"}`,
  );
  if (status.npm) {
    for (const result of status.npm.packages) {
      out.push(
        `  npm ${result.state.padEnd(19)} ${result.name}  ${result.error ?? result.detail ?? result.method ?? ""}`,
      );
    }
  }
  out.push(
    `  dist-tags: ${status.distTags.settled ? "settled" : status.distTags.problems.join("; ")}`,
  );
  for (const run of status.runs) {
    out.push(
      `  run ${run.databaseId} attempt ${run.attempt} ${run.event} ${run.status}/${run.conclusion ?? "-"} ${run.url}`,
    );
  }
  if (status.capabilities) out.push(`  capabilities: ${JSON.stringify(status.capabilities)}`);
  out.push(`recommended next action: ${status.recommendedAction}`);
  return `${out.join("\n")}\n`;
}

export async function main(
  argv,
  { gh = execGh, env = process.env, out = process.stdout, request } = {},
) {
  const opts = { json: false, probe: false };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--json") opts.json = true;
    else if (arg === "--probe-capabilities") opts.probe = true;
    else if (["--tag", "--repo", "--repo-root"].includes(arg) && argv[i + 1])
      opts[arg.slice(2)] = argv[++i];
    else opts.bad = true;
  }
  if (
    opts.bad ||
    !/^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-(next|beta|rc)\.(0|[1-9]\d*))?$/.test(
      opts.tag ?? "",
    )
  ) {
    process.stderr.write(
      "usage: release-status.mjs --tag vX.Y.Z [--json] [--probe-capabilities] [--repo owner/name] [--repo-root DIR]\n",
    );
    return 2;
  }
  let status;
  try {
    status = await collectStatus({
      tag: opts.tag,
      repo: opts.repo ?? repoFromEnv(env),
      gh,
      request: request ?? defaultRequest,
      registry: env.ZFB_NPM_REGISTRY ?? DEFAULT_REGISTRY,
      repoRoot: opts["repo-root"] ?? process.cwd(),
      probe: opts.probe,
    });
  } catch (error) {
    process.stderr.write(`release-status: ${error.message}\n`);
    return 2;
  }
  out.write(opts.json ? `${JSON.stringify(status, null, 2)}\n` : render(status));
  return 0;
}

export { compareSemver };

if (isMain(import.meta.url)) {
  process.exitCode = await main(process.argv.slice(2));
}
