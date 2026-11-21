import { describe, expect, it } from "vitest";
import { ObserverPage } from "../lib/browser.mjs";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EMPTY_HOME_PROBE, materializeFieldNotes, planFieldNotesOp } from "../lib/fieldnotes.mjs";
import { HARNESS_DIR } from "../lib/util.mjs";
import { makeFakeSource } from "./fieldnotes-fake.mjs";
import { classifyNegative, classifyNegativeState } from "../lib/special-modes.mjs";

const NOT_IN_CACHE = {
  ok: true,
  httpStatus: 404,
  title: "zfb dev — 404",
  bodyExcerpt: "404 — page not in cache\n\nThe dev server has no rendered HTML for this URL.",
  notInCache: true,
  containsMarker: false,
};

function gonePage(views) {
  const p = new ObserverPage(null, "", () => 1n);
  const queue = [...views];
  p.observePage = async (path) => ({ path, ...(queue.length > 1 ? queue.shift() : queue[0]) });
  return p;
}

describe("gone semantics: zfb dev's steady 404 for a removed URL is the not-in-cache body", () => {
  const check = { kind: "gone", path: "/articles/article-00042/", absentText: "Title 42" };

  it("passes on one read when the preceding settle observed the write's native update", async () => {
    const r = await gonePage([NOT_IN_CACHE]).goneCheck(check, {
      quietMs: 1,
      nativeUpdateConfirmed: true,
    });
    expect(r).toMatchObject({
      ok: true,
      basis: "settle-native-update",
      stableReads: 1,
      httpStatus: 404,
    });
  });

  it("otherwise passes only when a second read after quietMs returns the same 404 without content", async () => {
    const r = await gonePage([NOT_IN_CACHE, NOT_IN_CACHE]).goneCheck(check, { quietMs: 1 });
    expect(r).toMatchObject({ ok: true, basis: "stable-reads", stableReads: 2 });
    expect(r.reads).toHaveLength(2);
  });

  it("fails when the second read shows the page again, when content is still rendered, or on a 200 error page", async () => {
    const back = {
      ok: true,
      httpStatus: 200,
      title: "Title 42",
      bodyExcerpt: "Title 42",
      notInCache: false,
      containsMarker: true,
    };
    expect((await gonePage([NOT_IN_CACHE, back]).goneCheck(check, { quietMs: 1 })).ok).toBe(false);
    expect(
      (
        await gonePage([{ ...NOT_IN_CACHE, containsMarker: true }]).goneCheck(check, {
          quietMs: 1,
          nativeUpdateConfirmed: true,
        })
      ).ok,
    ).toBe(false);
    expect(
      (
        await gonePage([
          {
            ok: true,
            httpStatus: 200,
            title: "Error",
            bodyExcerpt: "render threw",
            containsMarker: false,
          },
        ]).goneCheck(check, { quietMs: 1, nativeUpdateConfirmed: true })
      ).ok,
    ).toBe(false);
  });
});

describe("presence checks wait out a transient not-in-cache by navigation-only re-reads", () => {
  function presencePage(states) {
    const p = new ObserverPage(null, "", () => 1n);
    p.baseUrl = "http://127.0.0.1:1";
    const queue = [...states];
    const calls = [];
    p.prepare = async (path) => {
      calls.push(path);
      return queue.length > 1 ? queue.shift() : queue[0];
    };
    p.arm = async () => ({ armed: false, precondition: "true-before-write", actual: "ok" });
    p.failureSnapshot = async () => ({ bodyExcerpt: "Title", notInCache: false });
    return { p, calls };
  }

  it("re-reads until the rebuild lands, recording each transient read", async () => {
    let t = 0n;
    const { p, calls } = presencePage([
      { docUUID: "a", httpStatus: 404, notInCache: true },
      { docUUID: "b", httpStatus: 404, notInCache: true },
      { docUUID: "c", httpStatus: 200, notInCache: false },
    ]);
    p.clock = () => (t += 1_000_000n);
    const r = await p.navigateAndAwait(
      "/articles/x/",
      { kind: "text", selector: "h1", includes: "x" },
      5000,
      "restore",
    );
    expect(r).toMatchObject({ ok: true, docUUID: "c" });
    expect(r.transientReads).toHaveLength(2);
    expect(calls).toEqual(["/articles/x/", "/articles/x/", "/articles/x/"]);
  });

  it("fails with transient evidence when it never resolves within the check timeout", async () => {
    let t = 0n;
    const { p } = presencePage([{ docUUID: "a", httpStatus: 404, notInCache: true }]);
    p.clock = () => (t += 400_000_000n);
    const r = await p.navigateAndAwait(
      "/articles/x/",
      { kind: "text", selector: "h1", includes: "x" },
      1000,
      "restore",
    );
    expect(r).toMatchObject({ ok: false, transient: "transient-not-in-cache" });
    expect(r.transientReads.length).toBeGreaterThan(0);
  });
});

describe("expected-negative classification with basis", () => {
  it("route-not-expanded for zfb's 404 not-in-cache on the renamed article's own URL (fn02 rename-filename-only)", () => {
    expect(
      classifyNegative(NOT_IN_CACHE, { lastGood: { matched: false, reason: "no-element" } }),
    ).toEqual({ state: "route-not-expanded", basis: "http-404 page not in cache" });
  });

  it("last-good for '/' still serving the original listing (what fn02 actually saw at the browser)", () => {
    expect(
      classifyNegative(
        { ok: true, httpStatus: 200, title: "Home", bodyExcerpt: "100 articles" },
        { lastGood: { matched: true }, empty: { matched: false } },
      ),
    ).toMatchObject({ state: "last-good" });
  });

  it("empty-collection for a 200 home whose empty probe matches (ZB024 content snapshot failure)", () => {
    const c = classifyNegative(
      { ok: true, httpStatus: 200, bodyExcerpt: "0 articles in the collection" },
      { lastGood: { matched: false }, empty: { matched: true, actual: ["0", 0] } },
    );
    expect(c.state).toBe("empty-collection");
    expect(c.basis).toContain("empty probe matched");
  });

  it("error by 5xx or marker, and the old string helper still works", () => {
    expect(classifyNegative({ ok: true, httpStatus: 500, bodyExcerpt: "" }).state).toBe("error");
    expect(
      classifyNegative(
        { ok: true, httpStatus: 200, bodyExcerpt: "Article slug mismatch: x" },
        { errorMarkers: ["Article slug mismatch"] },
      ),
    ).toMatchObject({ state: "error", basis: expect.stringContaining("error-marker") });
    expect(
      classifyNegativeState(
        { ok: true, httpStatus: 404, bodyExcerpt: "", notInCache: false },
        null,
      ),
    ).toBe("other");
  });

  it("the declared expectation stays [error, last-good]; '/' carries the empty probe for both expected-negative ops", () => {
    const root = mkdtempSync(join(tmpdir(), "dbo-fn2-"));
    try {
      makeFakeSource(join(root, "src"));
      const m = materializeFieldNotes({
        fixtureDir: join(HARNESS_DIR, "fixtures", "field-notes"),
        sourceDir: join(root, "src"),
        workDir: join(root, "work"),
        adapter: "direct-read",
      });
      for (const op of [
        { op: "rename-filename-only", article: "article-00008" },
        { op: "edit-invalid-frontmatter", article: "article-00009" },
      ]) {
        const a = planFieldNotesOp(m, m.model, op);
        expect(a.expectState).toEqual(["error", "last-good"]);
        expect(a.observe.find((o) => o.path === "/").empty).toEqual(EMPTY_HOME_PROBE);
      }
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});
