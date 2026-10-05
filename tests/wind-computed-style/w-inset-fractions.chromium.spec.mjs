import { readFile } from "node:fs/promises";
import { expect, test } from "@playwright/test";
import { openFixture, readReport } from "./helpers.mjs";

async function relativePosition(page, selector) {
  const containingBlock = await page.locator("#containing-block").boundingBox();
  const target = await page.locator(selector).boundingBox();
  return { left: target.x - containingBlock.x, top: target.y - containingBlock.y };
}

test("all inset roots compute percentage offsets against a known containing block", async ({
  page,
}) => {
  await openFixture(page, "w-inset-fractions");

  for (const [selector, property, value] of [
    ["#inset", "left", "100px"],
    ["#inset", "top", "60px"],
    ["#inset-x", "left", "100px"],
    ["#inset-y", "top", "60px"],
    ["#top", "top", "60px"],
    ["#right", "right", "100px"],
    ["#bottom", "bottom", "60px"],
    ["#left", "left", "100px"],
  ]) {
    await expect(page.locator(selector)).toHaveCSS(property, value);
  }

  for (const [selector, left, top] of [
    ["#inset", 100, 60],
    ["#inset-x", 100, 0],
    ["#inset-y", 0, 60],
    ["#top", 0, 60],
    ["#right", 80, 0],
    ["#bottom", 0, 40],
    ["#left", 100, 0],
    ["#negative", -100, 0],
  ]) {
    expect(await relativePosition(page, selector)).toEqual({ left, top });
  }

  await page.locator("#containing-block").hover();
  await expect(page.locator("#variant")).toHaveCSS("top", "60px");
  expect(await relativePosition(page, "#variant")).toMatchObject({ top: 60 });
});

test("inset fraction fixture resolves all roots, negatives, and variants", async () => {
  const report = await readReport("w-inset-fractions");
  const catalog = JSON.parse(
    await readFile(
      new URL("../../crates/zudo-wind/catalog/zudo-wind-catalog.v1.json", import.meta.url),
      "utf8",
    ),
  );

  for (const [candidate, entryIdentifier] of [
    ["inset-1/2", "v1.inset"],
    ["inset-x-1/2", "v1.inset-x"],
    ["inset-y-1/2", "v1.inset-y"],
    ["top-1/2", "v1.top"],
    ["right-1/2", "v1.right"],
    ["bottom-1/2", "v1.bottom"],
    ["left-1/2", "v1.left"],
    ["-left-1/2", "v1.left"],
    ["group-hover:top-1/2", "v1.top"],
  ]) {
    expect(report.explanations[candidate]).toMatchObject({
      outcome: "resolved_utility",
      entryIdentifier,
      specRevision: catalog.specRevision,
    });
    expect(report.diagnostics.some((diagnostic) => diagnostic.candidate === candidate)).toBe(false);
  }
  expect(report.hasErrors).toBe(false);
});
