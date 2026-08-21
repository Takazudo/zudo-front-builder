import { chromium } from "@playwright/test";
import { build, createServer, preview, version as viteVersion } from "vite";
import { cpSync, mkdtempSync, mkdirSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";

const fixtureRoot = dirname(fileURLToPath(import.meta.url));
const archivePath = process.env.ZFB_MD_WASM_TARBALL;
if (!archivePath) {
  throw new Error("Set ZFB_MD_WASM_TARBALL to the packed @takazudo/zfb-md-wasm archive");
}
if (viteVersion !== "6.4.3") {
  throw new Error(`Expected Vite 6.4.3, received ${viteVersion}`);
}

const tempRoot = mkdtempSync(join(tmpdir(), "zfb-md-wasm-vite-"));
const unpackRoot = join(tempRoot, "unpacked");
const packageDestination = join(fixtureRoot, "node_modules", "@takazudo", "zfb-md-wasm");
mkdirSync(unpackRoot, { recursive: true });
execFileSync("tar", ["-xzf", resolve(archivePath), "-C", unpackRoot], { stdio: "pipe" });
rmSync(packageDestination, { recursive: true, force: true });
mkdirSync(dirname(packageDestination), { recursive: true });
cpSync(join(unpackRoot, "package"), packageDestination, { recursive: true });

function packageSelectionPlugin(seenIds) {
  return {
    name: "assert-zfb-md-wasm-browser-condition",
    transform(_code, id) {
      if (id.includes("/@takazudo/zfb-md-wasm/dist/")) {
        seenIds.add(id.split("?")[0]);
      }
    },
  };
}

function resourceKind(rawUrl) {
  const url = new URL(rawUrl);
  // Vite imports `?url` assets through a JavaScript module proxy before the
  // fixture application runs. Those proxy loads are not runtime resources;
  // only the URLs returned by the proxies and loaded by createWasmApi count
  // toward this browser-resource assertion.
  if (url.searchParams.has("url") && !url.searchParams.has("zfbMdWasmGen")) {
    return undefined;
  }
  const { pathname } = url;
  if (pathname.includes("zfb_md_wasm_highlight_glue.zfb-resource")) return "highlight-glue";
  if (pathname.includes("zfb_md_wasm_highlight_bg") && pathname.endsWith(".wasm")) {
    return "highlight-wasm";
  }
  if (pathname.includes("zfb_md_wasm_render_glue.zfb-resource")) return "render-glue";
  if (pathname.includes("zfb_md_wasm_render_bg") && pathname.endsWith(".wasm")) {
    return "render-wasm";
  }
  if (pathname.includes("zfb_md_wasm_parse_glue.zfb-resource")) return "parse-glue";
  if (pathname.includes("zfb_md_wasm_parse_bg") && pathname.endsWith(".wasm")) {
    return "parse-wasm";
  }
  if (pathname.includes("zfb_md_wasm_glue.zfb-resource")) return "glue";
  if (pathname.includes("zfb_md_wasm_bg") && pathname.endsWith(".wasm")) return "wasm";
  return undefined;
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function isExpectedTransient503(message) {
  return /^Failed to load resource: the server responded with a status of 503 \(Service Unavailable\)$/.test(
    message,
  );
}

function assertOwnPair(records, glueKind, wasmKind, label) {
  assert(records.length === 2, `${label}: expected exactly two own resource requests`);
  assert(
    records.filter(({ kind }) => kind === glueKind).length === 1 &&
      records.filter(({ kind }) => kind === wasmKind).length === 1,
    `${label}: action fetched ${records.map(({ kind }) => kind).join(", ")}`,
  );
  assert(
    records.every(({ status }) => status === 200),
    `${label}: action did not receive HTTP 200 for both resources`,
  );
  const glue = records.find(({ kind }) => kind === glueKind);
  const wasm = records.find(({ kind }) => kind === wasmKind);
  assert(
    glue !== undefined && /^(?:application|text)\/javascript(?:;|$)/.test(glue.contentType),
    `${label}: glue MIME was not JavaScript`,
  );
  assert(
    wasm !== undefined && /^application\/wasm(?:;|$)/.test(wasm.contentType),
    `${label}: wasm MIME was not application/wasm`,
  );
}

async function exercise(origin, selectedIds, label) {
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  const requests = [];
  const responses = [];
  const browserErrors = [];
  const browserRequests = [];
  const expectedTransientConsoleErrors = [];
  let failedFirstWasm = false;

  page.on("pageerror", (error) => {
    browserErrors.push(error.stack ?? error.message);
  });
  page.on("console", (message) => {
    if (message.type() !== "error") return;
    if (failedFirstWasm && isExpectedTransient503(message.text())) {
      expectedTransientConsoleErrors.push(message.text());
      return;
    }
    browserErrors.push(message.text());
  });
  page.on("request", (request) => {
    browserRequests.push(request.url());
  });

  await page.route("**/*", async (route) => {
    const request = route.request();
    const kind = resourceKind(request.url());
    if (kind === "wasm" && request.resourceType() === "fetch" && !failedFirstWasm) {
      failedFirstWasm = true;
      await route.fulfill({
        status: 503,
        contentType: "text/plain",
        body: "transient Vite fixture failure",
      });
      return;
    }
    await route.continue();
  });
  page.on("request", (request) => {
    const kind = resourceKind(request.url());
    if (kind) requests.push({ kind, url: request.url() });
  });
  page.on("response", (response) => {
    const kind = resourceKind(response.url());
    if (kind) {
      responses.push({
        kind,
        url: response.url(),
        status: response.status(),
        contentType: response.headers()["content-type"] ?? "",
      });
    }
  });

  try {
    await page.goto(new URL("/fixture/", origin).href, { waitUntil: "networkidle" });
    try {
      await page.waitForFunction(() => typeof window.runFixture === "function");
    } catch (error) {
      throw new Error(
        `${label}: fixture module did not load: ${browserErrors.join("\n") || error}\n` +
          `Requests: ${browserRequests.join("\n")}`,
      );
    }
    const result = await page.evaluate(() => window.runFixture());

    assert(
      result.transientError.includes("503"),
      `${label}: transient initialization was not seen`,
    );
    assert(result.trapName === "ZfbMdWasmTrapError", `${label}: trap was not wrapped once`);
    assert(result.parsed.ast.type === "root", `${label}: parseToAst failed`);
    const list = result.list.ast.children[0];
    assert(
      result.list.diagnostics.length === 0 &&
        list.type === "list" &&
        result.listSource.slice(list.position.start.offset, list.position.end.offset) ===
          "- 日本 😀",
      `${label}: packed list end position did not use the complete UTF-16 list span`,
    );
    assert(
      result.diagnostic.diagnostics.length === 1 &&
        result.diagnostic.diagnostics[0].source === "markdown" &&
        result.diagnostic.diagnostics[0].line === 1 &&
        result.diagnostic.diagnostics[0].column === 6 &&
        typeof result.diagnostic.diagnostics[0].message === "string",
      `${label}: packed diagnostic did not retain structured UTF-16 coordinates`,
    );
    for (const [name, value] of [
      ["root highlight", result.highlighted],
      ["recovery highlight", result.recovered],
    ]) {
      assert(
        value.diagnostics.length === 0 && value.html.includes("hi-"),
        `${label}: ${name} failed`,
      );
    }
    assert(
      result.afterTrap.currentGeneration === result.beforeTrap.currentGeneration + 1,
      `${label}: glue generation did not advance`,
    );
    assert(
      result.afterTrap.compiledModuleLoads === result.beforeTrap.compiledModuleLoads,
      `${label}: Wasm recompiled during trap recovery`,
    );
    const rootGlue = responses.filter(({ kind, status }) => kind === "glue" && status === 200);
    const rootWasm = responses.filter(({ kind, status }) => kind === "wasm" && status === 200);
    assert(rootGlue.length === 3, `${label}: expected retry + recovery glue module requests`);
    assert(rootWasm.length === 1, `${label}: expected one successful root Wasm request`);
    assert(
      requests.every(({ kind }) => kind === "glue" || kind === "wasm") &&
        responses.every(({ kind }) => kind === "glue" || kind === "wasm"),
      `${label}: root action fetched a sibling or unexpected resource`,
    );
    assert(
      rootGlue.every(({ contentType }) =>
        /^(?:application|text)\/javascript(?:;|$)/.test(contentType),
      ),
      `${label}: glue MIME was not JavaScript`,
    );
    assert(
      rootWasm.every(({ contentType }) => /^application\/wasm(?:;|$)/.test(contentType)),
      `${label}: Wasm MIME was not application/wasm`,
    );

    const glueMarkers = rootGlue
      .map(({ url }) => {
        const parsedUrl = new URL(url);
        return [
          parsedUrl.searchParams.get("zfbMdWasmGen"),
          parsedUrl.searchParams.get("zfbMdWasmAttempt"),
        ];
      })
      .sort((left, right) => Number(left[1]) - Number(right[1]));
    assert(
      JSON.stringify(glueMarkers) ===
        JSON.stringify([
          ["0", "1"],
          ["0", "2"],
          ["1", "3"],
        ]),
      `${label}: unexpected glue identity markers ${JSON.stringify(glueMarkers)}`,
    );
    assert(
      new Set(rootGlue.map(({ url }) => url)).size === 3,
      `${label}: glue URLs were not fresh`,
    );
    assert(
      requests.filter(({ kind }) => kind === "wasm").length === 2,
      `${label}: failed + successful Wasm fetch count changed`,
    );
    assert(
      expectedTransientConsoleErrors.length === 1,
      `${label}: expected exactly one deliberate transient 503 console signal, received ${expectedTransientConsoleErrors.length}`,
    );

    assert(
      [...selectedIds].some((id) => id.endsWith("/dist/browser.js")),
      `${label}: Vite did not select dist/browser.js`,
    );
    assert(
      [...selectedIds].some((id) => id.endsWith("/dist/highlight-browser.js")),
      `${label}: Vite did not select dist/highlight-browser.js`,
    );
    assert(
      [...selectedIds].some((id) => id.endsWith("/dist/render-browser.js")),
      `${label}: Vite did not select dist/render-browser.js`,
    );
    assert(
      [...selectedIds].some((id) => id.endsWith("/dist/parse-browser.js")),
      `${label}: Vite did not select dist/parse-browser.js`,
    );
    assert(
      ![...selectedIds].some(
        (id) =>
          id.endsWith("/dist/index.js") ||
          id.endsWith("/dist/highlight.js") ||
          id.endsWith("/dist/render.js") ||
          id.endsWith("/dist/parse.js"),
      ),
      `${label}: Vite selected a Node/default entry`,
    );
    assert(browserErrors.length === 0, `${label}: browser errors: ${browserErrors.join("\n")}`);

    // Every exact-pair action gets its own fresh page. The root action above
    // intentionally exercises only the root artifact; no previous lazy
    // initialization can satisfy these assertions.
    await exerciseFreshAction(
      origin,
      `${label} highlight`,
      "highlight",
      "highlight-glue",
      "highlight-wasm",
    );
    await exerciseFreshAction(origin, `${label} render`, "render", "render-glue", "render-wasm");
    await exerciseFreshAction(origin, `${label} parse`, "parse", "parse-glue", "parse-wasm");
    await exerciseCoexistence(origin, `${label} coexistence`);
  } finally {
    await browser.close();
  }
}

async function exerciseFreshAction(origin, label, action, glueKind, wasmKind) {
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  const requests = [];
  const responses = [];
  const browserErrors = [];
  page.on("pageerror", (error) => browserErrors.push(error.stack ?? error.message));
  page.on("console", (message) => {
    if (message.type() === "error") browserErrors.push(message.text());
  });
  page.on("request", (request) => {
    const kind = resourceKind(request.url());
    if (kind) requests.push({ kind, url: request.url() });
  });
  page.on("response", (response) => {
    const kind = resourceKind(response.url());
    if (kind) {
      responses.push({
        kind,
        url: response.url(),
        status: response.status(),
        contentType: response.headers()["content-type"] ?? "",
      });
    }
  });

  try {
    await page.goto(new URL("/fixture/", origin).href, { waitUntil: "networkidle" });
    await page.waitForFunction(() => typeof window.runFixture === "function");
    const result = await page.evaluate(
      (selectedAction) => window.runFixture(selectedAction),
      action,
    );
    if (action === "highlight") {
      assert(result.result.diagnostics.length === 0, `${label}: highlight action failed`);
    } else if (action === "render") {
      assert(
        result.result.diagnostics.length === 0 && result.result.html === "<h1>Render subpath</h1>",
        `${label}: render action failed`,
      );
    } else {
      assert(
        result.result.diagnostics.length === 0 && result.result.ast.type === "root",
        `${label}: parse action failed`,
      );
    }
    assert(
      requests.length === 2 && requests.every(({ kind }) => kind === glueKind || kind === wasmKind),
      `${label}: fresh action fetched a sibling or unexpected resource`,
    );
    assertOwnPair(responses, glueKind, wasmKind, label);
    assert(browserErrors.length === 0, `${label}: browser errors: ${browserErrors.join("\n")}`);
  } finally {
    await browser.close();
  }
}

async function exerciseCoexistence(origin, label) {
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  const requests = [];
  const responses = [];
  const browserErrors = [];
  page.on("pageerror", (error) => browserErrors.push(error.stack ?? error.message));
  page.on("console", (message) => {
    if (message.type() === "error") browserErrors.push(message.text());
  });
  page.on("request", (request) => {
    const kind = resourceKind(request.url());
    if (kind) requests.push({ kind, url: request.url() });
  });
  page.on("response", (response) => {
    const kind = resourceKind(response.url());
    if (kind) {
      responses.push({
        kind,
        url: response.url(),
        status: response.status(),
        contentType: response.headers()["content-type"] ?? "",
      });
    }
  });

  try {
    await page.goto(new URL("/fixture/", origin).href, { waitUntil: "networkidle" });
    await page.waitForFunction(() => typeof window.runFixture === "function");
    const coexistence = await page.evaluate(() => window.runFixture("coexist"));
    assert(
      coexistence.rendered.diagnostics.length === 0 &&
        coexistence.parsed.ast.type === "root" &&
        coexistence.parsedAfterRenderTrap.ast.type === "root",
      `${label}: render/parse same-realm coexistence failed`,
    );
    assert(coexistence.trapName === "ZfbMdWasmTrapError", `${label}: render trap was not wrapped`);
    assert(
      coexistence.afterRender.currentGeneration ===
        coexistence.beforeRender.currentGeneration + 1 &&
        coexistence.afterParse.currentGeneration === coexistence.beforeParse.currentGeneration,
      `${label}: render and parse recovery state was shared`,
    );
    assert(
      requests.length === 5 &&
        requests.filter(({ kind }) => kind === "render-glue").length === 2 &&
        requests.filter(({ kind }) => kind === "render-wasm").length === 1 &&
        requests.filter(({ kind }) => kind === "parse-glue").length === 1 &&
        requests.filter(({ kind }) => kind === "parse-wasm").length === 1,
      `${label}: coexistence request set was not independent`,
    );
    assert(
      responses.every(({ kind }) =>
        ["render-glue", "render-wasm", "parse-glue", "parse-wasm"].includes(kind),
      ),
      `${label}: coexistence fetched a root/highlight resource`,
    );
    assertOwnPair(
      [
        responses.filter(({ kind }) => kind === "render-glue").at(-1),
        responses.find(({ kind }) => kind === "render-wasm"),
      ].filter((record) => record !== undefined),
      "render-glue",
      "render-wasm",
      `${label} render`,
    );
    assertOwnPair(
      [
        responses.find(({ kind }) => kind === "parse-glue"),
        responses.find(({ kind }) => kind === "parse-wasm"),
      ].filter((record) => record !== undefined),
      "parse-glue",
      "parse-wasm",
      `${label} parse`,
    );
    assert(browserErrors.length === 0, `${label}: browser errors: ${browserErrors.join("\n")}`);
  } finally {
    await browser.close();
  }
}

async function exerciseRetry(origin, label, action, wasmKind, glueKind, invalidBytes) {
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  const requests = [];
  const responses = [];
  let failed = false;
  const browserErrors = [];
  const expectedTransientConsoleErrors = [];
  page.on("pageerror", (error) => browserErrors.push(error.stack ?? error.message));
  page.on("console", (message) => {
    if (message.type() !== "error") return;
    if (!invalidBytes && failed && isExpectedTransient503(message.text())) {
      expectedTransientConsoleErrors.push(message.text());
      return;
    }
    browserErrors.push(message.text());
  });
  page.on("request", (request) => {
    const kind = resourceKind(request.url());
    if (kind) requests.push({ kind, url: request.url() });
  });
  page.on("response", (response) => {
    const kind = resourceKind(response.url());
    if (kind) {
      responses.push({
        kind,
        url: response.url(),
        status: response.status(),
        contentType: response.headers()["content-type"] ?? "",
      });
    }
  });
  await page.route("**/*", async (route) => {
    const request = route.request();
    if (resourceKind(request.url()) === wasmKind && request.resourceType() === "fetch" && !failed) {
      failed = true;
      if (invalidBytes) {
        await route.fulfill({
          status: 200,
          contentType: "application/wasm",
          body: "not a WebAssembly module",
        });
      } else {
        await route.fulfill({
          status: 503,
          contentType: "text/plain",
          body: "transient Vite fixture failure",
        });
      }
      return;
    }
    await route.continue();
  });

  try {
    await page.goto(new URL("/fixture/", origin).href, { waitUntil: "networkidle" });
    const result = await page.evaluate((selectedAction) => window.runRetry(selectedAction), action);
    assert(
      typeof result.firstError === "string" &&
        /503|WebAssembly|compile|magic|module/i.test(result.firstError),
      `${label}: failure was not actionable: ${result.firstError}`,
    );
    assert(result.result.diagnostics.length === 0, `${label}: retry did not return a clean result`);
    const successfulPair = [
      responses.filter(({ kind }) => kind === glueKind).at(-1),
      responses.filter(({ kind }) => kind === wasmKind).at(-1),
    ].filter((record) => record !== undefined);
    assertOwnPair(successfulPair, glueKind, wasmKind, label);
    assert(
      requests.length === 4 && requests.every(({ kind }) => kind === glueKind || kind === wasmKind),
      `${label}: retry fetched a sibling or unexpected resource`,
    );
    assert(
      responses.filter(({ kind }) => kind === glueKind).length === 2 &&
        responses.filter(({ kind }) => kind === wasmKind).length === 2,
      `${label}: retry did not request exactly two own resources per attempt`,
    );
    assert(
      new Set(responses.filter(({ kind }) => kind === glueKind).map(({ url }) => url)).size === 2,
      `${label}: retry glue imports did not receive fresh module URLs`,
    );
    assert(
      expectedTransientConsoleErrors.length === (invalidBytes ? 0 : 1),
      `${label}: unexpected transient 503 console signal count ${expectedTransientConsoleErrors.length}`,
    );
    assert(browserErrors.length === 0, `${label}: browser errors: ${browserErrors.join("\n")}`);
  } finally {
    await browser.close();
  }
}

const common = {
  root: fixtureRoot,
  base: "/fixture/",
  logLevel: "error",
  optimizeDeps: { noDiscovery: true, include: [] },
};

try {
  const devSelectedIds = new Set();
  const devServer = await createServer({
    ...common,
    plugins: [packageSelectionPlugin(devSelectedIds)],
    server: { host: "127.0.0.1", port: 0, strictPort: false },
  });
  await devServer.listen();
  try {
    const address = devServer.httpServer.address();
    const origin = `http://127.0.0.1:${address.port}`;
    await exercise(origin, devSelectedIds, "vite dev");
    await exerciseRetry(
      origin,
      "vite dev render retry",
      "render",
      "render-wasm",
      "render-glue",
      false,
    );
    await exerciseRetry(
      origin,
      "vite dev parse invalid-byte retry",
      "parse",
      "parse-wasm",
      "parse-glue",
      true,
    );
  } finally {
    await devServer.close();
  }

  const outDir = join(tempRoot, "dist");
  const buildSelectedIds = new Set();
  await build({
    ...common,
    plugins: [packageSelectionPlugin(buildSelectedIds)],
    build: { outDir, emptyOutDir: true },
  });
  const builtFiles = readFileSync(join(outDir, "index.html"), "utf8");
  assert(builtFiles.includes("/fixture/assets/"), "vite build did not apply the non-root base");
  const emittedResources = readdirSync(outDir, { recursive: true }).filter(
    (name) => typeof name === "string" && /zfb_md_wasm.*\.(?:mjs|wasm)$/.test(name),
  );
  const expectedResourceNames = [
    "zfb_md_wasm_glue.zfb-resource.mjs",
    "zfb_md_wasm_bg.wasm",
    "zfb_md_wasm_highlight_glue.zfb-resource.mjs",
    "zfb_md_wasm_highlight_bg.wasm",
    "zfb_md_wasm_render_glue.zfb-resource.mjs",
    "zfb_md_wasm_render_bg.wasm",
    "zfb_md_wasm_parse_glue.zfb-resource.mjs",
    "zfb_md_wasm_parse_bg.wasm",
  ];
  assert(
    emittedResources.length === expectedResourceNames.length,
    `vite production output emitted ${emittedResources.length} runtime resources, expected ${expectedResourceNames.length}`,
  );
  for (const expected of expectedResourceNames) {
    const extension = expected.endsWith(".wasm") ? ".wasm" : ".mjs";
    const assetStem = expected.slice(0, -extension.length);
    assert(
      emittedResources.filter(
        (name) =>
          name.endsWith(expected) || (name.includes(`${assetStem}-`) && name.endsWith(extension)),
      ).length === 1,
      `vite production output did not emit exactly one ${expected}`,
    );
  }

  const previewServer = await preview({
    ...common,
    build: { outDir },
    preview: { host: "127.0.0.1", port: 0, strictPort: false },
  });
  try {
    const address = previewServer.httpServer.address();
    const origin = `http://127.0.0.1:${address.port}`;
    await exercise(origin, buildSelectedIds, "vite preview");
  } finally {
    await previewServer.close();
  }

  console.log("Vite 6.4.3 packed browser fixture passed in dev and production preview");
} finally {
  rmSync(packageDestination, { recursive: true, force: true });
  rmSync(tempRoot, { recursive: true, force: true });
}
