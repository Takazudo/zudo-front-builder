// Level 4: a submitter invalidated by an earlier form-level submit listener
// follows the native oracle (#4612). Native submission keeps the original
// submitter after the submit event, while `new FormData(form, submitter)`
// throws unless that submitter is still a submit button owned by `form`:
// https://xhr.spec.whatwg.org/#dom-formdata
// https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#form-submission-algorithm
//
// Oracle: identical markup with the router module stubbed, so the browser alone
// owns the request. The submitter starts valid and is mutated inside the form's
// own submit listener, which runs before the router's document listener.
// Activation is a trusted click or native requestSubmit(). Every case then
// submits a fresh valid form, so stale router state is visible and "no request"
// never rests on a timeout alone.
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
// Bounded wait for a first-attempt request before treating it as absent.
const SETTLE_MS = 1500;

/**
 * @typedef {"remove" | "remove-reinsert" | "move" | "move-restore" | "reassociate"
 *   | "reassociate-restore" | "unassociate" | "type" | "type-restore" | "disable"} Mutation
 * @typedef {{ name: string, method: "get" | "post", control: "button" | "input" | "image",
 *   external?: boolean, foreign?: boolean, buttonType?: string | null, mutate: Mutation | null,
 *   activation: "click" | "request-submit" | "request-submit-null", cancel?: boolean,
 *   routed: boolean }} Config
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
    await context.exposeFunction("__slLog", (entry) => events.push({ ...entry, phase }));
    await context.addInitScript((names) => {
      // @ts-expect-error - test-only globals
      window.__slDoc = crypto.randomUUID();
      for (const name of names) {
        document.addEventListener(name, () =>
          // @ts-expect-error
          window.__slLog({ name, docId: window.__slDoc }),
        );
      }
      // Capture phase on window: sees every submit event before any listener.
      window.addEventListener(
        "submit",
        (event) =>
          // @ts-expect-error
          window.__slLog({
            name: "submit-start",
            // @ts-expect-error
            docId: window.__slDoc,
            // @ts-expect-error
            target: event.target.id,
          }),
        true,
      );
      // Bubble phase on window: runs after the router's document listener.
      window.addEventListener("submit", (event) =>
        // @ts-expect-error
        window.__slLog({
          name: "submit-final",
          // @ts-expect-error
          docId: window.__slDoc,
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
    page.on("pageerror", (error) => errors.push(`${phase}: ${String(error)}`));
    page.on("console", (message) => {
      if (message.type() === "warning" || message.type() === "error")
        messages.push({ type: message.type(), text: message.text(), phase });
    });
    await page.goto("/search.html");
    await expect(page.locator("h1")).toHaveText("Search");
    if (!routerless)
      await expect.poll(() => events.filter((e) => e.name === "zfb:page-load").length).toBe(1);
    const docId = await page.evaluate(() => /** @type {any} */ (window).__slDoc);
    await context.request.get("/__form-requests");

    const submitterState = await page.evaluate(installForm, {
      config,
      id: "sl",
      value: "hello world",
      mutate: config.mutate,
    });
    const before = await page.evaluate(() => ({ url: location.href, length: history.length }));
    phase = 1;
    if (config.activation === "click") await page.locator("#sl-submit").click();
    else
      await page.evaluate((withSubmitter) => {
        const form = /** @type {HTMLFormElement} */ (document.getElementById("sl"));
        const submitter = /** @type {HTMLElement} */ (document.getElementById("sl-submit"));
        if (withSubmitter) form.requestSubmit(submitter);
        else form.requestSubmit();
      }, config.activation === "request-submit");

    /** @type {any[]} */
    let first = [];
    const deadline = Date.now() + SETTLE_MS;
    while (Date.now() < deadline) {
      first = first.concat(await (await context.request.get("/__form-requests")).json());
      if (first.length > 0) break;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    const proceeded = first.length > 0;
    const lifecycleOf = (/** @type {number} */ p, /** @type {string} */ doc) =>
      events.filter((e) => e.phase === p && e.docId === doc && LIFECYCLE.includes(e.name));
    if (lifecycleOf(1, docId).length > 0) {
      await expect
        .poll(() => lifecycleOf(1, docId).filter((e) => e.name === "zfb:page-load").length)
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
      docId: window.__slDoc,
    }));
    first = first.concat(await (await context.request.get("/__form-requests")).json());
    if (!routerless && after1.docId !== docId)
      await expect
        .poll(
          () => events.filter((e) => e.docId === after1.docId && e.name === "zfb:page-load").length,
        )
        .toBe(1);

    // Attempt 2: a fresh valid form on whatever document is current.
    phase = 2;
    await page.evaluate(installForm, { config: SECOND, id: "sl2", value: "second", mutate: null });
    await page.locator("#sl2-submit").click();
    /** @type {any[]} */
    let second = [];
    await expect
      .poll(async () => {
        second = second.concat(await (await context.request.get("/__form-requests")).json());
        return second.length;
      })
      .toBeGreaterThanOrEqual(1);
    const lifecycle2 = () => lifecycleOf(2, after1.docId);
    await page.waitForURL((url) => url.searchParams.get("q") === "second");
    await page.waitForLoadState("load");
    if (lifecycle2().length > 0) {
      await expect
        .poll(() => lifecycle2().filter((e) => e.name === "zfb:page-load").length)
        .toBe(1);
      await page.waitForFunction(
        () => !document.documentElement.hasAttribute("data-zfb-transition"),
      );
    }
    await page.evaluate(() => new Promise((resolve) => setTimeout(resolve, 0)));
    second = second.concat(await (await context.request.get("/__form-requests")).json());
    const docId2 = await page.evaluate(() => /** @type {any} */ (window).__slDoc);

    const byPhase = (/** @type {number} */ p, /** @type {string} */ name) =>
      events.filter((e) => e.phase === p && e.name === name);
    return {
      errors,
      warnings: messages.filter((c) => c.phase === 1).map(({ type, text }) => ({ type, text })),
      submitterState,
      proceeded,
      first: normalize(first),
      sameDocument1: after1.docId === docId,
      urlChanged1: after1.url !== before.url,
      historyDelta1: after1.length - before.length,
      lifecycle1: lifecycleOf(1, docId).map((e) => e.name),
      submits1: byPhase(1, "submit-start").map((e) => e.target),
      mutated: byPhase(1, "mutated").map(({ owner, type, connected }) => ({
        owner,
        type,
        connected,
      })),
      formdata1: byPhase(1, "formdata").length,
      submitFinal: byPhase(1, "submit-final").map(({ defaultPrevented, target }) => ({
        defaultPrevented,
        target,
      })),
      second: {
        requests: normalize(second),
        sameDocument: docId2 === after1.docId,
        lifecycle: lifecycle2().map((e) => e.name),
        submits: byPhase(2, "submit-start").map((e) => e.target),
        formdata: byPhase(2, "formdata").length,
      },
    };
  } finally {
    await context.close();
  }
}

