import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { chromium } from "@playwright/test";
const room = process.argv[2];
const browser = await chromium.launch({
  ...(process.env.CHROMIUM_EXECUTABLE ? { executablePath: process.env.CHROMIUM_EXECUTABLE } : {}),
});
try {
  const page = await browser.newPage();
  for (const [placement, expected] of [
    ["after-authored", "flex"],
    ["before-authored", "grid"],
  ]) {
    const css = fs.readFileSync(path.join(room, `${placement}.css`), "utf8");
    await page.setContent(`<style>${css}</style><div class="flex" id="probe">fixture</div>`);
    const actual = await page.evaluate(() => ({
      display: getComputedStyle(document.querySelector("#probe")).display,
      margin: getComputedStyle(document.body).margin,
      sizing: getComputedStyle(document.querySelector("#probe")).boxSizing,
    }));
    assert.equal(actual.display, expected, placement);
    assert.equal(actual.margin, "0px");
    assert.equal(actual.sizing, "border-box");
  }
  console.log("PASS: Chromium computed authored/utility placement and minimal reset");
} finally {
  await browser.close();
}
