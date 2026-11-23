import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { runInNewContext } from "node:vm";
import { afterEach, describe, expect, it } from "vitest";
import { ObserverPage } from "../lib/browser.mjs";
import {
  blockWrite,
  hashOf,
  materialize,
  prepareAttempt,
  performWrite,
  restoreSnapshot,
  snapshotSources,
  unblockWrite,
} from "../lib/fixture.mjs";
import { semanticErrors } from "../lib/profile.mjs";
import { runBatch } from "../lib/state-machine.mjs";
import { HARNESS_DIR } from "../lib/util.mjs";
import { deps, fakeFs, fakePage, fakeServer } from "./fakes.mjs";

const OBSERVER = readFileSync(join(HARNESS_DIR, "observer-init.js"), "utf8");
const FIXTURE = join(HARNESS_DIR, "fixtures", "tiny-two-article");
const tmps = [];
const tmp = () => {
  const d = mkdtempSync(join(tmpdir(), "dbo-r3-"));
  tmps.push(d);
  return d;
};
afterEach(() => {
  while (tmps.length) rmSync(tmps.pop(), { recursive: true, force: true });
});

/** One fake browser document sharing `storage` (sessionStorage survives the reload). */
function fakeDocument(storage, { items = 0, ack }) {
  const raf = [];
  const calls = [];
  const state = {
    items: Array.from({ length: items }, () => ({ textContent: "", getAttribute: () => null })),
  };
  const win = {
    sessionStorage: {
      getItem: (k) => (storage.has(k) ? storage.get(k) : null),
      setItem: (k, v) => storage.set(k, String(v)),
      removeItem: (k) => storage.delete(k),
    },
    requestAnimationFrame: (cb) => raf.push(cb),
    getComputedStyle: () => ({ getPropertyValue: () => "" }),
    crypto: { randomUUID: () => `doc-${Math.random().toString(16).slice(2)}` },
    location: { href: "http://x/" },
    scrollX: 0,
    scrollY: 0,
    performance: { getEntriesByType: () => [{ type: "reload" }], now: () => 1 },
    MutationObserver: class {
      observe() {}
    },
    document: {
      documentElement: {},
      activeElement: null,
      querySelectorAll: (sel) => (sel === "[data-home-item]" ? state.items : []),
    },
    __dboDocInit: () => Promise.resolve(true),
    __dboPing: () => Promise.resolve(true),
    __dboNotify: (p) => {
      calls.push(p);
      return ack();
    },
  };
  win.window = win;
  win.top = win;
  win.globalThis = win;
  runInNewContext(OBSERVER, win);
  const frames = async (n) => {
    for (let i = 0; i < n; i += 1) {
      for (const cb of raf.splice(0)) cb();
      await new Promise((r) => setImmediate(r));
    }
  };
  return { win, calls, state, frames };
}

describe("observer delivery is persisted only on the controller's ack (run-006 root cause candidate)", () => {
  const predicate = { kind: "count", selector: "[data-home-item]", equals: 2 };
  const armedRecord = (storage) => JSON.parse(storage.get("__dbo_expect_v1"));

  it("a document replaced mid-delivery leaves the expectation armed, and the next document re-delivers", async () => {
    const storage = new Map();
    const doc1 = fakeDocument(storage, { items: 0, ack: () => new Promise(() => {}) });
    expect(doc1.win.__dbo.arm("tok-1", predicate)).toMatchObject({ armed: true });
    doc1.state.items.push({}, {});
    await doc1.frames(6);
    expect(doc1.calls.map((c) => c.token)).toEqual(["tok-1"]);
    expect(armedRecord(storage).delivered).toBe(false);
    await doc1.frames(6);
    expect(doc1.calls).toHaveLength(1);

    const doc2 = fakeDocument(storage, { items: 2, ack: () => Promise.resolve(true) });
    await doc2.frames(6);
    expect(doc2.calls.map((c) => c.token)).toEqual(["tok-1"]);
    expect(armedRecord(storage).delivered).toBe(true);
    await doc2.frames(6);
    expect(doc2.calls).toHaveLength(1);
  });

  it("a rejected binding call is retried in the same document", async () => {
    const storage = new Map();
    let n = 0;
    const doc = fakeDocument(storage, {
      items: 2,
      ack: () => (n++ === 0 ? Promise.reject(new Error("detached")) : Promise.resolve(true)),
    });
    expect(
      doc.win.__dbo.arm("tok-2", { kind: "count", selector: "[data-home-item]", equals: 3 }),
    ).toMatchObject({ armed: true });
    doc.state.items.push({});
    await doc.frames(12);
    expect(doc.calls.length).toBe(2);
    expect(armedRecord(storage).delivered).toBe(true);
  });
});

