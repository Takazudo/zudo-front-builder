import { test, expect } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import {
  ARCHITECTURE_GROUPS,
  BUILD_GROUPS,
  DEPLOY_SLUGS,
} from "../../docs/src/config/navigation-groups.mjs";

const prefix = process.env.WIND_DOCS_BASE ?? "/";
const route = (slug, ja = false) => `${prefix}${ja ? "ja/" : ""}docs/${slug}`;
// Built hrefs carry a trailing slash; accept either spelling.
const linkTo = (slug, ja) => `a[href="${route(slug, ja)}/"], a[href="${route(slug, ja)}"]`;
const strip = (href) => href.replace(/\/$/, "");
const DEPLOY_GUIDES = DEPLOY_SLUGS.filter((slug) => slug.startsWith("guides/"));

// Open the visible navigation surface for this width: the desktop sidebar or
// the mobile drawer. Both can coexist in the DOM, so every count is scoped.
async function sidebarOf(page, width) {
  if (width >= 1024) return page.locator("#desktop-sidebar");
  await page.getByRole("button", { name: /Open sidebar|サイドバーを開く/ }).click();
  const drawer = page.locator("[data-zd-mobile-sidebar]");
  await expect(drawer.locator("[data-sidebar-tree-root]")).toBeVisible();
  return drawer;
}
const treeOf = (side) => side.locator("[data-sidebar-tree-root]");
// A linked category row: the div holding its link, focus control and disclosure.
const rowOf = (side, slug, ja) =>
  treeOf(side).locator(
    `div:has(> a[href="${route(slug, ja)}/"]), div:has(> a[href="${route(slug, ja)}"])`,
  );
const toggleOf = (side, slug, ja) =>
  rowOf(side, slug, ja).locator(":scope > button[aria-expanded]");
const focusOf = (side, slug, ja) => rowOf(side, slug, ja).locator(":scope > [data-sidebar-focus]");
async function treeHrefs(side) {
  return (
    await treeOf(side)
      .locator("a[href]")
      .evaluateAll((links) => links.map((a) => a.getAttribute("href")))
  ).map(strip);
}
// Client transitions update the URL before the sidebar swap settles, so poll.
const expectTree = (side, hrefs) => expect.poll(() => treeHrefs(side)).toEqual(hrefs);
function watchErrors(page) {
  const failures = [];
  page.on("pageerror", (error) => failures.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") failures.push(message.text());
  });
  return failures;
}

