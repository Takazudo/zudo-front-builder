#!/usr/bin/env node
// coordinate-release.mjs — the Actions release coordinator (#4701).
//
// For a caller that cannot push tags (Claude Code Cloud's git proxy refuses
// them; its `gh` may be unauthenticated), `.github/workflows/release-coordinator.yml`
// runs these subcommands with the run's own GITHUB_TOKEN:
//
//   gate      read-only. Re-derives the release state with release-status.mjs
//             and every candidate contract, then decides one plan:
//               start   draft Release at the candidate, Step 0 = publish-draft
//               resume  tag/Release already at the candidate, no release.yml
//                       run yet (an interrupted coordinator)
//               reuse   a release.yml run for exactly this tag + SHA is active
//                       or succeeded: nothing to do
//             anything else stops with a reason (exit 1).
//   tag       gate, then create refs/tags/v<version> at the candidate through
//             the REST API (create-only). Same name + same SHA = resume; a tag
//             at another commit is refused. Never moves or deletes a tag.
//   publish   gate, then publish the draft Release in one PATCH without
//             GitHub Latest (make_latest "false"). A published Release = no-op.
//   dispatch  gate, then workflow_dispatch release.yml at ref v<version>
//             (dry_run=false and NO other input), unless a run for this tag +
//             SHA is already active or succeeded. Reads the created run back.
//
//   node scripts/release/coordinate-release.mjs <gate|tag|publish|dispatch>
//        --version X.Y.Z --sha <40-hex> [--repo owner/name] [--repo-root DIR]
//        [--json PATH]
//
// Why release.yml is dispatched at the TAG: a Release published and a tag
// created by GITHUB_TOKEN do not start workflows; workflow_dispatch is the
// documented exception. Dispatching at refs/tags/v<version> runs the source's
// own release.yml (control SHA = source SHA), exactly like `release: published`,
// so the npm trusted publisher (workflow release.yml, no environment) and the
// honest mac-local provenance mode are unchanged. A main-based dispatch would be
// a recovery (control != source) and is NOT what this coordinator does.
//
// Owner-only decisions stay owner-only: this file never sets
// allow_provenance_downgrade, never edits a Release body and never writes
// release/provenance-policy.json. An unacknowledged downgrade stops the gate
// before anything is written. Missing or rejected Mac inputs stop it too.
//
// Exit codes: 0 done (or nothing to do), 1 refused/stopped (reason printed),
// 2 usage error, 3 inconclusive (registry/GitHub read failed; never a "no").
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { writeFileSync, appendFileSync } from "node:fs";
import {
  DEFAULT_POLICY_REF,
  MAC_LOCAL_PACKAGES,
  applyPolicy,
  checkMode,
  loadPolicyAtRef,
} from "../check-provenance-preflight.mjs";
import { PUBLISHED_PACKAGES } from "../retire-next-dist-tag.mjs";
import { GhError, execGh, isMain, parseJsonLines, repoFromEnv } from "./gh.mjs";
import { collectStatus } from "./release-status.mjs";

export const VERSION_RE =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-(next|beta|rc)\.(0|[1-9]\d*))?$/;
export const SHA_RE = /^[0-9a-f]{40}$/;
export const DOWNGRADE_MARKER = "<!-- zfb-release: allow-provenance-downgrade -->";

// Contract markers a candidate's own workflows must carry. release.yml at the
// tag is what the coordinator dispatches, so an older candidate whose
// release.yml cannot read the owner's marker on a tag dispatch (or whose
// installer smoke cannot be dispatched) is refused instead of half-supported.
export const CANDIDATE_CONTRACTS = [
  {
    path: ".github/workflows/release.yml",
    marker: "zfb-release-coordinator: tag-dispatch-v1",
  },
  {
    path: ".github/workflows/node-free-smoke.yml",
    marker: "zfb-release-coordinator: installer-dispatch-v1",
  },
];

// Workspace path of each lockstep package (PUBLISHED_PACKAGES is the source of
// truth for the set; a drift between the two fails the gate loudly).
export const PACKAGE_MANIFESTS = {
  "@takazudo/zfb-darwin-arm64": "packages/zfb-darwin-arm64/package.json",
  "@takazudo/zfb-darwin-x64": "packages/zfb-darwin-x64/package.json",
  "@takazudo/zfb-linux-arm64-gnu": "packages/zfb-linux-arm64-gnu/package.json",
  "@takazudo/zfb-linux-x64-gnu": "packages/zfb-linux-x64-gnu/package.json",
  "@takazudo/zfb-win32-x64-msvc": "packages/zfb-win32-x64-msvc/package.json",
  "@takazudo/zfb-slugify": "packages/zfb-slugify/package.json",
  "@takazudo/zfb": "packages/zfb/package.json",
  "@takazudo/zfb-runtime": "packages/zfb-runtime/package.json",
  "@takazudo/zfb-adapter-cloudflare": "packages/zfb-adapter-cloudflare/package.json",
  "create-zfb": "packages/create-zfb/package.json",
  "@takazudo/zfb-md-wasm": "crates/zfb-md-wasm/npm/package.json",
};

