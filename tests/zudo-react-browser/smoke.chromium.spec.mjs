// @ts-check
import { test, expect } from "@playwright/test";

function collectBrowserErrors(page) {
  const pageErrors = [];
  const consoleErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  return { pageErrors, consoleErrors };
}

function holdScenarioModule(page, pageName) {
  let release;
  const held = new Promise((resolve) => {
    release = resolve;
  });
  let requested = false;
  const routePattern = new RegExp(`/generated/${pageName}\\.js(?:\\?.*)?$`);
  const handler = async (route) => {
    requested = true;
    await held;
    await route.continue();
  };

  return {
    routePattern,
    requested: () => requested,
    release: () => release(),
    install: () => page.route(routePattern, handler),
    remove: () => page.unroute(routePattern, handler),
  };
}

function expectNoBrowserErrors(errors) {
  expect(errors.pageErrors, `page errors: ${errors.pageErrors.join("; ")}`).toEqual([]);
  expect(errors.consoleErrors, `console errors: ${errors.consoleErrors.join("; ")}`).toEqual([]);
}

test("loads the packed core after observing the server-rendered counter", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  const heldModule = holdScenarioModule(page, "counter-none");
  await heldModule.install();
  const navigation = page.goto("/counter-none.html", { waitUntil: "domcontentloaded" });

  try {
    await expect.poll(heldModule.requested).toBe(true);
    const importMap = await page
      .locator('script[type="importmap"]')
      .evaluate((script) => JSON.parse(script.textContent ?? "{}").imports ?? {});
    expect(Object.keys(importMap).sort()).toEqual(
      [
        "@takazudo/zfb",
        "@takazudo/zfb/zudo-react",
        "@takazudo/zfb/zudo-react/client",
        "@takazudo/zfb/zudo-react/jsx-dev-runtime",
        "@takazudo/zfb/zudo-react/jsx-runtime",
        "@takazudo/zfb/zudo-react/server",
        "@takazudo/zfb/zudo-react/testing",
      ].sort(),
    );
    await expect(page.locator("#counter")).toHaveText("Count: 0");

    heldModule.release();
    await navigation;
    await expect
      .poll(() => page.evaluate(() => globalThis.__zudoReactBrowser?.result?.settled))
      .toBe(6);
    expect(await page.evaluate(() => globalThis.__zudoReactBrowser?.root)).toBeNull();
    expectNoBrowserErrors(errors);
  } finally {
    heldModule.release();
    await navigation.catch(() => undefined);
    await heldModule.remove();
  }
});

test("hydrates the server counter without replacing its button node", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  const heldModule = holdScenarioModule(page, "counter-hydrate");
  await heldModule.install();
  const navigation = page.goto("/counter-hydrate.html", { waitUntil: "domcontentloaded" });

  try {
    await expect.poll(heldModule.requested).toBe(true);
    const button = page.locator("#counter");
    await expect(button).toHaveText("Count: 0");
    await page.evaluate(() => {
      globalThis.__zudoReactOriginalButton = document.querySelector("#counter");
    });

    heldModule.release();
    await navigation;
    await expect
      .poll(() => page.evaluate(() => globalThis.__zudoReactBrowser?.root?.protocol))
      .toBe("zudo-react/1");
    expect(
      await page.evaluate(
        () => globalThis.__zudoReactOriginalButton === document.querySelector("#counter"),
      ),
    ).toBe(true);

    await button.click();
    await page.evaluate(() => globalThis.__zudoReactBrowser.flush());
    await expect(button).toHaveText("Count: 1");
    expectNoBrowserErrors(errors);
  } finally {
    heldModule.release();
    await navigation.catch(() => undefined);
    await heldModule.remove();
  }
});
