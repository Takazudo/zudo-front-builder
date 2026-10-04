// @ts-check
import { test, expect } from "@playwright/test";
import { collectBrowserErrors } from "./browser-helpers.mjs";

test("packed testing entry hydrates, mounts, cleans up, and renders full-page SDK Islands", async ({
  page,
}) => {
  const errors = collectBrowserErrors(page);
  await page.goto("/testing-harness.html");
  await page.waitForFunction(() => Boolean(window.__zudoReactBrowser));

  const result = await page.evaluate(() => window.__zudoReactBrowser.roots[0].result);
  expect(result.html).toContain('data-zfb-island="Counter"');
  expect(result).toMatchObject({
    hydrated: true,
    flushedText: "after",
    cleanup: {
      rootDisposed: true,
      hostRemoved: true,
      textAfterDispose: "after",
      clicksAfterDispose: 1,
    },
    failureResult: {
      returnedNull: true,
      diagnostic: "ZR_PROPS",
      domPreserved: true,
    },
    mountReplacedChildren: true,
    fullPageIncludesIsland: true,
    fullPageIncludesPageShell: true,
    metadataRestored: true,
  });

  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toHaveLength(1);
  expect(errors.consoleErrors[0]).toMatch(
    /^\[zudo-react\] ZR_HYDRATION_MISMATCH preflight ConsoleProbe \/html:nth-child\(1\)\/body:nth-child\(2\)\/.+: hydration mismatch/,
  );
});
