import { expect, test } from "@playwright/test";
import { startWindRawDevServer } from "./start-wind-raw-dev.mjs";

let server;

test.beforeAll(async () => {
  // Allow the 25-minute cold zfb/V8 startup budget plus time to clean up.
  test.setTimeout(1_560_000);
  server = await startWindRawDevServer();
});

test.afterAll(async () => {
  await server?.stop();
});

test("W-A08 computes quoted rawHtml styles on the live /wind-raw SSR route", async ({ page }) => {
  const response = await page.goto(server.url, {
    waitUntil: "load",
  });
  expect(response?.status()).toBe(200);
  await expect(page.locator("#wind-raw-route")).toHaveText(server.marker);

  const style = page.locator("head > style").filter({ hasText: "--quoted-css" });
  await expect(style).toHaveCount(1);
  expect(await style.textContent()).toBe(
    'body[data-zfb-wind="raw"] { font-family: "Wind & Raw"; --quoted-css: "<raw & trusted>"; }',
  );

  const computed = await page.locator("body").evaluate((element) => ({
    fontFamily: getComputedStyle(element).fontFamily,
    quotedCustomProperty: getComputedStyle(element).getPropertyValue("--quoted-css").trim(),
  }));
  expect(computed.fontFamily).toBe('"Wind & Raw"');
  expect(computed.quotedCustomProperty).toBe('"<raw & trusted>"');

  const scripts = await page.evaluate(() => ({
    inline: window.__zfbWindRaw,
    external: window.__zfbWindExternal,
  }));
  expect(scripts.inline).toBe("quoted <& trusted");
  expect(scripts.external).toBe("served external script");
});
