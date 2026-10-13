// @ts-check
import { test, expect } from "@playwright/test";
import {
  expectNoBrowserErrors,
  openHeldScenario,
  releaseScenario,
  rootSelector,
} from "./browser-helpers.mjs";

async function counts(page, id) {
  return page.evaluate((key) => {
    const state = window.__zudoReactBrowser.roots[0].result[key];
    return {
      count: state.count.value,
      good: state.good,
      attempt: state.attempt,
      working: state.working,
      failed: state.failed,
      fallback: state.fallback,
    };
  }, id);
}

async function rememberGoodNodes(page) {
  await page.evaluate(() => {
    window.__boundaryGoodNodes = Object.fromEntries(
      ["interactive", "server-failed", "client-failed"].map((id) => [
        id,
        document.querySelector(`#good-${id}`),
      ]),
    );
  });
}

async function expectGoodNodePreserved(page, id) {
  expect(
    await page.evaluate((key) => {
      const previous = window.__boundaryGoodNodes[key];
      return previous === document.querySelector(`#good-${key}`) && previous.isConnected;
    }, id),
  ).toBe(true);
}

test("subtree failure, explicit retry and reset-key recovery preserve a hydrated stateful sibling and dispose each attempt once", async ({
  page,
}) => {
  const state = await openHeldScenario(page, "error-boundary");
  try {
    await rememberGoodNodes(page);
    await releaseScenario(page, state);
    await expectGoodNodePreserved(page, "interactive");
    await page.locator("#good-interactive").click();
    await expect(page.locator("#good-interactive")).toHaveText("Good 1");
    expect(await counts(page, "interactive")).toMatchObject({
      good: { setups: 1, activations: 1, cleanups: 0 },
      attempt: { setups: 1, activations: 1, cleanups: 0 },
      working: { setups: 1, activations: 1, cleanups: 0 },
      failed: { setups: 0, activations: 0, cleanups: 0 },
      fallback: { setups: 0, activations: 0, cleanups: 0 },
    });

    await page.locator("#fail-interactive").click();
    await expect(page.locator("#fallback-interactive output")).toHaveText("boundary branch failed");
    await expect(page.locator("#working-interactive")).toHaveCount(0);
    await expectGoodNodePreserved(page, "interactive");
    expect(await counts(page, "interactive")).toMatchObject({
      count: 1,
      good: { setups: 1, activations: 1, cleanups: 0 },
      attempt: { setups: 1, activations: 1, cleanups: 1 },
      working: { setups: 1, activations: 1, cleanups: 1 },
      failed: { setups: 1, activations: 0, cleanups: 1 },
      fallback: { setups: 1, activations: 1, cleanups: 0 },
    });

    // Changing the failed input alone must not implicitly retry a latched
    // fallback; only the supplied reset callback or resetKey requests recovery.
    await page.evaluate(async () => {
      window.__zudoReactBrowser.roots[0].result.interactive.fail.value = false;
      await window.__zudoReactBrowser.flush();
    });
    await expect(page.locator("#fallback-interactive")).toHaveCount(1);
    await page.locator("#retry-interactive").click();
    await expect(page.locator("#working-interactive")).toHaveText("Working child");
    await expect(page.locator("#fallback-interactive")).toHaveCount(0);
    await expectGoodNodePreserved(page, "interactive");
    expect(await counts(page, "interactive")).toMatchObject({
      attempt: { setups: 2, activations: 2, cleanups: 1 },
      working: { setups: 2, activations: 2, cleanups: 1 },
      failed: { setups: 1, activations: 0, cleanups: 1 },
      fallback: { setups: 1, activations: 1, cleanups: 1 },
    });

    await page.locator("#fail-interactive").click();
    await expect(page.locator("#fallback-interactive")).toHaveCount(1);
    await page.locator("#key-reset-interactive").click();
    await expect(page.locator("#working-interactive")).toHaveText("Working child");
    await expect(page.locator("#fallback-interactive")).toHaveCount(0);
    await expectGoodNodePreserved(page, "interactive");
    await page.locator("#good-interactive").click();
    await expect(page.locator("#good-interactive")).toHaveText("Good 2");
    expect(await counts(page, "interactive")).toMatchObject({
      count: 2,
      good: { setups: 1, activations: 1, cleanups: 0 },
      attempt: { setups: 3, activations: 3, cleanups: 2 },
      working: { setups: 3, activations: 3, cleanups: 2 },
      failed: { setups: 2, activations: 0, cleanups: 2 },
      fallback: { setups: 2, activations: 2, cleanups: 2 },
    });

    await page.evaluate(async () => {
      const harness = window.__zudoReactBrowser;
      harness.roots[0].root.dispose();
      harness.roots[0].root.dispose();
      harness.roots[0].result.interactive.resetKey.value++;
      await harness.flush();
    });
    expect(await counts(page, "interactive")).toMatchObject({
      good: { setups: 1, activations: 1, cleanups: 1 },
      attempt: { setups: 3, activations: 3, cleanups: 3 },
      working: { setups: 3, activations: 3, cleanups: 3 },
      failed: { setups: 2, activations: 0, cleanups: 2 },
      fallback: { setups: 2, activations: 2, cleanups: 2 },
    });
    expectNoBrowserErrors(state.errors);
  } finally {
    await releaseScenario(page, state).catch(() => undefined);
    await state.heldModule.remove();
  }
});