export const CHANGELOG_LANES = [
  "zfb",
  "zfb-runtime",
  "zfb-adapter-cloudflare",
  "create-zfb",
  "zfb-md-wasm",
];

export const SHIPPED_SIZES_PATH = "crates/zfb-md-wasm/shipped-sizes.json";

export class CoordinatorError extends Error {
  constructor(message, { code, exitCode = 1, details } = {}) {
    super(message);
    this.code = code;
    this.exitCode = exitCode;
    this.details = details;
  }
}

const stop = (code, message, details) => new CoordinatorError(message, { code, details });
const inconclusive = (code, message, details) =>
  new CoordinatorError(message, { code, exitCode: 3, details });

// ── default I/O (every piece is injectable for the tests) ───────────────────

export function defaultIo({ repo, repoRoot = process.cwd(), env = process.env } = {}) {
  return {
    repo,
    repoRoot,
    env,
    gh: execGh,
    git: (args) =>
      execFileSync("git", ["-C", repoRoot, ...args], {
        encoding: "utf8",
        stdio: ["ignore", "pipe", "pipe"],
        maxBuffer: 64 * 1024 * 1024,
      }),
    collectStatus,
    checkProvenance: (version) => checkMode("mac-local", { version, packages: MAC_LOCAL_PACKAGES }),
    loadPolicy: () => loadPolicyAtRef({ repo: repoRoot, ref: DEFAULT_POLICY_REF }),
    sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
    now: () => Date.now(),
    log: (line) => process.stdout.write(`${line}\n`),
  };
}

// ── pure helpers ────────────────────────────────────────────────────────────

export function validateInputs({ version, sha }) {
  if (!VERSION_RE.test(version ?? "")) {
    throw new CoordinatorError(`version '${version}' is not an allowed X.Y.Z[-next|beta|rc.N]`, {
      code: "USAGE",
      exitCode: 2,
    });
  }
  if (!SHA_RE.test(sha ?? "")) {
    throw new CoordinatorError(
      `sha '${sha}' must be the full 40-character lowercase commit SHA (never a branch or short SHA)`,
      { code: "USAGE", exitCode: 2 },
    );
  }
  return { version, sha, tag: `v${version}` };
}

/** Pure: exact-line marker match, CRLF tolerant (web-UI bodies are CRLF). */
export function bodyHasDowngradeMarker(body) {
  return String(body ?? "")
    .replace(/\r/g, "")
    .split("\n")
    .some((line) => line === DOWNGRADE_MARKER);
}

/**
 * Pure: is this release.yml run a real (non-dry-run) run for exactly `tag`?
 * Finalize-only runs count too: they prove the release already moved on.
 */
export function isRealRunForTag(run, tag) {
  const escaped = tag.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const title = String(run.displayTitle ?? "");
  if (/\(dry run\)$/.test(title)) return false;
  return new RegExp(`^Release ${escaped}( \\(finalize only\\))?$`).test(title);
}

/**
 * Pure: decide the coordinator plan from release-status's report and the
 * candidate. Returns { plan, reason } where plan is start|resume|reuse|stop.
 */
