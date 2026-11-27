// Drives the built worker over a fixed request list and prints one JSON
// document on stdout: `node drive.mjs <dist> [base]` (#4887).
import { loadWorker, stripBase } from "./worker-host.mjs";

const [dist, base = ""] = process.argv.slice(2);
const worker = await loadWorker(dist);
const origin = "https://worker.test";

function headerObject(headers) {
  const out = {};
  for (const [name, value] of headers) {
    if (name !== "set-cookie") out[name] = value;
  }
  out["set-cookie"] = headers.getSetCookie();
  return out;
}

async function request(path, init = {}) {
  const routed = stripBase(path, base) ?? path;
  const response = await worker.fetch(new Request(`${origin}${routed}`, init));
  const bytes = new Uint8Array(await response.arrayBuffer());
  return {
    status: response.status,
    statusText: response.statusText,
    headers: headerObject(response.headers),
    body: new TextDecoder().decode(bytes),
    bytes: [...bytes],
  };
}

async function streamProbe(path) {
  const response = await worker.fetch(new Request(`${origin}${path}`));
  const reader = response.body.getReader();
  const timeout = new Promise((resolve) => setTimeout(() => resolve("timeout"), 5000));
  const first = await Promise.race([reader.read(), timeout]);
  const firstText = first === "timeout" ? "timeout" : new TextDecoder().decode(first.value);
  globalThis.__ssrSharedAssetsRelease?.();
  let rest = "";
  for (;;) {
    const next = await reader.read();
    if (next.done) break;
    rest += new TextDecoder().decode(next.value);
  }
  return { status: response.status, first: firstText, rest };
}

const paths = JSON.parse(process.env.SSR_SHARED_ASSETS_PATHS ?? "null") ?? [
  "/",
  "/live",
  "/plain",
  "/missing",
  "/string",
  "/feed.xml",
  "/api/raw-html",
  "/api/data",
  "/api/bytes",
  "/api/moved",
  "/api/empty",
  "/docs/guide/intro",
  "/verbatim",
  "/no-such-page",
];
const results = {};
for (const path of paths) {
  results[path] = await request(`${base.replace(/\/+$/, "")}${path}`);
}
const extra = {};
if (process.env.SSR_SHARED_ASSETS_HTTP !== "0") {
  extra.headLive = await request(`${base.replace(/\/+$/, "")}/live`, { method: "HEAD" });
  extra.headData = await request(`${base.replace(/\/+$/, "")}/api/data`, { method: "HEAD" });
  extra.postLive = await request(`${base.replace(/\/+$/, "")}/live`, { method: "POST" });
  extra.stream = await streamProbe("/api/stream");
}
process.stdout.write(JSON.stringify({ results, extra }));
