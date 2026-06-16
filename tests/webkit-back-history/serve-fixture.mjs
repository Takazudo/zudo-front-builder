/**
 * Minimal static file server for the webkit-back-history Playwright fixture.
 *
 * Serves two roots under a single origin:
 *   /           -> tests/webkit-back-history/fixture/   (HTML pages + island-stubs.js)
 *   /dist/      -> packages/zfb-runtime/dist/            (built ESM client-router)
 *
 * The /dist/ mapping means relative imports inside the built dist tree resolve
 * correctly (e.g. dist/client-router/index.js imports ./events.js → /dist/client-router/events.js).
 *
 * Usage (called by the test:webkit-back pnpm script):
 *   node tests/webkit-back-history/serve-fixture.mjs [port]
 *
 * Prints "READY http://localhost:<port>" on stdout when listening.
 * Kill with SIGINT / SIGTERM (Playwright's webServer will handle that).
 */

import { createServer } from "node:http";
import { createReadStream, statSync } from "node:fs";
import { join, extname, sep } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = fileURLToPath(new URL(".", import.meta.url));
const REPO_ROOT = join(__dirname, "..", "..");

const FIXTURE_DIR = join(__dirname, "fixture");
const DIST_DIR = join(REPO_ROOT, "packages", "zfb-runtime", "dist");

const PORT = parseInt(process.argv[2] ?? "4321", 10);

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "application/javascript; charset=utf-8",
  ".mjs": "application/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".map": "application/json; charset=utf-8",
};

function mime(filepath) {
  return MIME[extname(filepath)] ?? "application/octet-stream";
}

// Join `rel` onto `baseDir` but reject anything that escapes it (path
// traversal, e.g. /dist/../../etc/passwd). `join` normalises the `..` segments,
// so a contained path always stays under `baseDir + sep`.
function safeJoin(baseDir, rel) {
  const target = join(baseDir, rel);
  if (target !== baseDir && !target.startsWith(baseDir + sep)) return null;
  return target;
}

function tryServe(res, filepath) {
  try {
    const stat = statSync(filepath);
    if (stat.isDirectory()) return false;
    res.writeHead(200, { "Content-Type": mime(filepath) });
    createReadStream(filepath).pipe(res);
    return true;
  } catch {
    return false;
  }
}

const server = createServer((req, res) => {
  const url = new URL(req.url, `http://localhost:${PORT}`);
  let pathname = decodeURIComponent(url.pathname);

  // /dist/* -> packages/zfb-runtime/dist/*
  if (pathname.startsWith("/dist/")) {
    const target = safeJoin(DIST_DIR, pathname.slice("/dist/".length));
    if (target && tryServe(res, target)) return;
  }

  // / or /foo.html -> fixture/*
  // Normalise bare / to /index.html
  if (pathname === "/") pathname = "/index.html";
  const fxTarget = safeJoin(FIXTURE_DIR, pathname.slice(1));
  if (fxTarget && tryServe(res, fxTarget)) return;

  res.writeHead(404, { "Content-Type": "text/plain" });
  res.end(`404 Not Found: ${pathname}`);
});

server.listen(PORT, "127.0.0.1", () => {
  // Playwright webServer looks for this exact line on stdout.
  console.log(`READY http://localhost:${PORT}`);
});

process.on("SIGTERM", () => server.close());
process.on("SIGINT", () => server.close());
