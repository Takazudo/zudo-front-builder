/**
 * form-submit.chromium.spec.mjs — Level-4 (real browser) confirm gate for
 * form-submit interception (issue #1400).
 *
 * ============================================================
 * WHY THIS SUITE EXISTS
 * ============================================================
 * `packages/zfb-runtime/src/__tests__/client-router/router.test.ts`'s
 * "handleSubmit — form intercept (#1400)" describe block dispatches
 * `SubmitEvent`s directly via happy-dom's `form.requestSubmit()` and asserts
 * on the mocked `fetch` call — it proves handleSubmit's BRANCH LOGIC (GET
 * query serialization, POST body, formaction/formmethod overrides, the
 * dialog/cross-origin/modifier-click skips), not that a real `<form>`
 * submission gesture in a real browser produces a genuine SPA navigation with
 * no full-page reload. This file is that proof for the GET path — the most
 * common real-world form (a search box).
 *
 * ============================================================
 * DEFLAKING DISCIPLINE (issue #1346) — same rules as router.chromium.spec.mjs
 * ============================================================
 * Every wait is keyed on the router's OWN lifecycle events or on state the
 * router itself sets — NO bare `waitForTimeout`, NO `networkidle`. The
 * harness below (init script + helpers) is intentionally a trimmed copy of
 * router.chromium.spec.mjs's, mirroring the tests/webkit-back-history vs.
 * tests/router-chromium precedent of deliberately duplicating shared harness
 * plumbing across sibling Level-4 suites rather than coupling them through a
 * shared module (see serve-fixture.mjs's header comment in this directory).
 */

// @ts-check
import { test, expect } from "@playwright/test";

// ---------------------------------------------------------------------------
// Harness installed before every document, before any page script runs.
// (Trimmed from router.chromium.spec.mjs — see that file's header for the
// full rationale of each piece; this spec only needs the lifecycle recorder
// and the per-document __docId, not the View Transitions instrumentation.)
// ---------------------------------------------------------------------------
const HARNESS_INIT = () => {
  // @ts-expect-error - test-only globals
  if (window.__zfbHarness) return;
  // @ts-expect-error
  window.__zfbHarness = true;

  // Unique per REAL document. An SPA nav preserves it (same JS context); a
  // full browser reload mints a new one — the signal a form submit must NOT
  // have caused a real page load.
  const uuid =
    typeof crypto !== "undefined" && crypto.randomUUID
      ? crypto.randomUUID()
      : String(Math.random());
  // @ts-expect-error
  window.__docId = uuid;

  const NAMES = [
    "zfb:before-preparation",
    "zfb:after-preparation",
    "zfb:before-swap",
    "zfb:after-swap",
    "zfb:page-load",
    "zfb:navigation-aborted",
  ];
  const rec = { events: [], seq: 0 };
  // @ts-expect-error
  window.__zfb = rec;
  for (const name of NAMES) {
    document.addEventListener(name, (ev) => {
      rec.events.push({ name, seq: ++rec.seq });
      void ev;
    });
  }
};

test.beforeEach(async ({ page }) => {
  await page.addInitScript(HARNESS_INIT);
});

// ---------------------------------------------------------------------------
// Event-keyed wait helpers (no bare timeouts, no networkidle).
// ---------------------------------------------------------------------------

/** Current lifecycle sequence counter — capture BEFORE triggering a navigation. */
async function seqNow(page) {
  return page.evaluate(() => /** @type {any} */ (window).__zfb?.seq ?? 0);
}

/** Wait until a lifecycle event `name` has been recorded with seq > `after`. */
async function waitForZfbEvent(page, name, after, timeout = 8000) {
  await page.waitForFunction(
    ({ name, after }) =>
      /** @type {any} */ (window.__zfb?.events ?? []).some((e) => e.name === name && e.seq > after),
    { name, after },
    { timeout },
  );
}

/**
 * Wait until a navigation is fully finished AND quiescent — see
 * router.chromium.spec.mjs's `waitForNavDone` for the full rationale (the
 * `data-zfb-transition` absence check keeps the next step from starting
 * mid-animation, one of the 5 deflaking root causes).
 */
async function waitForNavDone(page, after, timeout = 8000) {
  await waitForZfbEvent(page, "zfb:page-load", after, timeout);
  await page.waitForFunction(
    () => !document.documentElement.hasAttribute("data-zfb-transition"),
    undefined,
    { timeout },
  );
}

