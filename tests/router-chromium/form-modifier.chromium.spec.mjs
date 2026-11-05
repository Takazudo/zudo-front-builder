// L4 trusted-input oracle: identical markup with the router module stubbed out.
import { test, expect } from "@playwright/test";

const DESTINATION = "/search-results.html";
const MODIFIER = process.platform === "darwin" ? "Meta" : "Control";
const LIFECYCLE = [
  "zfb:before-preparation",
  "zfb:after-preparation",
  "zfb:before-swap",
  "zfb:after-swap",
  "zfb:page-load",
];

async function runCase(browser, config, routerless) {
  const context = await browser.newContext();
  const events = [];
  const requests = [];
  const errors = [];
  const pages = [];
  try {
    await context.exposeFunction("__modifierLog", (entry) => events.push(entry));
    context.on("page", (page) => {
      pages.push(page);
      page.on("pageerror", (error) => errors.push(String(error)));
    });
    context.on("request", (request) => {
      const url = new URL(request.url());
      if (url.pathname !== DESTINATION) return;
      requests.push({
        method: request.method(),
        resourceType: request.resourceType(),
        search: url.search,
        body: request.postData() ?? "",
        contentType: (request.headers()["content-type"] ?? "").split(";")[0],
      });
    });
    await context.addInitScript((names) => {
      window.__modifierDoc =
        typeof crypto.randomUUID === "function" ? crypto.randomUUID() : String(Math.random());
      for (const name of names) {
        document.addEventListener(name, () =>
          window.__modifierLog({ name, docId: window.__modifierDoc, url: location.href }),
        );
      }
      for (const name of ["click", "submit", "invalid"]) {
        document.addEventListener(
          name,
          (event) => {
            window.__modifierLog({
              name,
              docId: window.__modifierDoc,
              trusted: event.isTrusted,
              target: event.target.id,
              submitter: event.submitter?.id ?? null,
              ctrlKey: event.ctrlKey ?? false,
              metaKey: event.metaKey ?? false,
              altKey: event.altKey ?? false,
              shiftKey: event.shiftKey ?? false,
            });
          },
          true,
        );
      }
    }, LIFECYCLE);
    if (routerless) {
      await context.route("**/dist/client-router/router.js", (route) =>
        route.fulfill({ contentType: "application/javascript", body: "export function init() {}" }),
      );
    }
    const page = await context.newPage();
    await page.goto("/search.html");
    await expect(page.locator("h1")).toHaveText("Search");
    if (!routerless)
      await expect.poll(() => events.filter((e) => e.name === "zfb:page-load").length).toBe(1);
    const docId = await page.evaluate(() => window.__modifierDoc);
    await page.evaluate((config) => {
      const form = document.createElement("form");
      form.id = "modifier-form";
      form.action = "/search-results.html";
      form.method = config.method ?? "get";
      form.enctype = "application/x-www-form-urlencoded";
      form.innerHTML = '<input id="modifier-field" name="q" value="hello world">';
      const button = document.createElement(config.input ? "input" : "button");
      button.id = "modifier-submit";
      button.type = "submit";
      button.name = "go";
      button.value = "search";
      // A known button-only hit area, separate from both nested children.
      button.style.cssText = "display:block;width:220px;height:100px;padding:20px";
      if (!config.input)
        button.innerHTML =
          '<span id="modifier-span">Search</span><svg width="40" height="30"><rect id="modifier-svg" width="40" height="30" /></svg>';
      document.querySelector("main").append(form);
      if (config.external) {
        button.setAttribute("form", form.id);
        document.querySelector("main").append(button);
      } else form.append(button);
      if (config.block === "invalid") {
        const field = document.getElementById("modifier-field");
        field.required = true;
        field.value = "";
      }
      if (config.block === "cancel") {
        // Later than the router's document listener; cancellation must still win.
        window.addEventListener("click", (event) => event.preventDefault(), { once: true });
      }
      if (config.link) {
        const link = document.createElementNS(
          config.link === "svg" ? "http://www.w3.org/2000/svg" : "http://www.w3.org/1999/xhtml",
          config.link === "area" ? "area" : "a",
        );
        link.id = "modifier-link";
        link.setAttribute("href", "/search-results.html?q=hello+world&go=search");
        if (config.link === "svg") {
          link.innerHTML = '<rect id="modifier-link-child" width="100" height="40" />';
          const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
          svg.append(link);
          document.querySelector("main").append(svg);
        } else if (config.link === "area") {
          const map = document.createElement("map");
          map.name = "modifier-map";
          link.setAttribute("shape", "rect");
          link.setAttribute("coords", "0,0,100,40");
          map.append(link);
          const image = document.createElement("img");
          image.id = "modifier-map-image";
          image.width = 100;
          image.height = 40;
          image.useMap = "#modifier-map";
          image.src =
            'data:image/svg+xml,<svg xmlns="http://www.w3.org/2000/svg" width="100" height="40"><rect width="100" height="40" fill="red"/></svg>';
          document.querySelector("main").append(map, image);
        } else {
          link.innerHTML = '<span id="modifier-link-child">Results</span>';
          document.querySelector("main").append(link);
        }
      }
    }, config);
    await context.request.get("/__form-requests");
    const before = events.length;
    let hit =
      config.input || config.direct ? "#modifier-submit" : `#modifier-${config.child ?? "span"}`;
    if (config.link) hit = config.link === "area" ? "#modifier-map-image" : "#modifier-link-child";
    const clickOptions = { modifiers: config.ordinary ? [] : [MODIFIER] };
    if (config.direct) clickOptions.position = { x: 5, y: 5 };
    await page.locator(hit).click(clickOptions);
    if (config.block) {
      // Cross a task boundary so the later requestSubmit is a separate activation,
      // without an intervening click resetting router state.
      await page.evaluate(() => new Promise((resolve) => setTimeout(resolve, 0)));
      expect(requests).toEqual([]);
      expect(await (await context.request.get("/__form-requests")).json()).toEqual([]);
      const blocked = events.slice(before);
      expect(blocked.filter((e) => e.name === "submit")).toEqual([]);
      if (config.block === "invalid")
        expect(blocked.some((e) => e.name === "invalid" && e.trusted)).toBe(true);
      await page.evaluate((keyboard) => {
        document.getElementById("modifier-field").value = "hello world";
        if (keyboard) document.getElementById("modifier-submit").focus();
        else
          document
            .getElementById("modifier-form")
            .requestSubmit(document.getElementById("modifier-submit"));
      }, config.keyboard === true);
      if (config.keyboard) await page.keyboard.press("Enter");
    }
    await expect
      .poll(() => context.pages().filter((p) => new URL(p.url()).pathname === DESTINATION).length)
      .toBe(1);
    const destination = context.pages().find((p) => new URL(p.url()).pathname === DESTINATION);
    await expect(destination.locator("h1")).toHaveText("Search Results");
    const sameDocument = await page
      .evaluate(() => (window.__modifierDoc === undefined ? false : window.__modifierDoc))
      .then((id) => id === docId);
    const spa = requests.some((request) => request.resourceType === "fetch");
    if (spa) {
      await expect
        .poll(
          () =>
            events.slice(before).filter((e) => e.docId === docId && e.name === "zfb:page-load")
              .length,
        )
        .toBe(1);
      await page.waitForFunction(
        () => !document.documentElement.hasAttribute("data-zfb-transition"),
      );
    }
    // Complete the destination load and a browser task before checking duplicates.
    await destination.waitForLoadState("load");
    await destination.evaluate(() => new Promise((resolve) => setTimeout(resolve, 0)));
    const serverRequests = await (await context.request.get("/__form-requests")).json();
    const submissionRequests = serverRequests.map(({ destination, ...request }) => ({
      ...request,
      resourceType:
        destination === "document" ? "document" : destination === "empty" ? "fetch" : destination,
    }));
    // Sec-Fetch-Dest covers the initial popup navigation even on Chromium
    // versions whose protocol omits that request from Playwright's context events.
    for (const request of requests) expect(submissionRequests).toContainEqual(request);
    return {
      requests: submissionRequests,
      browserRequests: requests,
      errors,
      sameDocument,
      originalPath: new URL(page.url()).pathname,
      destinationSearch: new URL(destination.url()).search,
      destinationInOriginal: destination === page,
      pageCount: pages.length,
      lifecycle: events
        .slice(before)
        .filter((e) => e.docId === docId && LIFECYCLE.includes(e.name))
        .map((e) => e.name),
      clicks: events.slice(before).filter((e) => e.name === "click"),
      submits: events.slice(before).filter((e) => e.name === "submit"),
    };
  } finally {
    await context.close();
  }
}

