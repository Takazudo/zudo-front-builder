/**
 * traverse.chromium.spec.mjs — Level-4 (real browser) confirm gate for the
 * Traverse Fastpath epic (issue #1375, sub-issue #1378).
 *
 * ============================================================
 * WHY THIS SUITE EXISTS
 * ============================================================
 * The two deliverables this suite proves are documented, at the Level-2
 * (happy-dom) layer, as having a real-browser blind spot:
 *   - packages/zfb-runtime/src/__tests__/client-router/router.test.ts's
 *     "same-page traverse fast-path (#1374 / #1376)" describe block hand-feeds
 *     `dispatchPopstate()` — it proves the gate's BRANCH LOGIC (fetch vs.
 *     fast-path), not that a REAL Back/Forward gesture in a real browser hits
 *     that branch, that no reload occurs, and that live DOM/JS state survives.
 *   - the same file's "syncHistoryEntry() — public history bookkeeping API
 *     (#1377)" describe block has the identical blind spot for the pathname-
 *     dialog case.
 * Both describe blocks say so explicitly: "Definitive proof is the Wave-3
 * chromium harness (#1378)." This file is that proof.
 *
 * ============================================================
 * DEFLAKING DISCIPLINE (issue #1346) — same rules as router.chromium.spec.mjs
 * ============================================================
 * Every wait is keyed on the router's OWN lifecycle events or on state the
 * router itself sets — NO bare `waitForTimeout`, NO `networkidle`. The harness
 * below (init script + helpers) is intentionally a byte-for-byte copy of
 * router.chromium.spec.mjs's, mirroring the tests/webkit-back-history vs.
 * tests/router-chromium precedent of deliberately duplicating shared harness
 * plumbing across sibling Level-4 suites rather than coupling them through a
 * shared module (see serve-fixture.mjs's header comment in this directory).
 *
 * The fast-path specs (1-3 below) have NO `zfb:page-load` (or any zfb:* event)
 * to await on the happy path — that is the entire point of the fast path. Per
 * the issue's required wait shape: wait on router-owned/user-visible state
 * that MUST change (`location.hash` reaching the expected value, plus the
 * DOM-rendered counter marker), THEN assert the seq counter is unchanged since
 * the pre-trigger baseline. Spec 4 (syncHistoryEntry pathname dialog) is the
 * mirror image: pathname changes, so the FULL lifecycle fires and is awaited
 * the normal way (waitForNavDone).
 */

// @ts-check
import { test, expect } from "@playwright/test";

// ---------------------------------------------------------------------------
// Harness installed before every document, before any page script runs.
// (Copied from router.chromium.spec.mjs — see that file's header for the full
// rationale of each piece; only the parts these specs need are exercised.)
// ---------------------------------------------------------------------------
const HARNESS_INIT = () => {
  // @ts-expect-error - test-only globals
  if (window.__zfbHarness) return;
  // @ts-expect-error
  window.__zfbHarness = true;

  const uuid =
    typeof crypto !== "undefined" && crypto.randomUUID
      ? crypto.randomUUID()
      : String(Math.random());
  // @ts-expect-error
  window.__docId = uuid;

  const NAMES = [
    "zfb:before-preparation",
    "zfb:after-preparation",
    "zfb:before-swap",
    "zfb:after-swap",
    "zfb:page-load",
    "zfb:navigation-aborted",
  ];
  const rec = { events: [], seq: 0 };
  // @ts-expect-error
  window.__zfb = rec;
  for (const name of NAMES) {
    document.addEventListener(name, (ev) => {
      rec.events.push({
        name,
        seq: ++rec.seq,
        direction:
          ev && typeof (/** @type {any} */ (ev).direction) === "string"
            ? /** @type {any} */ (ev).direction
            : null,
      });
    });
  }
};

test.beforeEach(async ({ page }) => {
  await page.addInitScript(HARNESS_INIT);
});

// ---------------------------------------------------------------------------
// Event-keyed wait helpers (no bare timeouts, no networkidle).
// ---------------------------------------------------------------------------

