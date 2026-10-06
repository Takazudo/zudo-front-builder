import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";

const REPO_ROOT = process.cwd();
const ARTIFACTS =
  process.env.ZFB_BOUNDARY_ACCEPTANCE_ARTIFACTS ??
  join(REPO_ROOT, "target", "scanner-boundary-acceptance");
const expected = JSON.parse(readFileSync(join(ARTIFACTS, "acceptance.json"), "utf8"));

test("packed and local targets share exact minified SSR/client identity and hydrate", async ({
  page,
}) => {
  const pageErrors = [];
  const consoleErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });

  // Observe the real manifest passed to mountIslands. Values are filtered by
  // the public identity + mount shape, so unrelated null-prototype objects do
  // not enter the marker list.
  await page.addInitScript(() => {
    globalThis.__zfbBoundaryAcceptanceRegistryKeys = [];
    const originalCreate = Object.create;
    Object.create = function (prototype, properties) {
      const target = Reflect.apply(originalCreate, Object, [prototype, properties]);
      if (prototype !== null) return target;
      return new Proxy(target, {
        set(object, key, value, receiver) {
          if (
            value &&
            typeof value === "object" &&
            value.identity?.component === String(key) &&
            typeof value.mount === "function"
          ) {
            globalThis.__zfbBoundaryAcceptanceRegistryKeys.push(String(key));
          }
          return Reflect.set(object, key, value, receiver);
        },
      });
    };
  });

  await page.goto("/");

  const islands = page.locator("[data-zfb-island]");
  await expect(islands).toHaveCount(expected.wrapperCount);
  const ssrMarkers = await islands.evaluateAll((nodes) =>
    [...new Set(nodes.map((node) => node.getAttribute("data-zfb-island")))].sort(),
  );
  expect(ssrMarkers).toEqual([...expected.pageMarkers].sort());

  const clientRegistryWrites = await page.evaluate(() =>
    [...globalThis.__zfbBoundaryAcceptanceRegistryKeys].sort(),
  );
  expect(clientRegistryWrites).toEqual([...expected.markers].sort());

  for (let index = 0; index < expected.wrapperCount; index += 1) {
    await expect(islands.nth(index)).toHaveAttribute("data-zfb-island-mounted", "");
  }

  const tokens = await page
    .locator("[data-zfb-build]")
    .evaluateAll((nodes) =>
      [...new Set(nodes.map((node) => node.getAttribute("data-zfb-build")))].sort(),
    );
  expect(tokens).toHaveLength(1);
  expect(tokens[0]).toMatch(/^[0-9a-f]{16}$/);

  const assets = join(ARTIFACTS, "dist", "assets");
  const islandBundleName = readdirSync(assets).find((name) => /^islands-.+\.js$/.test(name));
  expect(islandBundleName).toBeDefined();
  expect(readFileSync(join(assets, islandBundleName), "utf8")).toContain(tokens[0]);

  const consumer = page.locator("#consumer-a");
  await expect(consumer).toHaveText("Consumer A: 0");
  await consumer.click();
  await expect(consumer).toHaveText("Consumer A: 1");

  const mdxCounter = page.locator("#mdx-counter");
  await expect(mdxCounter).toHaveText("Counter: 0");
  const defaultMapIsland = page.locator('[data-zfb-island="Counter"]');
  await expect(defaultMapIsland).toHaveCount(1);
  await expect(defaultMapIsland).toHaveAttribute("data-props", "{}");
  expect(await defaultMapIsland.evaluate((island) => island.parentElement?.tagName)).toBe("BODY");
  expect(
    await mdxCounter.evaluate((button) => ({
      parentMarker: button.parentElement?.getAttribute("data-zfb-island"),
      parentElementCount: button.parentElement?.children.length,
    })),
  ).toEqual({ parentMarker: "Counter", parentElementCount: 1 });
  await mdxCounter.click();
  await expect(mdxCounter).toHaveText("Counter: 1");

  const factoryCounter = page.locator("#factory-counter");
  await expect(factoryCounter).toHaveText("Factory counter: 0");
  await factoryCounter.click();
  await expect(factoryCounter).toHaveText("Factory counter: 1");

  const defaultPanel = page.locator("#default-panel");
  await expect(defaultPanel).toHaveText("Default panel: 0");
  await defaultPanel.click();
  await expect(defaultPanel).toHaveText("Default panel: 1");

  const namedMdxCounter = page.locator("#mdx-named-counter");
  await expect(namedMdxCounter).toHaveText("Named counter: 0");
  const namedMapIsland = page.locator('[data-zfb-island="NamedCounter"]');
  await expect(namedMapIsland).toHaveCount(1);
  await expect(namedMapIsland).toHaveAttribute("data-props", "{}");
  expect(await namedMapIsland.evaluate((island) => island.parentElement?.tagName)).toBe("BODY");
  expect(
    await namedMdxCounter.evaluate((button) => ({
      parentMarker: button.parentElement?.getAttribute("data-zfb-island"),
      parentElementCount: button.parentElement?.children.length,
    })),
  ).toEqual({ parentMarker: "NamedCounter", parentElementCount: 1 });
  await namedMdxCounter.click();
  await expect(namedMdxCounter).toHaveText("Named counter: 1");

  const localHelper = page.locator("#live-counter").first();
  await expect(localHelper).toHaveText("Live helper: LOCAL_LIVE_RESOURCE 0");
  await localHelper.click();
  await expect(localHelper).toHaveText("Live helper: LOCAL_LIVE_RESOURCE 1");

  const packed = page.locator("#packed-counter").first();
  await expect(packed).toHaveText("Packed default: PACKED_LIVE_RESOURCE 0");
  const wrapperStateBeforePackedClick = await islands.evaluateAll((nodes) =>
    nodes.map((node) => ({
      marker: node.getAttribute("data-zfb-island"),
      build: node.getAttribute("data-zfb-build"),
      mounted: node.hasAttribute("data-zfb-island-mounted"),
    })),
  );
  await page.evaluate(() => {
    globalThis.__zfbPackedCounterClickCount = 0;
  });
  await packed.click();
  try {
    await expect(packed).toHaveText("Packed default: PACKED_LIVE_RESOURCE 1", { timeout: 3000 });
  } catch (error) {
    const diagnostics = {
      packedText: await packed.innerText(),
      handlerCalls: await page.evaluate(() => globalThis.__zfbPackedCounterClickCount),
      signalSubscribers: await page.evaluate(() => globalThis.__zfbPackedSignalSubscribers),
      wrapperStateBeforePackedClick,
      pageErrors,
      consoleErrors,
    };
    throw new Error(
      `${error.message}\nPacked interaction diagnostics:\n${JSON.stringify(diagnostics, null, 2)}`,
    );
  }
  expect(await page.evaluate(() => globalThis.__zfbPackedCounterClickCount)).toBe(1);

  const equalDisplayName = page.locator("#equal-display-name");
  await expect(equalDisplayName).toHaveText("Equal displayName: 0");
  await equalDisplayName.click();
  await expect(equalDisplayName).toHaveText("Equal displayName: 1");

  await page.goto("/host-override/index.html");
  const overrideIslands = page.locator("[data-zfb-island]");
  await expect(overrideIslands).toHaveCount(1);
  await expect(overrideIslands).toHaveAttribute("data-zfb-island", "HostPanel");
  await expect(overrideIslands).toHaveAttribute("data-zfb-island-mounted", "");
  await expect(page.locator('[data-zfb-island="DefaultPanel"]')).toHaveCount(0);
  const hostPanel = page.locator("#host-panel");
  await expect(hostPanel).toHaveText("Host panel: 0");
  await hostPanel.click();
  await expect(hostPanel).toHaveText("Host panel: 1");

  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});

