import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "vitest";
import { CATEGORY_DETAIL, astroPredicateValue } from "../lib/astro-category.mjs";
import { fieldNotesOps } from "../lib/fieldnotes.mjs";
import { createModel, href } from "../lib/fieldnotes-model.mjs";
import { performWrite, hashOf } from "../lib/fixture.mjs";
import { runBatch } from "../lib/state-machine.mjs";
import { profileSchemaFor, semanticErrors } from "../lib/profile.mjs";
import { validateAs } from "../lib/schema.mjs";
import { SPEC, articleRow, articleText } from "./fieldnotes-fake.mjs";
import { deps, fakePage } from "./fakes.mjs";
const op = {
  id: "category-15",
  op: "edit-category",
  article: "article-00015",
  writeMethod: "in-place",
  expect: "classify",
};
const scenario = { id: "fn-category", set: "workload", warmOrder: "none", ops: [op] };
const article = (i) => href("article-" + String(i).padStart(5, "0"));
const paths = [
  "/category/performance/",
  "/category/testing/",
  "/category/testing/",
  ...[15, 5, 85, 95, 6, 86, 96].map(article),
];
const oldLinks = [6, 16, 26].map(article),
  newLinks = [6, 15, 16].map(article),
  title = "Finding useful signals in build output · 00015";
