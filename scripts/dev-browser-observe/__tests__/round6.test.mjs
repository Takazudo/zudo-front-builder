import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { main } from "../cli.mjs";
import { nativeReloadSurvival } from "../lib/calibrate.mjs";
import { EXIT } from "../lib/run.mjs";
import { batchExitOk, runBatch } from "../lib/state-machine.mjs";
import { ObserverPage } from "../lib/browser.mjs";
import { deps, fakeFs, fakePage, fakeServer } from "./fakes.mjs";

const neg = (inject, op = { op: "edit-title", article: "a", writeMethod: "in-place" }) => [
  {
    id: `neg-${inject}`,
    set: "negative-controls",
    ops: [
      { id: "x", inject, ...op },
      { id: "never", op: "edit-body", article: "b", writeMethod: "in-place" },
    ],
  },
];
const attempts = (d) => d.recorder.records.filter((r) => r.type === "attempt");

describe("#1 a declared injection that did not fire stops the batch, non-ok", () => {
  it("kill refused (birth mismatch) while the notify wins", async () => {
    const server = fakeServer();
    server.injectKill = () => ({ ok: false, reason: "birth-identity-mismatch-or-gone" });
    const d = deps({
      page: fakePage({ behaviors: [{ kind: "notify", t1: 2_000_000_000n }] }),
      server,
    });
    const { accounting, stopped } = await runBatch({
      scenarios: neg("kill-server-during-wait"),
      deps: d,
    });
    expect(attempts(d).map((r) => r.opId)).toEqual(["x"]);
    expect(attempts(d)[0]).toMatchObject({ outcome: "success", controlPass: false });
    expect(d.recorder.records.find((r) => r.type === "control-not-fired")).toMatchObject({
      inject: "kill-server-during-wait",
      detail: expect.stringContaining("kill refused"),
    });
    expect(accounting).toMatchObject({
      stopReason: "negative-control-not-fired",
      controlsNotFired: 1,
      notAttempted: 1,
      stoppedByExpectedControl: false,
    });
    expect(batchExitOk(accounting, stopped)).toBe(false);
  });

  it("restore-blocked whose block did not take (blockWrite threw) restores fine and is not credited", async () => {
    const fs = fakeFs();
    fs.blockWrite = () => {
      throw new Error("EPERM: running as root");
    };
    const d = deps({ page: fakePage({ behaviors: [{ kind: "notify", t1: 2_000_000_000n }] }), fs });
    const { accounting, stopped } = await runBatch({ scenarios: neg("restore-blocked"), deps: d });
    expect(fs.files["a.md"]).toBe("v0");
    expect(accounting).toMatchObject({
      stopReason: "negative-control-not-fired",
      controlsNotFired: 1,
      notAttempted: 1,
      restorationFailures: 0,
    });
    expect(batchExitOk(accounting, stopped)).toBe(false);
  });

  it("precondition-true whose predicate unexpectedly armed is not fired, even on success", async () => {
    const d = deps({
      page: fakePage({
        preconditionTrue: () => false,
        behaviors: [{ kind: "notify", t1: 2_000_000_000n }],
      }),
    });
    const { accounting, stopped } = await runBatch({
      scenarios: neg("precondition-true"),
      deps: d,
    });
    expect(accounting).toMatchObject({ stopReason: "negative-control-not-fired", notAttempted: 1 });
    expect(batchExitOk(accounting, stopped)).toBe(false);
  });

  it("write-error whose write unexpectedly succeeded is not fired", async () => {
    const fs = fakeFs();
    fs.missingSibling = (p) => p;
    const d = deps({ page: fakePage({ behaviors: [{ kind: "notify", t1: 2_000_000_000n }] }), fs });
    const { accounting, stopped } = await runBatch({ scenarios: neg("write-error"), deps: d });
    expect(accounting.stopReason).toBe("negative-control-not-fired");
    expect(batchExitOk(accounting, stopped)).toBe(false);
  });
});

describe("#3 the blocked restore is retried once unblocked", () => {
  it("a retry that also fails is recorded and counted, and the batch stays stopped and non-ok", async () => {
    const fs = fakeFs({ failRestore: true });
    const d = deps({ page: fakePage({ behaviors: [{ kind: "notify", t1: 2_000_000_000n }] }), fs });
    const { accounting, stopped } = await runBatch({ scenarios: neg("restore-blocked"), deps: d });
    const cleanups = d.recorder.records.filter((r) => r.type === "cleanup");
    expect(cleanups.map((c) => [c.retry, c.ok])).toEqual([
      [false, false],
      [true, false],
    ]);
    expect(accounting).toMatchObject({
      restorationFailures: 2,
      expectedErrors: 1,
      stoppedByExpectedControl: true,
    });
    expect(batchExitOk(accounting, stopped)).toBe(false);
  });
});

describe("#4 a throwing arm is an attempt with outcome arm-error", () => {
  it("records arm-error, not controller-error", async () => {
    const page = fakePage({});
    page.arm = async () => {
      throw new Error("Execution context was destroyed");
    };
    const d = deps({ page });
    const { accounting, stopped } = await runBatch({
      scenarios: [
        { id: "s", ops: [{ id: "a", op: "edit-title", article: "a", writeMethod: "in-place" }] },
      ],
      deps: d,
    });
    expect(attempts(d)[0]).toMatchObject({
      outcome: "arm-error",
      writePerformed: false,
      arm: { threw: true },
    });
    expect(d.recorder.records.some((r) => r.type === "controller-error")).toBe(false);
    expect(accounting).toMatchObject({ stopReason: "arm-error", error: 1 });
    expect(batchExitOk(accounting, stopped)).toBe(false);
  });
});

describe("#5 native-reload-survival keeps going when a live check throws", () => {
  it("records the thrown live check as a failed check", async () => {
    const o = new ObserverPage(null, "", () => 1_000_000n);
    o.prepare = async () => ({ docUUID: "doc-1" });
    o.arm = async () => ({ armed: true, docUUID: "doc-1" });
    o.disarm = async () => null;
    o.quietWindow = async (ms) => ({ quietMs: ms });
    o.waitNotify = async () => ({
      kind: "notify",
      payload: { docUUID: "doc-2", navigationType: "reload" },
      t1: 3_000_000n,
    });
    o.liveCheck = async () => {
      throw new Error("page.goto: net::ERR_ABORTED");
    };
    const fixtureOps = {
      snapshot: () => ({ files: {}, dirs: [] }),
      prepareAttempt: () => ({
        token: "t",
        predicate: {},
        write: { method: "in-place", path: "b.md" },
      }),
      performWrite: (w, onT0) => onT0(),
      restore: () => ({ writeMethod: "in-place", actions: [], mismatches: [] }),
      liveChecks: () => [{ path: "/articles/b", predicate: {} }],
      route: () => "/articles/b",
    };
    const res = await nativeReloadSurvival({
      observer: o,
      fixtureOps,
      timeoutMs: 10,
      restoreMs: 10,
      quietMs: 1,
    });
    expect(res.restorationOk).toBe(false);
    expect(res.restoration.live[0]).toMatchObject({
      ok: false,
      error: expect.stringContaining("ERR_ABORTED"),
    });
  });
});

describe("#6 hold-lock --ms needs a finite number", () => {
  it.each([[["--ms"]], [["--ms", "abc"]], [["--ms", "-5"]]])(
    "rejects %j with a usage exit",
    async (ms) => {
      expect(await main(["hold-lock", "--profile", join("nowhere", "p.json"), ...ms])).toBe(
        EXIT.usage,
      );
    },
  );
});
