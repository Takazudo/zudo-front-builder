// Investigation probe (refs #4607, review candidate 3): a `formdata` listener
// mutates action/method/enctype (or the submitter's formaction/formmethod/
// formenctype) while the entry list is built. The router samples these before
// `new FormData(form, submitter)`; native submission is measured here instead
// of assumed. Oracle = same markup with the router module stubbed.
//
// Measurement only: each case records native and router observations and
// soft-asserts router == native so every divergence is listed in one run.
// @ts-check
import { test, expect } from "@playwright/test";

const CROSS = "http://127.0.0.1:4322";
const DEST_HTML = `<!doctype html><html><head><meta charset="UTF-8" />
<meta name="zfb-view-transitions-enabled" content="" />
<meta name="zfb-view-transitions-fallback" content="animate" />
<title>Dest</title></head><body><main><h1>Search Results</h1></main>
<script type="module">import { init } from "/dist/client-router/router.js"; init();</script>
</body></html>`;

/**
 * @typedef {{ name: string, method: string, enctype?: string, formaction?: string,
 *   formmethod?: string, formenctype?: string,
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
  /** @type {any[]} */
  const requests = [];
  /** @type {string[]} */
  const errors = [];
  try {
    await context.exposeFunction("__fmLog", (entry) => events.push(entry));
    await context.addInitScript(() => {
      // @ts-expect-error - test-only global
      window.__fmDoc = crypto.randomUUID();
      window.addEventListener("submit", (event) =>
        // @ts-expect-error
        window.__fmLog({
          name: "submit-final",
          defaultPrevented: event.defaultPrevented,
        }),
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
    await context.route(
      (url) => url.pathname.startsWith("/probe-dest/"),
      async (route) => {
        const req = route.request();
        const headers = await req.allHeaders();
        requests.push({
          method: req.method(),
          url: req.url(),
          contentType: (headers["content-type"] ?? "").split(";")[0],
          body: (req.postData() ?? "").replace(/-*WebKitFormBoundary[A-Za-z0-9]+/g, "BOUNDARY"),
          resourceType: req.resourceType(),
        });
        await route.fulfill({
          contentType: "text/html; charset=utf-8",
          body: DEST_HTML,
        });
      },
    );
    const page = await context.newPage();
    page.on("pageerror", (error) => errors.push(String(error)));
    await page.goto("/search.html");
    await expect(page.locator("h1")).toHaveText("Search");
    if (!routerless) await page.waitForTimeout(100);
    const docId = await page.evaluate(() => /** @type {any} */ (window).__fmDoc);

    await page.evaluate(installForm, { config, cross: CROSS });
    await page.locator("#fm-submit").click();

    const deadline = Date.now() + 5000;
    while (requests.length === 0 && Date.now() < deadline) await page.waitForTimeout(50);
    await expect
      .poll(async () => {
        try {
          return await page.locator("h1").textContent();
        } catch {
          return null;
        }
      })
      .toBe("Search Results");
    await page.waitForLoadState("load");
    await page.waitForTimeout(400);
    const after = await page.evaluate(() => ({
      url: location.href,
      // @ts-expect-error
      docId: window.__fmDoc,
      // @ts-expect-error
      formdata: window.__fmFormdata ?? null,
    }));
    const formdataEvents = events.filter((e) => e.name === "formdata").length;
    const submitFinal = events.filter((e) => e.name === "submit-final");
    const toKey = (/** @type {string} */ u) => {
      const url = new URL(u);
      return `${url.origin === CROSS ? "CROSS" : ""}${url.pathname}${url.search}`;
    };
    return {
      errors,
      requests: requests.map((r) => ({ ...r, url: toKey(r.url) })),
      formdataEvents,
      intercepted: submitFinal.map((e) => e.defaultPrevented),
      finalUrl: toKey(after.url),
      sameDocument: after.docId === docId,
    };
  } finally {
    await context.close();
  }
}

/** @param {{ config: Config, cross: string }} args */
function installForm({ config, cross }) {
  const form = document.createElement("form");
  form.id = "fm";
  form.setAttribute("action", "/probe-dest/a");
  form.setAttribute("method", config.method);
  form.setAttribute("enctype", config.enctype ?? "application/x-www-form-urlencoded");
  const field = document.createElement("input");
  field.setAttribute("name", "q");
  field.setAttribute("value", "hello world");
  form.append(field);
  const button = document.createElement("button");
  button.id = "fm-submit";
  button.type = "submit";
  button.name = "go";
  button.value = "search";
  for (const attr of /** @type {const} */ (["formaction", "formmethod", "formenctype"])) {
    const value = config[attr];
    if (value !== undefined) button.setAttribute(attr, value);
  }
  button.textContent = "Search";
  form.append(button);
  form.addEventListener("formdata", () => {
    // @ts-expect-error - test-only global
    window.__fmLog({ name: "formdata" });
    for (const [who, attr, value] of config.mutate) {
      (who === "form" ? form : button).setAttribute(attr, value.replace("CROSS", cross));
    }
  });
  (document.querySelector("main") ?? document.body).append(form);
}

/** @type {Config[]} */
const CASES = [
  { name: "control GET, no mutation", method: "get", mutate: [] },
  { name: "control POST, no mutation", method: "post", mutate: [] },
  {
    name: "GET form action -> /b",
    method: "get",
    mutate: [["form", "action", "/probe-dest/b"]],
  },
  {
    name: "POST form action -> /b",
    method: "post",
    mutate: [["form", "action", "/probe-dest/b"]],
  },
  {
    name: "GET submitter formaction added -> /b",
    method: "get",
    mutate: [["submitter", "formaction", "/probe-dest/b"]],
  },
  {
    name: "POST submitter formaction /a -> /b",
    method: "post",
    formaction: "/probe-dest/a",
    mutate: [["submitter", "formaction", "/probe-dest/b"]],
  },
  {
    name: "form method get -> post",
    method: "get",
    mutate: [["form", "method", "post"]],
  },
  {
    name: "form method post -> get",
    method: "post",
    mutate: [["form", "method", "get"]],
  },
  {
    name: "submitter formmethod added post (form get)",
    method: "get",
    mutate: [["submitter", "formmethod", "post"]],
  },
  {
    name: "POST enctype urlencoded -> multipart",
    method: "post",
    mutate: [["form", "enctype", "multipart/form-data"]],
  },
  {
    name: "POST enctype multipart -> urlencoded",
    method: "post",
    enctype: "multipart/form-data",
    mutate: [["form", "enctype", "application/x-www-form-urlencoded"]],
  },
  {
    name: "POST enctype urlencoded -> text/plain",
    method: "post",
    mutate: [["form", "enctype", "text/plain"]],
  },
  {
    name: "POST submitter formenctype added text/plain",
    method: "post",
    mutate: [["submitter", "formenctype", "text/plain"]],
  },
  {
    name: "GET form action -> cross-origin",
    method: "get",
    mutate: [["form", "action", "CROSS/probe-dest/b"]],
  },
  {
    name: "POST form action -> cross-origin",
    method: "post",
    mutate: [["form", "action", "CROSS/probe-dest/b"]],
  },
];

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
    expect(oracle.errors).toEqual([]);
    expect(actual.errors).toEqual([]);
    const strip = (/** @type {any[]} */ rs) => rs.map(({ resourceType: _r, ...rest }) => rest);
    expect.soft(strip(actual.requests), "request(s) match native").toEqual(strip(oracle.requests));
    expect.soft(actual.finalUrl, "final URL matches native").toBe(oracle.finalUrl);
  });
}
