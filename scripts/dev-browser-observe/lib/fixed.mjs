import { existsSync } from "node:fs";
import { createServer } from "node:net";
import { dirname, join, resolve } from "node:path";
import { OBSERVER_PATH } from "./browser.mjs";
import {
  appTreeHash,
  distributionChecks,
  distributionOf,
  fixedHelperPath,
  npmIdentity,
  resolveNpmCli,
} from "./distribution.mjs";
import { FIXTURE_KINDS } from "./fixture-kinds.mjs";
import { currentPins } from "./identity.mjs";
import { sha256File } from "./util.mjs";

/** The nearest enclosing git work tree root (a dir holding `.git`), or null. */
function gitWorkTreeOf(dir) {
  let cur = resolve(dir);
  for (;;) {
    if (existsSync(join(cur, ".git"))) return cur;
    const parent = dirname(cur);
    if (parent === cur) return null;
    cur = parent;
  }
}

const check = (name, ok, expected, actual, extra = {}) => ({
  name,
  ok,
  expected,
  actual,
  ...extra,
});

/**
 * Identity of a fixed-distribution profile, checked before any edit: the declared framework
 * entry; the corpus and scale (the same source checks as source-built Field Notes); the
 * source app, helper, config and adapter files against the declarations; the install
 * (receipt, node binary, lockfile, pinned versions, hidden-lockfile match, executables,
 * node_modules tree); the imported-CSS variant files; the harness, fixture, observer and
 * helper pins; and both dirs outside any git work tree.
 */
export function fixedIdentityVerdict({
  profile,
  resolved,
  manifest,
  distributionImpl = distributionChecks,
}) {
  const checks = [];
  let entry;
  try {
    entry = distributionOf(resolved.fixtureDir, profile.framework, manifest);
  } catch (error) {
    checks.push(
      check("framework-declared", false, profile.framework, String(error?.message ?? error)),
    );
    return { ok: false, checks, facts: {} };
  }
  checks.push(
    ...FIXTURE_KINDS["field-notes"].extraChecks(
      { ...profile, fixture: { ...profile.fixture, managedCss: false, placement: "external" } },
      resolved,
      manifest,
    ),
  );
  const srcApp = resolved.sourceDir ? join(resolved.sourceDir, entry.app) : null;
  const srcTree = srcApp && existsSync(srcApp) ? appTreeHash(srcApp, entry).sha256 : null;
  checks.push(
    check("source-app-tree", srcTree === entry.appTree.sha256, entry.appTree.sha256, srcTree),
  );
  const fileSha = (p) => (p && existsSync(p) ? sha256File(p) : null);
  const cfg = fileSha(srcApp && join(srcApp, entry.config.path));
  checks.push(check("source-config", cfg === entry.config.sha256, entry.config.sha256, cfg));
  for (const f of entry.adapterFiles ?? []) {
    if (f.volatile) continue;
    const a = fileSha(srcApp && join(srcApp, f.path));
    checks.push(check(`adapter-file:${f.path}`, a === f.sha256, f.sha256, a, { label: f.label }));
  }
  let helperPath = null;
  try {
    helperPath = fixedHelperPath({
      fixtureDir: resolved.fixtureDir,
      manifest,
      entry,
      sourceDir: resolved.sourceDir ?? "",
      adapter: profile.fixture.adapter,
    });
  } catch (error) {
    checks.push(check("helper-declared", false, entry.helper, String(error?.message ?? error)));
  }
  const helperSha = fileSha(helperPath);
  const helperExpected = entry.helper.adapters
    ? (manifest.adapters.variants[profile.fixture.adapter]?.sha256 ?? null)
    : entry.helper.sha256;
  checks.push(
    check(
      `helper:${profile.fixture.adapter}`,
      helperSha !== null && helperSha === helperExpected,
      helperExpected,
      helperSha,
    ),
  );
  if (profile.fixture.importedCss)
    for (const f of entry.importedCss.files) {
      const a = fileSha(join(resolved.fixtureDir, f.file));
      checks.push(
        check(`imported-css:${f.target}`, a === f.sha256, f.sha256, a, {
          provenance: f.provenance,
        }),
      );
    }
  let dist = { ok: false, checks: [], receipt: null, verification: null };
  if (!resolved.installDir || !existsSync(resolved.installDir))
    checks.push(check("install-dir", false, "existing dir", resolved.installDir));
  else {
    dist = distributionImpl({ installDir: resolved.installDir, entry });
    checks.push(...dist.checks);
  }
  const launch = profile.distribution?.launch ?? "direct";
  let npm = null;
  if (launch === "npm-script") {
    npm = npmIdentity(resolveNpmCli());
    checks.push(check("npm-cli", npm.cli !== null, "npm-cli.js on PATH", npm.cli));
  }
  const pins =
    helperPath && existsSync(helperPath)
      ? currentPins({
          fixtureDir: resolved.fixtureDir,
          observerPath: OBSERVER_PATH,
          adapterPath: helperPath,
        })
      : null;
  for (const key of ["harnessTreeSha256", "fixtureTreeSha256", "observerSha256", "adapterSha256"]) {
    const pinned = profile.pins?.[key] ?? null;
    const actual = pins?.[key] ?? null;
    checks.push(check(`pin:${key}`, pinned !== null && pinned === actual, pinned, actual));
  }
  for (const [name, dir] of [
    ["outdir", resolved.outDir],
    ["installdir", resolved.installDir],
  ]) {
    const tree = dir ? gitWorkTreeOf(dir) : null;
    checks.push(check(`${name}-outside-git-worktree`, tree === null, null, tree));
  }
  return {
    ok: checks.every((c) => c.ok),
    checks,
    facts: {
      framework: profile.framework,
      entry: { app: entry.app, pinned: entry.pinned, manifestSha256: entry.manifestSha256 },
      helper: { path: helperPath, sha256: helperSha, adapter: profile.fixture.adapter },
      install: dist.verification?.facts ?? null,
      installReceipt: dist.receipt
        ? {
            createdAt: dist.receipt.createdAt,
            node: dist.receipt.node,
            npm: dist.receipt.npm,
            command: dist.receipt.command,
          }
        : null,
      node: dist.node ?? null,
      launch,
      npm,
      pins,
    },
  };
}

/** A currently free TCP port on `host` (bound then released; the server must bind it next). */
export function freePort(host) {
  return new Promise((resolvePort, reject) => {
    const s = createServer();
    s.once("error", reject);
    s.listen(0, host, () => {
      const { port } = s.address();
      s.close(() => resolvePort(port));
    });
  });
}
