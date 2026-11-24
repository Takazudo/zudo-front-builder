import {
  cpSync,
  existsSync,
  linkSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
  FN_SEL,
  caseRenameWrites,
  caseVariantName,
  fnEntrySlugOf,
  loadFieldNotesFixture,
  materializeFieldNotes,
  overlayChecks,
  planFieldNotesOp,
} from "../lib/fieldnotes.mjs";
import { FIXTURE_KINDS } from "../lib/fixture-kinds.mjs";
import {
  RENAME_NOOP,
  REAL_FS,
  aliasGuard,
  dirEntries,
  performWrite,
  performWrites,
  probeCaseSensitivity,
  restoreSnapshot,
  snapshotPaths,
  verifySnapshot,
} from "../lib/fixture.mjs";
import { semanticErrors } from "../lib/profile.mjs";
import { validateAs } from "../lib/schema.mjs";
import { DIAGNOSTIC_ENV, serverEnv } from "../lib/server.mjs";
import { classifyOpen, sameTickEvidence } from "../lib/special-modes.mjs";
import { summarize } from "../lib/stats.mjs";
import { batchExitOk, runBatch } from "../lib/state-machine.mjs";
import { HARNESS_DIR, sha256, sha256File } from "../lib/util.mjs";
import { deps, fakePage, fakeServer } from "./fakes.mjs";
import { makeFakeSource } from "./fieldnotes-fake.mjs";

const FIXTURE = join(HARNESS_DIR, "fixtures", "field-notes");
const MANIFEST = loadFieldNotesFixture(FIXTURE);
const OVERLAY = MANIFEST.placements.internal.config;
const tmps = [];
const tmp = () => {
  const d = mkdtempSync(join(tmpdir(), "dbo-q-"));
  tmps.push(d);
  return d;
};
afterEach(() => {
  while (tmps.length) rmSync(tmps.pop(), { recursive: true, force: true });
});

/** The original zfb.config.ts, rebuilt from the overlay by reversing its declared one-hunk diff. */
function baseConfigBytes() {
  const overlay = readFileSync(join(FIXTURE, OVERLAY.overlay), "utf8");
  return overlay.replace(
    '      path: "content/articles",\n',
    '      path: "../../content/articles",\n      allowOutsideRoot: true,\n',
  );
}

function setup({ placement = "external", adapter = "original-memoized", realConfig = true } = {}) {
  const root = tmp();
  const src = join(root, "src");
  const { datasetSha256: ds } = makeFakeSource(src, 20);
  if (realConfig) writeFileSync(join(src, "apps", "zfb", "zfb.config.ts"), baseConfigBytes());
  const m = materializeFieldNotes({
    fixtureDir: FIXTURE,
    sourceDir: src,
    workDir: join(root, "work"),
    adapter,
    managedCss: true,
    placement,
    expectedDatasetSha256: ds,
  });
  return { root, src, m, ds };
}

const fnFs = (m) =>
  FIXTURE_KINDS["field-notes"].ops(m, { restoration: { writeMethod: "in-place" } });

const example = (n) => JSON.parse(readFileSync(join(HARNESS_DIR, "profiles", n), "utf8"));

describe("diagnostic env gating covers ZFB_DEV_PROVENANCE_TRACE", () => {
  it("strips both diagnostic vars outside diagnostic profiles and keeps them inside", () => {
    expect(DIAGNOSTIC_ENV).toEqual(["ZFB_DEV_TIMING", "ZFB_DEV_PROVENANCE_TRACE"]);
    const plain = serverEnv(
      { ZFB_DEV_PROVENANCE_TRACE: "1", ZFB_DEV_TIMING: "1", PATH: "/bin" },
      {},
      false,
    );
    expect(plain.env.ZFB_DEV_PROVENANCE_TRACE).toBeUndefined();
    expect(plain.removedDiagnosticEnv).toEqual(["ZFB_DEV_TIMING", "ZFB_DEV_PROVENANCE_TRACE"]);
    const diag = serverEnv({ PATH: "/bin" }, { ZFB_DEV_PROVENANCE_TRACE: "1" }, true);
    expect(diag.zfbEnv).toEqual({ ZFB_DEV_PROVENANCE_TRACE: "1" });
  });

  it("a non-diagnostic profile may not set ZFB_DEV_PROVENANCE_TRACE", () => {
    const p = example("field-notes-direct-read-q.example.json");
    p.server.env = { ZFB_DEV_PROVENANCE_TRACE: "1" };
    expect(semanticErrors(p).join("\n")).toMatch(/ZFB_DEV_PROVENANCE_TRACE is only allowed/);
    p.diagnostic = true;
    expect(semanticErrors(p).join("\n")).not.toMatch(/ZFB_DEV_PROVENANCE_TRACE/);
  });
});

