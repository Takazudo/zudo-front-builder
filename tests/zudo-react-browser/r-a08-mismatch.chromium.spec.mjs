// @ts-check
import { test, expect } from "@playwright/test";
import {
  expectNoBrowserErrors,
  openHeldScenario,
  releaseScenario,
  rootSelector,
} from "./browser-helpers.mjs";

test("R-A08 mismatch and identity rejection preserve edited DOM and leave a healthy neighboring root active", async ({
  page,
}) => {
  const state = await openHeldScenario(page, "r-a08-mismatch");
  try {
    const rootInputs = [0, 1, 2, 3, 4, 5].map((index) =>
      page.locator(`${rootSelector(index)} input`),
    );
    for (let index = 0; index < rootInputs.length; index++) {
      await rootInputs[index].fill(`typed before hydration ${index}`);
    }
    await page.evaluate(() => {
      window.__mismatchBefore = [0, 1, 2, 3, 4].map((index) => {
        const root = document.querySelector(
          `#scenario-root > [data-zudo-browser-root="root-${index}"]`,
        );
        const input = root.querySelector("input");
        return { root, content: root.firstElementChild, input, value: input.value };
      });
    });

    await releaseScenario(page, state);
    const negative = await page.evaluate(() => {
      const entries = window.__zudoReactBrowser.roots;
      const saved = window.__mismatchBefore;
      const domPreserved = saved.map((before, index) => {
        const root = entries[index].container;
        const input = root.querySelector("input");
        return {
          contentSame: before.content === root.firstElementChild,
          inputSame: before.input === input,
          value: input.value,
          valuePreserved: before.value === input.value,
          connected: input.isConnected,
          rootSame: before.root === root,
        };
      });
      return {
        diagnostics: entries.slice(0, 4).map((entry) => entry.diagnostics[0]),
        transportError: entries[4].transportError,
        failedRootsNull: entries.slice(0, 5).every((entry) => entry.root === null),
        domPreserved,
        activationCounts: [
          entries[0].result["wrong-tag"]?.activations,
          entries[1].result["missing-marker"]?.activations,
          entries[2].result["wrong-text"]?.activations,
        ],
        healthyRoot: entries[5].root !== null,
        healthyActivations: entries[5].result.healthy.activations,
      };
    });
    expect(negative.failedRootsNull).toBe(true);
    expect(negative.diagnostics.map((diagnostic) => diagnostic.code)).toEqual([
      "ZR_HYDRATION_MISMATCH",
      "ZR_HYDRATION_MISMATCH",
      "ZR_HYDRATION_MISMATCH",
      "ZR_IDENTITY",
    ]);
    for (const diagnostic of negative.diagnostics) {
      expect(diagnostic).toMatchObject({
        phase: "preflight",
        component: "MismatchProbe",
        protocol: "zudo-react/1",
        build: "zudo-react-browser-harness-v1",
      });
      expect(diagnostic.path).toContain("/div:nth-child");
      expect(diagnostic.expected).not.toBe("");
      expect(diagnostic.actual).not.toBe("");
    }
    expect(negative.transportError).toContain("ZR_PROPS_JSON at props");
    expect(negative.domPreserved).toEqual(
      negative.domPreserved.map((entry) => ({
        contentSame: true,
        inputSame: true,
        value: entry.value,
        valuePreserved: true,
        connected: true,
        rootSame: true,
      })),
    );
    expect(negative.domPreserved.map((entry) => entry.value)).toEqual(
      [0, 1, 2, 3, 4].map((index) => `typed before hydration ${index}`),
    );
    expect(negative.activationCounts).toEqual([0, 0, 0]);
    expect(negative.healthyRoot).toBe(true);
    expect(negative.healthyActivations).toBe(1);

    await page.locator("#healthy-button").click();
    await expect(page.locator("#healthy-button")).toHaveText("Healthy 1");
    expectNoBrowserErrors(state.errors);
  } finally {
    await releaseScenario(page, state).catch(() => undefined);
    await state.heldModule.remove();
  }
});
