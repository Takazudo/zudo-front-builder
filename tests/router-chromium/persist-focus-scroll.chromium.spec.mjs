/**
 * Persisted focus vs. the router's scroll settle (#4893).
 *
 * A focused element inside a `data-zfb-transition-persist` subtree is moved into
 * the incoming body by the swap. Chromium keeps that element focused across
 * `moveBefore()` and scrolls it into view at the next layout. Without the fix
 * that layout only happens after the router's settle (scroll to top / to the
 * hash / to the traversed entry), so the reveal wins and the page lands on the
 * focused element instead of the settle target.
 *
 * Fixture: persist-focus-long-a.html, persist-focus-long-b.html (long pages with
 * a #deep hash target) and persist-focus-short.html (not scrollable). Each has a
 * persisted <header> with an <input> at the top and a persisted <footer> with a
 * <textarea> and a <button> at the bottom.
 *
 * Observation discipline: nothing in the harness reads layout between the swap
 * and the settle. A layout read there (e.g. `scrollY` in a wrapped swap) flushes
 * the pending reveal early and hides the defect, so custom-swap wrappers below
 * only count calls. Waits are keyed on the router's own `zfb:page-load` event
 * plus two animation frames, which guarantees a rendering update (where the
 * deferred reveal lands) has run. No timeouts.
 */

// @ts-check
import { test, expect } from "@playwright/test";

const LONG_A = "/persist-focus-long-a.html";
const LONG_B = "/persist-focus-long-b.html";
const SHORT = "/persist-focus-short.html";

/** @param {{ mode: string }} arg */
const HARNESS = ({ mode }) => {
  // @ts-expect-error - test-only globals
  const w = /** @type {any} */ (window);
  if (mode === "fallback") {
    // The router reads `!!document.startViewTransition` at module evaluation.
    // @ts-expect-error - removing the native API forces the simulated transition
    delete Document.prototype.startViewTransition;
  }
  w.__pf = { seq: 0, events: [], nativeTransitions: 0, swaps: 0 };
  for (const name of ["zfb:before-swap", "zfb:page-load", "zfb:navigation-aborted"]) {
    document.addEventListener(name, () => w.__pf.events.push({ name, seq: ++w.__pf.seq }));
  }
  const native = document.startViewTransition?.bind(document);
  if (native) {
    document.startViewTransition = (/** @type {any} */ cb) => {
      w.__pf.nativeTransitions++;
      return native(cb);
    };
  }
};

const MODES = [
  { mode: "native", reducedMotion: "no-preference", native: true },
  { mode: "fallback", reducedMotion: "no-preference", native: false },
  { mode: "reduced-motion", reducedMotion: "reduce", native: true },
];

/**
 * @param {import("@playwright/test").Page} page
 * @param {string} url
 * @param {(typeof MODES)[number]} m
 */
async function open(page, url, m) {
  await page.emulateMedia({ reducedMotion: /** @type {any} */ (m.reducedMotion) });
  await page.addInitScript(HARNESS, { mode: m.mode });
  await page.goto(url);
  await page.waitForFunction(
    () => typeof (/** @type {any} */ (window).__pfNavigate) === "function",
  );
}

/** @param {import("@playwright/test").Page} page */
const seq = (page) => page.evaluate(() => /** @type {any} */ (window).__pf.seq);

/**
 * @param {import("@playwright/test").Page} page
 * @param {number} after
 */
async function settled(page, after) {
  await page.waitForFunction(
    (base) =>
      /** @type {any} */ (window).__pf.events.some(
        (/** @type {any} */ e) => e.name === "zfb:page-load" && e.seq > base,
      ),
    after,
  );
  await page.evaluate(
    () => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))),
  );
}

/** @param {import("@playwright/test").Page} page */
const scrollYOf = (page) => page.evaluate(() => scrollY);

/** @param {import("@playwright/test").Page} page */
const deepTarget = (page) =>
  page.evaluate(() => {
    const top = document.getElementById("deep").getBoundingClientRect().top + scrollY;
    return Math.min(top, document.documentElement.scrollHeight - innerHeight);
  });

/**
 * Mark the element so the test can prove the SAME node is focused afterwards.
 * @param {import("@playwright/test").Page} page
 * @param {string} id
 */
