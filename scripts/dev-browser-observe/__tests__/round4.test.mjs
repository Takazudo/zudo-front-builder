import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { ObserverPage, closeOwnedBrowser, launchOwnedBrowser } from "../lib/browser.mjs";
import { sample } from "../lib/mac-process-adapter.mjs";
import { pgidSurvivors } from "../lib/process-owner.mjs";
import { semanticErrors } from "../lib/profile.mjs";
import { batchExitOk, newAccounting, runBatch } from "../lib/state-machine.mjs";
import { HARNESS_DIR } from "../lib/util.mjs";
import { deps, fakeFs, fakePage, fakeServer } from "./fakes.mjs";

const one = [
  {
    id: "s",
    ops: [
      { id: "a", op: "edit-title", article: "a", writeMethod: "in-place" },
      { id: "b", op: "edit-body", article: "a", writeMethod: "in-place" },
    ],
  },
];
const attempt = (d) => d.recorder.records.find((r) => r.type === "attempt");

describe("#1 early delivery and t1 <= t0 never count as success", () => {
  it("a delivery for the armed token before the write is precondition-unstable with no write", async () => {
    const page = fakePage({});
    page.hasEarly = (token) => token === "tok-a";
    const fs = fakeFs();
    const d = deps({ page, fs });
    const { accounting } = await runBatch({ scenarios: one, deps: d });
    expect(fs.writes).toEqual([]);
    expect(attempt(d)).toMatchObject({
      outcome: "precondition-unstable",
      unstable: { reason: "notified-before-write" },
      writePerformed: false,
    });
    expect(accounting).toMatchObject({ success: 0, stopReason: "precondition-unstable" });
  });

  it("a notify whose t1 is not after t0 is rejected as precondition-unstable", async () => {
    const d = deps({ page: fakePage({ behaviors: [{ kind: "notify", t1: 1n }] }) });
    const { accounting } = await runBatch({ scenarios: one, deps: d });
    const rec = attempt(d);
    expect(rec).toMatchObject({
      outcome: "precondition-unstable",
      unstable: { reason: "notify-not-after-t0" },
    });
    expect(rec.latencyMs).toBeUndefined();
    expect(accounting).toMatchObject({
      success: 0,
      notAttempted: 1,
      lastArmedUnknownOperation: { writeCompleted: true },
    });
  });

  it("ObserverPage reports an early delivery for the armed token", () => {
    const p = new ObserverPage(null, "", () => 1n);
    p.armedToken = "tok";
    p.onNotify({ token: "tok", docUUID: "d" }, 2n);
    expect(p.hasEarly("tok")).toBe(true);
  });
});

describe("#2 a throwing page call still cleans up and ends the batch", () => {
  it("records controller-error, restores the edited file and writes batch-end", async () => {
    const page = fakePage({
      onWait: () => {
        throw new Error("Target page, context or browser has been closed");
      },
    });
    const fs = fakeFs();
    const d = deps({ page, fs });
    const { accounting, stopped } = await runBatch({ scenarios: one, deps: d });
    const types = d.recorder.records.map((r) => r.type);
    expect(types).toEqual(expect.arrayContaining(["controller-error", "cleanup", "batch-end"]));
    expect(types.indexOf("cleanup")).toBeGreaterThan(types.indexOf("controller-error"));
    expect(fs.files["a.md"]).toBe("v0");
    expect(stopped).toBe(true);
    expect(accounting).toMatchObject({
      stopReason: "controller-error",
      error: 1,
      notAttempted: 1,
      lastArmedUnknownOperation: { op: "a", writeCompleted: true },
    });
    expect(d.recorder.records.find((r) => r.type === "controller-error")).toMatchObject({
      outcome: "controller-error",
      writePerformed: true,
      armedToken: "tok-a",
    });
  });

  it("exposes the live accounting through accountingRef", async () => {
    const ref = { current: null };
    const d = {
      ...deps({ page: fakePage({ behaviors: [{ kind: "timeout" }] }) }),
      accountingRef: ref,
    };
    const { accounting } = await runBatch({ scenarios: one, deps: d });
    expect(ref.current).toBe(accounting);
  });
});

