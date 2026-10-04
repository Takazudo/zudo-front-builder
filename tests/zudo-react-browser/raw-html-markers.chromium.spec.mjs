// @ts-check
import { test, expect } from "@playwright/test";
import {
  expectNoBrowserErrors,
  flush,
  openHeldScenario,
  releaseScenario,
  rootSelector,
} from "./browser-helpers.mjs";

// #3621: accepted rawHtml payloads whose marker-like text must stay display-only.
// Mirrors fixtures/raw-html-markers/component.tsx; the test checks they agree.
const CASES = [
  ["data-zfb-island=&quot;Demo&quot;", 'data-zfb-island="Demo"'],
  [
    "grep -roh &#39;data-zfb-island=&quot;[^&quot;]*&quot;&#39; dist/ | sort -u",
    `grep -roh 'data-zfb-island="[^"]*"' dist/ | sort -u`,
  ],
  [
    '<span style="color:#a3be8c;">&#39;data-zfb-island=&quot;[^&quot;]*&quot;&#39;</span>',
    `'data-zfb-island="[^"]*"'`,
  ],
  ['&lt;div data-zfb-island-skip-ssr="Demo"&gt;', '<div data-zfb-island-skip-ssr="Demo">'],
  ["&lt;!--zr:1:9:h--&gt;&lt;!--/zr:1:9--&gt;", "<!--zr:1:9:h--><!--/zr:1:9-->"],
  [`<span title='data-zfb-island="Demo"'>quoted</span>`, "quoted"],
  [`<span title="<!--zr:1:9:h-->">comment-like</span>`, "comment-like"],
  [`<span title=data-zfb-island=Demo>unquoted</span>`, "unquoted"],
  ["<!-- data-zfb-island=x -->a<!---->b<!-->c<!--->d</>e", "abcde"],
];
const MODES = ["none", "hydrate", "mount"];

/** Reports, per `pre`, its text plus every reserved attribute and protocol comment the parser created. */
function inspect(page, index) {
  return page.locator(rootSelector(index)).evaluate((container) =>
    [...container.querySelectorAll("pre")].map((pre) => {
      const reservedAttributes = [];
      const protocolComments = [];
      const walker = document.createTreeWalker(
        pre,
        NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_COMMENT,
      );
      for (let node = walker.nextNode(); node; node = walker.nextNode()) {
        if (node instanceof Element) {
          for (const name of ["data-zfb-island", "data-zfb-island-skip-ssr"]) {
            if (node.hasAttribute(name)) reservedAttributes.push(name);
          }
        } else if (node.nodeValue?.includes("zr:1:")) protocolComments.push(node.nodeValue);
      }
      return { id: pre.dataset.case, text: pre.textContent, reservedAttributes, protocolComments };
    }),
  );
}

function expectDisplayOnly(entries, mode, liveText) {
  expect(entries.map((entry) => entry.id)).toEqual([...CASES.map((_, i) => String(i)), "live"]);
  for (const entry of entries) {
    const label = `${mode} case ${entry.id}`;
    expect(entry.text, label).toBe(entry.id === "live" ? liveText : CASES[Number(entry.id)][1]);
    expect(entry.reservedAttributes, label).toEqual([]);
    // `mode` may carry a phase suffix such as "none SSR"; the root kind is its first word.
    if (mode.split(" ")[0] === "none") {
      expect(entry.protocolComments, label).toEqual([]);
    } else {
      // Only the renderer's own opaque-region pair; the payload adds none.
      expect(entry.protocolComments, label).toHaveLength(2);
      expect(entry.protocolComments[0], label).toMatch(/^zr:1:\d+:h$/);
      expect(entry.protocolComments[1], label).toMatch(/^\/zr:1:\d+$/);
    }
  }
}

test("accepted rawHtml creates no reserved attribute or protocol comment", async ({ page }) => {
  const state = await openHeldScenario(page, "raw-html-markers");
  try {
    for (const [index, mode] of MODES.entries()) {
      expectDisplayOnly(await inspect(page, index), `${mode} SSR`, CASES[0][1]);
    }

    await releaseScenario(page, state);
    const fixtureCases = await page.evaluate(() => window.__zudoReactBrowser.roots[0].result.cases);
    expect(fixtureCases).toEqual(CASES);
    const roots = await page.evaluate(() =>
      window.__zudoReactBrowser.roots.map((entry) => ({
        mode: entry.mode,
        hasRoot: entry.root !== null,
        diagnostics: entry.diagnostics,
      })),
    );
    expect(roots).toEqual([
      { mode: "none", hasRoot: false, diagnostics: [] },
      { mode: "hydrate", hasRoot: true, diagnostics: [] },
      { mode: "mount", hasRoot: true, diagnostics: [] },
    ]);
    for (const [index, mode] of MODES.entries()) {
      expectDisplayOnly(await inspect(page, index), mode, CASES[0][1]);
    }

    // Client updates parse each payload through the runtime's template path.
    for (const [payload, text] of CASES) {
      await page.evaluate((value) => {
        for (const entry of window.__zudoReactBrowser.roots) entry.result.live.value = value;
      }, payload);
      await flush(page);
      for (const index of [1, 2]) {
        expectDisplayOnly(await inspect(page, index), `${MODES[index]} update`, text);
      }
    }
    expect(
      await page.evaluate(() =>
        window.__zudoReactBrowser.roots.flatMap((entry) => entry.diagnostics),
      ),
    ).toEqual([]);
    expectNoBrowserErrors(state.errors);
  } finally {
    await releaseScenario(page, state).catch(() => undefined);
    await state.heldModule.remove();
  }
});
