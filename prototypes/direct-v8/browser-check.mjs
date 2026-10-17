// One bounded #4240 hydration representative; never installs a browser.
import { readFileSync, writeFileSync } from "node:fs";
import { chromium, expect } from "@playwright/test";

const [origin, expectationsPath, executablePath, output] = process.argv.slice(2);
if (!origin || !expectationsPath || !executablePath || !output) {
  throw new Error("usage: node browser-check.mjs ORIGIN EXPECTATIONS CHROMIUM OUTPUT");
}
const expected = JSON.parse(readFileSync(expectationsPath, "utf8")).chromium;
const pageErrors = [];
const consoleErrors = [];
const report = { executablePath, origin, passed: false };
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
  const response = await page.goto(origin + expected.url, {
    waitUntil: "networkidle",
    timeout: 15_000,
  });
  expect(response.status()).toBe(200);
  await expect(page.locator(expected.initial.selector)).toHaveText(expected.initial.text);
  report.initialText = await page.locator(expected.initial.selector).textContent();
  await expect(page.locator(expected.css.selector)).toHaveCSS(
    expected.css.property,
    expected.css.equals,
  );
  report.css = await page
    .locator(expected.css.selector)
    .evaluate(
      (element, property) => getComputedStyle(element).getPropertyValue(property),
      expected.css.property,
    );
  expect(expected.interaction.action).toBe("click");
  await page.locator(expected.interaction.selector).click();
  await expect(page.locator(expected.interaction.selector)).toHaveText(
    expected.interaction.expected_text,
  );
  report.afterClickText = await page.locator(expected.interaction.selector).textContent();
  expect(pageErrors).toEqual(expected.page_errors);
  expect(consoleErrors).toEqual(expected.console_errors);
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
