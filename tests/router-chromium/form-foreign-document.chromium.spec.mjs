// Level 4: a form moved into another document during its own submission
// follows the native oracle (#4617). Native submission targets the navigable of
// the form's current node document, so the router (running in a child frame)
// must leave such a form to the browser instead of SPA-navigating its own frame:
// https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#form-submission-algorithm
//
// Oracle: identical markup with the router module stubbed. A host page without
// the router embeds an iframe that runs it; a form listener moves the form into
// the host document. Every case compares against the running engine's own
// oracle, except one: when the move happens in a `formdata` listener, native
// WebKit navigates the child frame while Chromium and the spec navigate the top
// level. The router cannot reproduce WebKit there (see the WebKit branch below).
//
// Not covered: removing the iframe from a `formdata` listener. Native Chromium
// crashes the renderer there, so there is no oracle to assert against.
// @ts-check
import { test, expect } from "@playwright/test";
import { ENGINE } from "../webkit-token/engine.mjs";

const HOST_HTML = `<!doctype html><html><head><meta charset="UTF-8" /><title>Host</title></head>
<body><h1>Host</h1><iframe name="child" src="/search.html" style="width:600px;height:400px"></iframe></body></html>`;
const LIFECYCLE = [
  "zfb:before-preparation",
  "zfb:after-preparation",
  "zfb:before-swap",
  "zfb:after-swap",
  "zfb:page-load",
];
// Bounded wait for a request before treating the attempt as abandoned.
const SETTLE_MS = 1500;

/**
 * @typedef {{ name: string, method: "get" | "post",
 *   action: null | "move-to-parent" | "remove-iframe" | "adopted",
 *   on?: "submit" | "formdata", routed: boolean }} Config
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
  try {
    await context.exposeFunction("__fdLog", (entry) => events.push(entry));
    await context.addInitScript((names) => {
      // @ts-expect-error - test-only global
      window.__fdDoc = crypto.randomUUID();
      if (window.top === window) return;
      for (const name of names) {
        document.addEventListener(name, () =>
          // @ts-expect-error
          window.__fdLog({ name, docId: window.__fdDoc }),
        );
      }
    }, LIFECYCLE);
    if (routerless) {
      await context.route("**/dist/client-router/router.js", (route) =>
        route.fulfill({
          contentType: "application/javascript; charset=utf-8",
          body: "export function init() {}",
        }),
      );
    }
    await context.route("**/foreign-host.html", (route) =>
      route.fulfill({ contentType: "text/html; charset=utf-8", body: HOST_HTML }),
    );
    const page = await context.newPage();
    page.on("pageerror", (error) => errors.push(String(error)));
    await page.goto("/foreign-host.html");
    const child = page.frame({ name: "child" });
    if (!child) throw new Error("no child frame");
    await expect(child.locator("h1")).toHaveText("Search");
    if (!routerless)
      await expect.poll(() => events.filter((e) => e.name === "zfb:page-load").length).toBe(1);
    const hostDoc = await page.evaluate(() => /** @type {any} */ (window).__fdDoc);
    const childDoc = await child.evaluate(() => /** @type {any} */ (window).__fdDoc);
    await context.request.get("/__form-requests");
    events.length = 0;

    await child.evaluate(installForm, config);
    // A frame removed mid-evaluate may never settle the evaluate promise.
    await Promise.race([
      child
        .evaluate(() => /** @type {HTMLElement} */ (document.getElementById("fd-submit")).click())
        .catch(() => undefined),
      new Promise((resolve) => setTimeout(resolve, SETTLE_MS)),
    ]);

    /** @type {any[]} */
    let requests = [];
    const deadline = Date.now() + SETTLE_MS;
    while (Date.now() < deadline) {
      requests = requests.concat(await (await context.request.get("/__form-requests")).json());
      if (requests.length > 0) break;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    // Only the router's own document: a natively loaded destination boots its
    // own router and emits its own lifecycle events.
    const lifecycle = () =>
      events.filter((e) => e.docId === childDoc && LIFECYCLE.includes(e.name));
    if (lifecycle().length > 0) {
      await expect.poll(() => lifecycle().filter((e) => e.name === "zfb:page-load").length).toBe(1);
    } else if (requests.length > 0) {
      // A document load: whichever frame navigated must finish loading it.
      const loaded = () =>
        page.frames().find((frame) => new URL(frame.url()).pathname === "/search-results.html");
      await expect.poll(() => loaded() !== undefined).toBe(true);
      await loaded()?.waitForLoadState("load");
    }
    requests = requests.concat(await (await context.request.get("/__form-requests")).json());

    const top = await page.evaluate(() => ({
      path: location.pathname + location.search,
      // @ts-expect-error - test-only global
      docId: window.__fdDoc,
      iframe: document.querySelector("iframe") !== null,
    }));
    const childNow = page.frame({ name: "child" });
    /** @type {null | { path: string, sameDocument: boolean }} */
    let childState = null;
    if (childNow) {
      const state = await childNow.evaluate(() => ({
        path: location.pathname + location.search,
        // @ts-expect-error - test-only global
        docId: window.__fdDoc,
      }));
      childState = { path: state.path, sameDocument: state.docId === childDoc };
    }
    return {
      errors,
      // Sec-Fetch-Dest: "document" for the top level, "iframe" for the child
      // frame's own navigation, "empty" for the router's fetch.
      requests,
      formdata: events.filter((e) => e.name === "formdata").length,
      submitFinal: events.filter((e) => e.name === "submit-final").map((e) => e.defaultPrevented),
      lifecycle: lifecycle().map((e) => e.name),
      top: { path: top.path, sameDocument: top.docId === hostDoc, iframe: top.iframe },
      child: childState,
    };
  } finally {
    await context.close();
  }
}

