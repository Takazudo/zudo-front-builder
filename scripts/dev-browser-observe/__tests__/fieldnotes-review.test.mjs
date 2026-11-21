import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ObserverPage } from "../lib/browser.mjs";
import {
  isBuildOutputPath,
  materializeFieldNotes,
  nextArticleNumber,
  planFieldNotesOp,
} from "../lib/fieldnotes.mjs";
import { createModel, removeArticle } from "../lib/fieldnotes-model.mjs";
import { semanticErrors } from "../lib/profile.mjs";
import { classifyNegative, hiddenOutcome } from "../lib/special-modes.mjs";
import { batchExitOk, runBatch } from "../lib/state-machine.mjs";
import { HARNESS_DIR } from "../lib/util.mjs";
import { deps, fakeFs, fakePage } from "./fakes.mjs";
import { SPEC, articleRow, makeFakeSource } from "./fieldnotes-fake.mjs";
import { readFileSync } from "node:fs";

const tmps = [];
const tmp = () => {
  const d = mkdtempSync(join(tmpdir(), "dbo-fnr-"));
  tmps.push(d);
  return d;
};
afterEach(() => {
  while (tmps.length) rmSync(tmps.pop(), { recursive: true, force: true });
});

const hiddenAttempt = () => ({
  token: "tok-h",
  mode: "hidden",
  targetPath: "/",
  hiddenPath: "/articles/x/",
  hiddenPredicate: { kind: "text", selector: "b", includes: "tok-h" },
  observeMs: 1,
  accessDelayMs: 1,
  writes: [{ method: "in-place", path: "a.md", bytes: "x" }],
});

function modeSetup({ page = {}, attempt = hiddenAttempt(), fs = fakeFs() } = {}) {
  const p = Object.assign(fakePage({}), {
    mark: () => ({}),
    since: () => ({
      mainFrameNavigations: 0,
      newDocuments: 0,
      livereloadEvents: [],
      requests: [],
      wsFrames: [],
      navigations: [],
      docInits: [],
    }),
    settle: async () => ({ settled: true, nativeUpdateObserved: true }),
    navigateAndAwait: async () => ({ ok: true, stage: "confirmed", sinceNavigationMs: 5 }),
    ...page,
  });
  fs.prepareAttempt = () => ({ ...attempt, write: attempt.writes[0] });
  return { page: p, fs, d: deps({ page: p, fs }) };
}

const scen = (op = {}) => [
  {
    id: "h",
    ops: [
      { id: "h1", op: "hidden-edit", article: "x", ...op },
      { id: "n", op: "edit-body", article: "b" },
    ],
  },
];

describe("#1 a throw inside a special mode after writes is a mode-specific outcome with the write recorded", () => {
  it("hidden-edit throws after the write: hidden-error, writePerformed true, lastArmedUnknownOperation written", async () => {
    const { d, fs } = modeSetup({
      page: {
        navigateAndAwait: async () => {
          throw new Error("Target closed");
        },
      },
    });
    const { accounting, stopped } = await runBatch({ scenarios: scen(), deps: d });
    const rec = d.recorder.records.find((r) => r.type === "attempt");
    expect(rec).toMatchObject({ outcome: "hidden-error", writePerformed: true });
    expect(d.recorder.records.some((r) => r.type === "controller-error")).toBe(false);
    expect(accounting).toMatchObject({
      stopReason: "hidden-error",
      error: 1,
      lastArmedUnknownOperation: { op: "h1", writeCompleted: true },
    });
    expect(fs.files["a.md"]).toBe("v0");
    expect(batchExitOk(accounting, stopped)).toBe(false);
  });

  it("a partial multi-step write failure takes writeDone from error.steps", async () => {
    const attempt = {
      ...hiddenAttempt(),
      mode: "final-state",
      finalChecks: [],
      writes: [
        { method: "create", path: "n.md", bytes: "x" },
        { method: "unlink", path: "n.md" },
      ],
    };
    const fs = fakeFs();
    fs.performWrites = async () => {
      throw Object.assign(new Error("ENOENT"), {
        steps: [{ i: 0, method: "create" }],
        failedStep: 1,
      });
    };
    const { d } = modeSetup({ attempt, fs });
    const { accounting } = await runBatch({
      scenarios: [{ id: "f", ops: [{ id: "f1", op: "rapid-sequence" }] }],
      deps: d,
    });
    expect(d.recorder.records.find((r) => r.type === "attempt")).toMatchObject({
      outcome: "final-state-error",
      writePerformed: true,
      writeSteps: [{ i: 0 }],
    });
    expect(accounting.lastArmedUnknownOperation).toMatchObject({ writeCompleted: true });
  });
});

