// @ts-check
import { test, expect } from "@playwright/test";
import {
  expectNoBrowserErrors,
  openHeldScenario,
  releaseScenario,
  rootSelector,
} from "./browser-helpers.mjs";

test("builds standard head markup and preserves SVG through hydrate and fresh mount", async ({
  page,
}) => {
  const state = await openHeldScenario(page, "standard-markup");
  try {
    const head = await page.evaluate(() => {
      const meta = document.head.querySelector('meta[property="og:title"]');
      const preload = document.head.querySelector('link[rel="preload"]');
      const script = document.head.querySelector('script[src="/standard-head.js"]');
      return {
        title: meta?.getAttribute("content"),
        preloadAs: preload?.getAttribute("as"),
        scriptDeferred: script?.hasAttribute("defer"),
        nonce: script?.nonce,
      };
    });
    expect(head).toEqual({
      title: "Standard markup integration",
      preloadAs: "script",
      scriptDeferred: true,
      nonce: "fixture-nonce",
    });

    const before = await page.evaluate(
      (selectors) => {
        window.__standardMarkupBefore = selectors.map((selector) => {
          const root = document.querySelector(selector);
          return root?.querySelector("svg");
        });
        return selectors.map((selector) => {
          const root = document.querySelector(selector);
          const svg = root?.querySelector("svg");
          const use = svg?.querySelector("use");
          return {
            namespace: svg?.namespaceURI,
            xmlns: svg?.getAttribute("xmlns"),
            xlinkHref: use?.getAttributeNS("http://www.w3.org/1999/xlink", "href"),
          };
        });
      },
      [rootSelector(0), rootSelector(1)],
    );
    expect(
      before.map(({ namespace, xmlns, xlinkHref }) => ({ namespace, xmlns, xlinkHref })),
    ).toEqual(
      [0, 1].map(() => ({
        namespace: "http://www.w3.org/2000/svg",
        xmlns: "http://www.w3.org/2000/svg",
        xlinkHref: "#shape",
      })),
    );

    await releaseScenario(page, state);
    await expect
      .poll(() =>
        page.evaluate(() => globalThis.__zudoReactBrowser.roots.map((root) => root.root?.protocol)),
      )
      .toEqual(["zudo-react/1", "zudo-react/1"]);
    await expect
      .poll(() => page.evaluate(() => globalThis.__standardHeadDeferredLoaded))
      .toBe(true);

    const after = await page.evaluate(
      (selectors) => {
        return selectors.map((selector, index) => {
          const root = document.querySelector(selector);
          const svg = root?.querySelector("svg");
          const use = svg?.querySelector("use");
          return {
            sameSvg: svg === window.__standardMarkupBefore[index],
            namespace: svg?.namespaceURI,
            xmlns: svg?.getAttribute("xmlns"),
            xlinkHref: use?.getAttributeNS("http://www.w3.org/1999/xlink", "href"),
          };
        });
      },
      [rootSelector(0), rootSelector(1)],
    );
    expect(after[0].sameSvg).toBe(true);
    expect(after[1].sameSvg).toBe(false);
    expect(
      after.map(({ namespace, xmlns, xlinkHref }) => ({ namespace, xmlns, xlinkHref })),
    ).toEqual(
      [0, 1].map(() => ({
        namespace: "http://www.w3.org/2000/svg",
        xmlns: "http://www.w3.org/2000/svg",
        xlinkHref: "#shape",
      })),
    );
    expectNoBrowserErrors(state.errors);
  } finally {
    await releaseScenario(page, state).catch(() => undefined);
    await state.heldModule.remove();
  }
});
