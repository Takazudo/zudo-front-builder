// @ts-check
import { test, expect } from "@playwright/test";
import { expectNoBrowserErrors, openHeldScenario, releaseScenario } from "./browser-helpers.mjs";

test("R-A03 authored hidden CSS computes none, then restores the panel's flex layout", async ({
  page,
}) => {
  const state = await openHeldScenario(page, "r-a03-hidden-panel");
  try {
    await expect
      .poll(() => page.locator("#tab-panel").evaluate((panel) => getComputedStyle(panel).display))
      .toBe("none");
    await releaseScenario(page, state);
    await page.evaluate(async () => {
      window.__zudoReactBrowser.roots[0].result.hidden.value = false;
      await window.__zudoReactBrowser.flush();
    });
    await expect
      .poll(() => page.locator("#tab-panel").evaluate((panel) => getComputedStyle(panel).display))
      .toBe("flex");
    expectNoBrowserErrors(state.errors);
  } finally {
    await releaseScenario(page, state).catch(() => undefined);
    await state.heldModule.remove();
  }
});
