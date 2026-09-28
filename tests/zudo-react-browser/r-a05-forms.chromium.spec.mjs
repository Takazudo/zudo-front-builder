// @ts-check
import { test, expect } from "@playwright/test";
import { expectNoBrowserErrors, openHeldScenario, releaseScenario } from "./browser-helpers.mjs";

test("R-A05 preserves uncontrolled edits and reconciles the five supported models before writes", async ({
  page,
}) => {
  const state = await openHeldScenario(page, "r-a05-forms");
  try {
    const uncontrolled = page.locator("#uncontrolled");
    await uncontrolled.fill("typed before hydration");
    await page.evaluate(() => {
      document.querySelector("#model-text").value = "live text";
      document.querySelector("#model-area").value = "live area";
      document.querySelector("#model-checked").checked = true;
      document.querySelector("#model-choice").value = "two";
      document.querySelectorAll('input[type="radio"]')[1].checked = true;
      window.__uncontrolledBefore = document.querySelector("#uncontrolled");
    });
    expect(await page.evaluate(() => document.activeElement.id)).toBe("uncontrolled");
    expect(await uncontrolled.inputValue()).toBe("typed before hydration");

    await releaseScenario(page, state);
    expect(
      await page.evaluate(
        () => window.__uncontrolledBefore === document.querySelector("#uncontrolled"),
      ),
    ).toBe(true);
    expect(await page.evaluate(() => document.activeElement.id)).toBe("uncontrolled");
    await expect(uncontrolled).toHaveValue("typed before hydration");
    await expect(page.locator("#model-summary")).toHaveText("live text|live area|true|two|right");

    await page.evaluate(async () => {
      const models = window.__zudoReactBrowser.roots[0].result;
      models.text.value = "program text";
      models.area.value = "program area";
      models.checked.value = false;
      models.choice.value = "one";
      models.radio.value = "left";
      await window.__zudoReactBrowser.flush();
    });
    await expect(page.locator("#model-text")).toHaveValue("program text");
    await expect(page.locator("#model-area")).toHaveValue("program area");
    await expect(page.locator("#model-checked")).not.toBeChecked();
    await expect(page.locator("#model-choice")).toHaveValue("one");
    await expect(page.locator('input[type="radio"][value="left"]')).toBeChecked();

    await page.evaluate(async () => {
      document.querySelector("#forms").reset();
      await Promise.resolve();
      await window.__zudoReactBrowser.flush();
    });
    await expect(page.locator("#model-summary")).toHaveText(
      "text default|area default|false|one|left",
    );

    await page.evaluate(async () => {
      const models = window.__zudoReactBrowser.roots[0].result;
      models.text.value = "kept text";
      models.area.value = "kept area";
      models.checked.value = true;
      models.choice.value = "two";
      models.radio.value = "right";
      await window.__zudoReactBrowser.flush();
      const form = document.querySelector("#forms");
      form.addEventListener("reset", (event) => event.preventDefault(), { once: true });
      form.reset();
      await Promise.resolve();
      await window.__zudoReactBrowser.flush();
    });
    await expect(page.locator("#model-summary")).toHaveText("kept text|kept area|true|two|right");

    const unsupported = await page.evaluate(() => {
      const outer = document.createElement("div");
      outer.setAttribute("data-zfb-island", "UnsupportedMultiple");
      outer.setAttribute("data-when", "load");
      outer.setAttribute("data-zfb-transport", "json/1");
      outer.setAttribute("data-zfb-protocol", "zudo-react/1");
      outer.setAttribute("data-zfb-build", "browser-negative-control");
      outer.setAttribute("data-props", "{}");
      outer.innerHTML =
        '<select id="unsupported-multiple" multiple><option value="one">One</option><option value="two">Two</option></select>';
      document.body.append(outer);
      const before = outer.innerHTML;
      const diagnostics = [];
      const scenario = window.__zudoReactBrowser.roots[0].scenario;
      const node = window.__zudoReactBrowser.h(scenario.UnsupportedMultiple, {});
      const root = window.__zudoReactBrowser.client.hydrate(node, outer, {
        identity: { component: "UnsupportedMultiple", build: "browser-negative-control" },
        report: (diagnostic) => diagnostics.push(diagnostic),
      });
      return { before, after: outer.innerHTML, root, diagnostics };
    });
    expect(unsupported.root).toBeNull();
    expect(unsupported.after).toBe(unsupported.before);
    expect(unsupported.diagnostics).toHaveLength(1);
    expect(unsupported.diagnostics[0]).toMatchObject({
      code: "ZR_MODEL_UNSUPPORTED",
      protocol: "zudo-react/1",
    });
    expectNoBrowserErrors(state.errors);
  } finally {
    await releaseScenario(page, state).catch(() => undefined);
    await state.heldModule.remove();
  }
});
