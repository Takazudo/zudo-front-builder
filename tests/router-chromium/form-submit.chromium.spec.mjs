/**
 * form-submit.chromium.spec.mjs — Level-4 (real browser) confirm gate for
 * form-submit interception (issue #1400).
 *
 * ============================================================
 * WHY THIS SUITE EXISTS
 * ============================================================
 * `packages/zfb-runtime/src/__tests__/client-router/router.test.ts`'s
 * "handleSubmit — form intercept (#1400)" describe block dispatches
 * `SubmitEvent`s directly via happy-dom's `form.requestSubmit()` and asserts
 * on the mocked `fetch` call — it proves handleSubmit's BRANCH LOGIC (GET
 * query serialization, POST body, formaction/formmethod overrides, the
 * dialog/cross-origin/modifier-click skips), not that a real `<form>`
 * submission gesture in a real browser produces a genuine SPA navigation with
 * no full-page reload. This file is that proof for the GET path — the most
 * common real-world form (a search box).
 *
 * ============================================================
 * DEFLAKING DISCIPLINE (issue #1346) — same rules as router.chromium.spec.mjs
 * ============================================================
 * Every wait is keyed on the router's OWN lifecycle events or on state the
 * router itself sets — NO bare `waitForTimeout`, NO `networkidle`. The
 * harness below (init script + helpers) is intentionally a trimmed copy of
 * router.chromium.spec.mjs's, mirroring the tests/webkit-back-history vs.
 * tests/router-chromium precedent of deliberately duplicating shared harness
 * plumbing across sibling Level-4 suites rather than coupling them through a
 * shared module (see serve-fixture.mjs's header comment in this directory).
 */

// @ts-check
import { test, expect } from "@playwright/test";

// ---------------------------------------------------------------------------
// Harness installed before every document, before any page script runs.
// (Trimmed from router.chromium.spec.mjs — see that file's header for the
// full rationale of each piece; this spec only needs the lifecycle recorder
// and the per-document __docId, not the View Transitions instrumentation.)
// ---------------------------------------------------------------------------
const HARNESS_INIT = () => {
  // @ts-expect-error - test-only globals
  if (window.__zfbHarness) return;
  // @ts-expect-error
  window.__zfbHarness = true;

  // Unique per REAL document. An SPA nav preserves it (same JS context); a
  // full browser reload mints a new one — the signal a form submit must NOT
  // have caused a real page load.
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
      rec.events.push({ name, seq: ++rec.seq });
      void ev;
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

// ===========================================================================
// Spec — submitting a GET (search-style) form is a real SPA navigation with
// the fields serialized into the query string, not a full page load.
// ===========================================================================
test("submitting a GET search form performs a SPA navigation with the serialized query", async ({
  page,
}) => {
  await page.goto("/search.html");
  await waitForInitialLoad(page);
  await expect(page.locator("h1")).toHaveText("Search");

  const docIdBefore = await page.evaluate(() => /** @type {any} */ (window).__docId);

  await page.fill("#q", "hello world");

  const s = await seqNow(page);
  await page.click("#search-submit");
  await waitForNavDone(page, s);

  // Content swapped to the results page ...
  await expect(page.locator("h1")).toHaveText("Search Results");
  // ... the URL carries both the typed field and the form's own hidden field,
  // serialized via URLSearchParams (spaces become "+", not "%20") ...
  const url = new URL(page.url());
  expect(url.pathname).toBe("/search-results.html");
  expect(url.searchParams.get("q")).toBe("hello world");
  expect(url.searchParams.get("category")).toBe("docs");
  // ... and it was a client swap, NOT a document reload: the JS context (and
  // its per-document id) survived. A full reload would have minted a new
  // __docId — the same proof router.chromium.spec.mjs's Spec 1 uses for link
  // navigation.
  const docIdAfter = await page.evaluate(() => /** @type {any} */ (window).__docId);
  expect(docIdAfter).toBe(docIdBefore);

  // The full documented lifecycle fired, in order, for this navigation (a
  // form submit goes through the exact same transition() pipeline as a link
  // click) — same style as router.chromium.spec.mjs's Spec 2.
  const names = await page.evaluate(
    (after) =>
      /** @type {any} */ (window.__zfb.events).filter((e) => e.seq > after).map((e) => e.name),
    s,
  );
  const expectedOrder = [
    "zfb:before-preparation",
    "zfb:after-preparation",
    "zfb:before-swap",
    "zfb:after-swap",
    "zfb:page-load",
  ];
  const positions = expectedOrder.map((n) => names.indexOf(n));
  expect(
    positions.every((p) => p >= 0),
    `missing events; saw ${names.join(",")}`,
  ).toBe(true);
  for (let i = 1; i < positions.length; i++) {
    expect(positions[i]).toBeGreaterThan(positions[i - 1]);
  }
});
