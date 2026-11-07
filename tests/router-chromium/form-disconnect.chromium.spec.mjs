// Level 4: a form disconnected during its own submission follows the native
// oracle (#4607). The browser abandons a submission whose form "cannot
// navigate" after the submit event and again after building the entry list:
// https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#form-submission-algorithm
//
// Oracle: identical markup with the router module stubbed, so the browser alone
// owns the request. Activation is a trusted click. An aborted first attempt is
// followed by a second, valid submission; the server's ordered request log must
// then hold only that one, so "no request" never rests on a timeout alone.
// @ts-check
import { test, expect } from "@playwright/test";

const DESTINATION = "/search-results.html";
const LIFECYCLE = [
  "zfb:before-preparation",
  "zfb:after-preparation",
  "zfb:before-swap",
  "zfb:after-swap",
  "zfb:page-load",
];
// Bounded wait for a first-attempt request before treating it as abandoned.
const SETTLE_MS = 1500;

/**
 * @typedef {{ method: "get" | "post", remove: "submit" | "formdata" | null,
 *   reinsert?: boolean, shadow?: boolean, aborts: boolean, name: string }} Config
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
  /** @type {{ type: string, text: string, phase: number }[]} */
  const messages = [];
  let phase = 0;
  try {
    await context.exposeFunction("__dcLog", (entry) => events.push({ ...entry, phase }));
    await context.addInitScript((names) => {
      // @ts-expect-error - test-only globals
      window.__dcDoc = crypto.randomUUID();
      for (const name of names) {
        document.addEventListener(name, () =>
          // @ts-expect-error
          window.__dcLog({ name, docId: window.__dcDoc }),
        );
      }
      // Bubble phase on window: runs after the router's document listener.
      window.addEventListener("submit", (event) =>
        // @ts-expect-error
        window.__dcLog({
          name: "submit-final",
          // @ts-expect-error
          docId: window.__dcDoc,
          defaultPrevented: event.defaultPrevented,
          // @ts-expect-error
          target: event.target.id,
        }),
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
    page.on("console", (message) => {
      if (message.type() === "warning" || message.type() === "error")
        messages.push({ type: message.type(), text: message.text(), phase });
    });
    await page.goto("/search.html");
    await expect(page.locator("h1")).toHaveText("Search");
    if (!routerless)
      await expect.poll(() => events.filter((e) => e.name === "zfb:page-load").length).toBe(1);
    const docId = await page.evaluate(() => /** @type {any} */ (window).__dcDoc);
    await context.request.get("/__form-requests");

    await page.evaluate(installForm, {
      config,
      id: "dc",
      value: "hello world",
      remove: config.remove,
    });
    const before = await page.evaluate(() => ({
      url: location.href,
      length: history.length,
    }));
    phase = 1;
    await page.locator("#dc-submit").click();

    // Attempt 1: either a destination request reaches the server, or the
    // bounded settle passes and the attempt is treated as abandoned (later
    // proven by the second submission's ordered request log).
    /** @type {any[]} */
    let first = [];
    const deadline = Date.now() + SETTLE_MS;
    while (Date.now() < deadline) {
      first = first.concat(await (await context.request.get("/__form-requests")).json());
      if (first.length > 0) break;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    const proceeded = first.length > 0;
    const attempt1Lifecycle = () =>
      events.filter((e) => e.phase === 1 && e.docId === docId && LIFECYCLE.includes(e.name));
    if (attempt1Lifecycle().length > 0) {
      await expect
        .poll(() => attempt1Lifecycle().filter((e) => e.name === "zfb:page-load").length)
        .toBe(1);
      await page.waitForFunction(
        () => !document.documentElement.hasAttribute("data-zfb-transition"),
      );
    } else if (proceeded) {
      await page.waitForURL((url) => url.pathname === DESTINATION);
      await expect(page.locator("h1")).toHaveText("Search Results");
      await page.waitForLoadState("load");
    }
    const after1 = await page.evaluate(() => ({
      url: location.href,
      length: history.length,
      // @ts-expect-error - test-only global
      docId: window.__dcDoc,
    }));
    first = first.concat(await (await context.request.get("/__form-requests")).json());

    /** @type {any} */
    let second = null;
    if (!proceeded) {
      // Attempt 2: a fresh, connected form must still submit exactly once.
      phase = 2;
      await page.evaluate(installForm, {
        config,
        id: "dc2",
        value: "second",
        remove: null,
      });
      await page.locator("#dc2-submit").click();
      await page.waitForURL((url) => url.pathname === DESTINATION);
      await expect(page.locator("h1")).toHaveText("Search Results");
      await page.waitForLoadState("load");
      const lifecycle2 = () =>
        events.filter((e) => e.phase === 2 && e.docId === docId && LIFECYCLE.includes(e.name));
      if (lifecycle2().length > 0) {
        await expect
          .poll(() => lifecycle2().filter((e) => e.name === "zfb:page-load").length)
          .toBe(1);
        await page.waitForFunction(
          () => !document.documentElement.hasAttribute("data-zfb-transition"),
        );
      }
      await page.evaluate(() => new Promise((resolve) => setTimeout(resolve, 0)));
      const docId2 = await page.evaluate(() => /** @type {any} */ (window).__dcDoc);
      second = {
        requests: normalize(await (await context.request.get("/__form-requests")).json()),
        sameDocument: docId2 === docId,
        lifecycle: lifecycle2().map((e) => e.name),
      };
    }

    const submitFinal = events.filter((e) => e.phase === 1 && e.name === "submit-final");
    return {
      errors,
      warnings: messages.filter((c) => c.phase === 1).map(({ type, text }) => ({ type, text })),
      proceeded,
      first: normalize(first),
      sameDocument1: after1.docId === docId,
      urlChanged1: after1.url !== before.url,
      historyDelta1: after1.length - before.length,
      lifecycle1: attempt1Lifecycle().map((e) => e.name),
      formdata1: events.filter((e) => e.phase === 1 && e.name === "formdata").length,
      submitFinal: submitFinal.map(({ defaultPrevented, target }) => ({
        defaultPrevented,
        target,
      })),
      second,
    };
  } finally {
    await context.close();
  }
}

/** @param {any[]} requests */
function normalize(requests) {
  return requests.map(({ destination, ...request }) => ({
    ...request,
    resourceType:
      destination === "document" ? "document" : destination === "empty" ? "fetch" : destination,
  }));
}

/**
 * @param {{ config: Config, id: string, value: string, remove: Config["remove"] }} args
 */
function installForm({ config, id, value, remove }) {
  const form = document.createElement("form");
  form.id = id;
  form.setAttribute("action", "/search-results.html");
  form.setAttribute("method", config.method);
  form.setAttribute("enctype", "application/x-www-form-urlencoded");
  const field = document.createElement("input");
  field.setAttribute("name", "q");
  field.setAttribute("value", value);
  form.append(field);
  if (config.shadow) {
    // Named controls shadow form instance properties ([LegacyOverrideBuiltIns]).
    for (const name of ["isConnected", "ownerDocument"]) {
      const shadow = document.createElement("input");
      shadow.setAttribute("name", name);
      shadow.setAttribute("value", "shadowed");
      form.append(shadow);
    }
  }
  const button = document.createElement("button");
  button.id = `${id}-submit`;
  button.type = "submit";
  button.name = "go";
  button.value = "search";
  button.textContent = "Search";
  form.append(button);
  form.addEventListener("formdata", () =>
    // @ts-expect-error - test-only global
    window.__dcLog({ name: "formdata", docId: window.__dcDoc }),
  );
  if (remove) {
    form.addEventListener(
      remove,
      () => {
        const parent = /** @type {ParentNode} */ (form.parentNode);
        const next = form.nextSibling;
        form.remove();
        if (config.reinsert) parent.insertBefore(form, next);
      },
      { once: true },
    );
  }
  (document.querySelector("main") ?? document.body).append(form);
}

/** @type {Config[]} */
const CASES = ["get", "post"].flatMap((m) => {
  const method = /** @type {"get" | "post"} */ (m);
  const label = method.toUpperCase();
  return [
    { name: `${label} connected control`, method, remove: null, aborts: false },
    {
      name: `${label} removed in form submit listener`,
      method,
      remove: "submit",
      aborts: true,
    },
    {
      name: `${label} removed in formdata listener`,
      method,
      remove: "formdata",
      aborts: true,
    },
    {
      name: `${label} removed and reinserted in submit listener`,
      method,
      remove: "submit",
      reinsert: true,
      aborts: false,
    },
    {
      name: `${label} removed and reinserted in formdata listener`,
      method,
      remove: "formdata",
      reinsert: true,
      aborts: false,
    },
    {
      name: `${label} removed in submit listener with isConnected/ownerDocument controls`,
      method,
      remove: "submit",
      shadow: true,
      aborts: true,
    },
    {
      name: `${label} connected with isConnected/ownerDocument controls`,
      method,
      remove: null,
      shadow: true,
      aborts: false,
    },
  ];
});

const payload = (/** @type {Config} */ config, /** @type {string} */ q) => {
  const shadow = config.shadow ? "&isConnected=shadowed&ownerDocument=shadowed" : "";
  return `q=${q}${shadow}&go=search`;
};

for (const config of CASES) {
  test(config.name, async ({ browser }, testInfo) => {
    const oracle = await runCase(browser, config, true);
    const actual = await runCase(browser, config, false);
    await testInfo.attach("native-and-router", {
      body: JSON.stringify({ config, oracle, actual }, null, 2),
      contentType: "application/json",
    });

    // The oracle itself must show the specified native behavior.
    expect(oracle.proceeded, "native proceeded").toBe(!config.aborts);
    for (const result of [oracle, actual]) {
      expect(result.errors).toEqual([]);
      expect(result.submitFinal).toHaveLength(1);
    }
    // Native formdata counts are engine-specific (Chromium builds a POST entry
    // list twice, and none when it aborts before building), so a declined
    // submission must match the oracle while a routed one builds exactly once.
    if (config.aborts) expect(actual.formdata1).toBe(oracle.formdata1);
    else {
      expect(oracle.formdata1).toBeGreaterThanOrEqual(1);
      expect(actual.formdata1).toBe(1);
    }
    expect(oracle.submitFinal[0].defaultPrevented).toBe(false);
    // Disconnected-form diagnostics are the browser's own; the router must
    // neither add nor hide any during the first attempt.
    expect(actual.warnings).toEqual(oracle.warnings);

    const method = config.method.toUpperCase();
    if (config.aborts) {
      for (const result of [oracle, actual]) {
        expect(result.proceeded).toBe(false);
        expect(result.first).toEqual([]);
        expect(result.sameDocument1).toBe(true);
        expect(result.urlChanged1).toBe(false);
        expect(result.historyDelta1).toBe(0);
        expect(result.lifecycle1).toEqual([]);
        // Attempt 2 is the only request the server ever saw.
        expect(result.second.requests).toHaveLength(1);
        const request = result.second.requests[0];
        expect(request.method).toBe(method);
        expect(method === "POST" ? request.body : request.search).toBe(
          `${method === "POST" ? "" : "?"}${payload(config, "second")}`,
        );
      }
      expect(actual.submitFinal[0].defaultPrevented).toBe(false);
      expect(oracle.second.requests[0].resourceType).toBe("document");
      const { resourceType: _o, ...nativeSecond } = oracle.second.requests[0];
      const { resourceType: _a, ...actualSecond } = actual.second.requests[0];
      expect(actualSecond).toEqual(nativeSecond);
      expect(actual.second.requests[0].resourceType).toBe("fetch");
      expect(actual.second.sameDocument).toBe(true);
      expect(actual.second.lifecycle).toEqual(LIFECYCLE);
    } else {
      for (const result of [oracle, actual]) {
        expect(result.proceeded).toBe(true);
        expect(result.first).toHaveLength(1);
        expect(result.second).toBeNull();
        const request = result.first[0];
        expect(request.method).toBe(method);
        expect(method === "POST" ? request.body : request.search).toBe(
          `${method === "POST" ? "" : "?"}${payload(config, "hello+world")}`,
        );
      }
      expect(oracle.first[0].resourceType).toBe("document");
      const { resourceType: _o, ...nativeFirst } = oracle.first[0];
      const { resourceType: _a, ...actualFirst } = actual.first[0];
      expect(actualFirst).toEqual(nativeFirst);
      expect(actual.first[0].resourceType).toBe("fetch");
      expect(actual.submitFinal[0].defaultPrevented).toBe(true);
      expect(actual.sameDocument1).toBe(true);
      expect(actual.lifecycle1).toEqual(LIFECYCLE);
    }
  });
}
