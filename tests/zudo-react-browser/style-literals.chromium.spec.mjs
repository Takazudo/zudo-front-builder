// @ts-check
import { test, expect } from "@playwright/test";
import { expectNoBrowserErrors, openHeldScenario, releaseScenario } from "./browser-helpers.mjs";

const selectors = ["#style-hydrate", "#style-mount"];

function expectedStyles({ left, top, width, maxHeight, opacity, customNumber, maskSize }) {
  return selectors.map(() => ({
    left,
    top,
    width,
    maxHeight,
    paddingTop: "0px",
    opacity,
    customNumber,
    maskSize,
    hasVendorMask: true,
  }));
}

async function readComputedStyles(page) {
  return page.evaluate(
    (elementSelectors) =>
      elementSelectors.map((selector) => {
        const element = document.querySelector(selector);
        if (!(element instanceof HTMLElement)) throw new Error(`Missing ${selector}`);
        const style = getComputedStyle(element);
        return {
          left: style.left,
          top: style.top,
          width: style.width,
          maxHeight: style.maxHeight,
          paddingTop: style.paddingTop,
          opacity: style.opacity,
          customNumber: style.getPropertyValue("--zfb-style-number").trim(),
          maskSize: style.getPropertyValue("-webkit-mask-size"),
          hasVendorMask: style.getPropertyValue("-webkit-mask-image").includes("linear-gradient"),
        };
      }),
    selectors,
  );
}

test("keeps literal styles through parsed hydration, fresh mount, and signal updates", async ({
  page,
}) => {
  const state = await openHeldScenario(page, "style-literals");
  try {
    const parsed = await page.evaluate((elementSelectors) => {
      window.__styleLiteralsBefore = elementSelectors.map((selector) =>
        document.querySelector(selector),
      );
      return elementSelectors.map((selector) => {
        const element = document.querySelector(selector);
        return element instanceof HTMLElement ? element.getAttribute("style") : null;
      });
    }, selectors);
    expect(parsed).toEqual([
      expect.stringContaining("left:12px"),
      expect.stringContaining("left:12px"),
    ]);
    expect(await readComputedStyles(page)).toEqual(
      expectedStyles({
        left: "12px",
        top: "8px",
        width: "120px",
        maxHeight: "40px",
        opacity: "0.5",
        customNumber: "2",
        maskSize: "100% 100%",
      }),
    );

    await releaseScenario(page, state);
    expect(
      await page.evaluate(
        (elementSelectors) =>
          elementSelectors.map(
            (selector, index) =>
              document.querySelector(selector) === window.__styleLiteralsBefore[index],
          ),
        selectors,
      ),
    ).toEqual([true, false]);
    expect(await readComputedStyles(page)).toEqual(
      expectedStyles({
        left: "12px",
        top: "8px",
        width: "120px",
        maxHeight: "40px",
        opacity: "0.5",
        customNumber: "2",
        maskSize: "100% 100%",
      }),
    );

    await page.evaluate(async () => {
      const roots = window.__zudoReactBrowser.roots;
      for (const entry of roots) {
        entry.result.style.value = {
          position: "absolute",
          left: "24px",
          top: "1rem",
          width: "160px",
          height: "96px",
          "max-height": "48px",
          "font-size": "16px",
          padding: 0,
          opacity: 0.25,
          "line-height": 1.5,
          "--zfb-style-number": 3,
          "-webkit-mask-image": "linear-gradient(to bottom, black, transparent)",
          "-webkit-mask-size": "50% 100%",
        };
      }
      await window.__zudoReactBrowser.flush();
    });
    expect(await readComputedStyles(page)).toEqual(
      expectedStyles({
        left: "24px",
        top: "16px",
        width: "160px",
        maxHeight: "48px",
        opacity: "0.25",
        customNumber: "3",
        maskSize: "50% 100%",
      }),
    );
    expectNoBrowserErrors(state.errors);
  } finally {
    await releaseScenario(page, state).catch(() => undefined);
    await state.heldModule.remove();
  }
});
