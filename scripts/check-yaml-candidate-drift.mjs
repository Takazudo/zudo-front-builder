#!/usr/bin/env node
import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";
/**
 * Pure comparison and reporting core for the YAML candidate watcher.
 *
 * In scope: newly published versions, new tags and GitHub Releases, heads of
 * every branch (including non-default branches), branch addition/deletion and
 * force-push/divergence, pending release-PR state, and archive/unarchive state.
 *
 * Out of scope: #2755 acceptance criteria 2-6 — Error::location()
 * compatibility, the 18-case differential JSON corpus, wasm32 plus
 * `pnpm test:md-wasm`, gzip-9 size deltas, transitive dependency/license
 * differences, and `cargo deny`. Those semantic checks live in the committed
 * `crates/zfb-content/tests/yaml_differential_harness.rs` harness.
 *
 * This watches the known candidate set only; it does not discover a brand-new
 * fork. CANDIDATE_DRIFT is evidence requiring triage against #2755. It never
 * means that #2755's semantic trigger fired.
 *
 * Baseline/snapshot schema (schemaVersion 1):
 * {
 *   schemaVersion: 1,
 *   checkedAt: ISO-8601 string,
 *   candidates: {
 *     [candidateName]: {
 *       crate: string,
 *       repo: "owner/repository",
 *       versions: string[],
 *       branches: { [branchName]: string | { sha: string } },
 *       tags: string[],
 *       releases: string[],
 *       pendingReleasePr: null | { number: number, state: "OPEN"|"MERGED"|"CLOSED" },
 *       archived: boolean,
 *       checkedAt: ISO-8601 string
 *     }
 *   }
 * }
 *
 * An observed branch may additionally use { sha, relation: "ahead"|"diverged" }.
 * `relation` is transient comparison evidence, not required in a persisted
 * baseline. A changed head without this evidence is an operational failure:
 * guessing would conceal a force-push as an ordinary new commit.
 */

export const SCHEMA_VERSION = 1;
export const NO_DRIFT = "no-drift";
export const CANDIDATE_DRIFT = "CANDIDATE_DRIFT";
export const OPERATIONAL_FAILURE = "operational-failure";

export const EXIT_CODES = Object.freeze({
  [NO_DRIFT]: 0,
  [CANDIDATE_DRIFT]: 10,
  [OPERATIONAL_FAILURE]: 1,
});

export const TRACKED_CANDIDATES = Object.freeze([
  "serde_yaml_ng",
  "serde_yml",
  "saphyr",
  "serde_norway",
  "noyalib",
  "noyalib-serde-yaml",
  "serde-saphyr",
]);

export const CANDIDATE_CONFIG = Object.freeze({
  serde_yaml_ng: { crate: "serde_yaml_ng", repo: "acatton/serde-yaml-ng" },
  serde_yml: { crate: "serde_yml", repo: "sebastienrousseau/serde_yml" },
  saphyr: { crate: "saphyr", repo: "saphyr-rs/saphyr" },
  serde_norway: { crate: "serde_norway", repo: "cafkafk/serde-norway" },
  noyalib: {
    crate: "noyalib",
    repo: "sebastienrousseau/noyalib",
    pendingReleasePr: null,
  },
  "noyalib-serde-yaml": {
    crate: "noyalib-serde-yaml",
    repo: "sebastienrousseau/noyalib-serde-yaml",
  },
  "serde-saphyr": { crate: "serde-saphyr", repo: "bourumir-wyngs/serde-saphyr" },
});

export const BASELINE_COMMENT =
  "Anti-gaming rule: refresh this baseline only as part of a recorded triage; never bump it merely to turn the lane green.";

const DEFAULT_BASELINE_URL = new URL("./yaml-candidate-baseline.json", import.meta.url);
const USER_AGENT =
  "zudo-front-builder-yaml-candidate-watch/1.0 (+https://github.com/Takazudo/zudo-front-builder/issues/2810)";
const DEFAULT_TIMEOUT_MS = 15_000;
const DEFAULT_RETRIES = 2;
const DEFAULT_CONCURRENCY = 4;

