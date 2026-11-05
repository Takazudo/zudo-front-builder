// Level 4: POST encoding follows the submitter's real form owner, including a
// control created in another realm and adopted into this document. `instanceof
// HTMLElement` is realm-sensitive; the native `.form` IDL value is not.
// Closest-form is only the fallback when the element has no form-owner API.
//
// Oracle for real submissions: the same DOM in a document whose router module
// is stubbed, so the browser alone owns the request. Imperative null-owner
// cases have no native submit (the button's form owner is null) and call
// navigate() with an explicit FormData instead.
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

const HARNESS_INIT = () => {
  // @ts-expect-error - test-only globals
  if (window.__zfbHarness) return;
  // @ts-expect-error
  window.__zfbHarness = true;
  const docId = crypto.randomUUID();
  // @ts-expect-error
  window.__docId = docId;
  for (const name of [
    "zfb:before-preparation",
    "zfb:after-preparation",
    "zfb:before-swap",
    "zfb:after-swap",
    "zfb:page-load",
    "zfb:navigation-aborted",
  ]) {
    document.addEventListener(name, () =>
      // @ts-expect-error - exposed by the context
      window.__zfbLog({ name, docId, url: location.href }),
    );
  }
};

/**
 * @param {import("@playwright/test").Browser} browser
 * @param {Record<string, unknown>} config
 * @param {{ routerless: boolean, mutate?: boolean, activate: "submit" | "navigate" }} mode
 */
async function runCase(browser, config, mode) {
  const context = await browser.newContext();
  try {
    /** @type {{ name: string, docId: string, url: string }[]} */
    const log = [];
    await context.exposeFunction("__zfbLog", (entry) => log.push(entry));
    if (mode.routerless) {
      await context.route("**/dist/client-router/router.js", (route) =>
        route.fulfill({
          contentType: "application/javascript; charset=utf-8",
          body: "export function init() {}",
        }),
      );
    }
    const page = await context.newPage();
    await page.addInitScript(HARNESS_INIT);
    /** @type {string[]} */
    const errors = [];
    page.on("pageerror", (err) => errors.push(String(err)));
    /** @type {{ method: string, resourceType: string, contentType: string, body: string, pathname: string }[]} */
    const requests = [];
    context.on("request", (req) => {
      const url = new URL(req.url());
      if (url.pathname !== DESTINATION) return;
      requests.push({
        method: req.method(),
        resourceType: req.resourceType(),
        contentType: req.headers()["content-type"] ?? "",
        body: req.postDataBuffer()?.toString("latin1") ?? "",
        pathname: url.pathname,
      });
    });

    await page.goto("/search.html");
    await expect(page.locator("h1")).toHaveText("Search");
    const docId = await page.evaluate(() => /** @type {any} */ (window).__docId);
    if (!mode.routerless) {
      await expect.poll(() => log.filter((entry) => entry.name === "zfb:page-load").length).toBe(1);
    }

    await page.evaluate(installFixture, config);
    const probe = await page.evaluate(readProbe);
    // Distinguishing conditions are asserted before the single activation.
    expect(probe).toEqual(config.probe);
    await page.evaluate(() => fetch("/__form-posts"));
    const startHref = page.url();
    const logBefore = log.length;
    if (!mode.routerless) {
      await page.evaluate((mutate) => {
        document.addEventListener("zfb:before-preparation", (event) => {
          const submitter = document.getElementById("submitter");
          const preparation = /** @type {any} */ (event);
          // @ts-expect-error - test-only global
          window.__source = {
            same: preparation.sourceElement === submitter,
            marker: preparation.sourceElement?.getAttribute?.("data-marker") ?? null,
          };
          if (mutate && preparation.formData) preparation.formData.append("hooked", "after");
        });
      }, mode.mutate === true);
    }

    if (mode.activate === "submit") {
      await page.evaluate(() => {
        const owner = /** @type {HTMLFormElement} */ (document.getElementById("owner"));
        const submitter = /** @type {HTMLElement} */ (document.getElementById("submitter"));
        owner.requestSubmit(submitter);
      });
    } else {
      await page.evaluate(async () => {
        const { navigate } = await import("/dist/client-router/router.js");
        const submitter = document.getElementById("submitter");
        const body = new FormData();
        body.append("k", "v");
        body.append("note", new File(["hello-file"], "note.txt", { type: "text/plain" }));
        await navigate("/search-results.html", { formData: body, sourceElement: submitter });
      });
    }

    await page.waitForURL((url) => url.href !== startHref && url.pathname === DESTINATION);
    await expect(page.locator("h1")).toHaveText("Search Results");
    const sameDocument = (await page.evaluate(() => /** @type {any} */ (window).__docId)) === docId;
    if (sameDocument) {
      await expect
        .poll(() => log.slice(logBefore).filter((entry) => entry.name === "zfb:page-load").length)
        .toBe(1);
      await page.waitForFunction(
        () => !document.documentElement.hasAttribute("data-zfb-transition"),
      );
    }
    const serverPosts = await page.evaluate(async () => {
      const response = await fetch("/__form-posts");
      return response.json();
    });
    const source = mode.routerless
      ? null
      : await page.evaluate(() => /** @type {any} */ (window).__source ?? null);
    return {
      probe,
      errors,
      sameDocument,
      requests,
      serverPosts,
      source,
      lifecycle: log
        .slice(logBefore)
        .filter((entry) => entry.docId === docId)
        .map((entry) => entry.name),
    };
  } finally {
    await context.close();
  }
}

