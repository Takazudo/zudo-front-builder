// Load one page and wait for a selector's text; never installs a browser.
import { writeFileSync } from "node:fs";
import { chromium, expect } from "@playwright/test";

const [url, selector, text, executablePath, output] = process.argv.slice(2);
if (!url || !selector || !text || !executablePath || !output) {
  throw new Error("usage: node browser-text.mjs URL SELECTOR TEXT CHROMIUM OUTPUT");
}
const pageErrors = [];
const consoleErrors = [];
const report = { url, selector, expected: text, passed: false };
let browser;
try {
  browser = await chromium.launch({ executablePath, headless: true });
  report.version = browser.version();
  const page = await browser.newPage();
  page.setDefaultTimeout(10_000);
  page.on("pageerror", (error) => pageErrors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  const response = await page.goto(url, { waitUntil: "load", timeout: 15_000 });
  expect(response.status()).toBe(200);
  await expect(page.locator(selector)).toHaveText(text);
  report.text = await page.locator(selector).textContent();
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
  report.passed = true;
} catch (error) {
  report.error = String(error.stack ?? error);
} finally {
  report.pageErrors = pageErrors;
  report.consoleErrors = consoleErrors;
  if (browser) await browser.close();
  writeFileSync(output, JSON.stringify(report, null, 2) + "\n");
}
if (!report.passed) process.exitCode = 1;