/** Wait for the initial full-page load's `zfb:page-load` (router booted). */
async function waitForInitialLoad(page) {
  await page.waitForFunction(() =>
    /** @type {any} */ (window.__zfb?.events ?? []).some((e) => e.name === "zfb:page-load"),
  );
}

// ===========================================================================
// Spec — submitting a GET (search-style) form is a real SPA navigation with
// the fields serialized into the query string, not a full page load.
// ===========================================================================
test("submitting a GET search form performs a SPA navigation with the serialized query", async ({
  page,
}) => {
  await page.goto("/search.html");
  await waitForInitialLoad(page);
  await expect(page.locator("h1")).toHaveText("Search");

  const docIdBefore = await page.evaluate(() => /** @type {any} */ (window).__docId);

  await page.fill("#q", "hello world");

  const s = await seqNow(page);
  await page.click("#search-submit");
  await waitForNavDone(page, s);

  // Content swapped to the results page ...
  await expect(page.locator("h1")).toHaveText("Search Results");
  // ... the URL carries both the typed field and the form's own hidden field,
  // serialized via URLSearchParams (spaces become "+", not "%20") ...
  const url = new URL(page.url());
  expect(url.pathname).toBe("/search-results.html");
  expect(url.searchParams.get("q")).toBe("hello world");
  expect(url.searchParams.get("category")).toBe("docs");
  // ... and it was a client swap, NOT a document reload: the JS context (and
  // its per-document id) survived. A full reload would have minted a new
  // __docId — the same proof router.chromium.spec.mjs's Spec 1 uses for link
  // navigation.
  const docIdAfter = await page.evaluate(() => /** @type {any} */ (window).__docId);
  expect(docIdAfter).toBe(docIdBefore);

  // The full documented lifecycle fired, in order, for this navigation (a
  // form submit goes through the exact same transition() pipeline as a link
  // click) — same style as router.chromium.spec.mjs's Spec 2.
  const names = await page.evaluate(
    (after) =>
      /** @type {any} */ (window.__zfb.events).filter((e) => e.seq > after).map((e) => e.name),
    s,
  );
  const expectedOrder = [
    "zfb:before-preparation",
    "zfb:after-preparation",
    "zfb:before-swap",
    "zfb:after-swap",
    "zfb:page-load",
  ];
  const positions = expectedOrder.map((n) => names.indexOf(n));
  expect(
    positions.every((p) => p >= 0),
    `missing events; saw ${names.join(",")}`,
  ).toBe(true);
  for (let i = 1; i < positions.length; i++) {
    expect(positions[i]).toBeGreaterThan(positions[i - 1]);
  }
});

// Absent and empty submitter targets differ; Chromium applies base fallback to empty targets.
const TARGET_CASES = [
  { name: "form blank", target: "_blank", context: "popup" },
  { name: "submitter blank", target: "_self", override: "_BLANK", context: "popup" },
  { name: "base blank", base: "_blank", context: "popup" },
  { name: "named frame", target: "results", context: "frame" },
  { name: "submitter named frame", override: "results", context: "frame" },
  { name: "self override", target: "_blank", override: "_SeLf", context: "self" },
  { name: "empty form target", target: "", base: "_blank", context: "popup" },
  {
    name: "empty submitter target",
    target: "_blank",
    override: "",
    base: "_blank",
    context: "popup",
  },
  { name: "empty override without base", target: "_blank", override: "", context: "self" },
  { name: "base self", base: "_SELF", context: "self" },
  { name: "first base target wins", base: "", secondBase: "_blank", context: "self" },
];