test.each([
  "success",
  "wrong-article",
  "wrong-label",
  "rename-token",
  "reorder-visit",
  "missing-visit",
  "stale-same-route",
  "empty-old-page",
  "old-page-404",
  "wrong-neighbors",
  "wrong-meta",
  "wrong-meta-href",
  "reordered-related",
  "stale-restore",
  "wrong-inode",
  "fresh-wrong-detail",
])("fixed Astro category normal path: %s", async (variant) => {
  const dir = mkdtempSync(join(tmpdir(), "astro-category-"));
  try {
    const contentDir = join(dir, "articles"),
      layoutPath = join(dir, "layout.astro"),
      publicCssPath = join(dir, "public", "site.css");
    mkdirSync(contentDir);
    mkdirSync(join(dir, "public"));
    writeFileSync(layoutPath, "layout");
    writeFileSync(publicCssPath, "css");
    const rows = Array.from({ length: 100 }, (_, i) => articleRow(i + 1));
    rows[14].title = title;
    const file = join(contentDir, "article-00015.md"),
      original = articleText(rows[14]);
    writeFileSync(file, original);
    const f = fieldNotesOps(
      { model: createModel({ articles: rows }, SPEC), contentDir, layoutPath, publicCssPath },
      { strictAstroCategory: true },
    );
    const clock = {
      ns: 1000000000n,
      now() {
        return (this.ns += 100n);
      },
    };
    let path = CATEGORY_DETAIL,
      doc = "primary",
      num = 0,
      writes = 0;
    const edited = original
      .replace('category: "performance"', 'category: "testing"')
      .replace('categoryLabel: "Performance"', 'categoryLabel: "Testing"');
    const fs = {
      ...f,
      hashOf,
      performWrite: (w, stamp) => {
        writes++;
        return performWrite(w, stamp);
      },
      prepareAttempt(o) {
        const a = f.prepareAttempt(o);
        assert.deepEqual(a.touched, ["article-00015"]);
        assert.deepEqual(
          a.supplemental.map((q) => q.path),
          paths,
        );
        assert.deepEqual(a.write.bytes, Buffer.from(edited));
        assert(!a.write.bytes.includes(a.token));
        assert.deepEqual(a.supplemental.at(-1).predicate.equals, newLinks);
        assert.deepEqual(f.categoryContract.restore.at(-1).equals, oldLinks);
        if (variant === "reorder-visit") a.supplemental.reverse();
        if (variant === "missing-visit") a.supplemental.pop();
        return a;
      },
      restore(snap) {
        const r = f.restore(snap);
        if (variant === "wrong-inode" && r.actions.length) r.actions[0].evidence.inodeAfter++;
        return r;
      },
    };
    const after = [[1, title], [1, "Testing"], 1],
      before = [[1, title], [1, "Performance"], 1];
    const url = (p) => "http://localhost:4321" + p;
    const page = fakePage({
      onWait: (token, _late, p) => {
        const t1 = clock.now();
        Object.assign(p.evidence, { docInits: [], navigations: [], wsFrames: [] });
        return Promise.resolve({
          kind: "notify",
          t1,
          payload: {
            token: variant === "rename-token" ? token + "x" : token,
            docUUID: doc,
            armedDocUUID: doc,
            url: url(path),
            actual:
              variant === "wrong-label"
                ? before
                : variant === "wrong-article"
                  ? [[1, "wrong"], [1, "Testing"], 1]
                  : after,
            producer: {
              designatedMain: true,
              frameUrl: url(path),
              latestDocUUID: doc,
              latestDocNs: "1",
              receivedNs: String(t1),
            },
          },
        });
      },
    });
    page.url = url;
    page.peerSocketTracking = new Map([[1, { firstFrameNs: "1" }]]);
    page.prepare = async () => ({ url: url(path), docUUID: doc, httpStatus: 200 });
    page.arm = async () => ({ armed: true, observedUrl: url(path), docUUID: doc, actual: before });
    page.supplemental = async (q) => {
      assert.equal(readFileSync(file, "utf8"), edited);
      path = q.path;
      let actual = structuredClone(astroPredicateValue(q.predicate));
      num++;
      if (num === 1) {
        assert.equal(actual[0], 0);
        assert.deepEqual(actual[1], [1, "Performance"]);
        assert.equal(actual[2].length, 9);
        if (variant === "empty-old-page") actual[2] = [];
      }
      if (num === 2) {
        assert.equal(actual[0], 1);
        assert.deepEqual(actual[1], [1, "Testing"]);
        assert.equal(actual[2].length, 11);
        if (variant === "wrong-neighbors") actual[2].reverse();
      }
      if (num === 3) {
        assert.deepEqual(actual[0], [[1, "Testing"], "/category/testing/"]);
        if (variant === "wrong-meta-href") actual[0][1] = "/category/performance/";
        if (variant === "wrong-meta") actual[0] = [1, "Performance"];
      }
      if (variant === "reordered-related" && num === 5) actual.reverse();
      doc = variant === "stale-same-route" && num === 3 ? "supp2" : "supp" + num;
      return {
        ok: true,
        httpStatus: variant === "old-page-404" && num === 1 ? 404 : 200,
        actual,
        docUUID: doc,
      };
    };
    page.settle = async ({ bodyRevision: w }) => {
      const ok = w?.path === article(96) && w?.affectedDocUUID === doc;
      if (ok) assert.deepEqual(w.predicate.equals, oldLinks);
      return {
        settled: ok,
        nativeUpdateObserved: true,
        bodyRevision: {
          matched: ok,
          actual: variant === "stale-restore" ? newLinks : oldLinks,
          docUUID: doc,
        },
      };
    };
    page.liveCheck = async (c) => ({
      ok: true,
      httpStatus: 200,
      docUUID: "fresh" + c.path,
      actual: [[], variant === "fresh-wrong-detail" ? after : before],
    });
    const d = {
      ...deps({ fs, page, clock }),
      fixedAstroCategory: true,
      beforeAstroWrite: () => {},
      stageBudget: (ms) => ms,
      workDeadlineNs: 99000000000n,
    };
    const { accounting } = await runBatch({ deps: d, scenarios: [scenario] });
    assert.equal(readFileSync(file, "utf8"), original);
    assert.equal(writes, ["missing-visit", "reorder-visit"].includes(variant) ? 0 : 1);
    if (variant === "success") {
      assert.equal(accounting.error, 0);
      assert.equal(accounting.stopReason, null);
      assert.equal(num, 10);
      assert.equal(accounting.restorationFailures, 0);
      const c = d.recorder.records.find((r) => r.type === "cleanup");
      assert.deepEqual(
        c.live.map((x) => x.path),
        ["/", article(1), article(15), "/archive/1/", "/archive/2/", "/category/architecture/"],
      );
      assert.deepEqual(astroPredicateValue(c.live[2].predicate.of[1]), before);
    } else assert(accounting.stopReason || accounting.error || accounting.restorationFailures);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
test("only original category-15 classification is admitted", () => {
  const p = JSON.parse(
    readFileSync(new URL("../profiles/field-notes-fixed-astro-100.example.json", import.meta.url)),
  );
  delete p.calibration;
  Object.assign(p, {
    scenarios: [scenario],
    fixedRssSafety: "linux-fixed-astro-category.v1",
    fixedPeerBrowser: {
      executablePath: "/browser",
      executableSha256: "1".repeat(64),
      defaultExecutableSha256: "2".repeat(64),
    },
  });
  p.server.readyTimeoutMs = 300000;
  p.limits.wallCeilingMs = 7200000;
  Object.assign(p.timeouts, { attemptMs: 60000, supplementalMs: 60000, restoreMs: 120000 });
  const errs = (q) => [...validateAs(profileSchemaFor(q), q), ...semanticErrors(q)];
  assert.deepEqual(errs(p), []);
  for (const change of [
    (q) => (q.framework = "next"),
    (q) => (q.scenarios[0].ops[0].toCategory = "design"),
    (q) => (q.scenarios[0].ops[0].article = "article-00005"),
    (q) => (q.fixedRssSafety = "linux-fixed-astro-title.v1"),
  ]) {
    const q = structuredClone(p);
    change(q);
    assert(errs(q).length > 0);
  }
});
