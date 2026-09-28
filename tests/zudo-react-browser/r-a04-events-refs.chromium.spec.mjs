// @ts-check
import { test, expect } from "@playwright/test";
import { expectNoBrowserErrors, openHeldScenario, releaseScenario } from "./browser-helpers.mjs";

test("R-A04 native events, ref-before-activation order, duplicate hydration and async disposal", async ({
  page,
}) => {
  const state = await openHeldScenario(page, "r-a04-events-refs");
  try {
    await releaseScenario(page, state);
    expect(await page.evaluate(() => window.__zudoReactBrowser.roots[0].result.events)).toEqual([
      "child:event-button:event-parent",
      "parent:event-button:event-parent",
    ]);

    const duplicate = await page.evaluate(() => {
      const entry = window.__zudoReactBrowser.roots[0];
      const diagnostics = [];
      const root = window.__zudoReactBrowser.client.hydrate(entry.node, entry.container, {
        identity: entry.identity,
        report: (diagnostic) => diagnostics.push(diagnostic),
      });
      return { root, diagnostics };
    });
    expect(duplicate.root).toBeNull();
    expect(duplicate.diagnostics).toHaveLength(1);
    expect(duplicate.diagnostics[0]).toMatchObject({
      code: "ZR_DUPLICATE_ROOT",
      phase: "preflight",
      protocol: "zudo-react/1",
    });

    const button = page.locator("#event-button");
    await button.click();
    await expect(page.locator("#event-count")).toHaveText("1");
    await page.evaluate(() => {
      const entry = window.__zudoReactBrowser.roots[0];
      entry.root.dispose();
      entry.container
        .querySelector("button")
        .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await expect(page.locator("#event-count")).toHaveText("1");
    expect(await page.locator("#event-parent").count()).toBe(1);

    await page.evaluate(async () => {
      window.__zudoReactBrowser.roots[0].result.settleAsync();
      await Promise.resolve();
      await window.__zudoReactBrowser.flush();
    });
    await expect(page.locator("#async-settled")).toHaveText("0");
    const unmounted = await page.evaluate(() => {
      const entry = window.__zudoReactBrowser.roots[0];
      entry.root.unmount();
      return entry.container.childNodes.length;
    });
    expect(unmounted).toBe(0);
    expectNoBrowserErrors(state.errors);
  } finally {
    await releaseScenario(page, state).catch(() => undefined);
    await state.heldModule.remove();
  }
});
