// publish-receipts.mjs — what npm was told to publish, recorded by release.yml
// in the `npm-publish-receipts` artifact (TSV: name, version, attested|none,
// outcome), and the optional npm credential for the version-status API.
//
// Outcomes: already-present | publish-accepted (npm printed `+ name@version`) |
// processing (E409 "previously staged") | publish-attempted (legacy). The
// first two of the accepted kind prove npm took the publish, so a version that
// still reads E404 is processing, never absent.
import { mkdtempSync, readFileSync, readdirSync, rmSync, statSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";

export const RECEIPTS_ARTIFACT = "npm-publish-receipts";
export const PROCESSING_OUTCOMES = Object.freeze(["publish-accepted", "processing"]);
export const SUPPORT_AFTER_HOURS = 24;

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

/** Pure: parse a receipts TSV into Map<"name@version", { expected, outcome }>. */
export function parseReceipts(text) {
  const entries = new Map();
  for (const line of String(text).split("\n")) {
    if (!line.trim() || line.startsWith("#")) continue;
    const [name, version, expected, outcome] = line.split("\t").map((cell) => cell.trim());
    if (!name || !version || !outcome) continue;
    entries.set(`${name}@${version}`, { expected: expected || "none", outcome });
  }
  return entries;
}

function filesUnder(dir) {
  return readdirSync(dir, { recursive: true })
    .map((entry) => join(dir, entry))
    .filter((path) => statSync(path).isFile());
}

/**
 * Receipts of every release.yml run for `tag` that uploaded the artifact,
 * newest run first; the first run to name a package wins. Each entry carries
 * `acceptedAt`: the artifact's created_at, else the run's start time. A run
 * with no (or an expired) artifact is skipped; a gh failure throws.
 */
export function loadReceipts({ tag, repo, gh }) {
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
      "databaseId,attempt,headBranch,displayTitle,createdAt,startedAt",
    ]),
  )
    .filter((run) => runMatchesTag(run, tag))
    .sort((a, b) => String(b.createdAt).localeCompare(String(a.createdAt)))
    .slice(0, 10);
  const merged = new Map();
  for (const run of runs) {
    const listing = JSON.parse(
      gh(["api", `repos/${repo}/actions/runs/${run.databaseId}/artifacts?per_page=100`]),
    );
    const artifact = (listing.artifacts ?? [])
      .filter((candidate) => candidate.name === RECEIPTS_ARTIFACT && !candidate.expired)
      .sort((a, b) => String(b.created_at).localeCompare(String(a.created_at)))[0];
    if (!artifact) continue;
    const dir = mkdtempSync(join(tmpdir(), "zfb-publish-receipts-"));
    try {
      gh([
        "run",
        "download",
        String(run.databaseId),
        "--repo",
        repo,
        "--name",
        RECEIPTS_ARTIFACT,
        "--dir",
        dir,
      ]);
      const acceptedAt = artifact.created_at ?? run.startedAt ?? run.createdAt ?? null;
      for (const file of filesUnder(dir)) {
        for (const [key, entry] of parseReceipts(readFileSync(file, "utf8"))) {
          if (!merged.has(key)) merged.set(key, { ...entry, acceptedAt, runId: run.databaseId });
        }
      }
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }
  return merged;
}

/** Pure: the support deadline (accept time + 24 h) or null when the time is unknown. */
export function supportDeadline(acceptedAt) {
  const time = Date.parse(acceptedAt ?? "");
  return Number.isNaN(time)
    ? null
    : new Date(time + SUPPORT_AFTER_HOURS * 3600 * 1000).toISOString();
}

/**
 * The npm credential for the version-status API, or null: env NPM_TOKEN, else
 * the registry.npmjs.org _authToken line of the user's npmrc. An unexpanded
 * `${VAR}` placeholder is not a credential. The value is only ever returned to
 * the caller that sends it as a header; nothing here prints it.
 */
export function resolveNpmToken(
  env = process.env,
  readText = (path) => readFileSync(path, "utf8"),
) {
  const fromEnv = (env.NPM_TOKEN ?? "").trim();
  if (fromEnv) return fromEnv;
  const rc = env.NPM_CONFIG_USERCONFIG || join(env.HOME || homedir(), ".npmrc");
  let text;
  try {
    text = readText(rc);
  } catch {
    return null;
  }
  const match = /^\/\/registry\.npmjs\.org\/:_authToken=(.+)$/m.exec(text);
  const value = match?.[1].trim().replace(/^(["'])(.*)\1$/, "$2") ?? "";
  return value && !value.startsWith("${") ? value : null;
}
