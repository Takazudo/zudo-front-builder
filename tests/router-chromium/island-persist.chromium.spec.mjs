/**
 * island-persist.chromium.spec.mjs — Level-4 (real browser) proof that the
 * client-router's data-zfb-transition-persist contract (#1389) holds against a
 * REAL (non-stub) island mount/unmount lifecycle.
 *
 * ============================================================
 * WHY THIS SUITE EXISTS
 * ============================================================
 * Every other fixture page in this directory maps @takazudo/zfb/runtime to
 * island-stubs.js — three no-ops (see that file). That is precisely the blind
 * spot that let the pre-#1389 bug ship silently: unmountIslands() unmounted
 * EVERY island before a body swap, including ones about to be persisted,
 * defeating data-zfb-transition-persist entirely — but a no-op unmount can
 * never observe that regression, because it does nothing whether it was
 * called correctly or not.
 *
 * island-a.html / island-b.html instead map @takazudo/zfb/runtime to
 * island-real.js, a small real counter-island mount/unmount module. This spec
 * drives a genuine SPA navigation between them and asserts:
 *   - the persisted counter island keeps its exact DOM node identity AND its
 *     live counter value across the swap (never unmounted, never reset);
 *   - the non-persisted counter island is cleanly unmounted (its cleanup
 *     thunk runs) and cleanly re-mounted fresh on the incoming page (reset to
 *     0, and its click handler works again afterwards — proving the remount
 *     is functional, not just a DOM node that happens to still be present).
 *
 * ============================================================
 * PRE-FIX vs POST-FIX (statically verified against the #1389 diff — this
 * environment does not build/run a second copy of the router to prove it live)
 * ============================================================
 * router.ts's #1389 fix (commit 428984c6) changed exactly one call site:
 *   pre-fix:  unmountIslands()
 *   post-fix: unmountIslands(document.body, preparationEvent.newDocument.body)
 * island-real.js's unmountIslands(root, incomingBody) mirrors that contract
 * precisely: with no incomingBody, nothing is preserved — every counter,
 * persisted or not, is unmounted before the swap (this is deliberately
 * "the pre-#1389 behavior", per that file's own comment). mountNewIslands()
 * only skips re-mounting elements still present in its `mounted` map, so an
 * island unmounted (and dropped from the map) here is treated as brand-new
 * markup on the incoming page and remounted from scratch — resetting its
 * visible counter to 0. Against a router.js built from the pre-#1389 call
 * site, this spec's core assertion (the persisted counter keeps its non-zero
 * value across the swap) would therefore fail. Against the current
 * (post-#1389) router.js on this branch it passes, because the 2-arg call
 * lets island-real.js's unmountIslands skip the persisted counter.
 *
 * ============================================================
 * DEFLAKING DISCIPLINE (issue #1346) — same rules as router.chromium.spec.mjs
 * ============================================================
 * The harness below (init script + wait helpers) is a trimmed, byte-for-byte
 * copy of the relevant parts of router.chromium.spec.mjs, per that file's
 * documented precedent of duplicating shared harness plumbing across sibling
 * Level-4 suites rather than coupling them through a shared module. Every wait
 * is keyed on the router's own `zfb:page-load` lifecycle event (which fires
 * strictly after mountNewIslands() returns — see router.ts) or on Playwright's
 * auto-retrying locator assertions. No bare `waitForTimeout`, no
 * `networkidle`.
 */

// @ts-check
import { test, expect } from "@playwright/test";

const HARNESS_INIT = () => {
  // @ts-expect-error - test-only globals
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
  const rec = { events: [], seq: 0 };
  // @ts-expect-error
  window.__zfb = rec;
  for (const name of NAMES) {
    document.addEventListener(name, () => {
      rec.events.push({ name, seq: ++rec.seq });
    });
  }
};

test.beforeEach(async ({ page }) => {
  await page.addInitScript(HARNESS_INIT);
});