export function decidePlan(status, { sha, tag }) {
  const source = status.source ?? {};
  if (status.tag?.commit && status.tag.commit !== sha) {
    return {
      plan: "stop",
      reason: `TAG_ELSEWHERE: ${tag} already peels to ${status.tag.commit}, not the candidate ${sha}. Tags are never moved; pick the candidate the tag names or a new version.`,
    };
  }
  if (!status.release) {
    return {
      plan: "stop",
      reason: `RELEASE_MISSING: no Release exists for ${tag}. Create the draft first (RELEASING.md A10: gh release create ${tag} --draft --target ${sha}).`,
    };
  }
  if (source.commit !== sha) {
    return {
      plan: "stop",
      reason: `SOURCE_MISMATCH: the ${tag} Release resolves to ${source.commit ?? `nothing (${source.problem})`}, not the candidate ${sha}. The coordinator never substitutes a SHA.`,
    };
  }
  const runs = (status.runs ?? []).filter((run) => isRealRunForTag(run, tag));
  // A dispatch from main (recovery or finalize) builds the tag's commit but its
  // head SHA is main's: it means the release already went past a first run.
  const fromMain = runs.filter((run) => run.headBranch !== tag);
  if (fromMain.length) {
    return {
      plan: "stop",
      reason: `RECOVERY_STARTED: release.yml run ${fromMain[0].databaseId} for ${tag} was dispatched from ${fromMain[0].headBranch} (a recovery or finalize run). The coordinator only starts a first run; follow release-status.mjs (${status.recommendedAction}).`,
    };
  }
  const exact = runs.filter((run) => run.headSha === sha);
  const other = runs.filter((run) => run.headSha !== sha);
  if (other.length) {
    return {
      plan: "stop",
      reason: `RUN_FOR_OTHER_SOURCE: release.yml run ${other[0].databaseId} for ${tag} built ${other[0].headSha}, not ${sha}. Investigate before anything else.`,
    };
  }
  const tagDispatched = exact.filter(
    (run) => run.event === "workflow_dispatch" && run.headBranch === tag,
  );
  if (exact.some((run) => run.status !== "completed")) {
    return { plan: "reuse", reason: `release.yml run ${exact[0].databaseId} is in progress` };
  }
  const success = exact.find((run) => run.conclusion === "success");
  if (success) {
    return {
      plan: "reuse",
      reason: `release.yml run ${success.databaseId} for ${tag} at ${sha} already succeeded`,
    };
  }
  if (exact.length) {
    return {
      plan: "stop",
      reason: `RUN_FAILED: release.yml run ${exact[0].databaseId} (${exact[0].event}, ${exact[0].conclusion}${tagDispatched.length ? "" : ", not a tag dispatch"}) already ran for ${tag}. The coordinator does not re-run a failed release; follow release-status.mjs (${status.recommendedAction}).`,
    };
  }
  // No real run exists yet.
  if (status.release.isDraft) {
    if (status.recommendedAction !== "publish-draft") {
      return {
        plan: "stop",
        reason: `NOT_READY: release-status recommends ${status.recommendedAction}, not publish-draft.${status.recommendedAction === "prepare-darwin" ? " The owner's Mac must build and upload the Darwin inputs (RELEASING.md Phase M); Cloud cannot." : ""}`,
      };
    }
    return {
      plan: status.tag?.commit ? "resume" : "start",
      reason: "draft at the candidate, Step 0 = publish-draft",
    };
  }
  // Published, never run: an interrupted coordinator between publish and
  // dispatch. Every pre-publication gate must still hold, and nothing may be on
  // npm yet (anything else is recovery territory, not a first run).
  const problems = [];
  if (!status.sourceCi?.passed) problems.push("source CI is not passed");
  if (status.admission?.admitted !== true) problems.push("Darwin admission is not admitted");
  if (status.assets?.conflicting?.length) problems.push("Release assets conflict");
  const npmSummary = status.npm?.summary ?? {};
  const notAbsent = PUBLISHED_PACKAGES.length - (npmSummary.absent?.length ?? 0);
  if (notAbsent !== 0) problems.push(`${notAbsent} package(s) are not absent on npm`);
  if (problems.length) {
    return {
      plan: "stop",
      reason: `PUBLISHED_NOT_RESUMABLE: ${tag} is published with no release.yml run, but ${problems.join("; ")}. Follow release-status.mjs (${status.recommendedAction}).`,
    };
  }
  return { plan: "resume", reason: "published at the candidate, no release.yml run yet" };
}

// ── candidate contract (git objects of the candidate, never a working tree) ─

function gitShow(io, sha, path) {
  try {
    return io.git(["show", `${sha}:${path}`]);
  } catch {
    return null;
  }
}

