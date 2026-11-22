import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
  FN_SEL,
  datasetSha256,
  fieldNotesOps,
  liveRestoreChecks,
  loadFieldNotesFixture,
  materializeFieldNotes,
  planFieldNotesOp,
  sourceTreeHashes,
} from "../lib/fieldnotes.mjs";
import {
  breakFrontmatter,
  computeIndex,
  createModel,
  generatedMetadata,
  getFrontmatter,
  placement,
  referrersOf,
  relatedHrefs,
  removeArticle,
  setCategory,
  setFrontmatter,
  tokenInFirstParagraph,
} from "../lib/fieldnotes-model.mjs";
import { performWrites, restoreSnapshot } from "../lib/fixture.mjs";
import { semanticErrors } from "../lib/profile.mjs";
import { validateAs } from "../lib/schema.mjs";
import { HARNESS_DIR, sha256File } from "../lib/util.mjs";
import { SPEC, articleRow, makeFakeSource } from "./fieldnotes-fake.mjs";

const FIXTURE = join(HARNESS_DIR, "fixtures", "field-notes");
const tmps = [];
const tmp = () => {
  const d = mkdtempSync(join(tmpdir(), "dbo-fn-"));
  tmps.push(d);
  return d;
};
afterEach(() => {
  while (tmps.length) rmSync(tmps.pop(), { recursive: true, force: true });
});

function setup({ count = 20, adapter = "direct-read", managedCss = true } = {}) {
  const root = tmp();
  const src = join(root, "src");
  const fake = makeFakeSource(src, count);
  const m = materializeFieldNotes({
    fixtureDir: FIXTURE,
    sourceDir: src,
    workDir: join(root, "work"),
    adapter,
    managedCss,
    expectedDatasetSha256: fake.datasetSha256,
  });
  return { root, src, m, fake };
}

const model20 = () =>
  createModel({ articles: Array.from({ length: 20 }, (_, i) => articleRow(i + 1)) }, SPEC);

describe("field-notes model mirrors the app's index rules", () => {
  it("sorts by slug, groups by category in spec order, related = next 3 in category (cyclic)", () => {
    const idx = computeIndex(model20());
    expect(idx.articles[0].slug).toBe("article-00001");
    expect(idx.categories.get("architecture").map((a) => a.slug)).toEqual([
      "article-00001",
      "article-00011",
    ]);
    expect(relatedHrefs(model20(), "article-00001")).toEqual(["/articles/article-00011/"]);
  });

  it("placement and referrers", () => {
    const big = createModel(
      { articles: Array.from({ length: 100 }, (_, i) => articleRow(i + 1)) },
      SPEC,
    );
    expect(placement(big, "article-00012")).toMatchObject({
      onHome: true,
      archivePage: 1,
      category: "content",
      onCategoryPage: true,
    });
    expect(placement(big, "article-00051")).toMatchObject({ onHome: false, archivePage: 2 });
    expect(referrersOf(big, "article-00005").sort()).toEqual([
      "article-00075",
      "article-00085",
      "article-00095",
    ]);
    expect(relatedHrefs(big, "article-00095")).toEqual([
      "/articles/article-00005/",
      "/articles/article-00015/",
      "/articles/article-00025/",
    ]);
  });

  it("mutations are pure and recompute related lists", () => {
    const m = model20();
    const moved = setCategory(m, "article-00001", "content");
    expect(computeIndex(m).categories.get("architecture")).toHaveLength(2);
    expect(
      computeIndex(moved)
        .categories.get("content")
        .map((a) => a.slug),
    ).toEqual(["article-00001", "article-00002", "article-00012"]);
    expect(computeIndex(removeArticle(m, "article-00001")).articles).toHaveLength(19);
  });

  it("generatedMetadata follows scripts/generate.mjs (category, author, date)", () => {
    const g = generatedMetadata(model20(), 101);
    expect(g).toMatchObject({
      slug: "article-00101",
      category: "architecture",
      categoryLabel: "Architecture",
      author: "Mika Tanaka",
      date: "2026-04-11",
    });
  });

  it("frontmatter and body transforms touch exactly one field", () => {
    const t = readFileSync(join(setup().m.contentDir, "article-00001.md"), "utf8");
    expect(getFrontmatter(setFrontmatter(t, "title", "New"), "title")).toBe("New");
    expect(getFrontmatter(setFrontmatter(t, "readingMinutes", 9), "readingMinutes")).toBe(9);
    expect(tokenInFirstParagraph(t, "tok-1")).toContain(
      "tok-1 Body of article-00001 first paragraph.",
    );
    expect(tokenInFirstParagraph(t, "tok-1")).toContain("## Start with a clear question");
    expect(breakFrontmatter(t)).toContain("[unbalanced");
    expect(() => setFrontmatter(t, "nope", "x")).toThrow(/no nope field/);
  });
});

