/** R-A09: real ClientRouter swaps with owned island roots and persist metadata. */
import { test, expect } from "@playwright/test";

async function navigate(page, link, heading) {
  const finished = page.evaluate(
    () =>
      new Promise((resolve) => {
        document.addEventListener("zfb:page-load", () => resolve(true), { once: true });
      }),
  );
  await page.locator(link).click();
  await finished;
  await expect(page.locator("h1")).toHaveText(heading);
}

test("disposal, equal-prop persistence, changed-prop remount and Back", async ({ page }) => {
  await page.goto("/nav-a.html");
  for (const name of ["DisposableProbe", "EqualProbe", "ChangedProbe"]) {
    await expect(page.locator(`[data-zfb-island="${name}"]`)).toHaveAttribute(
      "data-zfb-island-mounted",
      "",
    );
  }
  await page.locator("#equal-button").click();
  await page.locator("#changed-button").click();
  await expect(page.locator("#equal-button")).toHaveText("same: 1");
  await expect(page.locator("#changed-button")).toHaveText("old: 1");
  await expect
    .poll(() => page.evaluate(() => window.__builtSiteProof.disposable.ticks))
    .toBeGreaterThan(0);
  await page.evaluate(() => {
    window.__equalNode = document.querySelector('[data-zfb-island="EqualProbe"]');
    window.__changedNode = document.querySelector('[data-zfb-island="ChangedProbe"]');
  });
  await navigate(page, "#to-b", "Navigation B");
  expect(
    await page.evaluate(
      () => window.__equalNode === document.querySelector('[data-zfb-island="EqualProbe"]'),
    ),
  ).toBe(true);
  expect(
    await page.evaluate(
      () => window.__changedNode === document.querySelector('[data-zfb-island="ChangedProbe"]'),
    ),
  ).toBe(true);
  await expect(page.locator("#equal-button")).toHaveText("same: 1");
  await expect(page.locator("#changed-button")).toHaveText("new: 0");
  expect(await page.evaluate(() => window.__builtSiteProof)).toMatchObject({
    disposable: { mounts: 1, cleanups: 1 },
    equal: { mounts: 1, cleanups: 0 },
    changed: { mounts: 2, cleanups: 1 },
  });
  const ticks = await page.evaluate(() => window.__builtSiteProof.disposable.ticks);
  await page.evaluate(
    () =>
      new Promise((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => requestAnimationFrame(resolve))),
      ),
  );
  expect(await page.evaluate(() => window.__builtSiteProof.disposable.ticks)).toBe(ticks);
  const backFinished = page.evaluate(
    () =>
      new Promise((resolve) => {
        document.addEventListener("zfb:page-load", () => resolve(true), { once: true });
      }),
  );
  await page.goBack();
  await backFinished;
  await expect(page.locator("h1")).toHaveText("Navigation A");
  await expect(page.locator("#equal-button")).toHaveText("same: 1");
  await expect(page.locator("#changed-button")).toHaveText("old: 0");
  await expect(page.locator("#disposable-button")).toHaveText("Disposable: 0");
  expect(await page.evaluate(() => window.__builtSiteProof)).toMatchObject({
    disposable: { mounts: 2, cleanups: 1 },
    equal: { mounts: 1, cleanups: 0 },
    changed: { mounts: 3, cleanups: 2 },
  });
});
