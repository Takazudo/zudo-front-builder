// @ts-check
import { readFileSync } from "node:fs";
import { createServer } from "node:http";

import { expect, test } from "@playwright/test";
import { WebSocketServer } from "ws";

const LIVERELOAD_JS = readFileSync(
  new URL("../../crates/zfb-server/src/livereload.js", import.meta.url),
);
const DETAIL_COUNT = 10;
const NAVIGATION_TIMEOUT_MS = 5000;
const CONNECTION_SETTLE_TIMEOUT_MS = 5000;
const MAX_LIVE_CONNECTIONS = 2;

test.use({
  // Playwright's default headless shell disables bfcache via its delegate.
  channel: "chromium",
  launchOptions: {
    ignoreDefaultArgs: ["--disable-back-forward-cache"],
  },
});

/** @type {ReturnType<typeof createHarness>} */
let harness;

test.beforeAll(async () => {
  harness = createHarness();
  await harness.start();
});

test.afterAll(async () => {
  await harness?.stop();
});

test("rapid Back walk keeps livereload connections bounded through bfcache", async ({ page }) => {
  test.setTimeout(60_000);

  const historyPaths = ["/index.html"];
  for (let index = 1; index <= DETAIL_COUNT; index += 1) {
    historyPaths.push(`/detail-${index}.html`);
  }

  await navigateToReadyPage(page, historyPaths[0]);
  for (const path of historyPaths.slice(1)) {
    await navigateToReadyPage(page, path);
  }

  for (let index = historyPaths.length - 2; index >= 0; index -= 1) {
    const path = historyPaths[index];
    const opensBeforeBack = harness.openCount;

    const startedAt = performance.now();
    await page.goBack({ waitUntil: "commit", timeout: NAVIGATION_TIMEOUT_MS });
    expect(performance.now() - startedAt).toBeLessThan(NAVIGATION_TIMEOUT_MS);

    await expect(page).toHaveURL(new URL(path, harness.origin).href, {
      timeout: NAVIGATION_TIMEOUT_MS,
    });
    await expect
      .poll(() => harness.openCount, {
        message: `${path} should open its own livereload stream after Back`,
        timeout: NAVIGATION_TIMEOUT_MS,
      })
      .toBeGreaterThan(opensBeforeBack);
  }

  await expect
    .poll(() => harness.liveConnectionCount, {
      message: "obsolete livereload streams should settle after the rapid Back walk",
      timeout: CONNECTION_SETTLE_TIMEOUT_MS,
    })
    .toBeLessThanOrEqual(MAX_LIVE_CONNECTIONS);

  expect(harness.peakLiveConnectionCount).toBeLessThanOrEqual(MAX_LIVE_CONNECTIONS);
  expect(harness.liveConnectionCount).toBeLessThanOrEqual(MAX_LIVE_CONNECTIONS);
  await expect
    .poll(() => harness.persistedPageShowCount, {
      message: "the Back walk must restore at least one document from bfcache",
      timeout: NAVIGATION_TIMEOUT_MS,
    })
    .toBeGreaterThan(0);
});

test("ten linked tabs finish loading while every tab keeps live reload", async ({
  page,
  context,
}) => {
  test.setTimeout(30_000);
  await navigateToReadyPage(page, "/index.html");
  const tabs = [];
  for (let index = 1; index <= DETAIL_COUNT; index += 1) {
    const opensBefore = harness.openCount;
    const opened = context.waitForEvent("page", { timeout: NAVIGATION_TIMEOUT_MS });
    await page.locator(`a[href="/detail-${index}.html"]`).click({
      modifiers: ["ControlOrMeta"],
      noWaitAfter: true,
    });
    const tab = await opened;
    tabs.push(tab);
    await tab.waitForLoadState("load", { timeout: NAVIGATION_TIMEOUT_MS });
    await expect.poll(() => harness.openCount).toBeGreaterThan(opensBefore);
  }
  await expect.poll(() => harness.liveConnectionCount).toBe(DETAIL_COUNT + 1);
  harness.broadcast("css");
  for (const tab of [page, ...tabs]) {
    await expect(tab.locator('link[rel="stylesheet"]')).toHaveAttribute("href", /\?v=\d+$/);
  }
  const beforeReconnect = harness.openCount;
  harness.disconnect();
  await expect.poll(() => harness.openCount).toBe(beforeReconnect + DETAIL_COUNT + 1);
  await expect.poll(() => harness.liveConnectionCount).toBe(DETAIL_COUNT + 1);
  const beforeReload = harness.openCount;
  harness.broadcast("page");
  await expect.poll(() => harness.openCount).toBe(beforeReload + DETAIL_COUNT + 1);
  // A fresh ordinary HTTP request must still have a connection available.
  expect(await page.evaluate(() => fetch("/index.html").then((r) => r.status))).toBe(200);
  for (const tab of tabs) await tab.close();
});

