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
 *   3. The Counter wrapper has no mounted marker while the islands entry is
 *      held, then receives it after the real mount completes.
 *
 * The `[data-zfb-island="Counter"]` selector targets the wrapper `<div>`
 * emitted by the `<Island>` JSX component (packages/zfb/src/island.ts) —
 * the same marker attribute crates/zfb-islands/src/hydration.rs documents
 * and crates/zfb/src/commands/island_marker_check.rs cross-checks against
 * the islands registry at build time.
 *
 * The second test (issue #1405) is the direct #1385 pt.1 repro: the Gallery
 * island imports a module that calls `import.meta.glob(...)`. Before #1404 the
 * emitted client bundle carried the raw `import.meta.glob(...)` call and threw
 * at hydration; after #1404's islands-shadow materialisation the glob is
 * expanded at build time. "Gallery items: 2" proves the two ./gallery/*.tsx
 * modules were collected; the button responding to a click proves the
 * glob-consuming island actually hydrated.
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

  // Hold the real build's islands entry before it can execute. A first
  // assertion after an ordinary page.goto() is too late: the entry can load
  // and mount the island before the test gets its first turn, making the
  // "before" check tautological and unable to catch an SSR-time write.
  let releaseIslandsModule = () => {};
  const islandsModuleHeld = new Promise((resolve) => {
    releaseIslandsModule = resolve;
  });
  let islandsModuleRequested = false;
  const islandsModuleRoute = /\/assets\/islands[^/]*\.js(?:\?.*)?$/;
  await page.route(islandsModuleRoute, async (route) => {
    islandsModuleRequested = true;
    await islandsModuleHeld;
    await route.continue();
  });

  const navigation = page.goto("/", { waitUntil: "domcontentloaded" });
  try {
    await expect.poll(() => islandsModuleRequested).toBe(true);

    const counterIsland = page.locator('[data-zfb-island="Counter"]');
    // The SSR wrapper is already in the DOM while the islands module is held,
    // so this is a genuine pre-hydration observation.
    await expect(counterIsland).toHaveCount(1);
    expect(await counterIsland.getAttribute("data-zfb-island-mounted")).toBeNull();

    releaseIslandsModule();
    await navigation;
    await expect(counterIsland).toHaveAttribute("data-zfb-island-mounted", "");

    const counterButton = counterIsland.locator("button");

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
  } finally {
    // Always unblock and settle navigation before removing the route, so a
    // failed assertion cannot leave the held request or its promise behind.
    releaseIslandsModule();
    await navigation.catch(() => undefined);
    await page.unroute(islandsModuleRoute);
  }
});

test("glob-consuming island (import.meta.glob) builds, expands, and hydrates — #1385 pt.1", async ({
  page,
}) => {
  /** @type {string[]} */
  const pageErrors = [];
  /** @type {string[]} */
  const consoleErrors = [];
  page.on("pageerror", (err) => {
    pageErrors.push(err.message);
  });
  page.on("console", (msg) => {
    if (msg.type() === "error") {
      consoleErrors.push(msg.text());
    }
  });

  await page.goto("/");

  // Build-time proof: import.meta.glob expanded to the two ./gallery/*.tsx
  // modules (red + blue). Rendered in SSR markup and again after hydration.
  const count = page.locator('[data-zfb-island="Gallery"] #gallery-count');
  await expect(count).toHaveText("Gallery items: 2");

  // Hydration proof: the glob-consuming island's button only cycles the
  // selection if the emitted bundle loaded and ran. A pre-#1404 bundle would
  // have thrown at hydration on the raw import.meta.glob(...) call.
  const nextButton = page.locator('[data-zfb-island="Gallery"] #gallery-next');
  await expect(nextButton).toHaveText("Selected: blue");
  await nextButton.click();
  await expect(nextButton).toHaveText("Selected: red");
  await nextButton.click();
  await expect(nextButton).toHaveText("Selected: blue");

  expect(pageErrors, `page errors fired during load/hydration: ${pageErrors.join("; ")}`).toEqual(
    [],
  );
  expect(
    consoleErrors,
    `console.error calls fired during load/hydration: ${consoleErrors.join("; ")}`,
  ).toEqual([]);
});
