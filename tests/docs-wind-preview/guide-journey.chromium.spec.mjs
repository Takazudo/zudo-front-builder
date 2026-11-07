import { test, expect } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// Reading journey for the frontend developer guide (#4639): Getting Started →
// guide in one click, guide → each canonical detail page, and the fragments the
// guide and its detail pages are linked by.
const prefix = process.env.WIND_DOCS_BASE ?? "/";
const route = (slug, ja = false) => `${prefix}${ja ? "ja/" : ""}docs/${slug}`;
const strip = (href) => href.replace(/\/$/, "");
const GUIDE = "architecture/architecture-overview";
const DETAILS = [
  "getting-started/your-first-site",
  "architecture/three-ways-to-run-zfb",
  "architecture/ssr-on-a-worker",
  "guides/ssr-and-cloudflare-bindings",
  "architecture/islands",
  "guides/client-scripts",
  "architecture/routing",
  "architecture/client-side-routing",
  "zudo-react/overview",
  "zudo-react/hydration",
  "zudo-react/server-rendering",
  "zudo-wind/sources-and-candidates",
  "zudo-wind/configuration",
  "api/compile-css",
  "architecture/js-runtime",
];
const ENTRY_PAGES = [
  "getting-started",
  "getting-started/introduction",
  "architecture",
  "architecture/fundamentals",
  "build-with-zfb",
  "deploy-embed",
  "zudo-react",
  "zudo-wind",
];

async function sidebarOf(page, width) {
  if (width >= 1024) return page.locator("#desktop-sidebar");
  await page.getByRole("button", { name: /Open sidebar|サイドバーを開く/ }).click();
  const drawer = page.locator("[data-zd-mobile-sidebar]");
  await expect(drawer.locator("[data-sidebar-tree-root]")).toBeVisible();
  return drawer;
}
const guideLink = (scope, ja) =>
  scope.locator(`a[href="${route(GUIDE, ja)}/"], a[href="${route(GUIDE, ja)}"]`);

async function expectFragment(page, href) {
  const [path, rawHash] = href.split("#");
  await page.goto(path);
  if (rawHash) {
    const id = decodeURIComponent(rawHash);
    await expect(page.locator(`[id="${id.replace(/"/g, '\\"')}"]`), href).toHaveCount(1);
  }
}

for (const width of [1440, 390]) {
  for (const ja of [false, true]) {
    test(`frontend guide journey ${ja ? "JA" : "EN"} ${width}`, async ({ page, request }) => {
      await page.setViewportSize({ width, height: 950 });
      const failures = [];
      page.on("pageerror", (error) => failures.push(error.message));

      await page.goto(route("getting-started/introduction", ja));
      await guideLink(page.locator("main"), ja).first().click();
      await expect(page).toHaveURL(new RegExp(`${route(GUIDE, ja)}/?$`));
      const title = (await page.locator("main h1").textContent()).trim();
      expect(title.length).toBeGreaterThan(0);

      const side = await sidebarOf(page, width);
      await expect(guideLink(side.locator("[data-sidebar-tree-root]"), ja).first()).toHaveAttribute(
        "aria-current",
        "page",
      );
      if (width < 1024) await page.keyboard.press("Escape");

      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
        true,
      );
      const screenshotDir = process.env.WIND_DOCS_SCREENSHOT_DIR;
      if (screenshotDir) {
        mkdirSync(screenshotDir, { recursive: true });
        await page.screenshot({
          path: join(screenshotDir, `guide-${ja ? "ja" : "en"}-${width}.png`),
          fullPage: true,
        });
      }

      const hrefs = await page
        .locator("main article a[href], main a[href]")
        .evaluateAll((links) => [...new Set(links.map((a) => a.getAttribute("href")))]);
      const docHrefs = hrefs.filter((href) => href.startsWith(`${prefix}${ja ? "ja/" : ""}docs/`));
      for (const slug of DETAILS)
        expect(
          docHrefs.some((href) => strip(href.split("#")[0]) === route(slug, ja)),
          `guide links ${slug}`,
        ).toBe(true);

      if (width === 1440) {
        for (const href of docHrefs) {
          const response = await request.get(href.split("#")[0]);
          expect(response.status(), href).toBe(200);
        }
        for (const href of docHrefs.filter((h) => h.includes("#")))
          await expectFragment(page, href);
        await page.goto(route(GUIDE, ja));
      }

      if (width < 1024) {
        const drawer = await sidebarOf(page, width);
        await drawer.locator(`a[lang="${ja ? "en" : "ja"}"]`).click();
      } else {
        const lang = page.locator("header [data-language-switcher]:visible").first();
        await lang.locator("[data-language-toggle]").click();
        await lang.locator(`a[lang="${ja ? "en" : "ja"}"]`).click();
      }
      await expect(page.locator("html")).toHaveAttribute("lang", ja ? "en" : "ja");
      await expect(page).toHaveURL(new RegExp(`${route(GUIDE, !ja)}/?$`));
      expect(failures).toEqual([]);
    });
  }
}

