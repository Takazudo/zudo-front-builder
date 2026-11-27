// Serves a preserved `ssr-shared-assets` build through its real worker:
// `node serve.mjs <port> <variant> [base]` (#4887). The Rust e2e
// `crates/zfb/tests/ssr_shared_assets_e2e.rs` preserves the builds under
// `target/ssr-shared-assets/<variant>/dist`.
import { createServer } from "node:http";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { Readable } from "node:stream";
import { loadWorker, stripBase } from "./worker-host.mjs";

const [portArg, variant = "root", base = ""] = process.argv.slice(2);
const PORT = Number.parseInt(portArg ?? "4331", 10);
const ARTIFACTS =
  process.env.ZFB_SSR_SHARED_ASSETS_ARTIFACTS ?? join(process.cwd(), "target", "ssr-shared-assets");
const DIST = join(ARTIFACTS, variant, "dist");

if (!existsSync(join(DIST, "_worker.js"))) {
  console.error(
    `[ssr-shared-assets] no built worker at ${DIST}; run the ssr_shared_assets_e2e Rust test first.`,
  );
  process.exit(1);
}

const worker = await loadWorker(DIST);

const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url ?? "/", `http://127.0.0.1:${PORT}`);
    const routed = stripBase(url.pathname, base);
    if (routed === null) {
      res.writeHead(404, { "Content-Type": "text/plain" }).end("outside base");
      return;
    }
    const request = new Request(`http://127.0.0.1:${PORT}${routed}${url.search}`, {
      method: req.method,
      headers: req.headers,
    });
    const response = await worker.fetch(request);
    const headers = {};
    for (const [name, value] of response.headers) {
      if (name !== "set-cookie") headers[name] = value;
    }
    const cookies = response.headers.getSetCookie();
    if (cookies.length > 0) headers["set-cookie"] = cookies;
    res.writeHead(response.status, headers);
    if (response.body) {
      Readable.fromWeb(response.body).pipe(res);
    } else {
      res.end();
    }
  } catch (error) {
    res.writeHead(500, { "Content-Type": "text/plain" }).end(String(error?.stack ?? error));
  }
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`READY http://127.0.0.1:${PORT}`);
});
process.on("SIGTERM", () => server.close());
process.on("SIGINT", () => server.close());
