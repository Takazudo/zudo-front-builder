import { describe, expect, it } from "vitest";
import { classifyMechanism, runBatch } from "../lib/state-machine.mjs";
import { deps, fakeFs, fakePage, fakeServer } from "./fakes.mjs";

const three = [
  {
    id: "s1",
    ops: [
      { id: "o1", op: "edit-title", article: "a" },
      { id: "o2", op: "edit-body", article: "a" },
      { id: "o3", op: "edit-category", article: "b" },
    ],
  },
];

const attempts = (r) => r.filter((x) => x.type === "attempt");

describe("runBatch fail-stop", () => {
  it("interrupts stalled preparation and records unverified live restoration at the wall reserve", async () => {
    const controller = new AbortController();
    const page = fakePage();
    page.prepare = () => {
      queueMicrotask(() => controller.abort(new Error("session wall cleanup reserve reached")));
      return new Promise(() => {});
    };
    const d = deps({ page });
    const result = await runBatch({ scenarios: three, deps: { ...d, signal: controller.signal } });
    expect(result.stopped).toBe(true);
    expect(result.accounting).toMatchObject({
      attempted: 0,
      notAttempted: 3,
      restorationFailures: 1,
    });
    expect(d.recorder.records.find((r) => r.type === "cleanup")).toMatchObject({
      liveSkipped: "session-deadline",
      ok: false,
    });
  });

  it("counts successes and classifies a native full reload from evidence", async () => {
    const page = fakePage({
      behaviors: [
        {
          kind: "notify",
          docUUID: "doc-2",
          evidence: { mainFrameNavigations: 1, newDocuments: 1, livereloadEvents: ["page"] },
          t1: 2_000_000_000n,
        },
        { kind: "notify", docUUID: "doc-1", t1: 2_100_000_000n },
        {
          kind: "notify",
          docUUID: "doc-9",
          evidence: { mainFrameNavigations: 0, newDocuments: 1 },
          t1: 2_200_000_000n,
        },
      ],
    });
    const d = deps({ page });
    const { accounting, stopped } = await runBatch({ scenarios: three, deps: d });
    expect(stopped).toBe(false);
    expect(accounting).toMatchObject({
      planned: 3,
      attempted: 3,
      success: 3,
      notAttempted: 0,
      error: 0,
      lastArmedUnknownOperation: null,
    });
    const recs = attempts(d.recorder.records);
    expect(recs.map((r) => r.mechanism)).toEqual([
      "native-full-reload",
      "in-document-update",
      "unclassified",
    ]);
    for (const r of recs) {
      expect(r.endpoint).toBe("predicate confirmed + 2 rAF + binding delivery");
      expect(r.latencyMs).toBeGreaterThan(0);
    }
    expect(d.recorder.records.filter((r) => r.type === "cleanup")).toHaveLength(1);
  });

  it("stops after a DOM timeout even though the write completed; remainder is not-attempted", async () => {
    const page = fakePage({
      behaviors: [{ kind: "notify", t1: 2_000_000_000n }, { kind: "timeout" }],
    });
    const fs = fakeFs();
    const d = deps({ page, fs });
    const { accounting, stopped } = await runBatch({ scenarios: three, deps: d });
    expect(stopped).toBe(true);
    expect(accounting).toMatchObject({
      planned: 3,
      attempted: 2,
      success: 1,
      timeout: 1,
      notAttempted: 1,
      stopReason: "timeout",
    });
    expect(accounting.lastArmedUnknownOperation).toMatchObject({ op: "o2", writeCompleted: true });
    expect(fs.writes).toEqual(["a.md", "a.md"]);
    const t = attempts(d.recorder.records)[1];
    expect(t).toMatchObject({
      outcome: "timeout",
      observerDisarmed: true,
      nativeWorkCancelled: false,
    });
    const order = d.recorder.records.map((r) => r.type);
    expect(order.indexOf("cleanup")).toBeGreaterThan(order.lastIndexOf("attempt"));
    expect(d.recorder.records.find((r) => r.type === "cleanup")).toMatchObject({
      label: "cleanup",
      measured: false,
    });
    expect(fs.files["a.md"]).toBe("v0");
  });

  it("classifies a reload without the predicate as stale and stops", async () => {
    const page = fakePage({
      behaviors: [{ kind: "timeout", evidence: { mainFrameNavigations: 1, newDocuments: 1 } }],
    });
    const d = deps({ page });
    const { accounting } = await runBatch({ scenarios: three, deps: d });
    expect(accounting).toMatchObject({
      stale: 1,
      attempted: 1,
      notAttempted: 2,
      stopReason: "stale",
    });
  });

  it("records late tokens without counting them as success", async () => {
    const page = fakePage({
      behaviors: [{ kind: "timeout", late: [{ token: "tok-old", docUUID: "doc-1" }] }],
    });
    const d = deps({ page });
    const { accounting } = await runBatch({ scenarios: three, deps: d });
    expect(accounting.success).toBe(0);
    expect(accounting.lateTokens).toBe(1);
    const late = d.recorder.records.find((r) => r.type === "late-token");
    expect(late).toMatchObject({
      label: "not-success",
      armedToken: "tok-o1",
      payload: { token: "tok-old" },
    });
  });

  it("stops on a precondition that is already true before the write, with no write", async () => {
    const page = fakePage({ preconditionTrue: () => true });
    const fs = fakeFs();
    const d = deps({ page, fs });
    const { accounting } = await runBatch({ scenarios: three, deps: d });
    expect(accounting).toMatchObject({
      attempted: 1,
      error: 1,
      notAttempted: 2,
      stopReason: "precondition-failed",
    });
    expect(fs.writes).toEqual([]);
    expect(attempts(d.recorder.records)[0]).toMatchObject({
      outcome: "precondition-failed",
      writePerformed: false,
    });
  });

  it("stops on a write error and reports the armed op as unknown", async () => {
    const page = fakePage({});
    const d = deps({ page, fs: fakeFs({ failWrite: true }) });
    const { accounting } = await runBatch({ scenarios: three, deps: d });
    expect(accounting).toMatchObject({
      error: 1,
      attempted: 1,
      notAttempted: 2,
      stopReason: "write-error",
    });
    expect(accounting.lastArmedUnknownOperation).toMatchObject({
      op: "o1",
      writeCompleted: "unknown",
    });
    expect(page.disarms).toBeGreaterThan(0);
  });

  it("stops when the server exits with a code during an attempt", async () => {
    const server = fakeServer();
    const page = fakePage({
      onWait: () => {
        setTimeout(() => server.die({ exitCode: 101, signalCode: null }), 5);
        return new Promise(() => {});
      },
    });
    const d = deps({ page, server });
    const { accounting } = await runBatch({ scenarios: three, deps: d });
    expect(accounting).toMatchObject({ stopReason: "server-exit", error: 1, notAttempted: 2 });
    expect(attempts(d.recorder.records)[0].exit).toEqual({ exitCode: 101, signalCode: null });
    expect(d.recorder.records.find((r) => r.type === "cleanup")).toMatchObject({
      liveSkipped: "server-not-alive",
    });
  });

  it("refuses to start an attempt when the server is already gone", async () => {
    const server = fakeServer();
    server.die({ exitCode: null, signalCode: "SIGKILL" });
    const fs = fakeFs();
    const d = deps({ page: fakePage({}), server, fs });
    const { accounting } = await runBatch({ scenarios: three, deps: d });
    expect(accounting).toMatchObject({ attempted: 0, notAttempted: 3, stopReason: "server-exit" });
    expect(fs.writes).toEqual([]);
  });

  it("trips the RSS ceiling before the next workload edit", async () => {
    const server = fakeServer();
    server.rss = 999_999;
    const fs = fakeFs();
    const d = deps({ page: fakePage({}), server, fs, limits: { rssCeilingKiB: 1000 } });
    const { accounting } = await runBatch({ scenarios: three, deps: d });
    expect(accounting).toMatchObject({ stopReason: "resource-limit", attempted: 0 });
    expect(fs.writes).toEqual([]);
  });

  it("stops on a supplemental failure after a successful timed endpoint", async () => {
    const scen = [
      {
        id: "s",
        ops: [
          {
            id: "t",
            op: "edit-title",
            article: "a",
            supplementalChecks: [{ path: "/", predicate: {} }],
          },
          { id: "u", op: "edit-body", article: "a" },
        ],
      },
    ];
    const page = fakePage({
      behaviors: [{ kind: "notify", t1: 2_000_000_000n }],
      supplemental: () => ({ ok: false, firstReadMatched: false }),
    });
    const d = deps({ page });
    const { accounting } = await runBatch({ scenarios: scen, deps: d });
    expect(accounting).toMatchObject({
      success: 1,
      supplementalFailures: 1,
      stopReason: "supplemental-failed",
      notAttempted: 1,
    });
    expect(d.recorder.records.find((r) => r.type === "supplemental")).toMatchObject({
      label: "supplemental",
      measured: false,
    });
  });

  it("stops on a restoration failure and never treats cleanup as a trial", async () => {
    const scen = [
      { id: "s1", ops: [{ id: "a", op: "edit-title", article: "a" }] },
      { id: "s2", ops: [{ id: "b", op: "edit-title", article: "b" }] },
    ];
    const page = fakePage({ behaviors: [{ kind: "notify", t1: 2_000_000_000n }] });
    const d = deps({ page, fs: fakeFs({ failRestore: true }) });
    const { accounting } = await runBatch({ scenarios: scen, deps: d });
    expect(accounting).toMatchObject({
      success: 1,
      restorationFailures: 1,
      stopReason: "restoration-failed",
      notAttempted: 1,
    });
    expect(d.recorder.records.find((r) => r.type === "cleanup")).toMatchObject({
      ok: false,
      liveSkipped: "hash-mismatch",
    });
  });

  it("restoration failure after a timeout is reported on top of the stop reason", async () => {
    const d = deps({
      page: fakePage({ behaviors: [{ kind: "timeout" }] }),
      fs: fakeFs({ failRestore: true }),
    });
    const { accounting } = await runBatch({ scenarios: three, deps: d });
    expect(accounting.stopReason).toBe("timeout+restoration-failed");
  });

  it("an expected negative-control timeout is a control pass and still stops", async () => {
    const scen = [
      {
        id: "neg",
        ops: [
          { id: "stale", op: "stale-predicate", article: "a", expect: "timeout" },
          { id: "next", op: "edit-title", article: "b" },
        ],
      },
    ];
    const fs = fakeFs();
    const d = deps({ page: fakePage({ behaviors: [{ kind: "timeout" }] }), fs });
    const { accounting, stopped } = await runBatch({ scenarios: scen, deps: d });
    expect(stopped).toBe(true);
    expect(accounting).toMatchObject({
      timeout: 1,
      expectedControlPasses: 1,
      stoppedByExpectedControl: true,
      notAttempted: 1,
    });
    expect(fs.writes).toEqual(["a.md"]);
    expect(attempts(d.recorder.records)[0]).toMatchObject({
      controlPass: true,
      outcome: "timeout",
    });
  });

  it("an unexpected success of a negative control stops and is not a control pass", async () => {
    const scen = [
      {
        id: "neg",
        ops: [
          { id: "stale", op: "stale-predicate", article: "a", expect: "timeout" },
          { id: "n", op: "edit-title", article: "b" },
        ],
      },
    ];
    const d = deps({ page: fakePage({ behaviors: [{ kind: "notify", t1: 2_000_000_000n }] }) });
    const { accounting } = await runBatch({ scenarios: scen, deps: d });
    expect(accounting).toMatchObject({
      stopReason: "negative-control-unexpected-success",
      expectedControlPasses: 0,
      notAttempted: 1,
    });
  });
});

