// Level 4: outgoing form request content-type, payload, and submission count.
// Body-object class is not the oracle.
import { test, expect } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const Original = window.FormData;
    function WrappedFormData(...args) {
      console.info("zfb-formdata-constructed");
      return new Original(...args);
    }
    WrappedFormData.prototype = Original.prototype;
    window.FormData = WrappedFormData;
  });
});

async function boot(page) {
  await page.goto("/search.html");
  await page.waitForFunction(() => history.state?.index !== undefined);
  await expect(page.locator("h1")).toHaveText("Search");
}

function trackAction(page, posts) {
  page.on("request", (req) => {
    const url = new URL(req.url());
    if (url.pathname !== "/search-results.html") return;
    posts.push({
      method: req.method(),
      resourceType: req.resourceType(),
      contentType: req.headers()["content-type"] ?? "",
      body: req.postDataBuffer()?.toString("latin1") ?? "",
      search: url.search,
    });
  });
}

async function installForm(page, config) {
  await page.evaluate((config) => {
    window.__encodingSentinel = "retained";
    const form = document.createElement("form");
    form.id = "encoding-form";
    form.action = "/search-results.html";
    form.method = config.method;
    if (config.enctype !== undefined) form.setAttribute("enctype", config.enctype);
    for (const field of config.fields ?? []) {
      const input = document.createElement("input");
      input.name = field.name;
      input.value = field.value;
      form.append(input);
    }
    if (config.file) {
      const file = document.createElement("input");
      file.type = "file";
      file.name = config.file.name;
      file.id = "encoding-file";
      form.append(file);
    }
    const submitter = document.createElement(config.submitterTag ?? "button");
    submitter.id = "encoding-submit";
    if (config.submitterTag === "input") submitter.type = "submit";
    else submitter.setAttribute("type", "submit");
    if (config.submitterName) {
      submitter.name = config.submitterName;
      submitter.value = config.submitterValue ?? "";
    }
    if (config.formenctype !== undefined) submitter.setAttribute("formenctype", config.formenctype);
    if (config.submitterOutside) {
      form.id = "encoding-form";
      submitter.setAttribute("form", "encoding-form");
      document.body.append(form, submitter);
    } else {
      form.append(submitter);
      document.body.append(form);
    }
    if (config.file) {
      const input = document.getElementById("encoding-file");
      const data = new File([config.file.bytes], config.file.filename, { type: "text/plain" });
      const transfer = new DataTransfer();
      transfer.items.add(data);
      input.files = transfer.files;
    }
  }, config);
}

test("multipart form overridden to URL-encoded sends one URL-encoded POST", async ({ page }) => {
  const posts = [];
  trackAction(page, posts);
  await boot(page);
  await installForm(page, {
    method: "post",
    enctype: "multipart/form-data",
    formenctype: "application/x-www-form-urlencoded",
    fields: [
      { name: "q", value: "café&=雪" },
      { name: "tag", value: "a" },
      { name: "tag", value: "b" },
    ],
    submitterName: "go",
    submitterValue: "save",
  });
  const docId = await page.evaluate(() => {
    window.__docId = "encoding-doc";
    return window.__docId;
  });
  await page.locator("#encoding-submit").click();
  await page.waitForURL(/\/search-results\.html$/);
  await expect(page.locator("h1")).toHaveText("Search Results");
  expect(await page.evaluate(() => window.__docId)).toBe(docId);
  expect(posts).toHaveLength(1);
  expect(posts[0].method).toBe("POST");
  expect(posts[0].resourceType).toBe("fetch");
  expect(posts[0].contentType).toMatch(/^application\/x-www-form-urlencoded/i);
  const expected = new URLSearchParams();
  expected.append("q", "café&=雪");
  expected.append("tag", "a");
  expected.append("tag", "b");
  expected.append("go", "save");
  expect(posts[0].body).toBe(expected.toString());
});

test("URL-encoded form overridden to multipart sends the file and a browser boundary", async ({
  page,
}) => {
  const posts = [];
  trackAction(page, posts);
  await boot(page);
  await installForm(page, {
    method: "post",
    enctype: "application/x-www-form-urlencoded",
    formenctype: "multipart/form-data",
    fields: [{ name: "title", value: "hello" }],
    file: { name: "note", filename: "note.txt", bytes: "hello-file" },
    submitterName: "go",
    submitterValue: "upload",
  });
  const docId = await page.evaluate(() => {
    window.__docId = "encoding-doc";
    return window.__docId;
  });
  await page.evaluate(() => fetch("/__form-posts"));
  await page.locator("#encoding-submit").click();
  await page.waitForURL(/\/search-results\.html$/);
  await expect(page.locator("h1")).toHaveText("Search Results");
  expect(await page.evaluate(() => window.__docId)).toBe(docId);
  expect(posts).toHaveLength(1);
  expect(posts[0].method).toBe("POST");
  expect(posts[0].resourceType).toBe("fetch");
  expect(posts[0].contentType).toMatch(
    /^multipart\/form-data;\s*boundary=(?:"([^"]+)"|([^;\s]+))/i,
  );
  const arrived = await page.evaluate(async () => {
    const response = await fetch("/__form-posts");
    return response.json();
  });
  expect(arrived).toHaveLength(1);
  expect(arrived[0].contentType).toBe(posts[0].contentType);
  const boundary = arrived[0].contentType.match(/boundary=(?:"([^"]+)"|([^;\s]+))/i);
  const token = boundary?.[1] ?? boundary?.[2];
  expect(token).toBeTruthy();
  expect(arrived[0].body).toContain(token);
  expect(arrived[0].body).toContain('name="title"');
  expect(arrived[0].body).toContain("hello");
  expect(arrived[0].body).toContain('name="note"');
  expect(arrived[0].body).toContain('filename="note.txt"');
  expect(arrived[0].body).toContain("hello-file");
  expect(arrived[0].body).toContain('name="go"');
  expect(arrived[0].body).toContain("upload");
});