describe("source identity and materialization", () => {
  it("source tree hashes exclude lib/content.ts and the dataset hash matches the generator rule", () => {
    const root = tmp();
    const fake = makeFakeSource(join(root, "s"));
    const manifest = loadFieldNotesFixture(FIXTURE);
    const a = sourceTreeHashes(join(root, "s"), manifest);
    expect(a.datasetSha256).toBe(fake.datasetSha256);
    expect(datasetSha256(join(root, "s", "content"))).toBe(fake.datasetSha256);
    writeFileSync(join(root, "s", "apps", "zfb", "lib", "content.ts"), "// different helper\n");
    expect(sourceTreeHashes(join(root, "s"), manifest).appsZfbTreeSha256).toBe(a.appsZfbTreeSha256);
    writeFileSync(join(root, "s", "apps", "zfb", "components", "Layout.tsx"), "changed\n");
    expect(sourceTreeHashes(join(root, "s"), manifest).appsZfbTreeSha256).not.toBe(
      a.appsZfbTreeSha256,
    );
  });

  it("materializes apps/zfb + content + shared, installs the adapter bytes and the declared managed CSS", () => {
    const { m } = setup({ adapter: "original-memoized" });
    expect(sha256File(join(m.appDir, "lib", "content.ts"))).toBe(
      "c104d8f06e11297d646cc59a360b29622a9a03ec8aaca1bc39db3d112cfbac40",
    );
    expect(readFileSync(m.managedCssPath, "utf8")).toContain("--fieldnotes-managed-marker: v0;");
    expect(existsSync(join(m.workDir, "shared", "site.css"))).toBe(true);
    expect(m.model.articles).toHaveLength(20);
    expect(setup({ managedCss: false }).m.managedCssPath).toBeNull();
  });

  it("the fixture manifest validates and pins every adapter", () => {
    const manifest = loadFieldNotesFixture(FIXTURE);
    expect(validateAs("field-notes-fixture", manifest)).toEqual([]);
    for (const [id, v] of Object.entries(manifest.adapters.variants))
      expect(sha256File(join(FIXTURE, v.file)), id).toBe(v.sha256);
    expect(manifest.adapters.variants["original-memoized"].facts).toHaveLength(2);
    expect(
      readFileSync(join(FIXTURE, manifest.adapters.variants["direct-read"].file), "utf8"),
    ).toContain("RECONSTRUCTED control");
  });
});

