// Data-only release policy. CONTROL interprets immutable SOURCE data, never SOURCE code.
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { execGh, isMain, repoFromEnv } from "./gh.mjs";

export const POLICY_PATH = "release/npm-publication-policy.json";
export const SUSPENDED_PACKAGE = "@takazudo/zfb-linux-arm64-gnu";
export const KNOWN_PACKAGE_DIRS = Object.freeze({
  "packages/zfb-darwin-arm64": "@takazudo/zfb-darwin-arm64",
  "packages/zfb-darwin-x64": "@takazudo/zfb-darwin-x64",
  "packages/zfb-linux-arm64-gnu": SUSPENDED_PACKAGE,
  "packages/zfb-linux-x64-gnu": "@takazudo/zfb-linux-x64-gnu",
  "packages/zfb-win32-x64-msvc": "@takazudo/zfb-win32-x64-msvc",
  "packages/zfb-slugify": "@takazudo/zfb-slugify",
  "packages/zfb": "@takazudo/zfb",
  "packages/zfb-runtime": "@takazudo/zfb-runtime",
  "packages/zfb-adapter-cloudflare": "@takazudo/zfb-adapter-cloudflare",
  "packages/create-zfb": "create-zfb",
  "crates/zfb-md-wasm/npm": "@takazudo/zfb-md-wasm",
});

export function policyFromSourceFiles({ policyText = null, hasSlugify = true, mode } = {}) {
  let suspended = [];
  if (policyText !== null) {
    const data = JSON.parse(policyText);
    const suspendedList = JSON.stringify(data.suspended);
    const allowed =
      suspendedList === JSON.stringify([]) || suspendedList === JSON.stringify([SUSPENDED_PACKAGE]);
    if (
      data.schema !== "zfb-npm-publication-policy/v1" ||
      JSON.stringify(Object.keys(data).sort()) !== JSON.stringify(["schema", "suspended"]) ||
      !allowed
    ) {
      throw new Error("invalid source npm publication policy");
    }
    suspended = data.suspended;
    if (!hasSlugify) throw new Error("policy source is missing required slugify package");
  }
  if (!hasSlugify && mode !== "recovery-no-provenance") {
    throw new Error(
      "publish target is missing expected package manifest: packages/zfb-slugify/package.json",
    );
  }
  const packages = Object.entries(KNOWN_PACKAGE_DIRS)
    .filter(
      ([dir, name]) => !suspended.includes(name) && (hasSlugify || dir !== "packages/zfb-slugify"),
    )
    .map(([dir, name]) => ({ dir, name }));
  return {
    schema: "zfb-resolved-npm-policy/v1",
    packages,
    names: packages.map((p) => p.name),
    suspended,
  };
}

export function policyForTree(root = ".", mode) {
  const path = join(root, POLICY_PATH);
  return policyFromSourceFiles({
    policyText: existsSync(path) ? readFileSync(path, "utf8") : null,
    hasSlugify: existsSync(join(root, "packages/zfb-slugify/package.json")),
    mode,
  });
}

export function policyForCommit(sourceCommit, { root = ".", mode } = {}) {
  if (!/^[0-9a-f]{40}$/.test(sourceCommit))
    throw new Error("policy requires immutable 40-hex source SHA");
  const git = (args) => execFileSync("git", args, { cwd: root, encoding: "utf8" });
  const paths = git(["ls-tree", "-r", "--name-only", sourceCommit]).split("\n");
  return policyFromSourceFiles({
    policyText: paths.includes(POLICY_PATH)
      ? git(["show", `${sourceCommit}:${POLICY_PATH}`])
      : null,
    hasSlugify: paths.includes("packages/zfb-slugify/package.json"),
    mode,
  });
}

// Suitable for the checkout-free OIDC job: GitHub API data only, no dependency install.
export function policyForSource(sourceCommit, { repo = repoFromEnv(), gh = execGh, mode } = {}) {
  if (!/^[0-9a-f]{40}$/.test(sourceCommit))
    throw new Error("policy requires immutable 40-hex source SHA");
  const tree = JSON.parse(gh(["api", `repos/${repo}/git/trees/${sourceCommit}?recursive=1`]));
  if (tree.truncated || !Array.isArray(tree.tree))
    throw new Error("source policy tree is incomplete");
  const entry = tree.tree.find((e) => e.path === POLICY_PATH);
  let policyText = null;
  if (entry) {
    if (entry.type !== "blob" || entry.mode !== "100644")
      throw new Error("source policy is not a regular data file");
    const blob = JSON.parse(gh(["api", `repos/${repo}/git/blobs/${entry.sha}`]));
    if (blob.encoding !== "base64") throw new Error("source policy blob encoding is invalid");
    policyText = Buffer.from(blob.content, "base64").toString("utf8");
  }
  return policyFromSourceFiles({
    policyText,
    hasSlugify: tree.tree.some(
      (e) => e.path === "packages/zfb-slugify/package.json" && e.type === "blob",
    ),
    mode,
  });
}

export function assertPackageRows(rows, policy, version) {
  if (!Array.isArray(rows) || rows.length !== policy.packages.length)
    throw new Error("npm package set differs from source policy");
  for (let i = 0; i < rows.length; i++) {
    if (
      rows[i].name !== policy.packages[i].name ||
      rows[i].dir !== policy.packages[i].dir ||
      rows[i].version !== version
    ) {
      throw new Error(`npm package row ${i} differs from source policy/name/order/version`);
    }
  }
}

if (isMain(import.meta.url)) {
  const args = process.argv.slice(2);
  const value = (name) => args[args.indexOf(name) + 1];
  try {
    const policy = policyForSource(value("--source-commit"), {
      repo: args.includes("--repo") ? value("--repo") : repoFromEnv(),
      mode: args.includes("--mode") ? value("--mode") : undefined,
    });
    process.stdout.write(
      args.includes("--names") ? `${policy.names.join("\n")}\n` : `${JSON.stringify(policy)}\n`,
    );
  } catch (error) {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  }
}
