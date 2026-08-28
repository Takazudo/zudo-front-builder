/**
 * back-race.chromium.spec.mjs — Level-4 (real browser) regression proof for
 * Back during the early history-commit/pre-swap window (#2603 / #2619).
 *
 * ============================================================
 * WHY THIS SUITE EXISTS
 * ============================================================
 * The Level-2 router-vt-history test for #2603 hand-feeds a popstate while a
 * navigation is inside its before-swap callback. That proves the abort
 * boundary's branch logic, but it cannot prove that a real Chromium Back
 * gesture observes the URL committed before the View Transition update
 * callback runs. This spec waits for that real early URL commit while the old
 * body is still present, then issues Back immediately.
 *
 * The regression is specifically about the newer Back traversal winning over
 * the superseded forward navigation: the final URL and body must remain on A,
 * the forward transition must report navigation-aborted, and its aborted
 * update callback must never emit after-swap or page-load.
 *
 * ============================================================
 * DEFLAKING DISCIPLINE (issue #1346)
 * ============================================================
 * Every wait is keyed on router lifecycle events, router-set transition state,
 * or Playwright's auto-retrying URL/DOM assertions. There are NO bare
 * waitForTimeout calls and NO waitForLoadState('networkidle') calls. The
 * lifecycle recorder is installed with addInitScript before the fixture's
 * router module runs, so the early before-preparation event cannot be missed.
 */

// @ts-check
import { test, expect } from "@playwright/test";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

// ---------------------------------------------------------------------------
// Harness installed before every document, before any page script runs.
// ---------------------------------------------------------------------------
//
// SPA swaps preserve this recorder and its sequence counter. That lets the
// test distinguish the forward click's preparation from the Back traversal's
// fast path, which intentionally emits no preparation event.
const HARNESS_INIT = () => {
  // @ts-expect-error - test-only global
  if (window.__zfbHarness) return;
  // @ts-expect-error
  window.__zfbHarness = true;

  const NAMES = [
    "zfb:before-preparation",
    "zfb:after-preparation",
    "zfb:before-swap",
    "zfb:after-swap",
    "zfb:page-load",
    "zfb:navigation-aborted",
  ];
  const rec = { events: [], timeline: [], seq: 0, timelineSeq: 0 };
  // @ts-expect-error
  window.__zfb = rec;
  // @ts-expect-error
  window.__zfbBackRaceMark = (name, detail = {}) => {
    rec.timeline.push({
      name,
      seq: ++rec.timelineSeq,
      at: performance.now(),
      url: location.href,
      heading: document.querySelector("h1")?.textContent ?? null,
      ...detail,
    });
  };
  for (const name of NAMES) {
    document.addEventListener(name, (ev) => {
      const entry = {
        name,
        seq: ++rec.seq,
        at: performance.now(),
        direction:
          ev && typeof (/** @type {any} */ (ev).direction) === "string"
            ? /** @type {any} */ (ev).direction
            : null,
        navigationType:
          ev && typeof (/** @type {any} */ (ev).navigationType) === "string"
            ? /** @type {any} */ (ev).navigationType
            : null,
      };
      rec.events.push(entry);
      // @ts-expect-error
      window.__zfbBackRaceMark(`event:${name}`, {
        eventSeq: entry.seq,
        direction: entry.direction,
        navigationType: entry.navigationType,
      });
    });
  }
  addEventListener("popstate", (event) => {
    // @ts-expect-error
    window.__zfbBackRaceMark("browser-popstate-dispatch", {
      stateIndex: /** @type {any} */ (event.state)?.index ?? null,
    });
  });
};

test.beforeEach(async ({ page }) => {
  await page.addInitScript(HARNESS_INIT);
});

