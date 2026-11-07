/**
 * form-unusable-response.chromium.spec.mjs — Level-4 regression for #4622.
 *
 * When the router has already sent a routed form POST and cannot use the
 * response (non-HTML body, failed fetch), it must end the navigation with
 * `zfb:navigation-aborted` and keep the page. It must not follow up with a
 * GET of the action URL: "A submitted request is never sent twice".
 *
 * Waits are keyed on router events and on a sentinel request, never on a bare
 * timeout: after the abort the page issues one sentinel fetch and awaits it, so
 * any request the fallback started has been routed by then, and a document
 * load would have torn the page down before the sentinel resolved.
 */

// @ts-check
import { test, expect } from "@playwright/test";

const HARNESS_INIT = () => {
  // @ts-expect-error - test-only globals
  if (window.__zfbHarness) return;
  // @ts-expect-error
  window.__zfbHarness = true;
  // @ts-expect-error
  window.__docId = crypto.randomUUID();
  const rec = { events: [], seq: 0 };
  // @ts-expect-error
  window.__zfb = rec;
  for (const name of ["zfb:page-load", "zfb:navigation-aborted", "zfb:after-swap"]) {
    document.addEventListener(name, () => rec.events.push({ name, seq: ++rec.seq }));
  }
};

const CASES = [
  {
    name: "a JSON response",
    respond: (route) =>
      route.fulfill({ status: 200, contentType: "application/json", body: '{"ok":true}' }),
  },
  { name: "a failed fetch", respond: (route) => route.abort("failed") },
];

for (const c of CASES) {
  test(`routed POST after ${c.name} ends without a second request`, async ({ page }) => {
    await page.addInitScript(HARNESS_INIT);
    /** @type {string[]} */
    const requests = [];
    await page.route("**/search-results.html*", async (route) => {
      const req = route.request();
      const url = new URL(req.url());
      requests.push(`${req.method()} ${url.pathname}${url.search}`);
      if (url.searchParams.has("sentinel")) {
        return route.fulfill({ status: 204, body: "" });
      }
      if (req.method() === "POST") return c.respond(route);
      return route.fulfill({ status: 405, contentType: "text/plain", body: "GET not allowed" });
    });

    await page.goto("/post-form.html");
    await page.waitForFunction(() =>
      /** @type {any} */ (window).__zfb.events.some((e) => e.name === "zfb:page-load"),
    );
    const docId = await page.evaluate(() => /** @type {any} */ (window).__docId);
    const startUrl = page.url();

    await page.click("#post-submit");
    await page.waitForFunction(() =>
      /** @type {any} */ (window).__zfb.events.some((e) => e.name === "zfb:navigation-aborted"),
    );
    await page.evaluate(() => fetch("/search-results.html?sentinel=1").then(() => undefined));

    expect(requests).toEqual(["POST /search-results.html", "GET /search-results.html?sentinel=1"]);
    expect(await page.evaluate(() => /** @type {any} */ (window).__docId)).toBe(docId);
    expect(page.url()).toBe(startUrl);
    expect(
      await page.evaluate(
        () =>
          /** @type {any} */ (window).__zfb.events.filter((e) => e.name === "zfb:after-swap")
            .length,
      ),
    ).toBe(0);
  });
}