const CASES = [
  { name: "modified span GET" },
  { name: "modified SVG GET", child: "svg" },
  { name: "modified external span GET", external: true },
  { name: "modified span URL-encoded POST", method: "post" },
  { name: "ordinary span GET", ordinary: true },
  { name: "ordinary SVG URL-encoded POST", ordinary: true, child: "svg", method: "post" },
  { name: "direct modified button", direct: true },
  { name: "direct modified input", input: true },
  ...["html", "svg", "area"].map((link) => ({ name: `modified ${link} link`, link })),
  ...["cancel", "invalid"].flatMap((block) => [
    { name: `${block} then requestSubmit without another click`, block, direct: true },
    { name: `${block} then trusted keyboard activation`, block, direct: true, keyboard: true },
  ]),
];

for (const config of CASES) {
  test(config.name, async ({ browser }, testInfo) => {
    const oracle = await runCase(browser, config, true);
    const actual = await runCase(browser, config, false);
    await testInfo.attach("native-and-router", {
      body: JSON.stringify({ oracle, actual }, null, 2),
      contentType: "application/json",
    });
    for (const result of [oracle, actual]) {
      expect(result.errors).toEqual([]);
      expect(result.requests).toHaveLength(1);
      const click = result.clicks[0];
      expect(click.trusted).toBe(true);
      expect(click[MODIFIER === "Meta" ? "metaKey" : "ctrlKey"]).toBe(!config.ordinary);
      const target = config.link
        ? config.link === "area"
          ? "modifier-link"
          : "modifier-link-child"
        : config.input || config.direct
          ? "modifier-submit"
          : `modifier-${config.child ?? "span"}`;
      expect(click.target).toBe(target);
      if (!config.link) {
        expect(result.submits).toHaveLength(1);
        expect(result.submits[0]).toMatchObject({ submitter: "modifier-submit", trusted: true });
      }
    }
    const nativeRequest = oracle.requests[0];
    expect(nativeRequest.resourceType).toBe("document");
    expect(nativeRequest.method).toBe(config.method === "post" ? "POST" : "GET");
    expect(nativeRequest.method === "POST" ? nativeRequest.body : nativeRequest.search).toBe(
      `${nativeRequest.method === "POST" ? "" : "?"}q=hello+world&go=search`,
    );
    if (config.method === "post")
      expect(nativeRequest.contentType).toBe("application/x-www-form-urlencoded");
    const { resourceType: _nativeType, ...nativePayload } = nativeRequest;
    const { resourceType: _actualType, ...actualPayload } = actual.requests[0];
    expect(actualPayload).toEqual(nativePayload);
    if (config.ordinary || config.block) {
      expect(actual.requests[0].resourceType).toBe("fetch");
      expect(actual.sameDocument).toBe(true);
      expect(actual.pageCount).toBe(1);
      expect(actual.lifecycle).toEqual(LIFECYCLE);
    } else {
      expect(actual.requests[0].resourceType).toBe("document");
      for (const key of [
        "sameDocument",
        "originalPath",
        "destinationSearch",
        "destinationInOriginal",
        "pageCount",
      ])
        expect(actual[key], key).toEqual(oracle[key]);
      expect(actual.lifecycle).toEqual([]);
    }
  });
}
