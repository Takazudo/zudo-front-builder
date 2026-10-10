/**
 * history-rejection.chromium.spec.mjs — Level-4 (real browser) regression for
 * a REJECTED router History write (#4125 / #4127 / #4128).
 *
 * ============================================================
 * WHAT THIS IS — AND IS NOT
 * ============================================================
 * WebKit throws from `history.pushState`/`replaceState` once a page exceeds its
 * History-write budget (#4125). The fixed router treats such a rejection as a
 * navigation failure: a GET navigation recovers with ONE deliberate document
 * load of its resolved destination (`location.href`, or `location.replace` when
 * it was a replace or already made an early entry); a submitted form aborts and
 * keeps the old page. The defect it replaced swapped the new content in under
 * the OLD URL.
 *
 * Chromium never rejects these writes, so this file injects the rejection
 * through a controlled seam: `History.prototype.pushState/replaceState` are
 * wrapped before any page script runs, and a test arms ONE rejection for one
 * method + target path in the current document. Every other call goes straight
 * to the native method. This is a deterministic fault-injection contract test
 * of the REAL built runtime — it is NOT evidence of WebKit's native quota
 * behavior. That lives in tests/webkit-back-history/history-quota.webkit.spec.mjs.
 *
 * ============================================================
 * EVIDENCE THAT SURVIVES DOCUMENT REPLACEMENT
 * ============================================================
 * Recovery replaces the document, so in-memory recorders die with it. The init
 * script appends every lifecycle event and every URL-carrying History write to
 * a `sessionStorage` log, each tagged with the per-document id that wrote it.
 * That separates the OLD document's aborted SPA attempt from the NEW document's
 * own legitimate load, and keeps the old document's observations readable
 * after it is gone.
 *
 * Waits are keyed on the log (router lifecycle events / a new document id) —
 * no bare timeouts, no networkidle (same discipline as router.chromium.spec.mjs).
 */

// @ts-check
import { test, expect } from "@playwright/test";

const LOG_KEY = "__hrLog";

// `arg.noViewTransition` hides the native View Transitions API before the router
// module evaluates, so the router takes its simulated (fallback) transition.
const HARNESS_INIT = (arg) => {
  const w = /** @type {any} */ (window);
  if (w.__hrDoc) return;
  const LOG_KEY = "__hrLog";
  const doc =
    typeof crypto !== "undefined" && crypto.randomUUID
      ? crypto.randomUUID()
      : String(Math.random());
  w.__hrDoc = doc;
  const log = (entry) => {
    const all = JSON.parse(sessionStorage.getItem(LOG_KEY) ?? "[]");
    all.push({ doc, ...entry });
    sessionStorage.setItem(LOG_KEY, JSON.stringify(all));
  };
  const nav = /** @type {any} */ (performance.getEntriesByType("navigation")[0]);
  log({ kind: "document", href: location.href, navigationType: nav?.type ?? null });

  let insideViewTransition = 0;
  if (arg?.noViewTransition) {
    // Own-property shadow of Document.prototype.startViewTransition; the router
    // reads `!!document.startViewTransition` at module evaluation.
    /** @type {any} */ (document).startViewTransition = undefined;
  } else if (typeof document.startViewTransition === "function") {
    const native = document.startViewTransition.bind(document);
    /** @type {any} */ (document).startViewTransition = (update) => {
      log({ kind: "view-transition" });
      return native(async () => {
        insideViewTransition++;
        try {
          return await update();
        } finally {
          insideViewTransition--;
        }
      });
    };
  }

  // The router's simulated transition marks each phase on <html>; this proves
  // the fallback path actually ran rather than inferring it from a missing VT.
  // Logged synchronously at the write so ordering against lifecycle events holds.
  const nativeSetAttribute = Element.prototype.setAttribute;
  Element.prototype.setAttribute = function (name, value) {
    if (name === "data-zfb-transition-fallback") log({ kind: "fallback-phase", phase: value });
    return nativeSetAttribute.call(this, name, value);
  };

  for (const method of /** @type {const} */ (["pushState", "replaceState"])) {
    const native = History.prototype[method];
    History.prototype[method] = function (_data, _unused, url) {
      // URL-less writes (scroll saves, the init seed) are not navigation writes.
      if (arguments.length < 3 || url == null) return native.apply(this, arguments);
      const target = new URL(String(url), location.href).href;
      const entry = {
        kind: "write",
        method,
        url: target,
        from: location.href,
        insideViewTransition: insideViewTransition > 0,
      };
      const arm = w.__hrReject;
      if (arm && arm.method === method && new URL(target).pathname === arm.path) {
        w.__hrReject = null;
        log({ ...entry, outcome: "injected-rejection" });
        throw new DOMException("injected History rejection (#4128 seam)", "SecurityError");
      }
      try {
        const result = native.apply(this, arguments);
        log({ ...entry, outcome: "committed" });
        return result;
      } catch (error) {
        log({ ...entry, outcome: "native-rejection", error: /** @type {any} */ (error)?.name });
        throw error;
      }
    };
  }

  for (const name of [
    "zfb:before-preparation",
    "zfb:after-preparation",
    "zfb:before-swap",
    "zfb:after-swap",
    "zfb:page-load",
    "zfb:navigation-aborted",
  ]) {
    document.addEventListener(name, () => {
      log({
        kind: "event",
        name,
        href: location.href,
        h1: document.querySelector("h1")?.textContent ?? null,
      });
    });
  }
};

