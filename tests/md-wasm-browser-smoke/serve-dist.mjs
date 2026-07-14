/**
 * Static server for the already-built md-wasm browser fixture. The server is
 * intentionally boring: it verifies the production artifacts as a normal
 * static host would, including the required .mjs and .wasm MIME types.
 */

import { createServer } from "node:http";
import { createReadStream, existsSync, statSync } from "node:fs";
import { dirname, extname, join, sep } from "node:path";
import { fileURLToPath } from "node:url";

const testRoot = dirname(fileURLToPath(import.meta.url));
const distDir = join(testRoot, "fixture-site", "dist");
const port = Number.parseInt(process.argv[2] ?? "4331", 10);

const MIME = {
  ".css": "text/css; charset=utf-8",
  ".html": "text/html; charset=utf-8",
  ".js": "application/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".map": "application/json; charset=utf-8",
  ".mjs": "application/javascript; charset=utf-8",
  ".wasm": "application/wasm",
};

function safeJoin(baseDir, relativePath) {
  const target = join(baseDir, relativePath);
  return target === baseDir || target.startsWith(baseDir + sep) ? target : null;
}

function serveFile(response, filePath) {
  try {
    if (statSync(filePath).isDirectory()) return false;
    response.writeHead(200, {
      "Content-Type": MIME[extname(filePath)] ?? "application/octet-stream",
    });
    createReadStream(filePath).pipe(response);
    return true;
  } catch {
    return false;
  }
}

if (!existsSync(distDir)) {
  console.error(`[md-wasm-browser-smoke] dist/ not found at ${distDir} — run zfb build first.`);
  process.exit(1);
}

const server = createServer((request, response) => {
  const url = new URL(request.url ?? "/", `http://localhost:${port}`);
  const pathname = url.pathname === "/" ? "/index.html" : decodeURIComponent(url.pathname);
  const target = safeJoin(distDir, pathname.slice(1));
  if (target && serveFile(response, target)) return;

  response.writeHead(404, { "Content-Type": "text/plain; charset=utf-8" });
  response.end(`404 Not Found: ${pathname}`);
});

server.listen(port, "127.0.0.1", () => {
  console.log(`READY http://localhost:${port}`);
});

process.on("SIGINT", () => server.close());
process.on("SIGTERM", () => server.close());
