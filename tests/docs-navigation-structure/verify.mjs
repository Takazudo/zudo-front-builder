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
  storedExpansion: [],
  settle: [],
  knownDefects: [],
  navigationFailures: [],
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
// A cross-browser main-thread signal per document: a requestAnimationFrame
// loop records every frame gap; navigations start only after a quiet second
// (see settled()).
await page.addInitScript(() => {
  const frames = { prev: performance.now(), lastGap: performance.now(), maxGap: 0, gaps: [] };
  window.__verifyFrames = frames;
  const tick = (now) => {
    const gap = now - frames.prev;
    if (gap > 50) {
      frames.lastGap = now;
      frames.gaps.push([Math.round(frames.prev), Math.round(gap)]);
      if (frames.gaps.length > 40) frames.gaps.shift();
    }
    frames.maxGap = Math.max(frames.maxGap, gap);
    frames.prev = now;
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
});
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
// Each local-view check starts from fresh 06R scope storage, so the exact link
// list is deterministic: persisted expansion from earlier pages cannot open
// extra branches here, and a leaked or expanded subtree fails the assertion.
// Persistence itself is exercised by verifyBroaderTree().
const resetScopeState = () => page.evaluate(() => sessionStorage.removeItem("zd-sidebar-scope"));
const storedScope = () =>
  page.evaluate(() => JSON.parse(sessionStorage.getItem("zd-sidebar-scope") ?? "null"));
// The sidebar islands write zd-sidebar-scope after activation or navigation.
// Poll until a non-null value has stayed identical across two reads 250 ms
// apart; "unwritten" if nothing appears within the evidence window.
async function writtenScope(windowMs = 5000) {
  const deadline = Date.now() + windowMs;
  let last = null;
  while (Date.now() < deadline) {
    const current = JSON.stringify(await storedScope());
    if (current !== "null" && current === last) return JSON.parse(current);
    last = current;
    await page.waitForTimeout(250);
  }
  return "unwritten";
}
// A round's first hard load must not adopt state persisted by an earlier round
// (popstate leaves explicit expansion behind; see the asymmetry check below).
// Load, clear the key, then reload so the islands also start from fresh memory.
async function freshGoto(path) {
  await goto(path);
  await resetScopeState();
  await page.reload();
  await page.locator("main").waitFor();
  await page
    .locator(`${sidebar} [data-zfb-island="SidebarTree"][data-zfb-island-mounted]`)
    .waitFor({ state: "attached" });
  const state = await writtenScope();
  assert.notEqual(state, "unwritten", `fresh ${path}: sidebar never wrote its scope state`);
  assert.equal(state.selected, null, `fresh ${path} adopted a stored scope`);
  assert.deepEqual(state.expansion ?? {}, {}, `fresh ${path} adopted stored expansion`);
}
// The configured local view (06R baseline/Restore view) is exactly the scope
// root's members, all collapsed except the branch holding the current page. The
// root row is revealed by one Broaden, where a scope-root page is marked current.
async function assertScope(root, members, mobile = false, prefix = "/docs/") {
  const target = page.locator(mobile ? "[data-zd-mobile-sidebar]" : sidebar);
  if (mobile) {
    await page.getByRole("button", { name: /Open sidebar|サイドバーを開く/, exact: false }).click();
    await expect(target).not.toHaveAttribute("inert");
  }
  const localView = members.map((slug) => prefix + slug);
  await expect.poll(() => prefixedLinks(target, prefix)).toEqual(localView);
  const current = new URL(page.url()).pathname.replace(/\/$/, "");
  const currentLink = target.locator(`a[href="${current}"], a[href="${current}/"]`);
  if (localView.includes(current)) {
    await expect(currentLink).toHaveAttribute("aria-current", "page");
  } else if (prefix + root === current) {
    await expect(target.locator('[data-sidebar-tree-root] a[aria-current="page"]')).toHaveCount(0);
    await target.locator("[data-sidebar-broaden]").click();
    await expect.poll(async () => (await prefixedLinks(target, prefix))[0]).toBe(current);
    assert.equal(
      (await prefixedLinks(target, prefix)).filter((href) => href === current).length,
      1,
    );
    await expect(currentLink).toHaveAttribute("aria-current", "page");
    await target.locator("[data-sidebar-restore]").click();
    await expect.poll(() => prefixedLinks(target, prefix)).toEqual(localView);
  }
  if (mobile) await page.getByRole("button", { name: /Close sidebar|サイドバーを閉じる/ }).click();
}
// Wide EN pages such as zudo-wind/compatibility spend several seconds in
// compositor layerization after a swap, also on the pre-migration host
// (https://github.com/Takazudo/zudo-front-builder/issues/4156). A click landing
// between those tasks waits behind them, so navigations start only after the
// main thread has produced frames with no gap above 50 ms for 1000 ms.
// Liveness budget, derived from the measured distribution (old and new hosts,
// 1440px, 4 interleaved rounds): at most two consecutive stalls of <= 7.5 s
// each => 2 x 7.5 s = 15 s, plus the 1 s quiet window and 4 s margin = 20 s.
const SETTLE_BUDGET_MS = 20_000;
async function settled(label) {
  const started = Date.now();
  try {
    await page.waitForFunction(
      () => {
        const frames = window.__verifyFrames;
        const now = performance.now();
        return !!frames && now - frames.lastGap >= 1000 && now - frames.prev < 100;
      },
      null,
      { polling: 100, timeout: SETTLE_BUDGET_MS },
    );
  } catch (error) {
    const maxGap = await page
      .evaluate(() => window.__verifyFrames?.maxGap ?? null)
      .catch(() => null);
    throw new Error(
      `main thread not quiet within ${SETTLE_BUDGET_MS} ms before ${label} (max frame gap ${maxGap} ms; see #4156): ${error.message}`,
    );
  }
  const sample = await page.evaluate(() => {
    const frames = window.__verifyFrames;
    const max = Math.round(frames.maxGap);
    frames.maxGap = 0;
    return { maxFrameGapMs: max };
  });
  evidence.settle.push({ before: label, waitedMs: Date.now() - started, ...sample });
}
// Known defect, zudolab/zudo-doc#4547: below 1024px a soft navigation persists
// the visited page's ancestors, so after utilities → Flex & grid → grid the
// entry view on the next Wind entry page (compatibility) also shows the Utility
// reference groups and the Flex & grid family pages. The following soft
// navigation (Coming from Tailwind) is clean again. Asserted as the exact leaked list
// observed with zudo-doc 6.0.1, so it fails loudly when upstream changes either
// way (fixed, or leaking differently).
const KNOWN_4547_LEAK = [
  "zudo-wind/learn",
  "zudo-wind/utilities",
  ...WIND_UTILITY_GROUPS.flatMap((group) =>
    group.id === "flex-grid" ? [group.slug, ...group.members] : [group.slug],
  ),
  "zudo-wind/compatibility",
  "playground/design-system",
];
async function assertKnownMobileLeak(prefix, slug) {
  const drawer = page.locator("[data-zd-mobile-sidebar]");
  await page.getByRole("button", { name: /Open sidebar|サイドバーを開く/ }).click();
  await expect(drawer).not.toHaveAttribute("inert");
  await expect
    .poll(() => prefixedLinks(drawer, prefix))
    .toEqual(KNOWN_4547_LEAK.map((member) => prefix + member));
  await page.getByRole("button", { name: /Close sidebar|サイドバーを閉じる/ }).click();
  evidence.knownDefects.push({ issue: "zudolab/zudo-doc#4547", page: prefix + slug });
}
async function soft(path) {
  await settled(`soft ${path}`);
  await resetScopeState();
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
  const clickedAt = await page.evaluate(() => Math.round(performance.now()));
  await page.locator("#verification-link").click();
  try {
    await expect(page).toHaveURL(origin + path);
  } catch (error) {
    // Keep the main-thread timeline for diagnosis (#4156) before failing.
    evidence.navigationFailures.push({
      path,
      clickedAt,
      now: await page.evaluate(() => Math.round(performance.now())).catch(() => null),
      frameGaps: await page.evaluate(() => window.__verifyFrames?.gaps ?? null).catch(() => null),
    });
    throw error;
  }
  await page.waitForFunction(() => window.__structureSwap === true);
  await page.locator("#verification-link").evaluateAll((nodes) => nodes.forEach((n) => n.remove()));
  await page.locator("main").waitFor();
}
// A native traversal swaps asynchronously; assert only after the router's swap,
// as soft() does. WebKit's slower swap otherwise races the next assertion.
async function traverse(direction) {
  await settled(`traverse ${direction}`);
  await resetScopeState();
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
    await freshGoto("/docs/zudo-wind/");
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
    // Evidence only, not an assertion: zudo-doc 6.0.1 persists internal
    // expansion differently per viewport and per navigation kind
    // (https://github.com/zudolab/zudo-doc/issues/4547).
    // User-visible link lists stay strictly asserted by assertScope.
    const writtenAfterSoft = await writtenScope();
    const expansionAfterSoft =
      writtenAfterSoft === "unwritten" ? "unwritten" : writtenAfterSoft.expansion;
    await traverse("back");
    await assertScope(type.slug, type.members, width < 1024);
    await traverse("back");
    await assertScope(flex.slug, flex.members, width < 1024);
    await traverse("forward");
    await assertScope(type.slug, type.members, width < 1024);
    evidence.storedExpansion.push({
      width,
      soft: { page: "/docs/zudo-wind/utilities/font-size/", expansion: expansionAfterSoft },
      popstate: {
        page: `/docs/${type.slug}/`,
        expansion: await writtenScope().then((state) =>
          state === "unwritten" ? state : state.expansion,
        ),
      },
    });
  }
  for (const locale of ["", "/ja"])
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 1000 });
      const prefix = `${locale}/docs/`;
      await freshGoto(prefix + "zudo-wind/");
      // Leave the persisted drawer unopened across several distinct native scopes.
      await soft(prefix + "zudo-wind/utilities/");
      await soft(prefix + flex.slug + "/");
      await soft(prefix + "zudo-wind/utilities/grid/");
      await assertScope(flex.slug, flex.members, width < 1024, prefix);
      await soft(prefix + "zudo-wind/compatibility/");
      if (width >= 1024) await assertScope("zudo-wind", WIND_ENTRY_SLUGS, false, prefix);
      else await assertKnownMobileLeak(prefix, "zudo-wind/compatibility");
      // The leak does not survive the next soft navigation: exact entry view again.
      await soft(prefix + "zudo-wind/coming-from-tailwind/");
      await assertScope("zudo-wind", WIND_ENTRY_SLUGS, width < 1024, prefix);
      await soft(prefix + "zudo-wind/learn/");
      await assertScope("zudo-wind/learn", WIND_LEARN_SLUGS, width < 1024, prefix);
      // Isolated fresh start: the clean entry view at every width.
      await freshGoto(prefix + "zudo-wind/compatibility/");
      await assertScope("zudo-wind", WIND_ENTRY_SLUGS, width < 1024, prefix);
    }
  evidence.checks.push(
    "Soft cross-group navigation and history scopes: exact local views at 1440px; below 1024px the compatibility entry view right after utilities → Flex & grid → grid carries the known zudolab/zudo-doc#4547 leak (asserted exactly as a known defect) and the next soft navigation is exact again; clean entry view also checked from a fresh start",
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