/** @param {import("@playwright/test").Page} page */
async function readLog(page) {
  return page.evaluate((key) => JSON.parse(sessionStorage.getItem(key) ?? "[]"), LOG_KEY);
}

/** @param {import("@playwright/test").Page} page */
async function docId(page) {
  return page.evaluate(() => /** @type {any} */ (window).__hrDoc);
}

/** Wait until document `doc` has logged `zfb:page-load` (`count` times, default once). */
async function waitForPageLoad(page, doc, count = 1) {
  await expect
    .poll(
      async () =>
        (await readLog(page).catch(() => [])).filter(
          (e) => e.doc === doc && e.kind === "event" && e.name === "zfb:page-load",
        ).length,
      { timeout: 8000 },
    )
    .toBeGreaterThanOrEqual(count);
  await page.waitForFunction(() => !document.documentElement.hasAttribute("data-zfb-transition"));
}

/** Load `path` as a fresh document and wait for its router to boot. */
async function open(page, path) {
  await page.goto(path);
  const doc = await docId(page);
  await waitForPageLoad(page, doc);
  return doc;
}

/** SPA-navigate by clicking `selector` and wait for the swap in the same document. */
async function spaClick(page, selector, expectedH1) {
  const doc = await docId(page);
  const loads = (await readLog(page)).filter(
    (e) => e.doc === doc && e.name === "zfb:page-load",
  ).length;
  await page.click(selector);
  await waitForPageLoad(page, doc, loads + 1);
  await expect(page.locator("h1")).toHaveText(expectedH1);
  expect(await docId(page)).toBe(doc);
}

/** Wait for the recovery document (any document other than `oldDoc`) to boot. */
async function waitForRecoveryDocument(page, oldDoc) {
  let newDoc = null;
  await expect
    .poll(
      async () => {
        newDoc = await docId(page).catch(() => null);
        return newDoc !== null && newDoc !== oldDoc;
      },
      { timeout: 8000 },
    )
    .toBe(true);
  await waitForPageLoad(page, newDoc);
  return /** @type {string} */ (newDoc);
}

/** Arm one rejection of `method` for a write targeting `path` in the current document. */
async function arm(page, method, path) {
  await page.evaluate(
    ({ method, path }) => {
      /** @type {any} */ (window).__hrReject = { method, path };
    },
    { method, path },
  );
}

const pathOf = (href) => new URL(href).pathname;
const historyLength = (page) => page.evaluate(() => history.length);

/**
 * Traverse with the browser's Back/Forward and wait for the landed page's
 * heading. A traversal may land on an SPA entry of a live document or require a
 * document load; either way the URL and content must agree.
 */
async function traverse(page, direction, expectedPath, expectedH1) {
  if (direction === "back") await page.goBack();
  else await page.goForward();
  await expect(page.locator("h1")).toHaveText(expectedH1);
  expect(pathOf(page.url())).toBe(expectedPath);
}

/** The old document's lifecycle record for the navigation that was rejected. */
function eventsOf(log, doc, sinceIndex) {
  return log
    .slice(sinceIndex)
    .filter((e) => e.doc === doc && e.kind === "event")
    .map((e) => e.name);
}