/**
 * @param {Record<string, any>} config
 */
function installFixture(config) {
  const owner = document.createElement("form");
  owner.id = "owner";
  owner.setAttribute("action", "/search-results.html");
  owner.setAttribute("method", "post");
  if (config.ownerEnctype !== undefined) owner.setAttribute("enctype", config.ownerEnctype);
  for (const field of config.fields ?? [{ name: "owned", value: "yes" }]) {
    const input = document.createElement("input");
    input.setAttribute("name", field.name);
    input.setAttribute("value", field.value);
    owner.append(input);
  }
  if (config.file) {
    const file = document.createElement("input");
    file.type = "file";
    file.name = "note";
    const transfer = new DataTransfer();
    transfer.items.add(new File(["hello-file"], "note.txt", { type: "text/plain" }));
    file.files = transfer.files;
    owner.append(file);
  }

  /** @type {HTMLFormElement | null} */
  let decoy = null;
  if (config.decoyEnctype !== undefined) {
    decoy = document.createElement("form");
    decoy.id = "decoy";
    decoy.setAttribute("method", "post");
    decoy.setAttribute("enctype", config.decoyEnctype);
    const noise = document.createElement("input");
    noise.setAttribute("name", "decoy");
    noise.setAttribute("value", "nope");
    decoy.append(noise);
  }

  const tag = config.submitterTag ?? "button";
  let submitter;
  if (config.realm === "foreign") {
    const frame = document.createElement("iframe");
    document.body.append(frame);
    const foreignDocument = frame.contentDocument;
    if (!foreignDocument) throw new Error("iframe has no document");
    const created = foreignDocument.createElement(tag);
    submitter = document.adoptNode(created);
    if (submitter instanceof HTMLElement) throw new Error("adopted submitter is same-realm");
  } else {
    submitter = document.createElement(tag);
    if (!(submitter instanceof HTMLElement)) throw new Error("same-realm submitter is foreign");
  }
  submitter.id = "submitter";
  submitter.setAttribute("data-marker", "origin");
  submitter.setAttribute("type", "submit");
  submitter.setAttribute("name", "go");
  submitter.setAttribute("value", "send");
  if (config.formenctype !== undefined) submitter.setAttribute("formenctype", config.formenctype);
  if (config.placement === "null-owner") submitter.setAttribute("form", "missing");
  else if (config.placement !== "in-owner") submitter.setAttribute("form", "owner");

  if (config.placement === "decoy") {
    if (!decoy) throw new Error("decoy placement requires a decoy form");
    decoy.append(submitter);
    document.body.append(decoy, owner);
  } else if (config.placement === "external") {
    document.body.append(owner, submitter);
  } else {
    owner.append(submitter);
    document.body.append(owner);
    if (decoy) document.body.append(decoy);
  }
}

function readProbe() {
  const submitter = /** @type {any} */ (document.getElementById("submitter"));
  const closest = submitter.closest("form");
  return {
    instanceofHTMLElement: submitter instanceof HTMLElement,
    formId: submitter.form?.id ?? null,
    closestId: closest?.id ?? null,
  };
}

/**
 * @param {string} contentType
 * @param {string} body
 */
function multipartEntries(contentType, body) {
  const match = /boundary=(?:"([^"]+)"|([^;\s]+))/i.exec(contentType);
  const boundary = match?.[1] ?? match?.[2];
  if (!boundary) throw new Error(`missing multipart boundary in ${contentType}`);
  if (!body.includes(`--${boundary}`)) throw new Error("boundary missing from body");
  /** @type {{ name: string, filename: string | null, content: string }[]} */
  const entries = [];
  for (const chunk of body.split(`--${boundary}`)) {
    if (chunk === "" || chunk.startsWith("--")) continue;
    const normalized = chunk.replace(/^\r\n/, "").replace(/\r\n$/, "");
    const splitAt = normalized.indexOf("\r\n\r\n");
    if (splitAt < 0) continue;
    const headers = normalized.slice(0, splitAt);
    const name = /name="([^"]*)"/.exec(headers)?.[1];
    if (!name) continue;
    entries.push({
      name,
      filename: /filename="([^"]*)"/.exec(headers)?.[1] ?? null,
      content: normalized.slice(splitAt + 4),
    });
  }
  return entries;
}

