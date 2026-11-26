import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, readlinkSync, statSync } from "node:fs";
import { dirname, isAbsolute, join, resolve } from "node:path";
import {
  HARNESS_DIR,
  gitDirtyState,
  gitHead,
  gitToplevel,
  run,
  sha256,
  sha256File,
  treeHashDir,
  treeHashFromEntries,
  tryRun,
  gitToplevelAndHead,
} from "./util.mjs";

import { rendererMemoryBuild } from "./renderer-memory-protocol.mjs";

export const BUILD_RECEIPT_SCHEMA = "dev-browser-observe.build-receipt.v1";

// Mirrors crates/zfb/build.rs `embed_runtime()`: (source package dir, vendor name).
export const EMBEDDED_PACKAGES = [
  ["packages/zfb", "zfb"],
  ["packages/zfb-runtime", "zfb-runtime"],
  ["packages/zfb-slugify", "zfb-slugify"],
];

/**
 * The exact filter of crates/zfb/build.rs `copy_ts_src`, applied to a path
 * relative to a package's `src/` dir: every component must not start with
 * `__` or `.`, and the file must end in `.ts`. Symlinks are never copied.
 */
export function isEmbeddedSrcPath(relUnderSrc) {
  const parts = relUnderSrc.split("/");
  if (parts.some((p) => p.startsWith("__") || p.startsWith("."))) return false;
  return parts[parts.length - 1].endsWith(".ts");
}

function vendorPathFor(pkgRel, vendorName, repoRelPath) {
  if (repoRelPath === `${pkgRel}/package.json`) return `@takazudo/${vendorName}/package.json`;
  const srcPrefix = `${pkgRel}/src/`;
  if (repoRelPath.startsWith(srcPrefix)) {
    const under = repoRelPath.slice(srcPrefix.length);
    if (isEmbeddedSrcPath(under)) return `@takazudo/${vendorName}/src/${under}`;
  }
  return null;
}

function catFileBatch(repo, shas) {
  if (shas.length === 0) return new Map();
  const out = execFileSync("git", ["cat-file", "--batch"], {
    cwd: repo,
    input: `${shas.join("\n")}\n`,
    maxBuffer: 1024 * 1024 * 1024,
  });
  const blobs = new Map();
  let pos = 0;
  while (pos < out.length) {
    const nl = out.indexOf(0x0a, pos);
    const header = out.subarray(pos, nl).toString("utf8").split(" ");
    const size = Number(header[2]);
    const start = nl + 1;
    blobs.set(header[0], out.subarray(start, start + size));
    pos = start + size + 1;
  }
  return blobs;
}

/** Source package files as build.rs would embed them, read from the git object store at `sha`. */
export function sourcePackagesAtSha(repo, sha) {
  const rows = [];
  // One ls-tree for all packages: each git spawn is costly on a loaded host (the macOS /usr/bin/git shim).
  const pathspecs = EMBEDDED_PACKAGES.flatMap(([pkgRel]) => [
    `${pkgRel}/package.json`,
    `${pkgRel}/src`,
  ]);
  const listing = run("git", ["ls-tree", "-r", "-z", "--full-tree", sha, "--", ...pathspecs], {
    cwd: repo,
  });
  for (const line of listing.split("\0").filter(Boolean)) {
    const [meta, path] = line.split("\t");
    const [mode, type, blob] = meta.split(" ");
    if (type !== "blob" || mode === "120000") continue;
    for (const [pkgRel, vendorName] of EMBEDDED_PACKAGES) {
      const vendorPath = vendorPathFor(pkgRel, vendorName, path);
      if (vendorPath) rows.push({ path: vendorPath, blob });
    }
  }
  const blobs = catFileBatch(
    repo,
    rows.map((r) => r.blob),
  );
  const entries = rows.map((r) => ({
    path: r.path,
    sha256: sha256(blobs.get(r.blob)),
    bytes: blobs.get(r.blob),
  }));
  const tree = treeHashFromEntries(entries.map(({ path, sha256: s }) => ({ path, sha256: s })));
  return { ...tree, contents: entries };
}

