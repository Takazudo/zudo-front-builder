import { describe, expect, it } from "vitest";
import { classifyNegativeState } from "../lib/special-modes.mjs";
import { batchExitOk, runBatch } from "../lib/state-machine.mjs";
import { deps, fakeFs, fakePage } from "./fakes.mjs";

const quiet = { settled: true, reason: "no-frame-for-quietMs" };

function modePage(overrides = {}) {
  const p = fakePage({});
  return Object.assign(p, {
    mark: () => ({ m: 1 }),
    since: () => ({
      mainFrameNavigations: 0,
      newDocuments: 0,
      livereloadEvents: [],
      requests: [],
      wsFrames: [],
      navigations: [],
      docInits: [],
    }),
    settle: async () => quiet,
    observePage: async (path) => ({
      ok: true,
      path,
      httpStatus: 500,
      title: "Error",
      bodyExcerpt: "Article slug mismatch: article-00008",
    }),
    evaluateNow: async () => ({ matched: false }),
    navigateAndAwait: async () => ({ ok: true, firstReadMatched: true, sinceNavigationMs: 42 }),
    ...overrides,
  });
}

function modeFs(attempt) {
  const fs = fakeFs();
  fs.prepareAttempt = () => ({ ...attempt, write: attempt.writes[0] });
  fs.performWrites = async (writes, now) => ({
    t0: now(),
    tLast: now(),
    steps: writes.map((w, i) => ({ i, method: w.method, path: w.path })),
  });
  return fs;
}

const negAttempt = (overrides = {}) => ({
  token: "tok-n",
  mode: "expected-negative",
  targetPath: "/",
  writes: [{ method: "rename", path: "a.md", to: "a-renamed.md" }],
  expectState: ["error", "last-good"],
  observe: [{ path: "/", lastGood: { kind: "count", selector: "x", equals: 1 } }],
  errorMarkers: ["Article slug mismatch"],
  recovery: [{ method: "rename", path: "a-renamed.md", to: "a.md" }],
  recoveryChecks: [{ path: "/", predicate: {} }],
  ...overrides,
});

const scen = (op) => [
  { id: "neg", set: "expected-negative", ops: [op, { id: "next", op: "edit-body", article: "b" }] },
];

describe("classifyNegativeState", () => {
  it("error by 5xx or a declared marker, last-good by 200 + original content, otherwise other", () => {
    expect(classifyNegativeState({ ok: true, httpStatus: 500, bodyExcerpt: "" }, null)).toBe(
      "error",
    );
    expect(
      classifyNegativeState(
        { ok: true, httpStatus: 200, bodyExcerpt: "Article slug mismatch: x" },
        null,
        ["Article slug mismatch"],
      ),
    ).toBe("error");
    expect(
      classifyNegativeState({ ok: true, httpStatus: 200, bodyExcerpt: "home" }, { matched: true }),
    ).toBe("last-good");
    expect(
      classifyNegativeState({ ok: true, httpStatus: 200, bodyExcerpt: "home" }, { matched: false }),
    ).toBe("other");
    expect(
      classifyNegativeState({ ok: true, httpStatus: 404, bodyExcerpt: "" }, { matched: true }),
    ).toBe("other");
    expect(classifyNegativeState({ ok: false }, null)).toBe("navigation-error");
  });
});