describe("runBatch round-2 semantics", () => {
  it("a navigation between the precondition check and t0 is precondition-unstable, with no write", async () => {
    const page = fakePage({});
    page.unstable = { mainFrameNavigations: 1, newDocuments: 1, wsFrames: [{ event: "page" }] };
    const fs = fakeFs();
    const d = deps({ page, fs });
    const { accounting } = await runBatch({ scenarios: three, deps: d });
    expect(accounting).toMatchObject({
      stopReason: "precondition-unstable",
      attempted: 1,
      error: 1,
      notAttempted: 2,
    });
    expect(fs.writes).toEqual([]);
    expect(attempts(d.recorder.records)[0]).toMatchObject({
      outcome: "precondition-unstable",
      writePerformed: false,
      observerDisarmed: true,
    });
  });

  it("records the quiet window after each live-confirmed cleanup", async () => {
    const page = fakePage({
      behaviors: [
        { kind: "notify", t1: 2_000_000_000n },
        { kind: "notify", t1: 2_000_000_000n },
        { kind: "notify", t1: 2_000_000_000n },
      ],
    });
    const d = deps({ page });
    await runBatch({ scenarios: three, deps: d });
    const cleanup = d.recorder.records.find((r) => r.type === "cleanup");
    expect(cleanup.quiet).toMatchObject({ quietMs: 5 });
    expect(page.quietWindows).toEqual([5]);
  });

  it("a classify timeout is recorded as timeout, still stops, and flags stoppedByClassifyTimeout", async () => {
    const scen = [
      {
        id: "c",
        ops: [
          { id: "t", op: "edit-title", article: "a", expect: "classify" },
          { id: "n", op: "edit-body", article: "a" },
        ],
      },
    ];
    const d = deps({ page: fakePage({ behaviors: [{ kind: "timeout" }] }) });
    const { accounting, stopped } = await runBatch({ scenarios: scen, deps: d });
    expect(stopped).toBe(true);
    expect(accounting).toMatchObject({
      timeout: 1,
      success: 0,
      stoppedByClassifyTimeout: true,
      stoppedByExpectedControl: false,
      expectedControlPasses: 0,
      notAttempted: 1,
    });
    expect(attempts(d.recorder.records)[0]).toMatchObject({
      outcome: "timeout",
      expect: "classify",
    });
    expect(attempts(d.recorder.records)[0].controlPass).toBeUndefined();
  });

  it("a classify success is an ordinary success and the batch continues", async () => {
    const scen = [
      {
        id: "c",
        ops: [
          { id: "t", op: "edit-title", article: "a", expect: "classify" },
          { id: "n", op: "edit-body", article: "a" },
        ],
      },
    ];
    const d = deps({
      page: fakePage({
        behaviors: [
          { kind: "notify", t1: 2_000_000_000n },
          { kind: "notify", t1: 2_000_000_000n },
        ],
      }),
    });
    const { accounting, stopped } = await runBatch({ scenarios: scen, deps: d });
    expect(stopped).toBe(false);
    expect(accounting.success).toBe(2);
  });

  it("trips the RSS ceiling during the armed wait and stops", async () => {
    const server = fakeServer();
    server.sampleResourcesAsync = async () => ({ summedRssKiB: server.rss });
    const page = fakePage({
      onWait: () => {
        server.rss = 50_000;
        return new Promise(() => {});
      },
    });
    const d = deps({ page, server, limits: { rssCeilingKiB: 10_000, sampleEveryMs: 5 } });
    d.timeouts.attemptMs = 10_000;
    const { accounting } = await runBatch({ scenarios: three, deps: d });
    expect(accounting).toMatchObject({
      stopReason: "resource-limit",
      attempted: 1,
      notAttempted: 2,
    });
    const rec = attempts(d.recorder.records)[0];
    expect(rec).toMatchObject({ outcome: "resource-limit", resource: { tripped: "rss" } });
    expect(rec.waitSamples.count).toBeGreaterThan(0);
    expect(accounting.lastArmedUnknownOperation).toMatchObject({ writeCompleted: true });
  });

  it("records unmeasured warm navigations before the first attempt", async () => {
    const scen = [
      { id: "w", warmOrder: "index-first", ops: [{ id: "t", op: "edit-title", article: "a" }] },
    ];
    const page = fakePage({ behaviors: [{ kind: "notify", t1: 2_000_000_000n }] });
    const d = deps({ page });
    await runBatch({ scenarios: scen, deps: d });
    const warm = d.recorder.records.find((r) => r.type === "warm");
    expect(warm).toMatchObject({ label: "unmeasured", warmOrder: "index-first", ok: true });
    expect(warm.visits.map((v) => v.path)).toEqual(["/", "/articles/a"]);
    expect(page.prepared.slice(0, 3)).toEqual(["/", "/articles/a", "/articles/a"]);
  });
});

