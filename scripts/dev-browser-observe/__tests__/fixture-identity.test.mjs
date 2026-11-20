import { mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
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
} from "../lib/fixture.mjs";
import {
  BUILD_RECEIPT_SCHEMA,
  chooseVendorCandidate,
  embeddedBytesCheck,
  evaluateIdentity,
  findLocalTakazudoPackages,
  isEmbeddedSrcPath,
  locateVendorCandidates,
  recordBuild,
  sourcePackagesWorkingTree,
} from "../lib/identity.mjs";
import { HARNESS_DIR, gitToplevel } from "../lib/util.mjs";

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
      path: "../external-content/articles",
      allowOutsideRoot: true,
    });
    const ext = materialize({
      fixtureDir: FIXTURE,
      workDir: join(tmp(), "w"),
      placement: "external",
      adapter: "A0",
    });
    expect(ext.contentDir.endsWith("external-content/articles")).toBe(true);
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
    expect(del.write.kind).toBe("unlink");
  });

  it("performs writes with t0 taken before the syscall, then restores and verifies bytes", () => {
    const m = materialize({
      fixtureDir: FIXTURE,
      workDir: join(tmp(), "w"),
      placement: "internal",
      adapter: "A0",
    });
    const snap = snapshotSources(m);
    const calls = [];
    const a = prepareAttempt(
      m,
      { op: "edit-body", article: "a", writeMode: "rename" },
      { token: "tok-r" },
    );
    performWrite(
      a.write,
      () => {
        calls.push("t0");
        return 1n;
      },
      {
        writeFileSync: (p, b) => {
          calls.push(`write:${p.includes(".dbo-tmp") ? "tmp" : "target"}`);
          writeFileSync(p, b);
        },
        renameSync: (x, y) => {
          calls.push("rename");
          return renameSync(x, y);
        },
      },
    );
    expect(calls).toEqual(["write:tmp", "t0", "rename"]);
    expect(readFileSync(m.articles.a, "utf8")).toContain("tok-r");
    expect(verifySnapshot(snap)).toHaveLength(1);
    const r = restoreSnapshot(snap);
    expect(r.restored).toEqual([m.articles.a]);
    expect(r.mismatches).toEqual([]);
    const live = originalLiveChecks(m, snap);
    expect(live.map((c) => c.path)).toEqual(["/articles/a", "/articles/b", "/independent"]);
    expect(live[0].predicate.equals).toBe("Article A");
  });

  it("reports an unexpected extra file as a restoration mismatch", () => {
    const m = materialize({
      fixtureDir: FIXTURE,
      workDir: join(tmp(), "w"),
      placement: "internal",
      adapter: "A0",
    });
    const snap = snapshotSources(m);
    writeFileSync(join(m.contentDir, "c.md"), "x");
    expect(restoreSnapshot(snap).mismatches).toEqual([
      { path: join(m.contentDir, "c.md"), want: null, got: "unexpected-file" },
    ]);
  });
});

describe("identity", () => {
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
    const repo = gitToplevel(HARNESS_DIR);
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
    if (!receipt.source.dirty) expect(receipt.vendor.matchesExpected).toBe(true);
    else expect(receipt.vendor.chosen.treeSha256).toBe(receipt.vendor.expectedWorkingTree.sha256);
    expect(locateVendorCandidates(target, { triple: null, profile: "dev" })).toHaveLength(1);
    expect(
      chooseVendorCandidate([{ ...receipt.vendor.chosen, declaredVendorDir: null }]),
    ).toBeNull();
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
