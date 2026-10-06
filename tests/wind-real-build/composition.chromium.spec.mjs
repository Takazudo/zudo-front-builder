import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { expect, test } from "@playwright/test";

const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");

test("public build serves the generated stylesheet and synthetic composition", async ({ page }) => {
  await page.setViewportSize({ width: 1120, height: 800 });
  await page.goto("/", { waitUntil: "networkidle" });
  const stylesheet = page.locator('link[rel="stylesheet"]').first();
  const href = await stylesheet.getAttribute("href");
  expect(href).toMatch(/^\/assets\/styles-[^/]+\.css$/);
  const served = await page.request.get(href);
  expect(served.status()).toBe(200);
  expect(served.headers()["content-type"]).toBe("text/css; charset=utf-8");
  const bytes = await served.body();
  expect(bytes.byteLength).toBeGreaterThan(100);
  expect(hash(bytes)).toMatch(/^[0-9a-f]{64}$/);
  const generated = await readFile(resolve(process.env.ZFB_WIND_REAL_BUILD_DIST, href.slice(1)));
  expect(hash(bytes)).toBe(hash(generated));
  expect(bytes.toString()).toContain(".shipping-layout");
  expect(bytes.toString()).toContain(".bg-asset-probe");

  const card = page.locator("#shipping-card");
  await expect(card).toHaveCSS("background-color", "rgb(51, 102, 153)");
  await expect(card.locator("h2")).toHaveCSS("font-size", "24px");
  await expect(page.locator("#shipping-layout")).toHaveCSS("grid-template-columns", /\d+px \d+px/);
  await expect(page.locator("#shipping-dark")).toHaveCSS("color", "rgb(247, 247, 247)");
  await expect(page.locator("#shipping-email")).toHaveCSS("border-top-width", "1px");

  const nav = page.locator('#shipping-nav a[href="#shipping-card"]');
  await nav.focus();
  await expect(nav).toHaveCSS("outline-width", "3px");
  const input = page.locator("#shipping-email");
  await input.focus();
  await expect(input).toHaveCSS("outline-width", "2px");
  await input.fill("invalid");
  expect(await input.evaluate((element) => element.checkValidity())).toBe(false);
  await input.fill("fixture@example.invalid");
  expect(await input.evaluate((element) => element.checkValidity())).toBe(true);
  await page.locator("#shipping-submit").hover();
  await expect(page.locator("#shipping-submit")).toHaveCSS("background-color", "rgb(34, 68, 102)");

  await page.setViewportSize({ width: 390, height: 800 });
  const cardBox = await card.boundingBox();
  const formBox = await page.locator("#shipping-form").boundingBox();
  expect(cardBox && formBox && formBox.y > cardBox.y + cardBox.height).toBe(true);
});
