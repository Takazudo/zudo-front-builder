/**
 * WebKit native History-quota acceptance — LOCAL-HEAVY (T4), Mac only (#4128).
 *
 * ============================================================
 * PURPOSE
 * ============================================================
 * WebKit throws from `history.pushState`/`replaceState` once a document exceeds
 * its History-write budget (#4125). Before #4127 the router ignored that
 * rejection and swapped the new page in under the OLD URL. The fixed router
 * treats the rejection as a navigation failure and recovers with one document
 * load of the resolved destination.
 *
 * This spec proves that against WebKit's REAL native rejection:
 *   1. A fresh context primes the native budget with real, URL-less
 *      `history.replaceState` calls, bounded by an attempt cap and a deadline,
 *      and stops at the first native exception.
 *   2. In the same task it clicks an ordinary router link.
 *   3. An observational wrapper around `History.prototype.pushState` /
 *      `replaceState` calls the native method, records the outcome of every
 *      URL-carrying write (including the native exception's name and whether
 *      the call came from the built router module), and rethrows the exact
 *      native exception. It never throws on its own.
 *   4. The record lives in `sessionStorage`, so it survives the recovery
 *      document load, tagged per document.
 *
 * The acceptance requires that the ROUTER's own URL-changing write to the
 * destination was natively rejected — a priming failure alone does not count,
 * because the budget can recover before the router writes. If that
 * precondition is not reached, the test FAILS with a "PRECONDITION NOT
 * REACHED" message; it never passes silently. No call-count threshold is
 * assumed, no error message is matched, and nothing is paced or retried.
 *
 * Chromium never rejects these writes; the deterministic fault-injection
 * regression is tests/router-chromium/history-rejection.chromium.spec.mjs.
 *
 * Run: pnpm test:webkit-back (or, after building the runtime,
 *   pnpm exec playwright test --config tests/webkit-back-history/playwright.config.mjs history-quota)
 */

// @ts-check
import { test, expect } from "@playwright/test";

const LOG_KEY = "__quotaLog";
const PRIMING_MAX_ATTEMPTS = 2000;
const PRIMING_DEADLINE_MS = 5000;

const OBSERVER_INIT = () => {
  const w = /** @type {any} */ (window);
  if (w.__quotaDoc) return;
  const LOG_KEY = "__quotaLog";
  const doc = crypto.randomUUID();
  w.__quotaDoc = doc;
  const log = (entry) => {
    const all = JSON.parse(sessionStorage.getItem(LOG_KEY) ?? "[]");
    all.push({ doc, ...entry });
    sessionStorage.setItem(LOG_KEY, JSON.stringify(all));
  };
  log({ kind: "document", href: location.href });

  for (const method of /** @type {const} */ (["pushState", "replaceState"])) {
    const native = History.prototype[method];
    History.prototype[method] = function (_data, _unused, url) {
      if (arguments.length < 3 || url == null) return native.apply(this, arguments);
      const entry = {
        kind: "write",
        method,
        url: new URL(String(url), location.href).href,
        from: location.href,
        fromRouter: (new Error().stack ?? "").includes("/dist/client-router/"),
      };
      try {
        const result = native.apply(this, arguments);
        log({ ...entry, outcome: "committed" });
        return result;
      } catch (error) {
        log({
          ...entry,
          outcome: "native-rejection",
          errorName: /** @type {any} */ (error)?.name ?? null,
          errorIsDOMException: error instanceof DOMException,
        });
        throw error;
      }
    };
  }

  for (const name of [
    "zfb:before-preparation",
    "zfb:after-preparation",
    "zfb:before-swap",
    "zfb:after-swap",
    "zfb:page-load",
    "zfb:navigation-aborted",
  ]) {
    document.addEventListener(name, () => {
      log({
        kind: "event",
        name,
        href: location.href,
        h1: document.querySelector("h1")?.textContent ?? null,
      });
    });
  }
};

/** @param {import("@playwright/test").Page} page */
const readLog = (page) =>
  page.evaluate((key) => JSON.parse(sessionStorage.getItem(key) ?? "[]"), LOG_KEY);
/** @param {import("@playwright/test").Page} page */
const docId = (page) => page.evaluate(() => /** @type {any} */ (window).__quotaDoc);
const pathOf = (href) => new URL(href).pathname;

async function waitForPageLoad(page, doc, count = 1) {
  await expect
    .poll(
      async () =>
        (await readLog(page).catch(() => [])).filter(
          (e) => e.doc === doc && e.kind === "event" && e.name === "zfb:page-load",
        ).length,
      { timeout: 8000 },
    )
    .toBeGreaterThanOrEqual(count);
}

async function traverse(page, direction, expectedPath, expectedH1) {
  if (direction === "back") await page.goBack();
  else await page.goForward();
  await expect(page.locator("h1")).toHaveText(expectedH1, { timeout: 5000 });
  expect(pathOf(page.url())).toBe(expectedPath);
}