describe("watch tokens (burst) are recorded across documents", () => {
  it("records intermediate tokens seen before the final predicate, persisted through a reload", async () => {
    const storage = new Map();
    const doc1 = fakeDocument(storage, { items: 0, ack: () => Promise.resolve(true) });
    doc1.state.items.push({ textContent: "Title t1" });
    expect(
      doc1.win.__dbo.arm("t3", { kind: "count", selector: "[data-home-item]", equals: 3 }, [
        { selector: "[data-home-item]", tokens: ["t1", "t2"] },
      ]),
    ).toMatchObject({ armed: true });
    await doc1.frames(3);
    const doc2 = fakeDocument(storage, { items: 0, ack: () => Promise.resolve(true) });
    doc2.state.items.push({ textContent: "Title t2" }, {}, {});
    await doc2.frames(6);
    expect(doc2.calls).toHaveLength(1);
    expect(Object.keys(doc2.calls[0].watchSeen).sort()).toEqual(["t1", "t2"]);
  });
});

describe("ObserverPage controller side", () => {
  it("acks every notification and records duplicates of a resolved token without calling them late", async () => {
    const p = new ObserverPage(null, "", () => 5n);
    const late = [];
    const wait = p.waitNotify("tok", 1000, (x) => late.push(x));
    expect(p.onNotify({ token: "tok", docUUID: "d" }, 7n)).toBe(true);
    expect(await wait).toMatchObject({ kind: "notify", t1: 7n });
    expect(p.onNotify({ token: "tok", docUUID: "d2" }, 9n)).toBe(true);
    expect(p.duplicates.map((d) => d.docUUID)).toEqual(["d2"]);
    expect(late).toEqual([]);
  });

  it("settle waits for quiet after the restore's own native update", async () => {
    let now = 0n;
    const p = new ObserverPage(null, "", () => now);
    const mark = p.mark();
    const run = p.settle({ mark, startNs: 0n, quietMs: 100, boundMs: 5000, stepMs: 1 });
    const tick = setInterval(() => {
      now += 10_000_000n;
      if (now === 50_000_000n) p.wsFrames.push({ ns: String(now), event: "page" });
      if (now === 60_000_000n) p.navigations.push({ ns: String(now), url: "x" });
    }, 1);
    const r = await run;
    clearInterval(tick);
    expect(r).toMatchObject({
      settled: true,
      reason: "native-update-observed-then-quiet",
      nativeUpdateObserved: true,
      livereloadEvents: ["page"],
    });
    expect(r.waitedMs).toBeGreaterThanOrEqual(160);
  });

  it("settle reports bound-exceeded when frames keep coming", async () => {
    let now = 0n;
    const p = new ObserverPage(null, "", () => now);
    const mark = p.mark();
    const run = p.settle({ mark, startNs: 0n, quietMs: 100, boundMs: 300, stepMs: 1 });
    const tick = setInterval(() => {
      now += 10_000_000n;
      p.wsFrames.push({ ns: String(now), event: "css" });
    }, 1);
    const r = await run;
    clearInterval(tick);
    expect(r).toMatchObject({ settled: false, reason: "bound-exceeded" });
  });

  function stubbed({ prepareErrors = [], resultingUrl, preconditionTrue = true }) {
    const p = new ObserverPage(null, "", () => 1n);
    p.baseUrl = "http://127.0.0.1:1";
    const calls = [];
    p.prepare = async (path) => {
      calls.push(`prepare:${path}`);
      const e = prepareErrors.shift();
      if (e) throw new Error(e);
      return { docUUID: "doc-target", httpStatus: 200 };
    };
    p.awaitResultingDocument = async () => {
      calls.push("await-resulting");
      return { ok: true, url: resultingUrl, state: { docUUID: "doc-resulting" } };
    };
    p.arm = async () =>
      preconditionTrue
        ? { armed: false, precondition: "true-before-write", actual: 2 }
        : { armed: true, actual: 0 };
    p.waitNotify = async () => ({ kind: "timeout" });
    p.disarm = async () => null;
    p.failureSnapshot = async () => ({ url: "x", bodyExcerpt: "partial" });
    return { p, calls };
  }

  it("an aborted live-check navigation uses the resulting document when it is the target", async () => {
    const { p, calls } = stubbed({
      prepareErrors: ["page.goto: net::ERR_ABORTED at /articles/b"],
      resultingUrl: "http://127.0.0.1:1/articles/b",
    });
    const r = await p.navigateAndAwait(
      "/articles/b",
      { kind: "count", selector: "x", equals: 1 },
      100,
      "restore",
    );
    expect(calls).toEqual(["prepare:/articles/b", "await-resulting"]);
    expect(r).toMatchObject({
      ok: true,
      docUUID: "doc-resulting",
      recovery: [{ usedResultingDocument: true }],
    });
  });

  it("an aborted navigation to another page re-navigates once (never a reload) and records it", async () => {
    const { p, calls } = stubbed({
      prepareErrors: ["page.goto: net::ERR_ABORTED"],
      resultingUrl: "http://127.0.0.1:1/articles/a",
    });
    const r = await p.navigateAndAwait(
      "/articles/b",
      { kind: "count", selector: "x", equals: 1 },
      100,
      "restore",
    );
    expect(calls).toEqual(["prepare:/articles/b", "await-resulting", "prepare:/articles/b"]);
    expect(r).toMatchObject({ ok: true, recovery: [{ usedResultingDocument: false }] });
  });

  it("is bounded: a second abort fails the check, and a non-abort error is not retried", async () => {
    const twice = stubbed({
      prepareErrors: ["net::ERR_ABORTED", "net::ERR_ABORTED"],
      resultingUrl: "http://127.0.0.1:1/x",
    });
    const r = await twice.p.navigateAndAwait("/articles/b", {}, 100, "restore");
    expect(r.ok).toBe(false);
    expect(r.recovery).toHaveLength(1);
    const other = stubbed({ prepareErrors: ["net::ERR_CONNECTION_REFUSED"] });
    const r2 = await other.p.navigateAndAwait("/articles/b", {}, 100, "restore");
    expect(other.calls).toEqual(["prepare:/articles/b"]);
    expect(r2.ok).toBe(false);
  });

  it("an arm interrupted by a framework reload re-navigates and re-arms once, recorded", async () => {
    const { p, calls } = stubbed({ resultingUrl: "http://127.0.0.1:1/" });
    const realArm = p.arm;
    let arms = 0;
    p.arm = async (...a) => {
      calls.push("arm");
      arms += 1;
      if (arms === 1)
        throw new Error(
          "page.evaluate: Execution context was destroyed, most likely because of a navigation",
        );
      return realArm(...a);
    };
    const r = await p.navigateAndAwait("/", { kind: "count", selector: "x", equals: 1 }, 100, "r");
    expect(calls).toEqual(["prepare:/", "arm", "await-resulting", "prepare:/", "arm"]);
    expect(r).toMatchObject({
      ok: true,
      recovery: [{ armInterruptedBy: expect.stringMatching(/Execution context was destroyed/) }],
    });
  });

  it("a second arm interruption or a non-navigation arm error is not retried", async () => {
    const destroyed = "page.evaluate: Execution context was destroyed";
    const twice = stubbed({ resultingUrl: "http://127.0.0.1:1/" });
    twice.p.arm = async () => {
      twice.calls.push("arm");
      throw new Error(destroyed);
    };
    await expect(twice.p.navigateAndAwait("/", {}, 100, "r")).rejects.toThrow(destroyed);
    expect(twice.calls).toEqual(["prepare:/", "arm", "await-resulting", "prepare:/", "arm"]);
    const other = stubbed({});
    other.p.arm = async () => {
      other.calls.push("arm");
      throw new Error("observer missing");
    };
    await expect(other.p.navigateAndAwait("/", {}, 100, "r")).rejects.toThrow("observer missing");
    expect(other.calls).toEqual(["prepare:/", "arm"]);
  });

  it("a failed wait records the first-read value and a read-only failure snapshot", async () => {
    const { p } = stubbed({ preconditionTrue: false });
    const r = await p.navigateAndAwait(
      "/",
      { kind: "count", selector: "[data-home-item]", equals: 2 },
      10,
      "restore",
    );
    expect(r).toMatchObject({
      ok: false,
      firstReadMatched: false,
      firstReadActual: 0,
      failure: { bodyExcerpt: "partial" },
    });
  });
});

