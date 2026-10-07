import { test, expect } from "@playwright/test";
test("fresh bundled starters retain design, routes and blog theme", async ({ page }) => {
  test.skip(
    !process.env.ZFB_BASIC_BLOG_DIST || !process.env.ZFB_NODE_FREE_DIST,
    "Build fresh starters and supply both dist paths",
  );
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  for (const [port, name] of [
    [4341, "basic-blog"],
    [4342, "node-free"],
  ]) {
    await page.goto(`http://127.0.0.1:${port}/`);
    await expect(page.locator("h1")).toContainText(name);
    await expect(page.locator("body")).toHaveCSS("font-size", "16px");
    await expect(page.locator("body")).toHaveCSS("line-height", "26.4px");
    await expect(page.locator("html")).toHaveCSS("--ds-brand", "#3159d8");
    await expect(page.getByText(/Use 16px within vertical groups/)).toBeVisible();
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 1000 });
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth))
        .toBe(true);
      await page.screenshot({
        path: `/tmp/zfb-${name}-${width}.png`,
        fullPage: true,
        animations: "disabled",
      });
    }
    if (name === "basic-blog") {
      await page.getByRole("button", { name: "Switch to dark theme" }).click();
      await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
      await expect(page.locator("body")).not.toHaveCSS("background-color", "rgb(244, 247, 251)");
      await page.reload();
      await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
      await page.goto(`http://127.0.0.1:${port}/blog/styling-with-zudo-wind/`);
      await expect(page.locator("h1")).toHaveText("How this site is styled");
      await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
    } else {
      await expect(page.locator("[data-zfb-island]")).toHaveCount(0);
      await page.goto(`http://127.0.0.1:${port}/posts/hello/`);
      await expect(page.locator("h1")).toHaveText("Hello, zfb");
      await expect(page.locator("strong")).toContainText("node-free");
    }
  }
  expect(errors).toEqual([]);
});
