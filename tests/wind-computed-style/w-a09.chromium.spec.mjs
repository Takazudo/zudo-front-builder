import { readFile } from "node:fs/promises";
import { expect, test } from "@playwright/test";
import { openFixture, readReport, readStylesheet } from "./helpers.mjs";

const CURSOR_KEYWORDS = [
  "help",
  "none",
  "progress",
  "crosshair",
  "cell",
  "copy",
  "alias",
  "no-drop",
  "context-menu",
  "vertical-text",
  "all-scroll",
  "zoom-in",
  "zoom-out",
];

const ADDED = [
  ["wrap-anywhere", "v1.wrap-anywhere"],
  ["decoration-panel", "v1.decoration.color"],
  ["hover:decoration-[#ff0000]", "v1.decoration.color"],
  ["leading-none", "v1.leading"],
  ["invisible", "v1.invisible"],
  ["md:visible", "v1.visible"],
  ["cursor-col-resize", "v1.cursor"],
  ["focus:cursor-row-resize", "v1.cursor"],
  ...CURSOR_KEYWORDS.map((keyword) => [`cursor-${keyword}`, "v1.cursor"]),
  ["underline-offset-4", "v1.underline-offset"],
  ["hover:underline-offset-[6px]", "v1.underline-offset"],
];

test("W-A09 applies added utility groups under base, state, and media variants", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1024, height: 768 });
  await openFixture(page, "w-a09");

  await expect(page.locator("#wrap")).toHaveCSS("overflow-wrap", "anywhere");
  await expect(page.locator("#leading")).toHaveCSS("line-height", "20px");

  const decoration = page.locator("#decoration");
  await expect(decoration).toHaveCSS("text-decoration-color", "rgb(18, 52, 86)");
  await decoration.hover();
  await expect(decoration).toHaveCSS("text-decoration-color", "rgb(255, 0, 0)");

  const offset = page.locator("#offset");
  await expect(offset).toHaveCSS("text-underline-offset", "4px");
  await offset.hover();
  await expect(offset).toHaveCSS("text-underline-offset", "6px");

  const cursor = page.locator("#cursor");
  await expect(cursor).toHaveCSS("cursor", "col-resize");
  await cursor.focus();
  await expect(cursor).toHaveCSS("cursor", "row-resize");

  const visibility = page.locator("#visibility");
  await expect(visibility).toHaveCSS("visibility", "visible");
  await page.setViewportSize({ width: 600, height: 768 });
  await expect(visibility).toHaveCSS("visibility", "hidden");
});

test("W-A09 computes each added standard cursor keyword", async ({ page }) => {
  await openFixture(page, "w-a09");

  for (const keyword of CURSOR_KEYWORDS) {
    await expect(page.locator(`#cursor-${keyword}`)).toHaveCSS("cursor", keyword);
  }
});

test("W-A09 reports explain metadata, a clean audit, and deterministic output", async () => {
  const report = await readReport("w-a09");
  const catalog = JSON.parse(
    await readFile(
      new URL("../../crates/zudo-wind/catalog/zudo-wind-catalog.v1.json", import.meta.url),
      "utf8",
    ),
  );
  const firstCss = await readStylesheet("w-a09");
  const secondCss = await readStylesheet("w-a09", "wind-order-2.css");

  expect(firstCss).toBe(secondCss);
  expect(report.determinism.sameStylesheet).toBe(true);
  expect(report.determinism.sameExplanations).toBe(true);

  for (const [candidate, entryIdentifier] of ADDED) {
    expect(report.rules).toContain(candidate);
    expect(report.explanations[candidate]).toMatchObject({
      outcome: "resolved_utility",
      entryIdentifier,
      specRevision: catalog.specRevision,
    });
    expect(report.diagnostics.some((diagnostic) => diagnostic.candidate === candidate)).toBe(false);
    expect(report.audit.diagnostics.some((diagnostic) => diagnostic.candidate === candidate)).toBe(
      false,
    );
  }

  for (const candidate of ["ring-2", "animate-spin"]) {
    expect(report.diagnostics).toContainEqual(
      expect.objectContaining({ candidate, code: "ZW004", severity: "error" }),
    );
    expect(report.rules).not.toContain(candidate);
  }
});
