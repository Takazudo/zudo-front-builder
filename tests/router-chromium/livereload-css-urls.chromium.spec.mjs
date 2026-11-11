// @ts-check
// CSS live-reload keeps stylesheet URL meaning in a real browser (#4684).
//
// A local fixture server serves the real crates/zfb-server/src/livereload.js
// and a page with a <base> element. After a "css" event, each stylesheet's
// actual network request must keep its functional query parameters and carry
// the new `v` in the request query (the fragment never reaches the server).
import { readFileSync } from "node:fs";
import { createServer } from "node:http";

import { expect, test } from "@playwright/test";
import { WebSocketServer } from "ws";

const LIVERELOAD_JS = readFileSync(
  new URL("../../crates/zfb-server/src/livereload.js", import.meta.url),
);

/** @type {ReturnType<typeof createHarness>} */
let harness;

test.beforeAll(async () => {
  harness = createHarness();
  await harness.start();
});

test.afterAll(async () => {
  await harness?.stop();
});

test("css reload requests keep functional parameters and add v before the fragment", async ({
  page,
}) => {
  await page.goto(`${harness.origin}/site/page.html`);
  await expect.poll(() => harness.connections).toBeGreaterThan(0);
  for (const path of ["/theme.css", "/site/docs/local.css", "/fonts/css2", "/plain.css"]) {
    await expect.poll(() => harness.requestsFor(path).length, { message: path }).toBe(1);
  }

  harness.broadcast("css");

  const expectations = [
    { path: "/theme.css", params: { theme: ["dark"] } },
    { path: "/site/docs/local.css", params: { a: ["1", "2"] } },
    { path: "/fonts/css2", params: { family: ["Inter:wght@400"], display: ["swap"] } },
    { path: "/plain.css", params: {} },
  ];
  for (const { path, params } of expectations) {
    await expect.poll(() => harness.requestsFor(path).length, { message: path }).toBe(2);
    const reloaded = harness.requestsFor(path)[1];
    const v = reloaded.searchParams.getAll("v");
    expect(v, `${path} must carry exactly one new v`).toHaveLength(1);
    expect(v[0]).toMatch(/^\d+$/);
    expect(v[0]).not.toBe("old");
    for (const [name, values] of Object.entries(params)) {
      expect(reloaded.searchParams.getAll(name), `${path} ${name}`).toEqual(values);
    }
  }

  // The fragment is kept on the element, after the query that carries v.
  const hrefs = await page.$$eval('link[rel="stylesheet"]', (links) =>
    links.map((link) => link.getAttribute("href")),
  );
  expect(hrefs[0]).toMatch(/\/theme\.css\?theme=dark&v=\d+#sheet$/);
  expect(hrefs[1]).toMatch(/\/site\/docs\/local\.css\?a=1&a=2&v=\d+#frag$/);
});

function createHarness() {
  /** @type {URL[]} */
  const requests = [];
  let origin = "";
  let connections = 0;

  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", origin || "http://127.0.0.1");
    if (url.pathname === "/__zfb/livereload.js") {
      response.writeHead(200, { "content-type": "text/javascript; charset=utf-8" });
      response.end(LIVERELOAD_JS);
      return;
    }
    if (url.pathname === "/site/page.html") {
      const host = new URL(origin).host;
      response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      response.end(`<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <base href="/site/docs/">
    <title>css urls</title>
    <link rel="stylesheet" href="/theme.css?theme=dark&amp;v=old#sheet">
    <link rel="stylesheet" href="local.css?a=1&amp;a=2#frag">
    <link rel="stylesheet" href="//${host}/fonts/css2?family=Inter:wght@400&amp;display=swap">
    <link rel="stylesheet" href="/plain.css">
    <script src="/__zfb/livereload.js"></script>
  </head>
  <body><h1>css urls</h1></body>
</html>`);
      return;
    }
    if (url.pathname.endsWith(".css") || url.pathname === "/fonts/css2") {
      requests.push(url);
      response.writeHead(200, { "content-type": "text/css", "cache-control": "no-store" });
      response.end("body { color: black; }");
      return;
    }
    response.writeHead(404);
    response.end("Not found");
  });

  const websocketServer = new WebSocketServer({ server, path: "/__zfb/reload/ws" });
  websocketServer.on("connection", () => {
    connections += 1;
  });

  return {
    get origin() {
      return origin;
    },
    get connections() {
      return connections;
    },
    /** @param {string} path */
    requestsFor(path) {
      return requests.filter((url) => url.pathname === path);
    },
    /** @param {string} event */
    broadcast(event) {
      for (const socket of websocketServer.clients) {
        socket.send(JSON.stringify({ event, data: "" }));
      }
    },
    async start() {
      await new Promise((resolve, reject) => {
        server.once("error", reject);
        server.listen(0, "127.0.0.1", () => resolve(undefined));
      });
      const address = server.address();
      if (!address || typeof address === "string") {
        throw new Error("css livereload harness did not bind a TCP port");
      }
      origin = `http://127.0.0.1:${address.port}`;
    },
    async stop() {
      for (const socket of websocketServer.clients) socket.terminate();
      websocketServer.close();
      server.closeAllConnections();
      await new Promise((resolve, reject) => {
        server.close((error) => (error ? reject(error) : resolve(undefined)));
      });
    },
  };
}