async function navigateToReadyPage(page, path) {
  const opensBeforeNavigation = harness.openCount;
  const startedAt = performance.now();

  await page.goto(new URL(path, harness.origin).href, {
    waitUntil: "commit",
    timeout: NAVIGATION_TIMEOUT_MS,
  });
  expect(performance.now() - startedAt).toBeLessThan(NAVIGATION_TIMEOUT_MS);

  await expect
    .poll(() => harness.openCount, {
      message: `${path} should open its own livereload stream before navigation continues`,
      timeout: NAVIGATION_TIMEOUT_MS,
    })
    .toBeGreaterThan(opensBeforeNavigation);
}

function createHarness() {
  const openSseResponses = new Set();
  const sockets = new Set();
  let openCount = 0;
  let liveConnectionCount = 0;
  let peakLiveConnectionCount = 0;
  let persistedPageShowCount = 0;
  let origin = "";

  const server = createServer((request, response) => {
    const requestUrl = new URL(request.url ?? "/", origin || "http://127.0.0.1");

    if (requestUrl.pathname === "/__zfb/livereload.js") {
      response.writeHead(200, { "content-type": "text/javascript; charset=utf-8" });
      response.end(LIVERELOAD_JS);
      return;
    }

    if (requestUrl.pathname === "/__zfb/reload") {
      openCount += 1;
      liveConnectionCount += 1;
      peakLiveConnectionCount = Math.max(peakLiveConnectionCount, liveConnectionCount);
      openSseResponses.add(response);

      let closed = false;
      response.on("close", () => {
        if (closed) return;
        closed = true;
        openSseResponses.delete(response);
        liveConnectionCount -= 1;
      });
      response.writeHead(200, {
        "cache-control": "no-cache",
        connection: "keep-alive",
        "content-type": "text/event-stream",
      });
      response.write(": connected\n\n");
      return;
    }

    if (requestUrl.pathname === "/style.css") {
      response.writeHead(200, { "content-type": "text/css" });
      response.end("body { color: black; }");
      return;
    }

    if (requestUrl.pathname === "/__harness/pageshow") {
      if (requestUrl.searchParams.get("persisted") === "true") {
        persistedPageShowCount += 1;
      }
      response.writeHead(204);
      response.end();
      return;
    }

    if (requestUrl.pathname === "/" || requestUrl.pathname === "/index.html") {
      serveHtml(response, fixtureHtml("/index.html"));
      return;
    }

    const detailMatch = requestUrl.pathname.match(/^\/detail-(\d+)\.html$/);
    const detailNumber = Number(detailMatch?.[1]);
    if (detailNumber >= 1 && detailNumber <= DETAIL_COUNT) {
      serveHtml(response, fixtureHtml(requestUrl.pathname));
      return;
    }

    response.writeHead(404);
    response.end("Not found");
  });

  const websocketServer = new WebSocketServer({ server, path: "/__zfb/reload/ws" });
  websocketServer.on("connection", (socket) => {
    openCount += 1;
    liveConnectionCount += 1;
    peakLiveConnectionCount = Math.max(peakLiveConnectionCount, liveConnectionCount);
    socket.on("close", () => {
      liveConnectionCount -= 1;
    });
  });

  server.on("connection", (socket) => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
  });

  return {
    get origin() {
      return origin;
    },
    get liveConnectionCount() {
      return liveConnectionCount;
    },
    get peakLiveConnectionCount() {
      return peakLiveConnectionCount;
    },
    get persistedPageShowCount() {
      return persistedPageShowCount;
    },
    get openCount() {
      return openCount;
    },
    broadcast(event, data = "{}") {
      for (const socket of websocketServer.clients) {
        socket.send(JSON.stringify({ event, data }));
      }
    },
    disconnect() {
      for (const socket of websocketServer.clients) socket.terminate();
    },
    async start() {
      await new Promise((resolve, reject) => {
        server.once("error", reject);
        server.listen(0, "127.0.0.1", resolve);
      });
      const address = server.address();
      if (!address || typeof address === "string") {
        throw new Error("livereload Back-navigation harness did not bind a TCP port");
      }
      origin = `http://127.0.0.1:${address.port}`;
    },
    async stop() {
      for (const socket of websocketServer.clients) socket.terminate();
      websocketServer.close();
      for (const response of openSseResponses) response.destroy();
      for (const socket of sockets) socket.destroy();
      await new Promise((resolve, reject) => {
        server.close((error) => (error ? reject(error) : resolve()));
      });
    },
  };
}

function serveHtml(response, html) {
  response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
  response.end(html);
}

function fixtureHtml(path) {
  const links = Array.from(
    { length: DETAIL_COUNT },
    (_, index) => `<a href="/detail-${index + 1}.html">Detail ${index + 1}</a>`,
  ).join("\n");

  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <title>${path}</title>
    <link rel="stylesheet" href="/style.css">
    <script>
      addEventListener("pageshow", (event) => {
        fetch("/__harness/pageshow?persisted=" + event.persisted, {
          method: "POST",
          keepalive: true,
        });
      });
    </script>
    <script src="/__zfb/livereload.js"></script>
  </head>
  <body>
    <h1>${path}</h1>
    <nav>${links}</nav>
  </body>
</html>`;
}