const mark = (page, id) =>
  page.evaluate((id) => {
    /** @type {any} */ (document.getElementById(id)).__pfMark = true;
  }, id);

/** @param {import("@playwright/test").Page} page */
const focused = (page) =>
  page.evaluate(() => {
    const el = /** @type {any} */ (document.activeElement);
    return {
      id: el?.id ?? null,
      same: el?.__pfMark === true,
      start: el?.selectionStart ?? null,
      end: el?.selectionEnd ?? null,
    };
  });

/**
 * Pointer focus on the footer textarea at the bottom of the current page, with
 * a non-collapsed selection so caret/selection restore is observable.
 * @param {import("@playwright/test").Page} page
 */
async function pointerFocusFooterTextarea(page) {
  await page.evaluate(() => scrollTo(0, document.documentElement.scrollHeight));
  await page.click("#pf-footer-input");
  await page.evaluate(() =>
    /** @type {HTMLTextAreaElement} */ (
      document.getElementById("pf-footer-input")
    ).setSelectionRange(2, 6),
  );
  await mark(page, "pf-footer-input");
}

/**
 * Keyboard focus on the footer button: Tab from the textarea.
 * @param {import("@playwright/test").Page} page
 */
async function keyboardFocusFooterButton(page) {
  await page.evaluate(() => scrollTo(0, document.documentElement.scrollHeight));
  await page.click("#pf-footer-input");
  await page.keyboard.press("Tab");
  expect(await page.evaluate(() => document.activeElement?.id)).toBe("pf-footer-button");
  expect(await page.evaluate(() => document.activeElement?.matches(":focus-visible"))).toBe(true);
  await mark(page, "pf-footer-button");
}

/**
 * Navigate by pressing Enter in the focused persisted control.
 * @param {import("@playwright/test").Page} page
 * @param {string} href
 */
async function enterTo(page, href) {
  const base = await seq(page);
  await page.evaluate((href) => {
    /** @type {any} */ (window).__pfEnterTarget = href;
  }, href);
  await page.keyboard.press("Enter");
  await settled(page, base);
}

/**
 * @param {import("@playwright/test").Page} page
 * @param {string} href
 */
async function navigateTo(page, href) {
  const base = await seq(page);
  await page.evaluate((href) => /** @type {any} */ (window).__pfNavigate(href), href);
  await settled(page, base);
}

/**
 * @param {import("@playwright/test").Page} page
 * @param {(typeof MODES)[number]} m
 */
async function expectTransitionPath(page, m) {
  const calls = await page.evaluate(() => /** @type {any} */ (window).__pf.nativeTransitions);
  if (m.native) expect(calls).toBeGreaterThan(0);
  else expect(calls).toBe(0);
}

