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

// ===========================================================================
// Spec 5 — syncHistoryEntry same-pathname (hash) dialog: Forward-reopen after
// a fast-path Back preserves scroll — NOT reset to (0,0). (#1398)
//
// Unlike spec 4 (a DIFFERENT pathname, full lifecycle), #open-dialog-samepage
// keeps the current pathname and only changes the hash, so samePage() is true
// and Back/Forward takes the traverse fast path — the exact path
// syncHistoryEntry's scroll-stamping fix needs a real-browser proof for.
// Before the fix, syncHistoryEntry stamped the pushed entry's scroll as
// (0,0), so re-entering it via Forward would scrollTo(0,0) and snap the
// underlying page to the top under the reopened modal.
// ===========================================================================
test("syncHistoryEntry same-pathname (hash) dialog: Forward-reopen preserves scroll, not reset to (0,0)", async ({
  page,
}) => {
  await page.goto("/sync-dialog.html");
  await waitForInitialLoad(page);

  // Scroll the underlying page down before opening the dialog.
  await page.evaluate(() => window.scrollTo(0, 600));
  await page.waitForFunction(() => window.scrollY === 600);

  await page.click("#open-dialog-samepage");
  await page.waitForFunction(() => location.hash === "#modal");
  // syncHistoryEntry is side-effect free — scroll is unchanged by the push itself.
  expect(await page.evaluate(() => window.scrollY)).toBe(600);

  // Back: the same-page traverse fast path restores the pre-dialog entry's
  // persisted scroll (600) — already correct before this fix (updateScrollPosition
  // persists the OUTGOING entry's live scroll before the push).
  let s = await seqNow(page);
  await page.goBack();
  await page.waitForFunction(() => location.hash === "");
  expect(await eventNamesSince(page, s)).toEqual([]);
  expect(await page.evaluate(() => window.scrollY)).toBe(600);

  // Forward: reopen the dialog. Before the fix, syncHistoryEntry had stamped
  // THIS entry's scroll as (0,0), so the fast path's scrollTo(historyState.scrollX,
  // historyState.scrollY) would snap the page to the top here. After the fix,
  // the entry carries the CURRENT scroll at push time (600), so Forward
  // preserves it.
  s = await seqNow(page);
  await page.goForward();
  await page.waitForFunction(() => location.hash === "#modal");
  expect(await eventNamesSince(page, s)).toEqual([]);
  expect(await page.evaluate(() => window.scrollY)).toBe(600);
});

// ===========================================================================
// Specs 6-8 — fragment entries across a soft navigation (#4165).
//
// A → same-document fragment link → soft navigation to B → Back must render A
// again, not just restore A#fragment in the address bar. frag-a.html carries
// two links to the same fragment: one the router intercepts (router-managed
// entry) and one it leaves to the browser (data-zfb-reload), which creates a
// stateless entry. Before the fix the router ignored every popstate with a
// null state, so Back to the browser-created entry kept B rendered. Spec 8 is
// the counterpart: Back/Forward among the rendered document's own fragment
// entries stays the browser's job (no lifecycle, no swap, live DOM kept).
// ===========================================================================

const FULL_SWAP = [
  "zfb:before-preparation",
  "zfb:after-preparation",
  "zfb:before-swap",
  "zfb:after-swap",
  "zfb:page-load",
];

/** Collect uncaught page errors and console errors for the whole test. */
function collectErrors(page) {
  const errors = [];
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  page.on("pageerror", (error) => errors.push(error.message));
  return errors;
}

/**
 * Wait until #frag-target sits at its authored scroll margin below the
 * viewport top — the position a fragment navigation settles on.
 */
async function waitForFragmentSettled(page) {
  await page.waitForFunction(() => {
    const target = document.getElementById("frag-target");
    if (!target) return false;
    const margin = parseFloat(getComputedStyle(target).scrollMarginTop);
    return window.scrollY > 0 && Math.abs(target.getBoundingClientRect().top - margin) <= 1;
  });
}

async function expectFragmentPageA(page) {
  const url = new URL(page.url());
  expect(url.pathname).toBe("/frag-a.html");
  expect(url.hash).toBe("#frag-target");
  await expect(page.locator("h1")).toHaveText("Fragment A");
  await expect(page.locator("#a-only")).toHaveCount(1);
  await expect(page.locator("#b-only")).toHaveCount(0);
  await waitForFragmentSettled(page);
}

async function expectFragmentPageB(page) {
  expect(new URL(page.url()).pathname).toBe("/frag-b.html");
  await expect(page.locator("h1")).toHaveText("Fragment B");
  await expect(page.locator("#b-only")).toHaveCount(1);
  await expect(page.locator("#a-only")).toHaveCount(0);
}