describe("edit-prop (description)", () => {
  it("declares the field and every rendered output before running; the timed predicate is the detail .lede", () => {
    const { m } = setup();
    const a = planFieldNotesOp(
      m,
      m.model,
      { op: "edit-prop", article: "article-00005", writeMethod: "in-place" },
      { token: "tok-p" },
    );
    expect(a.targetPath).toBe("/articles/article-00005/");
    expect(a.predicate).toEqual({ kind: "text", selector: FN_SEL.lede, includes: "tok-p" });
    expect(a.declared).toMatchObject({
      field: "description",
      from: "Field note 5.",
      to: "Field note 5. tok-p",
    });
    const paths = a.supplemental.map((s) => s.path);
    // home card, archive card, category card, and the referrers' related cards (20 articles: one referrer)
    expect(paths).toEqual([
      "/",
      "/archive/1/",
      "/category/performance/",
      "/articles/article-00015/",
    ]);
    expect(a.supplemental[0].predicate.selector).toBe(FN_SEL.cardDescription("article-00005"));
    expect(a.supplemental[3].predicate.selector).toBe(FN_SEL.relatedDescription("article-00005"));
    expect(a.writes[0].bytes.toString()).toMatch(/^description: "Field note 5\. tok-p"$/m);
    expect(a.nextModel.articles.find((x) => x.slug === "article-00005").description).toBe(
      "Field note 5. tok-p",
    );
  });

  it("targetPath / times the home card; the detail .lede becomes a supplemental", () => {
    const { m } = setup();
    const a = planFieldNotesOp(
      m,
      m.model,
      { op: "edit-prop", article: "article-00006", targetPath: "/", writeMethod: "in-place" },
      { token: "tok-h" },
    );
    expect(a.predicate.selector).toBe(FN_SEL.cardDescription("article-00006"));
    expect(a.supplemental[0]).toEqual({
      path: "/articles/article-00006/",
      predicate: { kind: "text", selector: FN_SEL.lede, includes: "tok-h" },
    });
    expect(() =>
      planFieldNotesOp(m, m.model, { op: "edit-prop", article: "article-00006", prop: "title" }),
    ).toThrow(/cannot edit title/);
  });

  it("accounting: success, then every dependent output checked, then restoration", async () => {
    const { m } = setup();
    const fs = fnFs(m);
    const page = fakePage({ behaviors: [{ kind: "notify", t1: 9_000_000_000_000n }] });
    const checked = [];
    page.supplemental = async (check) => {
      checked.push(check.path);
      return { ok: true };
    };
    const d = deps({ page, fs });
    const { accounting, stopped } = await runBatch({
      scenarios: [
        {
          id: "prop",
          set: "workload",
          ops: [{ id: "p", op: "edit-prop", article: "article-00005", writeMethod: "in-place" }],
        },
      ],
      deps: d,
    });
    expect(accounting).toMatchObject({ success: 1, error: 0, attempted: 1 });
    expect(stopped).toBe(false);
    expect(checked).toHaveLength(4);
    const att = d.recorder.records.find((r) => r.type === "attempt");
    expect(att.declared.field).toBe("description");
    expect(d.recorder.records.find((r) => r.type === "cleanup").ok).toBe(true);
    expect(readFileSync(join(m.contentDir, "article-00005.md"), "utf8")).toMatch(
      /^description: "Field note 5\."$/m,
    );
  });
});

describe("internal placement overlay", () => {
  it("the overlay is the base config with only the collection path changed", () => {
    expect(sha256(Buffer.from(baseConfigBytes()))).toBe(OVERLAY.baseSha256);
    expect(sha256File(join(FIXTURE, OVERLAY.overlay))).toBe(OVERLAY.overlaySha256);
    expect(MANIFEST.placements.internal.provenance).toBe("derived");
    expect(OVERLAY.diff).toContain('-      path: "../../content/articles",');
    expect(OVERLAY.diff).toContain("-      allowOutsideRoot: true,");
    expect(OVERLAY.diff).toContain('+      path: "content/articles",');
    expect(OVERLAY.diff.split("\n").filter((l) => /^[-+][^-+]/.test(l))).toHaveLength(3);
  });

  it("materializes the corpus inside the app and records the derived config; ops follow the placement", () => {
    const { m, root } = setup({ placement: "internal" });
    expect(m.contentDir).toBe(join(root, "work", "apps", "zfb", "content", "articles"));
    expect(existsSync(join(root, "work", "content"))).toBe(false);
    expect(readdirSync(m.contentDir)).toHaveLength(20);
    expect(m.configSha256).toBe(OVERLAY.overlaySha256);
    expect(m.configProvenance).toBe("derived-overlay");
    expect(m.configOverlay).toMatchObject({
      baseSha256: OVERLAY.baseSha256,
      provenance: "derived",
    });
    expect(m.layout).toMatchObject({
      placement: "internal",
      contentDir: "apps/zfb/content/articles",
      collectionPath: "content/articles",
      allowOutsideRoot: false,
    });
    const a = planFieldNotesOp(m, m.model, {
      op: "edit-body",
      article: "article-00003",
      writeMethod: "in-place",
    });
    expect(a.writes[0].path).toBe(join(m.contentDir, "article-00003.md"));
    expect(fnFs(m).snapshot().dirs).toEqual([m.contentDir]);
    const receipt = FIXTURE_KINDS["field-notes"].receiptFixture(m, MANIFEST, {
      fixture: { adapter: "original-memoized", source: { dir: "/s" } },
    });
    expect(receipt).toMatchObject({
      placement: "internal",
      configSha256: OVERLAY.overlaySha256,
      configBaseSha256: OVERLAY.baseSha256,
    });
    expect(receipt.materializedDatasetSha256).toMatch(/^[0-9a-f]{64}$/);
  });

  it("external stays the exact source layout", () => {
    const { m, root } = setup({ placement: "external" });
    expect(m.contentDir).toBe(join(root, "work", "content", "articles"));
    expect(m.configSha256).toBe(OVERLAY.baseSha256);
    expect(m.configProvenance).toBe("source-dir");
  });

  it("a tampered overlay or a different base config is rejected before anything is materialized", () => {
    const fixtureCopy = join(tmp(), "fn");
    cpSync(FIXTURE, fixtureCopy, { recursive: true });
    writeFileSync(
      join(fixtureCopy, OVERLAY.overlay),
      `${readFileSync(join(fixtureCopy, OVERLAY.overlay), "utf8")}// tampered\n`,
    );
    const root = tmp();
    const src = join(root, "src");
    makeFakeSource(src, 20);
    writeFileSync(join(src, "apps", "zfb", "zfb.config.ts"), baseConfigBytes());
    const checks = overlayChecks(fixtureCopy, src, MANIFEST, "internal");
    expect(checks.map((c) => [c.name, c.ok])).toEqual([
      ["placement-overlay:internal:base", true],
      ["placement-overlay:internal:overlay", false],
    ]);
    expect(() =>
      materializeFieldNotes({
        fixtureDir: fixtureCopy,
        sourceDir: src,
        workDir: join(root, "work"),
        adapter: "direct-read",
        placement: "internal",
      }),
    ).toThrow(/overlay identity mismatch/);
    expect(existsSync(join(root, "work"))).toBe(false);
    // The fake's own placeholder config is not the original base.
    const root2 = tmp();
    makeFakeSource(join(root2, "src"), 20);
    expect(() =>
      materializeFieldNotes({
        fixtureDir: FIXTURE,
        sourceDir: join(root2, "src"),
        workDir: join(root2, "work"),
        adapter: "direct-read",
        placement: "internal",
      }),
    ).toThrow(/placement-overlay:internal:base/);
  });

  it("verify-identity carries the overlay checks for an internal profile", () => {
    const fixtureCopy = join(tmp(), "fn");
    cpSync(FIXTURE, fixtureCopy, { recursive: true });
    writeFileSync(join(fixtureCopy, OVERLAY.overlay), "tampered\n");
    const root = tmp();
    makeFakeSource(join(root, "src"), 20);
    const profile = {
      fixture: { kind: "field-notes", placement: "internal", adapter: "direct-read", source: {} },
    };
    const checks = FIXTURE_KINDS["field-notes"].extraChecks(
      profile,
      { fixtureDir: fixtureCopy, sourceDir: join(root, "src") },
      MANIFEST,
    );
    const overlay = checks.filter((c) => c.name.startsWith("placement-overlay"));
    expect(overlay).toHaveLength(2);
    expect(overlay.every((c) => c.ok === false)).toBe(true);
  });
});

