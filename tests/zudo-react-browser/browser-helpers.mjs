// @ts-check
import { expect } from "@playwright/test";

export function collectBrowserErrors(page) {
  const pageErrors = [];
  const consoleErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  return { pageErrors, consoleErrors };
}

export function holdScenarioModule(page, pageName) {
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
    requested: () => requested,
    release: () => release(),
    install: () => page.route(routePattern, handler),
    remove: () => page.unroute(routePattern, handler),
  };
}

export async function openHeldScenario(page, pageName) {
  const errors = collectBrowserErrors(page);
  const heldModule = holdScenarioModule(page, pageName);
  await heldModule.install();
  const navigation = page.goto(`/${pageName}.html`, { waitUntil: "domcontentloaded" });
  await expect.poll(heldModule.requested).toBe(true);
  await expect(page.locator("#scenario-root")).toBeVisible();
  return { errors, heldModule, navigation };
}

export async function releaseScenario(page, state) {
  state.heldModule.release();
  await state.navigation;
  await expect.poll(() => page.evaluate(() => Boolean(globalThis.__zudoReactBrowser))).toBe(true);
}

export async function flush(page) {
  await page.evaluate(() => globalThis.__zudoReactBrowser.flush());
}

export function rootSelector(index) {
  return `#scenario-root > [data-zudo-browser-root="root-${index}"]`;
}

export function expectNoBrowserErrors(errors) {
  expect(errors.pageErrors, `page errors: ${errors.pageErrors.join("; ")}`).toEqual([]);
  expect(errors.consoleErrors, `console errors: ${errors.consoleErrors.join("; ")}`).toEqual([]);
}