/** @param {any[]} requests */
function normalize(requests) {
  return requests.map(({ destination, ...request }) => ({
    ...request,
    // Image activation coordinates are engine/layout values, not router behavior.
    search: request.search.replace(/(\.[xy]=)\d+/g, "$1N"),
    body: request.body.replace(/(\.[xy]=)\d+/g, "$1N"),
    resourceType:
      destination === "document" ? "document" : destination === "empty" ? "fetch" : destination,
  }));
}

/**
 * Builds the form, an unrelated second form, and the submitter. The returned
 * record states the submitter's validity before activation, proving every
 * mutation starts from an eligible submitter.
 * @param {{ config: Config, id: string, value: string, mutate: Mutation | null }} args
 */
function installForm({ config, id, value, mutate }) {
  const host = document.querySelector("main") ?? document.body;
  const make = (/** @type {string} */ formId, /** @type {string} */ fieldValue) => {
    const form = document.createElement("form");
    form.id = formId;
    form.setAttribute("action", "/search-results.html");
    form.setAttribute("method", config.method);
    form.setAttribute("enctype", "application/x-www-form-urlencoded");
    const field = document.createElement("input");
    field.setAttribute("name", "q");
    field.setAttribute("value", fieldValue);
    form.append(field);
    form.addEventListener("formdata", () =>
      // @ts-expect-error - test-only global
      window.__slLog({ name: "formdata", docId: window.__slDoc, form: formId }),
    );
    host.append(form);
    return form;
  };
  const form = make(id, value);
  const other = make(`${id}-other`, "other");

  let ownerDocument = document;
  if (config.foreign) {
    const frame = document.createElement("iframe");
    document.body.append(frame);
    const foreign = frame.contentDocument;
    if (!foreign) throw new Error("iframe has no document");
    ownerDocument = foreign;
  }
  const tag = config.control === "button" ? "button" : "input";
  const created = ownerDocument.createElement(tag);
  const submitter = config.foreign ? document.adoptNode(created) : created;
  if (config.foreign && submitter instanceof HTMLElement)
    throw new Error("adopted submitter is same-realm");
  submitter.id = `${id}-submit`;
  if (config.control === "button") {
    if (config.buttonType !== null) submitter.setAttribute("type", config.buttonType ?? "submit");
    submitter.textContent = "Search";
  } else {
    submitter.setAttribute("type", config.control === "image" ? "image" : "submit");
    if (config.control === "image") {
      submitter.setAttribute("alt", "Search");
      submitter.setAttribute("width", "40");
      submitter.setAttribute("height", "20");
    }
  }
  submitter.setAttribute("name", "go");
  submitter.setAttribute("value", "search");
  if (config.external) {
    submitter.setAttribute("form", id);
    host.append(submitter);
  } else {
    form.append(submitter);
  }

  const nonSubmitType = config.control === "button" ? "button" : "text";
  const originalType = submitter.getAttribute("type");
  const restoreType = () =>
    originalType === null
      ? submitter.removeAttribute("type")
      : submitter.setAttribute("type", originalType);
  if (mutate || config.cancel) {
    form.addEventListener(
      "submit",
      (event) => {
        const parent = /** @type {ParentNode} */ (submitter.parentNode);
        const next = submitter.nextSibling;
        switch (mutate) {
          case "remove":
            submitter.remove();
            break;
          case "remove-reinsert":
            submitter.remove();
            parent.insertBefore(submitter, next);
            break;
          case "move":
            other.append(submitter);
            break;
          case "move-restore":
            other.append(submitter);
            parent.insertBefore(submitter, next);
            break;
          case "reassociate":
            submitter.setAttribute("form", other.id);
            break;
          case "reassociate-restore":
            submitter.setAttribute("form", other.id);
            submitter.setAttribute("form", id);
            break;
          case "unassociate":
            submitter.removeAttribute("form");
            break;
          case "type":
            submitter.setAttribute("type", nonSubmitType);
            break;
          case "type-restore":
            submitter.setAttribute("type", nonSubmitType);
            restoreType();
            break;
          case "disable":
            submitter.setAttribute("disabled", "");
            break;
        }
        if (config.cancel) event.preventDefault();
        // @ts-expect-error - test-only global
        window.__slLog({
          name: "mutated",
          // @ts-expect-error
          docId: window.__slDoc,
          owner: /** @type {any} */ (submitter).form?.id ?? null,
          type: /** @type {any} */ (submitter).type,
          connected: submitter.isConnected,
        });
      },
      { once: true },
    );
  }
  return {
    owner: /** @type {any} */ (submitter).form?.id ?? null,
    type: /** @type {any} */ (submitter).type,
    connected: submitter.isConnected,
    sameRealm: submitter instanceof HTMLElement,
  };
}

