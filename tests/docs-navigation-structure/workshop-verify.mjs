// Supplemental actual-host acceptance for #4079; manager runs in the guarded browser lane.
import { chromium, expect, webkit } from "@playwright/test";
import assert from "node:assert/strict";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  createState,
  getSeedFiles,
  setValue,
  zipData,
} from "../../docs/src/components/playground/design-workshop/model.js";
import { WIND_UTILITY_GROUPS } from "../../docs/src/config/navigation-groups.mjs";

const origin = process.env.DOCS_VERIFY_ORIGIN ?? "http://127.0.0.1:4333";
const output = process.env.DOCS_VERIFY_OUTPUT ?? "/tmp/zfb-docs-verification";
mkdirSync(output, { recursive: true });
const evidence = {
  commit: execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim(),
  origin,
  started: new Date().toISOString(),
  checks: [],
  failures: [],
  pageErrors: [],
};
const browserName = process.env.DOCS_VERIFY_BROWSER ?? "chromium";
const browserType = { chromium, webkit }[browserName];
if (!browserType)
  throw new Error(`DOCS_VERIFY_BROWSER must be chromium or webkit, got ${browserName}`);
const browser = await browserType.launch();
evidence.browser = browserName;
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
async function scenario(name, width, locale, run) {
  const context = await browser.newContext({
    viewport: { width, height: 1000 },
    reducedMotion: "reduce",
    colorScheme: "light",
    acceptDownloads: true,
  });
  const page = await context.newPage();
  page.setDefaultTimeout(15000);
  page.on("pageerror", (error) =>
    evidence.pageErrors.push({ name, width, locale, message: error.message }),
  );
  const record = { name, width, locale: locale || "en", status: "running", details: [] };
  evidence.checks.push(record);
  try {
    await run(page, record);
    record.status = "passed";
  } catch (error) {
    record.status = "failed";
    record.error = error.stack ?? String(error);
    evidence.failures.push({ name, width, locale, message: error.message });
    await page
      .screenshot({
        path: `${output}/failure-${name}-${locale ? "ja" : "en"}-${width}.png`,
        fullPage: true,
      })
      .catch(() => {});
  } finally {
    await context.close();
    writeFileSync(`${output}/workshop.json`, JSON.stringify(evidence, null, 2));
  }
}
const routeLink = (page, path, scope = page) =>
  scope.locator(`a[href="${path}"],a[href="${path.replace(/\/$/, "")}"]`).first();
