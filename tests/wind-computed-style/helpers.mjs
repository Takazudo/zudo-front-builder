import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect } from "@playwright/test";

const SUITE_DIR = dirname(fileURLToPath(import.meta.url));

export async function openFixture(page, fixture) {
  await page.goto(`/fixtures/${fixture}/index.html`, { waitUntil: "load" });
  const stylesheet = page.locator('link[rel="stylesheet"]');
  await expect
    .poll(() =>
      stylesheet.evaluate((link) => Boolean(link.sheet && link.sheet.cssRules.length > 0)),
    )
    .toBe(true);
}

export async function readReport(fixture) {
  const report = await readFile(join(SUITE_DIR, "dist", fixture, "report.json"), "utf8");
  return JSON.parse(report);
}

export async function readStylesheet(fixture, filename = "wind.css") {
  return readFile(join(SUITE_DIR, "dist", fixture, filename), "utf8");
}
