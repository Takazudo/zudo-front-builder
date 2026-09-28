#!/usr/bin/env node

/**
 * Capture the pre-change basic-blog visual baseline for issue #3245.
 *
 * Usage:
 *   node capture.mjs <built-site-dir> <zfb-binary> <output-dir> [repo-root]
 *
 * output-dir must be outside the repository. It receives PNG files,
 * computed-style JSON snapshots and manifest.json. The caller can upload the
 * PNGs and copy only the JSON files plus manifest.json into the repository.
 */

import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import net from "node:net";
import { spawn, spawnSync } from "node:child_process";
import { finished } from "node:stream/promises";
import { createRequire } from "node:module";

const [siteArg, zfbArg, outputArg, repoArg = process.cwd()] = process.argv.slice(2);

if (!siteArg || !zfbArg || !outputArg) {
  console.error(
    "usage: node capture.mjs <built-site-dir> <zfb-binary> <output-dir> [repo-root]",
  );
  process.exit(2);
}

const siteDir = path.resolve(siteArg);
const zfbBinary = path.resolve(zfbArg);
const outputDir = path.resolve(outputArg);
const repoRoot = path.resolve(repoArg);
const distDir = path.join(siteDir, "dist");

if (!fs.statSync(siteDir).isDirectory()) throw new Error(`site directory not found: ${siteDir}`);
if (!fs.statSync(zfbBinary).isFile()) throw new Error(`zfb binary not found: ${zfbBinary}`);
if (!fs.statSync(distDir).isDirectory()) throw new Error(`built dist directory not found: ${distDir}`);

const relativeOutput = path.relative(repoRoot, outputDir);
if (relativeOutput === "" || (!relativeOutput.startsWith("..") && !path.isAbsolute(relativeOutput))) {
  throw new Error(`output-dir must be outside repo-root: ${outputDir}`);
}
fs.mkdirSync(outputDir, { recursive: true });
if (fs.readdirSync(outputDir).length > 0) {
  throw new Error(`output-dir must be empty: ${outputDir}`);
}

// Resolve Playwright from the caller's repository root. This keeps the script
// usable from a separate worktree while relying on the root dev dependency.
const requireFromRepo = createRequire(path.join(repoRoot, "package.json"));
const { chromium } = requireFromRepo("@playwright/test");
const playwrightEntry = requireFromRepo.resolve("@playwright/test");
const playwrightPackage = JSON.parse(
  fs.readFileSync(path.join(path.dirname(playwrightEntry), "package.json"), "utf8"),
);

const propertyList = [
  "display",
  "position",
  "box-sizing",
  "margin-top",
  "margin-right",
  "margin-bottom",
  "margin-left",
  "padding-top",
  "padding-right",
  "padding-bottom",
  "padding-left",
  "border-top-width",
  "border-top-style",
  "border-top-color",
  "border-right-width",
  "border-right-style",
  "border-right-color",
  "border-bottom-width",
  "border-bottom-style",
  "border-bottom-color",
  "border-left-width",
  "border-left-style",
  "border-left-color",
  "border-radius",
  "width",
  "height",
  "min-width",
  "min-height",
  "max-width",
  "max-height",
  "flex-direction",
  "flex-wrap",
  "flex-grow",
  "flex-shrink",
  "flex-basis",
  "justify-content",
  "align-items",
  "align-content",
  "align-self",
  "place-content",
  "place-items",
  "grid-template-columns",
  "grid-template-rows",
  "grid-auto-columns",
  "grid-auto-rows",
  "grid-auto-flow",
  "gap",
  "row-gap",
  "column-gap",
  "color",
  "background-color",
  "opacity",
  "font-family",
  "font-size",
  "font-weight",
  "font-style",
  "line-height",
  "letter-spacing",
  "text-align",
  "text-decoration-line",
  "text-transform",
  "font-variant-numeric",
  "list-style-type",
  "cursor",
  "transition-property",
  "transition-duration",
];

function collectHtmlFiles(root) {
  const found = [];
  function walk(directory) {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      const full = path.join(directory, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.isFile() && entry.name.endsWith(".html")) found.push(full);
    }
  }
  walk(root);
  return found.sort((a, b) => a.localeCompare(b));
}

function pageInfo(htmlPath) {
  const relativeFile = path.relative(distDir, htmlPath).split(path.sep).join("/");
  if (relativeFile === "index.html") {
    return { slug: "home", sourceFile: relativeFile, path: "/" };
  }
  if (relativeFile === "404.html") {
    return { slug: "404", sourceFile: relativeFile, path: "/404.html" };
  }
  if (relativeFile.endsWith("/index.html")) {
    const route = relativeFile.slice(0, -"index.html".length);
    const segments = route.split("/").filter(Boolean);
    return { slug: segments.at(-1) ?? "home", sourceFile: relativeFile, path: `/${route}` };
  }
  const route = relativeFile.replace(/\.html$/, "");
  return { slug: route.replaceAll("/", "-"), sourceFile: relativeFile, path: `/${route}` };
}