async function workshop(page, record, locale) {
  const path = `${locale}/docs/playground/design-system/`;
  await page.goto(origin + path);
  const root = page.locator("#design-workshop");
  const frame = page.frameLocator("#design-preview");
  await expect(root.locator("#start-view")).toBeVisible();
  await expect(root.locator("[data-preset]")).toHaveCount(3);
  for (const preset of ["everyday", "editorial", "workbench"]) {
    await root.locator('[data-view="start"]').first().click();
    await root.locator(`[data-preset="${preset}"]`).click();
    await expect(root.locator("#chosen-name")).toHaveText(
      preset[0].toUpperCase() + preset.slice(1),
    );
    await root.locator('[data-view="playground"]').first().click();
    await expect(root.locator("#editor-preset")).toHaveValue(preset);
    await root.locator("#accent-hex").fill("#963142");
    await root.locator("#group-gap").fill("24");
    await expect(frame.locator("html")).toHaveCSS("--ds-brand", "#963142");
    await expect(frame.locator(".cards")).toHaveCSS("row-gap", "24px");
    await expect(frame.locator(".cards")).toHaveCSS(
      "column-gap",
      preset === "editorial" ? "20px" : preset === "workbench" ? "12px" : "16px",
    );
    await root.locator("#accent-hex").fill("#xx");
    await expect(root.locator("#accent-hex")).toHaveAttribute("aria-invalid", "true");
    await root.locator("#accent-hex").blur();
    await expect(root.locator("#accent-hex")).toHaveValue("#963142");
    const expected = createState(preset);
    setValue(expected, "accent", "#963142");
    setValue(expected, "groupGap", 24);
    const files = getSeedFiles(expected);
    await root.locator('[data-action="export"]').first().click();
    for (const [name, contents] of Object.entries(files)) {
      await root.locator(`[data-export-file="${name}"]`).click();
      // Exact bytes, including whitespace, rather than a loose rendered-text match.
      await expect.poll(() => root.locator("#export-code").textContent()).toBe(contents);
    }
    const pending = page.waitForEvent("download");
    await root.locator("#download-seed").click();
    const download = await pending;
    const bytes = readFileSync(await download.path());
    assert.deepEqual(bytes, Buffer.from(zipData(files)));
    record.details.push({
      preset,
      exportedFiles: Object.keys(files),
      download: download.suggestedFilename(),
      zipSha256: sha256(bytes),
    });
    await root.locator("#export-dialog [data-close]").click();
    for (const mode of ["components", "foundations", "page"]) {
      await root.locator(`[data-preview="${mode}"]`).click();
      await expect(frame.locator("html")).toHaveCSS("--ds-brand", "#963142");
      await expect(frame.locator("body")).not.toBeEmpty();
    }
    await root.locator('[data-action="reset"]').click();
    await expect(root.locator("#accent-hex")).toHaveValue(createState(preset).values.accent);
    await expect(root.locator("#group-gap")).toHaveValue(
      String(createState(preset).values.groupGap),
    );
  }
  await root.locator("#editor-preset").selectOption("editorial");
  await root.locator("#accent-hex").fill("#963142");
  await root.locator('[data-role="body"]').focus();
  await page.keyboard.press("Enter");
  await expect(root.locator("#token-chain")).toContainText("--ds-font-body");
  await root.locator('[data-view="starter"]').first().click();
  const starter = page.frameLocator("#starter-preview");
  await expect(starter.locator("body")).not.toHaveClass(/inspect-on/);
  await starter.getByRole("link", { name: "Journal", exact: true }).click();
  assert.equal(page.url(), origin + path);
  await starter.locator("#sample-email").fill("local@example.com");
  await starter.getByRole("button", { name: "Try signup" }).click();
  await expect(starter.locator("#signup-status")).toContainText("No signup was submitted");
  await root.locator('[data-view="playground"]').first().click();
  const themes = [
    {
      mode: "default",
      theme: await page.locator("html").getAttribute("data-theme"),
      controlColor: await root.evaluate((el) => getComputedStyle(el).color),
    },
  ];
  for (const theme of ["light", "dark"]) {
    // Native mobile appearance controls live in the drawer footer. Its translated
    // offscreen tree still matches :visible before opening; do not force-click it.
    const mobile = record.width < 1024;
    if (mobile) {
      await page.getByRole("button", { name: /Open sidebar|サイドバーを開く/ }).click();
      await expect(page.locator("[data-zd-mobile-sidebar]")).not.toHaveAttribute("inert");
    }
    const themeScope = page.locator(mobile ? "[data-zd-mobile-sidebar]" : "header");
    await themeScope.locator("[data-zd-theme-menu]:visible > button").first().click();
    await page
      .getByRole("menuitemradio", {
        name: theme === "light" ? /^(Light|ライト)$/ : /^(Dark|ダーク)$/,
      })
      .click();
    await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
    if (mobile)
      await page.getByRole("button", { name: /Close sidebar|サイドバーを閉じる/ }).click();
    await expect(frame.locator("body")).toHaveCSS("background-color", "rgb(250, 248, 242)");
    themes.push({
      mode: theme,
      controlColor: await root.evaluate((el) => getComputedStyle(el).color),
    });
    await page.screenshot({
      path: `${output}/workshop-controls-${locale ? "ja" : "en"}-${record.width}-${theme}.png`,
    });
  }
  assert.notEqual(themes[1].controlColor, themes[2].controlColor);
  record.details.push({ themes });
  await root.locator('[data-width="mobile"]').click();
  await expect(root.locator("#frame-shell")).toHaveClass(/phone/);
  await page.evaluate(() => {
    window.__workshopNavigationSentinel = "alive";
    window.__oldWorkshopRoot = document.querySelector("#design-workshop");
  });
  if (record.width < 1024) {
    await page.getByRole("button", { name: /Open sidebar|サイドバーを開く/ }).click();
    await expect(page.locator("[data-zd-mobile-sidebar]")).not.toHaveAttribute("inert");
  }
  await page
    .locator(
      `a[href="${locale}/docs/playground/"]:visible,a[href="${locale}/docs/playground"]:visible`,
    )
    .first()
    .click();
  await expect(page).toHaveURL(origin + `${locale}/docs/playground/`);
  await expect(root).toHaveCount(0);
  assert.equal(await page.evaluate(() => window.__oldWorkshopRoot.childElementCount), 0);
  await routeLink(page, path, page.locator("main")).click();
  await expect(root.locator("#accent-hex")).toHaveValue("#963142");
  await expect(root.locator("#editor-preset")).toHaveValue("editorial");
  await expect(root.locator("#frame-shell")).toHaveClass(/phone/);
  assert.equal(await page.evaluate(() => window.__workshopNavigationSentinel), "alive");
  await expect(root).toHaveCount(1);
  await expect(page.locator('[data-zfb-island="DesignSystemPlayground"]')).toHaveCount(1);
  await root.locator('[data-action="inspect"]').click();
  await expect(root.locator("#inspect-button")).toHaveAttribute("aria-pressed", "true");
  record.details.push(
    "Native links restore exactly one live workshop with preset, token, view and width state",
  );
}

const resetScopeState = (page) =>
  page.evaluate(() => sessionStorage.removeItem("zd-sidebar-scope"));