describe("op planners and predicate builders", () => {
  it("edit-body arms on the detail body with a token in the first paragraph", () => {
    const { m } = setup();
    const a = planFieldNotesOp(
      m,
      m.model,
      { op: "edit-body", article: "article-00005" },
      { token: "tok-b" },
    );
    expect(a).toMatchObject({
      mode: "standard",
      targetPath: "/articles/article-00005/",
      predicate: { kind: "text", selector: FN_SEL.body("article-00005"), includes: "tok-b" },
    });
    expect(a.writes[0].bytes.toString()).toContain("tok-b Body of article-00005");
  });

  it("edit-title on the detail page: supplemental home card, category, archive and referrers' related cards", () => {
    const { m } = setup();
    const a = planFieldNotesOp(
      m,
      m.model,
      { op: "edit-title", article: "article-00005" },
      { token: "tok-t" },
    );
    expect(a.predicate).toEqual({ kind: "text", selector: FN_SEL.title, includes: "tok-t" });
    expect(a.supplemental.map((c) => c.path)).toEqual([
      "/",
      "/category/performance/",
      "/archive/1/",
      "/articles/article-00015/",
    ]);
    expect(a.supplemental[3].predicate.selector).toBe(FN_SEL.relatedLinkTo("article-00005"));
  });

  it("edit-title targeted at home arms on the home card; a non-home article cannot target home", () => {
    const { m } = setup();
    const a = planFieldNotesOp(
      m,
      m.model,
      { op: "edit-title", article: "article-00006", targetPath: "/" },
      { token: "x" },
    );
    expect(a.predicate.selector).toBe(FN_SEL.cardLink("article-00006"));
    expect(a.supplemental[0].path).toBe("/articles/article-00006/");
    expect(() =>
      planFieldNotesOp(m, m.model, { op: "edit-title", article: "article-00019", targetPath: "/" }),
    ).toThrow(/first 12/);
  });

  it("edit-category writes category + categoryLabel, observes the eyebrow and recomputes related lists", () => {
    const { m } = setup();
    const a = planFieldNotesOp(m, m.model, {
      op: "edit-category",
      article: "article-00001",
      toCategory: "content",
    });
    const t = a.writes[0].bytes.toString();
    expect(getFrontmatter(t, "category")).toBe("content");
    expect(getFrontmatter(t, "categoryLabel")).toBe("Content");
    expect(a.predicate).toEqual({ kind: "text", selector: FN_SEL.eyebrow, equals: "Content" });
    const paths = a.supplemental.map((c) => c.path);
    expect(paths).toContain("/category/architecture/");
    expect(paths).toContain("/category/content/");
    const rel = a.supplemental.find((c) => c.path === "/articles/article-00001/");
    expect(rel.predicate).toEqual({
      kind: "attribute-list",
      selector: FN_SEL.relatedLinks,
      name: "href",
      equals: ["/articles/article-00002/", "/articles/article-00012/"],
    });
  });

  it("add-article creates the next article with consistent frontmatter and checks the hero count, URL, archive and category", () => {
    const { m } = setup();
    const a = planFieldNotesOp(m, m.model, { op: "add-article" }, { token: "tok-add" });
    expect(a.writes[0]).toMatchObject({ method: "create" });
    expect(a.writes[0].path.endsWith("article-00021.md")).toBe(true);
    const t = a.writes[0].bytes.toString();
    expect(getFrontmatter(t, "slug")).toBe("article-00021");
    expect(getFrontmatter(t, "category")).toBe("architecture");
    expect(a.predicate).toEqual({ kind: "text", selector: FN_SEL.heroCount, equals: "21" });
    expect(a.supplemental.map((c) => c.path)).toEqual([
      "/articles/article-00021/",
      "/archive/1/",
      "/category/architecture/",
    ]);
  });

  it("delete-article requires the old URL to be gone (status AND body) and lists to drop it", () => {
    const { m } = setup();
    const a = planFieldNotesOp(m, m.model, { op: "delete-article", article: "article-00011" });
    expect(a.writes).toEqual([{ method: "unlink", path: join(m.contentDir, "article-00011.md") }]);
    expect(a.predicate).toEqual({ kind: "text", selector: FN_SEL.heroCount, equals: "19" });
    expect(a.supplemental[0]).toEqual({
      kind: "gone",
      path: "/articles/article-00011/",
      absentText: "Title 11 · 00011",
    });
  });

  it("rename-slug-change renames the file then updates the slug; rename-filename-only is expected-negative with a recovery", () => {
    const { m } = setup();
    const a = planFieldNotesOp(m, m.model, { op: "rename-slug-change", article: "article-00007" });
    expect(a.writes.map((w) => w.method)).toEqual(["rename", "in-place"]);
    expect(getFrontmatter(a.writes[1].bytes.toString(), "slug")).toBe("article-00007-renamed");
    expect(a.predicate).toEqual({
      kind: "count",
      selector: FN_SEL.cardLink("article-00007-renamed"),
      equals: 1,
    });
    expect(a.supplemental[0]).toMatchObject({ kind: "gone", path: "/articles/article-00007/" });
    const n = planFieldNotesOp(m, m.model, {
      op: "rename-filename-only",
      article: "article-00008",
    });
    expect(n).toMatchObject({
      mode: "expected-negative",
      expectState: ["error", "last-good"],
      errorMarkers: ["Article slug mismatch"],
    });
    expect(n.recovery).toEqual([
      {
        method: "rename",
        path: join(m.contentDir, "article-00008-renamed.md"),
        to: join(m.contentDir, "article-00008.md"),
      },
    ]);
    expect(n.observe.map((o) => o.path)).toEqual(["/", "/articles/article-00008/"]);
  });

  it("hidden-edit, burst and rapid-sequence shapes", () => {
    const { m } = setup();
    const h = planFieldNotesOp(
      m,
      m.model,
      { op: "hidden-edit", article: "article-00017" },
      { token: "tok-h" },
    );
    expect(h).toMatchObject({
      mode: "hidden",
      targetPath: "/",
      hiddenPath: "/articles/article-00017/",
      observeMs: 3000,
      accessDelayMs: 1000,
    });
    expect(() =>
      planFieldNotesOp(m, m.model, {
        op: "hidden-edit",
        article: "article-00017",
        targetPath: "/articles/article-00017/",
      }),
    ).toThrow(/other than/);
    const b = planFieldNotesOp(
      m,
      m.model,
      { op: "burst", article: "article-00003" },
      { tokens: ["t1", "t2", "t3", "t4", "t5"] },
    );
    expect(b.writes.map((w) => w.afterMs)).toEqual([0, 50, 50, 50, 50]);
    expect(b.predicate.includes).toBe("t5");
    expect(b.watch).toEqual([{ selector: FN_SEL.title, tokens: ["t1", "t2", "t3", "t4"] }]);
    const r = planFieldNotesOp(
      m,
      m.model,
      { op: "rapid-sequence", article: "article-00004" },
      { token: "tok-r" },
    );
    expect(r.predicate.kind).toBe("all");
    expect(r.writes.map((w) => w.afterMs ?? 0)).toEqual([0, 100]);
    const ad = planFieldNotesOp(
      m,
      m.model,
      { op: "rapid-sequence", variant: "add-then-delete", writeMethod: "create" },
      { token: "tok-ad" },
    );
    expect(ad).toMatchObject({
      mode: "final-state",
      declared: { finalState: "unchanged", articleCount: 20 },
    });
    expect(ad.writes.map((w) => w.method)).toEqual(["create", "unlink"]);
    expect(ad.finalChecks[1]).toMatchObject({ kind: "gone", path: "/articles/article-00021/" });
  });

  it("public/managed CSS edits use computed-style custom properties", () => {
    const { m } = setup();
    const p = planFieldNotesOp(
      m,
      m.model,
      { op: "edit-public-css", expect: "classify" },
      { token: "tok-p" },
    );
    expect(p.writes[0].bytes.toString()).toContain(":root{--fn-public-marker:tok-p;");
    const g = planFieldNotesOp(m, m.model, { op: "edit-managed-css" }, { token: "tok-g" });
    expect(g.predicate).toEqual({
      kind: "computed-style",
      selector: ":root",
      property: "--fieldnotes-managed-marker",
      includes: "tok-g",
    });
    expect(() =>
      planFieldNotesOp(setup({ managedCss: false }).m, m.model, { op: "edit-managed-css" }),
    ).toThrow(/managedCss/);
  });
});

