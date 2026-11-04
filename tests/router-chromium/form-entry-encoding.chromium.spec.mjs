// Level 4: URL-encoded form entry conversion (File -> filename, CR/LF -> CRLF).
// The native submission of the same form (data-zfb-reload) is the oracle:
// expected payloads are captured from Chromium's own request, never computed
// with the router's serializer.
// https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#converting-an-entry-list-to-a-list-of-name-value-pairs
// @ts-check
import { test, expect } from "@playwright/test";

const HARNESS_INIT = () => {
  // @ts-expect-error - test-only globals
  if (window.__zfbHarness) return;
  // @ts-expect-error
  window.__zfbHarness = true;
  // @ts-expect-error
  window.__docId = crypto.randomUUID();
  const rec = { events: [], seq: 0 };
  // @ts-expect-error
  window.__zfb = rec;
  for (const name of ["zfb:page-load", "zfb:navigation-aborted"]) {
    document.addEventListener(name, () => rec.events.push({ name, seq: ++rec.seq }));
  }
};

test.beforeEach(async ({ page }) => {
  await page.addInitScript(HARNESS_INIT);
});

const ACTION = "/search-results.html";

async function boot(page) {
  await page.goto("/search.html");
  await page.waitForFunction(() =>
    /** @type {any} */ (window).__zfb?.events.some((e) => e.name === "zfb:page-load"),
  );
  await expect(page.locator("h1")).toHaveText("Search");
}

async function waitForNavDone(page, after) {
  await page.waitForFunction(
    (after) =>
      /** @type {any} */ (window).__zfb.events.some(
        (e) => e.name === "zfb:page-load" && e.seq > after,
      ),
    after,
  );
  await page.waitForFunction(() => !document.documentElement.hasAttribute("data-zfb-transition"));
}

// Fields: { tag: "input" | "textarea" | "file", type?, name, value?, files?, multiple? }.
// Files: { name, bytes }. appended: [name, value] pairs added from a formdata event.
async function installForm(page, config, native) {
  await page.evaluate(
    ({ config, native }) => {
      const form = document.createElement("form");
      form.id = "entry-form";
      form.setAttribute("action", "/search-results.html");
      form.setAttribute("method", config.method);
      if (config.enctype !== undefined) form.setAttribute("enctype", config.enctype);
      if (native) form.setAttribute("data-zfb-reload", "");
      const pending = [];
      for (const field of config.fields) {
        let el;
        if (field.tag === "textarea") {
          el = document.createElement("textarea");
          el.name = field.name;
          el.value = field.value;
        } else if (field.tag === "file") {
          el = document.createElement("input");
          el.type = "file";
          el.name = field.name;
          if (field.multiple) el.multiple = true;
          if (field.files?.length) pending.push([el, field.files]);
        } else {
          el = document.createElement("input");
          el.type = field.type ?? "text";
          el.setAttribute("name", field.name);
          if (field.type === "hidden") el.setAttribute("value", field.value);
          else el.value = field.value;
        }
        form.append(el);
      }
      if (config.appended) {
        form.addEventListener("formdata", (ev) => {
          for (const [name, value] of config.appended) ev.formData.append(name, value);
        });
      }
      if (config.foreignFile) {
        // A File from another realm, so the router cannot rely on instanceof File.
        const frame = document.createElement("iframe");
        document.body.append(frame);
        const ForeignFile = /** @type {typeof File} */ (
          /** @type {any} */ (frame.contentWindow).File
        );
        const file = new ForeignFile(["f"], config.foreignFile.filename);
        form.addEventListener("formdata", (ev) => {
          ev.formData.append(config.foreignFile.name, file);
        });
      }
      const submitter = document.createElement("button");
      submitter.id = "entry-submit";
      submitter.setAttribute("type", "submit");
      if (config.submitter) {
        submitter.setAttribute("name", config.submitter.name);
        submitter.setAttribute("value", config.submitter.value);
      }
      if (config.formenctype !== undefined) {
        submitter.setAttribute("formenctype", config.formenctype);
      }
      form.append(submitter);
      document.body.append(form);
      for (const [input, files] of pending) {
        const transfer = new DataTransfer();
        for (const file of files) {
          transfer.items.add(new File([file.bytes], file.name, { type: "text/plain" }));
        }
        input.files = transfer.files;
      }
    },
    { config, native },
  );
}

