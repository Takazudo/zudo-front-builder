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
  storage: [],
  settle: [],
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
const frameProbe = () => {
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
};
await page.addInitScript(frameProbe);
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
// Membership checks keep soft()/traverse()'s default `reset`: each transition
// starts from empty 06R scope storage, isolating its exact link list.
// Persistence itself is proven by the uninterrupted sequence and by
// verifyExplicitDisclosure(), which never clear storage between transitions.
const SCOPE_KEY = "zd-sidebar-scope";
const resetScopeState = (p = page) =>
  p.evaluate((key) => sessionStorage.removeItem(key), SCOPE_KEY);
const storedScope = (p = page) =>
  p.evaluate((key) => JSON.parse(sessionStorage.getItem(key) ?? "null"), SCOPE_KEY);
// zudo-doc 6.1 writes zd-sidebar-scope (version 2) after activation and after
// each navigation; poll until the record names the settled destination.
async function scopeRecordFor(slug, p = page) {
  await expect
    .poll(async () => (await storedScope(p))?.page, { message: `scope record for ${slug}` })
    .toBe(slug);
  const state = await storedScope(p);
  assert.equal(state.version, 2, `${slug}: scope record version`);
  return state;
}
const mounted = (p = page) =>
  p
    .locator(`${sidebar} [data-zfb-island="SidebarTree"][data-zfb-island-mounted]`)
    .waitFor({ state: "attached" });