for (const width of [1440, 390]) {
  for (const ja of [false, true]) {
    test(`Architecture groups are linked parent rows ${ja ? "JA" : "EN"} ${width}`, async ({
      page,
    }) => {
      await page.setViewportSize({ width, height: 950 });
      const failures = watchErrors(page);
      const [fundamentals, pagesContent, rendering] = ARCHITECTURE_GROUPS;
      await page.goto(route(fundamentals.slug, ja));
      let side = await sidebarOf(page, width);

      // Fresh state: four linked group rows as peers, only the current one open,
      // and the group's own landing page is not repeated as its first child.
      expect(await treeHrefs(side)).toEqual([
        route(fundamentals.slug, ja),
        ...fundamentals.members.map((slug) => route(slug, ja)),
        ...ARCHITECTURE_GROUPS.slice(1).map((group) => route(group.slug, ja)),
      ]);
      for (const group of ARCHITECTURE_GROUPS) {
        await expect(rowOf(side, group.slug, ja)).toHaveCount(1);
        await expect(focusOf(side, group.slug, ja)).toHaveCount(1);
        await expect(toggleOf(side, group.slug, ja)).toHaveAttribute(
          "aria-expanded",
          group === fundamentals ? "true" : "false",
        );
      }
      await expect(rowOf(side, fundamentals.slug, ja).locator(":scope > a")).toHaveAttribute(
        "aria-current",
        "page",
      );
      await expect(treeOf(side).locator(linkTo("architecture", ja))).toHaveCount(0);

      // Disclosure toggles children without navigating or moving the reader.
      const url = page.url();
      const scrollY = await page.evaluate(() => scrollY);
      await toggleOf(side, pagesContent.slug, ja).click();
      await expect(toggleOf(side, pagesContent.slug, ja)).toHaveAttribute("aria-expanded", "true");
      for (const slug of pagesContent.members)
        await expect(treeOf(side).locator(linkTo(slug, ja))).toHaveCount(1);
      await toggleOf(side, fundamentals.slug, ja).click();
      await expect(toggleOf(side, fundamentals.slug, ja)).toHaveAttribute("aria-expanded", "false");
      await expect(treeOf(side).locator(linkTo(fundamentals.members[0], ja))).toHaveCount(0);
      expect(page.url()).toBe(url);
      expect(await page.evaluate(() => scrollY)).toBe(scrollY);

      // Native Broaden reaches the real Architecture landing page, the highest
      // scope, where the native toolbar hides (as on the Wind root). Showing
      // one branch from there brings Restore back to the four-group baseline.
      await side.locator("[data-sidebar-broaden]").click();
      await expect(treeOf(side).locator(linkTo("architecture", ja))).toHaveCount(1);
      for (const group of ARCHITECTURE_GROUPS)
        await expect(rowOf(side, group.slug, ja)).toHaveCount(1);
      await expect(side.locator("[data-sidebar-scope-toolbar]")).toHaveCount(0);
      await focusOf(side, pagesContent.slug, ja).click();
      await side.locator("[data-sidebar-restore]").click();
      await expect(side.locator("[data-sidebar-restore]")).toHaveCount(0);
      await expect(treeOf(side).locator(linkTo("architecture", ja))).toHaveCount(0);

      // Show only this branch, then Restore.
      await focusOf(side, rendering.slug, ja).click();
      expect(await treeHrefs(side)).toEqual([
        route(rendering.slug, ja),
        ...rendering.members.map((slug) => route(slug, ja)),
      ]);
      await side.locator("[data-sidebar-restore]").click();
      await expect(rowOf(side, pagesContent.slug, ja)).toHaveCount(1);
      expect(page.url()).toBe(url);

      if (width >= 1024) {
        const expanded = (group, value) =>
          expect(toggleOf(side, group.slug, ja)).toHaveAttribute("aria-expanded", value);
        // Restore cleared every explicit choice: back to the fresh baseline.
        await expanded(fundamentals, "true");
        await expanded(pagesContent, "false");
        // Explicit choices survive same-page hash changes, filtering and reload.
        await toggleOf(side, fundamentals.slug, ja).click();
        await toggleOf(side, pagesContent.slug, ja).click();
        await expanded(fundamentals, "false");
        await expanded(pagesContent, "true");
        await page.evaluate(() => {
          location.hash = "sidebar-hash-check";
        });
        await expanded(fundamentals, "false");
        const filter = side.getByRole("textbox", { name: "Filter navigation" });
        await filter.fill("zz-no-such-article-zz");
        await expect(treeOf(side).locator("a[href]")).toHaveCount(0);
        await filter.fill("");
        await expanded(fundamentals, "false");
        await expanded(pagesContent, "true");
        await page.reload();
        side = await sidebarOf(page, width);
        await expanded(fundamentals, "false");
        await expanded(pagesContent, "true");
        // Filtering finds articles in a collapsed group; Restore keeps the filter text.
        await filter.fill(ja ? "なぜ Rust" : "Why Rust");
        await expect(treeOf(side).locator(linkTo("architecture/why-rust", ja))).toHaveCount(1);
        await focusOf(side, fundamentals.slug, ja).click();
        await side.locator("[data-sidebar-restore]").click();
        await expect(filter).toHaveValue(ja ? "なぜ Rust" : "Why Rust");
        await filter.fill("");
        await expanded(fundamentals, "true");

        // The group label is a real link; the next page reveals its own path and
        // keeps the unrelated explicit collapse.
        await toggleOf(side, fundamentals.slug, ja).click();
        await expanded(fundamentals, "false");
        await rowOf(side, rendering.slug, ja).locator(":scope > a").click();
        await expect(page).toHaveURL(new RegExp(`${rendering.slug}/?$`));
        await expanded(rendering, "true");
        await expanded(fundamentals, "false");
        // Keyboard: the disclosure is a focusable button with an accessible name.
        await expanded(pagesContent, "false");
        await expect(toggleOf(side, pagesContent.slug, ja)).toHaveAttribute("aria-label", /Expand/);
        await toggleOf(side, pagesContent.slug, ja).focus();
        await page.keyboard.press("Enter");
        await expanded(pagesContent, "true");
        await expect(toggleOf(side, pagesContent.slug, ja)).toHaveAttribute(
          "aria-label",
          /Collapse/,
        );
      }

      // The real landing page is the root view of the same tree.
      await page.goto(route("architecture", ja));
      side = await sidebarOf(page, width);
      // Root row first, then its four groups; explicit reader choices from the
      // shared context may keep members of other groups open.
      const rows = [
        route("architecture", ja),
        ...ARCHITECTURE_GROUPS.map((group) => route(group.slug, ja)),
      ];
      expect((await treeHrefs(side)).filter((href) => rows.includes(href))).toEqual(rows);
      expect((await treeHrefs(side))[0]).toBe(route("architecture", ja));
      expect(failures).toEqual([]);
    });
  }
}

