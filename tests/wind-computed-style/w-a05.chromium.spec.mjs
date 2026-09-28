import { expect, test } from "@playwright/test";
import { openFixture, readReport, readStylesheet } from "./helpers.mjs";

test("W-A05 generates a complete class literal that is applied only after a click", async ({
  page,
}) => {
  await openFixture(page, "w-a05");
  const target = page.locator("#client-state");

  await expect(target).toHaveAttribute("class", "");
  await expect(target).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await page.locator("#activate").click();
  await expect(target).toHaveAttribute("class", "bg-panel");
  await expect(target).toHaveCSS("background-color", "rgb(18, 52, 86)");
});

test("W-A05 leaves ordinary and allowlisted authored classes without generated rules or errors", async ({
  page,
}) => {
  await openFixture(page, "w-a05");
  const report = await readReport("w-a05");
  const stylesheet = await readStylesheet("w-a05");

  expect(report.ordinaryClasses).toContain("custom-card");
  expect(report.rules).not.toContain("custom-card");
  expect(report.diagnostics.some((diagnostic) => diagnostic.candidate === "custom-card")).toBe(
    false,
  );
  expect(
    report.audit.diagnostics.some((diagnostic) => diagnostic.candidate === "custom-card"),
  ).toBe(false);

  expect(report.authoredClasses).toContain("p-4");
  expect(report.rules).not.toContain("p-4");
  expect(report.diagnostics.some((diagnostic) => diagnostic.candidate === "p-4")).toBe(false);
  expect(report.audit.diagnostics.some((diagnostic) => diagnostic.candidate === "p-4")).toBe(false);
  expect(report.audit.unrecognizedClasses.some((entry) => entry.candidate === "p-4")).toBe(false);
  expect(stylesheet).not.toContain(".p-4");
  await expect(page.locator("#authored")).toHaveCSS("padding-top", "0px");
});

test("W-A05 records dynamic construction without generating its completed class", async ({
  page,
}) => {
  await openFixture(page, "w-a05");
  const report = await readReport("w-a05");
  const stylesheet = await readStylesheet("w-a05");
  expect(report.audit.dynamicConstructions).toContainEqual(
    expect.objectContaining({ text: "bg-", sourceId: "fixtures/w-a05/index.html" }),
  );
  expect(report.rules).not.toContain("bg-missing");
  expect(stylesheet).not.toContain(".bg-missing");

  const dynamic = page.locator("#dynamic-state");
  await page.locator("#activate-dynamic").click();
  await expect(dynamic).toHaveAttribute("class", "bg-missing");
  await expect(dynamic).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
});