const slugOf = (path) => path.replace(/^(\/ja)?\/docs\//, "").replace(/\/$/, "");
// Isolate a membership round once: clear the key on the current origin
// document, then hard-load the round's first page. That load must write a
// fresh version-2 record with no selection and no expansion.
async function isolatedGoto(path) {
  assert(page.url().startsWith(origin), "isolatedGoto needs a loaded origin document");
  await resetScopeState();
  await goto(path);
  await mounted();
  const state = await scopeRecordFor(slugOf(path));
  assert.equal(state.selected, null, `fresh ${path} adopted a stored scope`);
  assert.deepEqual(state.expansion, {}, `fresh ${path} adopted stored expansion`);
}
// The configured local view (06R node baseline/Restore view) is the linked
// scope root followed by its members, all collapsed except the branch holding
// the current page. The root appears exactly once and is current on its own
// page. The highest scope (Wind) hides the scope toolbar; elsewhere Broaden is
// offered and Restore is absent because the view is the authored baseline.
async function assertScope(root, members, mobile = false, prefix = "/docs/") {
  const target = page.locator(mobile ? "[data-zd-mobile-sidebar]" : sidebar);
  if (mobile) {
    await page.getByRole("button", { name: /Open sidebar|サイドバーを開く/, exact: false }).click();
    await expect(target).not.toHaveAttribute("inert");
  }
  const localView = [root, ...members].map((slug) => prefix + slug);
  await expect.poll(() => prefixedLinks(target, prefix)).toEqual(localView);
  const current = new URL(page.url()).pathname.replace(/\/$/, "");
  if (localView.includes(current))
    await expect(
      target.locator(`[data-sidebar-tree-root] :is(a[href="${current}"], a[href="${current}/"])`),
    ).toHaveAttribute("aria-current", "page");
  if (root === "zudo-wind")
    await expect(target.locator("[data-sidebar-scope-toolbar]")).toHaveCount(0);
  else await expect(target.locator("[data-sidebar-broaden]")).toBeVisible();
  await expect(target.locator("[data-sidebar-restore]")).toHaveCount(0);
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
async function settled(label, p = page) {
  const started = Date.now();
  try {
    await p.waitForFunction(
      () => {
        const frames = window.__verifyFrames;
        const now = performance.now();
        return !!frames && now - frames.lastGap >= 1000 && now - frames.prev < 100;
      },
      null,
      { polling: 100, timeout: SETTLE_BUDGET_MS },
    );
  } catch (error) {
    const maxGap = await p.evaluate(() => window.__verifyFrames?.maxGap ?? null).catch(() => null);
    throw new Error(
      `main thread not quiet within ${SETTLE_BUDGET_MS} ms before ${label} (max frame gap ${maxGap} ms; see #4156): ${error.message}`,
    );
  }
  const sample = await p.evaluate(() => {
    const frames = window.__verifyFrames;
    const max = Math.round(frames.maxGap);
    frames.maxGap = 0;
    return { maxFrameGapMs: max };
  });
  evidence.settle.push({ before: label, waitedMs: Date.now() - started, ...sample });
}
// Soft navigation through a real router link. `reset` clears scope storage
// first (membership isolation only); persistence checks pass `reset: false`.
async function soft(path, { reset = true, on = page } = {}) {
  await settled(`soft ${path}`, on);
  if (reset) await resetScopeState(on);
  await on.evaluate((path) => {
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
  const clickedAt = await on.evaluate(() => Math.round(performance.now()));
  await on.locator("#verification-link").click();
  // The router pushes the URL before it swaps, but Playwright learns of a
  // same-document navigation only when the renderer's main thread is free. On
  // the wide compatibility page a multi-second stall can sit between pushState
  // and the swap (#4156; recorded pushState 0.87 s after the click, then a
  // 6.1 s stall), so wait for the router's swap and then require the exact URL.
  try {
    await on.waitForFunction(() => window.__structureSwap === true);
    await expect(on).toHaveURL(origin + path);
  } catch (error) {
    // Keep the main-thread timeline for diagnosis (#4156) before failing.
    evidence.navigationFailures.push({
      path,
      clickedAt,
      now: await on.evaluate(() => Math.round(performance.now())).catch(() => null),
      frameGaps: await on.evaluate(() => window.__verifyFrames?.gaps ?? null).catch(() => null),
    });
    throw error;
  }
  await on.locator("#verification-link").evaluateAll((nodes) => nodes.forEach((n) => n.remove()));
  await on.locator("main").waitFor();
}
// A native traversal swaps asynchronously; assert only after the router's swap,
// as soft() does. WebKit's slower swap otherwise races the next assertion.
async function traverse(direction, { reset = true } = {}) {
  await settled(`traverse ${direction}`);
  if (reset) await resetScopeState();
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
// A sidebar row (`div` holding the link) for one rendered category node; its
// toggle carries aria-expanded and its focus control the native occurrence ID.
const rowOf = (target, href) =>
  target.locator(`[data-sidebar-tree-root] div:has(> a[href="${href}/"], > a[href="${href}"])`);
const toggleOf = (target, href) => rowOf(target, href).locator(":scope > button[aria-expanded]");
const occurrenceOf = (target, href) =>
  rowOf(target, href)
    .locator(":scope > [data-sidebar-focus]")
    .getAttribute("data-sidebar-focus-scope");
// 06R (#4099): native Broaden / Show only this branch / Restore on real content.
async function verifyBroaderTree() {
  const surfaces = WIND_UTILITY_GROUPS.find((g) => g.id === "surfaces-borders");
  const typography = WIND_UTILITY_GROUPS.find((g) => g.id === "typography");
  const flexGroup = WIND_UTILITY_GROUPS.find((g) => g.id === "flex-grid");
  // Background starts at its linked Surfaces & borders root; each Broaden is one
  // real editorial level above it.
  const ancestry = ["zudo-wind/utilities", "zudo-wind"];
  const surfacesView = [surfaces.slug, ...surfaces.members];
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
    const headingMatch = html.match(/<h2[^>]*\bid="?([^"\s>]+)/);
    assert(headingMatch, `${article}: no <h2 id> in the served HTML to use as the fragment target`);
    const hash = headingMatch[1];
    await tab.goto(origin + article + "#" + hash);
    await expect(side.locator("[data-sidebar-broaden]")).toHaveText("↑" + text.broaden);
    await expect
      .poll(() => prefixedLinks(side, prefix))
      .toEqual(surfacesView.map((m) => prefix + m));
    await expect(side.locator("[data-sidebar-restore]")).toHaveCount(0);
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
    // Broaden one real editorial level per action: the first click reaches Utility
    // reference, the second Wind. The root renders once; the toolbar hides at the top.
    for (const [index, root] of ancestry.entries()) {
      await side.locator("[data-sidebar-broaden]").click();
      await expect.poll(async () => (await prefixedLinks(side, prefix))[0]).toBe(prefix + root);
      const links = await prefixedLinks(side, prefix);
      assert.equal(links.filter((href) => href === prefix + root).length, 1, `${root} once`);
      for (const deeper of [surfaces.slug, ...ancestry.slice(0, index)])
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
      .toEqual(surfacesView.map((m) => prefix + m));
    await expect(side.locator("[data-sidebar-restore]")).toHaveCount(0);
    await articleUnchanged();
    // Filter (including no results) is retained across scope actions. The linked
    // root stays as the ancestor of matching family pages.
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
          .filter(
            ([href, label]) =>
              href === prefix + surfaces.slug || label.toLowerCase().includes(query.toLowerCase()),
          )
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
      .toEqual(surfacesView.map((m) => prefix + m));
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
    // Same-branch native navigation keeps the local view; cross-branch keeps a
    // containing scope. One Broaden from Background already reaches Utilities.
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
    // After navigation, Restore returns to the destination's configured root.
    await side.locator("[data-sidebar-restore]").click();
    await expect
      .poll(() => prefixedLinks(side, prefix))
      .toEqual([flexGroup.slug, ...flexGroup.members].map((m) => prefix + m));
    await expect(side.locator("[data-sidebar-restore]")).toHaveCount(0);
    assert.equal((await scopeState(tab)).selected, null);
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
      .toEqual([flexGroup.slug, ...flexGroup.members].map((m) => prefix + m));
    assert.equal((await scopeState(tab)).selected, null);
    // A locale change is a different navigation context: the other locale starts locally.
    await side.locator("[data-sidebar-broaden]").click();
    const otherPrefix = locale ? "/docs/" : "/ja/docs/";
    await tab.goto(origin + otherPrefix + flexGroup.slug + "/");
    await tab.locator("main").waitFor();
    await expect
      .poll(() => prefixedLinks(side, otherPrefix))
      .toEqual([flexGroup.slug, ...flexGroup.members].map((m) => otherPrefix + m));
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
      .toBe(prefix + "zudo-wind/utilities");
    await drawer.locator("[data-sidebar-restore]").click();
    await expect(drawer).not.toHaveAttribute("inert");
    await expect
      .poll(() => prefixedLinks(drawer, prefix))
      .toEqual(surfacesView.map((m) => prefix + m));
    await drawer.locator(`a[href^="${prefix}zudo-wind/utilities/shadow"]`).click();
    await expect(tab).toHaveURL(new RegExp(`utilities/shadow/?$`));
    await expect(tab.getByRole("button", { name: /Open sidebar|サイドバーを開く/ })).toBeVisible();
    await context.close();
    evidence.checks.push(
      `${locale || "EN"} 06R Broaden/branch/Restore, filter, nav, history, stale scope, locale, modified link, mobile drawer`,
    );
  }
}
// zudo-doc 6.1 explicit-only disclosure persistence (#4099), using occurrence IDs
// read from the rendered native controls. Storage is never cleared here.
async function verifyExplicitDisclosure() {
  const surfaces = WIND_UTILITY_GROUPS.find((g) => g.id === "surfaces-borders");
  const typography = WIND_UTILITY_GROUPS.find((g) => g.id === "typography");
  const background = "zudo-wind/utilities/background";
  const radius = "zudo-wind/utilities/radius";
  for (const locale of ["", "/ja"]) {
    const browserContext = await browser.newContext({
      viewport: { width: 1440, height: 1000 },
      reducedMotion: "reduce",
    });
    await browserContext.addInitScript(frameProbe);
    const tab = await browserContext.newPage();
    tab.on("pageerror", (e) => evidence.errors.push(e.message));
    const prefix = `${locale}/docs/`;
    const href = (slug) => prefix + slug;
    const side = tab.locator(sidebar);
    const filter = side.getByRole("textbox", { name: "Filter navigation" });
    const surfacesView = [surfaces.slug, ...surfaces.members].map(href);
    const setRecord = (value) =>
      tab.evaluate(
        ([key, value]) => sessionStorage.setItem(key, JSON.stringify(value)),
        [SCOPE_KEY, value],
      );
    await tab.goto(origin + href(background) + "/");
    await mounted(tab);
    let state = await scopeRecordFor(background, tab);
    await expect.poll(() => prefixedLinks(side, prefix)).toEqual(surfacesView);
    const labels = new Map(
      await side
        .locator("[data-sidebar-tree-root] a[href]")
        .evaluateAll((nodes) =>
          nodes.map((n) => [n.getAttribute("href").replace(/\/$/, ""), n.textContent.trim()]),
        ),
    );
    // Rows the native filter keeps from the Surfaces baseline: the root as the
    // ancestor of matching family pages, or the whole branch when it matches itself.
    const filteredBaseline = (query) => {
      const match = (h) => labels.get(h).toLowerCase().includes(query.toLowerCase());
      if (match(surfacesView[0])) return surfacesView;
      const members = surfacesView.slice(1).filter(match);
      return members.length ? [surfacesView[0], ...members] : [];
    };
    const surfacesId = await occurrenceOf(side, href(surfaces.slug));
    // Showing only the current root is the baseline itself: it canonicalizes to
    // no selection and offers no Restore.
    await side.locator("[data-sidebar-broaden]").click();
    await expect
      .poll(async () => (await prefixedLinks(side, prefix))[0])
      .toBe(href("zudo-wind/utilities"));
    const utilitiesId = await occurrenceOf(side, href("zudo-wind/utilities"));
    await side.locator(`[data-sidebar-focus][data-sidebar-focus-scope="${surfacesId}"]`).click();
    await expect.poll(() => prefixedLinks(side, prefix)).toEqual(surfacesView);
    await expect(side.locator("[data-sidebar-restore]")).toHaveCount(0);
    state = await storedScope(tab);
    assert.equal(state.selected, null, "a baseline-equal scope is stored as null");
    assert.deepEqual(state.expansion, {});
    // 1. Broaden to Utilities, collapse Surfaces and expand Typography: only
    // those deliberate choices are stored.
    await side.locator("[data-sidebar-broaden]").click();
    await expect
      .poll(async () => (await prefixedLinks(side, prefix))[0])
      .toBe(href("zudo-wind/utilities"));
    const typographyId = await occurrenceOf(side, href(typography.slug));
    const surfacesToggle = toggleOf(side, href(surfaces.slug));
    const typographyToggle = toggleOf(side, href(typography.slug));
    await expect(surfacesToggle).toHaveAttribute("aria-expanded", "true");
    await expect(typographyToggle).toHaveAttribute("aria-expanded", "false");
    await surfacesToggle.click();
    await expect(surfacesToggle).toHaveAttribute("aria-expanded", "false");
    await typographyToggle.click();
    await expect(typographyToggle).toHaveAttribute("aria-expanded", "true");
    const choices = { [surfacesId]: false, [typographyId]: true };
    state = await storedScope(tab);
    assert.equal(state.version, 2);
    assert.equal(state.selected, utilitiesId);
    assert.deepEqual(state.expansion, choices, "only deliberate choices are stored");
    // 2. Same-page hash and filter actions, then reload, keep the stored choices.
    const tocLink = tab.locator("[data-zd-toc] a[href^='#']").first();
    const hash = await tocLink.getAttribute("href");
    await tocLink.click();
    await expect(tab).toHaveURL(origin + href(background) + "/" + hash);
    assert.deepEqual((await scopeRecordFor(background, tab)).expansion, choices);
    const backgroundLabel = labels.get(href(background));
    await filter.fill(backgroundLabel);
    // A matching filter exposes the collapsed branch without changing the choice.
    await expect.poll(() => prefixedLinks(side, prefix)).toContain(href(background));
    state = await storedScope(tab);
    assert.equal(state.query, backgroundLabel);
    assert.deepEqual(state.expansion, choices, "filtering keeps the stored collapse");
    await filter.fill("");
    await expect(surfacesToggle).toHaveAttribute("aria-expanded", "false");
    assert(
      !(await prefixedLinks(side, prefix)).includes(href(background)),
      "clearing the filter restores the deliberate collapse",
    );
    await tab.reload();
    await mounted(tab);
    await expect(surfacesToggle).toHaveAttribute("aria-expanded", "false");
    await expect(typographyToggle).toHaveAttribute("aria-expanded", "true");
    state = await scopeRecordFor(background, tab);
    assert.equal(state.selected, utilitiesId);
    assert.deepEqual(state.expansion, choices, "reload keeps the stored choices");
    // A new destination inside Surfaces reveals its path by dropping only the
    // conflicting collapse; the unrelated Typography choice survives.
    await soft(href(radius) + "/", { reset: false, on: tab });
    state = await scopeRecordFor(radius, tab);
    assert.equal(state.selected, utilitiesId);
    assert.deepEqual(state.expansion, { [typographyId]: true });
    await expect(surfacesToggle).toHaveAttribute("aria-expanded", "true");
    await expect(typographyToggle).toHaveAttribute("aria-expanded", "true");
    // 3. Restore from Utilities clears the selection and choices, keeps the
    // filter, and desktop and mobile agree on the destination's filtered baseline.
    const radiusLabel = labels.get(href(radius));
    await filter.fill(radiusLabel);
    await side.locator("[data-sidebar-restore]").click();
    await expect(filter).toHaveValue(radiusLabel);
    await expect.poll(() => prefixedLinks(side, prefix)).toEqual(filteredBaseline(radiusLabel));
    state = await scopeRecordFor(radius, tab);
    assert.equal(state.selected, null);
    assert.deepEqual(state.expansion, {});
    assert.equal(state.query, radiusLabel);
    await tab.setViewportSize({ width: 390, height: 844 });
    await tab.getByRole("button", { name: /Open sidebar|サイドバーを開く/ }).click();
    const drawer = tab.locator("[data-zd-mobile-sidebar]");
    await expect(drawer).not.toHaveAttribute("inert");
    await expect(drawer.getByRole("textbox", { name: "Filter navigation" })).toHaveValue(
      radiusLabel,
    );
    await expect.poll(() => prefixedLinks(drawer, prefix)).toEqual(filteredBaseline(radiusLabel));
    await expect(drawer.locator("[data-sidebar-restore]")).toHaveCount(0);
    await tab.getByRole("button", { name: /Close sidebar|サイドバーを閉じる/ }).click();
    await tab.setViewportSize({ width: 1440, height: 1000 });
    await filter.fill("");
    await expect.poll(() => prefixedLinks(side, prefix)).toEqual(surfacesView);
    // 4. A legacy unversioned same-page record with a non-baseline selection
    // migrates once: selection and query stay, its mixed expansion is dropped.
    const legacy = {
      context: state.context,
      page: radius,
      selected: utilitiesId,
      query: radiusLabel,
      expansion: { [surfacesId]: false, [typographyId]: true },
    };
    await setRecord(legacy);
    await tab.reload();
    await mounted(tab);
    await expect.poll(async () => (await storedScope(tab))?.version).toBe(2);
    assert.deepEqual(await storedScope(tab), { version: 2, ...legacy, expansion: {} });
    await expect(filter).toHaveValue(radiusLabel);
    await filter.fill("");
    await expect
      .poll(async () => (await prefixedLinks(side, prefix))[0])
      .toBe(href("zudo-wind/utilities"));
    await expect(typographyToggle).toHaveAttribute("aria-expanded", "false");
    await typographyToggle.click();
    await expect(typographyToggle).toHaveAttribute("aria-expanded", "true");
    await tab.reload();
    await mounted(tab);
    await expect(typographyToggle).toHaveAttribute("aria-expanded", "true");
    state = await scopeRecordFor(radius, tab);
    assert.equal(state.selected, utilitiesId);
    assert.deepEqual(state.expansion, { [typographyId]: true }, "a post-migration choice persists");
    // 5. A legacy selection equal to the destination baseline canonicalizes to
    // null without a spurious Restore.
    await setRecord({ ...legacy, selected: surfacesId, query: "", expansion: {} });
    await tab.reload();
    await mounted(tab);
    await expect.poll(async () => (await storedScope(tab))?.version).toBe(2);
    state = await storedScope(tab);
    assert.equal(state.selected, null);
    assert.deepEqual(state.expansion, {});
    await expect.poll(() => prefixedLinks(side, prefix)).toEqual(surfacesView);
    await expect(side.locator("[data-sidebar-restore]")).toHaveCount(0);
    await browserContext.close();
    evidence.checks.push(
      `${locale || "EN"} 06R explicit-only persistence: deliberate choices only, hash/filter/reload retention, path reveal on a new destination, Restore with retained filter across desktop/mobile, one-time legacy migration, baseline canonicalization`,
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
  const utilitiesView = {
    slug: "zudo-wind/utilities",
    members: WIND_UTILITY_GROUPS.map((g) => g.slug),
  };
  // Membership with history: each transition starts from empty scope storage.
  for (const width of [1440, 390]) {
    await page.setViewportSize({ width, height: 1000 });
    await isolatedGoto("/docs/zudo-wind/");
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
            ? utilitiesView
            : flex;
      await assertScope(group.slug, group.members, width < 1024);
    }
    await traverse("back");
    await assertScope(type.slug, type.members, width < 1024);
    await traverse("back");
    await assertScope(flex.slug, flex.members, width < 1024);
    await traverse("forward");
    await assertScope(type.slug, type.members, width < 1024);
  }
  evidence.checks.push(
    "Isolated soft/history membership: linked root plus exact members at 1440px and 390px",
  );
  // Uninterrupted persistence (zudo-doc 6.1 explicit-only storage): clear the key
  // once, then navigate without clearing it. No disclosure is toggled, so every
  // settled destination stores version 2, its own page, no selection and
  // `expansion: {}`. Below 1024px the drawer stays unopened until the inspection
  // after utilities → Flex & grid → grid → compatibility, the sequence whose
  // automatic expansion leaked into the entry view before 6.1.
  for (const locale of ["", "/ja"])
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 1000 });
      const mobile = width < 1024;
      const prefix = `${locale}/docs/`;
      const otherPrefix = locale ? "/docs/" : "/ja/docs/";
      const settledRecord = async (slug, label = prefix + slug) => {
        await settled(`record ${label}`);
        const state = await scopeRecordFor(slug);
        assert.equal(state.selected, null, `${label}: stored selection`);
        assert.deepEqual(state.expansion, {}, `${label}: automatic expansion was persisted`);
        evidence.storage.push({ locale: locale || "en", width, page: label, state });
        return state;
      };
      await isolatedGoto(prefix + "zudo-wind/");
      const { context } = await settledRecord("zudo-wind");
      for (const [slug, view] of [
        ["zudo-wind/utilities", utilitiesView],
        [flex.slug, flex],
        ["zudo-wind/utilities/grid", flex],
        ["zudo-wind/compatibility", { slug: "zudo-wind", members: WIND_ENTRY_SLUGS }],
      ]) {
        await soft(prefix + slug + "/", { reset: false });
        assert.equal((await settledRecord(slug)).context, context, `${slug}: navigation context`);
        if (!mobile) await assertScope(view.slug, view.members, false, prefix);
      }
      // First drawer inspection: the exact configured Wind entry view.
      if (mobile) await assertScope("zudo-wind", WIND_ENTRY_SLUGS, true, prefix);
      await traverse("back", { reset: false });
      await settledRecord("zudo-wind/utilities/grid", "back to grid");
      await assertScope(flex.slug, flex.members, mobile, prefix);
      await traverse("back", { reset: false });
      await settledRecord(flex.slug, "back to Flex & grid");
      await assertScope(flex.slug, flex.members, mobile, prefix);
      await traverse("forward", { reset: false });
      await settledRecord("zudo-wind/utilities/grid", "forward to grid");
      await assertScope(flex.slug, flex.members, mobile, prefix);
      await page.reload();
      await mounted();
      await settledRecord("zudo-wind/utilities/grid", "reload grid");
      await assertScope(flex.slug, flex.members, mobile, prefix);
      await soft(prefix + "zudo-wind/learn/", { reset: false });
      await settledRecord("zudo-wind/learn");
      await assertScope("zudo-wind/learn", WIND_LEARN_SLUGS, mobile, prefix);
      // A different navigation context (the other locale) neither adopts nor keeps this record.
      await goto(otherPrefix + flex.slug + "/");
      await mounted();
      await expect.poll(async () => (await storedScope())?.context).not.toBe(context);
      await settledRecord(flex.slug, otherPrefix + flex.slug);
      await assertScope(flex.slug, flex.members, mobile, otherPrefix);
      // Hard return to Wind: the original context, the exact entry view.
      await goto(prefix + "zudo-wind/");
      await mounted();
      assert.equal((await settledRecord("zudo-wind", "hard return to Wind")).context, context);
      await assertScope("zudo-wind", WIND_ENTRY_SLUGS, mobile, prefix);
      // Coming from Tailwind keeps the Wind entry view without joining its members.
      await isolatedGoto(prefix + "zudo-wind/coming-from-tailwind/");
      await assertScope("zudo-wind", WIND_ENTRY_SLUGS, mobile, prefix);
    }
  evidence.checks.push(
    "Uninterrupted EN/JA storage at 1440px and 390px (drawer unopened until inspection): utilities → Flex & grid → grid → compatibility, Back/Back/Forward, reload, cross-branch, other-locale context and hard return to Wind each store version 2, the destination page, no selection and expansion {}, with exact linked-root views",
  );
  await verifyBroaderTree();
  await verifyExplicitDisclosure();
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