export function checkCandidate(io, { version, sha }) {
  const errors = [];
  try {
    io.git(["cat-file", "-e", `${sha}^{commit}`]);
  } catch {
    throw inconclusive(
      "CANDIDATE_NOT_FETCHED",
      `candidate ${sha} is not in ${io.repoRoot}; fetch main with full history first`,
    );
  }
  let mainHead;
  try {
    mainHead = io.git(["rev-parse", "refs/remotes/origin/main^{commit}"]).trim();
  } catch {
    throw inconclusive("MAIN_NOT_FETCHED", "refs/remotes/origin/main is not available");
  }
  try {
    io.git(["merge-base", "--is-ancestor", sha, mainHead]);
  } catch {
    errors.push(`candidate ${sha} is not on main (${mainHead}); only commits on main are released`);
  }
  const names = Object.keys(PACKAGE_MANIFESTS).sort();
  if (JSON.stringify(names) !== JSON.stringify([...PUBLISHED_PACKAGES].sort())) {
    errors.push("PACKAGE_MANIFESTS drifted from PUBLISHED_PACKAGES (fix the coordinator)");
  }
  for (const [name, path] of Object.entries(PACKAGE_MANIFESTS)) {
    const text = gitShow(io, sha, path);
    let manifest;
    try {
      manifest = text && JSON.parse(text);
    } catch {
      manifest = null;
    }
    if (!manifest) errors.push(`${path} is missing or unreadable at the candidate`);
    else if (manifest.name !== name) errors.push(`${path} names ${manifest.name}, not ${name}`);
    else if (manifest.version !== version)
      errors.push(`${name} is ${manifest.version} at the candidate, not ${version}`);
  }
  for (const lane of CHANGELOG_LANES) {
    const path = `docs/src/content/docs/changelog/${lane}/v${version}.mdx`;
    if (gitShow(io, sha, path) === null) errors.push(`changelog lane page ${path} is missing`);
  }
  const sizesText = gitShow(io, sha, SHIPPED_SIZES_PATH);
  let measured = null;
  try {
    measured = sizesText ? JSON.parse(sizesText).measuredOnVersion : null;
  } catch {
    measured = null;
  }
  if (measured !== version) {
    errors.push(
      `${SHIPPED_SIZES_PATH} measuredOnVersion is ${measured}, not ${version} (A4 measurement missing)`,
    );
  }
  for (const contract of CANDIDATE_CONTRACTS) {
    const text = gitShow(io, sha, contract.path);
    if (!text || !text.includes(contract.marker)) {
      errors.push(
        `${contract.path} at the candidate lacks '${contract.marker}': it predates the coordinator; release it through the local route (RELEASING.md A12)`,
      );
    }
  }
  return { mainHead, candidateIsMainHead: mainHead === sha, errors };
}

// ── provenance (owner acknowledgement is read, never written) ───────────────

export async function checkProvenanceAck(io, { version, releaseBody }) {
  let policy;
  try {
    policy = io.loadPolicy();
  } catch (error) {
    throw inconclusive(
      "POLICY_UNREADABLE",
      `provenance policy unreadable, failing closed: ${error.message}`,
    );
  }
  const raw = await io.checkProvenance(version);
  const results = applyPolicy(raw, {
    mode: "mac-local",
    version,
    policy,
    localBuilt: MAC_LOCAL_PACKAGES,
  });
  const errors = results.filter((r) => r.status === "error");
  if (errors.length) {
    throw inconclusive(
      "REGISTRY_ERROR",
      `provenance preflight could not read the registry (${errors.map((e) => `${e.name}: ${e.detail}`).join("; ")}); failing closed`,
    );
  }
  const downgrades = results.filter((r) => r.status === "downgrade");
  const marker = bodyHasDowngradeMarker(releaseBody);
  const summary = results.map((r) => ({
    name: r.name,
    status: r.status,
    earlierAttested: r.earlierAttested ?? null,
  }));
  if (downgrades.length && !marker) {
    throw stop(
      "OWNER_ACK_REQUIRED",
      `provenance trust downgrade for ${downgrades.map((d) => `${d.name}@${version} (earlier attested ${d.earlierAttested})`).join(", ")} is not acknowledged. Only the owner may acknowledge it: the exact line '${DOWNGRADE_MARKER}' in the draft Release body, or a committed release/provenance-policy.json on main. Nothing was written.`,
      { provenance: summary },
    );
  }
  return {
    results: summary,
    acknowledgement: downgrades.length
      ? "release-body-marker"
      : results.some((r) => r.status === "policy-acknowledged")
        ? "committed-policy"
        : "none-needed",
  };
}

// ── gate ─────────────────────────────────────────────────────────────────────

