#!/usr/bin/env node

import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const DIST_DIR = process.env.WIND_DOCS_DIST
  ? resolve(process.env.WIND_DOCS_DIST)
  : join(REPO_ROOT, "docs/dist");
const PORT = Number.parseInt(process.argv[2] ?? "4333", 10);

function normalizeBase(value = "/") {
  if (value === "" || value === "/") return "/";
  const segments = value.replace(/^\/+|\/+$/g, "").split("/");
  if (
    segments.some((segment) => !segment || segment === "." || segment === "..") ||
    segments.some((segment) => !/^[A-Za-z0-9._~-]+$/.test(segment))
  ) {
    throw new Error(`WIND_DOCS_BASE must be a local URL path, received ${JSON.stringify(value)}`);
  }
  return `/${segments.join("/")}/`;
}

const BASE_PATH = normalizeBase(process.env.WIND_DOCS_BASE);
const MIME = {
  ".avif": "image/avif",
  ".css": "text/css; charset=utf-8",
  ".gif": "image/gif",
  ".html": "text/html; charset=utf-8",
  ".ico": "image/x-icon",
  ".jpeg": "image/jpeg",
  ".jpg": "image/jpeg",
  ".js": "application/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".mjs": "application/javascript; charset=utf-8",
  ".otf": "font/otf",
  ".png": "image/png",
  ".svg": "image/svg+xml",
  ".ttf": "font/ttf",
  ".txt": "text/plain; charset=utf-8",
  ".webp": "image/webp",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
};

function safeJoin(relativePath) {
  const target = resolve(DIST_DIR, relativePath);
  if (target !== DIST_DIR && !target.startsWith(`${DIST_DIR}${sep}`)) return null;
  return target;
}

function routeFile(pathname) {
  let relativePath = pathname;
  if (BASE_PATH !== "/") {
    if (relativePath === BASE_PATH.slice(0, -1)) return { redirect: BASE_PATH };
    if (!relativePath.startsWith(BASE_PATH)) return null;
    relativePath = relativePath.slice(BASE_PATH.length);
  } else {
    relativePath = relativePath.replace(/^\/+/, "");
  }

  let decoded;
  try {
    decoded = decodeURIComponent(relativePath);
  } catch {
    return null;
  }
  if (decoded.split("/").some((segment) => segment === "..")) return null;

  const normalized = decoded === "" ? "index.html" : decoded;
  let target = safeJoin(normalized);
  if (!target) return null;

  if (existsSync(target) && statSync(target).isDirectory()) {
    target = join(target, "index.html");
  } else if (normalized.endsWith("/")) {
    target = join(DIST_DIR, `${normalized}index.html`);
  } else if (!extname(normalized) && !existsSync(target)) {
    const routeIndex = safeJoin(`${normalized}/index.html`);
    if (routeIndex) target = routeIndex;
  }

  return target;
}

if (!existsSync(DIST_DIR)) {
  console.error(`[wind-doc-preview] built docs dist not found: ${DIST_DIR}`);
  process.exit(1);
}

const server = createServer((request, response) => {
  let pathname;
  try {
    pathname = new URL(request.url, `http://127.0.0.1:${PORT}`).pathname;
  } catch {
    response.writeHead(400).end("Bad Request");
    return;
  }

  const target = routeFile(pathname);
  if (target && typeof target === "object" && target.redirect) {
    response.writeHead(308, { Location: target.redirect, "Cache-Control": "no-store" }).end();
    return;
  }
  if (target && target !== null && existsSync(target) && statSync(target).isFile()) {
    response.writeHead(200, {
      "Cache-Control": "no-store",
      "Content-Type": MIME[extname(target).toLowerCase()] ?? "application/octet-stream",
    });
    if (request.method === "HEAD") {
      response.end();
    } else {
      createReadStream(target).pipe(response);
    }
    return;
  }

  response.writeHead(404, {
    "Cache-Control": "no-store",
    "Content-Type": "text/plain; charset=utf-8",
  });
  response.end(`404 Not Found: ${pathname}`);
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`[wind-doc-preview] READY http://127.0.0.1:${PORT}${BASE_PATH}`);
});

process.on("SIGTERM", () => server.close());
process.on("SIGINT", () => server.close());