describe("expected-negative accounting", () => {
  it("a declared error state plus a successful recovery is counted as expected-negative-observed and the batch continues", async () => {
    const page = modePage({ behaviors: [] });
    page.supplemental = async () => ({ ok: true });
    const fs = modeFs(negAttempt());
    let n = 0;
    const realPrepare = fs.prepareAttempt;
    fs.prepareAttempt = (op) =>
      n++ === 0
        ? realPrepare(op)
        : {
            token: "tok-b",
            targetPath: "/b",
            predicate: { kind: "text", selector: "x", includes: "tok-b" },
            supplemental: [],
            write: { method: "in-place", path: "b.md", bytes: "x" },
          };
    page.waitNotify = () =>
      Promise.resolve({
        kind: "notify",
        payload: { token: "tok-b", docUUID: "doc-1" },
        t1: 9_000_000_000n,
      });
    const d = deps({ page, fs });
    const { accounting, stopped } = await runBatch({
      scenarios: scen({ id: "neg-op", op: "rename-filename-only", article: "a" }),
      deps: d,
    });
    const rec = d.recorder.records.find(
      (r) => r.type === "attempt" && r.mode === "expected-negative",
    );
    expect(rec).toMatchObject({
      outcome: "expected-negative-observed",
      statePass: true,
      observed: [{ state: "error" }],
      recovery: { ok: true },
    });
    expect(accounting).toMatchObject({
      expectedNegativePasses: 1,
      error: 0,
      success: 1,
      attempted: 2,
      notAttempted: 0,
    });
    expect(stopped).toBe(false);
    expect(batchExitOk(accounting, stopped)).toBe(true);
  });

  it("an undeclared state is expected-negative-mismatch: an error that stops the batch (still recovering files)", async () => {
    const page = modePage({
      observePage: async (path) => ({
        ok: true,
        path,
        httpStatus: 200,
        title: "ok",
        bodyExcerpt: "fine",
      }),
    });
    page.supplemental = async () => ({ ok: true });
    const fs = modeFs(negAttempt());
    const d = deps({ page, fs });
    const { accounting, stopped } = await runBatch({
      scenarios: scen({ id: "neg-op", op: "rename-filename-only", article: "a" }),
      deps: d,
    });
    const rec = d.recorder.records.find((r) => r.type === "attempt");
    expect(rec).toMatchObject({
      outcome: "expected-negative-mismatch",
      observed: [{ state: "other" }],
      recovery: { ok: true },
    });
    expect(accounting).toMatchObject({
      stopReason: "expected-negative-mismatch",
      error: 1,
      expectedNegativePasses: 0,
      notAttempted: 1,
    });
    expect(batchExitOk(accounting, stopped)).toBe(false);
  });

  it("a recovery that does not restore live behaviour is recovery-failed, never relabelled as expected", async () => {
    const page = modePage();
    page.supplemental = async () => ({ ok: false, firstReadMatched: false });
    const d = deps({ page, fs: modeFs(negAttempt()) });
    const { accounting, stopped } = await runBatch({
      scenarios: scen({ id: "neg-op", op: "rename-filename-only", article: "a" }),
      deps: d,
    });
    expect(d.recorder.records.find((r) => r.type === "attempt").outcome).toBe("recovery-failed");
    expect(accounting).toMatchObject({ stopReason: "recovery-failed", error: 1 });
    expect(batchExitOk(accounting, stopped)).toBe(false);
  });
});

describe("hidden-edit mode", () => {
  const hiddenAttempt = {
    token: "tok-h",
    mode: "hidden",
    targetPath: "/",
    hiddenPath: "/articles/x/",
    hiddenPredicate: { kind: "text", selector: "b", includes: "tok-h" },
    observeMs: 1,
    accessDelayMs: 1,
    writes: [{ method: "in-place", path: "a.md", bytes: "x" }],
  };

  it("records active-page effects, prefetch requests and first display after navigation, with no update latency", async () => {
    const page = modePage({
      since: () => ({
        mainFrameNavigations: 0,
        newDocuments: 0,
        livereloadEvents: ["page"],
        requests: [
          { url: "http://h/articles/x/", ns: "1" },
          { url: "http://h/other/", ns: "2" },
        ],
        wsFrames: [],
        navigations: [],
        docInits: [],
      }),
    });
    const d = deps({ page, fs: modeFs(hiddenAttempt) });
    const { accounting } = await runBatch({
      scenarios: [{ id: "h", ops: [{ id: "h1", op: "hidden-edit", article: "x" }] }],
      deps: d,
    });
    const rec = d.recorder.records.find((r) => r.type === "attempt");
    expect(rec).toMatchObject({
      mode: "hidden",
      outcome: "success",
      firstDisplayAfterNavigationMs: 42,
      activeEffects: { reloaded: false, livereloadEvents: ["page"] },
      accessDelayMs: 1,
    });
    expect(rec.prefetchRequests.map((r) => r.url)).toEqual(["http://h/articles/x/"]);
    expect(rec.latencyMs).toBeUndefined();
    expect(accounting.success).toBe(1);
  });

  it("a stale body after navigation under expect classify stops with stoppedByClassifyTimeout", async () => {
    const page = modePage({
      navigateAndAwait: async () => ({ ok: false, firstReadMatched: false }),
    });
    const d = deps({ page, fs: modeFs(hiddenAttempt) });
    const { accounting, stopped } = await runBatch({
      scenarios: [
        {
          id: "h",
          ops: [
            { id: "h1", op: "hidden-edit", article: "x", expect: "classify" },
            { id: "n", op: "edit-body", article: "b" },
          ],
        },
      ],
      deps: d,
    });
    expect(accounting).toMatchObject({
      stale: 1,
      stoppedByClassifyTimeout: true,
      expectedTimeouts: 1,
      notAttempted: 1,
    });
    expect(batchExitOk(accounting, stopped)).toBe(true);
  });
});

