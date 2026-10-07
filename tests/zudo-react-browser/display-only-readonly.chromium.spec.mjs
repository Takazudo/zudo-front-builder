// @ts-check
import { test, expect } from "@playwright/test";
import { expectNoBrowserErrors, openHeldScenario, releaseScenario } from "./browser-helpers.mjs";

test("display-only readonly controls hydrate, follow a computed value, and reset to their SSR values", async ({
  page,
}) => {
  const state = await openHeldScenario(page, "display-only-readonly");
  try {
    const input = page.locator("#display-only-input");
    const textarea = page.locator("#display-only-textarea");
    const initial = "DISPLAY-ONLY INITIAL";

    await expect(input).toHaveAttribute("value", initial);
    await expect(textarea).toHaveText(initial);
    expect(await textarea.evaluate((node) => node.innerHTML)).toBe(initial);

    await releaseScenario(page, state);
    await expect(input).toHaveValue(initial);
    await expect(textarea).toHaveValue(initial);
    expect(
      await page.evaluate(() => window.__zudoReactBrowser.roots[0].result.display.value),
    ).toBe(initial);

    await page.locator("#display-only-update").click();
    await expect(input).toHaveValue("UPDATED BY BUTTON");
    await expect(textarea).toHaveValue("UPDATED BY BUTTON");

    await input.focus();
    await page.keyboard.press("End");
    await page.keyboard.type(" typed by user");
    await expect(input).toHaveValue("UPDATED BY BUTTON");
    expect(
      await page.evaluate(() => ({
        source: window.__zudoReactBrowser.roots[0].result.source.value,
        display: window.__zudoReactBrowser.roots[0].result.display.value,
      })),
    ).toEqual({ source: "updated by button", display: "UPDATED BY BUTTON" });

    await page.locator("#display-only-form").evaluate((form) => form.reset());
    await expect(input).toHaveValue(initial);
    await expect(textarea).toHaveValue(initial);
    expect(
      await page.evaluate(() => window.__zudoReactBrowser.roots[0].result.source.value),
    ).toBe("updated by button");

    expectNoBrowserErrors(state.errors);
  } finally {
    await releaseScenario(page, state).catch(() => undefined);
    await state.heldModule.remove();
  }
});