for (const ja of [false, true]) {
  test(`Architecture owns Routing and Three ways to run zfb ${ja ? "JA" : "EN"}`, async ({
    page,
  }) => {
    const failures = watchErrors(page);
    const side = page.locator("#desktop-sidebar");
    const expectArchitecture = async (slug, group) => {
      await expect(page).toHaveURL(new RegExp(`${slug}/?$`));
      await expect(rowOf(side, group.slug, ja)).toHaveCount(1);
      await expect(toggleOf(side, group.slug, ja)).toHaveAttribute("aria-expanded", "true");
      await expect(treeOf(side).locator(linkTo(slug, ja))).toHaveAttribute("aria-current", "page");
      await expect(treeOf(side).locator(linkTo("build-with-zfb", ja))).toHaveCount(0);
      await expect(treeOf(side).locator(linkTo("deploy-embed", ja))).toHaveCount(0);
      for (const other of ARCHITECTURE_GROUPS)
        await expect(rowOf(side, other.slug, ja)).toHaveCount(1);
    };
    const pagesContent = ARCHITECTURE_GROUPS.find((group) => group.id === "pages-content");
    const fundamentals = ARCHITECTURE_GROUPS.find((group) => group.id === "fundamentals");

    await page.goto(route("architecture/routing", ja));
    await expectArchitecture("architecture/routing", pagesContent);

    // Reached from Build with zfb (soft navigation) and back through history.
    await page.goto(route("build-with-zfb/pages-routes", ja));
    await expect(rowOf(side, "build-with-zfb/pages-routes", ja)).toHaveCount(1);
    await page
      .locator(`main :is(${linkTo("architecture/routing", ja)})`)
      .first()
      .click();
    await expectArchitecture("architecture/routing", pagesContent);
    await page.goBack();
    await expect(page).toHaveURL(/build-with-zfb\/pages-routes\/?$/);
    await expect(treeOf(side).locator(linkTo("build-with-zfb", ja))).toHaveCount(1);
    await page.goForward();
    await expectArchitecture("architecture/routing", pagesContent);

    // Build's sidebar cross-link lands on the Architecture section too.
    await page.goto(route("build-with-zfb/pages-routes", ja));
    await treeOf(side).locator(linkTo("architecture/routing", ja)).first().click();
    await expectArchitecture("architecture/routing", pagesContent);

    // Reached from Deploy & Embed, in content and in its sidebar.
    await page.goto(route("deploy-embed", ja));
    await page
      .locator(`main :is(${linkTo("architecture/three-ways-to-run-zfb", ja)})`)
      .first()
      .click();
    await expectArchitecture("architecture/three-ways-to-run-zfb", fundamentals);
    const expectDeploy = async (slug) => {
      await expect(page).toHaveURL(new RegExp(`${slug}/?$`));
      await expect(rowOf(side, "deploy-embed", ja)).toHaveCount(1);
      await expect(treeOf(side).locator(linkTo(slug, ja))).toHaveAttribute("aria-current", "page");
      for (const group of ARCHITECTURE_GROUPS)
        await expect(rowOf(side, group.slug, ja)).toHaveCount(0);
    };
    await page.goBack();
    await expectDeploy("deploy-embed");
    await page.goForward();
    await expectArchitecture("architecture/three-ways-to-run-zfb", fundamentals);
    await page.goto(route("deploy-embed", ja));
    await treeOf(side).locator(linkTo("architecture/three-ways-to-run-zfb", ja)).click();
    await expectArchitecture("architecture/three-ways-to-run-zfb", fundamentals);
    await page.goBack();
    await expectDeploy("deploy-embed");
    await page.goForward();
    await expectArchitecture("architecture/three-ways-to-run-zfb", fundamentals);
    // The same from a Deploy-owned guide's sidebar.
    await page.goto(route("guides/desktop-deployment", ja));
    await expectDeploy("guides/desktop-deployment");
    await treeOf(side).locator(linkTo("architecture/three-ways-to-run-zfb", ja)).click();
    await expectArchitecture("architecture/three-ways-to-run-zfb", fundamentals);
    await page.goBack();
    await expectDeploy("guides/desktop-deployment");
    await page.goForward();
    await expectArchitecture("architecture/three-ways-to-run-zfb", fundamentals);
    await page.goto(route("architecture/three-ways-to-run-zfb", ja));
    await expectArchitecture("architecture/three-ways-to-run-zfb", fundamentals);

    // Locale switching keeps the canonical section.
    await page.goto(route("architecture/routing", ja));
    const lang = page.locator("header [data-language-switcher]:visible").first();
    await lang.locator("[data-language-toggle]").click();
    await lang.locator(`a[lang="${ja ? "en" : "ja"}"]`).click();
    await expect(page.locator("html")).toHaveAttribute("lang", ja ? "en" : "ja");
    await expect(rowOf(side, pagesContent.slug, !ja)).toHaveCount(1);
    await expect(toggleOf(side, pagesContent.slug, !ja)).toHaveAttribute("aria-expanded", "true");
    expect(failures).toEqual([]);
  });

  test(`Build with zfb groups are linked parent rows ${ja ? "JA" : "EN"}`, async ({ page }) => {
    const failures = watchErrors(page);
    const side = page.locator("#desktop-sidebar");
    await page.goto(route("guides/troubleshooting", ja));
    const hrefs = await treeHrefs(side);
    expect(hrefs[0]).toBe(route("build-with-zfb", ja));
    expect(hrefs.filter((href) => href === route("build-with-zfb", ja))).toHaveLength(1);
    for (const group of BUILD_GROUPS) {
      await expect(rowOf(side, group.slug, ja)).toHaveCount(1);
      await expect(focusOf(side, group.slug, ja)).toHaveCount(1);
      expect(hrefs.filter((href) => href === route(group.slug, ja))).toHaveLength(1);
    }
    // Troubleshooting intentionally belongs to two groups, both revealed here.
    expect(hrefs.filter((href) => href === route("guides/troubleshooting", ja))).toHaveLength(2);
    // Cross-links stay leaves instead of importing their sections.
    await page.goto(route("build-with-zfb/content-data", ja));
    const content = BUILD_GROUPS.find((group) => group.id === "content-data");
    const styles = BUILD_GROUPS.find((group) => group.id === "styles-assets");
    if ((await toggleOf(side, styles.slug, ja).getAttribute("aria-expanded")) === "false")
      await toggleOf(side, styles.slug, ja).click();
    const leaves = await treeHrefs(side);
    for (const slug of content.members)
      expect(leaves.filter((href) => href === route(slug, ja))).toHaveLength(1);
    for (const leaf of ["markdown-features", "zudo-wind"]) {
      expect(leaves).toContain(route(leaf, ja));
      expect(leaves.some((href) => href.startsWith(`${route(leaf, ja)}/`))).toBe(false);
    }
    expect(failures).toEqual([]);
  });
}

