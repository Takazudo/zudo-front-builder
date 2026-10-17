import { test, expect } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

const prefix = process.env.WIND_DOCS_BASE ?? "/";
const route = (slug, ja = false) => `${prefix}${ja ? "ja/" : ""}docs/${slug}`;

for (const width of [1440, 390]) {
  for (const ja of [false, true]) {
    test(`workflow navigation ${ja ? "JA" : "EN"} ${width}`, async ({ page, request }) => {
      await page.setViewportSize({ width, height: 950 });
      const failures = [];
      page.on("pageerror", (error) => failures.push(error.message));
      await page.goto(route("build-with-zfb", ja));
      await expect(page.locator("main h1")).toHaveText(ja ? "zfb で作る" : "Build with zfb");
      await expect(page.getByRole("navigation", { name: "Breadcrumb", exact: true })).toBeVisible();
      const pages = page
        .locator(`main a[href="${route("build-with-zfb/pages-routes", ja)}"]`)
        .first();
      await pages.click();
      await expect(page.locator("main h1")).toHaveText(
        ja ? "ページとルーティング" : "Pages & routes",
      );
      await page.goBack();
      await expect(page.locator("main h1")).toHaveText(ja ? "zfb で作る" : "Build with zfb");
      await page.goForward();
      await expect(page.locator("main h1")).toHaveText(
        ja ? "ページとルーティング" : "Pages & routes",
      );
      await page
        .locator(`main a[href="${route("architecture/routing", ja)}"]`)
        .first()
        .click();
      if (width < 1024) {
        await page.getByRole("button", { name: /Open sidebar|サイドバーを開く/ }).click();
      }
      const sidebar = page.locator(width < 1024 ? "[data-zd-mobile-sidebar]" : "#desktop-sidebar");
      await expect(sidebar.locator(`a[href="${route("build-with-zfb", ja)}"]`)).toBeVisible();
      if (width < 1024) {
        await sidebar.locator(`a[lang="${ja ? "en" : "ja"}"]`).click();
      } else {
        const lang = page.locator("header [data-language-switcher]:visible").first();
        await lang.locator("[data-language-toggle]").click();
        await lang.locator(`a[lang="${ja ? "en" : "ja"}"]`).click();
      }
      await expect(page.locator("html")).toHaveAttribute("lang", ja ? "en" : "ja");
      await expect(page).toHaveURL(new RegExp(`${ja ? "" : "/ja"}/docs/architecture/routing/?$`));

      await page.goto(route("api/cli", ja) + "#zfb-wind");
      await expect(page.locator('[id="zfb-wind"]')).toBeVisible();
      await page.locator("header [data-open-search]:visible").first().click();
      await page.locator("[data-search-input]:visible").fill("zfb wind");
      await expect(page.locator("[data-search-dialog] a[href*='/docs/']").first()).toBeVisible();
      await page.locator("[data-close-search]:visible").click();

      await page.goto(route("deploy-embed", ja));
      await expect(page.locator("main h1")).toHaveText(
        ja ? "デプロイと組み込み" : "Deploy & Embed",
      );
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
        true,
      );
      for (const href of await page
        .locator("main nav a[href]")
        .evaluateAll((links) => links.map((a) => a.getAttribute("href")))) {
        const response = await request.get(href);
        expect(response.status(), href).toBe(200);
      }
      const screenshotDir = process.env.WIND_DOCS_SCREENSHOT_DIR;
      if (screenshotDir) {
        mkdirSync(screenshotDir, { recursive: true });
        await page.screenshot({
          path: join(screenshotDir, `workflow-${ja ? "ja" : "en"}-${width}.png`),
          fullPage: true,
        });
      }
      expect(failures).toEqual([]);
    });
  }
}

test("fallback, legacy anchors and discovery retain their contracts", async ({ page, request }) => {
  await page.goto(route("architecture/render-artifacts"));
  const title = await page.locator("main h1").textContent();
  await page.goto(route("architecture/render-artifacts", true));
  await expect(page.locator("html")).toHaveAttribute("lang", "ja");
  await expect(page.locator("main h1")).toHaveText(title);
  for (const ja of [false, true]) {
    const anchor = ja ? "6-つのエンジンプリミティブ" : "the-six-engine-primitives";
    await page.goto(route("architecture/engine-vs-framework", ja) + "#" + anchor);
    await expect(page.locator(`[id="${anchor}"]`)).toHaveCount(1);
  }
  const sitemap = await (await request.get(`${prefix}sitemap.xml`)).text();
  const llms = await (await request.get(`${prefix}llms.txt`)).text();
  const jaLlms = await (await request.get(`${prefix}ja/llms.txt`)).text();
  for (const slug of ["build-with-zfb", "deploy-embed"]) {
    expect(sitemap).toContain(`/docs/${slug}`);
    expect(llms).toContain(`/docs/${slug}`);
    expect(jaLlms).toContain(`/ja/docs/${slug}`);
  }
});