/** @param {Config} config */
function installForm(config) {
  // An adopted form is created by the host document and moved into this one.
  const form = /** @type {HTMLFormElement} */ (
    config.action === "adopted"
      ? document.adoptNode(window.parent.document.createElement("form"))
      : document.createElement("form")
  );
  form.id = "fd";
  form.setAttribute("action", "/search-results.html");
  form.setAttribute("method", config.method);
  form.setAttribute("enctype", "application/x-www-form-urlencoded");
  const field = document.createElement("input");
  field.setAttribute("name", "q");
  field.setAttribute("value", "hello");
  form.append(field);
  // Named controls shadow form instance properties ([LegacyOverrideBuiltIns]).
  const shadow = document.createElement("input");
  shadow.setAttribute("name", "ownerDocument");
  shadow.setAttribute("value", "shadowed");
  form.append(shadow);
  const button = document.createElement("button");
  button.id = "fd-submit";
  button.type = "submit";
  button.textContent = "Go";
  form.append(button);
  // Log through the host's binding: the child's dies with its frame.
  // @ts-expect-error - test-only global
  const log = window.parent.__fdLog;
  form.addEventListener("formdata", () => log({ name: "formdata" }));
  // Bubble phase on window: runs after the router's document listener.
  window.addEventListener(
    "submit",
    (event) => log({ name: "submit-final", defaultPrevented: event.defaultPrevented }),
    { once: true },
  );
  if (config.action === "move-to-parent" || config.action === "remove-iframe") {
    const action = config.action;
    form.addEventListener(
      config.on ?? "submit",
      () => {
        if (action === "remove-iframe") window.frameElement?.remove();
        else window.parent.document.body.append(form);
      },
      { once: true },
    );
  }
  (document.querySelector("main") ?? document.body).append(form);
}

/** @type {Config[]} */
const CASES = ["get", "post"].flatMap((m) => {
  const method = /** @type {"get" | "post"} */ (m);
  const L = method.toUpperCase();
  return [
    { name: `${L} control in iframe`, method, action: null, routed: true },
    {
      name: `${L} form adopted from the host document`,
      method,
      action: "adopted",
      routed: true,
    },
    {
      name: `${L} iframe removed in submit listener`,
      method,
      action: "remove-iframe",
      on: "submit",
      routed: false,
    },
    {
      name: `${L} form moved to parent in submit listener`,
      method,
      action: "move-to-parent",
      on: "submit",
      routed: false,
    },
    {
      name: `${L} form moved to parent in formdata listener`,
      method,
      action: "move-to-parent",
      on: "formdata",
      routed: false,
    },
  ];
});

