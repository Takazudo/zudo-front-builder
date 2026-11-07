// Investigation probe (refs #4607, review candidate 2): "cannot navigate" also
// covers a form whose node document is not fully active, or a form moved to
// another document. The router runs inside an iframe (its own document's
// submit listener); a form listener either removes that iframe (document no
// longer fully active) or moves the form into the parent document. Native
// behavior is measured with the router module stubbed.
// @ts-check
import { test, expect } from "@playwright/test";

const HOST_HTML = `<!doctype html><html><head><meta charset="UTF-8" /><title>Host</title></head>
<body><h1>Host</h1><iframe name="child" src="/search.html" style="width:600px;height:400px"></iframe></body></html>`;
const DEST_HTML = `<!doctype html><html><head><meta charset="UTF-8" />
<meta name="zfb-view-transitions-enabled" content="" />
<meta name="zfb-view-transitions-fallback" content="animate" />
<title>Dest</title></head><body><main><h1>Search Results</h1></main>
<script type="module">import { init } from "/dist/client-router/router.js"; init();</script>
</body></html>`;

/**
 * @typedef {{ name: string, method: "get" | "post",
 *   action: null | "remove-iframe" | "move-to-parent",
 *   on?: "submit" | "formdata", dest?: string }} Config
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
  /** @type {any[]} */
  const requests = [];
  /** @type {string[]} */
  const errors = [];
  try {
    await context.exposeFunction("__idLog", (entry) => events.push(entry));
    await context.addInitScript(() => {
      // @ts-expect-error - test-only global
      window.__idDoc = crypto.randomUUID();
      window.addEventListener("submit", (event) =>
        // @ts-expect-error
        window.__idLog({
          name: "submit-final",
          defaultPrevented: event.defaultPrevented,
        }),
      );
      window.addEventListener("unhandledrejection", (event) =>
        // @ts-expect-error
        window.__idLog({ name: "rejection", reason: String(event.reason) }),
      );
    });
    if (routerless) {
      await context.route("**/dist/client-router/router.js", (route) =>
        route.fulfill({
          contentType: "application/javascript; charset=utf-8",
          body: "export function init() {}",
        }),
      );
    }
    await context.route("**/probe-host.html", (route) =>
      route.fulfill({
        contentType: "text/html; charset=utf-8",
        body: HOST_HTML,
      }),
    );
    await context.route(
      (url) => url.pathname.startsWith("/probe-dest/"),
      async (route) => {
        const req = route.request();
        let frame = "unknown";
        try {
          frame = req.frame() === page.mainFrame() ? "main" : "child";
        } catch {
          frame = "none";
        }
        requests.push({
          method: req.method(),
          url: new URL(req.url()).pathname + new URL(req.url()).search,
          body: req.postData() ?? "",
          resourceType: req.resourceType(),
          frame,
        });
        await route.fulfill({
          contentType: "text/html; charset=utf-8",
          body: DEST_HTML,
        });
      },
    );
    const page = await context.newPage();
    page.on("pageerror", (error) => errors.push(String(error)));
    page.on("crash", () => events.push({ name: "page-crash" }));
    await context.request.get("/__form-requests");
    await page.goto("/probe-host.html");
    const child = page.frame({ name: "child" });
    if (!child) throw new Error("no child frame");
    await expect(child.locator("h1")).toHaveText("Search");
    await page.waitForTimeout(150);
    const childDoc = await child.evaluate(() => /** @type {any} */ (window).__idDoc);

    await child.evaluate(installForm, config);
    // A frame removed mid-evaluate may never settle the evaluate promise.
    await Promise.race([
      child
        .evaluate(() => /** @type {HTMLElement} */ (document.getElementById("id-submit")).click())
        .then(() => events.push({ name: "trigger-returned" }))
        .catch((error) =>
          events.push({
            name: "trigger-error",
            message: String(error).slice(0, 120),
          }),
        ),
      new Promise((resolve) => setTimeout(resolve, 3000)).then(() =>
        events.push({ name: "trigger-timeout" }),
      ),
    ]);
    try {
      const deadline = Date.now() + 2500;
      while (requests.length === 0 && Date.now() < deadline) await page.waitForTimeout(50);
      await page.waitForTimeout(1000);
    } catch (error) {
      // A renderer crash is itself the observation; keep the server's view.
      const serverRequests = await (await context.request.get("/__form-requests")).json();
      return {
        errors,
        requests,
        serverRequests,
        closed: true,
        crash: String(error).slice(0, 80),
        events,
      };
    }

    const serverRequests = await (await context.request.get("/__form-requests")).json();
    if (page.isClosed()) return { errors, requests, serverRequests, closed: true, events };
    const mainUrl = page.url();
    const iframePresent = await page.evaluate(() => document.querySelector("iframe") !== null);
    const formInParent = await page.evaluate(() => document.getElementById("id") !== null);
    const childNow = page.frame({ name: "child" });
    let childState = null;
    if (childNow) {
      try {
        childState = await childNow.evaluate(() => ({
          url: location.pathname + location.search,
          // @ts-expect-error
          sameDocument: window.__idDoc,
          h1: document.querySelector("h1")?.textContent ?? null,
        }));
        childState.sameDocument = childState.sameDocument === childDoc;
      } catch (error) {
        childState = { error: String(error).slice(0, 80) };
      }
    }
    return {
      errors,
      requests,
      serverRequests,
      formdataEvents: events.filter((e) => e.name === "formdata").length,
      intercepted: events.filter((e) => e.name === "submit-final").map((e) => e.defaultPrevented),
      extra: {
        mainPath: new URL(mainUrl).pathname + new URL(mainUrl).search,
        iframePresent,
        formInParent,
        child: childState,
        other: events.filter((e) => !["formdata", "submit-final"].includes(e.name)),
      },
    };
  } finally {
    await context.close();
  }
}

