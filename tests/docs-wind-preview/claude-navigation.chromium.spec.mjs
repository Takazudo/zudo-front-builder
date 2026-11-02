import { test, expect } from "@playwright/test";
import { readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const dist = resolve(process.env.WIND_DOCS_DIST ?? join(root, "docs/dist"));
const prefix = `/${(process.env.WIND_DOCS_BASE ?? "/").replace(/^\/+|\/+$/g, "")}`.replace(
  /\/?$/,
  "/",
);
const route = (slug) => `${prefix}docs/${slug}/`;
const linkTo = (slug) => `a[href="${route(slug)}"], a[href="${route(slug).slice(0, -1)}"]`;
const treeOf = (side) => side.locator("[data-sidebar-tree-root]");

// Discover emitted HTML, not source MDX: generated categories intentionally
// have category_no_page, and absent resources must never become skipped tests.
function emittedClaudeSlugs() {
  function walk(directory, segments = []) {
    return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
      if (entry.isDirectory()) return walk(join(directory, entry.name), [...segments, entry.name]);
      return entry.name === "index.html" ? [segments.join("/")] : [];
    });
  }
  return walk(join(dist, "docs"))
    .filter((slug) => /^claude(?:-|\/|$)/.test(slug))
    .sort();
}

async function sidebarOf(page, width) {
  if (width >= 1024) return page.locator("#desktop-sidebar");
  const open = page.getByRole("button", { name: "Open sidebar", exact: true });
  if (await open.isVisible()) await open.click();
  const side = page.locator("[data-zd-mobile-sidebar]");
  await expect(side).not.toHaveAttribute("inert", "");
  await expect(treeOf(side)).toBeVisible();
  await expect(side).toBeInViewport();
  await expect.poll(async () => (await side.boundingBox())?.x).toBe(0);
  return side;
}
async function closeMobile(page, width) {
  if (width < 1024) {
    // A completed client navigation can already have closed the drawer.
    const close = page.getByRole("button", { name: "Close sidebar", exact: true });
    if (await close.isVisible()) await close.click();
    await expect(page.getByRole("button", { name: "Open sidebar", exact: true })).toBeVisible();
  }
}
async function expandTree(side) {
  // Use the package's real disclosure controls so links in another resource
  // category can be followed without injecting navigation or scope state.
  while (await treeOf(side).locator('button[aria-expanded="false"]').count()) {
    await treeOf(side).locator('button[aria-expanded="false"]').first().click();
  }
}
async function expectClaude(page, width, slug, resources) {
  await expect(page).toHaveURL(
    (url) => url.pathname.replace(/\/$/, "") === route(slug).replace(/\/$/, ""),
  );
  await expect(page.locator("main h1").first()).toBeVisible();
  await expect(page.locator("[data-zd-nav-section]")).toHaveAttribute(
    "data-zd-nav-section",
    "claude",
  );
  // The persisted header must update on both document loads and client hops,
  // including its offscreen original when the mobile main menu is in use.
  const headerClaude = page.locator('[data-header-nav] > a[data-nav-category="claude"]');
  await expect(headerClaude).toHaveAttribute("aria-current", "page");
  await expect(
    page.locator('[data-header-nav] > [data-nav-item][aria-current="page"]'),
  ).toHaveCount(1);
  if (width >= 1024) {
    const toggle = page.locator("[data-nav-more-toggle]");
    await expect(toggle).toBeVisible();
    await toggle.click();
    const active = page
      .locator("[data-nav-more-menu]")
      .getByRole("link", { name: "Claude", exact: true });
    await expect(active).toBeVisible();
    await expect(active).toHaveAttribute("aria-current", "page");
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
  }
  const side = await sidebarOf(page, width);
  await expandTree(side);
  await expect(treeOf(side).locator(linkTo(slug))).toHaveAttribute("aria-current", "page");
  const hrefs = await treeOf(side)
    .locator("a[href]")
    .evaluateAll((links) => links.map((link) => link.getAttribute("href").replace(/\/$/, "")));
  expect([...new Set(hrefs)].sort()).toEqual(
    resources.map((item) => route(item).slice(0, -1)).sort(),
  );
  expect(hrefs.length).toBe(new Set(hrefs).size);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  const overflows = await treeOf(side)
    .locator("a[href]")
    .evaluateAll((links) =>
      links.map((link) => {
        const surface = link.closest("aside").getBoundingClientRect();
        return link.getBoundingClientRect().right - surface.right;
      }),
    );
  for (const overflow of overflows) expect(overflow).toBeLessThanOrEqual(1);
  return side;
}
async function mainMenuLink(page, width, slug) {
  if (width < 1024) {
    const side = await sidebarOf(page, width);
    await side.getByRole("button", { name: "Back to main menu", exact: true }).click();
    return side.locator(linkTo(slug));
  }
  // Claude is late in the real header order and must be reachable through
  // overflow at this viewport. Assert and use it, rather than force-clicking
  // the hidden original anchor.
  const toggle = page.locator("[data-nav-more-toggle]");
  await expect(toggle).toBeVisible();
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  return page
    .locator("[data-nav-more-menu]")
    .locator(
      `a[href="${new URL(route(slug), page.url()).href}"], a[href="${new URL(route(slug).slice(0, -1), page.url()).href}"]`,
    );
}

