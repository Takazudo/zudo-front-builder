import { createServer } from "node:http";
import { createReadStream, existsSync, statSync } from "node:fs";
import { extname, join, sep } from "node:path";

const ARTIFACTS =
  process.env.ZFB_BOUNDARY_ACCEPTANCE_ARTIFACTS ??
  join(process.cwd(), "target", "scanner-boundary-acceptance");
const DIST = join(ARTIFACTS, "dist");
const PORT = Number.parseInt(process.argv[2] ?? "4324", 10);
const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "application/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
};

if (!existsSync(join(DIST, "index.html"))) {
  console.error(
    `[scanner-boundary-acceptance] no built index at ${DIST}; run the guarded Rust build fixture first.`,
  );
  process.exit(1);
}

function safeJoin(relative) {
  const target = join(DIST, relative);
  return target === DIST || target.startsWith(DIST + sep) ? target : null;
}

const server = createServer((request, response) => {
  const url = new URL(request.url ?? "/", `http://127.0.0.1:${PORT}`);
  const relative = decodeURIComponent(url.pathname === "/" ? "index.html" : url.pathname.slice(1));
  const filepath = safeJoin(relative);
  if (!filepath) {
    response.writeHead(400).end("Bad path");
    return;
  }
  try {
    if (statSync(filepath).isFile()) {
      response.writeHead(200, {
        "Content-Type": MIME[extname(filepath)] ?? "application/octet-stream",
      });
      createReadStream(filepath).pipe(response);
      return;
    }
  } catch {
    // A static fixture miss is an ordinary 404.
  }
  response.writeHead(404, { "Content-Type": "text/plain" }).end("Not found");
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`READY http://127.0.0.1:${PORT}`);
});
process.on("SIGTERM", () => server.close());
process.on("SIGINT", () => server.close());