test.afterEach(async ({ page }, testInfo) => {
  const evidenceDir = process.env.ZFB_BACK_RACE_EVIDENCE_DIR;
  if (!evidenceDir) return;

  let pageEvidence;
  try {
    pageEvidence = await page.evaluate(() => {
      const rec = /** @type {any} */ (window).__zfb;
      return {
        url: location.href,
        heading: document.querySelector("h1")?.textContent ?? null,
        events: rec?.events ?? [],
        timeline: rec?.timeline ?? [],
      };
    });
  } catch (error) {
    pageEvidence = { captureError: error instanceof Error ? error.message : String(error) };
  }

  const evidence = {
    title: testInfo.title,
    repeatEachIndex: testInfo.repeatEachIndex,
    status: testInfo.status,
    expectedStatus: testInfo.expectedStatus,
    durationMs: testInfo.duration,
    error: testInfo.error?.message ?? null,
    ...pageEvidence,
  };
  await mkdir(evidenceDir, { recursive: true });
  const filename = `back-race-${String(testInfo.repeatEachIndex + 1).padStart(2, "0")}-${testInfo.status}.json`;
  await writeFile(join(evidenceDir, filename), `${JSON.stringify(evidence, null, 2)}\n`, "utf8");
});

// ---------------------------------------------------------------------------
// Event/state-keyed wait helpers (no bare timeouts, no networkidle).
// ---------------------------------------------------------------------------

/** Current lifecycle sequence counter — capture before triggering a navigation. */
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

/** Wait for the initial full-page load's `zfb:page-load` (router booted). */
async function waitForInitialLoad(page) {
  await page.waitForFunction(() =>
    /** @type {any} */ (window.__zfb?.events ?? []).some((e) => e.name === "zfb:page-load"),
  );
}

/** Names (in sequence order) of lifecycle events recorded with seq > `after`. */
async function eventNamesSince(page, after) {
  return page.evaluate((after) => {
    const rec = /** @type {any} */ (window).__zfb;
    return rec.events.filter((e) => e.seq > after).map((e) => e.name);
  }, after);
}

// ===========================================================================
// Spec — Back wins after the forward navigation commits its URL but before
// the View Transition update callback swaps the body. (#2603)
// ===========================================================================
test("Back wins during the early-commit/pre-swap window", async ({ page }) => {
  await page.goto("/page-a.html");
  await waitForInitialLoad(page);
  await expect(page.locator("h1")).toHaveText("Page A");

  // Capture the sequence before the forward click. The one preparation event
  // asserted below must belong to this navigation, not the initial load.
  const beforeClick = await seqNow(page);
  await page.click("#to-page-b");

  // The router commits B's history entry before the View Transition update
  // callback runs. waitForURL therefore resolves while the old A body is still
  // live — the precise window in which a real Back can supersede the forward.
  await page.waitForURL(/\/page-b\.html$/);
  await expect(page.locator("h1")).toHaveText("Page A");

  // Capture the post-commit/pre-swap baseline immediately before Back. The
  // aborted event below is emitted by the superseded forward update callback;
  // its after-swap/page-load events must never appear after this baseline.
  const beforeBack = await seqNow(page);
  await page.evaluate(() => {
    // @ts-expect-error
    window.__zfbBackRaceMark?.("playwright-go-back-dispatch");
  });
  await page.goBack();

  // Back takes the same-page traverse fast path, while the forward update
  // callback observes its aborted signal and emits the router's settle signal.
  await waitForZfbEvent(page, "zfb:navigation-aborted", beforeBack);
  await page.waitForFunction(
    () => !document.documentElement.hasAttribute("data-zfb-transition"),
    undefined,
    { timeout: 8000 },
  );

  // The newer Back wins in both URL and DOM. These assertions are deliberately
  // after abort/transition cleanup so a stale superseded swap cannot overwrite
  // the result later.
  await expect(page).toHaveURL(/\/page-a\.html$/);
  await expect(page.locator("h1")).toHaveText("Page A");

  const afterClick = await eventNamesSince(page, beforeClick);
  // Exactly one preparation belongs to the forward click. The Back traversal
  // is same-page and therefore uses the fast path without another preparation.
  expect(afterClick.filter((name) => name === "zfb:before-preparation")).toHaveLength(1);

  const afterBack = await eventNamesSince(page, beforeBack);
  // The superseded forward navigation must not commit its body or run the
  // post-swap page-load lifecycle after Back has taken ownership.
  expect(afterBack.filter((name) => name === "zfb:after-swap" || name === "zfb:page-load")).toEqual(
    [],
  );
});