function mediaType(contentType) {
  return contentType.split(";")[0]?.trim().toLowerCase() ?? "";
}

/**
 * @param {{ contentType: string, body: string, serverBody: string, method: string }} native
 * @param {{ contentType: string, body: string, serverBody: string, method: string }} actual
 */
function expectSamePost(native, actual) {
  const nativeMedia = mediaType(native.contentType);
  const actualMedia = mediaType(actual.contentType);
  const detail = `native ${native.contentType} ${preview(native.serverBody)} | actual ${actual.contentType} ${preview(actual.serverBody)}`;
  expect(actual.method, detail).toBe("POST");
  expect(actual.method, detail).toBe(native.method);
  expect(actualMedia, detail).toBe(nativeMedia);
  if (nativeMedia === "application/x-www-form-urlencoded") {
    expect(actual.body, detail).toBe(native.body);
    expect(actual.serverBody, detail).toBe(native.serverBody);
    expect(actual.serverBody, detail).toBe(actual.body);
    return;
  }
  expect(native.contentType, detail).toMatch(/boundary=/i);
  expect(actual.contentType, detail).toMatch(/boundary=/i);
  expect(multipartEntries(actual.contentType, actual.serverBody), detail).toEqual(
    multipartEntries(native.contentType, native.serverBody),
  );
}

function preview(body) {
  return body.length > 280 ? `${body.slice(0, 280)}…(${body.length})` : body;
}

/**
 * @param {{ requests: { method: string, contentType: string, body: string }[], serverPosts: { contentType: string, body: string, pathname: string }[] }} result
 */
function posted(result) {
  expect(result.serverPosts).toHaveLength(1);
  expect(result.serverPosts[0]?.pathname).toBe(DESTINATION);
  return {
    method: result.requests[0]?.method ?? "",
    contentType: result.requests[0]?.contentType ?? "",
    body: result.requests[0]?.body ?? "",
    serverBody: result.serverPosts[0]?.body ?? "",
  };
}

/**
 * @param {import("@playwright/test").Browser} browser
 * @param {Record<string, unknown>} config
 * @param {{ mutate?: boolean }} [opts]
 */
async function expectOwnedSubmission(browser, config, opts = {}) {
  const native = await runCase(browser, config, { routerless: true, activate: "submit" });
  expect(native.probe).toEqual(config.probe);
  expect(native.errors).toEqual([]);
  expect(native.sameDocument).toBe(false);
  expect(native.requests).toHaveLength(1);
  expect(native.requests[0]?.resourceType).toBe("document");
  expect(native.lifecycle).toEqual([]);

  const actual = await runCase(browser, config, {
    routerless: false,
    activate: "submit",
    mutate: opts.mutate === true,
  });
  expect(actual.probe).toEqual(config.probe);
  expect(actual.errors).toEqual([]);
  expect(actual.sameDocument).toBe(true);
  expect(actual.requests).toHaveLength(1);
  expect(actual.requests[0]?.resourceType).toBe("fetch");
  expect(actual.lifecycle).toEqual(LIFECYCLE);
  expect(actual.source).toEqual({ same: true, marker: "origin" });
  const nativePost = posted(native);
  const actualPost = posted(actual);
  if (opts.mutate) {
    expect(mediaType(actualPost.contentType)).toBe("application/x-www-form-urlencoded");
    expect(actualPost.body).toBe(`${nativePost.body}&hooked=after`);
    expect(actualPost.serverBody).toBe(actualPost.body);
    return;
  }
  if (config.decoyEnctype !== undefined) {
    expect(actualPost.serverBody).not.toContain("decoy");
    expect(nativePost.serverBody).not.toContain("decoy");
  }
  expectSamePost(nativePost, actualPost);
}

test("adopted external button uses the owner's URL-encoded bytes", async ({ browser }) => {
  await expectOwnedSubmission(browser, {
    realm: "foreign",
    placement: "external",
    ownerEnctype: "application/x-www-form-urlencoded",
    fields: [{ name: "owned", value: "café&=雪" }],
    probe: { instanceofHTMLElement: false, formId: "owner", closestId: null },
  });
});

test("adopted external input[type=submit] uses the owner's URL encoding", async ({ browser }) => {
  await expectOwnedSubmission(browser, {
    realm: "foreign",
    placement: "external",
    submitterTag: "input",
    ownerEnctype: "application/x-www-form-urlencoded",
    probe: { instanceofHTMLElement: false, formId: "owner", closestId: null },
  });
});