test("entry pages link the guide in both locales", async ({ page }) => {
  for (const ja of [false, true])
    for (const slug of ENTRY_PAGES) {
      await page.goto(route(slug, ja));
      await expect(guideLink(page.locator("main"), ja).first(), slug).toBeVisible();
    }
});

// Fragments other pages, tests and package READMEs link to (inventory in #4639),
// plus the guide's pre-rewrite ids kept as compatibility anchors.
const RETAINED = {
  en: {
    [GUIDE]: [
      "build-time-vs-browser-the-two-classes-of-output",
      "what-runs-where",
      "the-hono-cloudflare-workers-portability-bet",
      "layering-summary",
    ],
    "architecture/islands": [
      "boundary-discovery-and-migration",
      "boundary-discovery-and-migration-factory-provided-targets",
      "boundary-discovery-and-migration-composition-seam-for-customisable-package-islands",
      "embedding-a-third-party-widget",
      "importing-text-with-raw",
      "module-workers",
      "import-meta-glob-support",
    ],
    "guides/ssr-and-cloudflare-bindings": [
      "dev-prod-parity-for-prerender-false",
      "importing-wasm-in-an-ssr-route",
      "importing-wasm-in-an-ssr-route-emitted-wasm-layout",
      "the-default-export-receives-props-not-the-request",
      "local-development",
    ],
    "architecture/ssr-on-a-worker": [
      "the-dispatch-flow",
      "the-dispatch-flow-the-in-worker-env-assets-probe-is-not-dead-code",
      "the-getcloudflarecontext-trick",
    ],
    "architecture/js-runtime": ["current-defaults-and-experiments"],
    "zudo-react/hydration": ["mount-from-a-host-you-own", "boundary-hydration"],
    "zudo-react/server-rendering": [
      "read-a-render-failure",
      "rendering-islands-outside-a-zfb-build",
    ],
    "zudo-wind/configuration": [
      "the-wind-key-manifest-path-resolution",
      "the-wind-key-source-exclusions-and-package-roots",
    ],
  },
  ja: {
    "architecture/islands": [
      "境界ターゲットの検出と移行",
      "境界ターゲットの検出と移行-ファクトリから渡すターゲット",
      "境界ターゲットの検出と移行-カスタマイズ可能なパッケージ島の合成境界",
      "サードパーティウィジェットの埋め込み",
      "raw-でテキストをインポートする",
      "module-worker",
      "import-meta-glob-のサポート",
    ],
    "guides/ssr-and-cloudflare-bindings": [
      "prerender-false-の-dev-と本番の同等性",
      "ssr-ルートで-wasm-をインポートする",
      "ssr-ルートで-wasm-をインポートする-生成される-wasm-レイアウト",
      "デフォルトエクスポートが受け取るのは-props-であり-request-ではない",
    ],
    "architecture/ssr-on-a-worker": [
      "ディスパッチフロー",
      "ディスパッチフロー-内側の-env-assets-プローブはデッドコードではない",
      "getcloudflarecontext-のトリック",
    ],
    "architecture/js-runtime": ["現在のデフォルトと実験"],
    "zudo-react/hydration": ["自分で所有するホストから-mount-する"],
    "zudo-react/server-rendering": ["zfb-ビルドの外で-island-をレンダリングする"],
    "zudo-wind/configuration": [
      "wind-キー-マニフェストパスの解決",
      "wind-キー-ソースの除外とパッケージルート",
    ],
  },
};

test("retained fragments resolve on the built pages", async ({ page }) => {
  for (const [locale, pages] of Object.entries(RETAINED))
    for (const [slug, ids] of Object.entries(pages)) {
      await page.goto(route(slug, locale === "ja"));
      for (const id of ids)
        await expect(page.locator(`[id="${id}"]`), `${locale} ${slug}#${id}`).toHaveCount(1);
    }
});