for (const width of [1440, 390]) {
  for (const ja of [false, true]) {
    test(`Deploy & Embed is one linked root over four leaves ${ja ? "JA" : "EN"} ${width}`, async ({
      page,
    }) => {
      await page.setViewportSize({ width, height: 950 });
      const failures = watchErrors(page);
      const deployTree = DEPLOY_SLUGS.map((slug) => route(slug, ja));
      const expanded = (value) =>
        expect(toggleOf(side, "deploy-embed", ja)).toHaveAttribute("aria-expanded", value);
      await page.goto(route("deploy-embed", ja));
      let side = await sidebarOf(page, width);

      // Fresh state: the real landing page once, as a linked parent row, then
      // its four members in DEPLOY_SLUGS order. No repeated Overview child.
      expect(await treeHrefs(side)).toEqual([route("deploy-embed", ja), ...deployTree]);
      await expect(rowOf(side, "deploy-embed", ja)).toHaveCount(1);
      await expect(focusOf(side, "deploy-embed", ja)).toHaveCount(1);
      await expanded("true");
      await expect(rowOf(side, "deploy-embed", ja).locator(":scope > a")).toHaveAttribute(
        "aria-current",
        "page",
      );
      // The members are leaves, visibly nested under the root row: their label
      // text starts right of and below the root label (measured in one frame;
      // the mobile drawer may still be sliding in).
      for (const slug of DEPLOY_SLUGS) {
        await expect(treeOf(side).locator(linkTo(slug, ja))).toHaveCount(1);
        await expect(toggleOf(side, slug, ja)).toHaveCount(0);
        await expect(focusOf(side, slug, ja)).toHaveCount(0);
      }
      const textBoxes = await treeOf(side).evaluate((tree) =>
        [...tree.querySelectorAll("a[href]")].map((a) => {
          const range = document.createRange();
          range.selectNodeContents(a);
          const { left, top } = range.getBoundingClientRect();
          return { left, top };
        }),
      );
      const [rootText, ...leafTexts] = textBoxes;
      expect(leafTexts).toHaveLength(DEPLOY_SLUGS.length);
      for (const [index, leaf] of leafTexts.entries()) {
        expect(leaf.left, DEPLOY_SLUGS[index]).toBeGreaterThan(rootText.left + 4);
        expect(leaf.top, DEPLOY_SLUGS[index]).toBeGreaterThan(rootText.top);
      }
      // The root is the highest scope: the native toolbar stays hidden.
      await expect(side.locator("[data-sidebar-scope-toolbar]")).toHaveCount(0);

      // Disclosure toggles children without navigating or moving the reader.
      const url = page.url();
      await page.evaluate(() => scrollTo(0, 200));
      const scrollY = await page.evaluate(() => scrollY);
      await toggleOf(side, "deploy-embed", ja).click();
      await expanded("false");
      expect(await treeHrefs(side)).toEqual([route("deploy-embed", ja)]);
      await expect(side.locator("[data-sidebar-scope-toolbar]")).toHaveCount(0);
      await toggleOf(side, "deploy-embed", ja).click();
      await expanded("true");
      expect(await treeHrefs(side)).toEqual([route("deploy-embed", ja), ...deployTree]);
      expect(page.url()).toBe(url);
      expect(await page.evaluate(() => scrollY)).toBe(scrollY);

      if (width >= 1024) {
        // An explicit collapse survives same-page hash changes, filtering and reload.
        await toggleOf(side, "deploy-embed", ja).click();
        await expanded("false");
        await page.evaluate(() => {
          location.hash = "sidebar-hash-check";
        });
        await expanded("false");
        const filter = side.getByRole("textbox", { name: "Filter navigation" });
        await filter.fill("zz-no-such-article-zz");
        await expect(treeOf(side).locator("a[href]")).toHaveCount(0);
        await filter.fill("");
        await expanded("false");
        await page.reload();
        side = await sidebarOf(page, width);
        await expanded("false");
        await expect(side.locator("[data-sidebar-scope-toolbar]")).toHaveCount(0);
        // Filtering finds a member of the collapsed root.
        await filter.fill(ja ? "デスクトップ" : "Desktop");
        await expect(treeOf(side).locator(linkTo("guides/desktop-deployment", ja))).toHaveCount(1);
        await filter.fill("");
        await expanded("false");
        // Keyboard: the disclosure is a focusable button with an accessible name.
        await expect(toggleOf(side, "deploy-embed", ja)).toHaveAttribute("aria-label", /Expand/);
        await toggleOf(side, "deploy-embed", ja).focus();
        await page.keyboard.press("Enter");
        await expanded("true");
        await expect(toggleOf(side, "deploy-embed", ja)).toHaveAttribute("aria-label", /Collapse/);
        await page.keyboard.press("Enter");
        await expanded("false");
        // Soft navigation to another owned page reveals its path.
        await page
          .locator(`main :is(${linkTo("guides/embed-as-library", ja)})`)
          .first()
          .click();
        await expect(page).toHaveURL(/guides\/embed-as-library\/?$/);
        await expanded("true");
        await expect(treeOf(side).locator(linkTo("guides/embed-as-library", ja))).toHaveAttribute(
          "aria-current",
          "page",
        );
        // The root label is a real link back to the landing page.
        await rowOf(side, "deploy-embed", ja).locator(":scope > a").click();
        await expect(page).toHaveURL(/deploy-embed\/?$/);
        await expect(rowOf(side, "deploy-embed", ja).locator(":scope > a")).toHaveAttribute(
          "aria-current",
          "page",
        );
      } else {
        // The mobile drawer carries the same tree as the desktop sidebar, and a
        // leaf link navigates to a page that keeps the Deploy tree.
        expect(await treeHrefs(page.locator("#desktop-sidebar"))).toEqual(await treeHrefs(side));
        await treeOf(side).locator(linkTo("guides/desktop-deployment", ja)).click();
        await expect(page).toHaveURL(/guides\/desktop-deployment\/?$/);
        side = await sidebarOf(page, width);
        await expectTree(side, [route("deploy-embed", ja), ...deployTree]);
        await expect(treeOf(side).locator(linkTo("guides/desktop-deployment", ja))).toHaveAttribute(
          "aria-current",
          "page",
        );
        await expect(side.locator("[data-sidebar-scope-toolbar]")).toHaveCount(0);
      }
      expect(failures).toEqual([]);
    });
  }
}

