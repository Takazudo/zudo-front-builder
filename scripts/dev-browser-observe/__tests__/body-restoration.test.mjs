import { afterEach, describe, expect, it, vi } from "vitest";
import { ObserverPage } from "../lib/browser.mjs";
import { runBatch } from "../lib/state-machine.mjs";
import { deps, fakeFs, fakePage } from "./fakes.mjs";

afterEach(() => vi.useRealTimers());

describe("Field Notes body restoration", () => {
  it("waits past the interrupted edit's reload until the open document serves the original body", async () => {
    vi.useFakeTimers();
    const clock = () => BigInt(Date.now()) * 1_000_000n;
    const p = new ObserverPage(null, "", clock);
    p.baseUrl = "http://127.0.0.1:9999";
    const path = "/articles/a/";
    const url = p.url(path);
    const mark = p.mark();
    let docUUID = "initial";
    let liveBody = "original";
    p.page = {
      evaluate: vi.fn(async (_fn, predicate) => ({
        url,
        docUUID,
        loading: false,
        matched: liveBody === predicate.equals,
      })),
    };
    // The edit captured its old input. Restoring disk while it is in flight does not change it.
    let disk = "interrupted-edit";
    const editInput = disk;
    disk = "original";
    const restoreInput = disk;
    const publish = (id, text) => {
      docUUID = id;
      liveBody = text;
      p.wsFrames.push({ ns: String(clock()), event: "page" });
      p.docInits.push({ ns: String(clock()), docUUID, url });
    };
    const bodyRevision = {
      path,
      predicate: { kind: "text", selector: "[data-body]", equals: "original" },
    };
    const options = {
      mark,
      startNs: clock(),
      bodyRevision,
      quietMs: 3,
      stepMs: 1,
      boundMs: 30,
      requireNativeUpdate: true,
    };
    const pending = p.settle(options);
    let finished = false;
    pending.then(() => {
      finished = true;
    });
    publish("unfinished-edit", editInput);
    await vi.advanceTimersByTimeAsync(8);
    expect(p.page.evaluate).toHaveBeenCalledTimes(1);
    expect({ disk, liveBody, finished }).toEqual({
      disk: "original",
      liveBody: "interrupted-edit",
      finished: false,
    });
    publish("restore", restoreInput);
    await vi.advanceTimersByTimeAsync(4);
    expect(await pending).toMatchObject({
      settled: true,
      bodyRevision: { docUUID: "restore", matched: true },
    });
  });

  it("fails restoration if a generic native update settles without a matching revision, even when later live checks pass", async () => {
    const page = fakePage({
      behaviors: [{ kind: "notify", t1: 2_000_000_000n }, { kind: "timeout" }],
    });
    const fs = fakeFs();
    const applied = fs.restore;
    fs.restore = (snapshot) => ({ ...applied(snapshot), actions: [{ path: "a.md" }] });
    const arm = page.arm;
    page.arm = async (...args) => ({ ...(await arm(...args)), actual: fs.files["a.md"] });
    page.settle = async ({ bodyRevision }) => {
      expect(fs.files["a.md"]).toBe("v0");
      expect(bodyRevision.predicate.equals).toBe("v0");
      return { settled: true, nativeUpdateObserved: true, bodyRevision: { matched: false } };
    };
    const d = { ...deps({ page, fs }), verifyBodyRevision: true };
    const { accounting } = await runBatch({
      deps: d,
      scenarios: [
        {
          id: "body",
          ops: [
            { id: "a1", op: "edit-body", article: "a" },
            { id: "a2", op: "edit-body", article: "a" },
          ],
        },
      ],
    });
    expect(accounting).toMatchObject({
      attempted: 2,
      success: 1,
      timeout: 1,
      restorationFailures: 1,
      stopReason: "timeout+restoration-failed",
    });
    expect(d.recorder.records.find((r) => r.type === "cleanup")).toMatchObject({
      ok: false,
      mismatches: [],
      live: [{ ok: true, predicate: { kind: "all", of: [{}, { kind: "text", equals: "v0" }] } }],
    });
  });

  it("rejects an unmet native settle requirement even when all live checks later pass", async () => {
    const fs = fakeFs();
    const restore = fs.restore;
    fs.restore = (snap) => ({ ...restore(snap), actions: [{ path: "a.md" }] });
    const page = fakePage({ behaviors: [{ kind: "notify", t1: 2_000_000_000n }] });
    page.settle = async () => ({ settled: false, reason: "bound-exceeded-without-native-update" });
    const d = deps({ fs, page });
    const { accounting } = await runBatch({
      deps: d,
      scenarios: [{ id: "title", ops: [{ op: "edit-title", article: "a" }] }],
    });
    expect(accounting).toMatchObject({
      success: 1,
      restorationFailures: 1,
      stopReason: "restoration-failed",
    });
    expect(d.recorder.records.find((r) => r.type === "cleanup")).toMatchObject({
      ok: false,
      mismatches: [],
      live: [{ ok: true }],
    });
  });

  it("a stalled disarm releases the controller and cannot clear a newer token when it executes", async () => {
    vi.useFakeTimers();
    const p = new ObserverPage(null, "");
    let delayed;
    p.armedToken = "old";
    p.page = {
      evaluate: (fn, input) =>
        new Promise((resolve) => {
          delayed = () => resolve(fn(input));
        }),
    };
    const old = p.disarm();
    await vi.advanceTimersByTimeAsync(500);
    expect(await old).toMatchObject({ error: "observation timed out" });
    const disarm = vi.fn();
    vi.stubGlobal("window", { __dbo: { state: () => ({ armed: { token: "new" } }), disarm } });
    try {
      delayed();
      expect(disarm).not.toHaveBeenCalled();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("keeps a navigation timeout final when its diagnostic snapshot remains pending", async () => {
    vi.useFakeTimers();
    const p = new ObserverPage(null, "");
    p.prepare = async () => {
      throw new Error("page.goto: Timeout 30000ms exceeded.");
    };
    p.failureSnapshot = async () => new Promise(() => {});
    const pending = p.navigateAndAwait("/", {}, 900000, "restore");
    await vi.advanceTimersByTimeAsync(500);
    expect(await pending).toMatchObject({
      ok: false,
      stage: "navigation",
      error: "page.goto: Timeout 30000ms exceeded.",
      failure: { error: "observation timed out", timeoutMs: 500 },
    });
  });
});

it("rejects a fresh tiny article-a document still rendering the last edit after disk restoration", async () => {
  vi.useFakeTimers();
  const clock = () => BigInt(Date.now()) * 1_000_000n;
  const observer = new ObserverPage(null, "", clock);
  observer.baseUrl = "http://127.0.0.1:9999";
  const path = "/articles/a";
  const url = observer.url(path);
  const mark = observer.mark();
  const original = "Body marker body-a-v0 for article a.";
  const stale = "Body marker twelfth-edit for article a.";
  observer.page = {
    evaluate: async (_fn, predicate) => ({
      docUUID: "after-restoration",
      url,
      loading: false,
      matched: stale === predicate.equals,
    }),
  };
  const startNs = clock();
  const waiting = observer.settle({
    mark,
    startNs,
    requireNativeUpdate: true,
    boundMs: 10,
    quietMs: 2,
    stepMs: 1,
    bodyRevision: {
      path,
      predicate: { kind: "text", selector: '[data-article-body="a"]', equals: original },
    },
  });
  observer.docInits.push({ ns: String(clock()), docUUID: "after-restoration", url });
  await vi.advanceTimersByTimeAsync(10);
  expect(await waiting).toMatchObject({
    settled: false,
    nativeUpdateObserved: true,
    reason: "bound-exceeded-without-restored-body",
    bodyRevision: { docUUID: "after-restoration", matched: false },
  });
});