for (const m of MODES) {
  test.describe(`persisted focus keeps the settle scroll (${m.mode})`, () => {
    test("top: long → long, pointer-focused footer textarea keeps its selection", async ({
      page,
    }) => {
      await open(page, LONG_A, m);
      await pointerFocusFooterTextarea(page);
      expect(await scrollYOf(page)).toBeGreaterThan(1000);

      await navigateTo(page, LONG_B);

      expect(await page.evaluate(() => location.pathname)).toBe(LONG_B);
      expect(await scrollYOf(page)).toBe(0);
      expect(await focused(page)).toEqual({ id: "pf-footer-input", same: true, start: 2, end: 6 });
      await expectTransitionPath(page, m);
    });

    test("top: short → long, keyboard-focused footer button, Enter navigates", async ({ page }) => {
      await open(page, SHORT, m);
      await keyboardFocusFooterButton(page);
      expect(await scrollYOf(page)).toBe(0);

      await enterTo(page, LONG_B);

      expect(await page.evaluate(() => location.pathname)).toBe(LONG_B);
      expect(await scrollYOf(page)).toBe(0);
      expect((await focused(page)).same).toBe(true);
      await expectTransitionPath(page, m);
    });

    test("top: long → short, focused footer stays focused on a page that cannot scroll", async ({
      page,
    }) => {
      await open(page, LONG_A, m);
      await pointerFocusFooterTextarea(page);

      await enterTo(page, SHORT);

      expect(await page.evaluate(() => location.pathname)).toBe(SHORT);
      expect(await scrollYOf(page)).toBe(0);
      expect(await focused(page)).toEqual({ id: "pf-footer-input", same: true, start: 2, end: 6 });
    });

    test("hash: long → long#deep with the footer textarea focused", async ({ page }) => {
      await open(page, LONG_A, m);
      await pointerFocusFooterTextarea(page);

      await navigateTo(page, `${LONG_B}#deep`);

      const target = await deepTarget(page);
      expect(target).toBeGreaterThan(1000);
      expect(Math.abs((await scrollYOf(page)) - target)).toBeLessThanOrEqual(1);
      // Focus is not asserted: scrolling to a fragment runs the focusing steps
      // for its target (here the viewport), so the persisted control is blurred
      // by the browser on the base runtime too.
    });

    test("hash: long → long#deep with the header input focused by keyboard", async ({ page }) => {
      await open(page, LONG_A, m);
      await page.keyboard.press("Tab");
      expect(await page.evaluate(() => document.activeElement?.id)).toBe("pf-header-input");

      await enterTo(page, `${LONG_B}#deep`);

      const target = await deepTarget(page);
      expect(Math.abs((await scrollYOf(page)) - target)).toBeLessThanOrEqual(1);
    });

    for (const savedY of [0, 1500]) {
      test(`history back restores scrollY ${savedY} with the footer focused`, async ({ page }) => {
        await open(page, LONG_A, m);
        await page.evaluate((y) => scrollTo(0, y), savedY);
        await navigateTo(page, LONG_B);
        await pointerFocusFooterTextarea(page);

        const base = await seq(page);
        await page.goBack();
        await settled(page, base);

        expect(await page.evaluate(() => location.pathname)).toBe(LONG_A);
        expect(await scrollYOf(page)).toBe(savedY);
        expect(await focused(page)).toEqual({
          id: "pf-footer-input",
          same: true,
          start: 2,
          end: 6,
        });
      });
    }

    test("a consumer-replaced swap runs exactly once and the settle still wins", async ({
      page,
    }) => {
      await open(page, LONG_A, m);
      // Counts only: reading layout in here would flush the reveal itself.
      await page.evaluate(() =>
        document.addEventListener("zfb:before-swap", (event) => {
          const original = /** @type {any} */ (event).swap;
          /** @type {any} */ (event).swap = () => {
            /** @type {any} */ (window).__pf.swaps++;
            original();
          };
        }),
      );
      await pointerFocusFooterTextarea(page);

      await navigateTo(page, LONG_B);

      expect(await page.evaluate(() => /** @type {any} */ (window).__pf.swaps)).toBe(1);
      expect(await scrollYOf(page)).toBe(0);
      expect((await focused(page)).same).toBe(true);
    });

    test("a superseded navigation is aborted and the winning one still settles to top", async ({
      page,
    }) => {
      await open(page, SHORT, m);
      // Hold the first navigation in its loader so the second one supersedes it.
      await page.evaluate(() =>
        document.addEventListener(
          "zfb:before-preparation",
          (event) => {
            const ev = /** @type {any} */ (event);
            const load = ev.loader;
            ev.loader = async () => {
              await new Promise((release) => {
                /** @type {any} */ (window).__pfRelease = release;
              });
              await load();
            };
          },
          { once: true },
        ),
      );
      await page.click("#pf-footer-input");
      await mark(page, "pf-footer-input");

      const base = await seq(page);
      await page.evaluate((href) => {
        void (/** @type {any} */ (window).__pfNavigate(href));
      }, LONG_A);
      await page.waitForFunction(
        () => typeof (/** @type {any} */ (window).__pfRelease) === "function",
      );
      await navigateTo(page, LONG_B);
      await page.evaluate(() => /** @type {any} */ (window).__pfRelease());
      await page.evaluate(
        () => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))),
      );

      const events = await page.evaluate(() => /** @type {any} */ (window).__pf.events);
      const after = events.filter((/** @type {any} */ e) => e.seq > base).map((e) => e.name);
      expect(after).toContain("zfb:navigation-aborted");
      expect(after.filter((/** @type {string} */ n) => n === "zfb:before-swap")).toHaveLength(1);
      expect(await page.evaluate(() => location.pathname)).toBe(LONG_B);
      expect(await scrollYOf(page)).toBe(0);
      expect((await focused(page)).same).toBe(true);
    });
  });
}