describe("field-notes ops: model tracking and restoration", () => {
  it("applied writes advance the model; restore reverts renames by rename and resets the model", async () => {
    const { m } = setup();
    const ops = fieldNotesOps(m);
    const snap = ops.snapshot();
    const a = ops.prepareAttempt({ op: "rename-slug-change", article: "article-00007" });
    await performWrites(
      a.writes,
      () => 0n,
      undefined,
      async () => {},
    );
    a.applied();
    expect(ops.state.model.articles.some((x) => x.slug === "article-00007-renamed")).toBe(true);
    const r = ops.restore(snap);
    expect(r.actions.map((x) => x.method)).toEqual(["rename", "in-place"]);
    expect(r.mismatches).toEqual([]);
    expect(ops.state.model).toBe(m.model);
    const live = ops.liveChecks();
    expect(live.find((c) => c.path === "/articles/article-00007-renamed/")).toMatchObject({
      kind: "gone",
    });
  });

  it("liveRestoreChecks cover home, details, every archive page, a category page and counts", () => {
    const checks = liveRestoreChecks(model20(), ["article-00005"]);
    expect(checks.map((c) => c.path)).toEqual([
      "/",
      "/articles/article-00001/",
      "/articles/article-00005/",
      "/archive/1/",
      "/category/architecture/",
    ]);
    expect(checks[0].predicate.of[0]).toEqual({
      kind: "text",
      selector: FN_SEL.heroCount,
      equals: "20",
    });
  });

  it("restoreSnapshot without renameRevert keeps the earlier create/unlink behaviour", async () => {
    const { m } = setup();
    const ops = fieldNotesOps(m);
    const snap = ops.snapshot();
    const a = ops.prepareAttempt({ op: "rename-slug-change", article: "article-00007" });
    await performWrites(
      a.writes,
      () => 0n,
      undefined,
      async () => {},
    );
    expect(
      restoreSnapshot(snap)
        .actions.map((x) => x.method)
        .sort(),
    ).toEqual(["create", "unlink"]);
  });
});

