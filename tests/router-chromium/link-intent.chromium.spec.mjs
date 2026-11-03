// Real destinations establish the native target oracle; a retained sentinel proves SPA navigation.
import { test, expect } from "@playwright/test";

const cases = [
  { name: "omitted target", base: "_blank", context: "popup" },
  { name: "named base", base: "results", context: "frame" },
  { name: "self override", base: "_blank", target: "_self", context: "self" },
  { name: "mixed self override", base: "_blank", target: "_SeLf", context: "self" },
  { name: "empty override", base: "_blank", target: "", context: "popup" },
  { name: "first target base", base: "results", second: "_blank", context: "frame" },
  { name: "first empty base", base: "", second: "_blank", context: "self" },
];

for (const tag of ["a", "area"]) {
  for (const config of cases) {
    for (const native of [true, false]) {
      test(`${tag} ${config.name}: ${native ? "native oracle" : "router"}`, async ({ page }) => {
        await page.goto("/search.html");
        await page.waitForFunction(() => history.state?.index !== undefined);
        await page.evaluate(
          ({ tag, config, native }) => {
            window.__intentSentinel = "retained";
            window.__intentDone = false;
            document.addEventListener("zfb:page-load", () => {
              window.__intentDone = true;
            });
            // A preceding base without target does not participate in target selection.
            document.head.append(document.createElement("base"));
            for (const target of [config.base, config.second]) {
              if (target === undefined) continue;
              const base = document.createElement("base");
              base.setAttribute("target", target);
              document.head.append(base);
            }
            const link = document.createElement(tag);
            link.id = "intent-link";
            link.href = "/search-results.html";
            link.textContent = "Go";
            if (config.target !== undefined) link.setAttribute("target", config.target);
            if (native) link.setAttribute("data-zfb-reload", "");
            document.body.append(link);
            if (config.context === "frame") {
              const frame = document.createElement("iframe");
              frame.name = "results";
              document.body.append(frame);
            }
          },
          { tag, config, native },
        );
        const before = await page.evaluate(() => ({
          url: location.href,
          state: history.state,
          length: history.length,
        }));
        const popupPromise = config.context === "popup" ? page.waitForEvent("popup") : null;
        // HTMLElement.click() activates both anchors and areas, including an area without a map.
        await page.locator("#intent-link").evaluate((link) => link.click());
        if (popupPromise) {
          const popup = await popupPromise;
          await expect(popup.locator("h1")).toHaveText("Search Results");
          await popup.close();
        } else if (config.context === "frame") {
          await expect(page.frameLocator('iframe[name="results"]').locator("h1")).toHaveText(
            "Search Results",
          );
        } else {
          await page.waitForURL(/search-results/);
          await expect(page.locator("h1")).toHaveText("Search Results");
          if (!native) await page.waitForFunction(() => window.__intentDone);
          expect(await page.evaluate(() => window.__intentSentinel)).toBe(
            native ? undefined : "retained",
          );
        }
        if (config.context !== "self") {
          expect(
            await page.evaluate(() => ({
              url: location.href,
              state: history.state,
              length: history.length,
            })),
          ).toEqual(before);
          expect(await page.evaluate(() => window.__intentSentinel)).toBe("retained");
          expect(await page.evaluate(() => window.__intentDone)).toBe(false);
        }
      });
    }
  }
}
