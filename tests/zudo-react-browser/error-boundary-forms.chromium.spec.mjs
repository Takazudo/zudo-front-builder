// @ts-check
import { test, expect } from "@playwright/test";
import {
  expectNoBrowserErrors,
  flush,
  openHeldScenario,
  releaseScenario,
} from "./browser-helpers.mjs";

test("native ancestor/explicit form radio groups survive boundary resets and detach stale nested listeners", async ({
  page,
}) => {
  const state = await openHeldScenario(page, "error-boundary-forms");
  try {
    await page.evaluate(() => {
      window.__boundaryFormsBefore = Object.fromEntries(
        ["ancestor", "explicit"].map((id) => [
          id,
          {
            form: document.querySelector(`#form-${id}`),
            outside: document.querySelector(`#outside-${id}`),
            stale: [],
          },
        ]),
      );
    });
    await releaseScenario(page, state);

    for (const [id, index] of [
      ["ancestor", 0],
      ["explicit", 1],
    ]) {
      const outside = page.locator(`#outside-${id}`);
      const inside = page.locator(`#inside-${id}`);
      await expect(inside).toBeChecked();
      await outside.check();
      await expect(page.locator(`#model-${id}`)).toHaveText("outside");

      for (let cycle = 1; cycle <= 3; cycle++) {
        await page.evaluate((key) => {
          window.__boundaryFormsBefore[key].stale.push(document.querySelector(`#inside-${key}`));
        }, id);
        await page.locator(`#reset-${id}`).click();
        await flush(page);
        await expect(inside).not.toBeChecked();
        await expect(outside).toBeChecked();
        await expect(page.locator(`#model-${id}`)).toHaveText("outside");

        expect(
          await page.evaluate((key) => {
            const before = window.__boundaryFormsBefore[key];
            const fresh = document.querySelector(`#inside-${key}`);
            const retained = document.querySelector(`#outside-${key}`);
            return {
              freshInside: before.stale.every((old) => old !== fresh && !old.isConnected),
              retainedOutside: before.outside === retained,
              retainedForm: before.form === document.querySelector(`#form-${key}`),
              sameNativeForm: fresh.form === retained.form && fresh.form === before.form,
            };
          }, id),
        ).toEqual({
          freshInside: true,
          retainedOutside: true,
          retainedForm: true,
          sameNativeForm: true,
        });

        // Native radio selection and current group membership must still work
        // after replacing only one subgroup.
        await inside.check();
        await expect(page.locator(`#model-${id}`)).toHaveText("inside");
        await expect(outside).not.toBeChecked();
        await outside.check();
        await expect(page.locator(`#model-${id}`)).toHaveText("outside");
        await expect(inside).not.toBeChecked();

        expect(
          await page.evaluate(
            async ({ key, rootIndex }) => {
              for (const old of window.__boundaryFormsBefore[key].stale) {
                old.checked = true;
                old.dispatchEvent(new Event("change", { bubbles: true }));
                old.dispatchEvent(new Event("input", { bubbles: true }));
              }
              await window.__zudoReactBrowser.flush();
              const entry = window.__zudoReactBrowser.roots[rootIndex];
              const current = entry.result[key];
              return {
                model: current.model.value,
                activations: current.activations,
                cleanups: current.cleanups,
                diagnostics: entry.diagnostics,
              };
            },
            { key: id, rootIndex: index },
          ),
        ).toEqual({ model: "outside", activations: cycle + 1, cleanups: cycle, diagnostics: [] });
        await expect(outside).toBeChecked();
      }

      // The inner boundary now fails after three outer replacements. Its last
      // radio/listeners must dispose once, without disturbing the retained form.
      await page.evaluate((key) => {
        window.__boundaryFormsBefore[key].stale.push(document.querySelector(`#inside-${key}`));
      }, id);
      await page.locator(`#fail-${id}`).click();
      await expect(page.locator(`#inner-fallback-${id}`)).toHaveText("Inner fallback");
      await expect(page.locator(`#outer-fallback-${id}`)).toHaveCount(0);
      await expect(inside).toHaveCount(0);
      expect(
        await page.evaluate(
          async ({ key, rootIndex }) => {
            const before = window.__boundaryFormsBefore[key];
            for (const old of before.stale) {
              old.checked = true;
              old.dispatchEvent(new Event("change", { bubbles: true }));
            }
            const harness = window.__zudoReactBrowser;
            await harness.flush();
            const entry = harness.roots[rootIndex];
            entry.root.dispose();
            entry.root.dispose();
            return {
              model: entry.result[key].model.value,
              activations: entry.result[key].activations,
              cleanups: entry.result[key].cleanups,
              retainedOutside: before.outside === document.querySelector(`#outside-${key}`),
              diagnostics: entry.diagnostics,
            };
          },
          { key: id, rootIndex: index },
        ),
      ).toEqual({
        model: "outside",
        activations: 4,
        cleanups: 4,
        retainedOutside: true,
        diagnostics: [],
      });
    }
    expectNoBrowserErrors(state.errors);
  } finally {
    await releaseScenario(page, state).catch(() => undefined);
    await state.heldModule.remove();
  }
});