// ===========================================================================
// Rejected early push: no SPA content under the old URL; one document load.
// ===========================================================================
test("rejected push recovers with one document load of the destination", async ({ page }) => {
  await page.addInitScript(HARNESS_INIT);
  await open(page, "/index.html");
  await spaClick(page, "#to-page-a", "Page A");
  const lengthBefore = await historyLength(page);
  const oldDoc = await docId(page);
  const mark = (await readLog(page)).length;

  await arm(page, "pushState", "/page-b.html");
  await page.click("#to-page-b");
  const newDoc = await waitForRecoveryDocument(page, oldDoc);

  const log = await readLog(page);
  // The router's own write was the one rejected — and nothing was retried.
  const writes = log.slice(mark).filter((e) => e.kind === "write");
  expect(writes).toEqual([
    expect.objectContaining({
      doc: oldDoc,
      method: "pushState",
      url: expect.stringMatching(/\/page-b\.html$/),
      outcome: "injected-rejection",
      insideViewTransition: false,
    }),
  ]);
  // The old document aborted before any swap ...
  expect(eventsOf(log, oldDoc, mark)).toEqual([
    "zfb:before-preparation",
    "zfb:after-preparation",
    "zfb:navigation-aborted",
  ]);
  // ... with its own content still under its own URL at the abort.
  const aborted = log.slice(mark).find((e) => e.name === "zfb:navigation-aborted");
  expect(pathOf(aborted.href)).toBe("/page-a.html");
  expect(aborted.h1).toBe("Page A");

  // The new document is a real load of the resolved destination.
  const newDocEntry = log.find((e) => e.doc === newDoc && e.kind === "document");
  expect(pathOf(newDocEntry.href)).toBe("/page-b.html");
  expect(newDocEntry.navigationType).toBe("navigate");
  expect(pathOf(page.url())).toBe("/page-b.html");
  await expect(page.locator("h1")).toHaveText("Page B");
  // `location.href` adds exactly one entry.
  expect(await historyLength(page)).toBe(lengthBefore + 1);

  await traverse(page, "back", "/page-a.html", "Page A");
  await traverse(page, "back", "/index.html", "Home");
  await traverse(page, "forward", "/page-a.html", "Page A");
  await traverse(page, "forward", "/page-b.html", "Page B");
  expect(await historyLength(page)).toBe(lengthBefore + 1);
});

// ===========================================================================
// Rejected replace: recovery uses location.replace — no entry is added.
// ===========================================================================
test("rejected replace recovers with location.replace", async ({ page }) => {
  await page.addInitScript(HARNESS_INIT);
  await open(page, "/index.html");
  await spaClick(page, "#to-page-a", "Page A");
  const lengthBefore = await historyLength(page);
  const oldDoc = await docId(page);
  const mark = (await readLog(page)).length;

  await arm(page, "replaceState", "/page-b.html");
  // Fire-and-forget: the document is replaced before navigate() could resolve.
  await page.evaluate(() => {
    void import("/dist/client-router/router.js").then((m) =>
      m.navigate("/page-b.html", { history: "replace" }),
    );
  });
  const newDoc = await waitForRecoveryDocument(page, oldDoc);

  const log = await readLog(page);
  expect(log.slice(mark).filter((e) => e.kind === "write")).toEqual([
    expect.objectContaining({
      doc: oldDoc,
      method: "replaceState",
      url: expect.stringMatching(/\/page-b\.html$/),
      outcome: "injected-rejection",
    }),
  ]);
  expect(eventsOf(log, oldDoc, mark)).toEqual([
    "zfb:before-preparation",
    "zfb:after-preparation",
    "zfb:navigation-aborted",
  ]);
  expect(pathOf(log.find((e) => e.doc === newDoc && e.kind === "document").href)).toBe(
    "/page-b.html",
  );
  await expect(page.locator("h1")).toHaveText("Page B");
  // The Page A entry was replaced, not pushed over.
  expect(await historyLength(page)).toBe(lengthBefore);

  await traverse(page, "back", "/index.html", "Home");
  await traverse(page, "forward", "/page-b.html", "Page B");
  expect(await historyLength(page)).toBe(lengthBefore);
});

