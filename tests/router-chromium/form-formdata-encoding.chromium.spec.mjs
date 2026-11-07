// Level 4: a `formdata` listener that rewrites the form's or submitter's
// encoding attributes does not change the POST body encoding (#4616). Native
// Chromium and WebKit select the encoding before firing `formdata`, the same
// moment as method and action, so the router encodes the request with the
// value it selected at decision time rather than re-reading live attributes.
//
// Oracle: identical markup with the router module stubbed, so the browser alone
// owns the request. The server's request log (the bytes that arrived) is the
// payload oracle; multipart boundaries are normalized.
// @ts-check
import { test, expect } from "@playwright/test";

const LIFECYCLE = [
  "zfb:before-preparation",
  "zfb:after-preparation",
  "zfb:before-swap",
  "zfb:after-swap",
  "zfb:page-load",
];
const URLENCODED = "application/x-www-form-urlencoded";
const MULTIPART = "multipart/form-data";

/**
 * @typedef {{ name: string, enctype?: string, formenctype?: string,
 *   mutate: [("form" | "submitter"), string, string][] }} Config
 */

/**
 * @param {import("@playwright/test").Browser} browser
 * @param {Config} config
 * @param {boolean} routerless
 */
async function runCase(browser, config, routerless) {
  const context = await browser.newContext();
  /** @type {any[]} */
  const events = [];
  /** @type {string[]} */
  const errors = [];
  let phase = 0;
  try {
    await context.exposeFunction("__feLog", (entry) => events.push({ ...entry, phase }));
    await context.addInitScript((names) => {
      // @ts-expect-error - test-only global
      window.__feDoc = crypto.randomUUID();
      for (const name of names) {
        document.addEventListener(name, () =>
          // @ts-expect-error
          window.__feLog({ name, docId: window.__feDoc }),
        );
      }
      // Bubble phase on window: runs after the router's document listener.
      window.addEventListener("submit", (event) =>
        // @ts-expect-error
        window.__feLog({ name: "submit-final", defaultPrevented: event.defaultPrevented }),
      );
    }, LIFECYCLE);
    if (routerless) {
      await context.route("**/dist/client-router/router.js", (route) =>
        route.fulfill({
          contentType: "application/javascript; charset=utf-8",
          body: "export function init() {}",
        }),
      );
    }
    const page = await context.newPage();
    page.on("pageerror", (error) => errors.push(String(error)));
    await page.goto("/search.html");
    await expect(page.locator("h1")).toHaveText("Search");
    if (!routerless)
      await expect.poll(() => events.filter((e) => e.name === "zfb:page-load").length).toBe(1);
    const docId = await page.evaluate(() => /** @type {any} */ (window).__feDoc);
    await context.request.get("/__form-requests");

    await page.evaluate(installForm, config);
    phase = 1;
    await page.locator("#fe-submit").click();

    const lifecycle = () =>
      events.filter((e) => e.phase === 1 && e.docId === docId && LIFECYCLE.includes(e.name));
    /** @type {any[]} */
    let requests = [];
    await expect
      .poll(async () => {
        requests = requests.concat(await (await context.request.get("/__form-requests")).json());
        return requests.length;
      })
      .toBeGreaterThan(0);
    if (routerless) {
      await page.waitForURL((url) => url.pathname === "/search-results.html");
      await page.waitForLoadState("load");
    } else {
      await expect.poll(() => lifecycle().filter((e) => e.name === "zfb:page-load").length).toBe(1);
    }
    await expect(page.locator("h1")).toHaveText("Search Results");
    requests = requests.concat(await (await context.request.get("/__form-requests")).json());
    const after = await page.evaluate(() => /** @type {any} */ (window).__feDoc);
    return {
      errors,
      requests: requests.map((request) => ({
        ...request,
        body: request.body.replace(/-*WebKitFormBoundary[A-Za-z0-9]+/g, "BOUNDARY"),
      })),
      formdata: events.filter((e) => e.phase === 1 && e.name === "formdata").length,
      submitFinal: events
        .filter((e) => e.phase === 1 && e.name === "submit-final")
        .map((e) => e.defaultPrevented),
      lifecycle: lifecycle().map((e) => e.name),
      sameDocument: after === docId,
    };
  } finally {
    await context.close();
  }
}

