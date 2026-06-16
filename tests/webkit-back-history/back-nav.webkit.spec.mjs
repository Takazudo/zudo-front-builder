/**
 * WebKit back-navigation regression harness — LOCAL-HEAVY (T4), Mac only.
 *
 * ============================================================
 * PURPOSE
 * ============================================================
 * Reproduces the iOS/WebKit bug fixed in #1076: after SPA navigations driven by
 * the ClientRouter, pressing Back (or triggering a back-swipe) could skip entries
 * and land on the wrong page.
 *
 * Root cause summary:
 *   WebKit serves Back after an SPA route change from the bfcache — the page is
 *   restored without re-evaluating the router module, so `currentHistoryIndex`
 *   (module-level state) desyncs from the live history stack. The desynced index
 *   makes `derivePopDirection()` misfire, causing Back to skip an entry.
 *   #1076 adds an `onPageShow` handler that re-seeds the index on a persisted
 *   (bfcache) restore and hardens `derivePopDirection` against NaN/missing values.
 *
 * ============================================================
 * HOW TO RUN (Mac operator)
 * ============================================================
 *   # 1. Install WebKit once per machine (requires network; not available in CI):
 *   npx playwright install webkit
 *
 *   # 2. From the repo root, run the full harness (build + serve + test):
 *   pnpm test:webkit-back
 *
 *   # Or step by step:
 *   pnpm -C packages/zfb-runtime build
 *   npx playwright test --config tests/webkit-back-history/playwright.config.mjs
 *
 * ============================================================
 * RED → GREEN
 * ============================================================
 *   Before #1076 fix:
 *     Cases 1 and 2 → Back lands on Home (index.html) instead of List (list.html). FAIL.
 *     Case 3         → popstate fires but direction misfires; Detail stays visible. FAIL.
 *
 *   After #1076 fix (this branch):
 *     All three cases pass: Back reliably returns to the List page.
 *
 * ============================================================
 * NOT IN CI
 * ============================================================
 *   - This file is NOT imported by `pnpm test` (vitest) or health.yml.
 *   - `pnpm test:webkit-back` is the only entry point.
 *   - WebKit browser download is network-blocked in CI/cloud environments.
 *
 * ============================================================
 * SWIPE-BACK SIMULATION (Case 3) — LIMITATION NOTE
 * ============================================================
 *   A real iOS back-swipe gesture is a native UA event that sets
 *   `PopStateEvent.hasUAVisualTransition = true` on the popstate event. This flag
 *   routes the router into the "swap" fallback path (skipping SPA animations since
 *   the browser is already providing the visual transition).
 *
 *   Playwright WebKit on macOS does NOT expose a public API to trigger the iOS
 *   native swipe gesture or to synthesize a PopStateEvent with hasUAVisualTransition
 *   set — the flag is set by the browser engine itself only on genuine gesture-driven
 *   navigations. See: https://webkit.org/blog/14885/smooth-back-forward-navigations/
 *
 *   APPROXIMATION USED HERE:
 *   Case 3 dispatches a synthetic `popstate` event with `hasUAVisualTransition: true`
 *   set on the event object via `page.evaluate()`. This exercises the `transition()`
 *   call with `hasUAVisualTransition=true` → "swap" fallback path in the router,
 *   verifying the code branch that the real swipe would hit. It does NOT simulate
 *   bfcache restore (which is the other half of the swipe-back trigger); for that,
 *   the `pageshow persisted` path is tested implicitly by Cases 1 and 2 exercising
 *   the full popstate flow after real browser.goBack() calls.
 *
 *   TO GET A GENUINE SWIPE-BACK PROOF:
 *   Run the fixture manually in iOS Safari Simulator, navigate home→list→detail,
 *   then swipe back. Observe that Detail goes back to List, not Home.
 */

// @ts-check
import { test, expect } from "@playwright/test";

/** Wait for the ClientRouter's SPA navigation to complete by polling the <h1>. */
async function waitForHeading(page, expectedText, options = {}) {
  await expect(page.locator("h1")).toHaveText(expectedText, { timeout: 5000, ...options });
}

/** Read key history state values from the live browser context. */
async function historySnapshot(page) {
  return page.evaluate(() => ({
    href: location.href,
    historyLength: history.length,
    stateIndex: history.state?.index,
  }));
}

