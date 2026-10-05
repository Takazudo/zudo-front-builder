import { expect, test } from "@playwright/test";
import { openFixture, readStylesheet } from "./helpers.mjs";

test("bare outline emits 1px solid and outline-2 wins the deterministic cascade", async ({
  page,
}) => {
  await openFixture(page, "w-outline");

  const bare = page.locator("#bare-outline");
  await page.keyboard.press("Tab");
  await expect(bare).toBeFocused();
  await expect(bare).toHaveCSS("outline-width", "1px");
  await expect(bare).toHaveCSS("outline-style", "solid");

  const pair = page.locator("#outline-pair");
  await page.keyboard.press("Tab");
  await expect(pair).toBeFocused();
  await expect(pair).toHaveCSS("outline-width", "2px");
  await expect(pair).toHaveCSS("outline-style", "solid");

  const stylesheet = await readStylesheet("w-outline");
  const bareRule = stylesheet.indexOf(".focus-visible\\:outline:focus-visible {");
  const sizedRule = stylesheet.indexOf(".focus-visible\\:outline-2:focus-visible {");
  expect(bareRule).toBeGreaterThanOrEqual(0);
  expect(sizedRule).toBeGreaterThan(bareRule);
});