const negScenario = (inject, op = { op: "edit-title", article: "a" }) => [
  {
    id: `neg-${inject}`,
    set: "negative-controls",
    ops: [
      { id: "x", writeMethod: "in-place", inject, ...op },
      { id: "never", op: "edit-body", article: "b", writeMethod: "in-place" },
    ],
  },
];

describe("cleanup settles before live checks", () => {
  it("calls settle from the restore time and records it", async () => {
    const page = fakePage({ behaviors: [{ kind: "timeout" }] });
    const d = deps({ page });
    await runBatch({
      scenarios: [{ id: "s", ops: [{ id: "a", op: "edit-title", article: "a" }] }],
      deps: d,
    });
    expect(page.settles).toHaveLength(1);
    expect(page.settles[0]).toMatchObject({ quietMs: 5, boundMs: 50 });
    expect(d.recorder.records.find((r) => r.type === "cleanup").settle).toMatchObject({
      settled: true,
    });
  });
});

describe("negative-control injections (state machine)", () => {
  it("precondition-true: precondition-failed, no write, equal target hashes, control pass", async () => {
    const fs = fakeFs();
    const d = deps({ page: fakePage({ preconditionTrue: (p) => p.selector === "html" }), fs });
    const { accounting } = await runBatch({ scenarios: negScenario("precondition-true"), deps: d });
    expect(fs.writes).toEqual([]);
    const rec = d.recorder.records.find((r) => r.type === "attempt");
    expect(rec).toMatchObject({
      outcome: "precondition-failed",
      writePerformed: false,
      controlPass: true,
      inject: { kind: "precondition-true" },
    });
    expect(rec.targetSha256Before).toBe(rec.targetSha256After);
    expect(accounting).toMatchObject({
      stoppedByExpectedControl: true,
      expectedErrors: 1,
      error: 1,
      notAttempted: 1,
    });
  });

  it("write-error: the write targets a missing path and fails before any DOM wait", async () => {
    const fs = fakeFs();
    let waited = 0;
    const d = deps({ page: fakePage({ onWait: () => (waited += 1) && null }), fs });
    const { accounting } = await runBatch({ scenarios: negScenario("write-error"), deps: d });
    expect(waited).toBe(0);
    const rec = d.recorder.records.find((r) => r.type === "attempt");
    expect(rec).toMatchObject({
      outcome: "write-error",
      controlPass: true,
      inject: { injectedFrom: "a.md" },
    });
    expect(rec.write.path).toBe(".a.md.missing");
    expect(accounting).toMatchObject({
      stoppedByExpectedControl: true,
      expectedErrors: 1,
      notAttempted: 1,
    });
  });

  it("kill-server-during-wait: server-exit with signalCode SIGKILL and no restart", async () => {
    const server = fakeServer();
    const d = deps({ page: fakePage({ behaviors: [{ kind: "never" }] }), server });
    d.timeouts.attemptMs = 10_000;
    const { accounting } = await runBatch({
      scenarios: negScenario("kill-server-during-wait"),
      deps: d,
    });
    const rec = d.recorder.records.find((r) => r.type === "attempt");
    expect(rec).toMatchObject({
      outcome: "server-exit",
      exit: { exitCode: null, signalCode: "SIGKILL" },
      controlPass: true,
      injectKill: { ok: true },
    });
    expect(accounting).toMatchObject({
      stopReason: "server-exit",
      stoppedByExpectedControl: true,
      notAttempted: 1,
    });
    expect(d.recorder.records.find((r) => r.type === "cleanup")).toMatchObject({
      liveSkipped: "server-not-alive",
    });
  });

  it("restore-blocked: restoration fails, the mode is reverted in a recorded final step, and the batch stops", async () => {
    const fs = fakeFs();
    const d = deps({ page: fakePage({ behaviors: [{ kind: "notify", t1: 2_000_000_000n }] }), fs });
    const scen = [
      {
        id: "neg",
        set: "negative-controls",
        ops: [
          {
            id: "x",
            op: "edit-title",
            article: "a",
            writeMethod: "in-place",
            inject: "restore-blocked",
          },
        ],
      },
      { id: "next", ops: [{ id: "n", op: "edit-body", article: "b", writeMethod: "in-place" }] },
    ];
    const { accounting } = await runBatch({ scenarios: scen, deps: d });
    const types = d.recorder.records.map((r) =>
      r.type === "inject" ? `inject:${r.step}` : r.type,
    );
    expect(types.indexOf("inject:apply")).toBeLessThan(types.indexOf("cleanup"));
    expect(types.indexOf("inject:revert-mode")).toBeGreaterThan(types.indexOf("cleanup"));
    expect(d.recorder.records.find((r) => r.type === "cleanup")).toMatchObject({
      ok: false,
      controlPass: true,
      liveSkipped: "hash-mismatch",
    });
    expect(accounting).toMatchObject({
      restorationFailures: 1,
      stopReason: "restoration-failed",
      stoppedByExpectedControl: true,
      expectedErrors: 1,
      notAttempted: 1,
    });
    expect(fs.blocked.size).toBe(0);
  });

  it("inject is rejected outside negative-controls scenarios", () => {
    const p = JSON.parse(
      readFileSync(join(HARNESS_DIR, "profiles", "neg-write-error.example.json"), "utf8"),
    );
    expect(semanticErrors(p)).toEqual([]);
    p.scenarios[0].set = "workload";
    expect(semanticErrors(p)[0]).toMatch(/only allowed in a scenario with set "negative-controls"/);
  });
});

describe("restore-blocked on the real filesystem", () => {
  it("a read-only target makes the in-place restoration fail, and the mode reverts", () => {
    const m = materialize({
      fixtureDir: FIXTURE,
      workDir: join(tmp(), "w"),
      placement: "internal",
      adapter: "A0",
    });
    const snap = snapshotSources(m);
    const before = hashOf(m.articles.a);
    performWrite(
      prepareAttempt(m, { op: "edit-title", article: "a", writeMethod: "in-place" }).write,
      () => 0n,
    );
    const block = blockWrite(m.articles.a);
    const r = restoreSnapshot(snap, { writeMethod: "in-place" });
    expect(r.actions[0].error).toMatch(/EACCES|permission/i);
    expect(r.mismatches).toHaveLength(1);
    expect(unblockWrite(block).modeRestored).toBe(block.modeBefore);
    expect(restoreSnapshot(snap).mismatches).toEqual([]);
    expect(hashOf(m.articles.a)).toBe(before);
  });
});