function sleep(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function retryDelay(response, attempt) {
  const retryAfter = response?.headers?.get("retry-after");
  if (retryAfter) {
    const seconds = Number(retryAfter);
    if (Number.isFinite(seconds)) return Math.max(0, seconds * 1_000);
    const date = Date.parse(retryAfter);
    if (!Number.isNaN(date)) return Math.max(0, date - Date.now());
  }
  return 250 * 2 ** attempt;
}

/** A dependency-free request primitive with timeout and retry semantics. */
export async function fetchJson(
  url,
  {
    fetchImpl = globalThis.fetch,
    headers = {},
    timeoutMs = DEFAULT_TIMEOUT_MS,
    retries = DEFAULT_RETRIES,
    sleepImpl = sleep,
  } = {},
) {
  let lastError;
  for (let attempt = 0; attempt <= retries; attempt += 1) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);
    let response;
    try {
      response = await fetchImpl(url, { headers, signal: controller.signal });
      if (!response.ok) {
        const error = new Error(`${response.status} ${response.statusText}`.trim());
        error.status = response.status;
        throw error;
      }
      return { data: await response.json(), headers: response.headers };
    } catch (error) {
      lastError = error;
      const retryable =
        error.name === "AbortError" ||
        response?.headers?.has("retry-after") ||
        error.status === 429 ||
        error.status >= 500 ||
        error.status === undefined;
      if (!retryable || attempt === retries) break;
      await sleepImpl(retryDelay(response, attempt));
    } finally {
      clearTimeout(timer);
    }
  }
  throw new Error(`request failed for ${url}: ${lastError?.message ?? "unknown error"}`);
}

/** Limit all requests, including pagination and ancestry checks, globally. */
export function createRequestLimiter(concurrency = DEFAULT_CONCURRENCY) {
  if (!Number.isInteger(concurrency) || concurrency < 1) {
    throw new Error("concurrency must be a positive integer");
  }
  let active = 0;
  const queue = [];
  const drain = () => {
    while (active < concurrency && queue.length > 0) {
      active += 1;
      const { operation, resolve, reject } = queue.shift();
      operation()
        .then(resolve, reject)
        .finally(() => {
          active -= 1;
          drain();
        });
    }
  };
  return (operation) =>
    new Promise((resolve, reject) => {
      queue.push({ operation, resolve, reject });
      drain();
    });
}

function githubHeaders(token) {
  return {
    Accept: "application/vnd.github+json",
    "User-Agent": USER_AGENT,
    "X-GitHub-Api-Version": "2022-11-28",
    ...(token ? { Authorization: `Bearer ${token}` } : {}),
  };
}

function cratesHeaders() {
  return { Accept: "application/json", "User-Agent": USER_AGENT };
}

function hasNextLink(headers) {
  return (headers.get("link") ?? "").split(",").some((part) => /;\s*rel="next"\s*$/.test(part));
}

export function createNetworkClients(options = {}) {
  const request = createRequestLimiter(options.concurrency ?? DEFAULT_CONCURRENCY);
  const requestOptions = Object.fromEntries(
    Object.entries({
      fetchImpl: options.fetchImpl,
      timeoutMs: options.timeoutMs,
      retries: options.retries,
      sleepImpl: options.sleepImpl,
    }).filter(([, value]) => value !== undefined),
  );
  const githubToken = options.githubToken ?? process.env.GITHUB_TOKEN;

  async function get(url, headers) {
    return request(() => fetchJson(url, { ...requestOptions, headers }));
  }

  async function crateVersions(crate) {
    const url = `https://crates.io/api/v1/crates/${encodeURIComponent(crate)}/versions`;
    const { data } = await get(url, cratesHeaders());
    if (!Array.isArray(data.versions)) throw new Error(`${crate}: malformed crates.io response`);
    return [...new Set(data.versions.map((version) => version.num))].sort();
  }

  async function githubPages(repo, resource) {
    const items = [];
    for (let page = 1; ; page += 1) {
      const url = `https://api.github.com/repos/${repo}/${resource}?per_page=100&page=${page}`;
      const { data, headers } = await get(url, githubHeaders(githubToken));
      if (!Array.isArray(data)) throw new Error(`${repo}: malformed GitHub ${resource} response`);
      items.push(...data);
      if (!hasNextLink(headers)) break;
    }
    return items;
  }

  return {
    crateVersions,
    repo: async (repo) =>
      (await get(`https://api.github.com/repos/${repo}`, githubHeaders(githubToken))).data,
    branches: async (repo) => githubPages(repo, "branches"),
    tags: async (repo) => githubPages(repo, "tags"),
    releases: async (repo) => githubPages(repo, "releases"),
    pullRequest: async (repo, number) =>
      (
        await get(
          `https://api.github.com/repos/${repo}/pulls/${number}`,
          githubHeaders(githubToken),
        )
      ).data,
    compare: async (repo, base, head) =>
      (
        await get(
          `https://api.github.com/repos/${repo}/compare/${encodeURIComponent(base)}...${encodeURIComponent(head)}`,
          githubHeaders(githubToken),
        )
      ).data,
  };
}