/** Current lifecycle sequence counter — capture BEFORE triggering a navigation. */
async function seqNow(page) {
  return page.evaluate(() => /** @type {any} */ (window).__zfb?.seq ?? 0);
}

/** Wait until a lifecycle event `name` has been recorded with seq > `after`. */
async function waitForZfbEvent(page, name, after, timeout = 8000) {
  await page.waitForFunction(
    ({ name, after }) =>
      /** @type {any} */ (window.__zfb?.events ?? []).some((e) => e.name === name && e.seq > after),
    { name, after },
    { timeout },
  );
}

/**
 * Wait until a navigation is fully finished AND quiescent — see
 * router.chromium.spec.mjs's `waitForNavDone` for the full rationale (the
 * `data-zfb-transition` absence check keeps the next step from starting
 * mid-animation, one of the 5 deflaking root causes).
 */
async function waitForNavDone(page, after, timeout = 8000) {
  await waitForZfbEvent(page, "zfb:page-load", after, timeout);
  await page.waitForFunction(
    () => !document.documentElement.hasAttribute("data-zfb-transition"),
    undefined,
    { timeout },
  );
}

/** Wait for the initial full-page load's `zfb:page-load` (router booted). */
async function waitForInitialLoad(page) {
  await page.waitForFunction(() =>
    /** @type {any} */ (window.__zfb?.events ?? []).some((e) => e.name === "zfb:page-load"),
  );
}

/** Names (in seq order) of lifecycle events recorded with seq > `after`. */
async function eventNamesSince(page, after) {
  return page.evaluate((after) => {
    const rec = /** @type {any} */ (window).__zfb;
    return rec.events.filter((e) => e.seq > after).map((e) => e.name);
  }, after);
}

/** Direction of the last `name` event recorded with seq > `after` (or null). */
async function lastDirectionSince(page, name, after) {
  return page.evaluate(
    ({ name, after }) => {
      const rec = /** @type {any} */ (window).__zfb;
      const evs = rec.events.filter((e) => e.name === name && e.seq > after);
      return evs.length ? evs[evs.length - 1].direction : null;
    },
    { name, after },
  );
}

// ===========================================================================
// Spec 1 — raw pushState modal + Back takes the same-page traverse fast path:
// no lifecycle event, no reload, live DOM/JS state preserved. (#1374 / #1376)
// ===========================================================================
test("raw pushState modal + Back takes the traverse fast path (no lifecycle event, no reload, state preserved)", async ({
  page,
}) => {
  await page.goto("/traverse-modal.html");
  await waitForInitialLoad(page);

  const docId = await page.evaluate(() => /** @type {any} */ (window).__docId);

  // Open the modal: a raw (non-router) pushState with a non-null state object.
  await page.click("#open-modal");
  await page.waitForFunction(() => location.hash === "#modal");
  await expect(page.locator("#counter")).toHaveText("1");

  const s = await seqNow(page);
  await page.goBack();
  // Deterministic wait shape (#1378): there is no zfb:* event to await on the
  // fast path, so wait on the one piece of router-owned/user-visible state
  // that MUST change — the hash clearing back to "".
  await page.waitForFunction(() => location.hash === "");

  // No lifecycle event fired for this Back at all: the fast path returns via
  // moveToLocation() before doPreparation() (which would dispatch
  // zfb:before-preparation) ever runs.
  expect(await eventNamesSince(page, s)).toEqual([]);
  // No full reload: same JS context, same __docId.
  expect(await page.evaluate(() => /** @type {any} */ (window).__docId)).toBe(docId);
  // No body swap either (a swap would re-execute the page's <script> and reset
  // the counter to "0") — the counter marker proves the live DOM/JS survived.
  await expect(page.locator("#counter")).toHaveText("1");
});

