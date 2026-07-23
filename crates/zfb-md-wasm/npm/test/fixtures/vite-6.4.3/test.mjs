import { chromium } from "@playwright/test";
import { build, createServer, preview, version as viteVersion } from "vite";
import { cpSync, mkdtempSync, mkdirSync, readFileSync, rmSync } from "node:fs";
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
  const pathname = new URL(rawUrl).pathname;
  if (pathname.includes("zfb_md_wasm_highlight_glue.zfb-resource")) return "highlight-glue";
  if (pathname.includes("zfb_md_wasm_highlight_bg") && pathname.endsWith(".wasm")) {
    return "highlight-wasm";
  }
  if (pathname.includes("zfb_md_wasm_glue.zfb-resource")) return "glue";
  if (pathname.includes("zfb_md_wasm_bg") && pathname.endsWith(".wasm")) return "wasm";
  return undefined;
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function exercise(origin, selectedIds, label) {
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  const requests = [];
  const responses = [];
  let failedFirstWasm = false;

  await page.route("**/*", async (route) => {
    const kind = resourceKind(route.request().url());
    if (kind === "wasm" && !failedFirstWasm) {
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
    const result = await page.evaluate(() => window.runFixture());

    assert(
      result.transientError.includes("503"),
      `${label}: transient initialization was not seen`,
    );
    assert(result.trapName === "ZfbMdWasmTrapError", `${label}: trap was not wrapped once`);
    assert(result.parsed.ast.type === "root", `${label}: parseToAst failed`);
    for (const [name, value] of [
      ["root highlight", result.highlighted],
      ["recovery highlight", result.recovered],
      ["highlight subpath", result.highlightOnlyResult],
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
    assert(
      result.highlightState.compiledModuleLoads === 1,
      `${label}: highlight subpath did not run`,
    );

    const rootGlue = responses.filter(({ kind, status }) => kind === "glue" && status === 200);
    const rootWasm = responses.filter(({ kind, status }) => kind === "wasm" && status === 200);
    const highlightGlue = responses.filter(
      ({ kind, status }) => kind === "highlight-glue" && status === 200,
    );
    const highlightWasm = responses.filter(
      ({ kind, status }) => kind === "highlight-wasm" && status === 200,
    );
    assert(rootGlue.length === 3, `${label}: expected retry + recovery glue module requests`);
    assert(rootWasm.length === 1, `${label}: expected one successful root Wasm request`);
    assert(highlightGlue.length === 1, `${label}: expected one highlight glue request`);
    assert(highlightWasm.length === 1, `${label}: expected one highlight Wasm request`);
    assert(
      rootGlue.every(({ contentType }) =>
        /^(?:application|text)\/javascript(?:;|$)/.test(contentType),
      ),
      `${label}: glue MIME was not JavaScript`,
    );
    assert(
      [...rootWasm, ...highlightWasm].every(({ contentType }) =>
        /^application\/wasm(?:;|$)/.test(contentType),
      ),
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
      [...selectedIds].some((id) => id.endsWith("/dist/browser.js")),
      `${label}: Vite did not select dist/browser.js`,
    );
    assert(
      [...selectedIds].some((id) => id.endsWith("/dist/highlight-browser.js")),
      `${label}: Vite did not select dist/highlight-browser.js`,
    );
    assert(
      ![...selectedIds].some(
        (id) => id.endsWith("/dist/index.js") || id.endsWith("/dist/highlight.js"),
      ),
      `${label}: Vite selected a Node/default entry`,
    );
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
