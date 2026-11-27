import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "vitest";
import { ASTRO_FOOTER, astroFooterPredicate } from "../lib/astro-footer.mjs";
import { createModel } from "../lib/fieldnotes-model.mjs";
import { fieldNotesOps } from "../lib/fieldnotes.mjs";
import { performWrite, hashOf } from "../lib/fixture.mjs";
import { semanticErrors, profileSchemaFor } from "../lib/profile.mjs";
import { validateAs } from "../lib/schema.mjs";
import { ObserverPage, updateSignalFrame } from "../lib/browser.mjs";
import { runBatch } from "../lib/state-machine.mjs";
import { SPEC, articleRow } from "./fieldnotes-fake.mjs";
import { deps, fakePage } from "./fakes.mjs";

const scenario = {
  id: "fn-shared-component",
  set: "workload",
  warmOrder: "none",
  ops: [{ id: "footer", op: "edit-shared-component", writeMethod: "in-place", targetPath: "/" }],
};
const routes = [
  "/",
  "/articles/article-00001/",
  "/archive/1/",
  "/archive/2/",
  "/category/architecture/",
];
const template = readFileSync(
  new URL("../fixtures/field-notes/distributions/astro/Site.astro.src", import.meta.url),
  "utf8",
);
test.each([
  "success",
  "source-ambiguous",
  "wrong-or-extra-token",
  "no-supplement",
  "supplement-wrong-text",
  "reused-archive",
  "wrong-restore-route",
  "missing-archive-2",
  "bad-inode",
  "late",
  "early",
  "foreign-uuid",
  "retired-home",
  "foreign-frame",
  "valid-reload",
])("real Astro layout planner, update and ordered restoration: %s", async (scenarioCase) => {
  const dir = mkdtempSync(join(tmpdir(), "astro-footer-"));
  try {
    const contentDir = join(dir, "content"),
      layoutPath = join(dir, "Site.astro"),
      publicCssPath = join(dir, "public", "site.css");
    mkdirSync(contentDir);
    mkdirSync(join(dir, "public"));
    writeFileSync(publicCssPath, "");
    writeFileSync(layoutPath, template + (scenarioCase === "source-ambiguous" ? ASTRO_FOOTER : ""));
    const model = createModel(
      { articles: Array.from({ length: 100 }, (_, i) => articleRow(i + 1)) },
      SPEC,
    );
    const ops = fieldNotesOps(
      { model, layoutPath, contentDir, publicCssPath, manifest: { layoutFooter: ASTRO_FOOTER } },
      { strictAstroFooter: true },
    );
    const fs = { ...ops, hashOf, performWrite };
    const clock = {
      ns: 1000000000n,
      now() {
        return (this.ns += 10000n);
      },
    };
    let token,
      target,
      deadline,
      active = "/",
      writes = 0;
    fs.prepareAttempt = (op) => {
      const plan = ops.prepareAttempt(op);
      token = plan.token;
      target = ASTRO_FOOTER + " " + token;
      assert.deepEqual(plan.touched, []);
      assert.equal(plan.write.bytes.toString(), template.replace(ASTRO_FOOTER, target));
      assert.deepEqual(plan.predicate, astroFooterPredicate(target));
      assert.deepEqual(plan.supplemental, [{ path: "/archive/1/", predicate: plan.predicate }]);
      if (scenarioCase === "no-supplement") plan.supplemental = [];
      return plan;
    };
    fs.performWrite = (w, now) => {
      writes++;
      return performWrite(w, now);
    };
    fs.restore = (snapshot) => {
      const result = ops.restore(snapshot);
      if (scenarioCase === "bad-inode" && result.actions.length)
        result.actions[0].evidence.inodeAfter++;
      return result;
    };
    fs.liveChecks = () => {
      const checks = ops.liveChecks();
      assert.deepEqual(
        checks.map((c) => c.path),
        routes,
      );
      return scenarioCase === "missing-archive-2"
        ? checks.filter((c) => c.path !== "/archive/2/")
        : checks;
    };
    const page = fakePage({
      onWait: (tok, _late, p) => {
        p.evidence.docInits = [];
        p.evidence.navigations = [];
        p.evidence.wsFrames = [];
        const t1 =
          scenarioCase === "late" ? deadline + 1n : scenarioCase === "early" ? 1n : clock.now();
        const changed = ["retired-home", "valid-reload"].includes(scenarioCase);
        const activeDoc = changed ? "home-reloaded" : "home-edited";
        const url = page.url("/");
        if (changed) {
          p.evidence.navigations.push({ ns: String(t1 - 2n), url });
          p.evidence.docInits.push({
            ns: String(t1 - 1n),
            url,
            docUUID: activeDoc,
            designatedMain: true,
          });
        }
        const docUUID =
          scenarioCase === "foreign-uuid"
            ? "unseen"
            : scenarioCase === "retired-home"
              ? "home-edited"
              : activeDoc;
        return Promise.resolve({
          kind: "notify",
          t1,
          payload: {
            token: tok,
            docUUID,
            armedDocUUID: "home-edited",
            url,
            producer: {
              designatedMain: scenarioCase !== "foreign-frame",
              frameUrl: url,
              latestDocUUID: activeDoc,
              latestDocNs: String(changed ? t1 - 1n : 100n),
              receivedNs: String(t1),
            },
            actual: [1, scenarioCase === "wrong-or-extra-token" ? target + " extra" : target],
          },
        });
      },
    });
    page.url = (path) => "http://localhost:1234" + path;
    page.prepare = async (path) => ({ docUUID: "home-edited", url: page.url(path) });
    page.peerSocketTracking = new Map([[1, { firstFrameNs: "1" }]]);
    page.arm = async () => ({
      armed: true,
      docUUID: "home-edited",
      observedUrl: page.url("/"),
      actual: [1, ASTRO_FOOTER],
    });
    page.supplemental = async (c) => {
      active = c.path;
      return {
        ok: true,
        docUUID: "archive-edited",
        actual: [1, scenarioCase === "supplement-wrong-text" ? "other" : target],
      };
    };
    page.settle = async (opts) => {
      assert.equal(readFileSync(layoutPath, "utf8"), template);
      const path = opts.bodyRevision?.path;
      return {
        settled: path === active && scenarioCase !== "wrong-restore-route",
        nativeUpdateObserved: true,
        bodyRevision: {
          matched: path === active,
          docUUID: "archive-edited",
          actual: [1, ASTRO_FOOTER],
        },
      };
    };
    page.liveCheck = async (c) => {
      if (c.path === "/archive/1/" && active === c.path)
        assert.deepEqual(c.predicate.of[1], astroFooterPredicate(ASTRO_FOOTER));
      return {
        ok: true,
        docUUID:
          scenarioCase === "reused-archive" && c.path === active
            ? "archive-edited"
            : "fresh" + c.path,
        actual: [[], [1, ASTRO_FOOTER]],
      };
    };
    const d = {
      ...deps({ fs, page, clock }),
      fixedAstroFooter: true,
      workDeadlineNs: 99000000000n,
      beforeAstroWrite: () => {},
      stageBudget: (ms, ns) => {
        deadline = ns + BigInt(ms) * 1000000n;
        return ms;
      },
    };
    const { accounting } = await runBatch({ deps: d, scenarios: [scenario] });
    assert.equal(writes, ["source-ambiguous", "no-supplement"].includes(scenarioCase) ? 0 : 1);
    if (["success", "valid-reload"].includes(scenarioCase)) {
      assert.equal(accounting.success, 1);
      assert.equal(accounting.restorationFailures, 0);
      assert.equal(accounting.supplementalFailures, 0);
      assert.equal(accounting.stopReason, null);
      assert.equal(d.recorder.records.find((r) => r.type === "cleanup").live.length, 5);
    } else assert(accounting.stopReason || accounting.error || accounting.restorationFailures);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
test("standalone footer capability rejects other frameworks, scales or operations", () => {
  const p = JSON.parse(
    readFileSync(new URL("../profiles/field-notes-fixed-astro-100.example.json", import.meta.url)),
  );
  p.fixedRssSafety = "linux-fixed-astro-footer.v1";
  p.fixedPeerBrowser = {
    executablePath: "/pinned/browser",
    executableSha256: "1".repeat(64),
    defaultExecutableSha256: "2".repeat(64),
  };
  delete p.calibration;
  p.scenarios = [scenario];
  p.server.readyTimeoutMs = 300000;
  p.limits.wallCeilingMs = 7200000;
  Object.assign(p.timeouts, { attemptMs: 60000, supplementalMs: 60000, restoreMs: 120000 });
  const errors = (q) => [...validateAs(profileSchemaFor(q), q), ...semanticErrors(q)];
  assert.deepEqual(errors(p), []);
  for (const change of [
    (q) => (q.framework = "next"),
    (q) => (q.fixture.scale = 1000),
    (q) => (q.scenarios[0].ops[0].repeat = { warmups: 1, measured: 1 }),
    (q) => (q.scenarios[0].ops[0].op = "edit-managed-css"),
    (q) => (q.scenarios[0].ops[0].targetPath = "/archive/1/"),
  ]) {
    const q = structuredClone(p);
    change(q);
    assert(errors(q).length > 0);
  }
});
test("only declared layout updates qualify for module attribution", () => {
  const socket = {
    name: "vite",
    typeKey: "type",
    events: { update: "module", "full-reload": "page" },
  };
  for (const path of ["/src/layouts/Site.astro", "/unrelated.astro"]) {
    const frame = updateSignalFrame(
      JSON.stringify({
        type: "update",
        updates: [{ type: "js-update", path, acceptedPath: path }],
      }),
      1n,
      "ws://localhost/",
      socket,
      true,
    );
    assert.equal(frame.layoutModule, path === "/src/layouts/Site.astro");
  }
});

test("Playwright binding independently identifies its actual main page/frame at dispatch", () => {
  const observer = new ObserverPage(null, "");
  const url = "http://localhost/";
  const main = { url: () => url },
    iframe = { url: () => url };
  const page = { mainFrame: () => main };
  observer.page = page;
  observer.peerSocketTracking = new Map();
  observer.fixedAstroObservation = true;
  const genuine = { page, frame: main },
    wrongFrame = { page, frame: iframe },
    wrongPage = { page: {}, frame: main };
  observer.onDocInit({ docUUID: "home", url }, 1n, genuine);
  for (const source of [wrongPage, wrongFrame]) {
    observer.onDocInit({ docUUID: "bogus", url }, 2n, source);
    assert.equal(observer.docInits.length, 1);
    observer.armedToken = "foreign";
    observer.onNotify(
      { token: "foreign", docUUID: "home", producer: { designatedMain: true } },
      3n,
      source,
    );
    assert.equal(observer.early.get("foreign").payload.producer.designatedMain, false);
    observer.early.clear();
  }
  observer.onDocInit({ docUUID: "reloaded", url }, 4n, genuine);
  observer.armedToken = "proper";
  observer.onNotify({ token: "proper", docUUID: "reloaded" }, 5n, genuine);
  assert.deepEqual(observer.early.get("proper").payload.producer, {
    designatedMain: true,
    frameUrl: url,
    latestDocUUID: "reloaded",
    latestDocNs: "4",
    receivedNs: "5",
  });
});

test("an absent global reload path and a malformed null reload path remain distinguishable", () => {
  const socket = { name: "vite", typeKey: "type", events: { "full-reload": "page" } };
  for (const msg of [{ type: "full-reload" }, { type: "full-reload", path: null }]) {
    const row = updateSignalFrame(JSON.stringify(msg), 7n, "ws://localhost/", socket, true);
    assert.equal(row.hasUpdatePath, Object.hasOwn(msg, "path"));
    assert.equal(row.pageSignal, !row.hasUpdatePath);
  }
});

test("body peer socket tracking preserves the ordinary document registration", () => {
  const observer = new ObserverPage(null, "");
  observer.peerSocketTracking = new Map();
  observer.onDocInit({ docUUID: "body" }, 1n);
  assert.equal(observer.docInits[0].docUUID, "body");
  assert.equal(observer.docInits[0].designatedMain, undefined);
});