export async function gate(io, inputs) {
  const { version, sha, tag } = validateInputs(inputs);
  const env = io.env ?? {};
  if (env.GITHUB_ACTIONS === "true" && env.GITHUB_REF && env.GITHUB_REF !== "refs/heads/main") {
    throw stop(
      "CONTROL_NOT_MAIN",
      `the coordinator must be dispatched from refs/heads/main, got ${env.GITHUB_REF}`,
    );
  }
  const candidate = checkCandidate(io, { version, sha });
  if (candidate.errors.length) {
    throw stop(
      "CANDIDATE_INVALID",
      `candidate ${sha} fails the release contract:\n- ${candidate.errors.join("\n- ")}`,
    );
  }
  if (!candidate.candidateIsMainHead) {
    io.log(
      `::notice::main advanced to ${candidate.mainHead}; the frozen candidate ${sha} is kept (never substituted).`,
    );
  }
  let status;
  try {
    status = await io.collectStatus({ tag, repo: io.repo, gh: io.gh, repoRoot: io.repoRoot });
  } catch (error) {
    throw inconclusive(
      "STATUS_UNREADABLE",
      `release-status could not be collected: ${error.message}`,
    );
  }
  // release-status keeps only the newest 5 matching runs out of 50 (dry runs
  // included), which can hide an older failed real run. The coordinator lists
  // the tag's runs itself, filters dry runs before any limit, and treats a
  // full page as inconclusive rather than as "never ran".
  let runs;
  try {
    runs = listRealRuns(io, tag);
  } catch (error) {
    if (error instanceof CoordinatorError) throw error;
    throw inconclusive("RUNS_UNREADABLE", `release.yml runs are unreadable: ${error.message}`);
  }
  status = { ...status, runs };
  const decision = decidePlan(status, { sha, tag });
  const result = {
    schema: "zfb-release-coordinator/v1",
    tag,
    version,
    candidate: sha,
    sourceTree: status.source?.tree ?? null,
    mainHead: candidate.mainHead,
    control: {
      sha: env.GITHUB_SHA ?? null,
      workflowRef: env.GITHUB_WORKFLOW_REF ?? null,
      runId: env.GITHUB_RUN_ID ?? null,
    },
    recommendedAction: status.recommendedAction,
    release: status.release,
    tagCommit: status.tag?.commit ?? null,
    plan: decision.plan,
    reason: decision.reason,
  };
  if (decision.plan === "stop") {
    throw stop(decision.reason.split(":")[0], decision.reason, result);
  }
  if (decision.plan !== "reuse") {
    const body = readReleaseByTag(io, tag)?.body ?? "";
    // Logged so a reader can compare it with the digest release.yml logs when
    // it reads the same body on the tag dispatch (both CR-stripped).
    result.releaseBodySha256 = bodyDigest(body);
    io.log(`gated Release body sha256 (CR-stripped): ${result.releaseBodySha256}`);
    result.provenance = await checkProvenanceAck(io, { version, releaseBody: body });
  }
  return result;
}

/** Pure: the digest both the coordinator and release.yml log for the body. */
export function bodyDigest(body) {
  return (
    createHash("sha256")
      // CR-stripped and without trailing newlines: the same bytes release.yml
      // hashes after `$(gh release view ... --jq .body)` drops them.
      .update(
        String(body ?? "")
          .replace(/\r/g, "")
          .replace(/\n+$/, ""),
      )
      .digest("hex")
  );
}

// Every Release, drafts included (only push-access tokens see drafts), so an
// older draft is never misread as missing.
function readReleaseByTag(io, tag) {
  const releases = parseJsonLines(
    io.gh([
      "api",
      "--paginate",
      `repos/${io.repo}/releases?per_page=100`,
      "--jq",
      ".[] | {id, tag_name, draft, prerelease, body, target_commitish}",
    ]),
  );
  return releases.find((r) => r.tag_name === tag) ?? null;
}

// ── tag ──────────────────────────────────────────────────────────────────────

function remoteTagCommit(io, tag) {
  let ref;
  try {
    ref = JSON.parse(io.gh(["api", `repos/${io.repo}/git/ref/tags/${tag}`]));
  } catch (error) {
    if (error instanceof GhError && error.notFound) return null;
    throw error;
  }
  let object = ref.object;
  for (let depth = 0; object?.type === "tag" && depth < 5; depth++) {
    object = JSON.parse(io.gh(["api", `repos/${io.repo}/git/tags/${object.sha}`])).object;
  }
  return object?.type === "commit" ? object.sha : `non-commit:${object?.type}`;
}

/**
 * Observations recorded next to a refused tag write. They are facts about the
 * candidate and control commits, NOT a diagnosis: the cause of an earlier 403
 * elsewhere (zdtp) was never established, so nothing here claims one.
 */