describe("performWrites", () => {
  it("records requested spacing and every step's time", async () => {
    const { m } = setup();
    const a = planFieldNotesOp(m, m.model, {
      op: "burst",
      article: "article-00003",
      count: 3,
      spacingMs: 7,
    });
    const sleeps = [];
    let t = 0n;
    const r = await performWrites(
      a.writes,
      () => (t += 1000n),
      undefined,
      async (ms) => sleeps.push(ms),
    );
    expect(sleeps).toEqual([7, 7]);
    expect(r.steps.map((s) => s.requestedAfterMs)).toEqual([0, 7, 7]);
    expect(r.t0).toBe(1000n);
    expect(r.tLast).toBe(3000n);
  });

  it("an empty step list is a clear no-write-steps error, not a TypeError", async () => {
    await expect(performWrites([], () => 1n)).rejects.toMatchObject({
      message: "no write steps",
      code: "no-write-steps",
      steps: [],
    });
    await expect(performWrites([], () => 1n)).rejects.not.toBeInstanceOf(TypeError);
  });

  it("a failing step reports the completed ones", async () => {
    const { m } = setup();
    const writes = [
      { method: "in-place", path: join(m.contentDir, "article-00001.md"), bytes: Buffer.from("x") },
      { method: "in-place", path: join(m.contentDir, "missing.md"), bytes: Buffer.from("y") },
    ];
    await expect(
      performWrites(
        writes,
        () => 1n,
        undefined,
        async () => {},
      ),
    ).rejects.toMatchObject({ failedStep: 1, steps: [{ i: 0 }] });
  });
});

describe("profile rules for field-notes", () => {
  const example = (n) => JSON.parse(readFileSync(join(HARNESS_DIR, "profiles", n), "utf8"));
  it("example matrices validate", () => {
    for (const n of [
      "field-notes-direct-read.example.json",
      "field-notes-original-memoized.example.json",
      "field-notes-recovered-snapshot.example.json",
    ]) {
      expect(validateAs("profile", example(n)), n).toEqual([]);
      expect(semanticErrors(example(n)), n).toEqual([]);
    }
  });

  it("rejects misplaced expected-negative ops, unclassified public CSS, visited hidden-edit, managed CSS without the addition and tiny ops", () => {
    const p = example("field-notes-direct-read.example.json");
    const one = (op, set = "workload", extra = {}) => ({
      ...p,
      ...extra,
      scenarios: [{ id: "x", set, ops: [op] }],
    });
    expect(
      semanticErrors(
        one({ op: "rename-filename-only", article: "article-00008", writeMethod: "rename" }),
      )[0],
    ).toMatch(/expected-invalid and must be in/);
    expect(
      semanticErrors(
        one(
          { op: "edit-body", article: "article-00005", writeMethod: "in-place" },
          "expected-negative",
        ),
      )[0],
    ).toMatch(/must not be in/);
    expect(semanticErrors(one({ op: "edit-public-css", writeMethod: "in-place" }))[0]).toMatch(
      /must declare expect: "classify"/,
    );
    expect(
      semanticErrors({
        ...p,
        scenarios: [
          {
            id: "x",
            warmOrder: "index-first",
            warmDetail: "article-00077",
            ops: [{ op: "hidden-edit", article: "article-00077", writeMethod: "in-place" }],
          },
        ],
      })[0],
    ).toMatch(/must not be visited/);
    expect(
      semanticErrors(
        one({ op: "edit-managed-css", writeMethod: "in-place" }, "workload", {
          fixture: { ...p.fixture, managedCss: false },
        }),
      )[0],
    ).toMatch(/managedCss: true/);
    expect(
      semanticErrors(one({ op: "create-article", article: "c", writeMethod: "create" }))[0],
    ).toMatch(/not a field-notes op/);
    const tiny = example("tiny-internal-a0.example.json");
    expect(
      semanticErrors({
        ...tiny,
        scenarios: [{ id: "x", ops: [{ op: "burst", article: "a", writeMethod: "in-place" }] }],
      })[0],
    ).toMatch(/field-notes op, not a tiny op/);
  });
});