describe("final-state mode", () => {
  const fsAttempt = {
    token: "tok-f",
    mode: "final-state",
    targetPath: "/",
    writes: [
      { method: "create", path: "n.md", bytes: "x" },
      { method: "unlink", path: "n.md", afterMs: 100 },
    ],
    finalChecks: [
      { path: "/", predicate: {} },
      { kind: "gone", path: "/n/" },
    ],
    declared: { finalState: "unchanged" },
  };
  it("success only when every final check passes; a mismatch is an error stop", async () => {
    const ok = modePage();
    ok.supplemental = async () => ({ ok: true });
    const d1 = deps({ page: ok, fs: modeFs(fsAttempt) });
    const r1 = await runBatch({
      scenarios: [{ id: "f", ops: [{ id: "f1", op: "rapid-sequence" }] }],
      deps: d1,
    });
    expect(d1.recorder.records.find((r) => r.type === "attempt")).toMatchObject({
      outcome: "success",
      mode: "final-state",
      declared: { finalState: "unchanged" },
    });
    expect(r1.accounting.success).toBe(1);
    const bad = modePage();
    bad.supplemental = async (c) => ({ ok: c.kind !== "gone" });
    const d2 = deps({ page: bad, fs: modeFs(fsAttempt) });
    const r2 = await runBatch({
      scenarios: [{ id: "f", ops: [{ id: "f1", op: "rapid-sequence" }] }],
      deps: d2,
    });
    expect(r2.accounting).toMatchObject({ stopReason: "final-state-mismatch", error: 1 });
  });
});

describe("standard multi-step writes", () => {
  it("records both latencies, every step and the watch evidence", async () => {
    const attempt = {
      token: "t5",
      targetPath: "/a",
      predicate: { kind: "text", selector: "h1", includes: "t5" },
      supplemental: [],
      watch: [{ selector: "h1", tokens: ["t1", "t2"] }],
      writes: [
        { method: "in-place", path: "a.md", bytes: "1" },
        { method: "in-place", path: "a.md", bytes: "2", afterMs: 50 },
      ],
    };
    const fs = fakeFs();
    fs.prepareAttempt = () => ({ ...attempt, write: attempt.writes[0] });
    fs.performWrites = async () => ({
      t0: 1_000_000_000n,
      tLast: 1_200_000_000n,
      steps: [{ i: 0 }, { i: 1 }],
    });
    const page = fakePage({});
    const armed = [];
    page.arm = async (token, predicate, watch) => {
      armed.push(watch);
      return { armed: true, docUUID: "doc-1" };
    };
    page.waitNotify = () =>
      Promise.resolve({
        kind: "notify",
        payload: { token: "t5", docUUID: "doc-1", watchSeen: { t1: { atPerfMs: 3 } } },
        t1: 1_500_000_000n,
      });
    const d = deps({ page, fs });
    await runBatch({
      scenarios: [{ id: "s", ops: [{ id: "b", op: "burst", article: "a" }] }],
      deps: d,
    });
    const rec = d.recorder.records.find((r) => r.type === "attempt");
    expect(armed[0]).toEqual(attempt.watch);
    expect(rec).toMatchObject({
      outcome: "success",
      latencyFromFirstWriteMs: 500,
      latencyFromLastWriteMs: 300,
      watchSeen: { t1: { atPerfMs: 3 } },
      writeSteps: [{ i: 0 }, { i: 1 }],
    });
  });

  it("a notify not after the LAST write is precondition-unstable", async () => {
    const attempt = {
      token: "t5",
      targetPath: "/a",
      predicate: { kind: "text", selector: "h1", includes: "t5" },
      supplemental: [],
      writes: [
        { method: "in-place", path: "a.md", bytes: "1" },
        { method: "in-place", path: "a.md", bytes: "2", afterMs: 50 },
      ],
    };
    const fs = fakeFs();
    fs.prepareAttempt = () => ({ ...attempt, write: attempt.writes[0] });
    fs.performWrites = async () => ({ t0: 1_000_000_000n, tLast: 1_200_000_000n, steps: [] });
    const page = fakePage({ behaviors: [{ kind: "notify", t1: 1_100_000_000n }] });
    const d = deps({ page, fs });
    const { accounting } = await runBatch({
      scenarios: [{ id: "s", ops: [{ id: "b", op: "burst", article: "a" }] }],
      deps: d,
    });
    expect(accounting.stopReason).toBe("precondition-unstable");
  });
});