describe("rename-case-only", () => {
  it("slug derivation: zfb keeps the file name's case, so the entry slug no longer matches the frontmatter", () => {
    expect(caseVariantName("article-00008.md")).toBe("Article-00008.md");
    expect(fnEntrySlugOf("Article-00008.md")).toBe("Article-00008");
  });

  it("probes the volume's case behaviour at run time and records the probe", () => {
    const d = tmp();
    const p = probeCaseSensitivity(d);
    expect(typeof p.caseSensitive).toBe("boolean");
    expect(p.sameInode).toBe(!p.caseSensitive);
    expect(readdirSync(d)).toEqual([]);
  });

  it("plans an expected-negative attempt only from a declared expectState, with readdir verification and a rename-back recovery", () => {
    const { m } = setup();
    const a = planFieldNotesOp(m, m.model, {
      op: "rename-case-only",
      article: "article-00008",
      writeMethod: "rename",
      expectState: ["error", "last-good"],
    });
    expect(a.mode).toBe("expected-negative");
    expect(a.writes).toEqual([
      {
        method: "rename",
        path: join(m.contentDir, "article-00008.md"),
        to: join(m.contentDir, "Article-00008.md"),
        caseOnly: true,
        verifyEntries: {
          dir: m.contentDir,
          present: ["Article-00008.md"],
          absent: ["article-00008.md"],
        },
      },
    ]);
    expect(a.recovery[0]).toMatchObject({
      path: join(m.contentDir, "Article-00008.md"),
      to: join(m.contentDir, "article-00008.md"),
      caseOnly: true,
    });
    expect(a.errorMarkers).toEqual(["Article slug mismatch"]);
    expect(a.declared).toMatchObject({
      entrySlugAfter: "Article-00008",
      frontmatterSlug: "article-00008",
      expectation: { set: "expected-negative", expectState: ["error", "last-good"] },
    });
    expect(typeof a.declared.caseProbe.caseSensitive).toBe("boolean");
    const via = planFieldNotesOp(m, m.model, {
      op: "rename-case-only",
      article: "article-00008",
      writeMethod: "rename",
      caseRenameSteps: "via-temp",
      expectState: ["error"],
    });
    expect(via.writes).toHaveLength(2);
    expect(via.writes[0].to).toMatch(/\.article-00008\.md\.dbo-case-[0-9a-f]+\.tmp$/);
    expect(() =>
      planFieldNotesOp(m, m.model, { op: "rename-case-only", article: "article-00008" }),
    ).toThrow(/declared targetPath and predicate/);
  });

  it("a real case rename changes the directory entry (either case behaviour), and restoration renames it back on the same inode", async () => {
    const { m } = setup();
    const snap = snapshotPaths({ dirs: [m.contentDir] });
    const ino = statSync(join(m.contentDir, "article-00008.md")).ino;
    const { caseSensitive } = probeCaseSensitivity(m.contentDir);
    for (const steps of ["direct", "via-temp"]) {
      const r = await performWrites(
        caseRenameWrites(m.contentDir, "article-00008.md", "Article-00008.md", steps),
        () => 1n,
      );
      const ev = r.steps.at(-1).evidence;
      expect(ev.readdirBefore).not.toContain("Article-00008.md");
      expect(ev.readdirAfter).toContain("Article-00008.md");
      expect(ev.readdirAfter).not.toContain("article-00008.md");
      expect(ev.inodeAfterTo).toBe(ino);
      const got = verifySnapshot(snap).map((x) => x.got);
      if (caseSensitive) {
        // Two distinct names on a case-sensitive volume: the original is missing and the variant is extra.
        expect(got).toContain(null);
        expect(got).toContain("unexpected-file");
      } else {
        expect(got).toContain("name-case-mismatch");
      }
      const res = restoreSnapshot(snap, { renameRevert: true });
      expect(res.mismatches).toEqual([]);
      expect(res.actions).toHaveLength(1);
      expect(res.actions[0]).toMatchObject({ method: "rename", caseOnly: true });
      expect(dirEntries(m.contentDir)).toContain("article-00008.md");
      expect(statSync(join(m.contentDir, "article-00008.md")).ino).toBe(ino);
    }
  });

  it("a rename that leaves the entry name unchanged is rename-noop, never success", () => {
    const { m } = setup();
    const [w] = caseRenameWrites(m.contentDir, "article-00008.md", "Article-00008.md");
    const noRename = { ...REAL_FS, renameSync: () => {} };
    let err;
    try {
      performWrite(w, () => 1n, noRename);
    } catch (e) {
      err = e;
    }
    expect(err.code).toBe(RENAME_NOOP);
    expect(err.evidence.entryCheck).toMatchObject({
      missing: ["Article-00008.md"],
      stillThere: ["article-00008.md"],
    });
  });

  const noopFs = (attempt) => {
    const f = fnFsLike(attempt);
    const noop = () => {
      const e = new Error("rename-noop: entry unchanged");
      e.code = RENAME_NOOP;
      e.evidence = { readdirBefore: ["article-00008.md"], readdirAfter: ["article-00008.md"] };
      throw e;
    };
    f.performWrite = noop;
    f.performWrites = async () => noop();
    return f;
  };

  it("rename-noop in a workload scenario stops the batch as an error", async () => {
    const attempt = {
      token: "tok-c",
      targetPath: "/articles/Article-00008/",
      predicate: { kind: "count", selector: "h1", equals: 1 },
      supplemental: [],
      writes: [{ method: "rename", path: "a.md", to: "A.md", caseOnly: true }],
    };
    const d = deps({ page: fakePage({}), fs: noopFs(attempt) });
    const { accounting, stopped } = await runBatch({
      scenarios: [{ id: "c", set: "workload", ops: [{ id: "c", op: "rename-case-only" }] }],
      deps: d,
    });
    const rec = d.recorder.records.find((r) => r.type === "attempt");
    expect(rec.outcome).toBe("rename-noop");
    expect(rec.renameNoop.readdirAfter).toEqual(["article-00008.md"]);
    expect(accounting).toMatchObject({ stopReason: "rename-noop", error: 1, success: 0 });
    expect(batchExitOk(accounting, stopped)).toBe(false);
  });

  it("rename-noop in an expected-negative scenario is not an expected-negative pass", async () => {
    const attempt = {
      token: "tok-c",
      mode: "expected-negative",
      targetPath: "/",
      writes: [{ method: "rename", path: "a.md", to: "A.md", caseOnly: true }],
      expectState: ["error"],
      observe: [],
      errorMarkers: [],
      recovery: [],
      recoveryChecks: [],
    };
    const page = fakePage({});
    page.mark = () => ({});
    const d = deps({ page, fs: noopFs(attempt) });
    const { accounting } = await runBatch({
      scenarios: [
        { id: "c", set: "expected-negative", ops: [{ id: "c", op: "rename-case-only" }] },
      ],
      deps: d,
    });
    expect(d.recorder.records.find((r) => r.type === "attempt").outcome).toBe("rename-noop");
    expect(accounting).toMatchObject({ expectedNegativePasses: 0, error: 1 });
  });

  it("profile rules: the expectation is declared up front and the op ends its scenario", () => {
    const p = example("field-notes-original-memoized-q.example.json");
    const neg = p.scenarios.find((s) => s.id === "q-neg-rename-case-only-direct");
    delete neg.ops[0].expectState;
    p.scenarios.push({
      id: "case-workload",
      set: "workload",
      ops: [
        { op: "rename-case-only", article: "article-00030", writeMethod: "rename" },
        { op: "edit-body", article: "article-00031", writeMethod: "in-place" },
      ],
    });
    const e = semanticErrors(p).join("\n");
    expect(e).toMatch(/set "expected-negative" must declare expectState/);
    expect(e).toMatch(/must declare targetPath and predicate/);
    expect(e).toMatch(/must be the last op in its scenario/);
  });
});

