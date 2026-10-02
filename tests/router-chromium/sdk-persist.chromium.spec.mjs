// Public <Island persist> markup from the built SDK, exercised through the real
// router and owned island runtime in Chromium.
// @ts-check
import { test, expect } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const events = [];
    document.addEventListener("zfb:page-load", () => events.push("load"));
    window.__sdkPersistEvents = events;
  });
});

async function navigate(page, target) {
  const previous = await page.evaluate(() => window.__sdkPersistEvents.length);
  await page.locator(`#to-sdk-${target}`).click();
  await page.waitForFunction((count) => window.__sdkPersistEvents.length > count, previous);
  await expect(page.locator("h1")).toHaveText(`SDK persistence ${target}`);
}

test("built SDK persistence keeps unchanged roots, refreshes props by default, and retains props on request", async ({
  page,
}) => {
  const errors = [];
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  page.on("pageerror", (error) => errors.push(error.message));

  await page.goto("/real-islands-sdk-a.html");
  await page.waitForFunction(
    () => window.__sdkPersistEvents.length > 0 && window.__realIslands?.mounts.length === 3,
  );

  await expect(page.locator("#sdk-island-same")).toHaveAttribute(
    "data-zfb-transition-persist",
    "sdk-same",
  );
  await expect(page.locator("#sdk-island-retain")).toHaveAttribute(
    "data-zfb-transition-persist-props",
    "true",
  );
  await expect(page.locator("#sdk-island-same")).toHaveAttribute("data-zfb-transport", "json/1");
  await expect(page.locator("#sdk-island-same")).toHaveAttribute(
    "data-zfb-protocol",
    "zudo-react/1",
  );
  await expect(page.locator("#sdk-island-same")).toHaveAttribute(
    "data-zfb-build",
    "browser-fixture-1",
  );

  await page.evaluate(() => {
    window.__sdkPersistNodes = Object.fromEntries(
      ["same", "refresh", "retain"].map((key) => [
        key,
        document.querySelector(`#sdk-island-${key}`),
      ]),
    );
    document.querySelector("#sdk-island-same").setAttribute("data-identity-tag", "same-node");
  });
  await page.locator("#button-sdk-same").click();
  await page.locator("#button-sdk-refresh").click();
  await page.locator("#button-sdk-retain").click();
  await expect(page.locator("#value-sdk-same")).toHaveText("3");
  await expect(page.locator("#value-sdk-refresh")).toHaveText("11");
  await expect(page.locator("#value-sdk-retain")).toHaveText("101");

  await navigate(page, "b");
  await expect(page.locator("#value-sdk-same")).toHaveText("3");
  await expect(page.locator("#value-sdk-refresh")).toHaveText("20");
  await expect(page.locator("#value-sdk-retain")).toHaveText("101");
  expect(
    await page.evaluate(() =>
      ["same", "refresh", "retain"].every(
        (key) => document.querySelector(`#sdk-island-${key}`) === window.__sdkPersistNodes[key],
      ),
    ),
  ).toBe(true);
  await expect(page.locator("#sdk-island-same")).toHaveAttribute("data-identity-tag", "same-node");
  await expect(page.locator("#sdk-island-retain")).toHaveAttribute(
    "data-props",
    '{"id":"sdk-retain","start":100}',
  );
  expect(
    await page.evaluate(() => window.__realIslands.mounts.map(({ id, mode }) => [id, mode])),
  ).toEqual([
    ["sdk-same", "hydrate"],
    ["sdk-refresh", "hydrate"],
    ["sdk-retain", "hydrate"],
    ["sdk-refresh", "render"],
  ]);
  expect(await page.evaluate(() => window.__realIslands.disposals.map(({ id }) => id))).toEqual([
    "sdk-refresh",
  ]);

  await page.locator("#button-sdk-refresh").click();
  await page.locator("#button-sdk-retain").click();
  await expect(page.locator("#value-sdk-refresh")).toHaveText("21");
  await expect(page.locator("#value-sdk-retain")).toHaveText("102");

  await navigate(page, "gone");
  await expect(
    page.locator("#sdk-island-same, #sdk-island-refresh, #sdk-island-retain"),
  ).toHaveCount(0);
  expect(
    await page.evaluate(() => window.__realIslands.disposals.map(({ id }) => id).sort()),
  ).toEqual(["sdk-refresh", "sdk-refresh", "sdk-retain", "sdk-same"]);
  expect(errors).toEqual([]);
});
