import { expect, test } from "@playwright/test";
import { openFixture, readReport, readStylesheet } from "./helpers.mjs";

test("W-A01 emits stable CSS and explanations for reordered candidates", async ({ page }) => {
  const firstCss = await readStylesheet("w-a01");
  const secondCss = await readStylesheet("w-a01", "wind-order-2.css");
  const report = await readReport("w-a01");

  expect(firstCss).toBe(secondCss);
  expect(report.determinism.sameStylesheet).toBe(true);
  expect(report.determinism.sameExplanations).toBe(true);
  expect(report.explanations).toEqual(report.determinism.reversedExplanations);

  const malformed = report.diagnostics.find((diagnostic) => diagnostic.candidate === "w-[red]");
  expect(malformed).toMatchObject({
    candidate: "w-[red]",
    severity: "error",
    code: "ZW005",
    origin: {
      kind: "manifest",
      producer: "wind-fixture-css",
      path: "w-a01/case.json",
      index: 0,
    },
  });

  await openFixture(page, "w-a01");
  const padding = page.locator("#padding");
  await expect(padding).toHaveCSS("padding-top", "16px");
  await expect(padding).toHaveCSS("padding-right", "8px");
  await expect(padding).toHaveCSS("padding-bottom", "16px");
  await expect(padding).toHaveCSS("padding-left", "4px");
  await expect(page.locator("#color")).toHaveCSS("color", "rgb(18, 52, 86)");
});
