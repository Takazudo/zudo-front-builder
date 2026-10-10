import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import { resolve, join, extname, dirname } from "node:path";
import { pathToFileURL } from "node:url";
import assert from "node:assert/strict";
const tools = resolve(process.env.ZFB_PROTOTYPE_TOOLS ?? "target/rolldown-tools");
const { chromium } = await import(pathToFileURL(join(tools, "node_modules/playwright/index.mjs")));
const root = resolve(process.argv[2]);
const browser = await chromium.launch({ headless: true });
try {
  for (const backend of ["esbuild", "rolldown"]) {
    console.log(`Checking ${backend} runtime`);
    const dist = join(dirname(root), `dist-${backend}`);
    const { entry_url } = JSON.parse(
      readFileSync(join(dirname(root), `browser-${backend}.json`), "utf8"),
    );
    const entry = entry_url.split("/").at(-1);
    assert.match(entry, /^islands-[0-9a-f]{8}\.js$/);
    assert.ok(entry, "real production entry hash");
    const server = createServer((req, res) => {
      try {
        const path =
          req.url === "/" ? "index.html" : decodeURIComponent(req.url.slice(1).split("?")[0]);
        assert.ok(!path.includes(".."));
        let body = readFileSync(join(dist, path));
        if (path === "index.html")
          body = Buffer.from(
            body
              .toString()
              .replace("</body>", `<script type="module" src="/assets/${entry}"></script></body>`),
          );
        res.setHeader(
          "Content-Type",
          {
            ".html": "text/html",
            ".js": "text/javascript",
            ".mjs": "text/javascript",
            ".wasm": "application/wasm",
          }[extname(path)] ?? "application/octet-stream",
        );
        res.end(body);
      } catch {
        res.statusCode = 404;
        res.end("not found");
      }
    });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    try {
      const page = await browser.newPage();
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.goto(`http://127.0.0.1:${server.address().port}/`);
      await page.waitForSelector('[data-zfb-island="Counter"][data-zfb-island-mounted]');
      assert.equal(await page.locator("#counter").textContent(), "Count: 0");
      await page.locator("#counter").click();
      await page.waitForFunction(
        () => document.querySelector("#counter")?.textContent === "Count: 1",
      );
      assert.deepEqual(errors, []);
      await page.close();
      console.log(
        `${backend}: real Chromium hydration + split import + module worker + Wasm + glue after production rename PASS`,
      );
    } finally {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    }
  }
} finally {
  await browser.close();
}
