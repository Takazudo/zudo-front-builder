/**
 * router-chromium — Level-4 (real browser) smoke suite for the client router.
 *
 * ============================================================
 * WHY THIS SUITE EXISTS
 * ============================================================
 * The client-router's Level-2 (happy-dom) vitest suite documents, in-file, the
 * behaviors happy-dom fundamentally cannot model:
 *   - real `document.startViewTransition` (happy-dom has no VT API → the router
 *     falls back to its simulation branch there, so the native path is untested);
 *   - real scroll + scroll restoration (happy-dom's `scrollTo` is a no-op and it
 *     never fires `scroll`/`scrollend`, so the scroll-position record→restore
 *     round-trip cannot even start);
 *   - real popstate direction detection across genuine Back/Forward;
 *   - the aria-live route announcer's real 60ms title write;
 *   - real `<link rel=prefetch>` insertion (depends on `relList.supports`).
 *
 * The only Level-4 counterpart until now was the Mac-manual WebKit harness
 * (tests/webkit-back-history/, unchanged by this suite). Chromium downloads fine
 * on ubuntu runners and genuinely supports View Transitions + scrollend, so this
 * suite closes those blind spots in CI.
 *
 * ============================================================
 * DEFLAKING DISCIPLINE (issue #1346 — the whole point of doing this carefully)
 * ============================================================
 * Every wait here is keyed on the router's OWN lifecycle events or on state the
 * router itself sets. There are NO bare `waitForTimeout` calls and NO
 * `waitForLoadState('networkidle')` (SPA navigations need not issue any network
 * request, so networkidle is meaningless — an explicit zudo-test-wisdom
 * deflaking-recipe violation).
 *
 * Listener-before-trigger: the lifecycle recorder below is installed via
 * `addInitScript`, which runs BEFORE any page script (before the router module
 * evaluates). So the `document`-level listeners are already attached before the
 * router can dispatch a single event — no event can slip through between "start
 * navigating" and "start listening". Each test then captures the recorder's
 * sequence counter immediately BEFORE it triggers a navigation and waits for a
 * lifecycle event whose seq exceeds that baseline, so it can only ever match
 * THIS navigation, never a stale earlier one.
 *
 * The lifecycle event names come straight from the runtime
 * (packages/zfb-runtime/src/client-router/events.ts):
 *   zfb:before-preparation → zfb:after-preparation → zfb:before-swap →
 *   zfb:after-swap → zfb:page-load   (plus zfb:navigation-aborted on bail-out).
 */

// @ts-check
import { test, expect } from "@playwright/test";

