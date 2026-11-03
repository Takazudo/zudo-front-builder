import { test, expect } from "@playwright/test";

for (const status of [404, 500, 503]) {
  test(`fetch prefetch retries HTTP ${status}, then deduplicates success`, async ({ page }) => {
    const path = `/prefetch-recovery-${status}`;
    let requests = 0;
    await page.route(`**${path}`, async (route) => {
      requests++;
      await route.fulfill({ status: requests === 1 ? status : 200, body: "prefetch response" });
    });
    await page.goto("/search.html");
    await page.evaluate(async () => {
      const { prefetch } = await import("/dist/client-router/index.js");
      window.__prefetch = prefetch;
      window.__fetchCalls = 0;
      window.__fetchSettled = 0;
      const original = window.fetch;
      window.fetch = async (...args) => {
        window.__fetchCalls++;
        const response = await original(...args);
        window.__fetchSettled++;
        return response;
      };
    });
    const trigger = () =>
      page.evaluate((path) => {
        window.__prefetch(path, { with: "fetch" });
        window.__prefetch(path, { with: "fetch" });
      }, path);
    await trigger();
    await page.waitForFunction(() => window.__fetchSettled === 1);
    expect(await page.evaluate(() => window.__fetchCalls)).toBe(1);
    await trigger();
    await page.waitForFunction(() => window.__fetchSettled === 2);
    await trigger();
    expect(await page.evaluate(() => window.__fetchCalls)).toBe(2);
    expect(requests).toBe(2);
  });
}