for (const config of TARGET_CASES) {
  for (const native of [false, true]) {
    test(`${config.name}: ${native ? "native oracle" : "router"}`, async ({ page, context }) => {
      const ambiguous =
        config.name === "empty form target" || config.name === "empty submitter target";
      let destination = config.context;
      const pageErrors = [];
      const trackErrors = (candidate) =>
        candidate.on("pageerror", (error) => pageErrors.push(String(error)));
      trackErrors(page);
      context.on("page", trackErrors);
      if (ambiguous) {
        const oracle = await context.newPage();
        await oracle.goto("/search.html");
        await oracle.waitForFunction(() => history.state?.index !== undefined);
        await oracle.evaluate((config) => {
          const form = document.querySelector("form");
          form.setAttribute("data-zfb-reload", "");
          const shadow = document.createElement("input");
          shadow.name = "ownerDocument";
          shadow.value = "shadow";
          form.append(shadow);
          const button = form.querySelector("button");
          button.name = "submitter";
          button.value = "search";
          if (config.target !== undefined) form.setAttribute("target", config.target);
          if (config.override !== undefined)
            form.querySelector("button").setAttribute("formtarget", config.override);
          const base = document.createElement("base");
          base.target = config.base;
          document.head.append(base);
        }, config);
        await oracle.fill("#q", "context query");
        await oracle.evaluate(() => fetch("/__form-requests"));
        const observed = Promise.race([
          oracle.waitForURL(/search-results/).then(() => "self"),
          oracle.waitForEvent("popup").then(async (popup) => {
            await expect(popup.locator("h1")).toHaveText("Search Results");
            return "popup";
          }),
        ]);
        await oracle.click("#search-submit");
        destination = await observed;
        const oracleRequests = await oracle.evaluate(() =>
          fetch("/__form-requests").then((r) => r.json()),
        );
        expect(oracleRequests).toHaveLength(1);
        expect(oracleRequests[0].method).toBe("GET");
        expect(oracleRequests[0].destination).toBe("document");
        const oracleQuery = new URLSearchParams(oracleRequests[0].search);
        expect(oracleQuery.get("q")).toBe("context query");
        expect(oracleQuery.get("submitter")).toBe("search");
        for (const candidate of context.pages())
          if (candidate !== page && candidate !== oracle) await candidate.close();
        await oracle.close();
      }
      await page.goto("/search.html");
      await waitForInitialLoad(page);
      await page.evaluate(
        ({ config, native }) => {
          const form = document.querySelector("form");
          const button = form.querySelector("button");
          if (native) form.setAttribute("data-zfb-reload", "");
          if (config.target !== undefined) form.setAttribute("target", config.target);
          if (config.override !== undefined) button.setAttribute("formtarget", config.override);
          const shadow = document.createElement("input");
          shadow.name = "ownerDocument";
          shadow.value = "shadow";
          form.append(shadow);
          button.name = "submitter";
          button.value = "search";
          for (const target of [config.base, config.secondBase]) {
            if (target === undefined) continue;
            const base = document.createElement("base");
            base.target = target;
            document.head.append(base);
          }
          if (config.context === "frame") {
            const frame = document.createElement("iframe");
            frame.name = "results";
            document.body.append(frame);
          }
          window.__formDataEvents = 0;
          form.addEventListener("formdata", () => window.__formDataEvents++);
        },
        { config, native },
      );
      await page.fill("#q", "context query");
      const before = await page.evaluate(() => ({
        id: window.__docId,
        state: history.state,
        seq: window.__zfb.seq,
        url: location.href,
      }));
      if (ambiguous) {
        await page.evaluate(() => fetch("/__form-requests"));
        await page.evaluate(() => {
          sessionStorage.removeItem("target-preparation");
          document.addEventListener("zfb:before-preparation", () =>
            sessionStorage.setItem("target-preparation", "yes"),
          );
        });
      }
      const requestPromise = page.context().waitForEvent("request", {
        predicate: (r) => new URL(r.url()).pathname === "/search-results.html",
      });
      const popupPromise = destination === "popup" ? page.waitForEvent("popup") : null;
      await page.click("#search-submit");
      const request = await requestPromise;
      expect(request.method()).toBe("GET");
      const submitted = new URL(request.url());
      expect(submitted.searchParams.get("q")).toBe("context query");
      expect(submitted.searchParams.get("submitter")).toBe("search");
      if (destination === "self") {
        if (native || ambiguous) await page.waitForURL(/search-results/);
        else await waitForNavDone(page, before.seq);
        await expect(page.locator("h1")).toHaveText("Search Results");
        const id = await page.evaluate(() => window.__docId);
        if (native || ambiguous) expect(id).not.toBe(before.id);
        else expect(id).toBe(before.id);
      } else {
        if (popupPromise) {
          const popup = await popupPromise;
          await expect(popup.locator("h1")).toHaveText("Search Results");
          await popup.close();
        } else {
          await expect(page.frameLocator('iframe[name="results"]').locator("h1")).toHaveText(
            "Search Results",
          );
        }
        expect(
          await page.evaluate(() => ({
            id: window.__docId,
            state: history.state,
            seq: window.__zfb.seq,
            url: location.href,
          })),
        ).toEqual(before);
        await expect(page.locator("h1")).toHaveText("Search");
        expect(await page.evaluate(() => window.__formDataEvents)).toBe(1);
      }
      if (ambiguous) {
        expect(await page.evaluate(() => sessionStorage.getItem("target-preparation"))).toBeNull();
        const requests = await page.evaluate(() => fetch("/__form-requests").then((r) => r.json()));
        expect(requests).toHaveLength(1);
        expect(requests[0].method).toBe("GET");
        expect(requests[0].destination).toBe("document");
        expect(pageErrors).toEqual([]);
        const query = new URLSearchParams(requests[0].search);
        expect(query.get("q")).toBe("context query");
        expect(query.get("submitter")).toBe("search");
      }
    });
  }
}