test("native History-quota rejection of the router's own push recovers to the destination", async ({
  page,
  browser,
}, testInfo) => {
  await page.addInitScript(OBSERVER_INIT);
  await page.goto("/index.html");
  const homeDoc = await docId(page);
  await waitForPageLoad(page, homeDoc);

  // One ordinary SPA entry first, so Back/Forward cross both an SPA entry and
  // the recovery document.
  await page.click("a[href='/list.html']");
  await expect(page.locator("h1")).toHaveText("List");
  await waitForPageLoad(page, homeDoc, 2);
  expect(await docId(page)).toBe(homeDoc);
  const lengthBefore = await page.evaluate(() => history.length);
  const mark = (await readLog(page)).length;

  // Prime with real native calls until the first native exception, then click
  // the router link in the same task so the router writes inside that window.
  const priming = await page.evaluate(
    ({ maxAttempts, deadlineMs }) => {
      const start = performance.now();
      let attempts = 0;
      let errorName = null;
      while (attempts < maxAttempts && performance.now() - start < deadlineMs) {
        attempts++;
        try {
          history.replaceState(history.state, "");
        } catch (error) {
          errorName = /** @type {any} */ (error)?.name ?? String(error);
          break;
        }
      }
      const elapsedMs = Math.round(performance.now() - start);
      if (errorName !== null)
        /** @type {HTMLElement} */ (document.getElementById("to-detail")).click();
      return { attempts, errorName, elapsedMs };
    },
    { maxAttempts: PRIMING_MAX_ATTEMPTS, deadlineMs: PRIMING_DEADLINE_MS },
  );

  const engine = `webkit ${browser.version()}`;
  testInfo.annotations.push({
    type: "priming",
    description: `${engine}: ${JSON.stringify(priming)}`,
  });
  console.log(`[history-quota] ${engine} priming ${JSON.stringify(priming)}`);
  expect(
    priming.errorName,
    `PRECONDITION NOT REACHED: ${engine} never rejected a native History write within ` +
      `${priming.attempts} attempts / ${priming.elapsedMs} ms`,
  ).not.toBeNull();

  // Settle on whichever outcome the router produced: a new document (recovery)
  // or an SPA swap in the primed document (the pre-#4127 defect).
  await expect
    .poll(
      async () => {
        const log = await readLog(page).catch(() => []);
        return log
          .slice(mark)
          .some(
            (e) =>
              (e.kind === "document" && e.doc !== homeDoc) ||
              (e.doc === homeDoc && e.name === "zfb:after-swap"),
          );
      },
      { timeout: 8000 },
    )
    .toBe(true);

  const log = await readLog(page);
  const routerWrites = log
    .slice(mark)
    .filter((e) => e.kind === "write" && e.doc === homeDoc && pathOf(e.url) === "/detail.html");
  console.log(`[history-quota] router writes ${JSON.stringify(routerWrites)}`);
  console.log(
    `[history-quota] lifecycle ${JSON.stringify(
      log
        .slice(mark)
        .filter((e) => e.kind !== "write")
        .map((e) => [e.doc === homeDoc ? "primed" : "new", e.kind, e.name ?? e.href, e.h1 ?? ""]),
    )}`,
  );
  const target = routerWrites[0];
  expect(
    target,
    "PRECONDITION NOT REACHED: the router never attempted its URL write to /detail.html",
  ).toBeDefined();
  expect(target.fromRouter).toBe(true);
  expect(
    target.outcome,
    "PRECONDITION NOT REACHED: the native History budget accepted the router's own write",
  ).toBe("native-rejection");
  testInfo.annotations.push({
    type: "router-write",
    description: `${target.method} ${target.url} -> ${target.errorName} (DOMException: ${target.errorIsDOMException})`,
  });

  // Recovery: no swap under the old URL, one aborted attempt, one new document.
  const primedEvents = log
    .slice(mark)
    .filter((e) => e.doc === homeDoc && e.kind === "event")
    .map((e) => e.name);
  expect(primedEvents).not.toContain("zfb:before-swap");
  expect(primedEvents).not.toContain("zfb:after-swap");
  expect(primedEvents).toContain("zfb:navigation-aborted");
  const aborted = log.slice(mark).find((e) => e.name === "zfb:navigation-aborted");
  expect(pathOf(aborted.href)).toBe("/list.html");
  expect(aborted.h1).toBe("List");

  const newDocs = log.slice(mark).filter((e) => e.kind === "document" && e.doc !== homeDoc);
  expect(newDocs.length).toBe(1);
  expect(pathOf(newDocs[0].href)).toBe("/detail.html");
  await waitForPageLoad(page, newDocs[0].doc);
  // WebKit has no navigation timing entry yet when the init script runs.
  expect(
    await page.evaluate(
      () => /** @type {any} */ (performance.getEntriesByType("navigation")[0])?.type,
    ),
  ).toBe("navigate");
  expect(await docId(page)).toBe(newDocs[0].doc);
  expect(pathOf(page.url())).toBe("/detail.html");
  await expect(page.locator("h1")).toHaveText("Detail");
  expect(await page.evaluate(() => history.length)).toBe(lengthBefore + 1);

  await traverse(page, "back", "/list.html", "List");
  await traverse(page, "back", "/index.html", "Home");
  await traverse(page, "forward", "/list.html", "List");
  await traverse(page, "forward", "/detail.html", "Detail");
  expect(await page.evaluate(() => history.length)).toBe(lengthBefore + 1);
});
