// @ts-check
import { test, expect } from "@playwright/test";
import {
  expectNoBrowserErrors,
  openHeldScenario,
  releaseScenario,
  rootSelector,
} from "./browser-helpers.mjs";

test("R-A02 hydrates two islands by adopting their existing nodes; mount is the negative control", async ({
  page,
}) => {
  const state = await openHeldScenario(page, "r-a02-adoption");
  try {
    const before = await page.evaluate((selectors) => {
      const saved = selectors.map((selector) => {
        const root = document.querySelector(selector);
        const section = root?.firstElementChild;
        const paragraph = section?.querySelector("p");
        const textNodes = paragraph
          ? [...paragraph.childNodes].filter(
              (node) => node.nodeType === Node.TEXT_NODE && node.textContent,
            )
          : [];
        return {
          root,
          section,
          paragraph,
          textNodes,
          fragmentA: section?.querySelector("strong"),
          fragmentB: section?.querySelector("em"),
        };
      });
      window.__adoptionBefore = saved;
      return saved.map((entry) => ({ textNodeCount: entry.textNodes.length }));
    }, [0, 1, 2].map(rootSelector));
    expect(before[0].textNodeCount).toBeGreaterThanOrEqual(3);
    expect(before[1].textNodeCount).toBeGreaterThanOrEqual(3);

    await releaseScenario(page, state);
    const identity = await page.evaluate(() => {
      return window.__adoptionBefore.map((saved, index) => {
        const currentRoot = document.querySelector(
          `#scenario-root > [data-zudo-browser-root="root-${index}"]`,
        );
        const currentSection = currentRoot?.firstElementChild;
        const currentParagraph = currentSection?.querySelector("p");
        const currentTextNodes = currentParagraph
          ? [...currentParagraph.childNodes].filter(
              (node) => node.nodeType === Node.TEXT_NODE && node.textContent,
            )
          : [];
        return {
          sectionSame: saved.section === currentSection,
          paragraphSame: saved.paragraph === currentParagraph,
          textNodesSame:
            saved.textNodes.length === currentTextNodes.length &&
            saved.textNodes.every((node, textIndex) => node === currentTextNodes[textIndex]),
          fragmentNodesSame:
            saved.fragmentA === currentSection?.querySelector("strong") &&
            saved.fragmentB === currentSection?.querySelector("em"),
          connected: saved.section?.isConnected && saved.paragraph?.isConnected,
        };
      });
    });
    expect(identity[0]).toEqual({
      sectionSame: true,
      paragraphSame: true,
      textNodesSame: true,
      fragmentNodesSame: true,
      connected: true,
    });
    expect(identity[1]).toEqual(identity[0]);
    expect(identity[2].sectionSame).toBe(false);
    expect(identity[2].paragraphSame).toBe(false);
    expect(identity[2].fragmentNodesSame).toBe(false);

    await page.evaluate(async () => {
      const states = window.__zudoReactBrowser.roots[0].result;
      states.first.left.value = "adopted update";
      states.first.empty.value = "filled empty slot";
      states.first.right.value = "right update";
      await window.__zudoReactBrowser.flush();
    });
    await expect(page.locator("#adjacent-first")).toContainText("adopted update");
    await expect(page.locator("#adjacent-first")).toContainText("filled empty slot");
    expectNoBrowserErrors(state.errors);
  } finally {
    await releaseScenario(page, state).catch(() => undefined);
    await state.heldModule.remove();
  }
});