for (const ja of [false, true]) {
  test(`Deploy owns its three guides on direct entry and locale switch ${ja ? "JA" : "EN"}`, async ({
    page,
  }) => {
    const failures = watchErrors(page);
    const side = page.locator("#desktop-sidebar");
    const expectDeploy = async (slug, locale) => {
      await expect(page).toHaveURL(new RegExp(`${slug}/?$`));
      await expectTree(side, [
        route("deploy-embed", locale),
        ...DEPLOY_SLUGS.map((member) => route(member, locale)),
      ]);
      await expect(toggleOf(side, "deploy-embed", locale)).toHaveAttribute("aria-expanded", "true");
      await expect(treeOf(side).locator(linkTo(slug, locale))).toHaveAttribute(
        "aria-current",
        "page",
      );
      for (const group of [...ARCHITECTURE_GROUPS, ...BUILD_GROUPS])
        await expect(rowOf(side, group.slug, locale)).toHaveCount(0);
      await expect(side.locator("[data-sidebar-scope-toolbar]")).toHaveCount(0);
    };
    for (const slug of DEPLOY_GUIDES) {
      await page.goto(route(slug, ja));
      await expectDeploy(slug, ja);
      const lang = page.locator("header [data-language-switcher]:visible").first();
      await lang.locator("[data-language-toggle]").click();
      await lang.locator(`a[lang="${ja ? "en" : "ja"}"]`).click();
      await expect(page.locator("html")).toHaveAttribute("lang", ja ? "en" : "ja");
      await expectDeploy(slug, !ja);
    }
    expect(failures).toEqual([]);
  });
}