function fnFsLike(attempt) {
  return {
    files: {},
    prepareAttempt: () => ({ ...attempt, write: attempt.writes[0] }),
    performWrite: () => 1n,
    performWrites: async (writes, now) => ({
      t0: now(),
      tLast: now(),
      steps: writes.map((w, i) => ({ i, method: w.method, path: w.path })),
    }),
    snapshot: () => ({}),
    restore: () => ({ restored: [], mismatches: [] }),
    liveChecks: () => [],
    warmPaths: () => [],
    hashOf: () => null,
  };
}

describe("same-tick evidence", () => {
  const tails = ["styles/global.css", "articles/article-00020.md"];
  const L = (text) => ({ text, msFromT0: 1 });
  it("true when one intake line names both paths before the rejection", () => {
    const r = sameTickEvidence(
      [
        L(
          "[zfb-timing] intake paths: /private/w/apps/zfb/styles/global.css:Modified, /private/w/content/articles/article-00020.md:Modified",
        ),
        L("[zfb] rebuild tick failed; watcher staying alive: bad yaml"),
        L("[zfb-timing] intake paths: /private/w/content/articles/article-00020.md:Modified"),
      ],
      tails,
    );
    expect(r.sameTick).toBe(true);
    expect(r.rejectionLine.text).toMatch(/rebuild tick failed/);
    expect(r.intakeLines).toHaveLength(1);
  });
  it("false when the paths reached the orchestrator in different intakes", () => {
    const r = sameTickEvidence(
      [
        L("[zfb-timing] intake paths: /w/apps/zfb/styles/global.css:Modified"),
        L("[zfb-timing] intake paths: /w/content/articles/article-00020.md:Modified"),
        L("x was not published"),
      ],
      tails,
    );
    expect(r.sameTick).toBe(false);
  });
  it("unknown without timing lines or with a path missing", () => {
    expect(sameTickEvidence([L("[zfb] rebuild tick failed; x")], tails).sameTick).toBe("unknown");
    expect(
      sameTickEvidence([L("[zfb-timing] intake paths: /w/styles/global.css:Modified")], tails)
        .sameTick,
    ).toBe("unknown");
  });
});