function outsideTempOutput(root) {
  return !root.startsWith(repoRoot + path.sep) && root !== repoRoot;
}

function reserveFreePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close((error) => {
        if (error) reject(error);
        else resolve(address.port);
      });
    });
  });
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitForPreview(child, url, logPath, getPreviewError) {
  const deadline = Date.now() + 30_000;
  let lastError = "not attempted";
  while (Date.now() < deadline) {
    if (getPreviewError()) {
      throw new Error(`failed to start zfb preview: ${getPreviewError().message}; see ${logPath}`);
    }
    if (child.exitCode !== null) {
      throw new Error(`zfb preview exited with ${child.exitCode}; see ${logPath}`);
    }
    try {
      const response = await fetch(url);
      if (response.status < 500) return;
      lastError = `HTTP ${response.status}`;
    } catch (error) {
      lastError = error.message;
    }
    await delay(200);
  }
  throw new Error(`preview did not become ready at ${url}: ${lastError}; see ${logPath}`);
}

async function stopPreview(child) {
  if (child.exitCode !== null || child.signalCode !== null || child.pid === undefined) return;

  const waitForExit = (timeoutMs) =>
    new Promise((resolve) => {
      let settled = false;
      const finish = (exited) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        child.removeListener("exit", onExit);
        child.removeListener("error", onError);
        resolve(exited);
      };
      const onExit = () => finish(true);
      const onError = () => finish(true);
      const timer = setTimeout(() => finish(false), timeoutMs);
      child.once("exit", onExit);
      child.once("error", onError);
      if (child.exitCode !== null || child.signalCode !== null) finish(true);
    });

  const gracefulExit = waitForExit(5_000);
  child.kill("SIGINT");
  if (await gracefulExit) return;

  const terminatedExit = waitForExit(2_000);
  child.kill("SIGTERM");
  if (await terminatedExit) return;

  const forcedExit = waitForExit(2_000);
  child.kill("SIGKILL");
  if (!(await forcedExit)) throw new Error("zfb preview did not exit after SIGKILL");
}

