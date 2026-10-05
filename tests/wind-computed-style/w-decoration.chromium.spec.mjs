import { expect, test } from "@playwright/test";
import { openFixture, readReport } from "./helpers.mjs";

test("decoration color, thickness, and style apply together and respond to hover", async ({
  page,
}) => {
  await openFixture(page, "w-decoration");
  const link = page.locator("#decorated");
  await expect(link).toHaveCSS("text-decoration-color", "rgb(18, 52, 86)");
  await expect(link).toHaveCSS("text-decoration-thickness", "2px");
  await expect(link).toHaveCSS("text-decoration-style", "dotted");
  await link.hover();
  await expect(link).toHaveCSS("text-decoration-thickness", "3px");
  await expect(link).toHaveCSS("text-decoration-style", "wavy");
});

test("decoration fixture reports all three catalog members without diagnostics", async () => {
  const report = await readReport("w-decoration");
  for (const [candidate, entryIdentifier] of [
    ["decoration-panel", "v1.decoration.color"],
    ["decoration-2", "v1.decoration.thickness"],
    ["decoration-dotted", "v1.decoration.style.dotted"],
    ["hover:decoration-[3px]", "v1.decoration.thickness"],
    ["hover:decoration-wavy", "v1.decoration.style.wavy"],
  ]) {
    expect(report.explanations[candidate]).toMatchObject({
      outcome: "resolved_utility",
      entryIdentifier,
      specRevision: 8,
    });
    expect(report.diagnostics.some((diagnostic) => diagnostic.candidate === candidate)).toBe(false);
  }
});