// ===========================================================================
// Spec 2 — Forward-reopen after the fast-path Back: same fast path, same
// guarantees, hash restored.
// ===========================================================================
test("Forward after the fast-path Back reopens the modal via the fast path too (no lifecycle event, no reload, state preserved)", async ({
  page,
}) => {
  await page.goto("/traverse-modal.html");
  await waitForInitialLoad(page);
  const docId = await page.evaluate(() => /** @type {any} */ (window).__docId);

  await page.click("#open-modal");
  await page.waitForFunction(() => location.hash === "#modal");

  let s = await seqNow(page);
  await page.goBack();
  await page.waitForFunction(() => location.hash === "");
  expect(await eventNamesSince(page, s)).toEqual([]);

  s = await seqNow(page);
  await page.goForward();
  await page.waitForFunction(() => location.hash === "#modal");

  expect(await eventNamesSince(page, s)).toEqual([]);
  expect(await page.evaluate(() => /** @type {any} */ (window).__docId)).toBe(docId);
  await expect(page.locator("#counter")).toHaveText("1");
});

// ===========================================================================
// Spec 3 — the zfb-traverse-refetch opt-out meta forces the FULL lifecycle
// back on for the identical Back gesture: opt-out respected. (#1376)
// ===========================================================================
test("the traverseRefetch opt-out meta forces the full lifecycle on Back (opt-out respected)", async ({
  page,
}) => {
  await page.goto("/traverse-modal-optout.html");
  await waitForInitialLoad(page);
  const docId = await page.evaluate(() => /** @type {any} */ (window).__docId);

  await page.click("#open-modal");
  await page.waitForFunction(() => location.hash === "#modal");

  const s = await seqNow(page);
  await page.goBack();
  // Opted out: this Back falls through to the normal fetch + swap pipeline, so
  // (unlike specs 1-2) there IS a zfb:before-preparation to await.
  await waitForZfbEvent(page, "zfb:before-preparation", s);
  await waitForNavDone(page, s);

  expect(await page.evaluate(() => location.hash)).toBe("");
  // Still a client-side swap, not a full browser reload: same JS context.
  expect(await page.evaluate(() => /** @type {any} */ (window).__docId)).toBe(docId);
});

// ===========================================================================
// Spec 4 — syncHistoryEntry()-driven pathname dialog: Back is a cross-page
// traverse (different pathname), so it is a NORMAL soft navigation — full
// lifecycle fires, correct direction, no stuck transition overlay, and
// history.state.index stays finite. (#1377)
// ===========================================================================
test("syncHistoryEntry pathname dialog: Back triggers a normal soft navigation with the correct direction", async ({
  page,
}) => {
  await page.goto("/sync-dialog.html");
  await waitForInitialLoad(page);

  const indexBefore = await page.evaluate(() => /** @type {any} */ (history.state)?.index);
  expect(Number.isFinite(indexBefore)).toBe(true);

  const s0 = await seqNow(page);
  await page.click("#open-dialog");
  await page.waitForFunction(() => location.pathname === "/photos/photo-1/");
  // syncHistoryEntry() is side-effect free: no lifecycle event, no DOM/scroll
  // change, just a router-managed history entry (#1377 contract).
  expect(await eventNamesSince(page, s0)).toEqual([]);
  const indexAfterPush = await page.evaluate(() => /** @type {any} */ (history.state)?.index);
  expect(indexAfterPush).toBe(indexBefore + 1);

  const s = await seqNow(page);
  await page.goBack();
  // The pathname changed (/photos/photo-1/ -> the dialog's own page), so
  // samePage() is false and this Back is a REAL cross-page traverse — unlike
  // specs 1-3, the full lifecycle fires and is awaited the normal way.
  await waitForNavDone(page, s);

  // Correct direction reported on zfb:before-swap.
  expect(await lastDirectionSince(page, "zfb:before-swap", s)).toBe("back");
  // Landed back on the dialog's own page, not stuck on the virtual photo URL.
  expect(new URL(page.url()).pathname).not.toBe("/photos/photo-1/");
  // No overlay anomaly: waitForNavDone already asserts data-zfb-transition is
  // gone; the page's own DOM is live and interactive again.
  await expect(page.locator("#open-dialog")).toBeVisible();
  // history.state.index is finite (a real, monotonic entry) — not NaN/
  // undefined, which a raw-pushState desync would have produced.
  const indexAfterBack = await page.evaluate(() => /** @type {any} */ (history.state)?.index);
  expect(Number.isFinite(indexAfterBack)).toBe(true);
});