describe("recovery ops: plans", () => {
  it("css-in-rejected-tick: CSS first, then broken content; repair restores only the article bytes", () => {
    const { m } = setup();
    const original = readFileSync(join(m.contentDir, "article-00020.md"));
    const a = planFieldNotesOp(
      m,
      m.model,
      {
        op: "css-in-rejected-tick",
        article: "article-00020",
        writeMethod: "in-place",
        expectState: ["last-good"],
      },
      { token: "tok-css" },
    );
    expect(a.mode).toBe("recovery");
    expect(a.targetPath).toBe("/articles/article-00020/");
    expect(a.rejectWrites.map((w) => w.path)).toEqual([
      m.managedCssPath,
      join(m.contentDir, "article-00020.md"),
    ]);
    expect(a.rejectWrites[0].bytes.toString()).toContain("--fieldnotes-managed-marker: tok-css;");
    expect(a.repairWrites).toHaveLength(1);
    expect(a.repairWrites[0].bytes.equals(original)).toBe(true);
    expect(a.predicate).toMatchObject({ kind: "computed-style", includes: "tok-css" });
    expect(a.rejectedObserve.kind).toBe("open");
    expect(a.sameTick.tails).toEqual(["styles/global.css", "articles/article-00020.md"]);
    expect(a.beforeRepair).toBe("must-be-absent");
  });

  it("css-in-rejected-discovery-tick: creates an invalid new article and repairs that file", () => {
    const { m } = setup();
    for (const invalid of ["frontmatter", "slug-mismatch"]) {
      const a = planFieldNotesOp(
        m,
        m.model,
        {
          op: "css-in-rejected-discovery-tick",
          article: "article-00002",
          invalid,
          writeMethod: "in-place",
          expectState: ["error"],
        },
        { token: "tok-d" },
      );
      expect(a.rejectWrites[1]).toMatchObject({
        method: "create",
        path: join(m.contentDir, "article-00021.md"),
      });
      const bad = a.rejectWrites[1].bytes.toString();
      if (invalid === "slug-mismatch") expect(bad).toMatch(/^slug: "article-00021-mismatch"$/m);
      else expect(bad).toContain(": [unbalanced");
      expect(a.repairWrites[0]).toMatchObject({
        method: "in-place",
        path: join(m.contentDir, "article-00021.md"),
      });
      expect(a.repairWrites[0].bytes.toString()).toMatch(/^slug: "article-00021"$/m);
      expect(a.recoveryChecks[0].path).toBe("/articles/article-00021/");
      expect(a.nextModel.articles).toHaveLength(21);
    }
  });

  it("edit-during-rejected: breaks X, edits Y while rejected, repairs X; requires Y on its page and home", () => {
    const { m } = setup();
    const a = planFieldNotesOp(
      m,
      m.model,
      {
        op: "edit-during-rejected",
        article: "article-00013",
        other: "article-00006",
        writeMethod: "in-place",
        expectState: ["error", "last-good"],
      },
      { token: "tok-y" },
    );
    expect(a.targetPath).toBe("/articles/article-00006/");
    expect(a.rejectWrites[0].path).toBe(join(m.contentDir, "article-00013.md"));
    expect(a.duringWrites[0].bytes.toString()).toMatch(/^title: ".* tok-y"$/m);
    expect(a.repairWrites[0].path).toBe(join(m.contentDir, "article-00013.md"));
    expect(a.predicate).toEqual({ kind: "text", selector: FN_SEL.title, includes: "tok-y" });
    expect(a.recoveryChecks[0]).toEqual({
      path: "/",
      predicate: { kind: "text", selector: FN_SEL.cardLink("article-00006"), includes: "tok-y" },
    });
    expect(a.recoveryChecks.some((c) => c.path === "/articles/article-00013/")).toBe(true);
    expect(a.rejectedObserve.kind).toBe("paths");
    expect(a.sameTick).toBeUndefined();
  });

  it("CSS recovery ops need the managed stylesheet", () => {
    const root = tmp();
    const { datasetSha256: ds } = makeFakeSource(join(root, "src"), 20);
    const m = materializeFieldNotes({
      fixtureDir: FIXTURE,
      sourceDir: join(root, "src"),
      workDir: join(root, "work"),
      adapter: "direct-read",
      expectedDatasetSha256: ds,
    });
    expect(() =>
      planFieldNotesOp(m, m.model, {
        op: "css-in-rejected-tick",
        article: "article-00002",
        expectState: ["error"],
      }),
    ).toThrow(/managedCss/);
  });
});

/** A fake page with the mode helpers; `open` is what inspectOpen returns for the open page. */
function recoveryPage({ open, notify = true, precondition = false, checksOk = true } = {}) {
  const p = fakePage({ behaviors: notify ? [{ kind: "notify", t1: 9_000_000_000_000n }] : [] });
  const calls = [];
  Object.assign(p, {
    calls,
    mark: () => ({}),
    inspectOpen: async () => {
      calls.push("inspectOpen");
      return (
        open ?? {
          ok: true,
          open: true,
          docUUID: "doc-1",
          markersFound: [],
          title: "t",
          bodyExcerpt: "",
        }
      );
    },
    evaluateNow: async () => ({ matched: true }),
    observePage: async (path) => {
      calls.push(`observe ${path}`);
      return { ok: true, path, httpStatus: 500, bodyExcerpt: "Article slug mismatch" };
    },
    supplemental: async (check) => {
      calls.push(`check ${check.path}`);
      return { ok: checksOk };
    },
  });
  const arm = p.arm.bind(p);
  p.arm = async (token, predicate) => {
    calls.push("arm");
    if (precondition) return { armed: false, precondition: "true-before-write", docUUID: "doc-1" };
    return arm(token, predicate);
  };
  const prepare = p.prepare.bind(p);
  p.prepare = async (path) => {
    calls.push(`prepare ${path}`);
    return prepare(path);
  };
  return p;
}

function recoveryAttempt(overrides = {}) {
  return {
    token: "tok-r",
    mode: "recovery",
    variant: "css-in-rejected-tick",
    targetPath: "/articles/x/",
    expectState: ["last-good"],
    beforeRepair: "must-be-absent",
    requireSameTick: false,
    errorMarkers: ["Article slug mismatch"],
    rejectWrites: [
      { method: "in-place", path: "css", bytes: "c" },
      { method: "in-place", path: "x.md", bytes: "bad" },
    ],
    rejectedObserve: { kind: "open", lastGood: { kind: "count", selector: "h1", equals: 1 } },
    sameTick: { tails: ["styles/global.css", "articles/x.md"] },
    repairWrites: [{ method: "in-place", path: "x.md", bytes: "good" }],
    predicate: { kind: "computed-style", selector: ":root", property: "--m", includes: "tok-r" },
    recoveryChecks: [{ path: "/articles/x/", predicate: {} }],
    writes: [{ method: "in-place", path: "css", bytes: "c" }],
    ...overrides,
  };
}