describe("#3 restore-blocked is credited only when nothing else stopped the batch", () => {
  it("restore-blocked stops right after the injected op: the next op is never reached (round 6 #2)", async () => {
    const scen = [
      {
        id: "neg",
        set: "negative-controls",
        ops: [
          {
            id: "blk",
            op: "edit-title",
            article: "a",
            writeMethod: "in-place",
            inject: "restore-blocked",
          },
          { id: "never", op: "edit-body", article: "a", writeMethod: "in-place" },
        ],
      },
    ];
    const fs = fakeFs();
    const d = deps({ page: fakePage({ behaviors: [{ kind: "notify", t1: 2_000_000_000n }] }), fs });
    const { accounting, stopped } = await runBatch({ scenarios: scen, deps: d });
    expect(d.recorder.records.filter((r) => r.type === "attempt").map((r) => r.opId)).toEqual([
      "blk",
    ]);
    const cleanups = d.recorder.records.filter((r) => r.type === "cleanup");
    expect(cleanups[0]).toMatchObject({
      reason: "inject:restore-blocked",
      ok: false,
      controlPass: true,
      retry: false,
    });
    expect(cleanups[1]).toMatchObject({
      reason: "inject:restore-blocked:retry-unblocked",
      ok: true,
      retry: true,
    });
    expect(fs.files["a.md"]).toBe("v0");
    expect(accounting).toMatchObject({
      notAttempted: 1,
      restorationFailures: 1,
      stopReason: "restoration-failed",
      stoppedByExpectedControl: true,
      expectedErrors: 1,
    });
    expect(batchExitOk(accounting, stopped)).toBe(true);
  });

  it("batchExitOk counts undeclared timeouts/stale and errors", () => {
    const acc = newAccounting(2);
    expect(batchExitOk(acc, false)).toBe(true);
    expect(
      batchExitOk(
        { ...acc, stoppedByExpectedControl: true, timeout: 1, expectedTimeouts: 1 },
        true,
      ),
    ).toBe(true);
    expect(
      batchExitOk(
        { ...acc, stoppedByExpectedControl: true, timeout: 1, expectedTimeouts: 0 },
        true,
      ),
    ).toBe(false);
    expect(
      batchExitOk({ ...acc, stoppedByExpectedControl: true, error: 2, expectedErrors: 1 }, true),
    ).toBe(false);
    expect(
      batchExitOk({ ...acc, stoppedByClassifyTimeout: true, stale: 1, expectedTimeouts: 1 }, true),
    ).toBe(true);
    expect(batchExitOk({ ...acc, timeout: 1 }, true)).toBe(false);
  });

  it("declared controls still exit ok", async () => {
    const neg = [
      {
        id: "n",
        ops: [
          {
            id: "s",
            op: "stale-predicate",
            article: "a",
            writeMethod: "in-place",
            expect: "timeout",
          },
        ],
      },
    ];
    const r1 = await runBatch({
      scenarios: neg,
      deps: deps({ page: fakePage({ behaviors: [{ kind: "timeout" }] }) }),
    });
    expect(batchExitOk(r1.accounting, r1.stopped)).toBe(true);
    const cls = [
      {
        id: "c",
        ops: [
          { id: "t", op: "edit-title", article: "a", writeMethod: "in-place", expect: "classify" },
        ],
      },
    ];
    const r2 = await runBatch({
      scenarios: cls,
      deps: deps({ page: fakePage({ behaviors: [{ kind: "timeout" }] }) }),
    });
    expect(batchExitOk(r2.accounting, r2.stopped)).toBe(true);
    const blk = [
      {
        id: "b",
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
    ];
    const r3 = await runBatch({
      scenarios: blk,
      deps: deps({ page: fakePage({ behaviors: [{ kind: "notify", t1: 2_000_000_000n }] }) }),
    });
    expect(batchExitOk(r3.accounting, r3.stopped)).toBe(true);
  });
});

describe("#4 a measured wait never outlives its attempt", () => {
  it("disarm cancels the pending waiter, so a later notification is not routed to it", async () => {
    const p = new ObserverPage(null, "", () => 1n);
    p.page = { evaluate: async () => null };
    const late = [];
    const stray = [];
    p.onStray = (x) => stray.push(x);
    const pending = p.waitNotify("old", 60_000, (x) => late.push(x));
    await p.disarm();
    expect(await pending).toEqual({ kind: "cancelled" });
    expect(p.waiter).toBeNull();
    const next = p.waitNotify("restore-1", 1000, (x) => late.push(x));
    p.onNotify({ token: "restore-1", docUUID: "d" }, 5n);
    expect(await next).toMatchObject({ kind: "notify" });
    p.onNotify({ token: "stray", docUUID: "d" }, 6n);
    expect(late).toEqual([]);
    expect(stray.map((x) => x.token)).toEqual(["stray"]);
  });

  it("the state machine disarms after a server-exit wins the race", async () => {
    const server = fakeServer();
    const page = fakePage({
      onWait: () => {
        setTimeout(() => server.die({ exitCode: 1, signalCode: null }), 1);
        return new Promise(() => {});
      },
    });
    const d = deps({ page, server });
    await runBatch({ scenarios: one, deps: d });
    expect(page.disarms).toBeGreaterThan(0);
  });
});

describe("#5 pgid scan after the root died", () => {
  const root = { pid: 100, pgid: 100, lstart: "Sat Nov 21 06:00:00 2026", comm: "zfb" };
  it("finds group members born no earlier than the root", () => {
    const table = [
      { pid: 101, ppid: 1, pgid: 100, lstart: "Sat Nov 21 06:00:01 2026", comm: "child" },
      { pid: 102, ppid: 1, pgid: 100, lstart: "Sat Nov 21 05:59:59 2026", comm: "older" },
      { pid: 103, ppid: 1, pgid: 999, lstart: "Sat Nov 21 06:00:02 2026", comm: "other" },
    ];
    expect(pgidSurvivors(table, root)).toEqual({ reused: false, found: [table[0]] });
  });

  it("trusts nothing when the pgid number was reused by a new leader", () => {
    const table = [
      { pid: 100, ppid: 1, pgid: 100, lstart: "Sat Nov 21 07:00:00 2026", comm: "new-leader" },
      { pid: 101, ppid: 100, pgid: 100, lstart: "Sat Nov 21 07:00:01 2026", comm: "child" },
    ];
    expect(pgidSurvivors(table, root)).toEqual({ reused: true, found: [] });
  });
});

describe("#6 injection profile constraints", () => {
  const neg = (name) => JSON.parse(readFileSync(join(HARNESS_DIR, "profiles", name), "utf8"));
  it("restore-blocked requires an in-place restoration", () => {
    const p = neg("neg-restore-blocked.example.json");
    expect(semanticErrors(p)).toEqual([]);
    p.restoration.writeMethod = "atomic-rename";
    expect(semanticErrors(p)[0]).toMatch(
      /restore-blocked" requires restoration.writeMethod "in-place"/,
    );
  });

  it("write-error requires in-place or atomic-rename", () => {
    const p = neg("neg-write-error.example.json");
    p.scenarios[0].ops[0] = {
      id: "c",
      op: "create-article",
      article: "c",
      writeMethod: "create",
      inject: "write-error",
    };
    expect(semanticErrors(p)[0]).toMatch(
      /write-error" requires writeMethod "in-place" or "atomic-rename"/,
    );
    p.scenarios[0].ops[0] = {
      id: "r",
      op: "edit-body",
      article: "a",
      writeMethod: "atomic-rename",
      inject: "write-error",
    };
    expect(semanticErrors(p)).toEqual([]);
  });
});

describe("#7 Chromium never leaks", () => {
  function fakeChromium({ failContext = false } = {}) {
    const state = { closed: 0 };
    const browser = {
      version: () => "1",
      close: async () => {
        state.closed += 1;
      },
      newContext: async () => {
        if (failContext) throw new Error("newContext failed");
        return { close: async () => {} };
      },
    };
    return {
      state,
      chromium: { executablePath: () => join(HARNESS_DIR, "cli.mjs"), launch: async () => browser },
    };
  }

  it("closes the browser when anything after launch throws, and rethrows", async () => {
    const a = fakeChromium({ failContext: true });
    await expect(launchOwnedBrowser({}, { chromiumImpl: a.chromium })).rejects.toThrow(
      /newContext failed/,
    );
    expect(a.state.closed).toBe(1);
    const b = fakeChromium();
    const failingObserver = () => ({
      install: async () => {
        throw new Error("install failed");
      },
    });
    await expect(
      launchOwnedBrowser({}, { chromiumImpl: b.chromium, observerFactory: failingObserver }),
    ).rejects.toThrow(/install failed/);
    expect(b.state.closed).toBe(1);
  });

  it("closes the browser even when context.close throws", async () => {
    let closed = false;
    const errors = await closeOwnedBrowser({
      context: {
        close: async () => {
          throw new Error("ctx boom");
        },
      },
      browser: {
        close: async () => {
          closed = true;
        },
      },
    });
    expect(closed).toBe(true);
    expect(errors).toEqual([{ step: "context.close", error: "ctx boom" }]);
  });
});

describe("#8 sync sample keeps live rows when ps -p exits non-zero", () => {
  it("parses stdout from the failed ps invocation", () => {
    const runImpl = (cmd, args) => {
      if (args[0] === "-M") return "USER PID\nx 1\n";
      throw Object.assign(new Error("Command failed: ps"), {
        status: 1,
        stdout: "  4242  1000  2000\n",
      });
    };
    const s = sample([4242, 999_999], { runImpl });
    expect(s.processes[0]).toMatchObject({ pid: 4242, present: true, rssKiB: 1000 });
    expect(s.processes[1]).toMatchObject({ pid: 999_999, present: false });
    expect(s.summedRssKiB).toBe(1000);
  });
});
