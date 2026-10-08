// @verification: issue #4079 actual-host acceptance; not automatically added to CI.
import { chromium, expect } from "@playwright/test";
import assert from "node:assert/strict";
import { mkdirSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import {
  ARCHITECTURE_GROUPS,
  WIND_UTILITY_GROUPS,
  COMPATIBILITY_BUCKETS,
  WIND_ENTRY_SLUGS,
  WIND_LEARN_SLUGS,
} from "../../docs/src/config/navigation-groups.mjs";
import { DOCS_ROUTE_MIGRATIONS } from "../../docs/src/config/route-migrations.mjs";
import { NAVIGATION_INDEX_MEMBERS } from "../../docs/scripts/sync-navigation-indexes.mjs";
const origin = process.env.DOCS_VERIFY_ORIGIN ?? "http://127.0.0.1:4333";
const output = process.env.DOCS_VERIFY_OUTPUT ?? "/tmp/zfb-docs-verification";
mkdirSync(output, { recursive: true });
const evidence = {
  commit: execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim(),
  origin,
  checks: [],
  geometry: [],
  errors: [],
};
const browser = await chromium.launch();
const page = await browser.newPage({
  viewport: { width: 1440, height: 1000 },
  reducedMotion: "reduce",
});
page.on("pageerror", (e) => evidence.errors.push(e.message));
const goto = async (path) => {
  await page.goto(origin + path);
  await page.locator("main").waitFor();
};
const sidebar = "#desktop-sidebar";
async function assertScope(root, members, mobile = false, prefix = "/docs/") {
  const target = page.locator(mobile ? "[data-zd-mobile-sidebar]" : sidebar);
  if (mobile) {
    await page.getByRole("button", { name: /Open sidebar|サイドバーを開く/, exact: false }).click();
    await expect(target).not.toHaveAttribute("inert");
  }
  await expect
    .poll(async () =>
      target.locator("a[href]").evaluateAll(
        (nodes, prefix) =>
          nodes
            .map((n) => n.getAttribute("href"))
            .filter((href) => href.startsWith(prefix))
            .map((href) => href.replace(/\/$/, "")),
        prefix,
      ),
    )
    .toEqual([root, ...members].map((slug) => prefix + slug));
  const current = new URL(page.url()).pathname.replace(/\/$/, "");
  if ([root, ...members].some((slug) => prefix + slug === current)) {
    await expect(target.locator(`a[href="${current}"], a[href="${current}/"]`)).toHaveAttribute(
      "aria-current",
      "page",
    );
  }
  if (mobile) await page.getByRole("button", { name: /Close sidebar|サイドバーを閉じる/ }).click();
}
async function soft(path) {
  await page.evaluate((path) => {
    window.__structureSwap = false;
    document.addEventListener(
      "zfb:after-swap",
      () => {
        window.__structureSwap = true;
      },
      { once: true },
    );
    const a = document.createElement("a");
    a.href = path;
    a.textContent = "verification navigation";
    a.id = "verification-link";
    document.querySelector("main").prepend(a);
  }, path);
  await page.locator("#verification-link").click();
  await expect(page).toHaveURL(origin + path);
  await page.waitForFunction(() => window.__structureSwap === true);
  await page.locator("#verification-link").evaluateAll((nodes) => nodes.forEach((n) => n.remove()));
  await page.locator("main").waitFor();
}
try {
  // Native emitted cards prove every shared ID resolves in both locale sources, including fallback.
  for (const locale of ["", "/ja"])
    for (const [slug, members] of NAVIGATION_INDEX_MEMBERS) {
      await goto(`${locale}/docs/${slug.replace(/\/index$/, "")}/`);
      const links = await page
        .locator("main a[href]")
        .evaluateAll((nodes) => nodes.map((n) => n.getAttribute("href")));
      for (const member of members)
        assert(
          links.some((href) => href.replace(/\/$/, "") === `${locale}/docs/${member}`),
          `${locale}/${slug} missing native card ${member}`,
        );
    }
  evidence.checks.push("All authored native card membership resolves in EN/JA");
  for (const locale of ["", "/ja"]) {
    await goto(locale + "/");
    const secondary = await page
      .locator("[data-home-secondary-nav] a")
      .evaluateAll((nodes) => nodes.map((n) => n.getAttribute("href")));
    assert.deepEqual(secondary, [`${locale}/docs/playground`, "/docs/changelog", "/docs/claude"]);
    evidence.checks.push(`${locale || "EN"} homepage secondary links`);
  }
  // Every actual Cloudflare local redirect, not an emulated mapping.
  for (const prefix of ["/docs", "/ja/docs"])
    for (const [from, to] of Object.entries(DOCS_ROUTE_MIGRATIONS))
      for (const slash of ["", "/"]) {
        const response = await page.request.get(origin + `${prefix}/${from}${slash}`, {
          maxRedirects: 0,
        });
        assert.equal(response.status(), 301, `${from}${slash}`);
        const [slug, hash] = to.split("#");
        assert.equal(
          new URL(response.headers().location, origin).pathname +
            new URL(response.headers().location, origin).hash,
          `${prefix}/${slug}/${hash ? "#" + hash : ""}`,
        );
      }
  evidence.checks.push(
    "200 explicit EN/JA slash/no-slash redirects return exact targets in local Cloudflare runtime",
  );
  for (const locale of ["", "/ja"]) {
    await goto(`${locale}/docs/zudo-wind/compatibility/m/`);
    await expect(page).toHaveURL(new RegExp("/compatibility/#compat-m$"));
    await expect(page.locator("#compat-m")).toBeInViewport();
  }
  const flex = WIND_UTILITY_GROUPS.find((g) => g.id === "flex-grid"),
    type = WIND_UTILITY_GROUPS.find((g) => g.id === "typography");
  for (const width of [1440, 390]) {
    await page.setViewportSize({ width, height: 1000 });
    await goto("/docs/zudo-wind/");
    for (const slug of [
      "zudo-wind/utilities",
      flex.slug,
      "zudo-wind/utilities/grid",
      type.slug,
      "zudo-wind/utilities/font-size",
    ]) {
      await soft("/docs/" + slug + "/");
      const group =
        slug.includes("font-size") || slug === type.slug
          ? type
          : slug === "zudo-wind/utilities"
            ? { slug, members: WIND_UTILITY_GROUPS.map((g) => g.slug) }
            : flex;
      await assertScope(group.slug, group.members, width < 1024);
    }
    await page.goBack();
    await assertScope(type.slug, type.members, width < 1024);
    await page.goBack();
    await assertScope(flex.slug, flex.members, width < 1024);
    await page.goForward();
    await assertScope(type.slug, type.members, width < 1024);
  }
  for (const locale of ["", "/ja"])
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 1000 });
      const prefix = `${locale}/docs/`;
      await goto(prefix + "zudo-wind/");
      // Leave the persisted drawer unopened across several distinct native scopes.
      await soft(prefix + "zudo-wind/utilities/");
      await soft(prefix + flex.slug + "/");
      await soft(prefix + "zudo-wind/utilities/grid/");
      await assertScope(flex.slug, flex.members, width < 1024, prefix);
      for (const slug of ["zudo-wind/compatibility", "zudo-wind/coming-from-tailwind"]) {
        await soft(prefix + slug + "/");
        await assertScope("zudo-wind", WIND_ENTRY_SLUGS, width < 1024, prefix);
      }
      await soft(prefix + "zudo-wind/learn/");
      await assertScope("zudo-wind/learn", WIND_LEARN_SLUGS, width < 1024, prefix);
    }
  evidence.checks.push(
    "Direct/native soft cross-group navigation, unopened/reopened mobile drawer and history scopes",
  );
  for (const width of [1440, 1280, 1024, 768, 390, 360])
    for (const locale of ["", "/ja"]) {
      await page.setViewportSize({ width, height: 1000 });
      for (const slug of ["zudo-wind/compatibility", "playground"]) {
        await goto(`${locale}/docs/${slug}/`);
        assert.equal(await page.locator("[data-zd-toc],.zd-desktop-toc-toggle").count(), 0);
        assert.equal(
          await page
            .getByText(locale ? "このページの内容" : "On this page", { exact: true })
            .count(),
          0,
        );
        const overflow = await page.evaluate(
          () => document.documentElement.scrollWidth > innerWidth + 1,
        );
        assert(!overflow, `${width}${locale}/${slug} overflow`);
        if (slug.includes("compatibility")) {
          assert.equal(await page.locator("main table").count(), 23);
          assert.equal(await page.locator("main tbody tr").count(), 1288);
          for (const bucket of COMPATIBILITY_BUCKETS)
            assert.equal(await page.locator("#compat-" + bucket).count(), 1);
          const scroll = await page
            .locator("main table")
            .first()
            .evaluate((t) => ({
              table: t.scrollWidth,
              parent: t.parentElement.clientWidth,
              overflow: getComputedStyle(t.parentElement).overflowX,
            }));
          if (width <= 390) assert(scroll.table > scroll.parent && scroll.overflow === "auto");
        }
        await page.screenshot({
          path: `${output}/${locale ? "ja" : "en"}-${slug.replaceAll("/", "-")}-${width}.png`,
        });
      }
      await goto(`${locale}/docs/playground/design-system/`);
      await page.locator('[data-view="playground"]').first().click();
      await expect(page.locator("#playground-view")).toBeVisible();
      const geometry = await page.locator(".workbench").evaluate((el) => {
        const box = (e) => {
          const r = e.getBoundingClientRect();
          return { x: r.x, y: r.y, width: r.width, height: r.height };
        };
        return {
          grid: getComputedStyle(el).gridTemplateColumns,
          children: [...el.children].map((e) => ({
            tag: e.tagName,
            class: e.className,
            ...box(e),
            gridColumn: getComputedStyle(e).gridColumn,
          })),
          before: getComputedStyle(el, "::before").content,
          after: getComputedStyle(el, "::after").content,
          overflow: document.documentElement.scrollWidth > innerWidth + 1,
        };
      });
      evidence.geometry.push({ width, locale: locale || "en", ...geometry });
      assert(!geometry.overflow, `workshop overflow ${width} ${locale}`);
      const controls = geometry.children.find((c) => c.class.includes("token-editor")),
        preview = geometry.children.find((c) => c.class.includes("preview-workspace"));
      assert(controls && preview);
      assert(controls.x <= preview.x && controls.y <= preview.y + 1);
      await page.screenshot({ path: `${output}/${locale ? "ja" : "en"}-workshop-${width}.png` });
    }
  evidence.checks.push(
    "All six widths EN/JA: compatibility complete/no TOC/table scrolling, wide Playground, workshop first-track/stacked geometry",
  );
  assert.deepEqual(evidence.errors, []);
} finally {
  writeFileSync(output + "/evidence.json", JSON.stringify(evidence, null, 2));
  await browser.close();
}
console.log(
  JSON.stringify(
    {
      commit: evidence.commit,
      checks: evidence.checks,
      geometryCases: evidence.geometry.length,
      errors: evidence.errors,
    },
    null,
    2,
  ),
);
