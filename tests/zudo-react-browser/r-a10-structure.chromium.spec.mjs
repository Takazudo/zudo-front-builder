// @ts-check
import { test, expect } from "@playwright/test";
import { expectNoBrowserErrors, openHeldScenario, releaseScenario } from "./browser-helpers.mjs";

test("R-A10 conditional lifetimes and keyed moves retain input identity, focus and selection", async ({
  page,
}) => {
  const state = await openHeldScenario(page, "r-a10-structure");
  try {
    await releaseScenario(page, state);
    const branch = page.locator("#conditional-branch");
    const inputA = page.locator("#item-input-a");
    const inputB = page.locator("#item-input-b");
    await expect(branch).toBeVisible();
    expect(await page.evaluate(() => window.__zudoReactBrowser.roots[0].result)).toMatchObject({
      branchActivations: 1,
      branchDisposals: 0,
    });

    await page.evaluate(async () => {
      const state = window.__zudoReactBrowser.roots[0].result;
      state.shown.value = false;
      await window.__zudoReactBrowser.flush();
    });
    await expect(branch).toHaveCount(0);
    expect(
      await page.evaluate(() => window.__zudoReactBrowser.roots[0].result.branchDisposals),
    ).toBe(1);
    await page.evaluate(async () => {
      const state = window.__zudoReactBrowser.roots[0].result;
      state.shown.value = true;
      await window.__zudoReactBrowser.flush();
    });
    await expect(branch).toHaveCount(1);
    expect(
      await page.evaluate(() => window.__zudoReactBrowser.roots[0].result.branchActivations),
    ).toBe(2);

    await inputB.fill("typed beta");
    await inputB.evaluate((input) => input.setSelectionRange(2, 6));
    await page.evaluate(() => {
      window.__keyedB = document.querySelector("#item-input-b");
    });
    await page.evaluate(async () => {
      const state = window.__zudoReactBrowser.roots[0].result;
      state.items.value = [
        { id: "b", label: "Beta" },
        { id: "a", label: "Alpha" },
      ];
      await window.__zudoReactBrowser.flush();
    });
    const moved = await page.evaluate(() => {
      const input = document.querySelector("#item-input-b");
      return {
        sameNode: window.__keyedB === input,
        value: input.value,
        focused: document.activeElement === input,
        selectionStart: input.selectionStart,
        selectionEnd: input.selectionEnd,
        order: [...document.querySelectorAll(".list-item")].map((item) =>
          item.getAttribute("data-item-id"),
        ),
      };
    });
    expect(moved).toEqual({
      sameNode: true,
      value: "typed beta",
      focused: true,
      selectionStart: 2,
      selectionEnd: 6,
      order: ["b", "a"],
    });

    await page.evaluate(() => {
      window.__keyedA = document.querySelector("#item-input-a");
    });
    await page.evaluate(async () => {
      const state = window.__zudoReactBrowser.roots[0].result;
      state.items.value = [{ id: "b", label: "Beta" }];
      await window.__zudoReactBrowser.flush();
    });
    expect(
      await page.evaluate(() => window.__zudoReactBrowser.roots[0].result.itemDisposals),
    ).toEqual(["a"]);
    await expect(inputB).toHaveValue("typed beta");
    await page.evaluate(async () => {
      const state = window.__zudoReactBrowser.roots[0].result;
      state.items.value = [
        { id: "b", label: "Beta" },
        { id: "a", label: "Alpha again" },
      ];
      await window.__zudoReactBrowser.flush();
    });
    const recreated = await page.evaluate(() => ({
      isNew: window.__keyedA !== document.querySelector("#item-input-a"),
      disposals: window.__zudoReactBrowser.roots[0].result.itemDisposals,
      activations: window.__zudoReactBrowser.roots[0].result.itemActivations,
    }));
    expect(recreated.isNew).toBe(true);
    expect(recreated.disposals).toEqual(["a"]);
    expect(recreated.activations).toEqual(["a", "b", "a"]);

    const duplicate = await page.evaluate(async () => {
      const entry = window.__zudoReactBrowser.roots[0];
      const before = [...entry.container.querySelectorAll(".list-item")].map((item) =>
        item.getAttribute("data-item-id"),
      );
      entry.result.items.value = [
        { id: "b", label: "Beta one" },
        { id: "b", label: "Beta duplicate" },
      ];
      await window.__zudoReactBrowser.flush();
      return {
        before,
        after: [...entry.container.querySelectorAll(".list-item")].map((item) =>
          item.getAttribute("data-item-id"),
        ),
        diagnostics: entry.diagnostics,
      };
    });
    expect(duplicate.before).toEqual(["b", "a"]);
    expect(duplicate.after).toEqual(duplicate.before);
    expect(duplicate.diagnostics.at(-1)).toMatchObject({
      code: "ZR_DUPLICATE_KEY",
      phase: "update",
      protocol: "zudo-react/1",
    });
    expectNoBrowserErrors(state.errors);
  } finally {
    await releaseScenario(page, state).catch(() => undefined);
    await state.heldModule.remove();
  }
});