function serverWithLog(lines) {
  const s = fakeServer();
  s.logMark = () => 0;
  s.logSince = () => ({ lines: lines.map((text) => ({ text, msFromT0: 1 })) });
  return s;
}

const REJECTED = "[zfb] rebuild tick failed; watcher staying alive: yaml";
const SAME = [
  "[zfb-timing] intake paths: /w/styles/global.css:Modified, /w/articles/x.md:Modified",
  "[zfb] rebuild tick failed; watcher staying alive: yaml",
];
const SPLIT = [
  "[zfb-timing] intake paths: /w/styles/global.css:Modified",
  "[zfb-timing] intake paths: /w/articles/x.md:Modified",
  "[zfb] rebuild tick failed; watcher staying alive: yaml",
];

async function runRecoveryBatch({ attempt, page, server }) {
  const d = deps({ page, fs: fnFsLike(attempt), server });
  const out = await runBatch({
    scenarios: [
      {
        id: "r",
        set: "recovery",
        ops: [
          { id: "r", op: attempt.variant },
          { id: "next", op: "edit-body" },
        ],
      },
    ],
    deps: d,
  });
  return { ...out, d, rec: d.recorder.records.find((r) => r.type === "attempt") };
}

describe("recovery mode", () => {
  it("rejected state on the open page, then the CSS appears on that page after the repair: recovery-observed", async () => {
    const page = recoveryPage();
    const { rec, accounting, d } = await runRecoveryBatch({
      attempt: recoveryAttempt(),
      page,
      server: serverWithLog(SAME),
    });
    expect(rec).toMatchObject({
      outcome: "recovery-observed",
      mode: "recovery",
      statePass: true,
      sameTick: { sameTick: true },
      observedSummary: [{ open: true, state: "last-good" }],
      endpoint: "predicate confirmed + 2 rAF + binding delivery",
    });
    expect(typeof rec.latencyMs).toBe("number");
    expect(rec.rejectSteps).toHaveLength(2);
    expect(rec.repairSteps).toHaveLength(1);
    // No controller navigation of the open page between the rejection and t1.
    expect(page.calls.slice(0, 3)).toEqual(["prepare /articles/x/", "inspectOpen", "arm"]);
    expect(accounting).toMatchObject({ recoveryPasses: 1, recoveryUnprovenTick: 0, error: 0 });
    expect(d.recorder.records.filter((r) => r.type === "attempt")).toHaveLength(2);
  });

  it("a proven different tick is not-same-tick: a stop, never a pass", async () => {
    const { rec, accounting, stopped } = await runRecoveryBatch({
      attempt: recoveryAttempt(),
      page: recoveryPage(),
      server: serverWithLog(SPLIT),
    });
    expect(rec.outcome).toBe("not-same-tick");
    expect(rec.writePerformed).toBe(true);
    expect(accounting).toMatchObject({
      recoveryPasses: 0,
      error: 1,
      stopReason: "not-same-tick",
      notAttempted: 1,
    });
    expect(batchExitOk(accounting, stopped)).toBe(false);
  });

  it("unknown same tick: required -> same-tick-unknown stop; not required -> counted apart from passes", async () => {
    const req = await runRecoveryBatch({
      attempt: recoveryAttempt({ requireSameTick: true }),
      page: recoveryPage(),
      server: serverWithLog([REJECTED]),
    });
    expect(req.rec.outcome).toBe("same-tick-unknown");
    expect(req.accounting.error).toBe(1);
    const free = await runRecoveryBatch({
      attempt: recoveryAttempt(),
      page: recoveryPage(),
      server: serverWithLog([REJECTED]),
    });
    expect(free.rec.outcome).toBe("recovery-observed");
    expect(free.rec.sameTickProven).toBeNull();
    expect(free.accounting).toMatchObject({ recoveryPasses: 0, recoveryUnprovenTick: 1 });
  });

  it("an undeclared rejected state is recovery-state-mismatch; no repair is attempted", async () => {
    const page = recoveryPage({
      open: {
        ok: true,
        open: true,
        docUUID: "doc-1",
        markersFound: ["Article slug mismatch"],
        bodyExcerpt: "Article slug mismatch",
      },
    });
    page.evaluateNow = async () => ({ matched: false });
    const { rec, accounting } = await runRecoveryBatch({
      attempt: recoveryAttempt(),
      page,
      server: serverWithLog(SAME),
    });
    expect(rec.outcome).toBe("recovery-state-mismatch");
    expect(rec.observedSummary[0].state).toBe("error");
    expect(rec.repairSteps).toBeUndefined();
    expect(accounting.error).toBe(1);
  });

  it("the timed predicate true before the repair is precondition-failed unless declared may-be-present", async () => {
    const strict = await runRecoveryBatch({
      attempt: recoveryAttempt(),
      page: recoveryPage({ precondition: true }),
      server: serverWithLog(SAME),
    });
    expect(strict.rec.outcome).toBe("precondition-failed");
    expect(strict.rec.predicateTrueBeforeRepair).toBe(true);
    const lax = await runRecoveryBatch({
      attempt: recoveryAttempt({ beforeRepair: "may-be-present" }),
      page: recoveryPage({ precondition: true }),
      server: serverWithLog(SAME),
    });
    expect(lax.rec.outcome).toBe("recovery-observed");
    expect(lax.rec.latencyMs).toBeNull();
  });

  it("a timeout after the repair stays a timeout and stops; failed checks are recovery-failed", async () => {
    const t = await runRecoveryBatch({
      attempt: recoveryAttempt(),
      page: recoveryPage({ notify: false }),
      server: serverWithLog(SAME),
    });
    expect(t.rec.outcome).toBe("timeout");
    expect(t.accounting).toMatchObject({ timeout: 1, recoveryPasses: 0, stopReason: "timeout" });
    const f = await runRecoveryBatch({
      attempt: recoveryAttempt(),
      page: recoveryPage({ checksOk: false }),
      server: serverWithLog(SAME),
    });
    expect(f.rec.outcome).toBe("recovery-failed");
    expect(f.accounting.error).toBe(1);
  });

  it("edit-during-rejected observes declared paths, returns to the open page, edits, then arms", async () => {
    const page = recoveryPage();
    const { rec } = await runRecoveryBatch({
      attempt: recoveryAttempt({
        variant: "edit-during-rejected",
        targetPath: "/articles/y/",
        expectState: ["error"],
        sameTick: undefined,
        rejectedObserve: {
          kind: "paths",
          observe: [{ path: "/articles/x/" }, { path: "/" }],
        },
        duringWrites: [{ method: "in-place", path: "y.md", bytes: "title" }],
        predicate: { kind: "text", selector: "h1", includes: "tok-r" },
      }),
      page,
      server: serverWithLog([REJECTED]),
    });
    expect(rec.outcome).toBe("recovery-observed");
    expect(rec.sameTickProven).toBeNull();
    expect(rec.duringSteps).toHaveLength(1);
    expect(page.calls.slice(0, 6)).toEqual([
      "prepare /articles/y/",
      "observe /articles/x/",
      "observe /",
      "prepare /articles/y/",
      "inspectOpen",
      "arm",
    ]);
  });

  it("profile rules for recovery ops", () => {
    const p = example("field-notes-direct-read-q.example.json");
    const s = p.scenarios.find((x) => x.id === "q-recovery-css-in-rejected-tick");
    delete s.ops[0].expectState;
    s.ops[0].requireSameTick = true;
    p.scenarios.push({
      id: "misplaced",
      set: "workload",
      ops: [
        {
          op: "edit-during-rejected",
          article: "article-00040",
          writeMethod: "in-place",
          expectState: ["error"],
        },
      ],
    });
    p.fixture.managedCss = false;
    const e = semanticErrors(p).join("\n");
    expect(e).toMatch(/must declare expectState for the rejected generation/);
    expect(e).toMatch(/requireSameTick needs a diagnostic profile/);
    expect(e).toMatch(/is a recovery op and must be in a scenario with set "recovery"/);
    expect(e).toMatch(/edit-during-rejected needs other/);
    expect(e).toMatch(/needs fixture.managedCss: true/);
  });
});