// ===========================================================================
// Rejected final correction after an early push: no phantom Back entry.
// ===========================================================================
test("rejected retarget correction after an early push leaves no phantom entry", async ({
  page,
}) => {
  await page.addInitScript(HARNESS_INIT);
  await open(page, "/index.html");
  await spaClick(page, "#to-page-a", "Page A");
  const lengthBefore = await historyLength(page);
  const oldDoc = await docId(page);
  const mark = (await readLog(page)).length;

  await page.evaluate(() => {
    document.addEventListener("zfb:before-swap", (ev) => {
      /** @type {any} */ (ev).to = new URL("/search-results.html", location.href);
    });
  });
  await arm(page, "replaceState", "/search-results.html");
  await page.click("#to-page-b");
  const newDoc = await waitForRecoveryDocument(page, oldDoc);

  const log = await readLog(page);
  const writes = log.slice(mark).filter((e) => e.kind === "write");
  // Early push committed outside the transition; the correction replaced it
  // and was the write that was rejected.
  expect(writes).toEqual([
    expect.objectContaining({
      doc: oldDoc,
      method: "pushState",
      url: expect.stringMatching(/\/page-b\.html$/),
      outcome: "committed",
      insideViewTransition: false,
    }),
    expect.objectContaining({
      doc: oldDoc,
      method: "replaceState",
      url: expect.stringMatching(/\/search-results\.html$/),
      outcome: "injected-rejection",
    }),
  ]);
  expect(eventsOf(log, oldDoc, mark)).toEqual([
    "zfb:before-preparation",
    "zfb:after-preparation",
    "zfb:before-swap",
    "zfb:navigation-aborted",
  ]);
  // The old DOM was never swapped.
  expect(log.slice(mark).find((e) => e.name === "zfb:navigation-aborted").h1).toBe("Page A");

  expect(pathOf(log.find((e) => e.doc === newDoc && e.kind === "document").href)).toBe(
    "/search-results.html",
  );
  await expect(page.locator("h1")).toHaveText("Search Results");
  // The early Page B entry was replaced by the recovery load, not left behind.
  expect(await historyLength(page)).toBe(lengthBefore + 1);

  await traverse(page, "back", "/page-a.html", "Page A");
  await traverse(page, "back", "/index.html", "Home");
  await traverse(page, "forward", "/page-a.html", "Page A");
  await traverse(page, "forward", "/search-results.html", "Search Results");
});

// ===========================================================================
// No early entry + late retarget under a native View Transition: the push is
// never attempted inside the transition, and before-swap fires once.
// ===========================================================================
test("late retarget without an early entry never pushes inside a native View Transition", async ({
  page,
}) => {
  await page.addInitScript(HARNESS_INIT);
  const oldDoc = await open(page, "/page-a.html");
  const lengthBefore = await historyLength(page);
  const mark = (await readLog(page)).length;

  await page.evaluate(() => {
    document.addEventListener("zfb:before-swap", (ev) => {
      /** @type {any} */ (ev).to = new URL("/page-b.html", location.href);
    });
  });
  // A link to the current URL: the early write is a no-op (same URL), so the
  // retargeted URL would need a NEW entry from inside the transition callback.
  await page.click("#to-page-a");
  const newDoc = await waitForRecoveryDocument(page, oldDoc);

  const log = await readLog(page);
  expect(log.slice(mark).some((e) => e.doc === oldDoc && e.kind === "view-transition")).toBe(true);
  // No URL write at all in the old document — in particular no push inside VT.
  expect(log.slice(mark).filter((e) => e.kind === "write")).toEqual([]);
  expect(eventsOf(log, oldDoc, mark)).toEqual([
    "zfb:before-preparation",
    "zfb:after-preparation",
    "zfb:before-swap",
    "zfb:navigation-aborted",
  ]);
  const newDocEntry = log.find((e) => e.doc === newDoc && e.kind === "document");
  expect(pathOf(newDocEntry.href)).toBe("/page-b.html");
  await expect(page.locator("h1")).toHaveText("Page B");
  expect(await historyLength(page)).toBe(lengthBefore + 1);

  await traverse(page, "back", "/page-a.html", "Page A");
  await traverse(page, "forward", "/page-b.html", "Page B");
});