async function main() {
  const htmlFiles = collectHtmlFiles(distDir);
  if (!htmlFiles.length) throw new Error(`no HTML pages found under ${distDir}`);
  const pages = htmlFiles.map(pageInfo);
  const slugs = pages.map((page) => page.slug);
  if (new Set(slugs).size !== slugs.length) {
    throw new Error(`page slug collision in built site: ${slugs.join(", ")}`);
  }

  const port = await reserveFreePort();
  const baseUrl = `http://127.0.0.1:${port}`;
  const serverLogPath = path.join(outputDir, "preview-server.log");
  const serverLog = fs.createWriteStream(serverLogPath);
  const serverLogClosed = finished(serverLog).catch(() => undefined);
  const child = spawn(
    zfbBinary,
    ["preview", "--host", "127.0.0.1", "--port", String(port)],
    { cwd: siteDir, stdio: ["ignore", "pipe", "pipe"] },
  );
  let previewError = null;
  child.once("error", (error) => {
    previewError = error;
  });
  child.stdout.pipe(serverLog);
  child.stderr.pipe(serverLog);

  let browser;
  let chromiumVersion = "unknown";
  const captures = [];
  const effectivePages = [];
  try {
    await waitForPreview(child, `${baseUrl}/`, serverLogPath, () => previewError);
    browser = await chromium.launch({ headless: true });
    chromiumVersion = browser.version();

    for (const page of pages) {
      let capturePath = page.path;
      if (page.slug === "404") {
        const directResponse = await fetch(`${baseUrl}${capturePath}`);
        if (!directResponse.ok) capturePath = "/__zfb_baseline_unknown_path__";
      }
      const effectivePage = { ...page, capturePath };
      effectivePages.push(effectivePage);

      for (const theme of ["light", "dark"]) {
        for (const width of [375, 768, 1280]) {
          const context = await browser.newContext({
            viewport: { width, height: 900 },
            deviceScaleFactor: 1,
            reducedMotion: "reduce",
          });
          const browserPage = await context.newPage();
          await browserPage.addInitScript(
            ({ key, value }) => {
              try {
                window.localStorage.setItem(key, value);
              } catch {
                // The script runs again on the real HTTP origin before paint.
              }
            },
            { key: "basic-blog:theme", value: theme },
          );

          const response = await browserPage.goto(`${baseUrl}${capturePath}`, {
            waitUntil: "networkidle",
            timeout: 30_000,
          });
          if (!response) throw new Error(`navigation returned no response for ${capturePath}`);

          const wantsDark = theme === "dark";
          await browserPage.waitForFunction(
            (expectedDark) => {
              const island = document.querySelector('[data-zfb-island="ThemeToggle"]');
              const button = island?.querySelector("button");
              const rootTheme = document.querySelector("[data-theme]")?.getAttribute("data-theme");
              return (
                island?.hasAttribute("data-zfb-island-mounted") &&
                button?.getAttribute("aria-pressed") === String(expectedDark) &&
                rootTheme === (expectedDark ? "dark" : "light")
              );
            },
            wantsDark,
            { timeout: 30_000 },
          );
          await browserPage.evaluate(async () => {
            await document.fonts.ready;
            await Promise.all(
              Array.from(document.images, (image) => image.decode().catch(() => undefined)),
            );
          });

          const baseName = `${page.slug}--${theme}--${width}`;
          const pngPath = path.join(outputDir, `${baseName}.png`);
          const jsonPath = path.join(outputDir, `${baseName}.json`);
          await browserPage.screenshot({
            path: pngPath,
            fullPage: true,
            animations: "disabled",
            scale: "css",
          });

          const snapshot = await browserPage.evaluate((properties) => {
            const elements = Array.from(document.querySelectorAll("*"), (element) => {
              const style = getComputedStyle(element);
              const rect = element.getBoundingClientRect();
              const ownText = Array.from(element.childNodes)
                .filter((node) => node.nodeType === Node.TEXT_NODE)
                .map((node) => node.textContent ?? "")
                .join(" ")
                .replace(/\s+/g, " ")
                .trim()
                .slice(0, 160);
              const computed = {};
              for (const property of properties) {
                computed[property] = style.getPropertyValue(property);
              }
              const pathParts = [];
              let current = element;
              while (current && current.nodeType === Node.ELEMENT_NODE) {
                const tag = current.tagName.toLowerCase();
                let sameTagIndex = 1;
                for (
                  let sibling = current.previousElementSibling;
                  sibling;
                  sibling = sibling.previousElementSibling
                ) {
                  if (sibling.tagName === current.tagName) sameTagIndex += 1;
                }
                pathParts.unshift(`${tag}[${sameTagIndex}]`);
                current = current.parentElement;
              }
              return {
                path: `/${pathParts.join("/")}`,
                tag: element.tagName.toLowerCase(),
                class: element.getAttribute("class"),
                ownText,
                boundingBox: {
                  x: rect.x,
                  y: rect.y,
                  top: rect.top,
                  right: rect.right,
                  bottom: rect.bottom,
                  left: rect.left,
                  width: rect.width,
                  height: rect.height,
                },
                computed,
              };
            });
            return { documentTitle: document.title, elements };
          }, propertyList);

          fs.writeFileSync(
            jsonPath,
            `${JSON.stringify(
              {
                page: { slug: page.slug, sourceFile: page.sourceFile, path: page.path, capturePath },
                theme,
                viewport: { width, height: 900, deviceScaleFactor: 1 },
                responseStatus: response.status(),
                ...snapshot,
              },
              null,
              2,
            )}\n`,
          );

          captures.push({
            page: page.slug,
            theme,
            width,
            png: path.basename(pngPath),
            json: path.basename(jsonPath),
            pngBytes: fs.statSync(pngPath).size,
            jsonBytes: fs.statSync(jsonPath).size,
          });
          await context.close();
        }
      }
    }
  } finally {
    if (browser) await browser.close();
    await stopPreview(child);
    serverLog.end();
    await serverLogClosed;
  }

  const revision = (() => {
    const result = spawnSync("git", ["-C", repoRoot, "rev-parse", "HEAD"], { encoding: "utf8" });
    return result.status === 0 ? result.stdout.trim() : "unknown";
  })();
  const pngTotalBytes = captures.reduce((sum, capture) => sum + capture.pngBytes, 0);
  const manifest = {
    revision,
    host: {
      hostname: os.hostname(),
      platform: os.platform(),
      osVersion: os.release(),
      architecture: os.arch(),
    },
    playwrightVersion: playwrightPackage.version,
    chromiumVersion,
    viewportSizes: [375, 768, 1280].map((width) => ({ width, height: 900, deviceScaleFactor: 1 })),
    themes: ["light", "dark"],
    propertyList,
    pageList: effectivePages,
    pageCount: effectivePages.length,
    captureCount: captures.length,
    captures,
    totalPngBytes: pngTotalBytes,
    outputDir,
    previewPort: port,
    previewServerLog: "preview-server.log",
  };
  fs.writeFileSync(path.join(outputDir, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);

  console.log(`Captured ${captures.length} screenshot/snapshot pairs for ${effectivePages.length} pages.`);
  console.log(`Total PNG bytes: ${pngTotalBytes}`);
  console.log(`Capture output: ${outputDir}`);
}

if (!outsideTempOutput(outputDir)) {
  throw new Error(`output-dir must be outside the repository: ${outputDir}`);
}

await main();