for (const method of ["GET", "gEt", "", "invalid", "POST", "pOsT"]) {
  test(`submitter formmethod=${JSON.stringify(method)} uses the native method`, async ({
    page,
  }) => {
    await page.goto("/search.html");
    await waitForInitialLoad(page);
    await page.evaluate((method) => {
      document.querySelector("form").method = "post";
      document.querySelector("button").setAttribute("formmethod", method);
    }, method);
    await page.fill("#q", "method query");
    const before = await page.evaluate(() => window.__docId);
    const seq = await seqNow(page);
    const requestPromise = page.waitForRequest(
      (r) => new URL(r.url()).pathname === "/search-results.html",
    );
    await page.click("#search-submit");
    const request = await requestPromise;
    const post = method.toLowerCase() === "post";
    expect(request.method()).toBe(post ? "POST" : "GET");
    if (!post) expect(new URL(request.url()).searchParams.get("q")).toBe("method query");
    else expect(request.postData()).toContain("method query");
    await waitForNavDone(page, seq);
    expect(await page.evaluate(() => window.__docId)).toBe(before);
  });
}

for (const method of ["DIALOG", "dIaLoG"]) {
  for (const native of [true, false]) {
    test(`${method} malformed action closes a dialog: ${native ? "native oracle" : "router"}`, async ({
      page,
    }) => {
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await page.goto("/search.html");
      await waitForInitialLoad(page);
      await page.evaluate(
        ({ method, native }) => {
          const dialog = document.createElement("dialog");
          const form = document.querySelector("form");
          form.setAttribute("action", "http://[");
          form.querySelector("button").setAttribute("formaction", "http://[");
          if (native) form.setAttribute("data-zfb-reload", "");
          document.body.append(dialog);
          dialog.append(form);
          form.querySelector("button").setAttribute("formmethod", method);
          window.__formDataEvents = 0;
          form.addEventListener("formdata", () => window.__formDataEvents++);
          dialog.showModal();
        },
        { method, native },
      );
      const requests = [];
      page.on("request", (r) => requests.push(r.url()));
      const before = await page.evaluate(() => ({
        id: window.__docId,
        seq: window.__zfb.seq,
        url: location.href,
      }));
      await page.click("#search-submit");
      await expect(page.locator("dialog")).not.toHaveAttribute("open", "");
      expect(
        await page.evaluate(() => ({
          id: window.__docId,
          seq: window.__zfb.seq,
          url: location.href,
        })),
      ).toEqual(before);
      expect(await page.evaluate(() => window.__formDataEvents)).toBe(0);
      expect(requests).toEqual([]);
      expect(errors).toEqual([]);
    });
  }
}

for (const target of ["_PaReNt", "_TOP"]) {
  test(`${target} submits from a frame into its ancestor natively`, async ({ page }) => {
    await page.goto("/search.html");
    await waitForInitialLoad(page);
    const before = await page.evaluate(() => window.__docId);
    await page.evaluate(() => {
      const frame = document.createElement("iframe");
      frame.name = "child";
      frame.src = "/search.html";
      document.body.append(frame);
    });
    const frame = page.frameLocator('iframe[name="child"]');
    await frame.locator("#q").fill("ancestor query");
    await waitForInitialLoad(page.frame({ name: "child" }));
    await frame
      .locator("#search-form")
      .evaluate((form, target) => form.setAttribute("target", target), target);
    const request = page.waitForRequest(
      (r) => new URL(r.url()).pathname === "/search-results.html",
    );
    await frame.locator("#search-submit").click();
    expect((await request).isNavigationRequest()).toBe(true);
    await page.waitForURL(/search-results/);
    await expect(page.locator("h1")).toHaveText("Search Results");
    expect(new URL(page.url()).searchParams.get("q")).toBe("ancestor query");
    expect(await page.evaluate(() => window.__docId)).not.toBe(before);
  });
}