/** Same set from the working tree (for a dirty source whose patch the profile declares). */
export function sourcePackagesWorkingTree(repo) {
  const entries = [];
  const contents = [];
  for (const [pkgRel, vendorName] of EMBEDDED_PACKAGES) {
    const pkgDir = join(repo, pkgRel);
    const pkgJson = readFileSync(join(pkgDir, "package.json"));
    entries.push({ path: `@takazudo/${vendorName}/package.json`, sha256: sha256(pkgJson) });
    contents.push({ path: `@takazudo/${vendorName}/package.json`, bytes: pkgJson });
    const t = treeHashDir(join(pkgDir, "src"), {
      filter: (rel, kind, name) =>
        kind === "dir" ? !(name.startsWith("__") || name.startsWith(".")) : isEmbeddedSrcPath(rel),
    });
    for (const f of t.files) {
      const path = `@takazudo/${vendorName}/src/${f.path}`;
      entries.push({ path, sha256: f.sha256 });
      contents.push({ path, bytes: readFileSync(join(pkgDir, "src", f.path)) });
    }
  }
  return { ...treeHashFromEntries(entries), contents };
}

export function vendorTreeHash(vendorDir) {
  const t = treeHashDir(join(vendorDir, "@takazudo"));
  return treeHashFromEntries(
    t.files.map((f) => ({ path: `@takazudo/${f.path}`, sha256: f.sha256 })),
  );
}

export function profileDirName(profile) {
  if (profile === "dev" || profile === "test") return "debug";
  if (profile === "bench") return "release";
  return profile;
}

/**
 * Every zfb build-script out dir under the target dir that holds a vendor
 * tree. The chosen candidate is the one whose `output` file (the build
 * script's recorded stdout, which names ZFB_VENDOR_DIR) is newest.
 */
export function locateVendorCandidates(targetDir, { triple, profile }) {
  const dirs = [join(targetDir, profileDirName(profile), "build")];
  if (triple) dirs.push(join(targetDir, triple, profileDirName(profile), "build"));
  const candidates = [];
  for (const buildDir of dirs) {
    if (!existsSync(buildDir)) continue;
    for (const name of readdirSync(buildDir).sort()) {
      if (!/^zfb-[0-9a-f]+$/.test(name)) continue;
      const outDir = join(buildDir, name, "out");
      const vendorDir = join(outDir, "vendor");
      const marker = join(vendorDir, "@takazudo", "zfb", "package.json");
      if (!existsSync(marker)) continue;
      const outputFile = join(buildDir, name, "output");
      let declaredVendorDir = null;
      let outputMtimeMs = null;
      if (existsSync(outputFile)) {
        outputMtimeMs = statSync(outputFile).mtimeMs;
        const m = readFileSync(outputFile, "utf8").match(/^cargo:rustc-env=ZFB_VENDOR_DIR=(.*)$/m);
        declaredVendorDir = m ? m[1].trim() : null;
      }
      const tree = vendorTreeHash(vendorDir);
      candidates.push({
        outDir,
        vendorDir,
        declaredVendorDir,
        outputMtimeMs,
        markerMtimeMs: statSync(marker).mtimeMs,
        treeSha256: tree.sha256,
        fileCount: tree.fileCount,
      });
    }
  }
  return candidates;
}

export function chooseVendorCandidate(candidates) {
  const usable = candidates.filter(
    (c) => c.declaredVendorDir && resolve(c.declaredVendorDir) === resolve(c.vendorDir),
  );
  const pool = usable.length > 0 ? usable : [];
  pool.sort((a, b) => (b.outputMtimeMs ?? 0) - (a.outputMtimeMs ?? 0));
  return pool[0] ?? null;
}

/** Proves the executable carries each embedded source file's exact bytes. */
export function embeddedBytesCheck(exeBytes, contents) {
  const missing = [];
  let checked = 0;
  for (const c of contents) {
    if (c.bytes.length === 0) continue;
    checked += 1;
    if (exeBytes.indexOf(c.bytes) === -1) missing.push(c.path);
  }
  return { checked, missing };
}

