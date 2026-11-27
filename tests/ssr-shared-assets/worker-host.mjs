// Runs a built `dist/_worker.js` in Node with a small Workers Static Assets
// stand-in, so tests execute the real adapter wrapper and SSR bundle (#4887).
//
// The stand-in serves files from `dist/` (exact file, `<path>/index.html`, then
// `<path>.html`) and answers a plain 404 otherwise, which is what makes the
// wrapper fall through to the inner SSR router. A configured `base` is removed
// from the request path first, the way a deployment mounted at that base sees
// it.
import { readFile, stat } from "node:fs/promises";
import { extname, join, sep } from "node:path";
import { pathToFileURL } from "node:url";

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".xml": "application/xml; charset=utf-8",
};

export function stripBase(pathname, base) {
  const prefix = (base ?? "").replace(/\/+$/, "");
  if (prefix === "") return pathname;
  if (pathname === prefix) return "/";
  return pathname.startsWith(`${prefix}/`) ? pathname.slice(prefix.length) : null;
}

async function isFile(path) {
  try {
    return (await stat(path)).isFile();
  } catch {
    return false;
  }
}

export function createAssets(dist) {
  const root = join(dist);
  const inside = (path) => path === root || path.startsWith(root + sep);
  return {
    async fetch(request) {
      const { pathname } = new URL(request.url);
      const relative = decodeURIComponent(pathname).replace(/^\/+/, "");
      const candidates = pathname.endsWith("/")
        ? [join(root, relative, "index.html")]
        : [
            join(root, relative),
            join(root, relative, "index.html"),
            join(root, `${relative}.html`),
          ];
      for (const candidate of candidates) {
        if (!inside(candidate) || candidate.includes(`${sep}_worker.js`)) continue;
        if (candidate.endsWith(".mjs") && candidate.includes("_zfb_inner")) continue;
        if (await isFile(candidate)) {
          const body = request.method === "HEAD" ? null : await readFile(candidate);
          return new Response(body, {
            status: 200,
            headers: { "Content-Type": MIME[extname(candidate)] ?? "application/octet-stream" },
          });
        }
      }
      return new Response("Not found", {
        status: 404,
        headers: { "Content-Type": "text/plain; charset=utf-8" },
      });
    },
  };
}

export async function loadWorker(dist) {
  const worker = (await import(pathToFileURL(join(dist, "_worker.js")).href)).default;
  const env = { ASSETS: createAssets(dist) };
  const ctx = { waitUntil: () => undefined, passThroughOnException: () => undefined };
  return {
    fetch(request) {
      return worker.fetch(request, env, ctx);
    },
  };
}
