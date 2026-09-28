import { createServer } from "node:http";
import { createReadStream, statSync } from "node:fs";
import { extname, join, sep } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = fileURLToPath(new URL(".", import.meta.url));
const FIXTURE_DIR = join(__dirname, "fixtures");
const DIST_DIR = join(__dirname, "dist");
const PORT = Number.parseInt(process.argv[2] ?? "4341", 10);

const MIME = {
  ".css": "text/css; charset=utf-8",
  ".html": "text/html; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".js": "application/javascript; charset=utf-8",
  ".mjs": "application/javascript; charset=utf-8",
};

function safeJoin(root, relativePath) {
  const target = join(root, relativePath);
  if (target !== root && !target.startsWith(root + sep)) return null;
  return target;
}

function tryServe(response, filepath) {
  try {
    const stat = statSync(filepath);
    if (!stat.isFile()) return false;
    response.writeHead(200, {
      "Content-Type": MIME[extname(filepath)] ?? "application/octet-stream",
    });
    createReadStream(filepath).pipe(response);
    return true;
  } catch {
    return false;
  }
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

  const routes = [
    ["/fixtures/", FIXTURE_DIR],
    ["/dist/", DIST_DIR],
  ];
  for (const [prefix, root] of routes) {
    if (!pathname.startsWith(prefix)) continue;
    const target = safeJoin(root, pathname.slice(prefix.length));
    if (target && tryServe(response, target)) return;
  }

  response.writeHead(404, { "Content-Type": "text/plain; charset=utf-8" });
  response.end(`404 Not Found: ${pathname}`);
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`READY http://localhost:${PORT}`);
});

process.on("SIGTERM", () => server.close());
process.on("SIGINT", () => server.close());