export function parseRustcVV(text) {
  const out = {};
  for (const line of text.split("\n")) {
    const m = line.match(/^([a-zA-Z -]+):\s*(.*)$/);
    if (m) out[m[1].trim()] = m[2].trim();
  }
  out.versionLine = text.split("\n")[0];
  return out;
}

/** Hash of the patch plus every untracked file's path and content. */
export function dirtyIdentity(repo) {
  const state = gitDirtyState(repo);
  const parts = [`patch:${state.patchSha256}`];
  for (const p of [...state.untracked].sort())
    parts.push(`untracked:${p}:${sha256File(join(repo, p))}`);
  return { ...state, dirtyIdentitySha256: state.dirty ? sha256(parts.join("\n")) : null };
}

export function recordBuild(opts) {
  const { top: repo, head: sha } = gitToplevelAndHead(opts.repo);
  const exe = resolve(opts.exe);
  const exeBytes = readFileSync(exe);
  const dirty = dirtyIdentity(repo);
  const env = { ...process.env };
  if (opts.toolchain) env.RUSTUP_TOOLCHAIN = opts.toolchain;
  const probe = opts.probe ?? {
    rustc: () => tryRun("rustc", ["-vV"], { cwd: repo, env }),
    cargo: () => tryRun("cargo", ["-V"], { cwd: repo, env }),
  };
  const rustc = probe.rustc();
  const cargo = probe.cargo();
  const rustcInfo = rustc.ok ? parseRustcVV(rustc.stdout) : { error: rustc.error };
  const triple = opts.triple ?? rustcInfo.host ?? null;
  const targetDir = resolve(opts.targetDir ?? join(repo, "target"));
  const candidates = locateVendorCandidates(targetDir, {
    triple: opts.triple ?? null,
    profile: opts.profile,
  });
  const targetDirChosen = chooseVendorCandidate(candidates);
  let explicitVendor = null;
  if (opts.vendorDir) {
    const vendorDir = resolve(opts.vendorDir);
    const tree = vendorTreeHash(vendorDir);
    explicitVendor = {
      source: "explicit",
      vendorDir,
      treeSha256: tree.sha256,
      fileCount: tree.fileCount,
    };
  }
  const chosen =
    explicitVendor ?? (targetDirChosen ? { source: "target-dir", ...targetDirChosen } : null);
  const exeSha = sha256(exeBytes);
  const defaultBuildExe = join(
    targetDir,
    ...(opts.triple ? [opts.triple] : []),
    profileDirName(opts.profile),
    "zfb",
  );
  const buildExePath = opts.buildExe
    ? resolve(opts.buildExe)
    : existsSync(defaultBuildExe)
      ? defaultBuildExe
      : null;
  const buildExe = buildExePath
    ? existsSync(buildExePath)
      ? { path: buildExePath, sha256: buildExePath === exe ? exeSha : sha256File(buildExePath) }
      : { path: buildExePath, sha256: null, missing: true }
    : null;
  if (buildExe) buildExe.matchesExe = buildExe.sha256 === exeSha;
  const atSha = sourcePackagesAtSha(repo, sha);
  const working = dirty.dirty ? sourcePackagesWorkingTree(repo) : null;
  const expected = dirty.dirty ? working : atSha;
  const embedded = embeddedBytesCheck(exeBytes, expected.contents);
  const honoDir = chosen ? join(chosen.vendorDir, "hono") : null;
  return {
    schema: BUILD_RECEIPT_SCHEMA,
    ...(rendererMemoryBuild({ build: { features: opts.features } }) ? { diagnostic: true } : {}),
    recordedAt: new Date().toISOString(),
    exe: {
      path: exe,
      sha256: exeSha,
      bytes: exeBytes.length,
      role: buildExe && buildExe.path === exe ? "build-output" : "frozen-copy",
    },
    buildExe,
    source: {
      repo,
      sha,
      dirty: dirty.dirty,
      patchSha256: dirty.patchSha256,
      patchBytes: dirty.patchBytes,
      untracked: dirty.untracked,
      dirtyIdentitySha256: dirty.dirtyIdentitySha256,
    },
    cargoLockSha256: existsSync(join(repo, "Cargo.lock"))
      ? sha256File(join(repo, "Cargo.lock"))
      : null,
    toolchain: {
      rustupToolchain: opts.toolchain ?? process.env.RUSTUP_TOOLCHAIN ?? null,
      rustc: rustcInfo,
      cargo: cargo.ok ? cargo.stdout.trim() : { error: cargo.error },
    },
    build: {
      triple,
      tripleExplicit: opts.triple ?? null,
      profile: opts.profile,
      features: opts.features ?? [],
      noDefaultFeatures: Boolean(opts.noDefaultFeatures),
      cargoBuildJobs: process.env.CARGO_BUILD_JOBS ?? null,
      rustflags: process.env.RUSTFLAGS ?? null,
      cargoEncodedRustflags: process.env.CARGO_ENCODED_RUSTFLAGS ?? null,
      targetDir,
      command: opts.buildCommand ?? null,
    },
    vendor: {
      chosen,
      explicit: explicitVendor,
      targetDirChosen,
      explicitMatchesTargetDir:
        explicitVendor && targetDirChosen
          ? explicitVendor.treeSha256 === targetDirChosen.treeSha256
          : null,
      candidates,
      expectedAtSha: { sha256: atSha.sha256, fileCount: atSha.fileCount },
      expectedWorkingTree: working
        ? { sha256: working.sha256, fileCount: working.fileCount }
        : null,
      expectedBasis: dirty.dirty ? "working-tree-at-record" : "git-sha",
      matchesExpected: Boolean(chosen && chosen.treeSha256 === expected.sha256),
      honoTreeSha256: honoDir && existsSync(honoDir) ? treeHashDir(honoDir).sha256 : null,
    },
    embeddedBytes: { basis: dirty.dirty ? "working-tree-at-record" : "git-sha", ...embedded },
  };
}

