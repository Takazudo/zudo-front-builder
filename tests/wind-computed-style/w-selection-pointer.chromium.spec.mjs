import { expect, test } from "@playwright/test";
import { openFixture, readReport } from "./helpers.mjs";

test("selection color and pointer media follow browser capability", async ({ page, browser }) => {
  await openFixture(page, "w-selection-pointer");
  const selectionColor = await page
    .locator("#selection")
    .evaluate((element) => getComputedStyle(element, "::selection").backgroundColor);
  expect(selectionColor).toBe("rgb(18, 52, 86)");
  await expect(page.locator("#fine")).toHaveCSS("background-color", "rgb(18, 52, 86)");
  await expect(page.locator("#coarse")).not.toHaveCSS("background-color", "rgb(18, 52, 86)");

  const coarseContext = await browser.newContext({ hasTouch: true });
  try {
    const coarsePage = await coarseContext.newPage();
    await coarsePage.goto(page.url());
    await expect(coarsePage.locator("#coarse")).toHaveCSS("background-color", "rgb(18, 52, 86)");
    await expect(coarsePage.locator("#fine")).not.toHaveCSS("background-color", "rgb(18, 52, 86)");
  } finally {
    await coarseContext.close();
  }

  const report = await readReport("w-selection-pointer");
  expect(report.hasErrors).toBe(false);
  expect(report.rules).toEqual(
    expect.arrayContaining([
      "selection:bg-panel",
      "pointer-coarse:bg-panel",
      "pointer-fine:bg-panel",
    ]),
  );
});
