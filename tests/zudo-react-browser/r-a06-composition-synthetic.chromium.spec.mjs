// @ts-check
import { test, expect } from "@playwright/test";
import { expectNoBrowserErrors, openHeldScenario, releaseScenario } from "./browser-helpers.mjs";

test("R-A06 synthetic composition events defer writes and preserve equal-value selection", async ({
  page,
}) => {
  const state = await openHeldScenario(page, "r-a06-composition-synthetic");
  try {
    await releaseScenario(page, state);
    const observation = await page.evaluate(async () => {
      const input = document.querySelector("#composition-input");
      const model = window.__zudoReactBrowser.roots[0].result.value;
      input.focus();
      input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
      input.dispatchEvent(new CompositionEvent("compositionupdate", { bubbles: true, data: "に" }));
      input.value = "に";
      input.dispatchEvent(
        new InputEvent("input", {
          bubbles: true,
          inputType: "insertCompositionText",
          data: "に",
          isComposing: true,
        }),
      );
      const modelDuringComposition = model.value;
      model.value = "external update";
      await window.__zudoReactBrowser.flush();
      const controlDuringComposition = input.value;
      input.value = "日本語";
      input.dispatchEvent(
        new CompositionEvent("compositionend", { bubbles: true, data: "日本語" }),
      );
      await window.__zudoReactBrowser.flush();
      input.setSelectionRange(1, 2);
      model.value = input.value;
      await window.__zudoReactBrowser.flush();
      return {
        modelDuringComposition,
        controlDuringComposition,
        finalModel: model.value,
        finalControl: input.value,
        focused: document.activeElement === input,
        selectionStart: input.selectionStart,
        selectionEnd: input.selectionEnd,
      };
    });
    expect(observation).toEqual({
      modelDuringComposition: "に",
      controlDuringComposition: "に",
      finalModel: "日本語",
      finalControl: "日本語",
      focused: true,
      selectionStart: 1,
      selectionEnd: 2,
    });
    expectNoBrowserErrors(state.errors);
  } finally {
    await releaseScenario(page, state).catch(() => undefined);
    await state.heldModule.remove();
  }
});