/** @type {Config} */
const SECOND = {
  name: "second",
  method: "get",
  control: "button",
  mutate: null,
  activation: "click",
  routed: true,
};

/** @type {Config[]} */
const CASES = [
  // Stable submitters keep router ownership.
  {
    name: "GET stable button",
    method: "get",
    control: "button",
    mutate: null,
    activation: "click",
    routed: true,
  },
  {
    name: "POST stable button",
    method: "post",
    control: "button",
    mutate: null,
    activation: "click",
    routed: true,
  },
  {
    name: "GET stable external button",
    method: "get",
    control: "button",
    external: true,
    mutate: null,
    activation: "click",
    routed: true,
  },
  {
    name: "GET stable input[type=submit]",
    method: "get",
    control: "input",
    mutate: null,
    activation: "click",
    routed: true,
  },
  {
    name: "GET stable input[type=image]",
    method: "get",
    control: "image",
    mutate: null,
    activation: "click",
    routed: true,
  },
  {
    name: "GET button without type attribute",
    method: "get",
    control: "button",
    buttonType: null,
    mutate: null,
    activation: "click",
    routed: true,
  },
  {
    name: "GET button with invalid type keyword",
    method: "get",
    control: "button",
    buttonType: "bogus",
    mutate: null,
    activation: "click",
    routed: true,
  },
  {
    name: "GET null-submitter requestSubmit()",
    method: "get",
    control: "button",
    mutate: null,
    activation: "request-submit-null",
    routed: true,
  },
  {
    name: "POST null-submitter requestSubmit()",
    method: "post",
    control: "button",
    mutate: null,
    activation: "request-submit-null",
    routed: true,
  },
  {
    name: "GET button disabled in submit listener",
    method: "get",
    control: "button",
    mutate: "disable",
    activation: "click",
    routed: true,
  },

  // Invalidated before the router's checkpoint.
  {
    name: "GET button removed",
    method: "get",
    control: "button",
    mutate: "remove",
    activation: "click",
    routed: false,
  },
  {
    name: "POST button removed",
    method: "post",
    control: "button",
    mutate: "remove",
    activation: "click",
    routed: false,
  },
  {
    name: "GET input[type=submit] removed",
    method: "get",
    control: "input",
    mutate: "remove",
    activation: "click",
    routed: false,
  },
  {
    name: "GET input[type=image] removed",
    method: "get",
    control: "image",
    mutate: "remove",
    activation: "click",
    routed: false,
  },
  {
    name: "GET requestSubmit(button) then removed",
    method: "get",
    control: "button",
    mutate: "remove",
    activation: "request-submit",
    routed: false,
  },
  {
    name: "GET external button removed",
    method: "get",
    control: "button",
    external: true,
    mutate: "remove",
    activation: "click",
    routed: false,
  },
  {
    name: "GET external button reassociated to a second form",
    method: "get",
    control: "button",
    external: true,
    mutate: "reassociate",
    activation: "click",
    routed: false,
  },
  {
    name: "POST external input[type=submit] reassociated to a second form",
    method: "post",
    control: "input",
    external: true,
    mutate: "reassociate",
    activation: "click",
    routed: false,
  },
  {
    name: "GET external button form attribute removed",
    method: "get",
    control: "button",
    external: true,
    mutate: "unassociate",
    activation: "click",
    routed: false,
  },
  {
    name: "GET button moved into a second form",
    method: "get",
    control: "button",
    mutate: "move",
    activation: "click",
    routed: false,
  },
  {
    name: "POST button moved into a second form",
    method: "post",
    control: "button",
    mutate: "move",
    activation: "click",
    routed: false,
  },
  {
    name: "GET button changed to type=button",
    method: "get",
    control: "button",
    mutate: "type",
    activation: "click",
    routed: false,
  },
  {
    name: "POST button changed to type=button",
    method: "post",
    control: "button",
    mutate: "type",
    activation: "click",
    routed: false,
  },
  {
    name: "GET input[type=submit] changed to type=text",
    method: "get",
    control: "input",
    mutate: "type",
    activation: "click",
    routed: false,
  },
  {
    name: "POST input[type=submit] changed to type=text",
    method: "post",
    control: "input",
    mutate: "type",
    activation: "click",
    routed: false,
  },
  {
    name: "GET adopted button removed",
    method: "get",
    control: "button",
    foreign: true,
    mutate: "remove",
    activation: "click",
    routed: false,
  },
  {
    name: "GET adopted external button changed to type=button",
    method: "get",
    control: "button",
    foreign: true,
    external: true,
    mutate: "type",
    activation: "click",
    routed: false,
  },

  // Restored before the router's checkpoint: judged by current state, no latch.
  {
    name: "GET button removed and reinserted",
    method: "get",
    control: "button",
    mutate: "remove-reinsert",
    activation: "click",
    routed: true,
  },
  {
    name: "POST external button removed and reinserted",
    method: "post",
    control: "button",
    external: true,
    mutate: "remove-reinsert",
    activation: "click",
    routed: true,
  },
  {
    name: "GET button moved away and back",
    method: "get",
    control: "button",
    mutate: "move-restore",
    activation: "click",
    routed: true,
  },
  {
    name: "GET external button reassociated and restored",
    method: "get",
    control: "button",
    external: true,
    mutate: "reassociate-restore",
    activation: "click",
    routed: true,
  },
  {
    name: "GET button type changed and restored",
    method: "get",
    control: "button",
    mutate: "type-restore",
    activation: "click",
    routed: true,
  },
  {
    name: "POST input[type=submit] type changed and restored",
    method: "post",
    control: "input",
    mutate: "type-restore",
    activation: "click",
    routed: true,
  },
  {
    name: "GET adopted external button reassociated and restored",
    method: "get",
    control: "button",
    foreign: true,
    external: true,
    mutate: "reassociate-restore",
    activation: "click",
    routed: true,
  },

  // Canceled by the page: neither the browser nor the router submits.
  {
    name: "GET canceled submission with removed button",
    method: "get",
    control: "button",
    mutate: "remove",
    cancel: true,
    activation: "click",
    routed: false,
  },
];