for (const width of [1440, 390]) {
  test(`Claude resources retain native navigation through direct entry and history ${width}`, async ({
    page,
    request,
  }, testInfo) => {
    const resources = emittedClaudeSlugs();
    const wisdom = "claude-skills/zfb-wisdom";
    const design = "claude-skills/zudo-doc-design-system";
    const instructions = "claude-md/root";
    for (const required of ["claude", wisdom, design, instructions])
      expect(resources, `required generated resource disappeared: ${required}`).toContain(required);
    expect(
      resources.filter((slug) => slug.startsWith("claude-skills/")).length,
    ).toBeGreaterThanOrEqual(4);
    expect(resources.some((slug) => slug.startsWith("claude-md/"))).toBe(true);
    // Every discovered sidebar target is actually served by this unchanged
    // static host, including a genuine WIND_DOCS_BASE subpath build.
    for (const slug of resources) {
      const response = await request.get(route(slug));
      expect(response.status(), slug).toBe(200);
    }
    await page.setViewportSize({ width, height: 950 });
    const failures = [];
    page.on("pageerror", (error) => failures.push(error.message));
    page.on("console", (message) => {
      if (message.type() === "error") failures.push(message.text());
    });
    // History API requests may target the separate history service; this
    // static-host check covers only routes/assets on the actual preview origin.
    const origin = new URL(testInfo.project.use.baseURL).origin;
    page.on("response", (response) => {
      if (new URL(response.url()).origin === origin && response.status() >= 400)
        failures.push(`${response.status()} ${response.url()}`);
    });
    page.on("requestfailed", (request) => {
      if (new URL(request.url()).origin === origin)
        failures.push(`${request.failure()?.errorText} ${request.url()}`);
    });
    for (const slug of [wisdom, design, instructions]) {
      await page.goto(route(slug));
      await expectClaude(page, width, slug, resources);
      if (slug === wisdom) {
        const screenshot = testInfo.outputPath(`claude-wisdom-${width}.png`);
        await page.screenshot({ path: screenshot, animations: "disabled" });
        await testInfo.attach(`Claude wisdom navigation ${width}`, {
          path: screenshot,
          contentType: "image/png",
        });
      }
    }

    await page.goto(route("claude"));
    await expectClaude(page, width, "claude", resources);
    await closeMobile(page, width);
    const marker = await page.evaluate(() => (window.__claudeNavigationMarker = Math.random()));
    await page.locator("main").locator(linkTo(wisdom)).click();
    let side = await expectClaude(page, width, wisdom, resources);
    await treeOf(side).locator(linkTo(design)).click();
    side = await expectClaude(page, width, design, resources);
    await treeOf(side).locator(linkTo(instructions)).click();
    await expectClaude(page, width, instructions, resources);
    expect(await page.evaluate(() => window.__claudeNavigationMarker)).toBe(marker);
    await closeMobile(page, width);
    await page.goBack();
    await expectClaude(page, width, design, resources);
    await closeMobile(page, width);
    await page.goForward();
    await expectClaude(page, width, instructions, resources);
    await page.reload();
    await expectClaude(page, width, instructions, resources);
    await closeMobile(page, width);
    const crossCategoryMarker = await page.evaluate(
      () => (window.__claudeNavigationMarker = Math.random()),
    );

    // Exercise the actual desktop overflow/mobile main-menu control to leave
    // Claude, then return through its visible menu link with active state.
    if (width >= 1024) {
      await page.locator("header").locator(linkTo("getting-started")).first().click();
    } else {
      await (await mainMenuLink(page, width, "getting-started")).click();
    }
    await expect(page).toHaveURL(
      (url) => url.pathname.replace(/\/$/, "") === route("getting-started").replace(/\/$/, ""),
    );
    await expect(page.locator("[data-zd-nav-section]")).toHaveAttribute(
      "data-zd-nav-section",
      "getting-started",
    );
    const otherSide = await sidebarOf(page, width);
    await expect(treeOf(otherSide).locator(linkTo(wisdom))).toHaveCount(0);
    await expect(
      page.locator('[data-header-nav] > a[data-nav-category="claude"]'),
    ).not.toHaveAttribute("aria-current", "page");
    await closeMobile(page, width);
    const claude = await mainMenuLink(page, width, "claude");
    await expect(claude).toBeVisible();
    await claude.click();
    await expectClaude(page, width, "claude", resources);
    expect(await page.evaluate(() => window.__claudeNavigationMarker)).toBe(crossCategoryMarker);
    expect(failures).toEqual([]);
  });
}
