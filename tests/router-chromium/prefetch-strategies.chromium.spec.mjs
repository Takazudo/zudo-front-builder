// @ts-check
// Per-link prefetch strategy precedence in a real browser (#4683).
//
// With prefetchAll enabled, the load and viewport scans must admit each link
// through its explicit data-zfb-prefetch strategy: a link assigned to a
// different strategy must not be requested by the scan, and must still be
// requested when its own trigger occurs. Waits are request- and idle-keyed.
import { expect, test } from "@playwright/test";

const STRATEGIES = ["hover", "tap", "viewport", "load"];

/**
 * @param {import("@playwright/test").Page} page
 */
async function recordPrefetchRequests(page) {
  /** @type {string[]} */
  const requested = [];
  await page.route("**/pf-*", async (route) => {
    requested.push(new URL(route.request().url()).pathname);
    await route.fulfill({ status: 200, contentType: "text/html", body: "<p>prefetched</p>" });
  });
  return requested;
}

/**
 * Replace the body with one in-viewport link per explicit strategy plus one
 * default (no attribute) link, all with distinct paths for this round.
 * @param {import("@playwright/test").Page} page
 * @param {string} round
 */
async function renderLinks(page, round) {
  await page.evaluate(
    ({ round, strategies }) => {
      document.body.replaceChildren();
      for (const strategy of [...strategies, "default"]) {
        const link = document.createElement("a");
        link.href = `/pf-${round}-${strategy}`;
        link.id = `${round}-${strategy}`;
        link.textContent = `${round} ${strategy}`;
        link.style.display = "block";
        link.style.padding = "4px";
        if (strategy !== "default") link.dataset.zfbPrefetch = strategy;
        document.body.append(link);
      }
    },
    { round, strategies: STRATEGIES },
  );
}

/**
 * Two idle periods: every load-scan callback queued by the scan has run.
 * @param {import("@playwright/test").Page} page
 */
async function drainIdle(page) {
  await page.evaluate(
    () =>
      new Promise((resolve) =>
        requestIdleCallback(() => requestIdleCallback(() => resolve(undefined))),
      ),
  );
}

for (const defaultStrategy of ["load", "viewport"]) {
  test(`prefetchAll ${defaultStrategy} default scans only admit matching links, initially and after a swap`, async ({
    page,
  }) => {
    const requested = await recordPrefetchRequests(page);
    await page.goto("/prefetch-target.html");
    await renderLinks(page, "initial");
    await page.evaluate(async (defaultStrategy) => {
      const { init } = await import("/dist/client-router/prefetch.js");
      init({ prefetchAll: true, defaultStrategy });
    }, defaultStrategy);

    for (const round of ["initial", "swapped"]) {
      if (round === "swapped") {
        await renderLinks(page, round);
        await page.evaluate(() => document.dispatchEvent(new Event("zfb:after-swap")));
      }
      // The default link and the explicit link of the default strategy are
      // scanned; an explicit link of the other scanned strategy is admitted
      // by its own scan. Hover and tap links must stay unrequested.
      const expected = [`/pf-${round}-load`, `/pf-${round}-viewport`, `/pf-${round}-default`];
      for (const path of expected) {
        await expect.poll(() => requested.includes(path), { message: path }).toBe(true);
      }
      await drainIdle(page);
      expect(requested.filter((path) => path.startsWith(`/pf-${round}-`)).sort()).toEqual(
        [...expected].sort(),
      );
    }

    // Each excluded explicit strategy still prefetches on its own trigger.
    await page.hover("#swapped-hover");
    await expect.poll(() => requested.includes("/pf-swapped-hover")).toBe(true);
    await page.dispatchEvent("#swapped-tap", "mousedown");
    await expect.poll(() => requested.includes("/pf-swapped-tap")).toBe(true);
  });
}

test("a load default scan does not request explicit viewport links that are out of view", async ({
  page,
}) => {
  const requested = await recordPrefetchRequests(page);
  await page.goto("/prefetch-target.html");
  await page.evaluate(() => {
    document.body.replaceChildren();
    const load = document.createElement("a");
    load.href = "/pf-far-load";
    const spacer = document.createElement("div");
    spacer.style.height = "5000px";
    const viewport = document.createElement("a");
    viewport.href = "/pf-far-viewport";
    viewport.id = "far-viewport";
    viewport.dataset.zfbPrefetch = "viewport";
    viewport.textContent = "far";
    document.body.append(load, spacer, viewport);
  });
  await page.evaluate(async () => {
    const { init } = await import("/dist/client-router/prefetch.js");
    init({ prefetchAll: true, defaultStrategy: "load" });
  });

  await expect.poll(() => requested.includes("/pf-far-load")).toBe(true);
  await drainIdle(page);
  expect(requested).not.toContain("/pf-far-viewport");

  await page.locator("#far-viewport").scrollIntoViewIfNeeded();
  await expect.poll(() => requested.includes("/pf-far-viewport")).toBe(true);
});
