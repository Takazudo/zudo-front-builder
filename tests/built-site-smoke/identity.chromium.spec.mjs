/** R-A02/R-A11: exact DOM node adoption from real, held zfb build output. */
import { test, expect } from "@playwright/test";

test("SSR nodes survive hydration while the skip-SSR control is replaced", async ({ page }) => {
  let release = () => {};
  const held = new Promise((resolve) => {
    release = resolve;
  });
  let requested = false;
  const entry = /\/assets\/islands[^/]*\.js(?:\?.*)?$/;
  await page.route(entry, async (route) => {
    requested = true;
    await held;
    await route.continue();
  });
  const navigation = page.goto("/identity.html", { waitUntil: "domcontentloaded" });
  try {
    await expect.poll(() => requested).toBe(true);
    await page.evaluate(() => {
      const selectors = [
        '[data-zfb-island="IdentityProbe"]',
        "#adjacent-first",
        "#fragment-a-first",
        "#fragment-b-first",
        "#identity-second",
        "#skip-fallback",
      ];
      const nodes = selectors.map((selector) => document.querySelector(selector));
      if (nodes.some((node) => !node)) throw new Error("missing pre-hydration witness");
      const adjacentChildren = [...nodes[1].childNodes];
      if (adjacentChildren.length < 3) throw new Error("missing adjacent text/marker witnesses");
      window.__identityBefore = { nodes, adjacentChildren };
    });
    await expect(page.locator('[data-zfb-island="IdentityProbe"]')).toHaveCount(2);
    await expect(page.locator('[data-zfb-island="IdentityProbe"]').first()).not.toHaveAttribute(
      "data-zfb-island-mounted",
      "",
    );
    release();
    await navigation;
    await expect(page.locator('[data-zfb-island="IdentityProbe"]').first()).toHaveAttribute(
      "data-zfb-island-mounted",
      "",
    );
    await expect(page.locator("#skip-client")).toBeVisible();
    const identity = await page.evaluate(() => {
      const selectors = [
        '[data-zfb-island="IdentityProbe"]',
        "#adjacent-first",
        "#fragment-a-first",
        "#fragment-b-first",
        "#identity-second",
      ];
      const before = window.__identityBefore;
      return {
        elements: selectors.map(
          (selector, i) =>
            before.nodes[i] === document.querySelector(selector) && before.nodes[i].isConnected,
        ),
        adjacentChildren: before.adjacentChildren.every(
          (node, i) =>
            node === document.querySelector("#adjacent-first").childNodes[i] && node.isConnected,
        ),
      };
    });
    expect(identity).toEqual({ elements: [true, true, true, true, true], adjacentChildren: true });
    expect(
      await page.evaluate(
        () =>
          window.__identityBefore.nodes[5] !== document.querySelector("#skip-client") &&
          !window.__identityBefore.nodes[5].isConnected,
      ),
    ).toBe(true);
    await expect(page.locator("#adjacent-first")).toHaveText("leadleft-firstright-firsttail");
  } finally {
    release();
    await navigation.catch(() => undefined);
    await page.unroute(entry);
  }
});
