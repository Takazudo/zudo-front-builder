#!/usr/bin/env node
// Browser benchmark runner (parseToAst, zfb#1857, epic zfb#1854): the same
// capability-matched parseToAst-vs-remark grid as the Node runner, executed
// in REAL Chromium (the consumer context is a browser live preview). One
// command, from the repo root, after the package is built:
//
//   pnpm --filter @takazudo/zfb-md-wasm build   # once, real dist/ required
//   node crates/zfb-md-wasm/npm/test/bench/browser/run-browser-bench.mjs
//
// Steps: esbuild-bundles the page entry (package browser entry + remark
// stack + shared fixtures/bench core) with the exact loader flags
// `test/package-browser.test.ts` proves the packaged browser entry against,
// serves the bundle over a local http server, drives Chromium via
// Playwright (root devDependency), and prints the same markdown table +
// raw JSON the Node runner prints. NOT wired into `pnpm test` or CI.

import { execFileSync } from "node:child_process";
import { createServer } from "node:http";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { chromium } from "@playwright/test";

import { markdownTable } from "../bench-core.mjs";

const benchBrowserDir = dirname(fileURLToPath(import.meta.url));
const packageRoot = resolve(benchBrowserDir, "../../..");

if (!existsSync(join(packageRoot, "dist", "browser.js"))) {
  console.error(
    "dist/browser.js not found — build the package first:\n" +
      "  pnpm --filter @takazudo/zfb-md-wasm build",
  );
  process.exit(1);
}

const outDir = mkdtempSync(join(tmpdir(), "zfb-md-wasm-browser-bench-"));

execFileSync(
  join(packageRoot, "node_modules", ".bin", "esbuild"),
  [
    join(benchBrowserDir, "browser-page-entry.mjs"),
    "--bundle",
    "--format=esm",
    "--platform=browser",
    `--outdir=${outDir}`,
    "--loader:.zfb-resource.mjs=file",
    "--loader:.wasm=file",
  ],
  { cwd: packageRoot, stdio: "inherit" },
);

writeFileSync(
  join(outDir, "index.html"),
  '<!doctype html><meta charset="utf-8"><title>parseToAst bench</title>' +
    '<script type="module" src="./browser-page-entry.js"></script>',
);

const MIME = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".wasm": "application/wasm",
};

const server = createServer((req, res) => {
  // Strip the query string before mapping to a filesystem path — the wasm glue
  // is requested with a cache-busting `?zfbMdWasmGen=…` suffix, which would
  // otherwise be joined onto the path and make existsSync fail.
  const urlPath = (req.url ?? "/").split("?")[0];
  const path = join(outDir, urlPath === "/" ? "index.html" : urlPath.replace(/^\/+/, ""));
  if (!path.startsWith(outDir) || !existsSync(path)) {
    res.writeHead(404).end("not found");
    return;
  }
  res.writeHead(200, { "content-type": MIME[extname(path)] ?? "application/octet-stream" });
  res.end(readFileSync(path));
});

async function main() {
  await new Promise((resolvePort) => server.listen(0, "127.0.0.1", resolvePort));
  const { port } = server.address();

  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    page.on("console", (msg) => process.stderr.write(`[page] ${msg.text()}\n`));
    page.on("pageerror", (err) => process.stderr.write(`[pageerror] ${err.message}\n`));
    await page.goto(`http://127.0.0.1:${port}/`);
    await page.waitForFunction(() => typeof window.__runBench === "function");
    // Playwright evaluate has no default timeout; the large doc keeps the
    // page busy for a while — that is expected.
    const { initMs, rows, implNames } = await page.evaluate(() => window.__runBench());

    console.log("## parseToAst vs remark-parse — Chromium benchmark\n");
    console.log(`Chromium: ${browser.version()}\n`);
    console.log(`Init (one-off, wasm fetch+compile+instantiate): ${initMs.toFixed(1)} ms\n`);
    console.log("### Steady state (per-parse, warmed)\n");
    console.log(markdownTable(rows, implNames));
    console.log(
      "\n`ratio` = parseToAst+JSON.parse mean ÷ remark-parse mean (>1 means remark is faster).\n",
    );
    console.log("### Raw JSON\n");
    console.log("```json");
    console.log(JSON.stringify({ chromium: browser.version(), initMs, rows }, null, 2));
    console.log("```");
  } finally {
    await browser.close();
    server.close();
    rmSync(outDir, { recursive: true, force: true });
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