function pullRequestState(pullRequest) {
  if (!pullRequest || typeof pullRequest !== "object" || Array.isArray(pullRequest)) {
    throw new Error("malformed pull request response");
  }
  if (typeof pullRequest.merged_at === "string" && pullRequest.merged_at !== "") {
    return "MERGED";
  }
  if (pullRequest.merged_at !== null) {
    throw new Error("malformed pull request response");
  }
  if (pullRequest.state === "open") return "OPEN";
  if (pullRequest.state === "closed") return "CLOSED";
  throw new Error("malformed pull request response");
}

function ancestryRelation(status) {
  if (status === "ahead") return "ahead";
  if (status === "behind" || status === "diverged") return "diverged";
  throw new Error(`unexpected GitHub comparison status: ${status}`);
}

/** Observe every known candidate, retaining per-candidate operational errors. */
export async function observeSnapshot({ baseline, clients = createNetworkClients(), now } = {}) {
  const checkedAt = (now ?? new Date()).toISOString();
  const entries = await Promise.all(
    TRACKED_CANDIDATES.map(async (name) => {
      const config = CANDIDATE_CONFIG[name];
      try {
        const [versions, repository, branchList, tagList, releaseList, pullRequest] =
          await Promise.all([
            clients.crateVersions(config.crate),
            clients.repo(config.repo),
            clients.branches(config.repo),
            clients.tags(config.repo),
            clients.releases(config.repo),
            config.pendingReleasePr
              ? clients.pullRequest(config.repo, config.pendingReleasePr)
              : Promise.resolve(null),
          ]);
        const oldBranches = baseline?.candidates?.[name]?.branches ?? {};
        const branches = Object.fromEntries(
          await Promise.all(
            branchList.map(async (branch) => {
              const sha = branch.commit?.sha;
              if (!branch.name || !sha)
                throw new Error(`${config.repo}: malformed branch response`);
              const old = branchValue(oldBranches[branch.name]);
              if (!old || old.sha === sha) return [branch.name, sha];
              const comparison = await clients.compare(config.repo, old.sha, sha);
              if (typeof comparison.status !== "string") {
                throw new Error(`${config.repo}: malformed ancestry response for ${branch.name}`);
              }
              return [branch.name, { sha, relation: ancestryRelation(comparison.status) }];
            }),
          ),
        );
        return [
          name,
          {
            crate: config.crate,
            repo: config.repo,
            versions,
            branches,
            tags: [...new Set(tagList.map((tag) => tag.name))].sort(),
            releases: [...new Set(releaseList.map((release) => release.tag_name))].sort(),
            pendingReleasePr: pullRequest
              ? { number: config.pendingReleasePr, state: pullRequestState(pullRequest) }
              : null,
            archived: repository.archived,
            checkedAt,
          },
        ];
      } catch (error) {
        return [name, { error: error.message }];
      }
    }),
  );
  return { schemaVersion: SCHEMA_VERSION, checkedAt, candidates: Object.fromEntries(entries) };
}

export function snapshotForBaseline(observed) {
  const candidates = Object.fromEntries(
    Object.entries(observed.candidates).map(([name, candidate]) => {
      if (candidate.error) throw new Error(`${name}: ${candidate.error}`);
      return [
        name,
        {
          ...candidate,
          branches: Object.fromEntries(
            Object.entries(candidate.branches).map(([branch, value]) => [
              branch,
              branchValue(value).sha,
            ]),
          ),
        },
      ];
    }),
  );
  return {
    schemaVersion: SCHEMA_VERSION,
    checkedAt: observed.checkedAt,
    comment: BASELINE_COMMENT,
    candidates,
  };
}

const PR_STATES = new Set(["OPEN", "MERGED", "CLOSED"]);
const BRANCH_RELATIONS = new Set(["ahead", "diverged"]);

function branchValue(value) {
  return typeof value === "string" ? { sha: value } : value;
}

function validateStringArray(value, field) {
  if (!Array.isArray(value) || value.some((item) => typeof item !== "string" || item === "")) {
    return `${field} must be an array of non-empty strings`;
  }
  return null;
}

