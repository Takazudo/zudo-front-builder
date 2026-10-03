import { expect, test } from "@playwright/test";
import { openFixture, readStylesheet } from "./helpers.mjs";

const RED = "rgb(255, 0, 0)";
const ACCENT = "rgb(0, 170, 0)";

// The same markup and authored CSS under both placements. Only an
// equal-specificity tie between an unlayered authored rule and a utility
// flips; every other cascade outcome is identical.
const placements = [
  { fixture: "w-placement-after", tie: ACCENT, padding: "16px" },
  { fixture: "w-placement-before", tie: RED, padding: "1px" },
];

for (const { fixture, tie, padding } of placements) {
  test(`${fixture} resolves the cascade matrix by computed style`, async ({ page }) => {
    await openFixture(page, fixture);

    await expect(page.locator("#tie")).toHaveCSS("color", tie);
    await expect(page.locator("#media-tie")).toHaveCSS("padding-left", padding);
    await expect(page.locator("#dark-tie")).toHaveCSS("color", tie);

    await expect(page.locator("#stronger")).toHaveCSS("color", RED);
    await expect(page.locator("#layered")).toHaveCSS("color", ACCENT);
    await expect(page.locator("#forced")).toHaveCSS("color", RED);
    await expect(page.locator("#reset-target")).toHaveCSS("box-sizing", "content-box");

    await expect(page.locator("#hover-tie")).toHaveCSS("color", RED);
    await page.locator("#group").hover();
    await expect(page.locator("#hover-tie")).toHaveCSS("color", tie);

    // group-hover keeps its (0,1,0) selector, so an authored (0,2,0) rule
    // wins under either placement.
    await page.locator(".panel").hover();
    await expect(page.locator("#relation")).toHaveCSS("color", RED);
  });
}

test("placement swaps only the authored and utility stages", async () => {
  const after = await readStylesheet("w-placement-after");
  const before = await readStylesheet("w-placement-before");
  expect(before.length).toBe(after.length);
  for (const css of [after, before]) {
    expect(css.indexOf("@layer zw-reset, zw-tokens")).toBe(0);
    expect(css.indexOf("@layer zw-reset {")).toBeLessThan(css.indexOf(".tie {"));
    expect(css.indexOf("@layer zw-tokens {")).toBeLessThan(css.indexOf(".text-accent"));
    expect(css).toContain(":where(.group:hover) .group-hover\\:text-accent");
    expect(css).not.toContain("@layer utilities");
  }
  expect(after.indexOf(".tie {")).toBeLessThan(after.indexOf(".text-accent"));
  expect(before.indexOf(".text-accent")).toBeLessThan(before.indexOf(".tie {"));
});