const HARNESS_EXCLUDE_TOP = new Set(["__tests__", "fixtures", "profiles", "README.md"]);

export function harnessTreeHash(harnessDir = HARNESS_DIR) {
  return treeHashDir(harnessDir, {
    filter: (rel, kind, name) => {
      const top = rel.split("/")[0];
      if (HARNESS_EXCLUDE_TOP.has(top)) return false;
      return !name.startsWith(".");
    },
  }).sha256;
}

export function fixtureTreeHash(fixtureDir) {
  return treeHashDir(fixtureDir, { filter: (_rel, _kind, name) => !name.startsWith(".") }).sha256;
}

export const FROZEN_STAMP = ".frozen.json";
export const FROZEN_SCHEMA = "dev-browser-observe.frozen.v1";

/** Every file of a harness copy (tests, fixtures and profiles included), dot-names excluded. */
export function fullTreeHash(dir) {
  return treeHashDir(dir, { filter: (_rel, _kind, name) => !name.startsWith(".") }).sha256;
}

/** Whether a frozen copy's @playwright/test link still points at the stamped package dir. */
export function frozenPlaywrightLink(stamp) {
  const link = stamp.playwright?.link ?? null;
  const expected = stamp.playwright?.target ?? null;
  let actual = null;
  try {
    actual = link ? readlinkSync(link) : null;
  } catch {
    actual = null;
  }
  return {
    ok: expected !== null && actual === expected && existsSync(expected),
    link,
    expected,
    actual,
  };
}

