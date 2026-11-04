// Level 4: form recognition and submission metadata when real controls shadow
// built-in form properties (getAttribute, tagName, dataset) by name or id.
// https://html.spec.whatwg.org/multipage/forms.html#the-form-element (named properties)
//
// Oracle: the same form submitted in a document whose router module is stubbed
// out, so the browser alone owns the submission. data-zfb-reload is never used
// to build the oracle, because that opt-out is itself under test here.
// @ts-check
import { test, expect } from "@playwright/test";

const HARNESS_INIT = () => {
  // @ts-expect-error - test-only globals
  if (window.__zfbHarness) return;
  // @ts-expect-error
  window.__zfbHarness = true;
  const docId = crypto.randomUUID();
  // @ts-expect-error
  window.__docId = docId;
  for (const name of ["zfb:before-preparation", "zfb:page-load", "zfb:navigation-aborted"]) {
    document.addEventListener(name, () =>
      // @ts-expect-error - exposed by the context
      window.__zfbLog({ name, docId, url: location.href }),
    );
  }
};

const TITLES = { "/search.html": "Search", "/search-results.html": "Search Results" };

// fields: { name?, id?, value, type?, checked?, external? }. external controls sit
// outside the form and join it through form="fm".
async function installForm(page, config) {
  await page.evaluate((config) => {
    let form = document.createElement("form");
    if (config.foreignRealm) {
      // Created in another realm, so it is not instanceof this window's HTMLFormElement.
      const frame = document.createElement("iframe");
      document.body.append(frame);
      const foreignDocument = /** @type {Document} */ (frame.contentDocument);
      form = document.adoptNode(foreignDocument.createElement("form"));
      if (form instanceof HTMLFormElement) throw new Error("fixture form is same-realm");
    }
    form.id = "fm";
    for (const [attr, value] of Object.entries(config.form)) form.setAttribute(attr, value);
    for (const field of config.fields) {
      const el = document.createElement("input");
      el.setAttribute("type", field.type ?? "text");
      if (field.name !== undefined) el.setAttribute("name", field.name);
      if (field.id !== undefined) el.id = field.id;
      el.setAttribute("value", field.value);
      if (field.checked) el.setAttribute("checked", "");
      if (field.external) {
        el.setAttribute("form", "fm");
        document.body.append(el);
      } else {
        form.append(el);
      }
    }
    if (config.submitter !== null) {
      const button = document.createElement("button");
      button.id = "fm-submit";
      button.setAttribute("type", "submit");
      for (const [attr, value] of Object.entries(config.submitter)) {
        button.setAttribute(attr, value);
      }
      button.textContent = "Go";
      form.append(button);
    }
    document.querySelector("main")?.append(form);
  }, config);
}

// Proves each fixture really shadows the property before anything is submitted.
async function readCollisions(page, collisions) {
  return page.evaluate(
    (collisions) =>
      collisions.map(({ prop }) => {
        const form = /** @type {any} */ (document.getElementById("fm"));
        const value = form[prop];
        if (value instanceof RadioNodeList) return { prop, kind: "list", length: value.length };
        if (value instanceof Element) return { prop, kind: "element", id: value.id };
        return { prop, kind: typeof value };
      }),
    collisions,
  );
}

/**
 * Submit once. routerless: stub the router module so the document is native.
 * Returns the observed ownership, lifecycle and the single submission request.
 */
