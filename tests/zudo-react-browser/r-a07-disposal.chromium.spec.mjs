// @ts-check
import { test, expect } from "@playwright/test";
import {
  expectNoBrowserErrors,
  openHeldScenario,
  releaseScenario,
  rootSelector,
} from "./browser-helpers.mjs";

test("R-A07 dispose releases subscriptions, observer and listeners once while preserving DOM; cancellation and preflight fail closed", async ({
  page,
}) => {
  const state = await openHeldScenario(page, "r-a07-disposal");
  try {
    await releaseScenario(page, state);
    await page.evaluate(async () => {
      const entry = window.__zudoReactBrowser.roots[0];
      entry.result.widget.value.value = 2;
      await window.__zudoReactBrowser.flush();
    });
    await expect(page.locator("#disposal-value")).toHaveText("4");
    expect(
      await page.evaluate(() => window.__zudoReactBrowser.roots[0].result.widget),
    ).toMatchObject({ effectRuns: 2, effectCleanups: 1 });

    const before = await page.evaluate(() => {
      const roots = window.__zudoReactBrowser.roots;
      roots[0].root.dispose();
      roots[0].root.dispose();
      roots[0].result.widget.value.value = 3;
      window.dispatchEvent(new Event("zudo-disposal-probe"));
      roots[0].container
        .querySelector("button")
        .dispatchEvent(new MouseEvent("click", { bubbles: true }));
      return {
        widgetDisposed: roots[0].root.disposed,
        domPresent: Boolean(roots[0].container.querySelector("#disposal-button")),
        observerCleanups: roots[0].result.widget.observerCleanups,
        listenerCleanups: roots[0].result.widget.listenerCleanups,
        effectRuns: roots[0].result.widget.effectRuns,
        effectCleanups: roots[0].result.widget.effectCleanups,
        clicks: roots[0].result.widget.clicks.value,
        observedChanges: roots[0].result.widget.observedChanges,
        abortedDiagnostic: roots[1].diagnostics[0],
        preflightDiagnostic: roots[2].diagnostics[0],
        preflightText: roots[2].container.textContent,
        preflightState: roots[2].result.failed.preflight,
        abortedState: roots[1].result.failed.aborted,
      };
    });
    expect(before).toMatchObject({
      widgetDisposed: true,
      domPresent: true,
      observerCleanups: 1,
      listenerCleanups: 1,
      effectRuns: 2,
      effectCleanups: 2,
      clicks: 0,
      observedChanges: 0,
      preflightText: "preflight typed",
      preflightState: { activations: 0, clicks: 0 },
      abortedDiagnostic: { code: "ZR_CANCELLED", phase: "commit" },
      abortedState: { activations: 0, clicks: 0 },
      preflightDiagnostic: { code: "ZR_HYDRATION_MISMATCH", phase: "preflight" },
    });

    const abortedRoot = page.locator(`${rootSelector(1)} button`);
    await abortedRoot.click();
    await expect(abortedRoot).toHaveText("aborted root");
    const removedChildren = await page.evaluate(() => {
      const entry = window.__zudoReactBrowser.roots[0];
      entry.root.unmount();
      return entry.container.childNodes.length;
    });
    expect(removedChildren).toBe(0);
    expectNoBrowserErrors(state.errors);
  } finally {
    await releaseScenario(page, state).catch(() => undefined);
    await state.heldModule.remove();
  }
});
