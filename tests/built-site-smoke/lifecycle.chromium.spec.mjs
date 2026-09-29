/** R-A04/R-A07: real emitted modules and zfb's own second mount pass/deferred scheduler. */
import { test, expect } from "@playwright/test";

test("a repeated zfb mount pass does not activate a root twice", async ({ page }) => {
  await page.goto("/identity/index.html");
  await expect(page.locator('[data-zfb-island="IdentityProbe"]').first()).toHaveAttribute(
    "data-zfb-island-mounted",
    "",
  );
  await page.evaluate(() => window.__proofActivateAgain());
  expect(await page.evaluate(() => window.__builtSiteProof.first.mounts)).toBe(1);
  await page.locator("#identity-click-first").click();
  await expect(page.locator("#adjacent-first")).toHaveText("leadleft-first!right-firsttail");
  expect(await page.evaluate(() => window.__builtSiteProof.first.mounts)).toBe(1);
});

test("a removed deferred island never activates when its media trigger fires", async ({ page }) => {
  await page.addInitScript(() => {
    const original = window.matchMedia.bind(window);
    window.matchMedia = (query) => {
      if (query !== "(min-width: 99999px)") return original(query);
      const listeners = new Set();
      const media = {
        media: query,
        matches: false,
        onchange: null,
        addEventListener: (_type, callback) => listeners.add(callback),
        removeEventListener: (_type, callback) => listeners.delete(callback),
        addListener: (callback) => listeners.add(callback),
        removeListener: (callback) => listeners.delete(callback),
        dispatchEvent: (event) => {
          listeners.forEach((callback) => callback(event));
          return true;
        },
      };
      window.__fireDeferred = () => {
        media.matches = true;
        media.dispatchEvent({ matches: true, media: query });
      };
      return media;
    };
  });
  await page.goto("/identity/index.html");
  await expect(page.locator('[data-zfb-island="IdentityProbe"]').first()).toHaveAttribute(
    "data-zfb-island-mounted",
    "",
  );
  expect(await page.evaluate(() => window.__builtSiteProof?.deferred?.mounts ?? 0)).toBe(0);
  await page.evaluate(() => {
    document.querySelector('[data-zfb-island="DeferredProbe"]')?.remove();
    window.__fireDeferred();
  });
  await expect
    .poll(() => page.evaluate(() => window.__builtSiteProof?.deferred?.mounts ?? 0))
    .toBe(0);
  await expect(page.locator("#deferred-client")).toHaveCount(0);
});
