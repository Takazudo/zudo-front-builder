import { describe, expect, it } from "vitest";
import { ObserverPage, PREPARE_INTERRUPTED, prepareInterruption } from "../lib/browser.mjs";
import { nativeReloadSurvival, runCalibration } from "../lib/calibrate.mjs";

const now = () => process.hrtime.bigint();

/**
 * An observer whose page gets the restore's native reload `lateMs` after each restoration
 * write. A prepare that starts while that reload is still in flight fails the way Playwright
 * does ("Execution context was destroyed"), which is what the native calibration run hit.
 */
function lateReloadObserver({ lateMs }) {
  const o = new ObserverPage(null, "", now);
  const log = [];
  let pending = 0;
  let docN = 1;
  o.reloadAfterRestore = () => {
    pending += 1;
    setTimeout(() => {
      const ns = String(now());
      o.wsFrames.push({ ns, event: "page" });
      o.navigations.push({ ns, url: "/articles/b" });
      docN += 1;
      o.docInits.push({ ns, docUUID: `doc-${docN}`, navigationType: "reload" });
      log.push("restore-reload");
      pending -= 1;
    }, lateMs);
  };
  o.prepare = async (path) => {
    log.push("prepare");
    if (pending > 0)
      throw prepareInterruption(
        new Error(
          "page.evaluate: Execution context was destroyed, most likely because of a navigation",
        ),
        path,
      );
    return { docUUID: `doc-${docN}` };
  };
  o.arm = async () => ({ armed: true, docUUID: `doc-${docN}` });
  o.disarm = async () => null;
  o.liveCheck = async () => ({ ok: true });
  o.quietWindow = async (ms) => ({ quietMs: ms });
  o.waitNotify = async () => {
    const ns = String(now());
    o.wsFrames.push({ ns, event: "page" });
    o.navigations.push({ ns, url: "/articles/b" });
    docN += 1;
    o.docInits.push({ ns, docUUID: `doc-${docN}`, navigationType: "reload" });
    return {
      kind: "notify",
      payload: { docUUID: `doc-${docN}`, navigationType: "reload" },
      t1: now(),
    };
  };
  return { o, log };
}

function fixtureOps(observer) {
  return {
    snapshot: () => ({ files: {}, dirs: [] }),
    prepareAttempt: () => ({
      token: "t",
      predicate: {},
      write: { method: "in-place", path: "/w/content/b.md" },
    }),
    performWrite: (_w, onT0) => onT0(),
    restore: () => {
      observer.reloadAfterRestore();
      return {
        writeMethod: "in-place",
        actions: [{ path: "/w/content/b.md", method: "in-place" }],
        mismatches: [],
      };
    },
    liveChecks: () => [{ path: "/articles/b", predicate: {} }],
    route: () => "/articles/b",
  };
}

describe("native-reload-survival waits for the restore's own reload before the next cycle", () => {
  it("the cleanup settle absorbs a late restore reload, so the next prepare is not interrupted", async () => {
    const { o, log } = lateReloadObserver({ lateMs: 60 });
    const records = [];
    const results = await runCalibration({
      observer: o,
      append: (r) => records.push(r),
      cells: ["native-reload-survival"],
      repeats: 3,
      timeoutMs: 100,
      staleTimeoutMs: 10,
      pagePath: "/independent",
      fixtureOps: fixtureOps(o),
      restoreMs: 2000,
      quietMs: 20,
    });
    expect(results["native-reload-survival"]).toMatchObject({ repeats: 3, failures: 0 });
    expect(results.aborted).toBeUndefined();
    expect(log).toEqual([
      "prepare",
      "restore-reload",
      "prepare",
      "restore-reload",
      "prepare",
      "restore-reload",
    ]);
    for (const r of records) {
      expect(r.ok).toBe(true);
      expect(r.restoration.settle).toMatchObject({
        settled: true,
        rule: "require-native-update",
        nativeUpdateObserved: true,
      });
    }
  });

  it("without the reload arriving inside the settle bound, the cycle still records the settle as not settled", async () => {
    const { o } = lateReloadObserver({ lateMs: 400 });
    const res = await nativeReloadSurvival({
      observer: o,
      fixtureOps: fixtureOps(o),
      timeoutMs: 100,
      restoreMs: 80,
      quietMs: 20,
    });
    expect(res.restoration.settle).toMatchObject({
      settled: false,
      reason: "bound-exceeded-without-native-update",
    });
    await new Promise((r) => setTimeout(r, 420));
  });

  it("a navigation that interrupts prepare fails the cell with a classified outcome, not a controller error", async () => {
    const { o, log } = lateReloadObserver({ lateMs: 300 });
    const ops = fixtureOps(o);
    // A bound shorter than the late reload: the settle gives up, and the next prepare is hit.
    const records = [];
    const results = await runCalibration({
      observer: o,
      append: (r) => records.push(r),
      cells: ["native-reload-survival"],
      repeats: 3,
      timeoutMs: 100,
      staleTimeoutMs: 10,
      pagePath: "/independent",
      fixtureOps: ops,
      restoreMs: 50,
      quietMs: 10,
    });
    expect(results["native-reload-survival"]).toMatchObject({
      failures: 1,
      aborted: "prepare-interrupted-by-navigation",
    });
    expect(results.aborted).toEqual({
      cell: "native-reload-survival",
      i: 1,
      reason: "prepare-interrupted-by-navigation",
    });
    expect(records.map((r) => r.outcome)).toEqual(["notify", "prepare-interrupted-by-navigation"]);
    expect(records[1]).toMatchObject({
      ok: false,
      writePerformed: false,
      prepareError: expect.stringMatching(
        /interrupted by a navigation: .*Execution context was destroyed/,
      ),
    });
    expect(log.filter((x) => x === "prepare")).toHaveLength(2);
    await new Promise((r) => setTimeout(r, 320));
  });
});

describe("prepareInterruption", () => {
  it("codes Playwright's navigation errors and passes any other error through", () => {
    const nav = prepareInterruption(
      new Error(
        "page.evaluate: Execution context was destroyed, most likely because of a navigation",
      ),
      "/a",
    );
    expect(nav).toMatchObject({ code: PREPARE_INTERRUPTED, path: "/a" });
    expect(prepareInterruption(new Error("frame was detached"), "/a").code).toBe(
      PREPARE_INTERRUPTED,
    );
    const other = new Error("ReferenceError: __dbo is not defined");
    expect(prepareInterruption(other, "/a")).toBe(other);
  });
});