// ---------------------------------------------------------------------------
// Harness installed before every document, before any page script runs.
// ---------------------------------------------------------------------------
//
// A body-swap SPA navigation does NOT create a new document, so this init
// script does NOT re-run across SPA navs — the listeners and counters below
// therefore persist across an entire SPA session, which is exactly what makes
// them a reliable cross-navigation record. It DOES re-run on a genuine full
// document load (initial goto, or any real browser reload), minting a fresh
// `__docId` — the "was this a real reload?" signal the SPA-swap spec asserts on.
const HARNESS_INIT = () => {
  // Guard: a fresh document means a fresh window, so this is effectively always
  // false at first run — but it makes re-entry a hard no-op regardless.
  // @ts-expect-error - test-only globals
  if (window.__zfbHarness) return;
  // @ts-expect-error
  window.__zfbHarness = true;

  // Unique per REAL document. An SPA nav preserves it (same JS context); a full
  // browser reload mints a new one.
  const uuid =
    typeof crypto !== "undefined" && crypto.randomUUID
      ? crypto.randomUUID()
      : String(Math.random());
  // @ts-expect-error
  window.__docId = uuid;

  // Lifecycle-event recorder. Attached to `document` BEFORE the router loads.
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
        // before-preparation / before-swap carry the resolved nav direction and
        // navigation type on the event object; the others leave these null.
        direction:
          ev && typeof (/** @type {any} */ (ev).direction) === "string"
            ? /** @type {any} */ (ev).direction
            : null,
        navigationType:
          ev && typeof (/** @type {any} */ (ev).navigationType) === "string"
            ? /** @type {any} */ (ev).navigationType
            : null,
      });
    });
  }

  // Instrument — NOT stub — the native View Transitions API. We wrap the real
  // `document.startViewTransition` and call straight through to it, only
  // recording that the native path ran and observing the real ViewTransition
  // object it returns. The genuine transition still executes. happy-dom has no
  // startViewTransition at all, so its router takes the simulation branch and
  // this record stays empty — that empty-vs-populated difference is precisely
  // the L4-only signal the VT spec asserts on.
  const native =
    typeof document.startViewTransition === "function"
      ? document.startViewTransition.bind(document)
      : null;
  const vt = { supported: !!native, calls: 0, finished: 0, lastReal: false };
  // @ts-expect-error
  window.__vt = vt;
  if (native) {
    // @ts-expect-error - override with a call-through wrapper
    document.startViewTransition = function (cb) {
      vt.calls++;
      const t = native(cb);
      // A real ViewTransition exposes ready/finished/updateCallbackDone promises.
      vt.lastReal = !!(
        t &&
        typeof t.finished?.then === "function" &&
        typeof t.ready?.then === "function" &&
        typeof t.updateCallbackDone?.then === "function"
      );
      // Count the real animation completing (resolve or reject both count as done).
      t.finished.then(
        () => {
          vt.finished++;
        },
        () => {
          vt.finished++;
        },
      );
      return t;
    };
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
 * Wait until a navigation is fully finished AND quiescent:
 *   1. `zfb:page-load` recorded with seq > `after` (DOM committed, islands
 *      mounted, route announcer created);
 *   2. `<html>` no longer carries `data-zfb-transition` — the router sets that
 *      attribute before the transition and removes it in the transition's
 *      `finished.finally`, so its absence means the REAL View Transition
 *      animation has fully settled (or the simulation has completed). Waiting
 *      for this keeps the next navigation from starting mid-animation
 *      (animations-in-flight is one of the 5 deflaking root causes).
 *
 * Both conditions are keyed on router-owned state — no timeouts, no networkidle.
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
// Spec 1 — SPA navigation swaps content + URL without a full page reload.
// ===========================================================================
test("SPA navigation swaps content and URL without a full page reload", async ({ page }) => {
  await page.goto("/index.html");
  await waitForInitialLoad(page);
  await expect(page.locator("h1")).toHaveText("Home");

  const docIdBefore = await page.evaluate(() => /** @type {any} */ (window).__docId);

  const s = await seqNow(page);
  await page.click("#to-page-a");
  await waitForNavDone(page, s);

  // Content swapped ...
  await expect(page.locator("h1")).toHaveText("Page A");
  // ... URL updated ...
  expect(new URL(page.url()).pathname).toBe("/page-a.html");
  // ... and it was a client swap, NOT a document reload: the JS context (and its
  // per-document id) survived. A full reload would have minted a new __docId.
  const docIdAfter = await page.evaluate(() => /** @type {any} */ (window).__docId);
  expect(docIdAfter).toBe(docIdBefore);
});

// ===========================================================================
// Spec 2 — lifecycle events fire in the documented order.
// ===========================================================================
test("lifecycle events fire in the documented order for one navigation", async ({ page }) => {
  await page.goto("/index.html");
  await waitForInitialLoad(page);

  const s = await seqNow(page);
  await page.click("#to-page-a");
  await waitForNavDone(page, s);

  const names = await eventNamesSince(page, s);
  const expectedOrder = [
    "zfb:before-preparation",
    "zfb:after-preparation",
    "zfb:before-swap",
    "zfb:after-swap",
    "zfb:page-load",
  ];
  const positions = expectedOrder.map((n) => names.indexOf(n));
  // Every expected lifecycle event was emitted ...
  expect(
    positions.every((p) => p >= 0),
    `missing events; saw ${names.join(",")}`,
  ).toBe(true);
  // ... and strictly in the documented order.
  for (let i = 1; i < positions.length; i++) {
    expect(positions[i]).toBeGreaterThan(positions[i - 1]);
  }
});

// ===========================================================================
// Spec 3 — Back restores the real scroll position (happy-dom cannot scroll).
// ===========================================================================
test("Back navigation restores the real scroll position", async ({ page }) => {
  await page.goto("/index.html");
  await waitForInitialLoad(page);

  // Scroll Home down to a real offset. happy-dom's scrollTo is a no-op, so this
  // whole record→restore round-trip is fundamentally L4-only.
  await page.evaluate(() => window.scrollTo(0, 1500));
  await page.waitForFunction(() => Math.round(window.scrollY) === 1500);

  // Forward-navigate to Page A. The router snapshots the current scroll (1500)
  // into Home's history entry as part of a non-traverse navigation. The fixed
  // nav means Playwright does not auto-scroll to click, so scrollY is still 1500
  // at click time.
  const s1 = await seqNow(page);
  await page.click("#to-page-a");
  await waitForNavDone(page, s1);
  await expect(page.locator("h1")).toHaveText("Page A");
  // A freshly navigated page starts at the top.
  expect(await page.evaluate(() => Math.round(window.scrollY))).toBe(0);

  // Genuine browser Back. The router restores Home's recorded scroll via a real
  // scrollTo inside the transition's update callback.
  const s2 = await seqNow(page);
  await page.goBack();
  await waitForNavDone(page, s2);
  await expect(page.locator("h1")).toHaveText("Home");
  expect(await page.evaluate(() => Math.round(window.scrollY))).toBe(1500);
});

// ===========================================================================
// Spec 4 — real document.startViewTransition drives the navigation.
// ===========================================================================
test("real document.startViewTransition drives the SPA navigation", async ({ page }) => {
  await page.goto("/index.html");
  await waitForInitialLoad(page);

  // Precondition: this browser natively supports View Transitions. happy-dom
  // does not, which is the entire reason this path needs an L4 test. If a future
  // Chromium ever dropped VT, this fails loudly rather than silently degrading
  // to the simulation path.
  expect(await page.evaluate(() => /** @type {any} */ (window).__vt.supported)).toBe(true);

  const s = await seqNow(page);
  await page.click("#to-page-a");
  await waitForNavDone(page, s);
  await expect(page.locator("h1")).toHaveText("Page A");

  // The router took the NATIVE startViewTransition branch (not the simulation
  // fallback): our call-through wrapper saw a real ViewTransition object.
  expect(await page.evaluate(() => /** @type {any} */ (window).__vt.calls)).toBeGreaterThanOrEqual(
    1,
  );
  expect(await page.evaluate(() => /** @type {any} */ (window).__vt.lastReal)).toBe(true);
  // And the real transition actually ran to completion (its finished promise
  // settled) — condition-keyed on the browser's own transition, not a timer.
  await page.waitForFunction(() => /** @type {any} */ (window).__vt.finished >= 1);
});

// ===========================================================================
// Spec 5 — popstate direction detection across real Back/Forward.
// ===========================================================================
test("popstate direction detection reports back/forward across real traversal", async ({
  page,
}) => {
  await page.goto("/index.html");
  await waitForInitialLoad(page);

  // Home -> A (forward push)
  let s = await seqNow(page);
  await page.click("#to-page-a");
  await waitForNavDone(page, s);
  expect(await lastDirectionSince(page, "zfb:before-swap", s)).toBe("forward");

  // A -> B (forward push)
  s = await seqNow(page);
  await page.click("#to-page-b");
  await waitForNavDone(page, s);
  expect(await lastDirectionSince(page, "zfb:before-swap", s)).toBe("forward");

  // B -> A (genuine Back)
  s = await seqNow(page);
  await page.goBack();
  await waitForNavDone(page, s);
  await expect(page.locator("h1")).toHaveText("Page A");
  expect(await lastDirectionSince(page, "zfb:before-swap", s)).toBe("back");

  // A -> B (genuine Forward)
  s = await seqNow(page);
  await page.goForward();
  await waitForNavDone(page, s);
  await expect(page.locator("h1")).toHaveText("Page B");
  expect(await lastDirectionSince(page, "zfb:before-swap", s)).toBe("forward");
});

// ===========================================================================
// Spec 6 — the aria-live route announcer announces the new route.
// ===========================================================================
test("route announcer exposes an aria-live region that announces the new route", async ({
  page,
}) => {
  await page.goto("/index.html");
  await waitForInitialLoad(page);

  const s = await seqNow(page);
  await page.click("#to-page-a");
  await waitForNavDone(page, s);

  const announcer = page.locator(".zfb-route-announcer");
  // Exactly one announcer, with the assertive/atomic aria-live contract.
  await expect(announcer).toHaveCount(1);
  await expect(announcer).toHaveAttribute("aria-live", "assertive");
  await expect(announcer).toHaveAttribute("aria-atomic", "true");
  // The real 60ms timer writes the new page's title into the live region.
  // toHaveText is a web-first assertion that auto-retries until the async write
  // lands — condition-keyed, not a fixed sleep.
  await expect(announcer).toHaveText("Page A");
});

// ===========================================================================
// Spec 7 — prefetch inserts a real <link rel=prefetch> into <head>.
// ===========================================================================
test("prefetch inserts a real <link rel=prefetch> for a tap-strategy link", async ({ page }) => {
  await page.goto("/index.html");
  await waitForInitialLoad(page);

  // No prefetch link exists before the trigger.
  await expect(page.locator('head link[rel="prefetch"][href*="prefetch-target.html"]')).toHaveCount(
    0,
  );

  // The "tap" strategy calls prefetch() synchronously on mousedown (no
  // requestIdleCallback), so this is deterministic. Dispatching only mousedown
  // (not a full click) exercises the prefetch path without navigating away.
  await page.dispatchEvent("#prefetch-link", "mousedown", { bubbles: true });

  // In a real browser, feature detection (relList.supports('prefetch')) is true,
  // so the router inserts a genuine <link rel=prefetch>. happy-dom's relList
  // support is unreliable, so this real-browser insertion is an L4-only signal.
  // toHaveCount is a web-first assertion that retries until the node appears.
  await expect(
    page.locator('head link[rel="prefetch"][href$="/prefetch-target.html"]'),
  ).toHaveCount(1);
});

// ===========================================================================
// Spec 8 — Forward after Back restores the entry's LIVE-tracked scroll offset.
// ===========================================================================
test("Forward after Back restores the router's live-tracked scroll position", async ({ page }) => {
  await page.goto("/index.html");
  await waitForInitialLoad(page);

  // Home -> A. Page A's history entry is pushed seeded at the top (scrollY 0).
  let s = await seqNow(page);
  await page.click("#to-page-a");
  await waitForNavDone(page, s);
  await expect(page.locator("h1")).toHaveText("Page A");

  // Scroll Page A down, then wait until the router has PERSISTED that offset into
  // Page A's history entry. A real scroll + scrollend (Chromium fires both;
  // happy-dom fires neither) drives onScrollEnd -> updateScrollPosition, which
  // replaceState()s the live offset (900) INTO the current entry, overwriting its
  // push-time seed of 0. This continuous live-tracking is the whole point — the
  // entry does NOT stay frozen at its push-time scroll.
  //
  // The second wait (on history.state, router-owned state — not a timer) is what
  // makes the record->restore round-trip DETERMINISTIC: scrollend is async, so a
  // fast goBack() could otherwise race ahead of onScrollEnd and leave the entry
  // still reading 0. Waiting for the persist is the event-keyed guard the suite's
  // deflaking discipline requires.
  await page.evaluate(() => window.scrollTo(0, 900));
  await page.waitForFunction(() => Math.round(window.scrollY) === 900);
  await page.waitForFunction(() => Math.round(window.history.state?.scrollY ?? -1) === 900);
  s = await seqNow(page);
  await page.goBack();
  await waitForNavDone(page, s);
  await expect(page.locator("h1")).toHaveText("Home");

  // Forward again to Page A. With history.scrollRestoration = "manual", the router
  // restores that entry's LAST-KNOWN scroll (900) — exactly like the browser's own
  // manual restoration — NOT the push-time 0. That the restored offset is the
  // live-tracked 900 proves onScrollEnd persisted it; happy-dom's L2 suite cannot
  // reach this, having no real scroll/scrollend events.
  s = await seqNow(page);
  await page.goForward();
  await waitForNavDone(page, s);
  await expect(page.locator("h1")).toHaveText("Page A");
  expect(await page.evaluate(() => Math.round(window.scrollY))).toBe(900);
});