test.describe("adopted submitter formenctype", () => {
  const rows = [
    {
      name: "URL-encoded override beats a multipart owner and decoy",
      ownerEnctype: "multipart/form-data",
      decoyEnctype: "multipart/form-data",
      formenctype: "application/x-www-form-urlencoded",
    },
    {
      name: "multipart override beats a URL-encoded owner and decoy",
      ownerEnctype: "application/x-www-form-urlencoded",
      decoyEnctype: "application/x-www-form-urlencoded",
      formenctype: "multipart/form-data",
      file: true,
    },
    {
      name: "present-empty formenctype selects URL-encoded",
      ownerEnctype: "multipart/form-data",
      decoyEnctype: "multipart/form-data",
      formenctype: "",
    },
    {
      name: "invalid formenctype selects URL-encoded",
      ownerEnctype: "multipart/form-data",
      decoyEnctype: "multipart/form-data",
      formenctype: "nope",
    },
  ];
  for (const row of rows) {
    test(row.name, async ({ browser }) => {
      await expectOwnedSubmission(browser, {
        realm: "foreign",
        placement: "decoy",
        submitterTag: "button",
        ownerEnctype: row.ownerEnctype,
        decoyEnctype: row.decoyEnctype,
        formenctype: row.formenctype,
        file: row.file === true,
        probe: { instanceofHTMLElement: false, formId: "owner", closestId: "decoy" },
      });
    });
  }
});

test("adopted submitter inside a multipart decoy posts the URL-encoded owner", async ({
  browser,
}) => {
  await expectOwnedSubmission(browser, {
    realm: "foreign",
    placement: "decoy",
    ownerEnctype: "application/x-www-form-urlencoded",
    decoyEnctype: "multipart/form-data",
    probe: { instanceofHTMLElement: false, formId: "owner", closestId: "decoy" },
  });
});

test("adopted submitter inside a URL-encoded decoy posts the multipart owner", async ({
  browser,
}) => {
  await expectOwnedSubmission(browser, {
    realm: "foreign",
    placement: "decoy",
    ownerEnctype: "multipart/form-data",
    decoyEnctype: "application/x-www-form-urlencoded",
    file: true,
    probe: { instanceofHTMLElement: false, formId: "owner", closestId: "decoy" },
  });
});

test("same-realm external submitter keeps the owner's URL encoding", async ({ browser }) => {
  await expectOwnedSubmission(browser, {
    realm: "same",
    placement: "external",
    ownerEnctype: "application/x-www-form-urlencoded",
    probe: { instanceofHTMLElement: true, formId: "owner", closestId: null },
  });
});

test("adopted submitter inside its owner keeps that owner's URL encoding", async ({ browser }) => {
  await expectOwnedSubmission(browser, {
    realm: "foreign",
    placement: "in-owner",
    ownerEnctype: "application/x-www-form-urlencoded",
    probe: { instanceofHTMLElement: false, formId: "owner", closestId: "owner" },
  });
});

test("preparation hook FormData is the body, and the submitter stays the source", async ({
  browser,
}) => {
  await expectOwnedSubmission(
    browser,
    {
      realm: "foreign",
      placement: "external",
      ownerEnctype: "application/x-www-form-urlencoded",
      probe: { instanceofHTMLElement: false, formId: "owner", closestId: null },
    },
    { mutate: true },
  );
});

test.describe("authoritative null form owner stays multipart", () => {
  for (const realm of ["same", "foreign"]) {
    test(`${realm}-realm button with form="missing" does not inherit the ancestor encoding`, async ({
      browser,
    }) => {
      const config = {
        realm,
        placement: "null-owner",
        ownerEnctype: "application/x-www-form-urlencoded",
        probe: {
          instanceofHTMLElement: realm === "same",
          formId: null,
          closestId: "owner",
        },
      };
      const actual = await runCase(browser, config, { routerless: false, activate: "navigate" });
      expect(actual.probe).toEqual(config.probe);
      expect(actual.errors).toEqual([]);
      expect(actual.sameDocument).toBe(true);
      expect(actual.requests).toHaveLength(1);
      expect(actual.requests[0]?.method).toBe("POST");
      expect(actual.requests[0]?.resourceType).toBe("fetch");
      expect(actual.lifecycle).toEqual(LIFECYCLE);
      expect(actual.source).toEqual({ same: true, marker: "origin" });
      const post = posted(actual);
      expect(mediaType(post.contentType)).toBe("multipart/form-data");
      expect(post.contentType).toMatch(/boundary=/i);
      const entries = multipartEntries(post.contentType, post.serverBody);
      expect(entries).toEqual([
        { name: "k", filename: null, content: "v" },
        { name: "note", filename: "note.txt", content: "hello-file" },
      ]);
    });
  }
});