function validatePr(value, field) {
  if (value === null) return null;
  if (
    !value ||
    !Number.isInteger(value.number) ||
    value.number <= 0 ||
    !PR_STATES.has(value.state)
  ) {
    return `${field} must be null or { number, state: OPEN|MERGED|CLOSED }`;
  }
  return null;
}

/** Return a validation error string, or null when a candidate record is complete. */
export function validateCandidateRecord(candidate, { observed = false } = {}) {
  if (!candidate || typeof candidate !== "object" || Array.isArray(candidate)) {
    return "candidate record must be an object";
  }
  if (typeof candidate.error === "string" && candidate.error !== "") {
    return observed ? candidate.error : "baseline candidate cannot contain an operational error";
  }
  if (typeof candidate.crate !== "string" || candidate.crate === "") {
    return "crate must be a non-empty string";
  }
  if (typeof candidate.repo !== "string" || !/^[^/]+\/[^/]+$/.test(candidate.repo)) {
    return "repo must be an owner/repository slug";
  }
  for (const field of ["versions", "tags", "releases"]) {
    const error = validateStringArray(candidate[field], field);
    if (error) return error;
  }
  if (
    !candidate.branches ||
    typeof candidate.branches !== "object" ||
    Array.isArray(candidate.branches)
  ) {
    return "branches must be an object keyed by branch name";
  }
  for (const [name, rawHead] of Object.entries(candidate.branches)) {
    const head = branchValue(rawHead);
    if (name === "" || !head || typeof head.sha !== "string" || head.sha === "") {
      return "every branch must have a non-empty name and head SHA";
    }
    if (head.relation !== undefined && !BRANCH_RELATIONS.has(head.relation)) {
      return `branch ${name} has an invalid relation`;
    }
  }
  const prError = validatePr(candidate.pendingReleasePr, "pendingReleasePr");
  if (prError) return prError;
  if (typeof candidate.archived !== "boolean") return "archived must be boolean";
  if (typeof candidate.checkedAt !== "string" || Number.isNaN(Date.parse(candidate.checkedAt))) {
    return "checkedAt must be an ISO-8601 timestamp";
  }
  return null;
}

function additions(before, after) {
  const known = new Set(before);
  return [...new Set(after)].filter((value) => !known.has(value)).sort();
}

function removals(before, after) {
  const observed = new Set(after);
  return [...new Set(before)].filter((value) => !observed.has(value)).sort();
}

function compareBranches(before, after) {
  const deltas = [];
  const errors = [];
  const names = [...new Set([...Object.keys(before), ...Object.keys(after)])].sort();

  for (const name of names) {
    const oldHead = branchValue(before[name]);
    const newHead = branchValue(after[name]);
    if (!oldHead) {
      deltas.push({ kind: "branch-added", branch: name, sha: newHead.sha });
    } else if (!newHead) {
      deltas.push({ kind: "branch-deleted", branch: name, sha: oldHead.sha });
    } else if (oldHead.sha !== newHead.sha) {
      if (newHead.relation === "ahead") {
        deltas.push({
          kind: "branch-advanced",
          branch: name,
          from: oldHead.sha,
          to: newHead.sha,
        });
      } else if (newHead.relation === "diverged") {
        deltas.push({
          kind: "branch-diverged",
          branch: name,
          from: oldHead.sha,
          to: newHead.sha,
        });
      } else {
        errors.push(
          `branch ${name} changed from ${oldHead.sha} to ${newHead.sha} without ancestry evidence`,
        );
      }
    }
  }
  return { deltas, errors };
}

/**
 * Compare one complete baseline candidate with one observed candidate.
 * This is pure: callers obtain the observation (and branch ancestry evidence)
 * through their own injected client.
 */