/** Submit the form once and return the single action request it produced. */
async function submit(page, config, native) {
  await boot(page);
  await installForm(page, config, native);
  const errors = [];
  page.on("pageerror", (err) => errors.push(String(err)));
  const before = await page.evaluate(() => ({
    id: /** @type {any} */ (window).__docId,
    seq: /** @type {any} */ (window).__zfb.seq,
  }));
  const requests = [];
  const onRequest = (req) => {
    if (new URL(req.url()).pathname === ACTION) requests.push(req);
  };
  page.context().on("request", onRequest);
  await page.locator("#entry-submit").click();
  if (native) {
    await page.waitForURL((url) => url.pathname === ACTION);
    await expect(page.locator("h1")).toHaveText("Search Results");
    expect(await page.evaluate(() => /** @type {any} */ (window).__docId)).not.toBe(before.id);
  } else {
    await waitForNavDone(page, before.seq);
    await expect(page.locator("h1")).toHaveText("Search Results");
    expect(await page.evaluate(() => /** @type {any} */ (window).__docId)).toBe(before.id);
    expect(
      await page.evaluate(() =>
        /** @type {any} */ (window).__zfb.events.some((e) => e.name === "zfb:navigation-aborted"),
      ),
    ).toBe(false);
  }
  page.context().off("request", onRequest);
  expect(errors).toEqual([]);
  expect(requests).toHaveLength(1);
  const [req] = requests;
  expect(req.resourceType()).toBe(native ? "document" : "fetch");
  const url = new URL(req.url());
  return {
    method: req.method(),
    contentType: req.headers()["content-type"] ?? "",
    body: req.postDataBuffer()?.toString("latin1") ?? "",
    search: url.search,
    location: native ? null : new URL(page.url()).search,
  };
}

function entries(serialized) {
  return [...new URLSearchParams(serialized)];
}

async function submitInFreshContext(browser, config, native) {
  const context = await browser.newContext();
  try {
    const page = await context.newPage();
    await page.addInitScript(HARNESS_INIT);
    return await submit(page, config, native);
  } finally {
    await context.close();
  }
}

async function compareWithNative(browser, config) {
  const oracle = await submitInFreshContext(browser, config, true);
  const routed = await submitInFreshContext(browser, config, false);

  expect(routed.method).toBe(oracle.method);
  if (config.method === "get") {
    expect(oracle.method).toBe("GET");
    expect(oracle.body).toBe("");
    expect(routed.body).toBe("");
    expect(routed.search).toBe(oracle.search);
    expect(routed.location).toBe(oracle.search);
    expect(entries(routed.search)).toEqual(entries(oracle.search));
  } else {
    expect(oracle.method).toBe("POST");
    expect(oracle.contentType).toMatch(/^application\/x-www-form-urlencoded/i);
    expect(routed.contentType).toMatch(/^application\/x-www-form-urlencoded/i);
    expect(routed.search).toBe("");
    expect(routed.body).toBe(oracle.body);
    expect(entries(routed.body)).toEqual(entries(oracle.body));
  }
  return oracle;
}

const FILE_FIELDS = [
  { tag: "input", name: "q", value: "files" },
  {
    tag: "file",
    name: "upload",
    files: [{ name: "hello.txt", bytes: "hello" }],
  },
  { tag: "file", name: "empty" },
  {
    tag: "file",
    name: "many",
    multiple: true,
    files: [
      { name: "a.txt", bytes: "a" },
      { name: "b.txt", bytes: "b" },
    ],
  },
];

const MULTILINE = "line1\nline2\rline3\r\nline4";

const MIXED_FIELDS = [
  { tag: "input", name: "tag", value: "first" },
  { tag: "input", name: "blank", value: "" },
  { tag: "input", name: "tag", value: "second" },
  { tag: "input", name: "text", value: "café 雪 + a&b=c" },
];