describe("example profiles", () => {
  it("every q and tiny body/category example validates", () => {
    const names = readdirSync(join(HARNESS_DIR, "profiles")).filter(
      (n) => n.endsWith("-q.example.json") || n.includes("-body-category."),
    );
    expect(names.length).toBe(17);
    for (const n of names) {
      expect(validateAs("profile", example(n)), n).toEqual([]);
      expect(semanticErrors(example(n)), n).toEqual([]);
    }
  });

  it("tiny body/category: A0-A3 x internal/external x warm orders; A1 is the original helper unchanged", () => {
    const p = example("tiny-external-a1-body-category.example.json");
    expect(p.fixture).toMatchObject({ adapter: "A1", placement: "external", configFormat: "ts" });
    expect(new Set(p.scenarios.map((s) => s.warmOrder))).toEqual(
      new Set(["index-first", "detail-first", "none"]),
    );
    const ops = p.scenarios.map((s) => `${s.ops[0].op}@${s.ops[0].targetPath ?? "detail"}`);
    expect(new Set(ops)).toEqual(
      new Set(["edit-body@detail", "edit-category@detail", "edit-category@/"]),
    );
  });
});

describe("review round: recovery premises are verified, never assumed", () => {
  it("no rejection line after the broken write is rejection-not-observed: a stop, no repair", async () => {
    for (const server of [
      serverWithLog(["[zfb-timing] intake paths: /w/styles/global.css:Modified"]),
      fakeServer(),
    ]) {
      const { rec, accounting, stopped } = await runRecoveryBatch({
        attempt: recoveryAttempt(),
        page: recoveryPage(),
        server,
      });
      expect(rec.outcome).toBe("rejection-not-observed");
      expect(rec.repairSteps).toBeUndefined();
      expect(rec.sameTickProven).toBeNull();
      expect(accounting).toMatchObject({ error: 1, recoveryPasses: 0 });
      expect(batchExitOk(accounting, stopped)).toBe(false);
    }
  });

  it("a rejection line printed before the broken write does not count", async () => {
    const s = fakeServer();
    s.logMark = () => 0;
    s.logSince = () => ({ lines: [{ text: REJECTED, msFromT0: -5 }] });
    const { rec } = await runRecoveryBatch({
      attempt: recoveryAttempt(),
      page: recoveryPage(),
      server: s,
    });
    expect(rec.outcome).toBe("rejection-not-observed");
  });

  it("the open page must still be the prepared document after the rejected settle", async () => {
    const cases = [
      { open: { ok: true, docUUID: "doc-2", markersFound: [] } },
      { settle: { settled: true, newDocuments: 1, mainFrameNavigations: 1 } },
      { settle: { settled: false, reason: "bound-exceeded" } },
    ];
    for (const c of cases) {
      const page = recoveryPage(c.open ? { open: c.open } : {});
      if (c.settle) page.settle = async () => c.settle;
      const { rec, accounting } = await runRecoveryBatch({
        attempt: recoveryAttempt(),
        page,
        server: serverWithLog(SAME),
      });
      expect(rec.outcome).toBe("open-page-replaced");
      expect(rec.openPage.reasons.length).toBeGreaterThan(0);
      expect(rec.repairSteps).toBeUndefined();
      // This fake supplies the same settle result for rejection and final cleanup.
      const failedCleanup = c.settle?.settled === false;
      expect(accounting.restorationFailures).toBe(failedCleanup ? 1 : 0);
      expect(accounting.error).toBe(failedCleanup ? 2 : 1);
    }
  });

  it("edit-during-rejected: a reload of the open page after the during-write is open-page-replaced", async () => {
    const page = recoveryPage();
    let n = 0;
    page.settle = async () => (n++ === 0 ? { settled: true } : { settled: true, newDocuments: 1 });
    const { rec } = await runRecoveryBatch({
      attempt: recoveryAttempt({
        variant: "edit-during-rejected",
        sameTick: undefined,
        expectState: ["error"],
        rejectedObserve: { kind: "paths", observe: [{ path: "/articles/x/" }] },
        duringWrites: [{ method: "in-place", path: "y.md", bytes: "t" }],
      }),
      page,
      server: serverWithLog([REJECTED]),
    });
    expect(rec.outcome).toBe("open-page-replaced");
    expect(rec.duringSteps).toHaveLength(1);
  });

  it("the open page's state never rests on its original navigation status", () => {
    const view = { ok: true, originalNavigationStatus: 500, markersFound: [] };
    expect(classifyOpen(view, { matched: true }).state).toBe("last-good");
    expect(classifyOpen(view, { matched: false }).state).toBe("other");
    expect(classifyOpen({ ok: true, markersFound: ["X"] }, null, ["X"]).state).toBe("error");
  });

  it("sameTickProven is on every recovery record and in the summary; proven passes are counted apart", async () => {
    const proven = await runRecoveryBatch({
      attempt: recoveryAttempt(),
      page: recoveryPage(),
      server: serverWithLog(SAME),
    });
    expect(proven.rec.sameTickProven).toBe(true);
    expect(proven.accounting).toMatchObject({
      recoveryPasses: 1,
      recoverySameTickProvenPasses: 1,
      recoveryNoTickPremisePasses: 0,
    });
    const split = await runRecoveryBatch({
      attempt: recoveryAttempt(),
      page: recoveryPage(),
      server: serverWithLog(SPLIT),
    });
    expect(split.rec.sameTickProven).toBe(false);
    const summary = summarize(
      [...proven.d.recorder.records, ...split.d.recorder.records],
      proven.accounting,
    );
    const first = summary.recovery.filter((r) => r.opId === "r");
    expect(first.map((r) => [r.outcome, r.sameTickProven])).toEqual([
      ["recovery-observed", true],
      ["not-same-tick", false],
    ]);
  });
});

