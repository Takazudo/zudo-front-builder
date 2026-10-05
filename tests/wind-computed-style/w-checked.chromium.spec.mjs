import { expect, test } from "@playwright/test";
import { openFixture, readReport } from "./helpers.mjs";

test("checked peer opens and closes a CSS-only drawer at the responsive breakpoint", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 800 });
  await openFixture(page, "w-checked");

  const checkbox = page.locator("#nav-toggle");
  const overlay = page.locator("#overlay");
  const drawer = page.locator("#drawer");
  await expect(checkbox).not.toBeChecked();
  await expect(checkbox).toHaveCSS("opacity", "0");
  await expect(overlay).toHaveCSS("opacity", "0");
  await expect(overlay).toHaveCSS("pointer-events", "none");
  await expect(drawer).toHaveCSS("visibility", "hidden");

  await page.locator("#nav-label").click();
  await expect(checkbox).toBeChecked();
  await expect(checkbox).toHaveCSS("opacity", "1");
  await expect(overlay).toHaveCSS("opacity", "1");
  await expect(overlay).toHaveCSS("pointer-events", "auto");
  await expect(drawer).toHaveCSS("visibility", "visible");
  expect(await drawer.evaluate((element) => getComputedStyle(element).translate)).toMatch(
    /^0px(?: 0px)?$/,
  );

  await overlay.click();
  await expect(checkbox).not.toBeChecked();
  await expect(overlay).toHaveCSS("opacity", "0");
  await expect(drawer).toHaveCSS("visibility", "hidden");

  const report = await readReport("w-checked");
  expect(report.hasErrors).toBe(false);
  expect(report.diagnostics).toEqual([]);
  expect(report.rules).toEqual(
    expect.arrayContaining([
      "checked:opacity-100",
      "peer-checked:opacity-100",
      "peer-checked:pointer-events-auto",
      "peer-checked:translate-x-0",
      "max-sm:peer-checked:visible",
    ]),
  );
});