for (const config of CASES) {
  test(config.name, async ({ browser }, testInfo) => {
    const oracle = await runCase(browser, config, true);
    const actual = await runCase(browser, config, false);
    await testInfo.attach("native-and-router", {
      body: JSON.stringify({ config, browser: browser.version(), oracle, actual }, null, 2),
      contentType: "application/json",
    });

    for (const result of [oracle, actual]) expect(result.errors).toEqual([]);
    const strip = (/** @type {any[]} */ rs) => rs.map(({ destination: _d, ...rest }) => rest);
    expect(strip(actual.requests), "request(s) match native").toEqual(strip(oracle.requests));
    for (const request of oracle.requests)
      expect(["document", "iframe"]).toContain(request.destination);
    const payload = "q=hello&ownerDocument=shadowed";
    const method = config.method.toUpperCase();

    if (config.routed) {
      expect(oracle.requests).toHaveLength(1);
      expect(oracle.requests[0].method).toBe(method);
      expect(method === "POST" ? oracle.requests[0].body : oracle.requests[0].search).toBe(
        method === "POST" ? payload : `?${payload}`,
      );
      expect(oracle.requests[0].destination).toBe("iframe");
      expect(actual.requests.map((r) => r.destination)).toEqual(["empty"]);
      expect(actual.submitFinal).toEqual([true]);
      expect(actual.lifecycle).toEqual(LIFECYCLE);
      expect(actual.formdata).toBe(1);
      expect(actual.child).toEqual({ path: oracle.child?.path, sameDocument: true });
      expect(actual.top).toEqual(oracle.top);
      return;
    }

    if (ENGINE === "webkit" && config.on === "formdata") {
      // #4617: the spec chooses the target navigable after constructing the
      // entry list, from the form's node document at that point (step order in
      // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#form-submission-algorithm).
      // Native WebKit fixes it earlier, so its oracle navigates the child frame
      // after a move in a formdata listener. The router's own FormData runs in
      // the submit event, before WebKit's sampling point, so after the decline
      // WebKit navigates the top level: the spec target, not WebKit's oracle.
      // If the oracle stops navigating the iframe, remove this branch.
      expect(
        oracle.requests.map((r) => r.destination),
        "WebKit oracle still targets the iframe",
      ).toEqual(["iframe"]);
      expect(oracle.top).toEqual({ path: "/foreign-host.html", sameDocument: true, iframe: true });
      expect(actual.submitFinal).toEqual(oracle.submitFinal);
      expect(actual.submitFinal.every((prevented) => prevented === false)).toBe(true);
      expect(actual.lifecycle).toEqual([]);
      expect(actual.requests.map((r) => r.destination)).toEqual(["document"]);
      const destination = `/search-results.html${method === "GET" ? `?${payload}` : ""}`;
      expect(actual.top.path).toBe(destination);
      expect(actual.top.sameDocument).toBe(false);
      expect(actual.formdata).toBe(oracle.formdata + 1);
      return;
    }

    // Declined: the browser owns the submission, so every observation is native.
    expect(actual.requests, "same navigable as native").toEqual(oracle.requests);
    expect(actual.submitFinal).toEqual(oracle.submitFinal);
    expect(actual.submitFinal.every((prevented) => prevented === false)).toBe(true);
    expect(actual.lifecycle).toEqual([]);
    expect(actual.top).toEqual(oracle.top);
    expect(actual.child?.path).toBe(oracle.child?.path);
    expect(actual.child?.sameDocument).toBe(oracle.child?.sameDocument);
    // Declining after the router's own FormData leaves the browser to rebuild
    // the entry list, so the formdata listener runs exactly once more.
    expect(actual.formdata).toBe(oracle.formdata + (config.on === "formdata" ? 1 : 0));
  });
}
