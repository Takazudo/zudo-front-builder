import { constants, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import {
  TRANSFORMS,
  materialize,
  originalLiveChecks,
  performWrite,
  prepareAttempt,
  restoreSnapshot,
  snapshotSources,
  verifySnapshot,
  zfbConfigFor,
  loadFixture,
  REAL_FS,
} from "../lib/fixture.mjs";
import {
  BUILD_RECEIPT_SCHEMA,
  chooseVendorCandidate,
  dirtyIdentity,
  embeddedBytesCheck,
  evaluateIdentity,
  findLocalTakazudoPackages,
  isEmbeddedSrcPath,
  locateVendorCandidates,
  recordBuild,
  sourcePackagesWorkingTree,
} from "../lib/identity.mjs";
import { execFileSync } from "node:child_process";
import { HARNESS_DIR } from "../lib/util.mjs";

const FIXTURE = join(HARNESS_DIR, "fixtures", "tiny-two-article");
const tmps = [];
const tmp = () => {
  const d = mkdtempSync(join(tmpdir(), "dbo-test-"));
  tmps.push(d);
  return d;
};
afterEach(() => {
  while (tmps.length) rmSync(tmps.pop(), { recursive: true, force: true });
});

describe("tiny-two-article fixture", () => {
  it("materializes internal and external placements with the right config", () => {
    const manifest = loadFixture(FIXTURE);
    expect(zfbConfigFor(manifest, "internal").collections[0]).toEqual({
      name: "articles",
      path: "content/articles",
    });
    expect(zfbConfigFor(manifest, "external").collections[0]).toEqual({
      name: "articles",
      path: "../content/articles",
      allowOutsideRoot: true,
    });
    const ext = materialize({
      fixtureDir: FIXTURE,
      workDir: join(tmp(), "w"),
      placement: "external",
      adapter: "A0",
    });
    expect(ext.contentDir.endsWith("/w/content/articles")).toBe(true);
    expect(readFileSync(join(ext.appDir, "lib", "content.ts"), "utf8")).toContain(
      'getCollection<ArticleData>("articles")',
    );
    expect(JSON.parse(readFileSync(join(ext.appDir, "zfb.config.json"), "utf8")).wind).toBe(false);
  });

  it("refuses an existing work dir and an unknown adapter", () => {
    const d = tmp();
    expect(() =>
      materialize({ fixtureDir: FIXTURE, workDir: d, placement: "internal", adapter: "A0" }),
    ).toThrow(/already exists/);
    expect(() =>
      materialize({
        fixtureDir: FIXTURE,
        workDir: join(d, "x"),
        placement: "internal",
        adapter: "A9",
      }),
    ).toThrow(/unknown adapter/);
  });

  it("transforms carry the token into exactly the intended field", () => {
    const src = readFileSync(join(FIXTURE, "content/articles/a.md"), "utf8");
    expect(TRANSFORMS["edit-title"](src, "tok-1", "a")).toContain("title: Article A tok-1\n");
    expect(TRANSFORMS["edit-category"](src, "tok-2", "a")).toContain("category: tok-2\n");
    expect(TRANSFORMS["edit-body"](src, "tok-3", "a")).toContain(
      "Body marker tok-3 for article a.",
    );
    expect(() => TRANSFORMS["edit-title"]("no frontmatter", "t", "a")).toThrow(/no title line/);
  });

  it("prepares attempts with filled predicates and supplemental checks", () => {
    const m = materialize({
      fixtureDir: FIXTURE,
      workDir: join(tmp(), "w"),
      placement: "internal",
      adapter: "A0",
    });
    const a = prepareAttempt(m, { op: "edit-title", article: "a" }, { token: "tok-x" });
    expect(a.predicate).toEqual({
      kind: "text",
      selector: '[data-article-title="a"]',
      includes: "tok-x",
    });
    expect(a.supplemental[0]).toEqual({
      path: "/",
      predicate: { kind: "text", selector: '[data-home-title="a"]', includes: "tok-x" },
    });
    const stale = prepareAttempt(
      m,
      { op: "stale-predicate", article: "a", expect: "timeout" },
      { token: "w", unwrittenToken: "never" },
    );
    expect(stale.predicate.includes).toBe("never");
    expect(stale.write.bytes.toString()).toContain("w");
    const css = prepareAttempt(m, { op: "edit-css" }, { token: "tok-c" });
    expect(css).toMatchObject({
      targetPath: "/independent",
      predicate: { kind: "computed-style", property: "--observe-css-marker", includes: "tok-c" },
    });
    const del = prepareAttempt(m, { op: "delete-article", article: "b" });
    expect(del.write.method).toBe("unlink");
    expect(prepareAttempt(m, { op: "edit-title", article: "a" }).write.method).toBe("in-place");
    expect(() =>
      prepareAttempt(m, { op: "edit-title", article: "a", writeMethod: "unlink" }),
    ).toThrow(/does not allow writeMethod unlink/);
    const ren = prepareAttempt(m, { op: "rename-article", article: "b", writeMethod: "rename" });
    expect(ren.write.to.endsWith("b-renamed.md")).toBe(true);
    expect(ren.predicate).toEqual({
      kind: "count",
      selector: '[data-home-item="b-renamed"]',
      equals: 1,
    });
  });

  const mat = () =>
    materialize({
      fixtureDir: FIXTURE,
      workDir: join(tmp(), "w"),
      placement: "internal",
      adapter: "A0",
    });
  const tracing = (calls) => ({
    ...REAL_FS,
    openSync: (p, f) => {
      calls.push(
        `open:${f & constants.O_CREAT ? "creat" : "nocreat"}:${f & constants.O_TRUNC ? "trunc" : ""}`,
      );
      return REAL_FS.openSync(p, f);
    },
    writeFileSync: (p, b, o) => {
      calls.push(
        `writeFile:${basename(p).startsWith(".") ? "temp-sibling" : "target"}:${dirname(p)}`,
      );
      return REAL_FS.writeFileSync(p, b, o);
    },
    renameSync: (x, y) => {
      calls.push("rename");
      return REAL_FS.renameSync(x, y);
    },
  });
  const t0 = (calls) => () => {
    calls.push("t0");
    return 1n;
  };

  it("in-place is a truncate-write on the existing inode with t0 before open", () => {
    const m = mat();
    const calls = [];
    const a = prepareAttempt(
      m,
      { op: "edit-body", article: "a", writeMethod: "in-place" },
      { token: "tok-i" },
    );
    performWrite(a.write, t0(calls), tracing(calls));
    expect(calls).toEqual(["t0", "open:nocreat:trunc"]);
    expect(a.write.evidence).toMatchObject({ method: "in-place" });
    expect(a.write.evidence.inodeAfter).toBe(a.write.evidence.inodeBefore);
    expect(readFileSync(m.articles.a, "utf8")).toContain("tok-i");
  });

  it("atomic-rename writes a temp sibling in the target dir, then renames (new inode)", () => {
    const m = mat();
    const calls = [];
    const a = prepareAttempt(
      m,
      { op: "edit-body", article: "a", writeMethod: "atomic-rename" },
      { token: "tok-r" },
    );
    performWrite(a.write, t0(calls), tracing(calls));
    expect(calls).toEqual([`writeFile:temp-sibling:${m.contentDir}`, "t0", "rename"]);
    expect(dirname(a.write.evidence.tempSibling)).toBe(m.contentDir);
    expect(a.write.evidence.inodeAfter).not.toBe(a.write.evidence.inodeBefore);
    expect(readFileSync(m.articles.a, "utf8")).toContain("tok-r");
  });

  it("lifecycle methods refuse the wrong precondition", () => {
    const m = mat();
    const noop = () => 1n;
    expect(() =>
      performWrite(
        { method: "in-place", path: join(m.contentDir, "zz.md"), bytes: Buffer.from("x") },
        noop,
      ),
    ).toThrow(/in-place target missing/);
    expect(() =>
      performWrite({ method: "create", path: m.articles.a, bytes: Buffer.from("x") }, noop),
    ).toThrow(/create target exists/);
    expect(() =>
      performWrite({ method: "rename", path: m.articles.a, to: m.articles.b }, noop),
    ).toThrow(/destination exists/);
    expect(() => performWrite({ method: "editor-save", path: m.articles.a }, noop)).toThrow(
      /unknown write method/,
    );
  });

  it("restores with explicit methods: in-place for changed, create for missing, unlink for extra", () => {
    const m = mat();
    const snap = snapshotSources(m);
    performWrite(
      prepareAttempt(m, { op: "edit-title", article: "a" }, { token: "t1" }).write,
      () => 1n,
    );
    performWrite(prepareAttempt(m, { op: "rename-article", article: "b" }).write, () => 1n);
    const r = restoreSnapshot(snap, { writeMethod: "in-place" });
    expect(r.actions.map((x) => [basename(x.path), x.method])).toEqual([
      ["a.md", "in-place"],
      ["b.md", "create"],
      ["b-renamed.md", "unlink"],
    ]);
    expect(r.mismatches).toEqual([]);
    expect(r.writeMethod).toBe("in-place");
    expect(() => restoreSnapshot(snap, { writeMethod: "unlink" })).toThrow(/not allowed/);
    const live = originalLiveChecks(m, snap);
    expect(live.map((c) => c.path)).toEqual(["/articles/a", "/articles/b", "/", "/independent"]);
    expect(live[0].predicate.equals).toBe("Article A");
    expect(live[2].predicate).toEqual({ kind: "count", selector: "[data-home-item]", equals: 2 });
  });

  it("restoration can use atomic-rename when the profile says so", () => {
    const m = mat();
    const snap = snapshotSources(m);
    performWrite(prepareAttempt(m, { op: "edit-css" }, { token: "t2" }).write, () => 1n);
    const r = restoreSnapshot(snap, { writeMethod: "atomic-rename" });
    expect(r.actions).toHaveLength(1);
    expect(r.actions[0]).toMatchObject({ method: "atomic-rename" });
    expect(r.mismatches).toEqual([]);
  });

  it("removes an unexpected extra file with an explicit unlink", () => {
    const m = materialize({
      fixtureDir: FIXTURE,
      workDir: join(tmp(), "w"),
      placement: "internal",
      adapter: "A0",
    });
    const snap = snapshotSources(m);
    writeFileSync(join(m.contentDir, "c.md"), "x");
    expect(verifySnapshot(snap)).toEqual([
      { path: join(m.contentDir, "c.md"), want: null, got: "unexpected-file" },
    ]);
    const r = restoreSnapshot(snap);
    expect(r.actions).toMatchObject([{ path: join(m.contentDir, "c.md"), method: "unlink" }]);
    expect(r.mismatches).toEqual([]);
  });
});

/**
 * A tiny committed git repo with the three embedded packages (plus files
 * build.rs's copy_ts_src filter must skip), so record-build tests never hash
 * or diff the real checkout.
 */
function makeTinySourceRepo() {
  const repo = mkdtempSync(join(tmpdir(), "dbo-tinyrepo-"));
  const files = {
    "Cargo.lock": "# tiny lock\n",
    "packages/zfb/package.json": '{"name":"@takazudo/zfb"}\n',
    "packages/zfb/src/index.ts": "export const zfb = 1;\n",
    "packages/zfb/src/__tests__/skip.test.ts": "skipped\n",
    "packages/zfb/src/.hidden/skip.ts": "skipped\n",
    "packages/zfb/src/notes.md": "skipped\n",
    "packages/zfb-runtime/package.json": '{"name":"@takazudo/zfb-runtime"}\n',
    "packages/zfb-runtime/src/runtime.ts": "export const runtime = 1;\n",
    "packages/zfb-slugify/package.json": '{"name":"@takazudo/zfb-slugify"}\n',
    "packages/zfb-slugify/src/slugify.ts": "export const slugify = 1;\n",
  };
  for (const [rel, text] of Object.entries(files)) {
    mkdirSync(dirname(join(repo, rel)), { recursive: true });
    writeFileSync(join(repo, rel), text);
  }
  const git = (...args) =>
    execFileSync(
      "git",
      [
        "-c",
        "user.name=dbo-test",
        "-c",
        "user.email=dbo-test@invalid",
        "-c",
        "commit.gpgsign=false",
        ...args,
      ],
      { cwd: repo, stdio: "ignore" },
    );
  git("init", "-q");
  git("add", "-A");
  git("commit", "-q", "-m", "tiny");
  return repo;
}

describe("identity", () => {
  // Built once, outside each test's budget: the git spawns (and a cold macOS git shim) cost seconds on a loaded host.
  let tinyRepo;
  let dirtyRepo;
  beforeAll(() => {
    tinyRepo = makeTinySourceRepo();
    dirtyRepo = makeTinySourceRepo();
    writeFileSync(join(dirtyRepo, "packages/zfb/src/index.ts"), "export const zfb = 2;\n");
    writeFileSync(
      join(dirtyRepo, "packages/zfb-runtime/src/extra.ts"),
      "export const extra = 1;\n",
    );
  });
  afterAll(() => {
    for (const repo of [tinyRepo, dirtyRepo])
      if (repo) rmSync(repo, { recursive: true, force: true });
  });

  it("mirrors build.rs copy_ts_src filtering", () => {
    expect(isEmbeddedSrcPath("content.ts")).toBe(true);
    expect(isEmbeddedSrcPath("zudo-react/index.ts")).toBe(true);
    expect(isEmbeddedSrcPath("__tests__/a.ts")).toBe(false);
    expect(isEmbeddedSrcPath(".hidden/a.ts")).toBe(false);
    expect(isEmbeddedSrcPath("a/__b.ts")).toBe(false);
    expect(isEmbeddedSrcPath("a.tsx")).toBe(false);
    expect(isEmbeddedSrcPath("a.js")).toBe(false);
  });

  it("finds local @takazudo/zfb* packages in the app dir or any ancestor", () => {
    const root = tmp();
    const app = join(root, "deep", "app");
    mkdirSync(app, { recursive: true });
    expect(findLocalTakazudoPackages(app)).toEqual([]);
    mkdirSync(join(root, "node_modules", "@takazudo", "zfb-runtime"), { recursive: true });
    mkdirSync(join(root, "node_modules", "@takazudo", "mdx-formatter"), { recursive: true });
    expect(findLocalTakazudoPackages(app)).toEqual([
      join(root, "node_modules", "@takazudo", "zfb-runtime"),
    ]);
  });

  it("detects missing embedded bytes", () => {
    const contents = [
      { path: "x", bytes: Buffer.from("alpha") },
      { path: "y", bytes: Buffer.from("omega") },
      { path: "z", bytes: Buffer.alloc(0) },
    ];
    expect(embeddedBytesCheck(Buffer.from("..alpha.."), contents)).toEqual({
      checked: 2,
      missing: ["y"],
    });
  });

  it("record-build locates the vendor out dir and binds it to the source and exe bytes", () => {
    const repo = tinyRepo;
    const target = tmp();
    const working = sourcePackagesWorkingTree(repo);
    const outDir = join(target, "debug", "build", "zfb-0123abcd", "out");
    for (const c of working.contents) {
      const p = join(outDir, "vendor", c.path);
      mkdirSync(join(p, ".."), { recursive: true });
      writeFileSync(p, c.bytes);
    }
    writeFileSync(
      join(target, "debug", "build", "zfb-0123abcd", "output"),
      `cargo:rustc-env=ZFB_VENDOR_DIR=${join(outDir, "vendor")}\n`,
    );
    const exe = join(target, "debug", "zfb");
    writeFileSync(
      exe,
      Buffer.concat([Buffer.from("HEADER"), ...working.contents.map((c) => c.bytes)]),
    );
    const receipt = recordBuild({
      exe,
      profile: "dev",
      repo,
      targetDir: target,
      probe: {
        rustc: () => ({ ok: true, stdout: "rustc 1.0.0\nhost: aarch64-apple-darwin\n" }),
        cargo: () => ({ ok: true, stdout: "cargo 1.0.0" }),
      },
    });
    expect(receipt.schema).toBe(BUILD_RECEIPT_SCHEMA);
    expect(receipt.vendor.chosen.outDir).toBe(outDir);
    expect(receipt.build.triple).toBe("aarch64-apple-darwin");
    expect(receipt.embeddedBytes.missing).toEqual([]);
    expect(receipt.source.dirty).toBe(false);
    expect(receipt.vendor.matchesExpected).toBe(true);
    expect(receipt.vendor.expectedAtSha.fileCount).toBe(6);
    expect(locateVendorCandidates(target, { triple: null, profile: "dev" })).toHaveLength(1);
    expect(
      chooseVendorCandidate([{ ...receipt.vendor.chosen, declaredVendorDir: null }]),
    ).toBeNull();
  });

  it("record-build on a dirty tree binds the vendor to the working tree and the dirty identity", () => {
    const repo = dirtyRepo;
    const target = tmp();
    const working = sourcePackagesWorkingTree(repo);
    const outDir = join(target, "debug", "build", "zfb-d1d1d1d1", "out");
    for (const c of working.contents) {
      const p = join(outDir, "vendor", c.path);
      mkdirSync(join(p, ".."), { recursive: true });
      writeFileSync(p, c.bytes);
    }
    writeFileSync(
      join(target, "debug", "build", "zfb-d1d1d1d1", "output"),
      `cargo:rustc-env=ZFB_VENDOR_DIR=${join(outDir, "vendor")}\n`,
    );
    const exe = join(target, "debug", "zfb");
    writeFileSync(exe, Buffer.concat(working.contents.map((c) => c.bytes)));
    const receipt = recordBuild({
      exe,
      profile: "dev",
      repo,
      targetDir: target,
      probe: {
        rustc: () => ({ ok: true, stdout: "host: aarch64-apple-darwin\n" }),
        cargo: () => ({ ok: true, stdout: "cargo" }),
      },
    });
    const dirty = dirtyIdentity(repo);
    expect(receipt.source.dirty).toBe(true);
    expect(receipt.source.untracked).toEqual(["packages/zfb-runtime/src/extra.ts"]);
    expect(receipt.source.patchBytes).toBeGreaterThan(0);
    expect(receipt.source.dirtyIdentitySha256).toBe(dirty.dirtyIdentitySha256);
    expect(receipt.source.dirtyIdentitySha256).toMatch(/^[0-9a-f]{64}$/);
    expect(receipt.vendor.expectedBasis).toBe("working-tree-at-record");
    expect(receipt.vendor.expectedWorkingTree).toEqual({
      sha256: working.sha256,
      fileCount: 7,
    });
    expect(receipt.vendor.expectedAtSha.fileCount).toBe(6);
    expect(receipt.vendor.expectedWorkingTree.sha256).not.toBe(receipt.vendor.expectedAtSha.sha256);
    expect(receipt.vendor.chosen.outDir).toBe(outDir);
    expect(receipt.vendor.chosen.treeSha256).toBe(receipt.vendor.expectedWorkingTree.sha256);
    expect(receipt.vendor.matchesExpected).toBe(true);
    expect(receipt.embeddedBytes).toMatchObject({ basis: "working-tree-at-record", missing: [] });
  });

  it("record-build binds an explicit frozen exe and vendor copy and records the original build exe", () => {
    const repo = tinyRepo;
    const root = tmp();
    const working = sourcePackagesWorkingTree(repo);
    const writeVendor = (dir) => {
      for (const c of working.contents) {
        const p = join(dir, c.path);
        mkdirSync(join(p, ".."), { recursive: true });
        writeFileSync(p, c.bytes);
      }
    };
    const target = join(root, "target");
    const outDir = join(target, "debug", "build", "zfb-feedface", "out");
    writeVendor(join(outDir, "vendor"));
    writeFileSync(
      join(target, "debug", "build", "zfb-feedface", "output"),
      `cargo:rustc-env=ZFB_VENDOR_DIR=${join(outDir, "vendor")}\n`,
    );
    const bytes = Buffer.concat(working.contents.map((c) => c.bytes));
    writeFileSync(join(target, "debug", "zfb"), bytes);
    const frozen = join(root, "frozen");
    writeVendor(join(frozen, "vendor"));
    writeFileSync(join(frozen, "zfb"), bytes);
    const receipt = recordBuild({
      exe: join(frozen, "zfb"),
      vendorDir: join(frozen, "vendor"),
      profile: "dev",
      repo,
      targetDir: target,
      probe: {
        rustc: () => ({ ok: true, stdout: "host: aarch64-apple-darwin\n" }),
        cargo: () => ({ ok: true, stdout: "cargo" }),
      },
    });
    expect(receipt.exe).toMatchObject({ path: join(frozen, "zfb"), role: "frozen-copy" });
    expect(receipt.buildExe).toMatchObject({
      path: join(target, "debug", "zfb"),
      matchesExe: true,
    });
    expect(receipt.vendor.chosen).toMatchObject({
      source: "explicit",
      vendorDir: join(frozen, "vendor"),
    });
    expect(receipt.vendor.targetDirChosen.outDir).toBe(outDir);
    expect(receipt.vendor.explicitMatchesTargetDir).toBe(true);
    expect(receipt.embeddedBytes.missing).toEqual([]);
  });

  const goodProfile = {
    productSha: "a".repeat(40),
    productDirtyIdentity: null,
    pins: {
      harnessTreeSha256: "h",
      fixtureTreeSha256: "f",
      observerSha256: "o",
      adapterSha256: "d",
    },
  };
  const goodReceipt = {
    schema: BUILD_RECEIPT_SCHEMA,
    exe: { sha256: "e" },
    source: { sha: "a".repeat(40), dirty: false, dirtyIdentitySha256: null },
    vendor: { chosen: { treeSha256: "v" } },
    embeddedBytes: { checked: 5, missing: [] },
  };
  const goodFacts = {
    exeSha256: "e",
    sourceAtReceiptSha256: "v",
    localTakazudo: [],
    pins: {
      harnessTreeSha256: "h",
      fixtureTreeSha256: "f",
      observerSha256: "o",
      adapterSha256: "d",
    },
  };
  const failing = (v) => v.checks.filter((c) => !c.ok).map((c) => c.name);

  it("accepts a fully bound identity", () => {
    expect(evaluateIdentity(goodProfile, goodReceipt, goodFacts).ok).toBe(true);
  });

  it.each([
    ["exe hash", {}, {}, { exeSha256: "other" }, "exe-sha256"],
    ["product sha", { productSha: "b".repeat(40) }, {}, {}, "product-sha"],
    [
      "undeclared dirty patch",
      {},
      {
        source: { sha: "a".repeat(40), dirty: true, dirtyIdentitySha256: "p" },
        vendor: { chosen: { treeSha256: "v" }, expectedWorkingTree: { sha256: "v" } },
      },
      {},
      "product-patch",
    ],
    ["vendor vs source", {}, {}, { sourceAtReceiptSha256: "other" }, "embedded-vendor-vs-source"],
    [
      "embedded bytes",
      {},
      { embeddedBytes: { checked: 5, missing: ["@takazudo/zfb/src/content.ts"] } },
      {},
      "embedded-bytes-in-exe",
    ],
    [
      "local package",
      {},
      {},
      { localTakazudo: ["/x/node_modules/@takazudo/zfb"] },
      "no-local-takazudo-packages",
    ],
    [
      "harness pin",
      {},
      {},
      { pins: { ...goodFacts.pins, harnessTreeSha256: "changed" } },
      "pin:harnessTreeSha256",
    ],
    [
      "fixture pin",
      {},
      {},
      { pins: { ...goodFacts.pins, fixtureTreeSha256: "changed" } },
      "pin:fixtureTreeSha256",
    ],
    [
      "observer pin",
      {},
      {},
      { pins: { ...goodFacts.pins, observerSha256: "changed" } },
      "pin:observerSha256",
    ],
    [
      "missing pin",
      { pins: { ...goodProfile.pins, adapterSha256: null } },
      {},
      {},
      "pin:adapterSha256",
    ],
  ])("rejects a %s mismatch", (_n, p, r, f, name) => {
    const v = evaluateIdentity(
      { ...goodProfile, ...p },
      { ...goodReceipt, ...r },
      { ...goodFacts, ...f },
    );
    expect(v.ok).toBe(false);
    expect(failing(v)).toEqual([name]);
  });

  it("accepts a dirty receipt only when the profile declares the same dirty identity", () => {
    const r = {
      ...goodReceipt,
      source: { sha: "a".repeat(40), dirty: true, dirtyIdentitySha256: "p" },
      vendor: { chosen: { treeSha256: "w" }, expectedWorkingTree: { sha256: "w" } },
    };
    expect(evaluateIdentity({ ...goodProfile, productDirtyIdentity: "p" }, r, goodFacts).ok).toBe(
      true,
    );
  });
});
