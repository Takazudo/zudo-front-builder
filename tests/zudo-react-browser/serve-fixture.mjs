/**
 * Static server for the packed-SDK zudo-react browser fixture.
 *
 * Serves generated pages and compiled fixtures from tests/zudo-react-browser/dist/
 * at /, and the staged package's dist/ tree at /zfb-dist/. The latter keeps the
 * browser's import-map URLs and the staged export map pointed at the same files.
 */

import { createServer } from "node:http";
import { createReadStream, statSync } from "node:fs";
import { extname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const HARNESS_DIR = fileURLToPath(new URL(".", import.meta.url));
const GENERATED_DIR = join(HARNESS_DIR, "dist");
const SDK_DIST_DIR = join(HARNESS_DIR, "node_modules", "@takazudo", "zfb", "dist");
const portArg = process.argv[2] ?? "4342";
const PORT = Number(portArg);
if (!Number.isInteger(PORT) || PORT < 1 || PORT > 65535) {
  throw new Error(`Invalid port: ${portArg}`);
}

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "application/javascript; charset=utf-8",
  ".mjs": "application/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".map": "application/json; charset=utf-8",
};

function safeJoin(baseDir, relativePath) {
  const segments = relativePath.split(/[\\/]+/).filter(Boolean);
  const target = resolve(baseDir, ...segments);
  if (target !== baseDir && !target.startsWith(baseDir + sep)) return null;
  return target;
}

function tryServe(response, filepath) {
  try {
    const stat = statSync(filepath);
    if (!stat.isFile()) return false;
    response.writeHead(200, {
      "Content-Type": MIME[extname(filepath)] ?? "application/octet-stream",
      "X-Content-Type-Options": "nosniff",
    });
    createReadStream(filepath).pipe(response);
    return true;
  } catch {
    return false;
  }
}

function notFound(response, pathname) {
  response.writeHead(404, { "Content-Type": "text/plain; charset=utf-8" });
  response.end(`404 Not Found: ${pathname}`);
}

const server = createServer((request, response) => {
  let pathname;
  try {
    pathname = decodeURIComponent(new URL(request.url, `http://localhost:${PORT}`).pathname);
  } catch {
    response.writeHead(400, { "Content-Type": "text/plain; charset=utf-8" });
    response.end("400 Bad Request");
    return;
  }

  if (pathname.startsWith("/zfb-dist/")) {
    const target = safeJoin(SDK_DIST_DIR, pathname.slice("/zfb-dist/".length));
    if (target && tryServe(response, target)) return;
    notFound(response, pathname);
    return;
  }

  if (pathname === "/") pathname = "/index.html";
  const target = safeJoin(GENERATED_DIR, pathname.slice(1));
  if (target && tryServe(response, target)) return;
  notFound(response, pathname);
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`READY http://localhost:${PORT}`);
});

process.on("SIGTERM", () => server.close());
process.on("SIGINT", () => server.close());