export function tagFailureDiagnostics(io, { sha, error }) {
  const env = io.env ?? {};
  const stderr = String(error?.stderr ?? error?.message ?? "");
  const stdout = String(error?.stdout ?? "");
  const diag = {
    githubResponse: stderr.slice(0, 2000),
    responseHead: stdout.split(/\r?\n\r?\n/)[0].slice(0, 2000) || null,
    httpStatus:
      /HTTP\/[\d.]+ (\d{3})/.exec(stdout)?.[1] ?? /HTTP (\d{3})/.exec(stderr)?.[1] ?? null,
    requestId: /x-github-request-id:\s*([A-Za-z0-9:]+)/i.exec(stdout)?.[1] ?? null,
    candidateSha: sha,
    controlSha: env.GITHUB_SHA ?? null,
    workflowRef: env.GITHUB_WORKFLOW_REF ?? null,
    workflowSha: env.GITHUB_WORKFLOW_SHA ?? null,
    runId: env.GITHUB_RUN_ID ?? null,
    runAttempt: env.GITHUB_RUN_ATTEMPT ?? null,
  };
  const tree = (rev) => {
    try {
      return io.git(["rev-parse", `${rev}:.github/workflows`]).trim();
    } catch {
      return null;
    }
  };
  diag.candidateWorkflowsTree = tree(sha);
  diag.controlWorkflowsTree = diag.controlSha ? tree(diag.controlSha) : null;
  diag.workflowsTreeDiffers =
    diag.candidateWorkflowsTree && diag.controlWorkflowsTree
      ? diag.candidateWorkflowsTree !== diag.controlWorkflowsTree
      : null;
  return diag;
}

export async function createTag(io, inputs, { readbackAttempts = 6, intervalMs = 5000 } = {}) {
  const g = await gate(io, inputs);
  const { candidate: sha, tag } = g;
  if (g.plan === "reuse") return { ...g, tagAction: "none (run already exists)" };
  const existing = remoteTagCommit(io, tag);
  if (existing === sha) return { ...g, tagAction: "exists-at-candidate (resume)" };
  if (existing)
    throw stop("TAG_ELSEWHERE", `${tag} already exists at ${existing}, not ${sha}; never moved`);
  try {
    io.gh(["api", "-X", "POST", `repos/${io.repo}/git/refs`, "--include", "--input", "-"], {
      input: JSON.stringify({ ref: `refs/tags/${tag}`, sha }),
    });
  } catch (error) {
    // A concurrent coordinator may have won the race: read back before failing.
    const now = remoteTagCommit(io, tag);
    if (now === sha) return { ...g, tagAction: "created-concurrently (resume)" };
    if (now) throw stop("TAG_ELSEWHERE", `${tag} appeared at ${now}, not ${sha}; never moved`);
    const diagnostics = tagFailureDiagnostics(io, { sha, error });
    throw stop(
      "TAG_CREATE_FAILED",
      `GitHub refused to create ${tag} at ${sha} (HTTP ${diagnostics.httpStatus ?? "?"}). GitHub's response and the candidate/control/workflow SHAs are recorded below; the cause is NOT inferred. Do not add a PAT or bypass protection: report it to the owner, who can push the tag from a user credential (RELEASING.md A12) or choose another candidate.`,
      { ...g, diagnostics },
    );
  }
  for (let attempt = 0; attempt < readbackAttempts; attempt++) {
    const now = remoteTagCommit(io, tag);
    if (now === sha) return { ...g, tagAction: "created" };
    if (now) throw stop("TAG_ELSEWHERE", `${tag} reads back at ${now}, not ${sha}`);
    await io.sleep(intervalMs);
  }
  throw inconclusive(
    "TAG_READBACK_PENDING",
    `${tag} was created but is not readable yet; re-run (an identical tag is a no-op)`,
  );
}

// ── publish the draft (no Latest) ────────────────────────────────────────────

export async function publishRelease(io, inputs) {
  const g = await gate(io, inputs);
  const { candidate: sha, tag, version } = g;
  if (g.plan === "reuse") return { ...g, releaseAction: "none (run already exists)" };
  if (remoteTagCommit(io, tag) !== sha) {
    throw stop(
      "TAG_NOT_AT_CANDIDATE",
      `${tag} must exist at ${sha} before the draft is published (run the tag step)`,
    );
  }
  const release = readReleaseByTag(io, tag);
  if (!release) throw stop("RELEASE_MISSING", `no Release for ${tag}`);
  if (release.target_commitish !== sha) {
    throw stop(
      "RELEASE_TARGET_MISMATCH",
      `${tag} Release targets ${release.target_commitish}, not ${sha}`,
    );
  }
  if (!release.draft) return { ...g, releaseAction: "already-published" };
  const prerelease = version.includes("-");
  io.gh(["api", "-X", "PATCH", `repos/${io.repo}/releases/${release.id}`, "--input", "-"], {
    input: JSON.stringify({ draft: false, prerelease, make_latest: "false" }),
  });
  const after = JSON.parse(
    io.gh([
      "api",
      `repos/${io.repo}/releases/${release.id}`,
      "--jq",
      "{draft, tag_name, target_commitish, prerelease}",
    ]),
  );
  if (after.draft !== false || after.tag_name !== tag) {
    throw stop(
      "PUBLISH_READBACK",
      `${tag} reads back as draft=${after.draft} tag=${after.tag_name}`,
    );
  }
  if (remoteTagCommit(io, tag) !== sha) {
    throw stop(
      "TAG_MOVED",
      `${tag} no longer peels to ${sha} after publishing; stop and investigate`,
    );
  }
  let latest = null;
  try {
    latest = JSON.parse(
      io.gh(["api", `repos/${io.repo}/releases/latest`, "--jq", "{tag_name}"]),
    ).tag_name;
  } catch {
    latest = null;
  }
  if (latest === tag) {
    throw stop(
      "LATEST_TOO_EARLY",
      `${tag} became GitHub Latest before delivery was verified; report it (A15 owns the promotion)`,
    );
  }
  return { ...g, releaseAction: "published (not Latest)", githubLatest: latest };
}