/** Current lifecycle sequence counter — capture BEFORE triggering a navigation. */
async function seqNow(page) {
  return page.evaluate(() => /** @type {any} */ (window).__zfb?.seq ?? 0);
}

/**
 * Wait until a navigation is fully finished: `zfb:page-load` recorded with
 * seq > `after`, and `<html>` no longer carries `data-zfb-transition`.
 * mountNewIslands() runs synchronously just before `zfb:page-load` is
 * dispatched (router.ts's updateDOM -> viewTransition.updateCallbackDone
 * .finally), so waiting for this event guarantees the swap's unmount/mount
 * calls have already completed.
 */
async function waitForNavDone(page, after, timeout = 8000) {
  await page.waitForFunction(
    ({ after }) =>
      /** @type {any} */ (window.__zfb?.events ?? []).some(
        (e) => e.name === "zfb:page-load" && e.seq > after,
      ),
    { after },
    { timeout },
  );
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

// ===========================================================================
// Spec — persisted island survives a real swap with its DOM node and live
// counter state intact; the non-persisted island unmounts and remounts clean.
// ===========================================================================
test("a persisted counter island keeps its DOM node and live state across a router swap; the non-persisted one cleanly unmounts and remounts", async ({
  page,
}) => {
  await page.goto("/island-a.html");
  await waitForInitialLoad(page);
  await expect(page.locator("h1")).toHaveText("Island A");

  // Both counter islands mounted exactly once on initial load; nothing
  // unmounted yet.
  expect(await page.evaluate(() => /** @type {any} */ (window).__islandDebug)).toEqual({
    mounts: 2,
    unmounts: 0,
  });

  // Tag the persisted node so we can prove node IDENTITY (not just a matching
  // id) survives the swap — mirrors the L2 persist-island-lifecycle test's
  // `expect(sidebarAfter).toBe(sidebarEl)`.
  await page.evaluate(() => {
    document.querySelector("#counter-persist")?.setAttribute("data-identity-tag", "tagged-in-a");
  });

  // Drive both counters to distinct non-zero values before navigating.
  await page.click("#persist-inc");
  await page.click("#persist-inc");
  await expect(page.locator("#persist-value")).toHaveText("2");
  await page.click("#plain-inc");
  await expect(page.locator("#plain-value")).toHaveText("1");

  const s = await seqNow(page);
  await page.click("#to-island-b");
  await waitForNavDone(page, s);
  await expect(page.locator("h1")).toHaveText("Island B");

  // Persisted counter: same physical DOM node (identity tag survived) and its
  // live counter value was never reset — proof its "component instance" was
  // never torn down, only physically relocated by swapBodyElement.
  await expect(page.locator("#counter-persist")).toHaveAttribute(
    "data-identity-tag",
    "tagged-in-a",
  );
  await expect(page.locator("#persist-value")).toHaveText("2");

  // Non-persisted counter: a fresh instance on the incoming page, reset to 0,
  // carrying none of the outgoing page's markers.
  await expect(page.locator("#plain-value")).toHaveText("0");
  expect(
    await page.evaluate(() =>
      document.querySelector("#counter-plain")?.hasAttribute("data-identity-tag"),
    ),
  ).toBe(false);

  // The remount is functional, not just present: clicking the fresh instance's
  // button still increments — an island torn down but never remounted would
  // leave no listener wired up to replace the old one.
  await page.click("#plain-inc");
  await expect(page.locator("#plain-value")).toHaveText("1");

  // Exactly one unmount happened across the swap (the plain counter, cleaned
  // up before the swap); the persisted counter was skipped. Mount count: 2
  // initial + 1 fresh remount of the plain counter on the incoming page = 3 —
  // the persisted counter's `mounted` entry survived (same element), so
  // mountNewIslands did not re-mount it.
  expect(await page.evaluate(() => /** @type {any} */ (window).__islandDebug)).toEqual({
    mounts: 3,
    unmounts: 1,
  });

  // And the persisted counter is still live and functional after the swap.
  await page.click("#persist-inc");
  await expect(page.locator("#persist-value")).toHaveText("3");
});