for (const variant of [
  { link: "#native-frag-link", entry: "browser-created", state: "null" },
  { link: "#router-frag-link", entry: "router-managed", state: "object" },
]) {
  test(`fragment entry (${variant.entry}) + soft nav to B: Back restores A at the fragment, Forward/Back repeat cleanly`, async ({
    page,
  }) => {
    const errors = collectErrors(page);
    await page.goto("/frag-a.html");
    await waitForInitialLoad(page);
    const docId = await page.evaluate(() => /** @type {any} */ (window).__docId);

    // Same-document fragment navigation on A: no lifecycle event either way.
    let s = await seqNow(page);
    await page.click(variant.link);
    await page.waitForFunction(() => location.hash === "#frag-target");
    await waitForFragmentSettled(page);
    expect(await eventNamesSince(page, s)).toEqual([]);
    // Pins which kind of entry this variant really exercises.
    expect(
      await page.evaluate(() => (history.state === null ? "null" : typeof history.state)),
    ).toBe(variant.state);

    // Soft navigation to B (the link sits in the fixed nav: no scroll first).
    s = await seqNow(page);
    await page.click("#to-frag-b");
    await waitForNavDone(page, s);
    await expectFragmentPageB(page);
    const lengthOnB = await page.evaluate(() => history.length);

    // Back to the A#frag-target entry: A is fetched and swapped back in.
    s = await seqNow(page);
    await page.goBack();
    await waitForNavDone(page, s);
    expect(await eventNamesSince(page, s)).toEqual(FULL_SWAP);
    expect(await lastDirectionSince(page, "zfb:before-swap", s)).toBe("back");
    await expectFragmentPageA(page);

    // Forward to B, then Back again: each traversal swaps exactly once and
    // nothing is left mid-transition (waitForNavDone checks the attribute).
    s = await seqNow(page);
    await page.goForward();
    await waitForNavDone(page, s);
    // The direction of a traversal out of an entry without a router index is
    // not knowable, so only the swap itself is asserted here.
    expect(await eventNamesSince(page, s)).toEqual(FULL_SWAP);
    await expectFragmentPageB(page);

    s = await seqNow(page);
    await page.goBack();
    await waitForNavDone(page, s);
    expect(await eventNamesSince(page, s)).toEqual(FULL_SWAP);
    await expectFragmentPageA(page);

    // No phantom entry: traversals wrote nothing, and one more Back lands on
    // A's original entry — the rendered document, so no lifecycle at all.
    expect(await page.evaluate(() => history.length)).toBe(lengthOnB);
    s = await seqNow(page);
    await page.goBack();
    await page.waitForFunction(() => location.hash === "");
    expect(new URL(page.url()).pathname).toBe("/frag-a.html");
    expect(await eventNamesSince(page, s)).toEqual([]);
    await expect(page.locator("#a-only")).toHaveCount(1);

    // Soft navigations only: one JS context, no aborted navigation, no errors.
    expect(await page.evaluate(() => /** @type {any} */ (window).__docId)).toBe(docId);
    expect(
      await page.evaluate(() =>
        /** @type {any} */ (window).__zfb.events.filter((e) => e.name === "zfb:navigation-aborted"),
      ),
    ).toEqual([]);
    expect(errors).toEqual([]);
  });
}

test("browser-created fragment entry of the rendered page: Back/Forward stay no-fetch / no-swap (live DOM kept)", async ({
  page,
}) => {
  const errors = collectErrors(page);
  await page.goto("/frag-a.html");
  await waitForInitialLoad(page);
  const docId = await page.evaluate(() => /** @type {any} */ (window).__docId);
  // Live DOM marker a body swap would discard.
  await page.evaluate(() => {
    /** @type {any} */ (document.getElementById("a-only")).dataset.live = "kept";
  });

  const s = await seqNow(page);
  await page.click("#native-frag-link");
  await page.waitForFunction(() => location.hash === "#frag-target");
  await waitForFragmentSettled(page);
  expect(await page.evaluate(() => history.state)).toBeNull();

  await page.goBack();
  await page.waitForFunction(() => location.hash === "");
  await page.goForward();
  await page.waitForFunction(() => location.hash === "#frag-target");

  expect(await eventNamesSince(page, s)).toEqual([]);
  expect(new URL(page.url()).pathname).toBe("/frag-a.html");
  await expect(page.locator("#a-only")).toHaveAttribute("data-live", "kept");
  expect(await page.evaluate(() => /** @type {any} */ (window).__docId)).toBe(docId);
  expect(errors).toEqual([]);
});