// ── dispatch release.yml at the tag ──────────────────────────────────────────

export const RUN_LIST_LIMIT = 200;

/**
 * Every real (non-dry-run) release.yml run for exactly `tag`: the runs on the
 * tag ref (release: published and the coordinator's tag dispatch) plus the
 * dispatches from main (recovery/finalize, matched by their exact run name).
 * Dry runs are filtered out of complete pages only, and a page that reaches
 * the limit is inconclusive: an unseen older run must never read as "never ran".
 */
export function listRealRuns(io, tag) {
  const fields =
    "databaseId,attempt,status,conclusion,event,headBranch,headSha,displayTitle,createdAt,url";
  const list = (extra) =>
    JSON.parse(
      io.gh([
        "run",
        "list",
        "--repo",
        io.repo,
        "--workflow",
        "release.yml",
        ...extra,
        "--limit",
        String(RUN_LIST_LIMIT),
        "--json",
        fields,
      ]),
    );
  const onTag = list(["--branch", tag]);
  const fromMain = list(["--branch", "main", "--event", "workflow_dispatch"]);
  for (const [label, page] of [
    [`--branch ${tag}`, onTag],
    ["--branch main --event workflow_dispatch", fromMain],
  ]) {
    if (page.length >= RUN_LIST_LIMIT) {
      throw inconclusive(
        "RUNS_TRUNCATED",
        `release.yml run list (${label}) reached ${RUN_LIST_LIMIT} entries; older runs for ${tag} cannot be ruled out`,
      );
    }
  }
  const seen = new Set();
  return [...onTag, ...fromMain].filter((run) => {
    if (seen.has(run.databaseId) || !isRealRunForTag(run, tag)) return false;
    seen.add(run.databaseId);
    return true;
  });
}

export async function dispatchRelease(
  io,
  inputs,
  { readbackAttempts = 24, intervalMs = 5000 } = {},
) {
  const g = await gate(io, inputs);
  const { candidate: sha, tag } = g;
  if (g.plan === "reuse") return { ...g, dispatchAction: "reused", reason: g.reason };
  if (remoteTagCommit(io, tag) !== sha) {
    throw stop(
      "TAG_NOT_AT_CANDIDATE",
      `${tag} must exist at ${sha} before release.yml is dispatched`,
    );
  }
  const release = readReleaseByTag(io, tag);
  if (!release || release.draft) {
    throw stop(
      "RELEASE_NOT_PUBLISHED",
      `${tag} Release must be published (not Latest) before dispatch`,
    );
  }
  // Last look immediately before the write: a run that appeared since the gate
  // (an owner's web-UI publish, a slow listing of an earlier dispatch) wins.
  const late = listRealRuns(io, tag);
  if (late.length) {
    throw stop(
      "RUN_APPEARED",
      `release.yml run ${late[0].databaseId} for ${tag} appeared after the gate; not dispatching a second run. Re-send to reuse it, or follow release-status.mjs.`,
    );
  }
  const startedAt = io.now();
  // dry_run is passed explicitly (release.yml's default is false). No other
  // input: allow_provenance_downgrade stays owner-only and release_tag would
  // turn this into a main-based recovery.
  io.gh([
    "workflow",
    "run",
    "release.yml",
    "--repo",
    io.repo,
    "--ref",
    `refs/tags/${tag}`,
    "-f",
    "dry_run=false",
  ]);
  for (let attempt = 0; attempt < readbackAttempts; attempt++) {
    await io.sleep(intervalMs);
    const run = listRealRuns(io, tag).find(
      (r) =>
        r.event === "workflow_dispatch" &&
        r.headBranch === tag &&
        r.headSha === sha &&
        Date.parse(r.createdAt) >= startedAt - 60_000,
    );
    if (run) return { ...g, dispatchAction: "dispatched", run };
  }
  throw inconclusive(
    "DISPATCH_READBACK_PENDING",
    `release.yml was dispatched at ${tag} but its run is not listed yet; check \`gh run list --workflow release.yml --branch ${tag}\` before re-sending; a re-send reuses a listed run and refuses to dispatch over one that appears late`,
  );
}