/** @param {Config} config */
function installForm(config) {
  const form = document.createElement("form");
  form.id = "fe";
  form.setAttribute("action", "/search-results.html");
  form.setAttribute("method", "post");
  if (config.enctype !== undefined) form.setAttribute("enctype", config.enctype);
  const field = document.createElement("input");
  field.setAttribute("name", "q");
  field.setAttribute("value", "hello world");
  form.append(field);
  const button = document.createElement("button");
  button.id = "fe-submit";
  button.type = "submit";
  button.name = "go";
  button.value = "search";
  if (config.formenctype !== undefined) button.setAttribute("formenctype", config.formenctype);
  button.textContent = "Search";
  form.append(button);
  form.addEventListener("formdata", () => {
    // @ts-expect-error - test-only global
    window.__feLog({ name: "formdata" });
    for (const [who, attr, value] of config.mutate) {
      (who === "form" ? form : button).setAttribute(attr, value);
    }
  });
  (document.querySelector("main") ?? document.body).append(form);
}

/** @type {Config[]} */
const CASES = [
  { name: "control URL-encoded, no mutation", enctype: URLENCODED, mutate: [] },
  { name: "control multipart, no mutation", enctype: MULTIPART, mutate: [] },
  {
    name: "form enctype URL-encoded -> multipart",
    enctype: URLENCODED,
    mutate: [["form", "enctype", MULTIPART]],
  },
  {
    name: "form enctype multipart -> URL-encoded",
    enctype: MULTIPART,
    mutate: [["form", "enctype", URLENCODED]],
  },
  {
    name: "form enctype URL-encoded -> text/plain",
    enctype: URLENCODED,
    mutate: [["form", "enctype", "text/plain"]],
  },
  {
    name: "submitter formenctype text/plain added",
    enctype: URLENCODED,
    mutate: [["submitter", "formenctype", "text/plain"]],
  },
  {
    name: "submitter formenctype multipart -> URL-encoded",
    enctype: URLENCODED,
    formenctype: MULTIPART,
    mutate: [["submitter", "formenctype", URLENCODED]],
  },
  {
    name: "submitter formenctype URL-encoded -> multipart",
    enctype: MULTIPART,
    formenctype: URLENCODED,
    mutate: [["submitter", "formenctype", MULTIPART]],
  },
];

for (const config of CASES) {
  test(config.name, async ({ browser }, testInfo) => {
    const oracle = await runCase(browser, config, true);
    const actual = await runCase(browser, config, false);
    await testInfo.attach("native-and-router", {
      body: JSON.stringify({ config, browser: browser.version(), oracle, actual }, null, 2),
      contentType: "application/json",
    });

    for (const result of [oracle, actual]) {
      expect(result.errors).toEqual([]);
      expect(result.requests).toHaveLength(1);
      expect(result.requests[0].method).toBe("POST");
    }
    const { destination: nativeDestination, ...native } = oracle.requests[0];
    const { destination: routerDestination, ...routed } = actual.requests[0];
    expect(nativeDestination).toBe("document");
    expect(routerDestination).toBe("empty");
    expect(routed, "encoded request matches native").toEqual(native);
    expect([URLENCODED, MULTIPART]).toContain(native.contentType);

    expect(oracle.submitFinal).toEqual([false]);
    expect(actual.submitFinal).toEqual([true]);
    expect(actual.sameDocument).toBe(true);
    expect(actual.lifecycle).toEqual(LIFECYCLE);
    // Native Chromium 149 and WebKit 26.5 both fire formdata twice for a POST
    // submission (#4616 evidence: Linux Chromium and the native Mac run); the
    // routed submission builds the entry list exactly once.
    expect(oracle.formdata).toBe(2);
    expect(actual.formdata).toBe(1);
  });
}