// ===========================================================================
// Success controls: the same seam, unarmed, on the native and fallback paths.
// ===========================================================================
for (const mode of ["native View Transition", "fallback simulation"]) {
  test(`unrejected push stays an SPA swap (${mode})`, async ({ page }) => {
    const noViewTransition = mode === "fallback simulation";
    await page.addInitScript(HARNESS_INIT, { noViewTransition });
    const doc = await open(page, "/index.html");
    const lengthBefore = await historyLength(page);
    const mark = (await readLog(page)).length;

    await spaClick(page, "#to-page-a", "Page A");
    await spaClick(page, "#to-page-b", "Page B");

    const log = await readLog(page);
    expect(log.slice(mark).filter((e) => e.kind === "document")).toEqual([]);
    expect(log.slice(mark).filter((e) => e.kind === "view-transition").length).toBe(
      noViewTransition ? 0 : 2,
    );
    // The wrapper is not installed in fallback mode, so a zero VT count alone
    // is vacuous there; require the simulation's own "new" phase per swap
    // ("old" is also re-applied by the root-attribute swap, so it is not 1:1).
    expect(
      log.slice(mark).filter((e) => e.kind === "fallback-phase" && e.phase === "new").length,
    ).toBe(noViewTransition ? 2 : 0);
    expect(
      log
        .slice(mark)
        .filter((e) => e.kind === "write")
        .map((e) => [e.method, pathOf(e.url), e.outcome, e.insideViewTransition]),
    ).toEqual([
      ["pushState", "/page-a.html", "committed", false],
      ["pushState", "/page-b.html", "committed", false],
    ]);
    expect(await historyLength(page)).toBe(lengthBefore + 2);

    await traverse(page, "back", "/page-a.html", "Page A");
    await traverse(page, "back", "/index.html", "Home");
    await traverse(page, "forward", "/page-a.html", "Page A");
    expect(await docId(page)).toBe(doc);
  });
}

// ===========================================================================
// POST form: the single final write is rejected → abort, old DOM, POST once.
// ===========================================================================
test("rejected POST form write aborts on the old page without replaying the POST", async ({
  page,
}) => {
  await page.addInitScript(HARNESS_INIT);
  const posts = [];
  const documentRequests = [];
  page.on("request", (req) => {
    if (req.method() === "POST") posts.push(req.url());
    if (req.resourceType() === "document") documentRequests.push(req.url());
  });
  const doc = await open(page, "/post-form.html");
  const lengthBefore = await historyLength(page);
  const mark = (await readLog(page)).length;
  documentRequests.length = 0;

  await arm(page, "pushState", "/search-results.html");
  await page.click("#post-submit");
  await expect
    .poll(async () => eventsOf(await readLog(page), doc, mark).includes("zfb:navigation-aborted"))
    .toBe(true);

  // A follow-up SPA navigation in the same document proves no document load
  // was pending behind the abort.
  await spaClick(page, "#to-home", "Home");

  const log = await readLog(page);
  expect(log.slice(mark).filter((e) => e.kind === "document")).toEqual([]);
  expect(documentRequests).toEqual([]);
  expect(posts.length).toBe(1);
  expect(pathOf(posts[0])).toBe("/search-results.html");

  // The form wrote once, at its final target, through the simulated transition.
  const formWrites = log
    .slice(mark)
    .filter((e) => e.kind === "write" && pathOf(e.url) === "/search-results.html");
  expect(formWrites).toEqual([
    expect.objectContaining({
      method: "pushState",
      outcome: "injected-rejection",
      insideViewTransition: false,
    }),
  ]);
  const aborted = log.slice(mark).find((e) => e.name === "zfb:navigation-aborted");
  expect(pathOf(aborted.href)).toBe("/post-form.html");
  expect(aborted.h1).toBe("Post Form");
  // The full record, not a prefix: a late swap of the POST response after the
  // abort would otherwise hide behind the follow-up Home navigation.
  expect(eventsOf(log, doc, mark)).toEqual([
    "zfb:before-preparation",
    "zfb:after-preparation",
    "zfb:before-swap",
    "zfb:navigation-aborted",
    "zfb:before-preparation",
    "zfb:after-preparation",
    "zfb:before-swap",
    "zfb:after-swap",
    "zfb:page-load",
  ]);
  // Before the abort, the form ran on the simulation, never a native VT.
  const beforeAbort = log.slice(mark, log.indexOf(aborted));
  expect(beforeAbort.filter((e) => e.kind === "view-transition")).toEqual([]);
  expect(beforeAbort.some((e) => e.kind === "fallback-phase" && e.phase === "old")).toBe(true);
  // Only the follow-up Home navigation added an entry.
  expect(await historyLength(page)).toBe(lengthBefore + 1);
  expect(await docId(page)).toBe(doc);
});
