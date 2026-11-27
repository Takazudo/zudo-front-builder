import { expect, test } from "@playwright/test";

// Request-time SSR pages built from the same Layout as the prerendered page
// load the finalized CSS, explicit client script and islands entry (#4887).
const VARIANTS = [
  { name: "root", origin: "http://127.0.0.1:4331", base: "" },
  { name: "subpath", origin: "http://127.0.0.1:4332", base: "/sub" },
];

for (const variant of VARIANTS) {
  test(`${variant.name}: SSR page uses the shipped assets and hydrates once`, async ({ page }) => {
    const pageErrors = [];
    const consoleErrors = [];
    const failed = [];
    const assetResponses = [];
    page.on("pageerror", (error) => pageErrors.push(error.message));
    page.on("console", (message) => {
      if (message.type() === "error") consoleErrors.push(message.text());
    });
    page.on("requestfailed", (request) => failed.push(request.url()));
    page.on("response", (response) => {
      if (response.url().includes("/assets/")) {
        assetResponses.push({ url: response.url(), status: response.status() });
      }
    });

    await page.goto(`${variant.origin}${variant.base}/live`);
    await expect(page.locator("[data-page=live]")).toHaveText("rendered per request");

    // CSS from the hashed stylesheet is applied.
    await expect(page.locator("h1.shared-marker")).toHaveCSS("color", "rgb(12, 34, 56)");
    // The explicit client script ran.
    await expect(page.locator("html")).toHaveAttribute("data-greeting", "ready");
    // The unreferenced client script did not.
    await expect(page.locator("html")).not.toHaveAttribute("data-unused", /.*/);

    // The island hydrates once and responds.
    const counter = page.locator("[data-counter]");
    await expect(counter).toHaveText("Live page: 0");
    await counter.click();
    await expect(counter).toHaveText("Live page: 1");
    // The lazy chunk shares module state with the eager island.
    const lazy = page.locator("[data-lazy]");
    await lazy.click();
    await expect(lazy).toHaveText("lazy owner=eager claims=1");
    expect(await page.evaluate(() => globalThis.__ssrSharedAssetsMounts)).toBe(1);

    expect(assetResponses.length).toBeGreaterThanOrEqual(3);
    for (const response of assetResponses) {
      expect(response.status, response.url).toBe(200);
      expect(response.url).toMatch(/-[0-9a-z]{8}\.(css|js)$/);
      expect(response.url).toContain(`${variant.base}/assets/`);
    }
    expect(failed).toEqual([]);
    expect(pageErrors).toEqual([]);
    expect(consoleErrors).toEqual([]);
  });
}
