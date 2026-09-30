// Level-4 persisted islands: built zfb runtime + built zudo-react SSR and client.
import { test, expect } from "@playwright/test";

for (const moveMode of ["native", "fallback"]) {
  test.describe(`real islands (${moveMode} moveBefore)`, () => {
    test.beforeEach(async ({ page }) => {
      await page.addInitScript(
        ({ fallback }) => {
          if (fallback) Element.prototype.moveBefore = undefined;
          window.__moveBeforeType = typeof Element.prototype.moveBefore;
          const events = [];
          document.addEventListener("zfb:page-load", () => events.push("load"));
          window.__realEvents = events;
          const pending = new Map();
          let next = 0;
          window.requestIdleCallback = (callback) => {
            const id = ++next;
            pending.set(id, callback);
            return id;
          };
          window.cancelIdleCallback = (id) => pending.delete(id);
          window.__idle = {
            pending,
            flush() {
              const callbacks = [...pending.values()];
              pending.clear();
              for (const callback of callbacks)
                callback({ didTimeout: false, timeRemaining: () => 50 });
            },
          };
        },
        { fallback: moveMode === "fallback" },
      );
    });

    test("preserves live roots and order, remounts changed identity and props, and disposes removals", async ({
      page,
    }) => {
      const errors = [];
      page.on("console", (message) => {
        if (message.type() === "error") errors.push(message.text());
      });
      page.on("pageerror", (error) => errors.push(error.message));
      await page.goto("/real-islands-a.html");
      expect(await page.evaluate(() => window.__moveBeforeType)).toBe(
        moveMode === "native" ? "function" : "undefined",
      );
      await page.waitForFunction(
        () => window.__realEvents?.length > 0 && window.__realIslands?.mounts.length === 3,
      );
      await page.evaluate(() => window.__idle.flush());
      await expect(page.locator("#island-idle")).toHaveAttribute("data-zfb-island-mounted", "");
      await expect(page.locator("#island-first")).toHaveAttribute("data-zfb-island-mounted", "");
      await expect(page.locator("#island-second")).toHaveAttribute("data-zfb-island-mounted", "");
      await page.evaluate(() => {
        window.__saved = {
          header: document.querySelector("header"),
          toggle: document.querySelector("#island-toggle"),
          first: document.querySelector("#island-first"),
          second: document.querySelector("#island-second"),
          firstButton: document.querySelector("#button-first"),
          toggleRoot:
            document.querySelector("#island-toggle")[
              Symbol.for("@takazudo/zfb/zudo-react/root-v1")
            ],
          firstRoot:
            document.querySelector("#island-first")[Symbol.for("@takazudo/zfb/zudo-react/root-v1")],
        };
      });
      await page.locator("#toggle-button").click();
      await expect(page.locator("#toggle-svg")).toBeVisible();
      await page.locator("#button-first").click();
      await expect(page.locator("#value-first")).toHaveText("2");
      await page.locator("#persistent-input").focus();
      await page.locator("#persistent-input").evaluate((input) => input.setSelectionRange(2, 4));

      async function navigate(target) {
        const count = await page.evaluate(() => window.__realEvents.length);
        await page.locator(`#to-${target}`).evaluate((link) => link.click());
        await page.waitForFunction((previous) => window.__realEvents.length > previous, count);
        await expect(page.locator("h1")).toHaveText(`real islands ${target}`);
      }
      await navigate("b");
      await expect(page.locator("#toggle-svg")).toBeVisible();
      await expect(page.locator("#value-first")).toHaveText("2");
      await expect(page.locator("#value-second")).toHaveText("20");
      expect(
        await page.evaluate(() => ({
          header: document.querySelector("header") === window.__saved.header,
          toggle: document.querySelector("#island-toggle") === window.__saved.toggle,
          first: document.querySelector("#island-first") === window.__saved.first,
          second: document.querySelector("#island-second") === window.__saved.second,
          toggleRoot:
            document.querySelector("#island-toggle")[
              Symbol.for("@takazudo/zfb/zudo-react/root-v1")
            ] === window.__saved.toggleRoot,
          firstRoot:
            document.querySelector("#island-first")[
              Symbol.for("@takazudo/zfb/zudo-react/root-v1")
            ] === window.__saved.firstRoot,
          focus: document.activeElement?.id,
          caret: [
            document.querySelector("#persistent-input").selectionStart,
            document.querySelector("#persistent-input").selectionEnd,
          ],
        })),
      ).toEqual({
        header: true,
        toggle: true,
        first: true,
        second: true,
        toggleRoot: true,
        firstRoot: true,
        focus: "persistent-input",
        caret: [2, 4],
      });
      await expect(page.locator("#island-toggle")).toHaveAttribute("data-zfb-island-mounted", "");
      expect(
        await page.evaluate(() => ({
          mounts: window.__realIslands.mounts.map(({ id, mode }) => [id, mode]),
          disposals: window.__realIslands.disposals.map(({ id }) => id),
        })),
      ).toEqual({
        mounts: [
          ["Toggle", "hydrate"],
          ["first", "hydrate"],
          ["second", "hydrate"],
          ["deferred", "hydrate"],
          ["second", "render"],
        ],
        disposals: ["second"],
      });
      await page.locator("#button-second").click();
      await expect(page.locator("#value-second")).toHaveText("21");

      await navigate("a");
      await expect(page.locator("#toggle-svg")).toBeVisible();
      await expect(page.locator("#value-first")).toHaveText("2");
      await expect(page.locator("#value-second")).toHaveText("10");
      expect(await page.evaluate(() => window.__realIslands.disposals.map(({ id }) => id))).toEqual(
        ["second", "second"],
      );

      await navigate("c");
      await expect(page.locator("#value-second")).toHaveText("20");
      expect(
        await page.evaluate(() => ({
          lastMount: window.__realIslands.mounts.at(-1).name,
          lastMode: window.__realIslands.mounts.at(-1).mode,
          disposals: window.__realIslands.disposals.map(({ id }) => id),
        })),
      ).toEqual({
        lastMount: "OtherCounter",
        lastMode: "render",
        disposals: ["second", "second", "second"],
      });
      await page.locator("#button-second").click();
      await expect(page.locator("#value-second")).toHaveText("21");

      await page.evaluate(() => {
        window.__removedButton = document.querySelector("#button-second");
        window.__removedValue = document.querySelector("#value-second");
      });
      await navigate("removed");
      await expect(page.locator("#island-second")).toHaveCount(0);
      expect(await page.evaluate(() => window.__realIslands.disposals.map(({ id }) => id))).toEqual(
        ["second", "second", "second", "second"],
      );
      // A disposed, detached button has no live listener.
      expect(
        await page.evaluate(() => {
          const before = window.__removedValue.textContent;
          window.__removedButton.click();
          return [before, window.__removedValue.textContent];
        }),
      ).toEqual(["21", "21"]);
      await page.evaluate(() => window.__saved.firstButton.click());
      await expect(page.locator("#value-first")).toHaveText("3");
      await navigate("gone");
      await expect(page.locator("header")).toHaveCount(0);
      expect(
        await page.evaluate(() => window.__realIslands.disposals.map(({ id }) => id).sort()),
      ).toEqual(["Toggle", "first", "deferred", "second", "second", "second", "second"].sort());
      expect(
        errors.filter(
          (error) => error.includes("ZR_HYDRATION_MISMATCH") || error.includes("[zfb]"),
        ),
      ).toEqual([]);
      expect(errors).toEqual([]);
    });

    test("cancels an idle mount before swap and activates its retained island once", async ({
      page,
    }) => {
      const errors = [];
      page.on("console", (message) => {
        if (message.type() === "error") errors.push(message.text());
      });
      page.on("pageerror", (error) => errors.push(error.message));
      await page.goto("/real-islands-a.html");
      await page.waitForFunction(
        () => window.__realEvents?.length > 0 && window.__idle?.pending.size === 1,
      );
      const oldIdleId = await page.evaluate(() => [...window.__idle.pending.keys()][0]);
      const count = await page.evaluate(() => window.__realEvents.length);
      await page.locator("#to-b").click();
      await page.waitForFunction((previous) => window.__realEvents.length > previous, count);
      await expect(page.locator("#island-idle")).not.toHaveAttribute("data-zfb-island-mounted", "");
      expect(await page.evaluate(() => [...window.__idle.pending.keys()])).toEqual([oldIdleId + 1]);
      await page.evaluate(() => window.__idle.flush());
      await expect(page.locator("#island-idle")).toHaveAttribute("data-zfb-island-mounted", "");
      expect(
        await page.evaluate(() =>
          window.__realIslands.mounts
            .filter((entry) => entry.id === "deferred")
            .map((entry) => entry.mode),
        ),
      ).toEqual(["hydrate"]);
      expect(errors).toEqual([]);
    });
  });
}
