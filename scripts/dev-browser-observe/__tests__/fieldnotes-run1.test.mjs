import { describe, expect, it } from "vitest";
import { ObserverPage } from "../lib/browser.mjs";
import { liveRestoreChecks } from "../lib/fieldnotes.mjs";
import { createModel } from "../lib/fieldnotes-model.mjs";
import { runBatch, settleRule } from "../lib/state-machine.mjs";
import { deps, fakeFs, fakePage } from "./fakes.mjs";
import { SPEC, articleRow } from "./fieldnotes-fake.mjs";

describe("BUG 1: cleanup settle waits for the restore's own native update", () => {
  it("settleRule: writes need the native update unless every write is under a no-auto-update root", () => {
    expect(settleRule([])).toEqual({ requireNativeUpdate: false, rule: "quiet-only:no-writes" });
    expect(
      settleRule(
        [{ path: "/w/apps/zfb/components/Layout.tsx", method: "in-place" }],
        ["/w/apps/zfb/public"],
      ),
    ).toEqual({ requireNativeUpdate: true, rule: "require-native-update" });
    expect(
      settleRule(
        [{ path: "/w/apps/zfb/public/site.css", method: "in-place" }],
        ["/w/apps/zfb/public"],
      ),
    ).toEqual({ requireNativeUpdate: false, rule: "quiet-only:no-auto-update-contract" });
    expect(
      settleRule([{ path: "/w/apps/zfb/public-x/a", method: "in-place" }], ["/w/apps/zfb/public"])
        .requireNativeUpdate,
    ).toBe(true);
    expect(settleRule([{ path: "/w/a.md", method: "in-place", error: "EACCES" }]).rule).toBe(
      "quiet-only:no-writes",
    );
    expect(
      settleRule([{ path: "/w/b.md", from: "/w/a.md", method: "rename" }], []).requireNativeUpdate,
    ).toBe(true);
  });

  it("with requireNativeUpdate, quiet alone never settles; the bound is reported as without-native-update", async () => {
    let now = 0n;
    const p = new ObserverPage(null, "", () => now);
    const tick = setInterval(() => (now += 10_000_000n), 1);
    const r = await p.settle({
      mark: p.mark(),
      startNs: 0n,
      quietMs: 50,
      boundMs: 300,
      requireNativeUpdate: true,
      rule: "require-native-update",
      stepMs: 1,
    });
    clearInterval(tick);
    expect(r).toMatchObject({
      settled: false,
      rule: "require-native-update",
      reason: "bound-exceeded-without-native-update",
      nativeUpdateObserved: false,
    });
  });

  it("with requireNativeUpdate, a page frame (or new document) then quiet settles", async () => {
    let now = 0n;
    const p = new ObserverPage(null, "", () => now);
    const mark = p.mark();
    const tick = setInterval(() => {
      now += 10_000_000n;
      if (now === 200_000_000n) p.docInits.push({ ns: String(now), docUUID: "d2" });
    }, 1);
    const r = await p.settle({
      mark,
      startNs: 0n,
      quietMs: 50,
      boundMs: 5000,
      requireNativeUpdate: true,
      stepMs: 1,
    });
    clearInterval(tick);
    expect(r).toMatchObject({
      settled: true,
      reason: "native-update-observed-then-quiet",
      nativeUpdateObserved: true,
    });
    expect(r.waitedMs).toBeGreaterThanOrEqual(250);
  });

  it("the state machine passes the rule from the restore's actions and the fixture's no-auto-update roots", async () => {
    const page = fakePage({ behaviors: [{ kind: "notify", t1: 2_000_000_000n }] });
    const fs = fakeFs();
    const restore = fs.restore;
    fs.restore = (snap) => ({
      ...restore(snap),
      actions: [{ path: "/w/apps/zfb/components/Layout.tsx", method: "in-place" }],
    });
    fs.noAutoUpdatePaths = ["/w/apps/zfb/public"];
    const d = deps({ page, fs });
    await runBatch({
      scenarios: [
        { id: "s", ops: [{ id: "a", op: "edit-title", article: "a", writeMethod: "in-place" }] },
      ],
      deps: d,
    });
    expect(page.settles[0]).toMatchObject({
      requireNativeUpdate: true,
      rule: "require-native-update",
    });
  });

  it("a positive check never accepts 'page not in cache' as presence", async () => {
    const p = new ObserverPage(null, "", () => 1n);
    p.baseUrl = "http://127.0.0.1:1";
    p.prepare = async () => ({ docUUID: "d", httpStatus: 404 });
    p.arm = async () => ({ armed: false, precondition: "true-before-write", actual: 0 });
    p.failureSnapshot = async () => ({ bodyExcerpt: "404 — page not in cache", notInCache: true });
    expect(
      await p.navigateAndAwait("/articles/x/", { kind: "absent", selector: "h1" }, 10, "restore"),
    ).toMatchObject({ ok: false, transient: "transient-not-in-cache" });
  });
});

describe("BUG 2: the cleanup gone check for a removed created file", () => {
  it("liveRestoreChecks emits a gone check (no predicate) and liveCheck routes it to goneCheck", async () => {
    const model = createModel(
      { articles: Array.from({ length: 20 }, (_, i) => articleRow(i + 1)) },
      SPEC,
    );
    const gone = liveRestoreChecks(model, ["article-00021"]).find(
      (c) => c.path === "/articles/article-00021/",
    );
    expect(gone).toEqual({
      kind: "gone",
      path: "/articles/article-00021/",
      absentText: "article-00021",
    });
    const p = new ObserverPage(null, "", () => 1n);
    p.observePage = async (path) => ({
      ok: true,
      path,
      httpStatus: 404,
      bodyExcerpt: "Not found",
      containsMarker: false,
      notInCache: false,
    });
    p.navigateAndAwait = async () => {
      throw new Error("navigateAndAwait must not receive a gone check");
    };
    expect(await p.liveCheck(gone, 10, { quietMs: 1 })).toMatchObject({
      ok: true,
      kind: "gone",
      httpStatus: 404,
      basis: "stable-reads",
    });
  });
});