describe("classifyMechanism", () => {
  const base = { preDocUUID: "a", serverPidBefore: 1, serverPidAfter: 1 };
  it("uses server pid, document identity, navigations and livereload frames", () => {
    expect(
      classifyMechanism({
        ...base,
        notifyDocUUID: "a",
        mainFrameNavigations: 0,
        serverPidAfter: 2,
      }),
    ).toBe("server-restart");
    expect(
      classifyMechanism({
        ...base,
        notifyDocUUID: "b",
        mainFrameNavigations: 1,
        livereloadEvents: ["page"],
      }),
    ).toBe("native-full-reload");
    expect(
      classifyMechanism({
        ...base,
        notifyDocUUID: "b",
        mainFrameNavigations: 1,
        livereloadEvents: [],
      }),
    ).toBe("full-reload-without-page-frame");
    expect(
      classifyMechanism({
        ...base,
        notifyDocUUID: "a",
        mainFrameNavigations: 0,
        livereloadEvents: ["css"],
      }),
    ).toBe("css-replacement");
    expect(classifyMechanism({ ...base, notifyDocUUID: "a", mainFrameNavigations: 0 })).toBe(
      "in-document-update",
    );
    expect(classifyMechanism({ ...base, notifyDocUUID: "a", mainFrameNavigations: 1 })).toBe(
      "unclassified",
    );
  });
});