async function submitOnce(browser, config, { routerless }) {
  const context = await browser.newContext();
  try {
    const log = [];
    await context.exposeFunction("__zfbLog", (entry) => log.push(entry));
    if (routerless) {
      await context.route("**/dist/client-router/router.js", (route) =>
        route.fulfill({
          contentType: "application/javascript; charset=utf-8",
          body: "export function init() {}",
        }),
      );
    }
    const page = await context.newPage();
    await page.addInitScript(HARNESS_INIT);
    const errors = [];
    page.on("pageerror", (err) => errors.push(String(err)));

    await page.goto("/search.html");
    await expect(page.locator("h1")).toHaveText("Search");
    const docId = await page.evaluate(() => /** @type {any} */ (window).__docId);
    if (routerless) {
      expect(log).toEqual([]);
    } else {
      await expect.poll(() => log.filter((e) => e.name === "zfb:page-load").length).toBe(1);
    }

    await installForm(page, config);
    expect(await readCollisions(page, config.collisions)).toEqual(config.collisions);
    // Native form properties stay intact on the router's side of the submission.
    expect(await page.evaluate(() => document.getElementById("fm")?.localName)).toBe("form");

    const logBefore = log.length;
    const requests = [];
    const onRequest = (req) => {
      if (new URL(req.url()).pathname === config.destination) requests.push(req);
    };
    context.on("request", onRequest);

    if (config.submitter === null) {
      await page.evaluate(() =>
        /** @type {HTMLFormElement} */ (document.getElementById("fm")).requestSubmit(),
      );
    } else {
      // A trusted keyboard activation of the submitter. locator.click() is not
      // usable: Playwright's own actionability checks call getAttribute on the
      // form ancestor, which these fixtures deliberately shadow.
      await page.evaluate(() => document.getElementById("fm-submit")?.focus());
      await page.keyboard.press("Enter");
    }

    const startHref = page.url();
    await page.waitForURL((url) => url.href !== startHref && url.pathname === config.destination);
    await expect(page.locator("h1")).toHaveText(TITLES[config.destination]);
    const sameDocument = (await page.evaluate(() => /** @type {any} */ (window).__docId)) === docId;
    if (sameDocument) {
      await expect
        .poll(() =>
          log.slice(logBefore).filter((e) => e.name === "zfb:page-load" && e.docId === docId),
        )
        .toHaveLength(1);
      await page.waitForFunction(
        () => !document.documentElement.hasAttribute("data-zfb-transition"),
      );
    }
    context.off("request", onRequest);

    const lifecycle = log
      .slice(logBefore)
      .filter((e) => e.docId === docId)
      .map((e) => e.name);
    return {
      sameDocument,
      lifecycle,
      errors,
      requests: requests.map((req) => {
        const url = new URL(req.url());
        return {
          resourceType: req.resourceType(),
          method: req.method(),
          pathname: url.pathname,
          search: url.search,
          contentType: req.headers()["content-type"] ?? "",
          body: req.postDataBuffer()?.toString("latin1") ?? "",
        };
      }),
      location: new URL(page.url()).search,
    };
  } finally {
    await context.close();
  }
}

// Content-Type is compared by media type: fetch adds ";charset=UTF-8".
function payload(request) {
  const { resourceType: _type, contentType, ...rest } = request;
  return { ...rest, urlencoded: /^application\/x-www-form-urlencoded/i.test(contentType) };
}

const getAttrByName = [{ prop: "getAttribute", kind: "element", id: "" }];