const CASES = [
  {
    name: "GET with selected, unselected and multiple files",
    method: "get",
    fields: FILE_FIELDS,
    expectInOracle: ["upload=hello.txt", "empty=&", "many=a.txt&many=b.txt"],
  },
  {
    name: "explicit URL-encoded POST with file inputs",
    method: "post",
    enctype: "application/x-www-form-urlencoded",
    fields: FILE_FIELDS,
    expectInOracle: ["upload=hello.txt", "empty=&", "many=a.txt&many=b.txt"],
  },
  {
    name: "multipart POST overridden to URL-encoded by the submitter",
    method: "post",
    enctype: "multipart/form-data",
    formenctype: "application/x-www-form-urlencoded",
    fields: [
      {
        tag: "file",
        name: "upload",
        files: [{ name: "hello.txt", bytes: "hello" }],
      },
      { tag: "textarea", name: "note", value: MULTILINE },
    ],
    submitter: { name: "go", value: "save" },
    expectInOracle: ["upload=hello.txt", "%0D%0A", "go=save"],
  },
  {
    name: "GET multiline textarea",
    method: "get",
    fields: [{ tag: "textarea", name: "note", value: MULTILINE }],
    expectInOracle: ["note=line1%0D%0Aline2%0D%0Aline3%0D%0Aline4"],
  },
  {
    name: "URL-encoded POST multiline textarea",
    method: "post",
    enctype: "application/x-www-form-urlencoded",
    fields: [{ tag: "textarea", name: "note", value: MULTILINE }],
    expectInOracle: ["note=line1%0D%0Aline2%0D%0Aline3%0D%0Aline4"],
  },
  {
    name: "GET names and values with LF, CR and CRLF",
    method: "get",
    fields: [{ tag: "input", type: "hidden", name: "h\nname", value: "v\rx\r\ny\nz" }],
    appended: [
      ["a\rb", "c\nd"],
      ["e\r\nf", "g\r\n\r\nh\n\ri\r\r\nj"],
    ],
    expectInOracle: [
      "h%0D%0Aname=v%0D%0Ax%0D%0Ay%0D%0Az",
      "a%0D%0Ab=c%0D%0Ad",
      "h%0D%0A%0D%0Ai%0D%0A%0D%0Aj",
    ],
  },
  {
    name: "URL-encoded POST names and values with LF, CR and CRLF",
    method: "post",
    enctype: "application/x-www-form-urlencoded",
    fields: [{ tag: "input", type: "hidden", name: "h\nname", value: "v\rx\r\ny\nz" }],
    appended: [
      ["a\rb", "c\nd"],
      ["e\r\nf", "g\r\n\r\nh\n\ri"],
    ],
    expectInOracle: ["h%0D%0Aname=v%0D%0Ax%0D%0Ay%0D%0Az", "a%0D%0Ab=c%0D%0Ad"],
  },
  {
    name: "URL-encoded POST file name containing line breaks",
    method: "post",
    enctype: "application/x-www-form-urlencoded",
    fields: [
      {
        tag: "file",
        name: "up\nload",
        files: [{ name: "two\nlines\r.txt", bytes: "x" }],
      },
    ],
    foreignFile: { name: "foreign", filename: "other\rrealm.txt" },
    // Chromium keeps the line breaks in File.name; the oracle normalizes them.
    expectInOracle: ["up%0D%0Aload=two%0D%0Alines%0D%0A.txt", "foreign=other%0D%0Arealm.txt"],
  },
  {
    name: "GET repeated names, empty values, Unicode and reserved characters",
    method: "get",
    fields: MIXED_FIELDS,
    submitter: { name: "go", value: "a+b" },
    expectInOracle: ["tag=first&blank=&tag=second", "go=a%2Bb"],
  },
  {
    name: "URL-encoded POST repeated names, empty values, Unicode and reserved characters",
    method: "post",
    enctype: "application/x-www-form-urlencoded",
    fields: MIXED_FIELDS,
    submitter: { name: "go", value: "a+b" },
    expectInOracle: ["tag=first&blank=&tag=second", "go=a%2Bb"],
  },
];

for (const config of CASES) {
  test(`${config.name} matches the native submission`, async ({ browser }) => {
    const oracle = await compareWithNative(browser, config);
    // Guard that the oracle exercised the conversion this case is about.
    const raw = config.method === "get" ? oracle.search : oracle.body;
    for (const fragment of config.expectInOracle) expect(raw).toContain(fragment);
  });
}

test("URL-encoded POST body reflects preparation-hook changes to formData", async ({ page }) => {
  await boot(page);
  await installForm(
    page,
    {
      method: "post",
      enctype: "application/x-www-form-urlencoded",
      fields: [
        { tag: "input", name: "orig", value: "kept" },
        {
          tag: "file",
          name: "upload",
          files: [{ name: "orig.txt", bytes: "o" }],
        },
      ],
    },
    false,
  );
  await page.evaluate(() => {
    document.addEventListener(
      "zfb:before-preparation",
      (ev) => {
        const event = /** @type {any} */ (ev);
        event.formData.append("added", "x\ny");
        const replacement = new FormData();
        for (const [name, value] of event.formData) replacement.append(name, value);
        replacement.append("swapped", new File(["r"], "late.txt"));
        event.formData = replacement;
      },
      { once: true },
    );
  });
  const before = await page.evaluate(() => /** @type {any} */ (window).__zfb.seq);
  const requestPromise = page.waitForRequest((r) => new URL(r.url()).pathname === ACTION);
  await page.locator("#entry-submit").click();
  const request = await requestPromise;
  await waitForNavDone(page, before);
  expect(request.resourceType()).toBe("fetch");
  expect(request.headers()["content-type"]).toMatch(/^application\/x-www-form-urlencoded/i);
  expect(request.postDataBuffer()?.toString("latin1")).toBe(
    "orig=kept&upload=orig.txt&added=x%0D%0Ay&swapped=late.txt",
  );
});
