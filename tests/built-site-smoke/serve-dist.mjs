/**
 * Minimal static file server for the built-site-smoke Playwright fixture
 * (issue #1401).
 *
 * Serves tests/built-site-smoke/fixture-site/dist/ — the real output of a
 * `zfb build` run against fixture-site/ — at the server root, the same
 * shape a plain static host serves a built zfb site under.
 *
 * dist/ must already exist when this starts: the CI workflow runs a real
 * `zfb build` in fixture-site/ as a separate step BEFORE invoking
 * `playwright test`. This script does not build anything itself — it is a
 * sibling of tests/router-chromium/serve-fixture.mjs, trimmed to a single
 * static root since there is no separate runtime dist/ to map in here (the
 * built site's own JS lives inside its own dist/ tree).
 *
 * Usage: node tests/built-site-smoke/serve-dist.mjs [port]
 * Prints "READY http://localhost:<port>" on stdout when listening.
 * Kill with SIGINT / SIGTERM (Playwright's webServer handles that).
 */

import { createServer } from "node:http";
import { createReadStream, existsSync, statSync } from "node:fs";
import { join, extname, sep } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = fileURLToPath(new URL(".", import.meta.url));
const DIST_DIR = join(__dirname, "fixture-site", "dist");

const PORT = parseInt(process.argv[2] ?? "4323", 10);

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
// traversal, e.g. /../../etc/passwd). `join` normalises the `..` segments,
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

if (!existsSync(DIST_DIR)) {
  console.error(
    `[serve-dist] dist/ not found at ${DIST_DIR} — run \`zfb build\` in ` +
      `tests/built-site-smoke/fixture-site/ before starting this server.`,
  );
  process.exit(1);
}

const server = createServer((req, res) => {
  const url = new URL(req.url, `http://localhost:${PORT}`);
  let pathname = decodeURIComponent(url.pathname);

  if (pathname === "/") pathname = "/index.html";
  const target = safeJoin(DIST_DIR, pathname.slice(1));
  if (target && tryServe(res, target)) return;

  res.writeHead(404, { "Content-Type": "text/plain" });
  res.end(`404 Not Found: ${pathname}`);
});

server.listen(PORT, "127.0.0.1", () => {
  // Playwright webServer looks for this exact line on stdout.
  console.log(`READY http://localhost:${PORT}`);
});

process.on("SIGTERM", () => server.close());
process.on("SIGINT", () => server.close());