test("section trees stay correct across Architecture, Build, Wind, Deploy and back", async ({
  page,
}) => {
  const failures = watchErrors(page);
  const side = page.locator("#desktop-sidebar");
  const views = {
    architecture: async () => {
      await page.goto(route("architecture/islands"));
      await expect(rowOf(side, "architecture/rendering")).toHaveCount(1);
      await expect(treeOf(side).locator(linkTo("build-with-zfb"))).toHaveCount(0);
    },
    build: async () => {
      await page.goto(route("build-with-zfb/styles-assets"));
      expect((await treeHrefs(side))[0]).toBe(route("build-with-zfb"));
      await expect(rowOf(side, "architecture/rendering")).toHaveCount(0);
    },
    wind: async () => {
      await page.goto(route("zudo-wind/learn"));
      expect((await treeHrefs(side))[0]).toBe(route("zudo-wind/learn"));
      await expect(rowOf(side, "build-with-zfb/styles-assets")).toHaveCount(0);
    },
    deploy: async () => {
      await page.goto(route("guides/ssr-and-cloudflare-bindings"));
      expect(await treeHrefs(side)).toEqual([
        route("deploy-embed"),
        ...DEPLOY_SLUGS.map((slug) => route(slug)),
      ]);
    },
    gettingStarted: async () => {
      await page.goto(route("getting-started"));
      const hrefs = await treeHrefs(side);
      expect(hrefs.some((href) => href.includes("/getting-started"))).toBe(true);
      for (const group of [...ARCHITECTURE_GROUPS, ...BUILD_GROUPS])
        expect(hrefs).not.toContain(route(group.slug));
      expect(hrefs.some((href) => href.includes("/zudo-wind/learn"))).toBe(false);
      expect(hrefs).not.toContain(route("deploy-embed"));
    },
  };
  for (const order of [
    ["architecture", "build", "wind", "deploy", "gettingStarted"],
    ["deploy", "wind", "build", "architecture", "gettingStarted"],
  ])
    for (const view of order) await views[view]();
  expect(failures).toEqual([]);
});