test("absent encoding attributes keep multipart as the compatibility default", async ({ page }) => {
  const posts = [];
  trackAction(page, posts);
  await boot(page);
  await installForm(page, {
    method: "post",
    fields: [{ name: "q", value: "plain" }],
    submitterName: "go",
    submitterValue: "send",
  });
  const docId = await page.evaluate(() => {
    window.__docId = "encoding-doc";
    return window.__docId;
  });
  await page.locator("#encoding-submit").click();
  await page.waitForURL(/\/search-results\.html$/);
  expect(await page.evaluate(() => window.__docId)).toBe(docId);
  expect(posts).toHaveLength(1);
  expect(posts[0].method).toBe("POST");
  expect(posts[0].resourceType).toBe("fetch");
  expect(posts[0].contentType).toMatch(/^multipart\/form-data;\s*boundary=/i);
  expect(posts[0].body).toContain('name="q"');
  expect(posts[0].body).toContain("plain");
  expect(posts[0].body).toContain('name="go"');
});

async function expectNativeTextPlain(page, posts, constructions) {
  await page.locator("#encoding-submit").click();
  await page.waitForURL(/\/search-results\.html$/);
  await expect(page.locator("h1")).toHaveText("Search Results");
  expect(await page.evaluate(() => window.__encodingSentinel)).toBeUndefined();
  expect(posts).toHaveLength(1);
  expect(posts[0].method).toBe("POST");
  expect(posts[0].resourceType).toBe("document");
  expect(posts[0].contentType).toMatch(/^text\/plain/i);
  expect(posts[0].body).toBe("q=hello world\r\ngo=send\r\n");
  expect(constructions).toEqual([]);
}

test("text/plain inherited from the form is one native POST", async ({ page }) => {
  const posts = [];
  const constructions = [];
  trackAction(page, posts);
  page.on("console", (msg) => {
    if (msg.text() === "zfb-formdata-constructed") constructions.push(msg.text());
  });
  await boot(page);
  await installForm(page, {
    method: "post",
    enctype: "text/plain",
    fields: [{ name: "q", value: "hello world" }],
    submitterName: "go",
    submitterValue: "send",
  });
  constructions.length = 0;
  await expectNativeTextPlain(page, posts, constructions);
});

test("text/plain selected by the submitter is one native POST", async ({ page }) => {
  const posts = [];
  const constructions = [];
  trackAction(page, posts);
  page.on("console", (msg) => {
    if (msg.text() === "zfb-formdata-constructed") constructions.push(msg.text());
  });
  await boot(page);
  await installForm(page, {
    method: "post",
    enctype: "application/x-www-form-urlencoded",
    formenctype: "text/plain",
    fields: [{ name: "q", value: "hello world" }],
    submitterName: "go",
    submitterValue: "send",
  });
  constructions.length = 0;
  await expectNativeTextPlain(page, posts, constructions);
});

test("text/plain form overridden to URL-encoded is a router POST", async ({ page }) => {
  const posts = [];
  trackAction(page, posts);
  await boot(page);
  await installForm(page, {
    method: "post",
    enctype: "text/plain",
    formenctype: "application/x-www-form-urlencoded",
    fields: [{ name: "q", value: "hello" }],
    submitterName: "go",
    submitterValue: "send",
  });
  const docId = await page.evaluate(() => {
    window.__docId = "encoding-doc";
    return window.__docId;
  });
  await page.locator("#encoding-submit").click();
  await page.waitForURL(/\/search-results\.html$/);
  expect(await page.evaluate(() => window.__docId)).toBe(docId);
  expect(posts).toHaveLength(1);
  expect(posts[0].method).toBe("POST");
  expect(posts[0].resourceType).toBe("fetch");
  expect(posts[0].contentType).toMatch(/^application\/x-www-form-urlencoded/i);
  const expected = new URLSearchParams();
  expected.append("q", "hello");
  expected.append("go", "send");
  expect(posts[0].body).toBe(expected.toString());
});

test("GET with text/plain is intercepted and has no request body", async ({ page }) => {
  const posts = [];
  trackAction(page, posts);
  await boot(page);
  await installForm(page, {
    method: "get",
    enctype: "text/plain",
    fields: [{ name: "q", value: "hello" }],
    submitterName: "go",
    submitterValue: "send",
  });
  const docId = await page.evaluate(() => {
    window.__docId = "encoding-doc";
    return window.__docId;
  });
  await page.locator("#encoding-submit").click();
  await page.waitForURL(/\/search-results\.html\?/);
  expect(await page.evaluate(() => window.__docId)).toBe(docId);
  expect(posts).toHaveLength(1);
  expect(posts[0].method).toBe("GET");
  expect(posts[0].body).toBe("");
  const url = new URL(page.url());
  expect(url.searchParams.get("q")).toBe("hello");
  expect(url.searchParams.get("go")).toBe("send");
});
