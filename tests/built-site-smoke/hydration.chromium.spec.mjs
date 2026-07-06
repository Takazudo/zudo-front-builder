/**
 * built-site-smoke — Level-4 (real browser, real `zfb build` output)
 * structural guard for the #1385 bug class: an emitted client bundle that
 * throws at hydration. See playwright.config.mjs's header comment for why
 * every other tier is structurally blind to this.
 *
 * Loads the real build output of tests/built-site-smoke/fixture-site/ (built
 * by the CI workflow before Playwright starts — see
 * .github/workflows/node-free-smoke.yml's `built-site-browser-smoke` job) in
 * real Chromium and asserts:
 *
 *   1. Zero page errors (uncaught exceptions) and zero `console.error` calls
 *      during load + hydration — the direct signal that the emitted bundle
 *      did not throw.
 *   2. The Counter island actually hydrated: clicking its button increments
 *      the rendered count. Static SSR markup alone renders "Count: 0" but
 *      cannot respond to a click — only a live, successfully-hydrated
 *      Preact instance can, so this assertion is also the sensitivity check
 *      that a broken bundle (one that loads but throws before attaching
 *      event listeners) fails this lane even when it produces no visible
 *      page error.
 *
 * The `[data-zfb-island="Counter"]` selector targets the wrapper `<div>`
 * emitted by the `<Island>` JSX component (packages/zfb/src/island.ts) —
 * the same marker attribute crates/zfb-islands/src/hydration.rs documents
 * and crates/zfb/src/commands/island_marker_check.rs cross-checks against
 * the islands registry at build time.
 */

// @ts-check
import { test, expect } from "@playwright/test";

test("built site loads with zero page errors and its island hydrates", async ({ page }) => {
  /** @type {string[]} */
  const pageErrors = [];
  /** @type {string[]} */
  const consoleErrors = [];

  // Listeners installed before navigation so no early error can slip through.
  page.on("pageerror", (err) => {
    pageErrors.push(err.message);
  });
  page.on("console", (msg) => {
    if (msg.type() === "error") {
      consoleErrors.push(msg.text());
    }
  });

  await page.goto("/");

  const counterButton = page.locator('[data-zfb-island="Counter"] button');

  // SSR-rendered initial state — present even before hydration completes.
  await expect(counterButton).toBeVisible();
  await expect(counterButton).toHaveText("Count: 0");

  // The actual hydration proof: a click only updates the DOM if the client
  // bundle loaded, ran, and attached a real Preact event listener.
  await counterButton.click();
  await expect(counterButton).toHaveText("Count: 1");

  await counterButton.click();
  await expect(counterButton).toHaveText("Count: 2");

  expect(pageErrors, `page errors fired during load/hydration: ${pageErrors.join("; ")}`).toEqual(
    [],
  );
  expect(
    consoleErrors,
    `console.error calls fired during load/hydration: ${consoleErrors.join("; ")}`,
  ).toEqual([]);
});