// Client transitions between sections: each soft link lands on the
// destination's own tree, never the previous section's.
test("soft links between Deploy and other sections swap the sidebar tree", async ({ page }) => {
  const failures = watchErrors(page);
  const side = page.locator("#desktop-sidebar");
  const deployTree = [route("deploy-embed"), ...DEPLOY_SLUGS.map((slug) => route(slug))];
  const headerLink = (slug) => page.locator(`header :is(${linkTo(slug)}):visible`).first();
  await page.goto(route("getting-started"));
  const marker = await page.evaluate(() => (window.__softNavMarker = Math.random()));
  await headerLink("deploy-embed").click();
  await expect(page).toHaveURL(/deploy-embed\/?$/);
  await expectTree(side, deployTree);
  await treeOf(side).locator(linkTo("guides/ssr-and-cloudflare-bindings")).click();
  await expect(page).toHaveURL(/guides\/ssr-and-cloudflare-bindings\/?$/);
  await expectTree(side, deployTree);
  await headerLink("build-with-zfb").click();
  await expect(page).toHaveURL(/build-with-zfb\/?$/);
  await expect.poll(async () => (await treeHrefs(side))[0]).toBe(route("build-with-zfb"));
  await expect(rowOf(side, "deploy-embed")).toHaveCount(0);
  await headerLink("deploy-embed").click();
  await expect(page).toHaveURL(/deploy-embed\/?$/);
  await expectTree(side, deployTree);
  await headerLink("zudo-wind").click();
  await expect(page).toHaveURL(/zudo-wind\/?$/);
  await expect.poll(async () => (await treeHrefs(side))[0]).toBe(route("zudo-wind"));
  await expect(rowOf(side, "deploy-embed")).toHaveCount(0);
  await page.goBack();
  await expect(page).toHaveURL(/deploy-embed\/?$/);
  await expectTree(side, deployTree);
  await treeOf(side).locator(linkTo("architecture/three-ways-to-run-zfb")).click();
  await expect(rowOf(side, "architecture/fundamentals")).toHaveCount(1);
  await expect(rowOf(side, "deploy-embed")).toHaveCount(0);
  await headerLink("getting-started").click();
  await expect(page).toHaveURL(/getting-started\/?$/);
  await expect.poll(() => treeHrefs(side)).not.toContain(route("deploy-embed"));
  // Every hop above was a client transition, not a full document load.
  expect(await page.evaluate(() => window.__softNavMarker)).toBe(marker);
  expect(failures).toEqual([]);
});

test("breadcrumbs, pagers and links stay valid on Architecture, Build and Deploy pages", async ({
  page,
  request,
}) => {
  const failures = watchErrors(page);
  for (const ja of [false, true])
    for (const slug of [
      "architecture",
      "architecture/fundamentals",
      "architecture/routing",
      "architecture/client-side-routing",
      "build-with-zfb/pages-routes",
      "guides/troubleshooting",
      "deploy-embed",
      ...DEPLOY_GUIDES,
    ]) {
      await page.goto(route(slug, ja));
      await expect(page.getByRole("navigation", { name: "Breadcrumb", exact: true })).toBeVisible();
      const hrefs = await page
        .locator("main nav a[href], #desktop-sidebar [data-sidebar-tree-root] a[href]")
        .evaluateAll((links) => [...new Set(links.map((a) => a.getAttribute("href")))]);
      for (const href of hrefs.filter((href) => href.startsWith("/"))) {
        const response = await request.get(href);
        expect(response.status(), href).toBe(200);
      }
    }
  expect(failures).toEqual([]);
});

for (const width of [1440, 390])
  for (const ja of [false, true])
    for (const scheme of ["light", "dark"])
      test(`long labels keep clear of row controls ${ja ? "JA" : "EN"} ${width} ${scheme}`, async ({
        page,
      }) => {
        await page.emulateMedia({ colorScheme: scheme });
        await page.setViewportSize({ width, height: 950 });
        await page.goto(route("architecture/client-side-routing", ja));
        const side = await sidebarOf(page, width);
        await toggleOf(side, "architecture/build-runtime", ja).click();
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
          true,
        );
        for (const group of ARCHITECTURE_GROUPS) {
          const row = rowOf(side, group.slug, ja);
          const link = await row.locator(":scope > a").boundingBox();
          for (const control of [
            row.locator(":scope > [data-sidebar-focus]"),
            row.locator(":scope > button[aria-expanded]"),
          ]) {
            const box = await control.boundingBox();
            expect(link.x + link.width, group.slug).toBeLessThanOrEqual(box.x + 0.5);
          }
        }
        const current = treeOf(side).locator(linkTo("architecture/client-side-routing", ja));
        await expect(current).toHaveAttribute("aria-current", "page");
        const box = await current.boundingBox();
        const sideBox = await side.boundingBox();
        expect(box.x + box.width).toBeLessThanOrEqual(sideBox.x + sideBox.width + 0.5);
        const screenshotDir = process.env.WIND_DOCS_SCREENSHOT_DIR;
        if (screenshotDir) {
          mkdirSync(screenshotDir, { recursive: true });
          await page.screenshot({
            path: join(screenshotDir, `architecture-${ja ? "ja" : "en"}-${width}-${scheme}.png`),
          });
        }
      });

