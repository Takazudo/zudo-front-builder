/**
 * Serves a real `zfb build` dist/ for the W-A06 local Playwright network
 * confirmation. The Rust build test can export its first build with
 * ZFB_WIND_REAL_BUILD_DIST; this server only serves that already-built tree.
 */

import { createServer } from "node:http";
import { createReadStream, existsSync, statSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { extname, join, resolve, sep } from "node:path";
import { verifyDistProof } from "./dist-proof.mjs";

const DIST_DIR = process.env.ZFB_WIND_REAL_BUILD_DIST
  ? resolve(process.env.ZFB_WIND_REAL_BUILD_DIST)
  : null;
const PORT = Number.parseInt(process.argv[2] ?? "4332", 10);

const MIME = {
  ".css": "text/css; charset=utf-8",
  ".html": "text/html; charset=utf-8",
  ".js": "application/javascript; charset=utf-8",
  ".mjs": "application/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
  ".ttf": "font/ttf",
  ".otf": "font/otf",
};

function safeJoin(baseDir, relativePath) {
  const target = join(baseDir, relativePath);
  if (target !== baseDir && !target.startsWith(`${baseDir}${sep}`)) return null;
  return target;
}

function tryServe(response, filepath) {
  try {
    const stat = statSync(filepath);
    if (!stat.isFile()) return false;
    response.writeHead(200, {
      "Content-Type": MIME[extname(filepath).toLowerCase()] ?? "application/octet-stream",
      "Content-Length": stat.size,
    });
    createReadStream(filepath).pipe(response);
    return true;
  } catch {
    return false;
  }
}

if (!DIST_DIR) {
  console.error("[wind-real-build] set ZFB_WIND_REAL_BUILD_DIST to a built dist/ directory.");
  process.exit(1);
}
if (!existsSync(DIST_DIR)) {
  console.error(`[wind-real-build] dist/ not found at ${DIST_DIR}; build wind-assets first.`);
  process.exit(1);
}
if (!process.env.ZFB_WIND_REAL_BUILD_PROOF)
  throw Error("Set ZFB_WIND_REAL_BUILD_PROOF to current-source dist provenance");
await verifyDistProof(
  JSON.parse(await readFile(resolve(process.env.ZFB_WIND_REAL_BUILD_PROOF), "utf8")),
  DIST_DIR,
);

const server = createServer((request, response) => {
  const url = new URL(request.url, `http://localhost:${PORT}`);
  let pathname = decodeURIComponent(url.pathname);
  if (pathname === "/") pathname = "/index.html";
  const target = safeJoin(DIST_DIR, pathname.slice(1));
  if (target && tryServe(response, target)) return;
  response.writeHead(404, { "Content-Type": "text/plain; charset=utf-8" });
  response.end(`404 Not Found: ${pathname}`);
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`READY http://localhost:${PORT}`);
});

process.on("SIGTERM", () => server.close());
process.on("SIGINT", () => server.close());
