import { expect, test } from "@playwright/test";

const CONTENT_TYPE_BY_EXTENSION = {
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
};

function localCssUrls(css, stylesheetUrl) {
  const origin = new URL(stylesheetUrl).origin;
  return [...css.matchAll(/url\(([^)]*)\)/g)]
    .map((match) => match[1].trim().replace(/^['"]|['"]$/g, ""))
    .filter((value) => !value.startsWith("data:"))
    .map((value) => new URL(value, stylesheetUrl))
    .filter((url) => url.origin === origin)
    .map((url) => url.href);
}

test("W-A06: every local stylesheet asset is served and usable", async ({ page }) => {
  const responses = new Map();
  page.on("response", (response) => {
    const url = new URL(response.url());
    if (url.origin !== "http://localhost:4332") return;
    const extension = url.pathname.slice(url.pathname.lastIndexOf(".")).toLowerCase();
    if (CONTENT_TYPE_BY_EXTENSION[extension]) {
      responses.set(url.href, {
        status: response.status(),
        contentType: response.headers()["content-type"] ?? "",
      });
    }
  });

  await page.goto("/", { waitUntil: "networkidle" });
  await expect(page.locator("#project-image")).toBeVisible();

  const stylesheetHref = await page
    .locator('link[rel="stylesheet"]')
    .first()
    .evaluate((link) => link.href);
  expect(stylesheetHref).toContain("/assets/styles-");
  const stylesheet = await page.evaluate(async (href) => {
    const response = await fetch(href);
    return {
      status: response.status,
      contentType: response.headers.get("content-type") ?? "",
      css: await response.text(),
    };
  }, stylesheetHref);
  expect(stylesheet.status).toBe(200);
  expect(stylesheet.contentType).toContain("text/css");

  const missingAsset = await page.evaluate(async () => {
    const response = await fetch("/assets/__wind-real-build-missing.svg");
    return {
      status: response.status,
      contentType: response.headers.get("content-type") ?? "",
      body: await response.text(),
    };
  });
  expect(missingAsset.status).toBe(404);
  expect(missingAsset.contentType).toContain("text/plain");
  expect(missingAsset.body).not.toContain("<html");

  const urls = localCssUrls(stylesheet.css, stylesheetHref);
  const fontUrls = urls.filter((url) => url.toLowerCase().endsWith(".woff2"));
  const imageUrls = urls.filter((url) => url.toLowerCase().endsWith(".svg"));
  expect(fontUrls).toHaveLength(3);
  expect(imageUrls).toHaveLength(3);

  // Fetch through the browser so even an asset behind an unused selector is
  // checked as a real same-origin request, with its response MIME recorded.
  const fetched = await page.evaluate(async (assetUrls) => {
    return Promise.all(
      assetUrls.map(async (url) => {
        const response = await fetch(url);
        const bytes = await response.arrayBuffer();
        return {
          url,
          status: response.status,
          contentType: response.headers.get("content-type") ?? "",
          byteLength: bytes.byteLength,
        };
      }),
    );
  }, urls);

  for (const asset of fetched) {
    const extension = new URL(asset.url).pathname
      .slice(new URL(asset.url).pathname.lastIndexOf("."))
      .toLowerCase();
    expect(asset.status, asset.url).toBe(200);
    expect(asset.contentType, asset.url).toContain(CONTENT_TYPE_BY_EXTENSION[extension]);
    expect(asset.byteLength, asset.url).toBeGreaterThan(0);
    expect(responses.get(asset.url)?.status, `browser response for ${asset.url}`).toBe(200);
    expect(responses.get(asset.url)?.contentType, `browser MIME for ${asset.url}`).toContain(
      CONTENT_TYPE_BY_EXTENSION[extension],
    );
  }

  const loadedFonts = await page.evaluate(async () => {
    const families = ["Wind Project Fixture", "Wind Package Fixture", "Wind Nested Fixture"];
    return Promise.all(
      families.map(async (family) => {
        const faces = await document.fonts.load(`16px "${family}"`, "wind assets");
        return {
          family,
          loadedFaceCount: faces.length,
          checked: document.fonts.check(`16px "${family}"`, "wind assets"),
        };
      }),
    );
  });
  for (const font of loadedFonts) {
    expect(font.loadedFaceCount, font.family).toBeGreaterThan(0);
    expect(font.checked, font.family).toBe(true);
  }

  const imageWidths = await page.evaluate(async (assetUrls) => {
    return Promise.all(
      assetUrls.map(
        (url) =>
          new Promise((resolve, reject) => {
            const image = new Image();
            image.onload = () => resolve({ url, naturalWidth: image.naturalWidth });
            image.onerror = () => reject(new Error(`image failed to load: ${url}`));
            image.src = url;
          }),
      ),
    );
  }, imageUrls);
  for (const image of imageWidths) {
    expect(image.naturalWidth, image.url).toBeGreaterThan(0);
  }
});
