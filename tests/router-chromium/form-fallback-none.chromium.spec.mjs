/**
 * form-fallback-none.chromium.spec.mjs — Level-4 regression for #4625.
 *
 * In a browser without native view transitions, a page whose fallback is
 * "none" is not router-eligible. When such a page is reached by a soft
 * navigation the listeners are already installed, and the router used to turn
 * a POST submit into `location.href = action`: a GET without the body. The
 * submit must stay native (one POST with the body), like the native oracle.
 *
 * `startViewTransition` is removed before any page script runs, so the router
 * module sees an engine without view transitions. Waits key on router events
 * and on the server's request log, never on a bare timeout.
 */

// @ts-check
import { test, expect } from "@playwright/test";

const HARNESS_INIT = () => {
  // @ts-expect-error - test-only engine emulation
  delete Document.prototype.startViewTransition;
  // @ts-expect-error - test-only globals
  if (window.__zfbHarness) return;
  // @ts-expect-error
  window.__zfbHarness = true;
  const rec = { events: [], seq: 0 };
  // @ts-expect-error
  window.__zfb = rec;
  document.addEventListener("zfb:page-load", () =>
    rec.events.push({ name: "zfb:page-load", seq: ++rec.seq }),
  );
};

async function formRequests(page) {
  const res = await page.request.get("/__form-requests");
  return (await res.json()).map((r) => `${r.method} ${r.search} ${r.body}`);
}

for (const who of ["native oracle", "router"]) {
  test(`POST on a soft-navigated fallback="none" page: ${who}`, async ({ page }) => {
    await page.addInitScript(HARNESS_INIT);
    await formRequests(page);
    await page.goto("/post-form.html");
    await page.waitForFunction(() =>
      /** @type {any} */ (window).__zfb.events.some((e) => e.name === "zfb:page-load"),
    );
    expect(await page.evaluate(() => "startViewTransition" in document)).toBe(false);

    const seq = await page.evaluate(() => /** @type {any} */ (window).__zfb.seq);
    await page.evaluate(() =>
      import("/dist/client-router/router.js").then((m) => m.navigate("/post-form-none.html")),
    );
    await page.waitForFunction(
      (seq) =>
        /** @type {any} */ (window).__zfb.events.some(
          (e) => e.name === "zfb:page-load" && e.seq > seq,
        ),
      seq,
    );
    await expect(page).toHaveTitle("Post Form (fallback none)");

    if (who === "native oracle") {
      await page.evaluate(() =>
        document.getElementById("post-form")?.setAttribute("data-zfb-reload", ""),
      );
    }
    await Promise.all([page.waitForURL("**/search-results.html"), page.click("#post-submit")]);
    await expect(page).toHaveTitle(/Search Results/i);

    expect(await formRequests(page)).toEqual(["POST  note=once"]);
  });
}