/** @param {any} request */
const wire = ({ resourceType: _r, ...request }) => request;

for (const config of CASES) {
  test(config.name, async ({ browser }, testInfo) => {
    const oracle = await runCase(browser, config, true);
    const actual = await runCase(browser, config, false);
    await testInfo.attach("native-and-router", {
      body: JSON.stringify({ config, oracle, actual }, null, 2),
      contentType: "application/json",
    });

    for (const result of [oracle, actual]) {
      // Every mutation starts from a valid submit button owned by the form.
      expect(result.submitterState.owner).toBe("sl");
      expect(result.submitterState.connected).toBe(true);
      expect(result.submitterState.sameRealm).toBe(!config.foreign);
      expect(result.submits1).toEqual(["sl"]);
      expect(result.submitFinal).toHaveLength(1);
    }
    // The router adds no page error or diagnostic of its own.
    expect(oracle.errors).toEqual([]);
    expect(actual.errors).toEqual([]);
    expect(actual.warnings).toEqual(oracle.warnings);

    // Wire semantics match the engine's native outcome, request for request.
    expect(actual.first.map(wire)).toEqual(oracle.first.map(wire));
    expect(oracle.submitFinal[0].defaultPrevented).toBe(Boolean(config.cancel));

    if (config.cancel) {
      for (const result of [oracle, actual]) {
        expect(result.proceeded).toBe(false);
        expect(result.sameDocument1).toBe(true);
        expect(result.urlChanged1).toBe(false);
        expect(result.historyDelta1).toBe(0);
        expect(result.lifecycle1).toEqual([]);
        expect(result.formdata1).toBe(0);
      }
    } else if (config.routed) {
      expect(oracle.first).toHaveLength(1);
      expect(oracle.first[0].resourceType).toBe("document");
      expect(actual.first[0].resourceType).toBe("fetch");
      expect(actual.submitFinal[0].defaultPrevented).toBe(true);
      expect(actual.sameDocument1).toBe(true);
      expect(actual.lifecycle1).toEqual(LIFECYCLE);
      expect(actual.formdata1).toBe(1);
    } else {
      // Declined: the browser's own submission, with no router lifecycle and
      // no extra entry-list construction.
      for (const result of [oracle, actual]) {
        expect(result.proceeded).toBe(true);
        expect(result.first).toHaveLength(1);
        expect(result.first[0].resourceType).toBe("document");
        expect(result.sameDocument1).toBe(false);
      }
      expect(actual.submitFinal[0].defaultPrevented).toBe(false);
      expect(actual.lifecycle1).toEqual([]);
      expect(actual.formdata1).toBe(oracle.formdata1);
    }

    // Follow-up valid submission: same wire request, routed by the router.
    expect(oracle.second.requests).toHaveLength(1);
    expect(actual.second.requests.map(wire)).toEqual(oracle.second.requests.map(wire));
    expect(actual.second.requests[0].search).toBe("?q=second&go=search");
    expect(actual.second.requests[0].resourceType).toBe("fetch");
    expect(actual.second.sameDocument).toBe(true);
    expect(actual.second.lifecycle).toEqual(LIFECYCLE);
    expect(actual.second.submits).toEqual(["sl2"]);
    expect(actual.second.formdata).toBe(1);
  });
}