async function expectInvalidBuildIdentityToStayUnmounted(page, mutateHtml) {
  const identityErrors = [];
  page.on("console", (message) => {
    if (message.type() === "error" && message.text().includes("ZR_IDENTITY")) {
      identityErrors.push(message.text());
    }
  });
  await page.route(
    (url) => url.pathname === "/",
    async (route) => {
      const response = await route.fetch();
      const html = await response.text();
      const mutated = mutateHtml(html);
      expect(mutated).not.toBe(html);
      await route.fulfill({ response, body: mutated });
    },
  );

  await page.goto("/");
  const islands = page.locator("[data-zfb-island]");
  await expect(islands).toHaveCount(expected.wrapperCount);
  await expect.poll(() => identityErrors.length).toBeGreaterThan(0);
  await expect
    .poll(() =>
      islands.evaluateAll(
        (nodes) => nodes.filter((node) => node.hasAttribute("data-zfb-island-mounted")).length,
      ),
    )
    .toBe(0);
  expect(identityErrors.some((message) => message.includes("ZR_IDENTITY"))).toBe(true);
  await expect(page.locator("#consumer-a")).toHaveText("Consumer A: 0");
}

test("missing SSR build tokens fail closed before hydration", async ({ page }) => {
  await expectInvalidBuildIdentityToStayUnmounted(page, (html) =>
    html.replaceAll(/ data-zfb-build="[^"]*"/g, ""),
  );
});

test("mismatched SSR build tokens fail closed before hydration", async ({ page }) => {
  await expectInvalidBuildIdentityToStayUnmounted(page, (html) =>
    html.replaceAll(/data-zfb-build="[^"]*"/g, 'data-zfb-build="different-build"'),
  );
});