export function compareCandidate(name, baseline, observed) {
  const baselineError = validateCandidateRecord(baseline);
  if (baselineError) {
    return { name, status: OPERATIONAL_FAILURE, error: `invalid baseline: ${baselineError}` };
  }
  const observedError = validateCandidateRecord(observed, { observed: true });
  if (observedError) {
    return { name, status: OPERATIONAL_FAILURE, error: observedError };
  }
  if (baseline.crate !== observed.crate || baseline.repo !== observed.repo) {
    return {
      name,
      status: OPERATIONAL_FAILURE,
      error: "observed crate/repo identity does not match the baseline",
    };
  }

  const deltas = [];
  const missingVersions = removals(baseline.versions, observed.versions);
  if (missingVersions.length > 0) {
    return {
      name,
      status: OPERATIONAL_FAILURE,
      error: `observation omitted known crates.io version(s): ${missingVersions.join(", ")}`,
    };
  }
  for (const version of additions(baseline.versions, observed.versions)) {
    deltas.push({ kind: "version-published", version });
  }
  for (const tag of additions(baseline.tags, observed.tags)) {
    deltas.push({ kind: "tag-added", tag });
  }
  for (const release of additions(baseline.releases, observed.releases)) {
    deltas.push({ kind: "release-added", release });
  }

  const branchComparison = compareBranches(baseline.branches, observed.branches);
  if (branchComparison.errors.length > 0) {
    return { name, status: OPERATIONAL_FAILURE, error: branchComparison.errors.join("; ") };
  }
  deltas.push(...branchComparison.deltas);

  const oldPr = baseline.pendingReleasePr;
  const newPr = observed.pendingReleasePr;
  if (oldPr?.number === newPr?.number && oldPr?.state !== newPr?.state) {
    deltas.push({
      kind: "release-pr-state-changed",
      number: oldPr.number,
      from: oldPr.state,
      to: newPr.state,
    });
  } else if (oldPr?.number !== newPr?.number) {
    deltas.push({ kind: "release-pr-changed", from: oldPr, to: newPr });
  }

  if (baseline.archived !== observed.archived) {
    deltas.push({
      kind: observed.archived ? "repository-archived" : "repository-unarchived",
    });
  }

  return { name, status: deltas.length === 0 ? NO_DRIFT : CANDIDATE_DRIFT, deltas };
}

function validateSnapshot(snapshot, label) {
  if (!snapshot || typeof snapshot !== "object" || Array.isArray(snapshot)) {
    return `${label} must be an object`;
  }
  if (snapshot.schemaVersion !== SCHEMA_VERSION) {
    return `${label} schemaVersion must be ${SCHEMA_VERSION}`;
  }
  if (typeof snapshot.checkedAt !== "string" || Number.isNaN(Date.parse(snapshot.checkedAt))) {
    return `${label} checkedAt must be an ISO-8601 timestamp`;
  }
  if (
    !snapshot.candidates ||
    typeof snapshot.candidates !== "object" ||
    Array.isArray(snapshot.candidates)
  ) {
    return `${label} candidates must be an object`;
  }
  return null;
}

/** Compare complete baseline and observed snapshots into the stable --json shape. */
export function compareSnapshots(baseline, observed) {
  const baselineError = validateSnapshot(baseline, "baseline");
  const observedError = validateSnapshot(observed, "observation");
  if (baselineError || observedError) {
    const error = baselineError ?? observedError;
    return {
      status: OPERATIONAL_FAILURE,
      exitCode: EXIT_CODES[OPERATIONAL_FAILURE],
      checkedAt: observed?.checkedAt ?? null,
      candidates: [],
      errors: [error],
    };
  }

  const results = TRACKED_CANDIDATES.map((name) => {
    if (!(name in baseline.candidates)) {
      return { name, status: OPERATIONAL_FAILURE, error: "candidate missing from baseline" };
    }
    if (!(name in observed.candidates)) {
      return { name, status: OPERATIONAL_FAILURE, error: "candidate was not observed" };
    }
    return compareCandidate(name, baseline.candidates[name], observed.candidates[name]);
  });
  const unknownBaseline = Object.keys(baseline.candidates).filter(
    (name) => !TRACKED_CANDIDATES.includes(name),
  );
  if (unknownBaseline.length > 0) {
    results.push({
      name: "baseline",
      status: OPERATIONAL_FAILURE,
      error: `unknown candidate(s): ${unknownBaseline.sort().join(", ")}`,
    });
  }

  // A partial observation can contain genuine drift, but must never be emitted
  // as a clean triage signal. Operational failure has global precedence.
  const hasFailure = results.some((result) => result.status === OPERATIONAL_FAILURE);
  const hasDrift = results.some((result) => result.status === CANDIDATE_DRIFT);
  const status = hasFailure ? OPERATIONAL_FAILURE : hasDrift ? CANDIDATE_DRIFT : NO_DRIFT;
  return {
    status,
    exitCode: EXIT_CODES[status],
    checkedAt: observed.checkedAt,
    candidates: results,
    errors: results
      .filter((result) => result.status === OPERATIONAL_FAILURE)
      .map((result) => ({ candidate: result.name, message: result.error })),
  };
}

