import {
  chmodSync,
  cpSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  realpathSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, relative, resolve } from "node:path";
import {
  FROZEN_SCHEMA,
  FROZEN_STAMP,
  fixtureTreeHash,
  frozenState,
  fullTreeHash,
  harnessCapability,
  harnessTreeHash,
} from "./identity.mjs";
import { HARNESS_DIR, isoNow, sha256File } from "./util.mjs";

export const FROZEN_HARNESS_DIRNAME = "dev-browser-observe";

function playwrightPackageDir(harnessDir) {
  const req = createRequire(join(harnessDir, "cli.mjs"));
  return realpathSync(dirname(req.resolve("@playwright/test/package.json")));
}

function setReadOnly(dir) {
  for (const name of readdirSync(dir)) {
    const abs = join(dir, name);
    const st = lstatSync(abs);
    if (st.isDirectory()) setReadOnly(abs);
    else if (st.isFile()) chmodSync(abs, st.mode & 0o555);
  }
  chmodSync(dir, lstatSync(dir).mode & 0o555);
}

/** Give a frozen copy write permission back (only for deleting it; the stamp then still decides intact). */
export function unfreezeForRemoval(dir) {
  chmodSync(dir, lstatSync(dir).mode | 0o200);
  for (const name of readdirSync(dir)) {
    const abs = join(dir, name);
    const st = lstatSync(abs);
    if (st.isDirectory()) unfreezeForRemoval(abs);
    else if (st.isFile()) chmodSync(abs, st.mode | 0o200);
  }
}

function anyWritable(dir) {
  const out = [];
  const visit = (d) => {
    if (lstatSync(d).mode & 0o222) out.push(d);
    for (const name of readdirSync(d)) {
      const abs = join(d, name);
      const st = lstatSync(abs);
      if (st.isDirectory()) visit(abs);
      else if (st.isFile() && st.mode & 0o222) out.push(abs);
    }
  };
  visit(dir);
  return out;
}

/**
 * Copy this harness into <outDir>/dev-browser-observe, stamp its hashes, link the exact
 * @playwright/test package it resolves today, and make the copy read-only. Refuses a dirty
 * harness dir unless allowDirty, and never overwrites.
 */
export function freezeHarness({
  outDir,
  harnessDir = HARNESS_DIR,
  allowDirty = false,
  capabilityImpl = harnessCapability,
}) {
  const out = resolve(outDir);
  if (existsSync(out)) throw new Error(`frozen out dir already exists: ${out}`);
  const cap = capabilityImpl(harnessDir);
  if (cap.frozen) throw new Error(`${harnessDir} is already a frozen copy; freeze from a checkout`);
  if (cap.harnessDirDirty && !allowDirty)
    throw new Error(
      `harness dir has uncommitted changes (patch ${cap.harnessDirPatchSha256}, untracked ${JSON.stringify(cap.harnessDirUntracked)}); commit them or pass --allow-dirty`,
    );
  const pwDir = playwrightPackageDir(harnessDir);
  const pwVersion = JSON.parse(readFileSync(join(pwDir, "package.json"), "utf8")).version;
  const dest = join(out, FROZEN_HARNESS_DIRNAME);
  mkdirSync(out, { recursive: true });
  // Unit tests are not part of a measurement series; everything the runs read is copied.
  cpSync(harnessDir, dest, {
    recursive: true,
    filter: (p) => {
      const rel = relative(harnessDir, p).split(/[\\/]/);
      return !rel.includes("node_modules") && rel[0] !== "__tests__";
    },
  });
  const link = join(out, "node_modules", "@playwright", "test");
  mkdirSync(dirname(link), { recursive: true });
  symlinkSync(pwDir, link, "dir");
  const fixturesDir = join(dest, "fixtures");
  const stamp = {
    schema: FROZEN_SCHEMA,
    frozenAt: isoNow(),
    source: {
      repo: cap.repo,
      sha: cap.sha,
      harnessDir,
      harnessDirDirty: cap.harnessDirDirty,
      harnessDirPatchSha256: cap.harnessDirPatchSha256,
      harnessDirUntracked: cap.harnessDirUntracked,
    },
    harnessTreeSha256: harnessTreeHash(dest),
    fullTreeSha256: fullTreeHash(dest),
    sourceHarnessTreeSha256: harnessTreeHash(harnessDir),
    fixtures: Object.fromEntries(
      readdirSync(fixturesDir)
        .filter((n) => !n.startsWith("."))
        .map((n) => [n, fixtureTreeHash(join(fixturesDir, n))]),
    ),
    observerSha256: sha256File(join(dest, "observer-init.js")),
    playwright: { link, target: pwDir, version: pwVersion },
  };
  if (stamp.harnessTreeSha256 !== stamp.sourceHarnessTreeSha256)
    throw new Error("frozen copy harness tree differs from the source harness tree");
  writeFileSync(join(dest, FROZEN_STAMP), `${JSON.stringify(stamp, null, 2)}\n`);
  setReadOnly(dest);
  return { dir: dest, cli: join(dest, "cli.mjs"), stamp };
}

/** Recompute a frozen copy's hashes, its read-only bits and its playwright link. */
export function verifyFrozen(dir) {
  const state = frozenState(dir);
  if (!state)
    return { ok: false, checks: [{ name: "frozen-stamp", ok: false, actual: "no stamp" }] };
  const { stamp } = state;
  const checks = [
    {
      name: "harness-tree",
      ok: state.actual.harnessTreeSha256 === stamp.harnessTreeSha256,
      expected: stamp.harnessTreeSha256,
      actual: state.actual.harnessTreeSha256,
    },
    {
      name: "full-tree",
      ok: state.actual.fullTreeSha256 === stamp.fullTreeSha256,
      expected: stamp.fullTreeSha256,
      actual: state.actual.fullTreeSha256,
    },
  ];
  const writable = anyWritable(dir);
  checks.push({ name: "read-only", ok: writable.length === 0, expected: [], actual: writable });
  const link = state.playwrightLink;
  checks.push({
    name: "playwright-link",
    ok: link.ok,
    expected: link.expected,
    actual: link.actual,
  });
  return { ok: checks.every((c) => c.ok), dir, stamp, checks };
}