describe("#2 hidden-edit: a never-rendered transient is its own non-ok outcome", () => {
  it("maps outcomes by the check's stage; stale only for a rendered document with the old body", () => {
    expect(hiddenOutcome({ ok: true, stage: "confirmed" })).toBe("success");
    expect(hiddenOutcome({ ok: false, stage: "transient" })).toBe("transient-not-rendered");
    expect(hiddenOutcome({ ok: false, stage: "arm", error: "arm failed" })).toBe("arm-error");
    expect(hiddenOutcome({ ok: false, stage: "navigation", error: "net::ERR" })).toBe(
      "prepare-error",
    );
    expect(hiddenOutcome({ ok: false, stage: "wait" })).toBe("stale");
    expect(hiddenOutcome({ ok: false })).toBe("hidden-check-failed");
  });

  it("transient-not-rendered is not credited as a classify result", async () => {
    const { d } = modeSetup({
      page: {
        navigateAndAwait: async () => ({
          ok: false,
          stage: "transient",
          transient: "transient-not-in-cache",
        }),
      },
    });
    const { accounting, stopped } = await runBatch({
      scenarios: scen({ expect: "classify" }),
      deps: d,
    });
    expect(accounting).toMatchObject({
      stopReason: "transient-not-rendered",
      stoppedByClassifyTimeout: false,
      expectedTimeouts: 0,
      error: 1,
    });
    expect(batchExitOk(accounting, stopped)).toBe(false);
  });
});

describe("#3 the transient re-read also applies after an aborted navigation", () => {
  it("awaitResultingDocument reports notInCache/httpStatus and navigateAndAwait re-reads past it", async () => {
    const p = new ObserverPage(null, "", () => 1n);
    p.baseUrl = "http://127.0.0.1:1";
    // Run the real page function against a stubbed DOM showing zfb's 404 "page not in cache".
    p.page = {
      waitForLoadState: async () => null,
      url: () => "http://127.0.0.1:1/articles/x/",
      evaluate: async (fn, arg) => {
        vi.stubGlobal("window", {
          __dbo: { state: () => ({ docUUID: "r", armed: null }), disarm: () => null },
        });
        vi.stubGlobal("document", {
          body: { innerText: "404 — page not in cache\nThe dev server has no rendered HTML" },
        });
        vi.stubGlobal("performance", { getEntriesByType: () => [{ responseStatus: 404 }] });
        try {
          return fn(arg);
        } finally {
          vi.unstubAllGlobals();
        }
      },
    };
    const resulting = await p.awaitResultingDocument(100);
    expect(resulting.state).toMatchObject({ notInCache: true, httpStatus: 404 });
    let n = 0;
    p.prepare = async () => {
      n += 1;
      if (n === 1) throw new Error("page.goto: net::ERR_ABORTED");
      return { docUUID: "ok", httpStatus: 200, notInCache: false };
    };
    p.arm = async () => ({ armed: false, precondition: "true-before-write", actual: "x" });
    p.failureSnapshot = async () => ({ bodyExcerpt: "fine", notInCache: false });
    const r = await p.navigateAndAwait(
      "/articles/x/",
      { kind: "text", selector: "h1", includes: "x" },
      5000,
      "restore",
    );
    expect(r).toMatchObject({ ok: true, docUUID: "ok" });
    expect(r.transientReads).toHaveLength(1);
    expect(r.recovery[0]).toMatchObject({ usedResultingDocument: true });
  });
});

