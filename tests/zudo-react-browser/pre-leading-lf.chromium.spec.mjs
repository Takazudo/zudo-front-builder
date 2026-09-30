// @ts-check
import { test, expect } from "@playwright/test";
import { expectNoBrowserErrors, openHeldScenario, releaseScenario } from "./browser-helpers.mjs";

const CASES = [
  { variant: "leading-lf", authored: "\nabc" },
  { variant: "leading-two-lf", authored: "\n\nabc" },
  { variant: "plain", authored: "abc" },
  { variant: "crlf", authored: "\r\nabc" },
  { variant: "empty-sibling", authored: "\nabc" },
  { variant: "nested-array", authored: "\nabc" },
  { variant: "component-fragment", authored: "\nabc" },
  { variant: "code-child", authored: "\nabc" },
  { variant: "raw-html", authored: "\nabc" },
];

function normalizeHtmlNewlines(value) {
  return value.replace(/\r\n?/g, "\n");
}

function expectedText(variant, authored, mode) {
  if (variant === "raw-html") {
    if (mode === "none") {
      // Direct rawHtml has no structured-text compensation, so the HTML parser
      // removes its first LF from a pre element.
      return normalizeHtmlNewlines(authored).slice(1);
    }
    // The island's opaque-HTML marker precedes rawHtml, so the parser sees a
    // comment before the LF and preserves the opaque bytes.
  }
  return normalizeHtmlNewlines(authored);
}

test("pre leading LF survives HTML parsing and island hydration", async ({ page }) => {
  const state = await openHeldScenario(page, "pre-leading-lf");
  try {
    let index = 0;
    for (const { variant, authored } of CASES) {
      for (const mode of ["none", "hydrate"]) {
        const container = page.locator(`#scenario-root > [data-zudo-browser-root="root-${index}"]`);
        if (mode === "none") {
          await expect(container).not.toHaveAttribute("data-zfb-island");
        } else {
          await expect(container).toHaveAttribute("data-zfb-island", "PreLeadingLf");
        }
        const pre = container.locator("pre");
        await expect(pre, `${mode} ${variant} pre`).toHaveCount(1);
        expect(await pre.textContent(), `${mode} ${variant} text`).toBe(
          expectedText(variant, authored, mode),
        );
        index++;
      }
    }

    const signalRootIndex = index;
    const signalPre = page.locator(
      `#scenario-root > [data-zudo-browser-root="root-${signalRootIndex}"] pre`,
    );
    await expect(signalPre).toHaveCount(1);
    expect(await signalPre.textContent()).toBe("\nabc");

    await releaseScenario(page, state);
    const hydratedRoots = await page.evaluate(() =>
      window.__zudoReactBrowser.roots
        .filter((entry) => entry.mode === "hydrate")
        .map((entry) => ({
          index: entry.index,
          hasRoot: entry.root !== null,
          diagnostics: entry.diagnostics,
        })),
    );
    expect(hydratedRoots).toHaveLength(CASES.length + 1);
    expect(hydratedRoots.every((entry) => entry.hasRoot && entry.diagnostics.length === 0)).toBe(
      true,
    );

    const updatedSignalRoot = await page.evaluate(async (rootIndex) => {
      const entry = window.__zudoReactBrowser.roots[rootIndex];
      entry.result.dynamicText.value = "updated";
      await window.__zudoReactBrowser.flush();
      return {
        text: entry.container.querySelector("pre")?.textContent,
        diagnostics: entry.diagnostics,
      };
    }, signalRootIndex);
    expect(updatedSignalRoot.text).toBe("\nupdated");
    expect(updatedSignalRoot.diagnostics).toEqual([]);
    expectNoBrowserErrors(state.errors);
  } finally {
    await releaseScenario(page, state).catch(() => undefined);
    await state.heldModule.remove();
  }
});