test("server/client failure disagreement replaces only the boundary range and hydrates each healthy sibling once", async ({
  page,
}) => {
  const state = await openHeldScenario(page, "error-boundary");
  try {
    await expect(page.locator("#fallback-server-failed output")).toHaveText(
      "boundary branch failed",
    );
    await expect(page.locator("#working-client-failed")).toHaveText("Working child");
    await rememberGoodNodes(page);
    await releaseScenario(page, state);

    await expect(page.locator("#working-server-failed")).toHaveText("Working child");
    await expect(page.locator("#fallback-server-failed")).toHaveCount(0);
    await expect(page.locator("#fallback-client-failed output")).toHaveText(
      "boundary branch failed",
    );
    await expect(page.locator("#working-client-failed")).toHaveCount(0);
    for (const [id, index] of [
      ["server-failed", 1],
      ["client-failed", 2],
    ]) {
      await expectGoodNodePreserved(page, id);
      await page.locator(`#good-${id}`).click();
      await expect(page.locator(`#good-${id}`)).toHaveText("Good 1");
      expect(await counts(page, id)).toMatchObject({
        good: { setups: 1, activations: 1, cleanups: 0 },
        count: 1,
      });
      expect(
        await page.evaluate((rootIndex) => {
          const entry = window.__zudoReactBrowser.roots[rootIndex];
          return { active: entry.root !== null, diagnostics: entry.diagnostics };
        }, index),
      ).toEqual({ active: true, diagnostics: [] });
      await expect(page.locator(`${rootSelector(index)} button`).first()).toBeVisible();
    }
    expect(await counts(page, "server-failed")).toMatchObject({
      attempt: { setups: 1, activations: 1, cleanups: 0 },
      working: { setups: 1, activations: 1, cleanups: 0 },
      failed: { setups: 0, activations: 0, cleanups: 0 },
      fallback: { setups: 0, activations: 0, cleanups: 0 },
    });
    expect(await counts(page, "client-failed")).toMatchObject({
      attempt: { setups: 1, activations: 0, cleanups: 1 },
      working: { setups: 0, activations: 0, cleanups: 0 },
      failed: { setups: 1, activations: 0, cleanups: 1 },
      fallback: { setups: 1, activations: 1, cleanups: 0 },
    });
    expectNoBrowserErrors(state.errors);
  } finally {
    await releaseScenario(page, state).catch(() => undefined);
    await state.heldModule.remove();
  }
});
