/** R-A11: one reactive runtime in the emitted shared bundle and its chunks. */
import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { test, expect } from "@playwright/test";

const assets = fileURLToPath(new URL("./fixture-site/dist/assets/", import.meta.url));
const definition = /["']@takazudo\/zfb\/zudo-react\/runtime-definition-v1["']\s*:\s*(?:true|!0)/g;

function definitionCount(sources) {
  return sources.reduce((count, source) => count + [...source.matchAll(definition)].length, 0);
}

test("one runtime definition exists across emitted JavaScript assets", () => {
  const files = readdirSync(assets).filter((name) => name.endsWith(".js"));
  expect(files.some((name) => /^islands[^/]*\.js$/.test(name))).toBe(true);
  const sources = files.map((name) => readFileSync(join(assets, name), "utf8"));
  expect(definitionCount(sources)).toBe(1);
  expect(definitionCount([...sources, ...sources])).toBe(2); // duplication negative control
});

test("two separately imported islands share a live signal", async ({ page }) => {
  await page.goto("/shared-state/index.html");
  await expect(page.locator('[data-zfb-island="SharedWriter"]')).toHaveAttribute(
    "data-zfb-island-mounted",
    "",
  );
  await expect(page.locator('[data-zfb-island="SharedReader"]')).toHaveAttribute(
    "data-zfb-island-mounted",
    "",
  );
  await expect(page.locator("#shared-reader")).toHaveText("Read: 0");
  await page.locator("#shared-writer").click();
  await expect(page.locator("#shared-reader")).toHaveText("Read: 1");
});