describe("#4 hidden-edit never-visited rule", () => {
  it("run-time guard: a request for the hidden path before the edit stops with no write", async () => {
    const fs = fakeFs();
    const { d } = modeSetup({
      fs,
      page: { requests: [{ url: "http://127.0.0.1:1/articles/x/", ns: "1" }] },
    });
    const { accounting } = await runBatch({ scenarios: scen(), deps: d });
    expect(fs.writes).toEqual([]);
    expect(d.recorder.records.find((r) => r.type === "attempt")).toMatchObject({
      outcome: "hidden-precondition-visited",
      writePerformed: false,
    });
    expect(accounting).toMatchObject({
      stopReason: "hidden-precondition-visited",
      lastArmedUnknownOperation: { writeCompleted: false },
    });
  });

  it("static rule covers cleanup's detail page, the calibration article, earlier scenarios and renamed targets", () => {
    const base = JSON.parse(
      readFileSync(join(HARNESS_DIR, "profiles", "field-notes-direct-read.example.json"), "utf8"),
    );
    const hidden = (article) => ({
      id: "h",
      set: "workload",
      warmOrder: "none",
      ops: [{ id: "h1", op: "hidden-edit", article, writeMethod: "in-place" }],
    });
    const err = (scenarios) => semanticErrors({ ...base, scenarios }).join("\n");
    expect(err([hidden("article-00001")])).toMatch(/must not be visited/);
    expect(err([hidden("article-00002")])).toMatch(/must not be visited/);
    expect(
      err([
        {
          id: "a",
          warmOrder: "none",
          ops: [{ op: "edit-body", article: "article-00050", writeMethod: "in-place" }],
        },
        hidden("article-00050"),
      ]),
    ).toMatch(/must not be visited/);
    expect(
      err([
        {
          id: "a",
          warmOrder: "none",
          ops: [{ op: "rename-slug-change", article: "article-00060", writeMethod: "rename" }],
        },
        hidden("article-00060-renamed"),
      ]),
    ).toMatch(/must not be visited/);
    expect(err([hidden("article-00077")])).toBe("");
  });
});

describe("#5 new article numbers come from max(existing)+1", () => {
  it("a delete does not make the next article reuse an existing slug", () => {
    const model = createModel(
      { articles: Array.from({ length: 20 }, (_, i) => articleRow(i + 1)) },
      SPEC,
    );
    const afterDelete = removeArticle(model, "article-00005");
    expect(afterDelete.articles).toHaveLength(19);
    expect(nextArticleNumber(afterDelete)).toBe(21);
    const root = tmp();
    makeFakeSource(join(root, "src"));
    const m = materializeFieldNotes({
      fixtureDir: join(HARNESS_DIR, "fixtures", "field-notes"),
      sourceDir: join(root, "src"),
      workDir: join(root, "w"),
      adapter: "direct-read",
    });
    expect(planFieldNotesOp(m, afterDelete, { op: "add-article" }).declared.slug).toBe(
      "article-00021",
    );
  });
});

describe("nits", () => {
  it("#6 the wait gets only the remaining budget", async () => {
    let t = 0n;
    const p = new ObserverPage(null, "", () => (t += 300_000_000n));
    p.baseUrl = "http://127.0.0.1:1";
    p.prepare = async () => ({ docUUID: "d", httpStatus: 200, notInCache: false });
    p.arm = async () => ({ armed: true, docUUID: "d" });
    let waitedFor = null;
    p.waitNotify = async (_token, ms) => {
      waitedFor = ms;
      return { kind: "timeout" };
    };
    p.disarm = async () => null;
    p.failureSnapshot = async () => ({ bodyExcerpt: "", notInCache: false });
    await p.navigateAndAwait("/", { kind: "text", selector: "h1", includes: "x" }, 1000, "supp");
    expect(waitedFor).toBeLessThan(1000);
    expect(waitedFor).toBeGreaterThanOrEqual(1);
  });

  it("#7 error markers are matched on the full page text", () => {
    const view = {
      ok: true,
      httpStatus: 200,
      bodyExcerpt: "a long page...",
      markersFound: ["Article slug mismatch"],
    };
    expect(classifyNegative(view, { errorMarkers: ["Article slug mismatch"] }).state).toBe("error");
    expect(
      classifyNegative({ ...view, markersFound: [] }, { errorMarkers: ["Article slug mismatch"] })
        .state,
    ).toBe("other");
  });

  it("#8 the copy filter judges source-relative paths, so a source under /dist/ still materializes", () => {
    expect(isBuildOutputPath(join("components", "Layout.tsx"))).toBe(false);
    expect(isBuildOutputPath(join("node_modules", "x"))).toBe(true);
    const root = tmp();
    makeFakeSource(join(root, "dist", "src"));
    const m = materializeFieldNotes({
      fixtureDir: join(HARNESS_DIR, "fixtures", "field-notes"),
      sourceDir: join(root, "dist", "src"),
      workDir: join(root, "w"),
      adapter: "direct-read",
    });
    expect(existsSync(join(m.appDir, "components", "Layout.tsx"))).toBe(true);
    expect(m.model.articles).toHaveLength(20);
  });
});
