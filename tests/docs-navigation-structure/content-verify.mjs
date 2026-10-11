// @verification: #4079 native-host content acceptance; manager-owned browser lane.
import { chromium, expect, webkit } from "@playwright/test";
import assert from "node:assert/strict";
import { mkdirSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import {
  ARCHITECTURE_GROUPS,
  DOCS_CATEGORY_ORDER,
} from "../../docs/src/config/navigation-groups.mjs";

const origin = process.env.DOCS_VERIFY_ORIGIN ?? "http://127.0.0.1:4333";
const output = process.env.DOCS_VERIFY_OUTPUT ?? "/tmp/zfb-docs-verification";
mkdirSync(output, { recursive: true });
const evidence = {
  commit: execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim(),
  origin,
  checks: [],
  pager: [],
  search: [],
  errors: [],
  status: "running",
};
const browserName = process.env.DOCS_VERIFY_BROWSER ?? "chromium";
const browserType = { chromium, webkit }[browserName];
if (!browserType)
  throw new Error(`DOCS_VERIFY_BROWSER must be chromium or webkit, got ${browserName}`);
const browser = await browserType.launch();
evidence.browser = browserName;
const page = await browser.newPage({
  viewport: { width: 1440, height: 1000 },
  reducedMotion: "reduce",
});
page.on("pageerror", (error) => evidence.errors.push(error.message));
const normalize = (href) => new URL(href, origin).pathname.replace(/\/$/, "") || "/";
const route = (locale, slug) => `${locale}/docs/${slug}`;
async function goto(path) {
  const response = await page.goto(origin + path);
  assert(response?.ok(), `${path}: HTTP ${response?.status()}`);
  await expect(page.locator("main")).toBeVisible();
}
// WebKit throws SecurityError past 100 history.pushState/replaceState calls per
// 10 s, and the router makes ~4 per soft navigation. Pace scripted clicks under
// that budget; the router's silent URL desync on the throw is tracked separately.
let lastNavigation = 0;
async function paceNavigation() {
  const wait = lastNavigation + 450 - Date.now();
  if (wait > 0) await page.waitForTimeout(wait);
  lastNavigation = Date.now();
}
async function at(path) {
  await expect.poll(() => normalize(page.url())).toBe(normalize(path));
  await expect(page.locator("main")).toBeVisible();
}
async function hrefs(locator) {
  return (await locator.evaluateAll((nodes) => nodes.map((n) => n.getAttribute("href")))).map(
    normalize,
  );
}
async function selectTab(label) {
  const tab = page.getByRole("tab", { name: label, exact: true });
  await expect(tab).toBeVisible();
  await tab.click();
  await expect(tab).toHaveAttribute("aria-selected", "true");
  const value = await tab.getAttribute("data-tab-btn");
  assert(value, `${label} tab must identify its native value`);
  const panel = page.locator(`[data-tab-value=${JSON.stringify(value)}]`);
  await expect(panel).toBeVisible();
  return panel;
}
try {
  const chapters = ["introduction", "installation", "your-first-site", "project-structure"];
  for (const locale of ["", "/ja"]) {
    await goto(`${locale}/`);
    const blocks = page.locator("[data-site-nav] > div");
    await expect(blocks).toHaveCount(8);
    const roots = [];
    for (const block of await blocks.all())
      roots.push(normalize(await block.locator("a[href]").first().getAttribute("href")));
    assert.deepEqual(
      roots,
      DOCS_CATEGORY_ORDER.slice(0, 8).map((slug) => route(locale, slug)),
    );
    const rows = blocks.first().locator("[data-note-tray-row]");
    assert.deepEqual(
      await hrefs(rows),
      chapters.map((slug) => route(locale, `getting-started/${slug}`)),
    );
    evidence.checks.push(
      `${locale || "EN"}: eight native homepage blocks in order and four note-tray chapter rows`,
    );

    await goto(route(locale, "getting-started"));
    const indexRows = page.locator('main ol > li > a[href*="/getting-started/"]');
    assert.deepEqual(
      await hrefs(indexRows),
      chapters.map((slug) => route(locale, `getting-started/${slug}`)),
    );
    const ranks = await indexRows.locator(":scope > span:first-child").allTextContents();
    assert.deepEqual(
      ranks.map((s) => s.trim()),
      ["01", "02", "03", "04"],
    );
    await goto(route(locale, "getting-started/installation"));
    await expect(await selectTab("npm")).toContainText("npm install -D @takazudo/zfb");
    await expect(await selectTab("pnpm")).toContainText("pnpm add -D @takazudo/zfb");
    await expect(page.locator("#standalone-installation")).toHaveCount(1);
    for (const command of [
      "brew install Takazudo/tap/zfb",
      "ZFB_INSTALL",
      "ZFB_VERSION",
      "install.ps1",
      "install.sh",
      "zfb --version",
      "brew upgrade zfb",
    ])
      await expect(page.locator("main")).toContainText(command);
    await goto(route(locale, "getting-started/your-first-site"));
    const nodeFree = await selectTab("Node-free");
    for (const command of [
      "zfb new my-site --template node-free",
      "cd my-site",
      "zfb dev",
      "zfb build",
      "zfb preview",
      "zfb check --skip-tsc",
    ])
      await expect(nodeFree).toContainText(command);
    const basic = await selectTab("Basic blog");
    for (const command of [
      "pnpm create zfb@latest my-site",
      "pnpm zfb dev",
      "pnpm zfb build",
      "pnpm zfb preview",
    ])
      await expect(basic).toContainText(command);
    evidence.checks.push(
      `${locale || "EN"}: numbered flat chapter index, package-manager tabs, full standalone install, both first-site flows`,
    );

    const sequence = [
      "architecture",
      ...ARCHITECTURE_GROUPS.flatMap((g) => [g.slug, ...g.members]),
    ];
    await goto(route(locale, sequence[0]));
    for (let i = 0; i < sequence.length; i++) {
      await at(route(locale, sequence[i]));
      const pager = page.locator("[data-doc-pager]");
      const links = await hrefs(pager.locator("a[href]"));
      const expected = [];
      if (i > 0) expected.push(route(locale, sequence[i - 1]));
      if (i + 1 < sequence.length) expected.push(route(locale, sequence[i + 1]));
      // Section endpoints may link to another category; every internal edge must be exact.
      assert.deepEqual(
        links.filter((href) => href.startsWith(route(locale, "architecture"))),
        expected,
      );
      evidence.pager.push({ locale: locale || "en", slug: sequence[i], links });
      if (i + 1 < sequence.length) {
        const next = pager.locator("a[href]").filter({ hasText: locale ? /次/ : /Next/ });
        await expect(next).toHaveCount(1);
        assert.equal(normalize(await next.getAttribute("href")), route(locale, sequence[i + 1]));
        await paceNavigation();
        await next.click();
      }
    }
    evidence.checks.push(
      `${locale || "EN"}: native pager clicked through Architecture root, four overviews, and all 28 articles`,
    );
  }

  // Actual native search index and result navigation, not a source-file inventory.
  for (const { query, slug } of [
    { query: "Installation", slug: "getting-started/installation" },
    { query: "Routing", slug: "architecture/routing" },
    { query: "Grid", slug: "zudo-wind/utilities/grid" },
  ]) {
    await goto("/docs/getting-started/introduction");
    await page.locator("[data-open-search]").click();
    const dialog = page.locator("[data-search-dialog]");
    await expect(dialog).toBeVisible();
    await page.locator("[data-search-input]").fill(query);
    const result = page
      .locator(
        `[data-search-results] a[href="/docs/${slug}"], [data-search-results] a[href="/docs/${slug}/"]`,
      )
      .first();
    await expect(result).toBeVisible({ timeout: 15000 });
    const links = await hrefs(page.locator("[data-search-results] a[href]"));
    assert(
      !links.some((href) => /\/docs\/(concepts|install)(\/|$)/.test(href)),
      "retired search result",
    );
    evidence.search.push({ query, canonical: await result.getAttribute("href"), links });
    await result.click();
    await at(route("", slug));
    await expect(dialog).not.toBeVisible();
  }
  evidence.checks.push(
    "Native search returns and navigates to canonical Installation, Architecture Routing, and Grid",
  );

  await goto("/docs/architecture/routing");
  for (const mode of ["Dark", "Light"]) {
    const trigger = page.locator("header [data-zd-theme-menu]:visible > button").first();
    await expect(trigger).not.toHaveAttribute("aria-disabled", "true");
    await trigger.click();
    await page.getByRole("menuitemradio", { name: new RegExp(`^${mode}`) }).click();
    await expect(page.locator("html")).toHaveAttribute("data-theme", mode.toLowerCase());
  }
  const language = page.locator("header [data-language-switcher]:visible").first();
  await language.locator("[data-language-toggle]").click();
  const jaLink = language.locator('a[lang="ja"]');
  assert.equal(normalize(await jaLink.getAttribute("href")), "/ja/docs/architecture/routing");
  await jaLink.click();
  await at("/ja/docs/architecture/routing");
  await expect(page.locator("html")).toHaveAttribute("lang", "ja");
  await expect(language.locator('[lang="ja"][aria-current="page"]')).toHaveCount(1);
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await language.locator("[data-language-toggle]").click();
  await language.locator('a[lang="en"]').click();
  await at("/docs/architecture/routing");
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await goto("/docs/architecture/render-artifacts");
  const fallbackTitle = await page.locator("main h1").textContent();
  await goto("/ja/docs/architecture/render-artifacts");
  await expect(page.locator("html")).toHaveAttribute("lang", "ja");
  await expect(page.locator("main h1")).toHaveText(fallbackTitle);
  await language.locator("[data-language-toggle]").click();
  assert.equal(
    normalize(await language.locator('a[lang="en"]').getAttribute("href")),
    "/docs/architecture/render-artifacts",
  );
  evidence.checks.push(
    "Native dark/light controls, theme persistence, EN↔JA language links and active state, Japanese-shell English render-artifacts fallback",
  );
  assert.deepEqual(evidence.errors, []);
  evidence.status = "passed";
} catch (error) {
  evidence.status = "failed";
  evidence.failure = { message: error.message, stack: error.stack, url: page.url() };
  await page.screenshot({ path: `${output}/content-failure.png`, fullPage: true }).catch(() => {});
  throw error;
} finally {
  writeFileSync(`${output}/content.json`, JSON.stringify(evidence, null, 2));
  await browser.close();
}
console.log(
  JSON.stringify(
    { status: evidence.status, commit: evidence.commit, checks: evidence.checks },
    null,
    2,
  ),
);
