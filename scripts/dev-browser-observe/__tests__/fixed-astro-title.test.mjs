import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "vitest";
import {
  ASTRO_TITLE,
  TITLE_DETAIL,
  TITLE_SELECTOR,
  TITLE_RELATED,
  TITLE_SUPPLEMENTAL,
} from "../lib/astro-title.mjs";
import { fieldNotesOps } from "../lib/fieldnotes.mjs";
import { createModel, referrersOf } from "../lib/fieldnotes-model.mjs";
import { performWrite, hashOf } from "../lib/fixture.mjs";
import { runBatch } from "../lib/state-machine.mjs";
import { profileSchemaFor, semanticErrors } from "../lib/profile.mjs";
import { validateAs } from "../lib/schema.mjs";
import { SPEC, articleRow, articleText } from "./fieldnotes-fake.mjs";
import { deps, fakePage } from "./fakes.mjs";

const op = {
  id: "title-05-detail",
  op: "edit-title",
  article: "article-00005",
  writeMethod: "in-place",
  expect: "classify",
};
const scenario = { id: "fn-title-detail", set: "workload", warmOrder: "none", ops: [op] };
const routes = [
  "/",
  "/articles/article-00001/",
  TITLE_DETAIL,
  "/archive/1/",
  "/archive/2/",
  "/category/architecture/",
];
test.each([
  "success",
  "wrong-target",
  "missing-supp",
  "swapped-supp",
  "extra-supp",
  "wrong-primary-title",
  "supplemental-previous-doc",
  "wrong-affected",
  "fresh-wrong-title",
  "omit-fresh",
  "wrong-inode",
  "bad-hash",
])(
  "title cell uses true planner, writer and distinct affected/fresh predicates: %s",
  async (variant) => {
    const dir = mkdtempSync(join(tmpdir(), "astro-title-"));
    try {
      const contentDir = join(dir, "content"),
        layoutPath = join(dir, "layout.astro"),
        publicCssPath = join(dir, "public", "a.css");
      mkdirSync(contentDir, { recursive: true });
      mkdirSync(join(dir, "public"));
      writeFileSync(layoutPath, "layout");
      writeFileSync(publicCssPath, "css");
      const rows = Array.from({ length: 100 }, (_, i) => articleRow(i + 1));
      rows[4].title = ASTRO_TITLE;
      const model = createModel({ articles: rows }, SPEC),
        file = join(contentDir, "article-00005.md"),
        original = articleText(rows[4]);
      writeFileSync(file, original);
      const f = fieldNotesOps(
        { model, layoutPath, contentDir, publicCssPath },
        { strictAstroTitle: true },
      );
      let edited,
        doc = "pre",
        path = TITLE_DETAIL,
        n = 0,
        primaryToken;
      const clock = {
        ns: 1000000000n,
        now() {
          return (this.ns += 100n);
        },
      };
      const fs = {
        ...f,
        hashOf,
        performWrite,
        prepareAttempt(o) {
          const plan = f.prepareAttempt(o);
          primaryToken = plan.token;
          edited = ASTRO_TITLE + " " + plan.token;
          assert.deepEqual(plan.touched, ["article-00005"]);
          assert.deepEqual(referrersOf(model, o.article), referrersOf(plan.nextModel, o.article));
          assert.equal(
            plan.write.bytes.toString(),
            original.replace(
              "title: " + JSON.stringify(ASTRO_TITLE),
              "title: " + JSON.stringify(edited),
            ),
          );
          if (variant === "wrong-target") plan.supplemental[3].predicate.of[1].selector = ".wrong";
          if (variant === "missing-supp") plan.supplemental.pop();
          if (variant === "swapped-supp") plan.supplemental.reverse();
          if (variant === "extra-supp") plan.supplemental.push(plan.supplemental[0]);
          return plan;
        },
        liveChecks() {
          const checks = f.liveChecks();
          assert.deepEqual(
            checks.map((c) => c.path),
            routes,
          );
          return variant === "omit-fresh" ? checks.filter((c) => c.path !== TITLE_DETAIL) : checks;
        },
        restore(snapshot) {
          const out = f.restore(snapshot);
          if (out.actions.length && variant === "wrong-inode") out.actions[0].evidence.inodeAfter++;
          if (variant === "bad-hash") out.mismatches.push({ path: file });
          return out;
        },
      };
      const url = (p) => "http://localhost:4321" + p;
      const page = fakePage({
        onWait: (token, _late, p) => {
          const t1 = clock.now();
          Object.assign(p.evidence, { docInits: [], navigations: [], wsFrames: [] });
          return Promise.resolve({
            kind: "notify",
            t1,
            payload: {
              token,
              docUUID: doc,
              armedDocUUID: doc,
              url: url(path),
              actual: [1, variant === "wrong-primary-title" ? edited + " extra" : edited],
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
      page.prepare = async () => ({ docUUID: doc, url: url(path) });
      page.arm = async () => ({
        armed: true,
        docUUID: doc,
        observedUrl: url(path),
        actual: [1, ASTRO_TITLE],
      });
      page.supplemental = async (check) => {
        assert.equal(check.path, TITLE_SUPPLEMENTAL[n][0]);
        assert.equal(check.predicate.of[1].selector, TITLE_SUPPLEMENTAL[n][1]);
        assert.equal(readFileSync(file, "utf8").includes(primaryToken), true);
        n++;
        path = check.path;
        doc = variant === "supplemental-previous-doc" && n === 2 ? "sup1" : "sup" + n;
        return { ok: true, docUUID: doc, actual: [1, edited] };
      };
      page.settle = async ({ bodyRevision: w }) => {
        const ok =
          w?.path === path &&
          w?.affectedDocUUID === doc &&
          w?.predicate?.of?.[1].selector === TITLE_RELATED &&
          variant !== "wrong-affected";
        return {
          settled: ok,
          nativeUpdateObserved: true,
          bodyRevision: { matched: ok, docUUID: doc, actual: [1, ASTRO_TITLE] },
        };
      };
      page.liveCheck = async (c) => {
        if (c.path === TITLE_DETAIL && n === 6)
          assert.equal(c.predicate.of[1].of[1].selector, TITLE_SELECTOR);
        return {
          ok: true,
          docUUID: "fresh" + c.path,
          actual: [[], [1, variant === "fresh-wrong-title" ? "bogus" : ASTRO_TITLE]],
        };
      };
      const d = {
        ...deps({ fs, page, clock }),
        fixedAstroTitle: true,
        beforeAstroWrite: () => {},
        stageBudget: (ms) => ms,
        workDeadlineNs: 99000000000n,
      };
      const { accounting } = await runBatch({ scenarios: [scenario], deps: d });
      assert.equal(readFileSync(file, "utf8"), original);
      if (variant === "success") {
        assert.equal(n, 6, JSON.stringify(d.recorder.records.filter((r) => r.error)));
        assert.equal(accounting.success, 1);
        assert.equal(accounting.stopReason, null);
        assert.equal(accounting.restorationFailures, 0);
        const c = d.recorder.records.find((r) => r.type === "cleanup");
        assert.equal(c.restorationWitness.path, "/articles/article-00095/");
        assert.equal(c.live[2].path, TITLE_DETAIL);
      } else assert(accounting.stopReason || accounting.error || accounting.restorationFailures);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  },
);
test("title-detail protocol refuses other selectors of the original fixed matrix", () => {
  const p = JSON.parse(
    readFileSync(new URL("../profiles/field-notes-fixed-astro-100.example.json", import.meta.url)),
  );
  delete p.calibration;
  Object.assign(p, {
    scenarios: [scenario],
    fixedRssSafety: "linux-fixed-astro-title.v1",
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
    (q) => (q.scenarios[0].ops[0].targetPath = "/"),
    (q) => (q.scenarios[0].id = "fn-title-home"),
    (q) => (q.scenarios[0].ops[0].article = "article-00006"),
    (q) => (q.fixedRssSafety = "linux-fixed-astro-footer.v1"),
  ]) {
    const q = structuredClone(p);
    change(q);
    assert(errs(q).length > 0);
  }
});
