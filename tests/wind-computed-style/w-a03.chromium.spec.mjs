import { expect, test } from "@playwright/test";
import { openFixture, readReport } from "./helpers.mjs";

test("W-A03 applies a configured breakpoint on its exact threshold", async ({ page }) => {
  await openFixture(page, "w-a03");
  const target = page.locator("#breakpoint");

  await page.setViewportSize({ width: 639, height: 800 });
  await expect(target).toHaveCSS("display", "none");
  await page.setViewportSize({ width: 640, height: 800 });
  await expect(target).toHaveCSS("display", "block");
});

test("W-A03 guards hover utilities by the browser context capability", async ({
  browser,
  page,
}) => {
  await openFixture(page, "w-a03");
  const desktopCanHover = await page.evaluate(() => matchMedia("(hover: hover)").matches);
  expect(desktopCanHover).toBe(true);

  const desktopTarget = page.locator("#hover-target");
  await desktopTarget.hover();
  expect(await desktopTarget.evaluate((element) => element.matches(":hover"))).toBe(true);
  await expect(desktopTarget).toHaveCSS("background-color", "rgb(18, 52, 86)");

  const touchContext = await browser.newContext({
    viewport: { width: 390, height: 844 },
    isMobile: true,
    hasTouch: true,
  });
  try {
    const touchPage = await touchContext.newPage();
    await openFixture(touchPage, "w-a03");
    const touchCanHover = await touchPage.evaluate(() => matchMedia("(hover: hover)").matches);
    expect(touchCanHover).toBe(false);

    const touchTarget = touchPage.locator("#hover-target");
    await touchTarget.hover();
    expect(await touchTarget.evaluate((element) => element.matches(":hover"))).toBe(true);
    await expect(touchTarget).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  } finally {
    await touchContext.close();
  }
});

test("W-A03 uses keyboard focus-visible state and root or ancestor dark matching", async ({
  page,
}) => {
  await openFixture(page, "w-a03");
  await page.keyboard.press("Tab");
  const focusTarget = page.locator("#focus-visible");
  await expect(focusTarget).toBeFocused();
  await expect(focusTarget).toHaveCSS("background-color", "rgb(18, 52, 86)");

  await expect(page.locator("#dark-root")).toHaveCSS("background-color", "rgb(18, 52, 86)");
  await expect(page.locator("#dark-descendant")).toHaveCSS("background-color", "rgb(18, 52, 86)");
});

test("W-A03 follows the layer cascade and reports rejected variant order", async ({ page }) => {
  await openFixture(page, "w-a03");
  await expect(page.locator("#layered-utility")).toHaveCSS("color", "rgb(18, 52, 86)");
  await expect(page.locator("#late-unlayered")).toHaveCSS("color", "rgb(0, 128, 0)");

  const report = await readReport("w-a03");
  expect(report.diagnostics).toContainEqual(
    expect.objectContaining({
      candidate: "hover:sm:block",
      code: "ZW003",
      suggestion: "sm:hover:block",
    }),
  );
});