for (const width of [1440, 390])
  for (const ja of [false, true])
    for (const scheme of ["light", "dark"])
      test(`Deploy rows keep clear of row controls ${ja ? "JA" : "EN"} ${width} ${scheme}`, async ({
        page,
      }) => {
        await page.emulateMedia({ colorScheme: scheme });
        await page.setViewportSize({ width, height: 950 });
        await page.goto(route("guides/ssr-and-cloudflare-bindings", ja));
        const side = await sidebarOf(page, width);
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
          true,
        );
        await expect(
          treeOf(side).locator(linkTo("guides/ssr-and-cloudflare-bindings", ja)),
        ).toHaveAttribute("aria-current", "page");
        // Measure in one frame: the mobile drawer may still be sliding in.
        const hrefs = [route("deploy-embed", ja), ...DEPLOY_SLUGS.map((slug) => route(slug, ja))];
        const layout = await side.evaluate((surface, wanted) => {
          const tree = surface.querySelector("[data-sidebar-tree-root]");
          const bounds = surface.getBoundingClientRect();
          const links = wanted.map((href) =>
            [...tree.querySelectorAll("a[href]")].find(
              (a) => a.getAttribute("href").replace(/\/$/, "") === href,
            ),
          );
          const [rootLink] = links;
          const controls = [...rootLink.parentElement.children].filter((el) =>
            el.matches("[data-sidebar-focus], button[aria-expanded]"),
          );
          return {
            rootRight: rootLink.getBoundingClientRect().right,
            controlLefts: controls.map((el) => el.getBoundingClientRect().left),
            overflow: links.map((a) => a.getBoundingClientRect().right - bounds.right),
          };
        }, hrefs);
        expect(layout.controlLefts).toHaveLength(2);
        for (const left of layout.controlLefts)
          expect(layout.rootRight).toBeLessThanOrEqual(left + 0.5);
        for (const [index, overflow] of layout.overflow.entries())
          expect(overflow, hrefs[index]).toBeLessThanOrEqual(0.5);
        const screenshotDir = process.env.WIND_DOCS_SCREENSHOT_DIR;
        if (screenshotDir) {
          mkdirSync(screenshotDir, { recursive: true });
          await page.screenshot({
            path: join(screenshotDir, `deploy-${ja ? "ja" : "en"}-${width}-${scheme}.png`),
          });
        }
      });

// The Header/Sidebar pairs are cached while the site is built, so check the
// emitted contexts: each editorial forest has one context, shared by every
// navigation surface on a page and never reused by another section.
test("built pages carry one sidebar context per editorial forest", async ({ request }) => {
  const contextsOf = async (slug, ja) => {
    const html = await (await request.get(route(slug, ja))).text();
    const contexts = [...html.matchAll(/"navigation":\{"id":"([^"]+)"/g)].map((m) => m[1]);
    expect(contexts.length, slug).toBeGreaterThan(0);
    expect(new Set(contexts).size, slug).toBe(1);
    return contexts[0];
  };
  for (const ja of [false, true]) {
    const sections = {
      architecture: [
        "architecture",
        "architecture/fundamentals",
        "architecture/routing",
        "architecture/three-ways-to-run-zfb",
      ],
      build: ["build-with-zfb", "build-with-zfb/pages-routes", "guides/troubleshooting"],
      wind: ["zudo-wind/learn", "zudo-wind/overview"],
      deploy: ["deploy-embed", ...DEPLOY_GUIDES],
      gettingStarted: ["getting-started"],
    };
    const ids = {};
    for (const [section, slugs] of Object.entries(sections)) {
      const contexts = new Set();
      for (const slug of slugs) contexts.add(await contextsOf(slug, ja));
      expect(contexts.size, section).toBe(1);
      ids[section] = [...contexts][0];
      expect(ids[section].includes(ja ? ":ja:" : ":en:"), ids[section]).toBe(true);
    }
    expect(new Set(Object.values(ids)).size).toBe(Object.keys(ids).length);
  }
});