const sidebarLinks = (sidebar, prefix) =>
  sidebar.locator("[data-sidebar-tree-root] a[href]").evaluateAll(
    (nodes, prefix) =>
      nodes
        .map((node) => node.getAttribute("href"))
        .filter((href) => href.startsWith(prefix))
        .map((href) => href.replace(/\/$/, "")),
    prefix,
  );
const exactLink = (sidebar, path) => sidebar.locator(`a[href="${path}"], a[href="${path}/"]`);
async function withSidebar(page, mobile, check) {
  const sidebar = page.locator(mobile ? "[data-zd-mobile-sidebar]" : "#desktop-sidebar");
  if (mobile) await page.getByRole("button", { name: /Open sidebar|サイドバーを開く/ }).click();
  await check(sidebar);
  if (mobile) await page.getByRole("button", { name: /Close sidebar|サイドバーを閉じる/ }).click();
}

async function wind(page, record, locale) {
  const prefix = `${locale}/docs/`;
  for (const group of WIND_UTILITY_GROUPS) {
    await page.goto(origin + prefix + "zudo-wind/utilities/");
    await page.evaluate(() => {
      window.__windNavigationSentinel = "alive";
    });
    const mobile = record.width < 1024;
    // 06R node baseline: the linked group root, then its family pages.
    const localView = [group.slug, ...group.members].map((slug) => prefix + slug);
    // Fresh 06R scope storage per page keeps the exact link list deterministic.
    await resetScopeState(page);
    await routeLink(page, prefix + group.slug + "/", page.locator("main")).click();
    await expect(page).toHaveURL(origin + prefix + group.slug + "/");
    await withSidebar(page, mobile, async (sidebar) => {
      // The group page is the linked scope root: listed once and current immediately.
      await expect.poll(() => sidebarLinks(sidebar, prefix)).toEqual(localView);
      await expect(exactLink(sidebar, prefix + group.slug)).toHaveAttribute("aria-current", "page");
      // The first Broaden reaches Utility reference; Restore returns to the group baseline.
      await sidebar.locator("[data-sidebar-broaden]").click();
      await expect
        .poll(async () => (await sidebarLinks(sidebar, prefix))[0])
        .toBe(prefix + "zudo-wind/utilities");
      await expect(exactLink(sidebar, prefix + group.slug)).toHaveAttribute("aria-current", "page");
      await sidebar.locator("[data-sidebar-restore]").click();
      await expect.poll(() => sidebarLinks(sidebar, prefix)).toEqual(localView);
    });
    const family = group.id === "sizing-spacing" ? "zudo-wind/utilities/gap" : group.members[0];
    await resetScopeState(page);
    await routeLink(page, prefix + family + "/", page.locator("main")).click();
    await expect(page).toHaveURL(origin + prefix + family + "/");
    let links;
    await withSidebar(page, mobile, async (sidebar) => {
      // Exactly the linked group root and its family pages: no leaked subtree.
      await expect.poll(() => sidebarLinks(sidebar, prefix)).toEqual(localView);
      links = await sidebarLinks(sidebar, prefix);
      await expect(exactLink(sidebar, prefix + family)).toHaveAttribute("aria-current", "page");
    });
    assert.equal(await page.evaluate(() => window.__windNavigationSentinel), "alive");
    if (group.id === "sizing-spacing") {
      const preview = page.locator('[data-zfb-island="HtmlPreviewWrapperInner"]').first();
      await preview.scrollIntoViewIfNeeded();
      await expect(preview).toHaveAttribute("data-zfb-island-mounted", "");
      const toggle = preview.locator("button[aria-expanded]").first();
      await expect(toggle).toHaveAttribute("aria-expanded", "true");
      await toggle.click();
      await expect(toggle).toHaveAttribute("aria-expanded", "false");
      await toggle.click();
      await expect(toggle).toHaveAttribute("aria-expanded", "true");
      const controls = preview.getByRole("group", {
        name: locale ? "ビューポートサイズ" : "Viewport size",
      });
      for (const name of locale
        ? ["モバイル", "タブレット", "フル"]
        : ["Mobile", "Tablet", "Full"]) {
        const button = controls.getByRole("button", { name, exact: true });
        await button.click();
        await expect(button).toHaveAttribute("aria-pressed", "true");
      }
      await expect(preview.locator("iframe")).toBeVisible();
    }
    record.details.push({
      group: group.id,
      family,
      nativeLinks: links,
      previewControls: group.id === "sizing-spacing",
    });
  }
}
try {
  for (const width of [1440, 390])
    for (const locale of ["", "/ja"]) {
      await scenario("workshop", width, locale, (page, record) => workshop(page, record, locale));
      await scenario("wind", width, locale, (page, record) => wind(page, record, locale));
    }
} finally {
  evidence.finished = new Date().toISOString();
  writeFileSync(`${output}/workshop.json`, JSON.stringify(evidence, null, 2));
  await browser.close();
}
console.log(JSON.stringify(evidence, null, 2));
assert.deepEqual(evidence.failures, []);
assert.deepEqual(evidence.pageErrors, []);