/** @param {Config} config */
function installForm(config) {
  const form = document.createElement("form");
  form.id = "id";
  form.setAttribute("action", config.dest ?? "/probe-dest/a");
  form.setAttribute("method", config.method);
  form.setAttribute("enctype", "application/x-www-form-urlencoded");
  const field = document.createElement("input");
  field.setAttribute("name", "q");
  field.setAttribute("value", "hello");
  form.append(field);
  const button = document.createElement("button");
  button.id = "id-submit";
  button.type = "submit";
  button.textContent = "Go";
  form.append(button);
  // Log through the parent's binding: the child's dies with its frame.
  // @ts-expect-error - test-only global
  const log = window.parent.__idLog;
  form.addEventListener("formdata", () => log({ name: "formdata" }));
  window.addEventListener("submit", (event) =>
    log({
      name: "submit-final-parentlog",
      defaultPrevented: event.defaultPrevented,
    }),
  );
  if (config.action) {
    form.addEventListener(
      config.on ?? "submit",
      () => {
        if (config.action === "remove-iframe") window.frameElement?.remove();
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
    { name: `${L} control in iframe`, method, action: null },
    {
      name: `${L} control in iframe, server-logged`,
      method,
      action: null,
      dest: "/search-results.html",
    },
    {
      name: `${L} iframe removed in submit listener`,
      method,
      action: "remove-iframe",
      on: "submit",
      dest: "/search-results.html",
    },
    {
      name: `${L} iframe removed in formdata listener`,
      method,
      action: "remove-iframe",
      on: "formdata",
      dest: "/search-results.html",
    },
    {
      name: `${L} form moved to parent in submit listener`,
      method,
      action: "move-to-parent",
      on: "submit",
    },
    {
      name: `${L} form moved to parent in formdata listener`,
      method,
      action: "move-to-parent",
      on: "formdata",
    },
  ];
});

for (const config of CASES) {
  test(config.name, async ({ browser }, testInfo) => {
    const oracle = await runCase(browser, config, true);
    const actual = await runCase(browser, config, false);
    const record = {
      config: config.name,
      browser: browser.version(),
      oracle,
      actual,
    };
    console.log(`PROBE ${JSON.stringify(record)}`);
    await testInfo.attach("native-and-router", {
      body: JSON.stringify(record, null, 2),
      contentType: "application/json",
    });
    const strip = (/** @type {any[]} */ rs) => rs.map(({ resourceType: _r, ...rest }) => rest);
    expect.soft(strip(actual.requests), "request(s) match native").toEqual(strip(oracle.requests));
    const server = (/** @type {any[]} */ rs) => rs.map(({ destination: _d, ...rest }) => rest);
    expect
      .soft(server(actual.serverRequests), "server log matches native")
      .toEqual(server(oracle.serverRequests));
    expect
      .soft(actual.extra?.mainPath, "top-level URL matches native")
      .toBe(oracle.extra?.mainPath);
  });
}