// ---------------------------------------------------------------------------
// Case 1: list → detail → Back → assert list
// ---------------------------------------------------------------------------
test("Case 1: Back from detail returns to list (not home)", async ({ page }) => {
  // Start at list (not home) to make the Back target unambiguous.
  await page.goto("/list.html");
  await waitForHeading(page, "List");
  const snapList = await historySnapshot(page);

  // SPA-navigate to detail via the client router link intercept.
  await page.click("#to-detail");
  await waitForHeading(page, "Detail");
  const snapDetail = await historySnapshot(page);

  // history.state.index must have incremented (pushState occurred).
  expect(snapDetail.stateIndex).toBeGreaterThan(snapList.stateIndex ?? -1);

  // Press browser Back.
  await page.goBack();
  await waitForHeading(page, "List");
  const snapAfterBack = await historySnapshot(page);

  // URL must be list.html, NOT index.html (home).
  expect(snapAfterBack.href).toContain("list.html");
  expect(snapAfterBack.stateIndex).toBe(snapList.stateIndex);
});

// ---------------------------------------------------------------------------
// Case 2: home → list → detail → Back → assert list
// ---------------------------------------------------------------------------
test("Case 2: Full chain home→list→detail→Back asserts list", async ({ page }) => {
  // Start at home.
  await page.goto("/index.html");
  await waitForHeading(page, "Home");
  const snapHome = await historySnapshot(page);

  // Navigate to list.
  await page.click("a[href='/list.html']");
  await waitForHeading(page, "List");
  const snapList = await historySnapshot(page);
  expect(snapList.stateIndex).toBeGreaterThan(snapHome.stateIndex ?? -1);

  // Navigate to detail.
  await page.click("#to-detail");
  await waitForHeading(page, "Detail");
  const snapDetail = await historySnapshot(page);
  expect(snapDetail.stateIndex).toBeGreaterThan(snapList.stateIndex);

  // history.length must reflect all three pushes + initial = at least 3.
  expect(snapDetail.historyLength).toBeGreaterThanOrEqual(3);

  // Back should land on List, not Home.
  await page.goBack();
  await waitForHeading(page, "List");
  const snapAfterBack = await historySnapshot(page);

  expect(snapAfterBack.href).toContain("list.html");
  expect(snapAfterBack.stateIndex).toBe(snapList.stateIndex);
});

// ---------------------------------------------------------------------------
// Case 3: Swipe-back simulation via synthetic popstate with hasUAVisualTransition
// ---------------------------------------------------------------------------
test("Case 3: Swipe-back simulation (synthetic hasUAVisualTransition popstate) returns to list", async ({
  page,
}) => {
  // Build up the same history chain: home → list → detail.
  await page.goto("/index.html");
  await waitForHeading(page, "Home");

  await page.click("a[href='/list.html']");
  await waitForHeading(page, "List");
  const snapList = await historySnapshot(page);

  await page.click("#to-detail");
  await waitForHeading(page, "Detail");

  // ---- Swipe-back simulation ----
  //
  // A real iOS swipe-back gesture fires a popstate event with
  // hasUAVisualTransition=true AND restores the page from bfcache
  // (persisted=true pageshow). Playwright WebKit cannot synthesize this
  // natively; see the LIMITATION NOTE at the top of this file.
  //
  // We approximate by:
  //   1. Calling history.back() to advance the history pointer (fires popstate).
  //   2. Before the popstate fires, injecting hasUAVisualTransition=true on the
  //      event object so the router's onPopState sees it as a swipe-back event
  //      and takes the "swap" fallback code path.
  //
  // This tests the router branch: `transition(..., hasUAVisualTransition=true)`
  // → updateDOM with fallback="swap" (skips animation, calls updateDOM directly).
  //
  // LIMITATION: This does NOT test the bfcache/pageshow re-seed path that
  // `onPageShow` handles. Cases 1 and 2 exercise real goBack() which triggers
  // the full popstate flow including bfcache restoration under WebKit.
  await page.evaluate(() => {
    // Intercept the next popstate event and stamp hasUAVisualTransition on it
    // before the router's listener sees it.
    window.addEventListener(
      "popstate",
      (ev) => {
        // hasUAVisualTransition is read-only on the real event in production;
        // here we use Object.defineProperty to override it for the simulation.
        try {
          Object.defineProperty(ev, "hasUAVisualTransition", {
            value: true,
            configurable: true,
          });
        } catch {
          // If the property cannot be defined (strict environments), fall through —
          // the test still exercises the goBack() path, just without the flag.
        }
      },
      // Capture phase: fires before the router's bubble-phase popstate listener.
      { capture: true, once: true },
    );
    history.back();
  });

  // Wait for the router to complete the "swap" fallback navigation.
  await waitForHeading(page, "List");
  const snapAfterSwipeBack = await historySnapshot(page);

  // URL must be list.html, NOT index.html (home) — same assertion as Cases 1 + 2.
  expect(snapAfterSwipeBack.href).toContain("list.html");
  expect(snapAfterSwipeBack.stateIndex).toBe(snapList.stateIndex);
});