// ── CLI ──────────────────────────────────────────────────────────────────────

const COMMANDS = { gate, tag: createTag, publish: publishRelease, dispatch: dispatchRelease };

export function parseArgs(argv) {
  const [command, ...rest] = argv;
  if (!COMMANDS[command]) return null;
  const opts = { command };
  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i];
    if (
      ["--version", "--sha", "--repo", "--repo-root", "--json"].includes(arg) &&
      rest[i + 1] !== undefined
    ) {
      opts[arg.slice(2)] = rest[++i];
    } else return null;
  }
  return opts;
}

function summarize(env, result, error) {
  if (!env.GITHUB_STEP_SUMMARY) return;
  const lines = ["### Release coordinator", ""];
  if (error) {
    lines.push(`**Stopped: ${error.code}**`, "", "```", error.message, "```");
    if (error.details?.diagnostics) {
      lines.push(
        "",
        "Diagnostics (observations, not a diagnosis):",
        "",
        "```json",
        JSON.stringify(error.details.diagnostics, null, 2),
        "```",
      );
    }
  } else {
    lines.push("| | |", "| --- | --- |");
    for (const key of [
      "tag",
      "candidate",
      "sourceTree",
      "mainHead",
      "plan",
      "reason",
      "tagAction",
      "releaseAction",
      "dispatchAction",
    ]) {
      if (result[key] !== undefined) lines.push(`| ${key} | \`${result[key]}\` |`);
    }
    lines.push(`| control | \`${result.control?.sha}\` (${result.control?.workflowRef}) |`);
    if (result.provenance)
      lines.push(`| provenance acknowledgement | \`${result.provenance.acknowledgement}\` |`);
    if (result.run) lines.push(`| release.yml run | ${result.run.url} |`);
  }
  appendFileSync(env.GITHUB_STEP_SUMMARY, `${lines.join("\n")}\n`);
}

export async function main(argv, { io: injected, env = process.env } = {}) {
  const opts = parseArgs(argv);
  if (!opts || !opts.version || !opts.sha) {
    process.stderr.write(
      "usage: coordinate-release.mjs <gate|tag|publish|dispatch> --version X.Y.Z --sha <40-hex> [--repo owner/name] [--repo-root DIR] [--json PATH]\n",
    );
    return 2;
  }
  const io =
    injected ??
    defaultIo({
      repo: opts.repo ?? repoFromEnv(env),
      repoRoot: opts["repo-root"] ?? process.cwd(),
      env,
    });
  try {
    const result = await COMMANDS[opts.command](io, { version: opts.version, sha: opts.sha });
    io.log(`${opts.command}: ${result.plan} — ${result.reason}`);
    for (const key of ["tagAction", "releaseAction", "dispatchAction"]) {
      if (result[key]) io.log(`  ${key}: ${result[key]}`);
    }
    if (result.provenance)
      io.log(`  provenance acknowledgement: ${result.provenance.acknowledgement}`);
    if (result.run) io.log(`  release.yml run: ${result.run.databaseId} ${result.run.url}`);
    if (opts.json) writeFileSync(opts.json, `${JSON.stringify(result, null, 2)}\n`);
    summarize(env, result, null);
    return 0;
  } catch (error) {
    if (!(error instanceof CoordinatorError)) {
      io.log(`::error::coordinator failed unexpectedly: ${error.message}`);
      return 3;
    }
    io.log(`::error::${error.code}: ${error.message.split("\n")[0]}`);
    io.log(error.message);
    if (error.details?.diagnostics) io.log(JSON.stringify(error.details.diagnostics, null, 2));
    if (opts.json)
      writeFileSync(
        opts.json,
        `${JSON.stringify({ error: error.code, message: error.message, details: error.details ?? null }, null, 2)}\n`,
      );
    summarize(env, null, error);
    return error.exitCode;
  }
}

if (isMain(import.meta.url)) {
  process.exitCode = await main(process.argv.slice(2));
}
