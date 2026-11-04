// Level 4: real Chromium clicks on rendered SVG <a> elements.
// HTMLElement.click() is not the oracle — SVG anchors are not HTML anchors.
import { test, expect } from "@playwright/test";

const XLINK = "http://www.w3.org/1999/xlink";

async function boot(page) {
  await page.goto("/source/page.html");
  await page.waitForFunction(() => history.state?.index !== undefined);
  await expect(page.locator("h1")).toHaveText("Source");
}

async function installLink(page, { base, href, xlink, target }) {
  await page.evaluate(
    ({ base, href, xlink, target, xlinkNs }) => {
      window.__svgSentinel = "retained";
      if (base !== undefined) {
        const baseEl = document.createElement("base");
        baseEl.setAttribute("href", base);
        document.head.append(baseEl);
      }
      const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      svg.setAttribute("width", "200");
      svg.setAttribute("height", "80");
      const anchor = document.createElementNS("http://www.w3.org/2000/svg", "a");
      if (href !== undefined) anchor.setAttribute("href", href);
      if (xlink !== undefined) anchor.setAttributeNS(xlinkNs, "href", xlink);
      if (target !== undefined) anchor.setAttribute("target", target);
      const rect = document.createElementNS("http://www.w3.org/2000/svg", "rect");
      rect.id = "svg-hit";
      rect.setAttribute("x", "10");
      rect.setAttribute("y", "10");
      rect.setAttribute("width", "180");
      rect.setAttribute("height", "60");
      rect.setAttribute("fill", "black");
      anchor.append(rect);
      svg.append(anchor);
      document.body.append(svg);
    },
    { base, href, xlink, target, xlinkNs: XLINK },
  );
}

test("base-relative SVG link reaches the base destination and keeps the document", async ({
  page,
}) => {
  await boot(page);
  await installLink(page, { base: "/docs/", href: "next.html" });
  await page.locator("#svg-hit").click();
  await page.waitForURL(/\/docs\/next\.html$/);
  await expect(page.locator("h1")).toHaveText("Docs Next");
  expect(await page.evaluate(() => window.__svgSentinel)).toBe("retained");
});

test("cross-origin base-relative SVG link stays native and does not router-fetch", async ({
  page,
}) => {
  const sameOriginFetches = [];
  page.on("request", (req) => {
    const url = new URL(req.url());
    if (url.origin === "http://localhost:4322" && url.pathname !== "/source/page.html") {
      sameOriginFetches.push(`${req.method()} ${url.pathname}`);
    }
  });
  await page.route("https://example.invalid/**", (route) =>
    route.fulfill({
      status: 200,
      contentType: "text/html",
      body: "<!doctype html><title>External</title><h1>External</h1>",
    }),
  );
  await boot(page);
  sameOriginFetches.length = 0;
  await installLink(page, { base: "https://example.invalid/docs/", href: "next.html" });
  await page.locator("#svg-hit").click();
  await page.waitForURL(/example\.invalid\/docs\/next\.html$/);
  await expect(page.locator("h1")).toHaveText("External");
  expect(sameOriginFetches).toEqual([]);
});

test("fragment-only SVG href under another base path does not stay on this document", async ({
  page,
}) => {
  await boot(page);
  await installLink(page, { base: "/search-results.html", href: "#section" });
  await page.locator("#svg-hit").click();
  await page.waitForURL(/\/search-results\.html#section$/);
  await expect(page.locator("h1")).toHaveText("Search Results");
  expect(await page.evaluate(() => window.__svgSentinel)).toBe("retained");
});

test("SVG link without a base resolves against the document URL", async ({ page }) => {
  await boot(page);
  await installLink(page, { href: "../search-results.html" });
  await page.locator("#svg-hit").click();
  await page.waitForURL(/\/search-results\.html$/);
  await expect(page.locator("h1")).toHaveText("Search Results");
  expect(await page.evaluate(() => window.__svgSentinel)).toBe("retained");
});

test("path-absolute SVG href ignores a document base", async ({ page }) => {
  await boot(page);
  await installLink(page, { base: "/docs/", href: "/search-results.html" });
  await page.locator("#svg-hit").click();
  await page.waitForURL(/\/search-results\.html$/);
  await expect(page.locator("h1")).toHaveText("Search Results");
  expect(await page.evaluate(() => window.__svgSentinel)).toBe("retained");
});

test("relative xlink:href resolves against the document base", async ({ page }) => {
  await boot(page);
  await installLink(page, { base: "/docs/", xlink: "from-xlink.html" });
  await page.locator("#svg-hit").click();
  await page.waitForURL(/\/docs\/from-xlink\.html$/);
  await expect(page.locator("h1")).toHaveText("From XLink");
  expect(await page.evaluate(() => window.__svgSentinel)).toBe("retained");
});

test("href wins over xlink:href", async ({ page }) => {
  await boot(page);
  await installLink(page, {
    base: "/docs/",
    href: "/search-results.html",
    xlink: "next.html",
  });
  await page.locator("#svg-hit").click();
  await page.waitForURL(/\/search-results\.html$/);
  await expect(page.locator("h1")).toHaveText("Search Results");
  expect(await page.evaluate(() => window.__svgSentinel)).toBe("retained");
});