describe("review round: same-tick window", () => {
  const tails = ["styles/global.css", "articles/x.md"];
  const L = (text, msFromT0 = 1) => ({ text, msFromT0 });
  const BOTH =
    "[zfb-timing] intake paths: /w/styles/global.css:Modified, /w/articles/x.md:Modified";
  it("drops lines from before the first write", () => {
    expect(sameTickEvidence([L(BOTH, -3), L(REJECTED)], tails).sameTick).toBe("unknown");
  });
  it("a truncated log is unknown", () => {
    expect(sameTickEvidence([L(BOTH), L(REJECTED)], tails, { truncated: true }).sameTick).toBe(
      "unknown",
    );
  });
  it("only the last intake before the rejection decides", () => {
    const r = sameTickEvidence(
      [L(BOTH), L("[zfb-timing] intake paths: /w/articles/x.md:Modified"), L(REJECTED)],
      tails,
    );
    expect(r.sameTick).toBe(false);
    expect(r.decidingLine.text).toMatch(/articles\/x\.md:Modified$/);
  });
});

describe("review round: restoration never unlinks a case alias of a snapshot file", () => {
  it("a failed case revert leaves the variant in place and reports a mismatch", () => {
    const { m } = setup();
    const snap = snapshotPaths({ dirs: [m.contentDir] });
    const from = join(m.contentDir, "article-00008.md");
    const to = join(m.contentDir, "Article-00008.md");
    performWrite(
      caseRenameWrites(m.contentDir, "article-00008.md", "Article-00008.md")[0],
      () => 1n,
    );
    const ino = statSync(to).ino;
    const brokenRename = {
      ...REAL_FS,
      renameSync: () => {
        throw new Error("EIO: simulated rename failure");
      },
    };
    const res = restoreSnapshot(snap, { renameRevert: true, fsImpl: brokenRename });
    expect(res.mismatches.length).toBeGreaterThan(0);
    expect(dirEntries(m.contentDir)).toContain("Article-00008.md");
    expect(statSync(to).ino).toBe(ino);
    expect(res.actions.find((a) => a.method === "unlink" && a.path === to)).toMatchObject({
      skipped: true,
    });
    expect(existsSync(from)).toBe(true);
  });

  it("guards by lower-cased name and by dev+inode", () => {
    const d = tmp();
    writeFileSync(join(d, "a.md"), "x");
    linkSync(join(d, "a.md"), join(d, "hard.md"));
    writeFileSync(join(d, "other.md"), "y");
    const snap = {
      dirs: [d],
      files: { [join(d, "a.md")]: { sha256: "", bytes: Buffer.from("x") } },
    };
    const guard = aliasGuard(snap);
    expect(guard(join(d, "A.MD"))).toMatchObject({ reason: "case variant of snapshot file" });
    expect(guard(join(d, "hard.md"))).toMatchObject({ reason: "same dev+inode as snapshot file" });
    expect(guard(join(d, "other.md"))).toBeNull();
  });
});

describe("review round: dataset pin at materialize", () => {
  it("refuses a source whose dataset is not the pinned one, for both placements", () => {
    for (const placement of ["external", "internal"]) {
      const root = tmp();
      const src = join(root, "src");
      makeFakeSource(src, 20);
      writeFileSync(join(src, "apps", "zfb", "zfb.config.ts"), baseConfigBytes());
      expect(() =>
        materializeFieldNotes({
          fixtureDir: FIXTURE,
          sourceDir: src,
          workDir: join(root, "work"),
          adapter: "direct-read",
          placement,
        }),
      ).toThrow(/is not the pinned dataset/);
      expect(existsSync(join(root, "work"))).toBe(false);
    }
  });
});

describe("review round: requireSameTick needs ZFB_DEV_TIMING=1", () => {
  it("a diagnostic profile without the env var is rejected", () => {
    const p = example("field-notes-direct-read-recovery-diagnostic-q.example.json");
    expect(semanticErrors(p)).toEqual([]);
    delete p.server.env;
    expect(semanticErrors(p).join("\n")).toMatch(/requireSameTick needs server.env.ZFB_DEV_TIMING/);
  });
});