/** A frozen copy's stamp and whether its bytes still equal the frozen hashes; null outside a frozen copy. */
export function frozenState(harnessDir = HARNESS_DIR) {
  const stampPath = join(harnessDir, FROZEN_STAMP);
  if (!existsSync(stampPath)) return null;
  const stamp = JSON.parse(readFileSync(stampPath, "utf8"));
  const harness = harnessTreeHash(harnessDir);
  const full = fullTreeHash(harnessDir);
  return {
    stamp,
    stampSha256: sha256File(stampPath),
    playwrightLink: frozenPlaywrightLink(stamp),
    actual: { harnessTreeSha256: harness, fullTreeSha256: full },
    intact:
      stamp.schema === FROZEN_SCHEMA &&
      harness === stamp.harnessTreeSha256 &&
      full === stamp.fullTreeSha256,
  };
}

/**
 * Identity checks a run from a frozen copy adds (none outside one): the copy's bytes still equal
 * its stamp, and its @playwright/test link still points at the stamped package. Shared by the
 * source-built and fixed-distribution verdicts.
 */
export function frozenIdentityChecks(harnessDir = HARNESS_DIR) {
  const frozen = frozenState(harnessDir);
  if (!frozen) return [];
  return [
    {
      name: "harness-frozen-copy-intact",
      ok: frozen.intact,
      expected: {
        harnessTreeSha256: frozen.stamp.harnessTreeSha256,
        fullTreeSha256: frozen.stamp.fullTreeSha256,
      },
      actual: frozen.actual,
    },
    {
      name: "harness-frozen-playwright-link",
      ok: frozen.playwrightLink.ok,
      expected: frozen.playwrightLink.expected,
      actual: frozen.playwrightLink.actual,
    },
  ];
}

export function harnessCapability(harnessDir = HARNESS_DIR) {
  const frozen = frozenState(harnessDir);
  if (frozen)
    return {
      frozen: true,
      repo: null,
      sha: frozen.stamp.source?.sha ?? null,
      sourceRepo: frozen.stamp.source?.repo ?? null,
      harnessDirDirty: frozen.stamp.source?.harnessDirDirty ?? null,
      harnessDirPatchSha256: frozen.stamp.source?.harnessDirPatchSha256 ?? null,
      harnessDirUntracked: frozen.stamp.source?.harnessDirUntracked ?? null,
      frozenAt: frozen.stamp.frozenAt ?? null,
      frozenDir: harnessDir,
      stampSha256: frozen.stampSha256,
      harnessTreeSha256: frozen.stamp.harnessTreeSha256 ?? null,
      fullTreeSha256: frozen.stamp.fullTreeSha256 ?? null,
      intact: frozen.intact,
    };
  const top = gitToplevel(harnessDir);
  const state = gitDirtyState(top, [harnessDir]);
  return {
    repo: top,
    sha: gitHead(top),
    harnessDirDirty: state.dirty,
    harnessDirPatchSha256: state.patchSha256,
    harnessDirUntracked: state.untracked,
  };
}

