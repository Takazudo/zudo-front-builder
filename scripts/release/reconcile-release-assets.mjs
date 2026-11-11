#!/usr/bin/env node
// reconcile-release-assets.mjs — attach local files to a GitHub Release without
// ever overwriting one (#4690). Replaces `gh release upload --clobber`.
//
//   node scripts/release/reconcile-release-assets.mjs --tag vX.Y.Z --dir DIR
//        [--verify-only] [--file NAME ...] [--repo owner/name]
//
// For every file in DIR (or only the named --file entries):
//   missing on the Release      → upload (refused under --verify-only)
//   present, identical SHA-256  → skip
//   present, different SHA-256  → fail, naming the asset
// The remote digest comes from the Release asset's `digest` field; when GitHub
// reports none, the asset is downloaded into a fresh temp dir and hashed.
// Nothing is uploaded until every file has been classified, so a conflict on
// one asset never leaves a partial upload of the others.
//
// Exit 0 reconciled, 1 conflict or (verify-only) missing, 2 usage/read error.
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, readdirSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execGh, isMain, repoFromEnv } from "./gh.mjs";

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

export function listReleaseAssets({ tag, repo, gh = execGh }) {
  const view = JSON.parse(gh(["release", "view", tag, "--repo", repo, "--json", "assets"]));
  return view.assets ?? [];
}

export function remoteDigest(asset, { tag, repo, gh = execGh }) {
  const match = /^sha256:([0-9a-f]{64})$/.exec(asset.digest ?? "");
  if (match) return { sha256: match[1], source: "api-digest" };
  const dir = mkdtempSync(join(tmpdir(), "zfb-reconcile-"));
  try {
    gh(["release", "download", tag, "--repo", repo, "--pattern", asset.name, "--dir", dir]);
    return { sha256: sha256(readFileSync(join(dir, asset.name))), source: "downloaded" };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

/** Pure: classify local files against remote assets (remote digests resolved lazily). */
export function plan(localFiles, remoteAssets, digestOf) {
  const byName = new Map();
  for (const asset of remoteAssets) {
    const list = byName.get(asset.name) ?? [];
    list.push(asset);
    byName.set(asset.name, list);
  }
  return localFiles.map((file) => {
    const matches = byName.get(file.name) ?? [];
    if (matches.length === 0) return { ...file, action: "upload" };
    if (matches.length > 1) {
      return { ...file, action: "conflict", reason: `${matches.length} assets share this name` };
    }
    const [asset] = matches;
    if (asset.state && asset.state !== "uploaded") {
      return { ...file, action: "conflict", reason: `remote asset state is ${asset.state}` };
    }
    const remote = digestOf(asset);
    if (remote.sha256 === file.sha256) return { ...file, action: "skip", remote };
    return {
      ...file,
      action: "conflict",
      remote,
      reason: `remote sha256 ${remote.sha256} differs from local ${file.sha256}`,
    };
  });
}

export function reconcile({ tag, dir, files, verifyOnly = false, repo, gh = execGh, log }) {
  const names = files?.length
    ? files
    : readdirSync(dir).filter((name) => statSync(join(dir, name)).isFile());
  if (names.length === 0) throw new Error(`no files to reconcile in ${dir}`);
  const local = names.map((name) => ({
    name,
    path: join(dir, name),
    sha256: sha256(readFileSync(join(dir, name))),
  }));
  const assets = listReleaseAssets({ tag, repo, gh });
  const decisions = plan(local, assets, (asset) => remoteDigest(asset, { tag, repo, gh }));
  const problems = [];
  for (const decision of decisions) {
    if (decision.action === "skip") {
      log(
        `skip     ${decision.name} (identical sha256 ${decision.sha256}, ${decision.remote.source})`,
      );
    } else if (decision.action === "conflict") {
      problems.push(`${decision.name}: ${decision.reason}`);
      log(`CONFLICT ${decision.name}: ${decision.reason}`);
    } else if (verifyOnly) {
      problems.push(`${decision.name}: missing from Release ${tag}`);
      log(`MISSING  ${decision.name}`);
    } else {
      log(`upload   ${decision.name} (sha256 ${decision.sha256})`);
    }
  }
  if (problems.length) return { ok: false, decisions, problems };
  const uploads = decisions.filter((decision) => decision.action === "upload");
  if (!verifyOnly && uploads.length) {
    gh(["release", "upload", tag, "--repo", repo, ...uploads.map((decision) => decision.path)]);
  }
  return { ok: true, decisions, problems };
}

export function parseArgs(argv) {
  const opts = { files: [], verifyOnly: false };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--verify-only") opts.verifyOnly = true;
    else if (["--tag", "--dir", "--file", "--repo"].includes(arg)) {
      const value = argv[++i];
      if (!value) return null;
      if (arg === "--file") opts.files.push(value);
      else opts[arg.slice(2)] = value;
    } else return null;
  }
  if (!opts.tag || !/^v\d/.test(opts.tag) || !opts.dir) return null;
  return opts;
}

export function main(argv, { gh = execGh, env = process.env } = {}) {
  const opts = parseArgs(argv);
  if (!opts) {
    process.stderr.write(
      "usage: reconcile-release-assets.mjs --tag vX.Y.Z --dir DIR [--verify-only] [--file NAME ...] [--repo owner/name]\n",
    );
    return 2;
  }
  let result;
  try {
    result = reconcile({
      ...opts,
      repo: opts.repo ?? repoFromEnv(env),
      gh,
      log: (line) => process.stdout.write(`${line}\n`),
    });
  } catch (error) {
    process.stderr.write(`reconcile-release-assets: ${error.message}\n`);
    return 2;
  }
  if (!result.ok) {
    for (const problem of result.problems) process.stdout.write(`::error::${problem}\n`);
    return 1;
  }
  return 0;
}

if (isMain(import.meta.url)) {
  process.exitCode = main(process.argv.slice(2));
}