const CASES = [
  {
    name: "collision-free control, GET, clicked submitter",
    form: { action: "/search-results.html", method: "get" },
    fields: [{ name: "q", value: "plain" }],
    submitter: { name: "go", value: "1" },
    collisions: [{ prop: "getAttribute", kind: "function" }],
    destination: "/search-results.html",
    owner: "router",
    expectInOracle: "q=plain&go=1",
  },
  {
    name: "getAttribute by name, GET, clicked submitter",
    form: { action: "/search-results.html", method: "get" },
    fields: [
      { name: "getAttribute", value: "g" },
      { name: "q", value: "a b" },
    ],
    submitter: { name: "go", value: "1" },
    collisions: getAttrByName,
    destination: "/search-results.html",
    owner: "router",
    expectInOracle: "getAttribute=g&q=a+b&go=1",
  },
  {
    name: "getAttribute by id, explicit URL-encoded POST, clicked submitter",
    form: {
      action: "/search-results.html",
      method: "post",
      enctype: "application/x-www-form-urlencoded",
    },
    fields: [{ id: "getAttribute", name: "idq", value: "by-id" }],
    submitter: {},
    collisions: [{ prop: "getAttribute", kind: "element", id: "getAttribute" }],
    destination: "/search-results.html",
    owner: "router",
    expectInOracle: "idq=by-id",
  },
  {
    name: "tagName by name, GET, requestSubmit() without submitter",
    form: { action: "/search-results.html", method: "get" },
    fields: [{ name: "tagName", value: "t" }],
    submitter: null,
    collisions: [{ prop: "tagName", kind: "element", id: "" }],
    destination: "/search-results.html",
    owner: "router",
    expectInOracle: "tagName=t",
  },
  {
    name: "tagName by id, explicit URL-encoded POST, requestSubmit() without submitter",
    form: {
      action: "/search-results.html",
      method: "post",
      enctype: "application/x-www-form-urlencoded",
    },
    fields: [{ id: "tagName", name: "tq", value: "x" }],
    submitter: null,
    collisions: [{ prop: "tagName", kind: "element", id: "tagName" }],
    destination: "/search-results.html",
    owner: "router",
    expectInOracle: "tq=x",
  },
  {
    name: "duplicate tagName radios (RadioNodeList), explicit URL-encoded POST",
    form: {
      action: "/search-results.html",
      method: "post",
      enctype: "application/x-www-form-urlencoded",
    },
    fields: [
      { type: "radio", name: "tagName", value: "one" },
      { type: "radio", name: "tagName", value: "two", checked: true },
    ],
    submitter: { name: "go", value: "r" },
    collisions: [{ prop: "tagName", kind: "list", length: 2 }],
    destination: "/search-results.html",
    owner: "router",
    expectInOracle: "tagName=two&go=r",
  },
  {
    name: "external form=... getAttribute control, GET",
    form: { action: "/search-results.html", method: "get" },
    fields: [
      { name: "inside", value: "i" },
      { name: "getAttribute", value: "ext", external: true },
    ],
    submitter: {},
    collisions: getAttrByName,
    destination: "/search-results.html",
    owner: "router",
    expectInOracle: "getAttribute=ext",
  },
  {
    name: "dataset by name without opt-out, GET",
    form: { action: "/search-results.html", method: "get" },
    fields: [{ name: "dataset", value: "d" }],
    submitter: {},
    collisions: [{ prop: "dataset", kind: "element", id: "" }],
    destination: "/search-results.html",
    owner: "router",
    expectInOracle: "dataset=d",
  },
  {
    name: "dataset by id with data-zfb-reload, GET, clicked submitter",
    form: { action: "/search-results.html", method: "get", "data-zfb-reload": "" },
    fields: [{ id: "dataset", name: "dq", value: "native" }],
    submitter: {},
    collisions: [{ prop: "dataset", kind: "element", id: "dataset" }],
    destination: "/search-results.html",
    owner: "native",
    expectInOracle: "dq=native",
  },
  {
    name: "dataset by name with data-zfb-reload, explicit URL-encoded POST, requestSubmit()",
    form: {
      action: "/search-results.html",
      method: "post",
      enctype: "application/x-www-form-urlencoded",
      "data-zfb-reload": "",
    },
    fields: [{ name: "dataset", value: "d" }],
    submitter: null,
    collisions: [{ prop: "dataset", kind: "element", id: "" }],
    destination: "/search-results.html",
    owner: "native",
    expectInOracle: "dataset=d",
  },
  {
    name: "getAttribute by name with explicit-empty formaction/formmethod/formtarget",
    // Empty overrides: document URL, GET, and the (absent) base target.
    form: { action: "/search-results.html", method: "post", target: "_blank" },
    fields: [{ name: "getAttribute", value: "e" }],
    submitter: { formaction: "", formmethod: "", formtarget: "" },
    collisions: getAttrByName,
    destination: "/search.html",
    owner: "router",
    expectInOracle: "getAttribute=e",
  },
  {
    name: "tagName by name with formmethod=post and explicit-empty formenctype",
    form: { action: "/search.html", method: "get" },
    fields: [{ name: "tagName", value: "p" }],
    submitter: { formaction: "/search-results.html", formmethod: "post", formenctype: "" },
    collisions: [{ prop: "tagName", kind: "element", id: "" }],
    destination: "/search-results.html",
    owner: "router",
    expectInOracle: "tagName=p",
  },
  {
    name: "form adopted from another realm with a tagName control, GET",
    foreignRealm: true,
    form: { action: "/search-results.html", method: "get" },
    fields: [{ name: "tagName", value: "realm" }],
    submitter: {},
    collisions: [{ prop: "tagName", kind: "element", id: "" }],
    destination: "/search-results.html",
    owner: "router",
    expectInOracle: "tagName=realm",
  },
];

for (const config of CASES) {
  test(`${config.name}: ${config.owner} owns the submission`, async ({ browser }) => {
    const oracle = await submitOnce(browser, config, { routerless: true });
    expect(oracle.errors).toEqual([]);
    expect(oracle.sameDocument).toBe(false);
    expect(oracle.requests).toHaveLength(1);
    expect(oracle.requests[0].resourceType).toBe("document");
    const [native] = oracle.requests;
    expect(native.method === "GET" ? native.search : native.body).toContain(config.expectInOracle);
    expect(payload(native).urlencoded).toBe(native.method === "POST");

    const actual = await submitOnce(browser, config, { routerless: false });
    expect(actual.errors).toEqual([]);
    expect(actual.requests).toHaveLength(1);
    expect(payload(actual.requests[0])).toEqual(payload(native));
    expect(actual.location).toBe(native.method === "GET" ? native.search : "");

    if (config.owner === "router") {
      expect(actual.sameDocument).toBe(true);
      expect(actual.requests[0].resourceType).toBe("fetch");
      expect(actual.lifecycle).toEqual(["zfb:before-preparation", "zfb:page-load"]);
    } else {
      expect(actual.sameDocument).toBe(false);
      expect(actual.requests[0].resourceType).toBe("document");
      expect(actual.lifecycle).toEqual([]);
    }
  });
}