function describeDelta(delta) {
  switch (delta.kind) {
    case "version-published":
      return `new crates.io version ${delta.version}`;
    case "tag-added":
      return `new tag ${delta.tag}`;
    case "release-added":
      return `new GitHub Release ${delta.release}`;
    case "branch-added":
      return `new branch ${delta.branch} at ${delta.sha}`;
    case "branch-deleted":
      return `deleted branch ${delta.branch} (was ${delta.sha})`;
    case "branch-advanced":
      return `branch ${delta.branch} advanced ${delta.from} -> ${delta.to}`;
    case "branch-diverged":
      return `branch ${delta.branch} force-pushed/diverged ${delta.from} -> ${delta.to}`;
    case "release-pr-state-changed":
      return `release PR #${delta.number} ${delta.from} -> ${delta.to}`;
    case "release-pr-changed":
      return `pending release PR changed ${JSON.stringify(delta.from)} -> ${JSON.stringify(delta.to)}`;
    case "repository-archived":
      return "repository archived";
    case "repository-unarchived":
      return "repository unarchived";
    default:
      return delta.kind;
  }
}

/** Render a comparison result for a human-readable CI log or issue body. */
export function formatReport(result) {
  const lines = [`YAML candidate watch: ${result.status}`];
  if (result.checkedAt) lines.push(`Checked at: ${result.checkedAt}`);
  for (const candidate of result.candidates) {
    if (candidate.status === NO_DRIFT) {
      lines.push(`- ${candidate.name}: no-drift`);
    } else if (candidate.status === OPERATIONAL_FAILURE) {
      lines.push(`- ${candidate.name}: operational failure: ${candidate.error}`);
    } else {
      lines.push(`- ${candidate.name}: CANDIDATE_DRIFT`);
      for (const delta of candidate.deltas) lines.push(`  - ${describeDelta(delta)}`);
    }
  }
  if (result.status === CANDIDATE_DRIFT) {
    lines.push(
      "CANDIDATE_DRIFT requires triage against #2755; this report does not decide its trigger.",
    );
  } else if (result.status === OPERATIONAL_FAILURE) {
    lines.push(
      "Monitor incomplete: retry the operational failure before triaging candidate drift.",
    );
  }
  return `${lines.join("\n")}\n`;
}

/** Render the documented --json representation deterministically. */
export function formatJsonReport(result) {
  return `${JSON.stringify(result, null, 2)}\n`;
}

export function parseCliArgs(args) {
  const options = { baseline: DEFAULT_BASELINE_URL, snapshot: false, json: false };
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--snapshot") options.snapshot = true;
    else if (argument === "--json") options.json = true;
    else if (argument === "--baseline") {
      const path = args[index + 1];
      if (!path || path.startsWith("--")) throw new Error("--baseline requires a path");
      options.baseline = path;
      index += 1;
    } else {
      throw new Error(`unknown argument: ${argument}`);
    }
  }
  return options;
}

async function readBaseline(path) {
  return JSON.parse(await readFile(path, "utf8"));
}

export async function runCli(
  args,
  { stdout = process.stdout, stderr = process.stderr, clients, now } = {},
) {
  try {
    const options = parseCliArgs(args);
    let baseline;
    try {
      baseline = await readBaseline(options.baseline);
    } catch (error) {
      if (!options.snapshot) throw error;
      if (error.code !== "ENOENT") throw error;
    }
    const observed = await observeSnapshot({ baseline, clients, now });
    if (options.snapshot) {
      stdout.write(`${JSON.stringify(snapshotForBaseline(observed), null, 2)}\n`);
      return 0;
    }
    const result = compareSnapshots(baseline, observed);
    stdout.write(options.json ? formatJsonReport(result) : formatReport(result));
    return result.exitCode;
  } catch (error) {
    if (args.includes("--json")) {
      stdout.write(
        formatJsonReport({
          status: OPERATIONAL_FAILURE,
          exitCode: EXIT_CODES[OPERATIONAL_FAILURE],
          checkedAt: null,
          candidates: [],
          errors: [{ candidate: "monitor", message: error.message }],
        }),
      );
    } else {
      stderr.write(`YAML candidate watch operational failure: ${error.message}\n`);
    }
    return EXIT_CODES[OPERATIONAL_FAILURE];
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = await runCli(process.argv.slice(2));
}