/** Every `node_modules/@takazudo/zfb*` the app dir or an ancestor exposes to module resolution. */
export function findLocalTakazudoPackages(appDir) {
  const found = [];
  let dir = resolve(appDir);
  for (;;) {
    const scope = join(dir, "node_modules", "@takazudo");
    if (existsSync(scope)) {
      for (const name of readdirSync(scope))
        if (name.startsWith("zfb")) found.push(join(scope, name));
    }
    const parent = dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return found;
}

/** Every node_modules dir module resolution could reach from `dir` (informational). */
export function ancestorNodeModules(dir) {
  const found = [];
  let cur = resolve(dir);
  for (;;) {
    if (existsSync(join(cur, "node_modules"))) found.push(join(cur, "node_modules"));
    const parent = dirname(cur);
    if (parent === cur) break;
    cur = parent;
  }
  return found;
}

function check(name, ok, expected, actual, extra = {}) {
  return { name, ok: Boolean(ok), expected, actual, ...extra };
}

/**
 * Pure verdict over already-collected facts, so tests can drive every
 * rejection without a real binary. `facts` is produced by `collectIdentityFacts`.
 */
export function evaluateIdentity(profile, receipt, facts) {
  const checks = [];
  checks.push(
    check(
      "receipt-schema",
      receipt?.schema === BUILD_RECEIPT_SCHEMA,
      BUILD_RECEIPT_SCHEMA,
      receipt?.schema,
    ),
  );
  checks.push(
    check(
      "exe-sha256",
      facts.exeSha256 === receipt?.exe?.sha256,
      receipt?.exe?.sha256,
      facts.exeSha256,
    ),
  );
  checks.push(
    check(
      "product-sha",
      receipt?.source?.sha === profile.productSha,
      profile.productSha,
      receipt?.source?.sha,
    ),
  );
  const rendererFeature = rendererMemoryBuild(receipt);
  const rendererStudy = Boolean(
    profile.rendererMemoryStudy || profile.startupAcceptance || profile.tinyAcceptance,
  );
  if (rendererFeature || rendererStudy || receipt?.diagnostic === true)
    checks.push(
      check(
        "renderer-memory-diagnostic-build",
        rendererStudy &&
          rendererFeature &&
          receipt?.diagnostic === true &&
          receipt?.build?.profile === "dev" &&
          receipt?.build?.noDefaultFeatures === false,
        "renderer-memory diagnostic with a declared dev build and default features",
        { rendererStudy, rendererFeature, diagnostic: receipt?.diagnostic, build: receipt?.build },
      ),
    );
  const declaredDirty = profile.productDirtyIdentity ?? null;
  const receiptDirty = receipt?.source?.dirty ? receipt.source.dirtyIdentitySha256 : null;
  checks.push(check("product-patch", declaredDirty === receiptDirty, declaredDirty, receiptDirty));
  const expectedVendor = receipt?.source?.dirty
    ? receipt?.vendor?.expectedWorkingTree?.sha256
    : facts.sourceAtReceiptSha256;
  checks.push(
    check(
      "embedded-vendor-vs-source",
      Boolean(expectedVendor) && receipt?.vendor?.chosen?.treeSha256 === expectedVendor,
      expectedVendor,
      receipt?.vendor?.chosen?.treeSha256,
    ),
  );
  const emb = receipt?.embeddedBytes;
  checks.push(
    check(
      "embedded-bytes-in-exe",
      Boolean(emb) && emb.checked > 0 && emb.missing.length === 0,
      "every embedded source file's bytes present in exe",
      emb ? { checked: emb.checked, missing: emb.missing.length } : null,
    ),
  );
  checks.push(
    check("no-local-takazudo-packages", facts.localTakazudo.length === 0, [], facts.localTakazudo),
  );
  for (const key of ["harnessTreeSha256", "fixtureTreeSha256", "observerSha256", "adapterSha256"]) {
    const pinned = profile.pins?.[key] ?? null;
    checks.push(
      check(`pin:${key}`, pinned !== null && pinned === facts.pins[key], pinned, facts.pins[key]),
    );
  }
  return { ok: checks.every((c) => c.ok), checks };
}

export function currentPins({ fixtureDir, observerPath, adapterPath, harnessDir = HARNESS_DIR }) {
  return {
    harnessTreeSha256: harnessTreeHash(harnessDir),
    fixtureTreeSha256: fixtureTreeHash(fixtureDir),
    observerSha256: sha256File(observerPath),
    adapterSha256: sha256File(adapterPath),
  };
}

export function collectIdentityFacts({ receipt, appDirs, pinInputs }) {
  const exePath = receipt?.exe?.path;
  const exeSha256 =
    exePath && isAbsolute(exePath) && existsSync(exePath) ? sha256File(exePath) : null;
  let sourceAtReceiptSha256 = null;
  try {
    sourceAtReceiptSha256 = sourcePackagesAtSha(receipt.source.repo, receipt.source.sha).sha256;
  } catch {
    sourceAtReceiptSha256 = null;
  }
  const localTakazudo = [...new Set(appDirs.flatMap((d) => findLocalTakazudoPackages(d)))];
  return { exeSha256, sourceAtReceiptSha256, localTakazudo, pins: currentPins(pinInputs) };
}