const ACTION_CASES = [
  {
    name: "relative submitter",
    action: "/fallback",
    override: "search-results.html?old=1#result",
    path: "/base/search-results.html",
    query: "?old=1",
    hash: "#result",
  },
  {
    name: "cross-origin base",
    action: "/fallback",
    override: "search-results.html",
    path: "/base/search-results.html",
    cross: true,
  },
  {
    name: "shadowed relative form action",
    action: "search-results.html",
    path: "/base/search-results.html",
  },
  { name: "missing action", path: "/search.html", query: "?existing=1", hash: "#initial" },
  {
    name: "empty action",
    action: "",
    path: "/search.html",
    query: "?existing=1",
    hash: "#initial",
  },
  {
    name: "empty submitter override",
    action: "/search-results.html",
    override: "",
    path: "/search.html",
    query: "?existing=1",
    hash: "#initial",
  },
  {
    name: "absolute action",
    action: "http://localhost:4322/search-results.html?old=1#result",
    path: "/search-results.html",
    query: "?old=1",
    hash: "#result",
  },
];

for (const config of ACTION_CASES) {
  for (const method of ["get", "post"]) {
    for (const native of [true, false]) {
      test(`action ${config.name} ${method}: ${native ? "native oracle" : "router"}`, async ({
        page,
      }) => {
        const errors = [];
        page.on("pageerror", (error) => errors.push(error.message));
        await page.goto("/search.html?existing=1#initial");
        await waitForInitialLoad(page);
        await page.evaluate(
          ({ config, method, native }) => {
            const form = document.querySelector("form");
            form.removeAttribute("action");
            if (config.action !== undefined) form.setAttribute("action", config.action);
            form.setAttribute("method", method);
            // Keep the oracle's encoding identical to the router's existing default serializer.
            form.setAttribute("enctype", "multipart/form-data");
            if (native) form.setAttribute("data-zfb-reload", "");
            const button = form.querySelector("button");
            if (config.override !== undefined) button.setAttribute("formaction", config.override);
            for (const name of ["action", "ownerDocument", "baseURI"]) {
              const input = document.createElement("input");
              input.name = name;
              input.value = "shadow";
              form.append(input);
            }
            const base = document.createElement("base");
            base.href = config.cross ? "http://127.0.0.1:4322/base/" : "/base/";
            document.head.append(base);
          },
          { config, method, native },
        );
        await page.fill("#q", "action query");
        const before = await page.evaluate(() => ({ id: window.__docId, seq: window.__zfb.seq }));
        const requests = [];
        page.on("request", (request) => {
          if (["document", "fetch"].includes(request.resourceType())) requests.push(request);
        });
        const requestPromise = page.waitForRequest((r) =>
          ["document", "fetch"].includes(r.resourceType()),
        );
        await page.click("#search-submit");
        const request = await requestPromise;
        const destination = new URL(request.url());
        expect(destination.origin).toBe(
          config.cross ? "http://127.0.0.1:4322" : "http://localhost:4322",
        );
        expect(destination.pathname).toBe(config.path);
        expect(request.method()).toBe(method.toUpperCase());
        expect(request.isNavigationRequest()).toBe(native || !!config.cross);
        if (method === "get") {
          expect(destination.searchParams.get("q")).toBe("action query");
          expect(destination.searchParams.has("old")).toBe(false);
          expect(destination.searchParams.has("existing")).toBe(false);
        } else {
          expect(destination.search).toBe(config.query ?? "");
          expect(request.postData()).toContain("action query");
          expect(request.postData()).toContain('name="action"');
        }
        if (native || config.cross) {
          await page.waitForURL(
            (url) =>
              url.pathname === config.path &&
              url.origin === destination.origin &&
              (method !== "get" || url.searchParams.get("q") === "action query"),
            { waitUntil: "load" },
          );
          await page.waitForFunction((id) => window.__docId !== id, before.id);
        } else {
          await waitForNavDone(page, before.seq);
          expect(await page.evaluate(() => window.__docId)).toBe(before.id);
        }
        expect(new URL(page.url()).hash).toBe(config.hash ?? "");
        expect(requests).toHaveLength(1);
        expect(errors).toEqual([]);
      });
    }
  }
}
