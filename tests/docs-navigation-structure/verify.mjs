// @verification: issue #4079 actual-host acceptance; not automatically added to CI.
import { chromium, expect, webkit } from "@playwright/test";
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
page.on("pageerror", (e) => evidence.errors.push(e.message));
const goto = async (path) => {
  await page.goto(origin + path);
  await page.locator("main").waitFor();
};
const sidebar = "#desktop-sidebar";
const prefixedLinks = (target, prefix) =>
  target.locator("[data-sidebar-tree-root] a[href]").evaluateAll(
    (nodes, prefix) =>
      nodes
        .map((n) => n.getAttribute("href"))
        .filter((href) => href.startsWith(prefix))
        .map((href) => href.replace(/\/$/, "")),
    prefix,
  );
// Top-level rows of the rendered tree (one per configured root occurrence).
const topLevelLinks = (target, prefix) =>
  target.locator("[data-sidebar-tree-root] > div").evaluateAll(
    (rows, prefix) =>
      rows
        .map((row) => row.querySelector("a[href]")?.getAttribute("href") ?? "")
        .filter((href) => href.startsWith(prefix))
        .map((href) => href.replace(/\/$/, "")),
    prefix,
  );
// The configured local view (06R baseline/Restore view) is the scope root's
// members; the editorial root itself is revealed by the first Broaden. Expansion
// state persists across pages of one navigation context, so descendants of
// earlier-visited branches may also be open here.
async function assertScope(members, mobile = false, prefix = "/docs/") {
  const target = page.locator(mobile ? "[data-zd-mobile-sidebar]" : sidebar);
  if (mobile) {
    await page.getByRole("button", { name: /Open sidebar|サイドバーを開く/, exact: false }).click();
    await expect(target).not.toHaveAttribute("inert");
  }
  await expect
    .poll(() => topLevelLinks(target, prefix))
    .toEqual(members.map((slug) => prefix + slug));
  const current = new URL(page.url()).pathname.replace(/\/$/, "");
  if (members.some((slug) => prefix + slug === current)) {
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
// A native traversal swaps asynchronously; assert only after the router's swap,
// as soft() does. WebKit's slower swap otherwise races the next assertion.
async function traverse(direction) {
  await page.evaluate(() => {
    window.__structureSwap = false;
    document.addEventListener(
      "zfb:after-swap",
      () => {
        window.__structureSwap = true;
      },
      { once: true },
    );
  });
  await (direction === "back" ? page.goBack() : page.goForward());
  await page.waitForFunction(() => window.__structureSwap === true);
  await page.locator("main").waitFor();
}
// 06R (#4099): native Broaden / Show only this branch / Restore on real content.
async function verifyBroaderTree() {
  const surfaces = WIND_UTILITY_GROUPS.find((g) => g.id === "surfaces-borders");
  const typography = WIND_UTILITY_GROUPS.find((g) => g.id === "typography");
  const flexGroup = WIND_UTILITY_GROUPS.find((g) => g.id === "flex-grid");
  const ancestry = [surfaces.slug, "zudo-wind/utilities", "zudo-wind"];
  const labels = {
    "": {
      broaden: "Broaden tree",
      restore: "Restore current page’s local tree",
      focus: "Show only this branch",
    },
    "/ja": {
      broaden: "ツリーを広げる",
      restore: "現在のページのツリーに戻す",
      focus: "この枝だけを表示",
    },
  };
  // The client router and sidebar island must be live before history traversal.
  const hydrated = (p) =>
    p.locator(`${sidebar} [data-zfb-island="SidebarTree"][data-zfb-island-mounted]`).waitFor();
  const scopeState = (p) =>
    p.evaluate(() => JSON.parse(sessionStorage.getItem("zd-sidebar-scope") ?? "null"));
  for (const locale of ["", "/ja"]) {
    const context = await browser.newContext({
      viewport: { width: 1440, height: 1000 },
      reducedMotion: "reduce",
    });
    const tab = await context.newPage();
    tab.on("pageerror", (e) => evidence.errors.push(e.message));
    const prefix = `${locale}/docs/`;
    const side = tab.locator(sidebar);
    const text = labels[locale];
    const article = prefix + "zudo-wind/utilities/background/";
    // One direct load with the hash: Back to an entry made by a same-document
    // fragment navigation does not restore content (pre-existing router gap).
    const html = await (await tab.request.get(origin + article)).text();
    const hash = html.match(/<h2[^>]*\bid="?([^"\s>]+)/)[1];
    await tab.goto(origin + article + "#" + hash);
    await expect(side.locator("[data-sidebar-broaden]")).toHaveText("↑" + text.broaden);
    await expect
      .poll(() => prefixedLinks(side, prefix))
      .toEqual(surfaces.members.map((m) => prefix + m));
    await tab.evaluate(() => window.scrollTo(0, 400));
    const scroll = await tab.evaluate(() => window.scrollY);
    const tocLinks = await tab.locator("[data-zd-toc] a[href^='#']").count();
    const title = await tab.locator("main h1").textContent();
    const articleUnchanged = async () => {
      await expect(tab).toHaveURL(origin + article + "#" + hash);
      assert.equal(await tab.evaluate(() => window.scrollY), scroll);
      assert.equal(await tab.locator("[data-zd-toc] a[href^='#']").count(), tocLinks);
      assert.equal(await tab.locator("main h1").textContent(), title);
    };
    // Broaden one real editorial level per action; the root renders once; stop at the top.
    for (const [index, root] of ancestry.entries()) {
      await side.locator("[data-sidebar-broaden]").click();
      await expect.poll(async () => (await prefixedLinks(side, prefix))[0]).toBe(prefix + root);
      const links = await prefixedLinks(side, prefix);
      assert.equal(links.filter((href) => href === prefix + root).length, 1, `${root} once`);
      for (const deeper of ancestry.slice(0, index))
        assert(links.includes(prefix + deeper), `${root} keeps ${deeper}`);
      await expect(side.locator('a[aria-current="page"]')).toHaveAttribute(
        "href",
        new RegExp(`${article}?$`),
      );
      await articleUnchanged();
    }
    await expect(side.locator("[data-sidebar-scope-toolbar]")).toHaveCount(0);
    // Show only this branch, then Restore the configured local view.
    const typographyLabel = (
      await side.locator(`a[href^="${prefix}${typography.slug}"]`).first().innerText()
    ).trim();
    await side
      .getByRole("button", { name: `${text.focus}: ${typographyLabel}`, exact: true })
      .click();
    await expect
      .poll(() => prefixedLinks(side, prefix))
      .toEqual([typography.slug, ...typography.members].map((m) => prefix + m));
    await articleUnchanged();
    await side.locator("[data-sidebar-restore]").click();
    await expect
      .poll(() => prefixedLinks(side, prefix))
      .toEqual(surfaces.members.map((m) => prefix + m));
    await articleUnchanged();
    // Filter (including no results) is retained across scope actions.
    const filter = side.getByRole("textbox", { name: "Filter navigation" });
    const rows = await side
      .locator("[data-sidebar-tree-root] a[href]")
      .evaluateAll((nodes) =>
        nodes.map((n) => [n.getAttribute("href").replace(/\/$/, ""), n.textContent.trim()]),
      );
    const query = rows.find(([href]) => href.endsWith("/outline-width"))[1];
    await filter.fill(query);
    await expect
      .poll(() => prefixedLinks(side, prefix))
      .toEqual(
        rows
          .filter(([, label]) => label.toLowerCase().includes(query.toLowerCase()))
          .map(([href]) => href),
      );
    await side.locator("[data-sidebar-broaden]").click();
    await expect(filter).toHaveValue(query);
    await filter.fill("no-such-utility-zz");
    await expect.poll(() => prefixedLinks(side, prefix)).toEqual([]);
    await side.locator("[data-sidebar-restore]").click();
    await expect(filter).toHaveValue("no-such-utility-zz");
    await filter.fill("");
    await expect
      .poll(() => prefixedLinks(side, prefix))
      .toEqual(surfaces.members.map((m) => prefix + m));
    // Modified clicks open a native tab without changing this page or its tree.
    const before = await prefixedLinks(side, prefix);
    const [popup] = await Promise.all([
      context.waitForEvent("page"),
      side
        .locator(`a[href^="${prefix}zudo-wind/utilities/radius"]`)
        .click({ modifiers: [process.platform === "darwin" ? "Meta" : "Control"] }),
    ]);
    await popup.waitForURL(/utilities\/radius\/?$/);
    await popup.close();
    await expect(tab).toHaveURL(origin + article + "#" + hash);
    assert.deepEqual(await prefixedLinks(side, prefix), before);
    // Same-branch native navigation keeps the local view; cross-branch keeps a containing scope.
    await side.locator("[data-sidebar-broaden]").click();
    await side.locator("[data-sidebar-broaden]").click();
    const swapped = tab.evaluate(
      () =>
        new Promise((resolve) =>
          document.addEventListener("zfb:after-swap", resolve, { once: true }),
        ),
    );
    await side.locator(`a[href^="${prefix}${flexGroup.slug}"]`).click();
    await swapped;
    await expect(tab).toHaveURL(new RegExp(`${prefix}${flexGroup.slug}/?$`));
    await expect
      .poll(async () => (await prefixedLinks(side, prefix))[0])
      .toBe(prefix + "zudo-wind/utilities");
    await expect(side.locator("[data-sidebar-restore]")).toBeVisible();
    // Refresh keeps the stored scope for the same page; Back/Forward keep a valid scope.
    await tab.reload();
    await hydrated(tab);
    await expect
      .poll(async () => (await prefixedLinks(side, prefix))[0])
      .toBe(prefix + "zudo-wind/utilities");
    await traverseOn(tab, "back", origin + article + "#" + hash);
    await expect(side.locator('a[aria-current="page"]')).toHaveAttribute(
      "href",
      new RegExp(`${article}?$`),
    );
    await traverseOn(tab, "forward", new RegExp(`${flexGroup.slug}/?$`));
    await expect(side.locator('a[aria-current="page"]')).toHaveAttribute(
      "href",
      new RegExp(`${flexGroup.slug}/?$`),
    );
    // A stale stored occurrence falls back to the configured local view.
    const stored = await scopeState(tab);
    assert(stored?.context, "scope state stored");
    await tab.evaluate(
      (state) => sessionStorage.setItem("zd-sidebar-scope", JSON.stringify(state)),
      { ...stored, selected: "custom/999/0" },
    );
    await tab.reload();
    await hydrated(tab);
    await expect
      .poll(() => prefixedLinks(side, prefix))
      .toEqual(flexGroup.members.map((m) => prefix + m));
    assert.equal((await scopeState(tab)).selected, null);
    // A locale change is a different navigation context: the other locale starts locally.
    await side.locator("[data-sidebar-broaden]").click();
    const otherPrefix = locale ? "/docs/" : "/ja/docs/";
    await tab.goto(origin + otherPrefix + flexGroup.slug + "/");
    await tab.locator("main").waitFor();
    await expect
      .poll(() => prefixedLinks(side, otherPrefix))
      .toEqual(flexGroup.members.map((m) => otherPrefix + m));
    assert.notEqual((await scopeState(tab))?.context, stored.context);
    // Mobile drawer: scope actions keep it open; page navigation closes it.
    await tab.setViewportSize({ width: 390, height: 844 });
    await tab.goto(origin + article);
    await tab.locator("main").waitFor();
    await tab.getByRole("button", { name: /Open sidebar|サイドバーを開く/ }).click();
    const drawer = tab.locator("[data-zd-mobile-sidebar]");
    await expect(drawer).not.toHaveAttribute("inert");
    await drawer.locator("[data-sidebar-broaden]").click();
    await expect(
      tab.getByRole("button", { name: /Close sidebar|サイドバーを閉じる/ }),
    ).toBeVisible();
    await expect
      .poll(async () => (await prefixedLinks(drawer, prefix))[0])
      .toBe(prefix + surfaces.slug);
    await drawer.locator("[data-sidebar-restore]").click();
    await expect(drawer).not.toHaveAttribute("inert");
    await expect
      .poll(() => prefixedLinks(drawer, prefix))
      .toEqual(surfaces.members.map((m) => prefix + m));
    await drawer.locator(`a[href^="${prefix}zudo-wind/utilities/shadow"]`).click();
    await expect(tab).toHaveURL(new RegExp(`utilities/shadow/?$`));
    await expect(tab.getByRole("button", { name: /Open sidebar|サイドバーを開く/ })).toBeVisible();
    await context.close();
    evidence.checks.push(
      `${locale || "EN"} 06R Broaden/branch/Restore, filter, nav, history, stale scope, locale, modified link, mobile drawer`,
    );
  }
}
// History entries created before a reload belong to the earlier document, so
// traversal may be a full load rather than a router swap; wait on the URL.
async function traverseOn(tab, direction, url) {
  await (direction === "back" ? tab.goBack() : tab.goForward());
  await expect(tab).toHaveURL(url);
  await tab.locator("main").waitFor();
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
      await assertScope(group.members, width < 1024);
    }
    await traverse("back");
    await assertScope(type.members, width < 1024);
    await traverse("back");
    await assertScope(flex.members, width < 1024);
    await traverse("forward");
    await assertScope(type.members, width < 1024);
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
      await assertScope(flex.members, width < 1024, prefix);
      for (const slug of ["zudo-wind/compatibility", "zudo-wind/coming-from-tailwind"]) {
        await soft(prefix + slug + "/");
        await assertScope(WIND_ENTRY_SLUGS, width < 1024, prefix);
      }
      await soft(prefix + "zudo-wind/learn/");
      await assertScope(WIND_LEARN_SLUGS, width < 1024, prefix);
    }
  evidence.checks.push(
    "Direct/native soft cross-group navigation, unopened/reopened mobile drawer and history scopes",
  );
  await verifyBroaderTree();
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
