import { test, expect } from "@playwright/test";
import { readFile } from "node:fs/promises";
import {
  createState,
  getSeedFiles,
  setValue,
  zipData,
} from "../../docs/src/components/playground/design-workshop/model.js";

const root = (page) => page.locator("#design-workshop");
async function open(page, locale = "") {
  await page.goto(`${locale}/docs/playground/design-system/`);
  await expect(root(page).locator('[data-preset="everyday"]')).toBeVisible();
}
async function edit(page) {
  await root(page).locator('[data-view="playground"]').first().click();
  await expect(root(page).locator("#accent-hex")).toBeVisible();
}
const frame = (page) => page.frameLocator("#design-preview");

test("native EN/JA hydration, exact current exports, local interactions and independent spacing", async ({
  page,
}) => {
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  for (const locale of ["", "/ja"]) {
    await open(page, locale);
    await expect(page.locator("h1")).toHaveCount(1);
    await edit(page);
    for (const preset of ["everyday", "editorial", "workbench"]) {
      await root(page).locator("#editor-preset").selectOption(preset);
      await root(page).locator("#accent-hex").fill("#963142");
      await root(page).locator("#group-gap").fill("24");
      await expect(frame(page).locator(".cards")).toHaveCSS(
        "column-gap",
        preset === "editorial" ? "20px" : preset === "workbench" ? "12px" : "16px",
      );
      await expect(frame(page).locator(".cards")).toHaveCSS("row-gap", "24px");
      await root(page).locator("#accent-hex").fill("#xx");
      await expect(root(page).locator("#accent-hex")).toHaveAttribute("aria-invalid", "true");
      await expect(frame(page).locator("html")).toHaveCSS("--ds-brand", "#963142");
      await root(page).locator("#accent-hex").blur();
      await expect(root(page).locator("#accent-hex")).toHaveValue("#963142");
      const expected = createState(preset);
      setValue(expected, "accent", "#963142");
      setValue(expected, "groupGap", 24);
      await root(page).locator('[data-action="export"]').first().click();
      await root(page).locator('[data-export-file="design-system.css"]').click();
      await root(page).locator('[data-action="copy-file"]').click();
      await expect
        .poll(() => page.evaluate(() => navigator.clipboard.readText()))
        .toBe(getSeedFiles(expected)["design-system.css"]);
      const downloadPromise = page.waitForEvent("download");
      await root(page).locator("#download-seed").click();
      const download = await downloadPromise;
      expect(Buffer.from(await readFile(await download.path()))).toEqual(
        Buffer.from(zipData(getSeedFiles(expected))),
      );
      await root(page).locator("#export-dialog [data-close]").click();
      for (const mode of ["components", "foundations", "page"]) {
        await root(page).locator(`[data-preview="${mode}"]`).click();
        await expect(frame(page).locator("html")).toHaveCSS("--ds-brand", "#963142");
        await expect(frame(page).locator("body")).not.toBeEmpty();
      }
      await root(page).locator('[data-action="reset"]').click();
      await expect(root(page).locator("#accent-hex")).toHaveValue(
        createState(preset).values.accent,
      );
      await expect(root(page).locator("#group-gap")).toHaveValue(
        String(createState(preset).values.groupGap),
      );
    }
    await root(page).locator('[data-role="body"]').focus();
    await page.keyboard.press("Enter");
    await expect(root(page).locator("#token-chain")).toContainText("--ds-font-body");
    await root(page).locator('[data-action="inspect"]').click();
    await root(page).locator('[data-view="starter"]').first().click();
    const starter = page.frameLocator("#starter-preview");
    await expect(starter.locator("body")).not.toHaveClass(/inspect-on/);
    const url = page.url();
    await starter.getByRole("link", { name: "Journal", exact: true }).click();
    expect(page.url()).toBe(url);
    await starter.locator("#sample-email").fill("local@example.com");
    await starter.getByRole("button", { name: "Try signup" }).click();
    await expect(starter.locator("#signup-status")).toContainText("No signup was submitted");
  }
  expect(errors).toEqual([]);
});

test("native navigation restores one live workshop and retains editing state", async ({ page }) => {
  await open(page);
  await edit(page);
  await root(page).locator("#editor-preset").selectOption("editorial");
  await root(page).locator("#accent-hex").fill("#963142");
  await root(page).locator('[data-width="mobile"]').click();
  await page.evaluate(() => {
    window.__workshopNavigationSentinel = "alive";
    window.__oldWorkshopRoot = document.querySelector("#design-workshop");
  });
  const indexLink = page
    .locator('a[href="/docs/playground/"]:visible,a[href="/docs/playground"]:visible')
    .filter({ hasText: /Playground|Index/ })
    .first();
  await indexLink.click();
  await expect(page).toHaveURL(/\/docs\/playground\/?$/);
  await expect(root(page)).toHaveCount(0);
  expect(await page.evaluate(() => window.__oldWorkshopRoot.childElementCount)).toBe(0);
  await page
    .getByRole("link", { name: /Open design playground|デザイン.*開く/ })
    .first()
    .click();
  await expect(root(page).locator("#accent-hex")).toHaveValue("#963142");
  await expect(root(page).locator("#editor-preset")).toHaveValue("editorial");
  await expect(root(page).locator("#frame-shell")).toHaveClass(/phone/);
  expect(await page.evaluate(() => window.__workshopNavigationSentinel)).toBe("alive");
  await expect(root(page)).toHaveCount(1);
  await expect(page.locator('[data-zfb-island="DesignSystemPlayground"]')).toHaveCount(1);
  await root(page).locator('[data-action="inspect"]').click();
  await expect(root(page).locator("#inspect-button")).toHaveAttribute("aria-pressed", "true");
});

test("desktop and narrow host layout keep seed appearance separate from docs theme", async ({
  page,
}) => {
  await open(page);
  await edit(page);
  await root(page).locator("#editor-preset").selectOption("editorial");
  const controlColor = await root(page).evaluate((el) => getComputedStyle(el).color);
  await page.locator("[data-zd-theme-menu]:visible > button").first().click();
  await page.getByRole("menuitemradio", { name: "Dark", exact: true }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect
    .poll(() => root(page).evaluate((el) => getComputedStyle(el).color))
    .not.toBe(controlColor);
  await expect(frame(page).locator("body")).toHaveCSS("background-color", "rgb(250, 248, 242)");
  await page.evaluate(() => scrollTo(0, 0));
  await page.screenshot({ path: "/tmp/zfb-workshop-dark.png", fullPage: true });
  await page.locator("[data-zd-theme-menu]:visible > button").first().click();
  await page.getByRole("menuitemradio", { name: "Light", exact: true }).click();
  for (const width of [1440, 768, 390]) {
    await page.setViewportSize({ width, height: 1000 });
    await expect
      .poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth))
      .toBe(true);
    await expect(frame(page).locator("body")).toHaveCSS("background-color", "rgb(250, 248, 242)");
    await root(page).locator('[data-width="mobile"]').click();
    expect(
      await root(page)
        .locator("#design-preview")
        .evaluate((el) => el.getBoundingClientRect().width),
    ).toBeLessThanOrEqual(390);
    await page.evaluate(() => scrollTo(0, 0));
    await page.screenshot({ path: `/tmp/zfb-workshop-${width}.png`, fullPage: true });
    await root(page).locator('[data-width="full"]').click();
  }
});
