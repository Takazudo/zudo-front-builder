import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { exampleSource } from "../../docs/scripts/wind-preview-assets.mjs";
import { contrastRatioSrgb } from "./srgb-contrast.mjs";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const BASE_PATH = normalizeBase(process.env.WIND_DOCS_BASE ?? "/");
const PREVIEW_PORT = Number.parseInt(process.env.WIND_DOCS_PREVIEW_PORT ?? "4333", 10);
const PREVIEW_ORIGIN = `http://127.0.0.1:${PREVIEW_PORT}`;
const SCREENSHOT_DIR =
  process.env.WIND_DOCS_SCREENSHOT_DIR ?? join(tmpdir(), "zfb-wind-doc-preview-screenshots");
const CAPTURE_SCREENSHOTS = process.env.WIND_DOCS_CAPTURE_SCREENSHOTS !== "0";
const BASE_LABEL =
  BASE_PATH === "/" ? "root" : BASE_PATH.replace(/^\/+|\/+$/g, "").replaceAll("/", "-");
const PREVIEW_MARKER = '[data-zfb-island="HtmlPreviewWrapperInner"]';
const ANCHOR_BASELINE = JSON.parse(
  readFileSync(
    join(REPO_ROOT, "docs/scripts/__tests__/fixtures/wind-built-anchors.v1.json"),
    "utf8",
  ),
);
const MANIFEST = JSON.parse(
  readFileSync(join(REPO_ROOT, "docs/public/wind-examples/manifest.json"), "utf8"),
);

function normalizeBase(value) {
  if (value === "" || value === "/") return "/";
  const segments = value.replace(/^\/+|\/+$/g, "").split("/");
  if (
    segments.some((segment) => !segment || segment === "." || segment === "..") ||
    segments.some((segment) => !/^[A-Za-z0-9._~-]+$/.test(segment))
  ) {
    throw new Error(`WIND_DOCS_BASE must be a local URL path, received ${JSON.stringify(value)}`);
  }
  return `/${segments.join("/")}/`;
}

function publicPath(path) {
  return `${BASE_PATH}${path.replace(/^\/+/, "")}`;
}

function digest(text) {
  return createHash("sha256").update(text).digest("hex");
}

const LOCALES = {
  en: {
    segment: "",
    lang: "en",
    viewportLabel: "Viewport size",
    mobile: "Mobile",
    tablet: "Tablet",
    full: "Full",
    showCode: "Show code",
    hideCode: "Hide code",
    dark: "Dark",
    oldHashes: { gap: "gap-accepted-values", padding: "p-accepted-values" },
  },
  ja: {
    segment: "ja/",
    lang: "ja",
    viewportLabel: "ビューポートサイズ",
    mobile: "モバイル",
    tablet: "タブレット",
    full: "フル",
    showCode: "コードを表示",
    hideCode: "コードを非表示",
    dark: "ダーク",
    oldHashes: { gap: "gap-受け入れる値", padding: "p-受け入れる値" },
  },
};

const FAMILIES = {
  gap: {
    spacing: {
      "grid-gap": ["16px", "16px"],
      "axis-gap": ["32px", "8px"],
      "custom-gap": ["24px", "24px"],
      "arbitrary-gap": ["18px", "18px"],
    },
  },
  padding: {
    padding: {
      "all-sides": ["16px", "16px", "16px", "16px"],
      "axis-padding": ["8px", "32px", "8px", "32px"],
      "edge-padding": ["8px", "16px", "24px", "32px"],
      "custom-padding": ["24px", "24px", "24px", "24px"],
      "arbitrary-padding": ["18px", "18px", "18px", "18px"],
    },
  },
};

function examplesFor(family) {
  const record = JSON.parse(
    readFileSync(join(REPO_ROOT, `docs/wind-examples/${family}.json`), "utf8"),
  );
  const examples = record.examples.map((example) => {
    const asset = MANIFEST.examples.find(
      (item) => item.family === family && item.id === example.id,
    );
    assert.ok(asset, `asset manifest entry is missing for ${family}/${example.id}`);
    assert.equal(
      asset.kind,
      "positive",
      `${family}/${example.id} is not a runnable positive sample`,
    );
    const html = readFileSync(join(REPO_ROOT, "docs/public", asset.htmlPath), "utf8");
    const css = readFileSync(join(REPO_ROOT, "docs/public", asset.cssPath), "utf8");
    assert.equal(
      html,
      exampleSource(example),
      `${family}/${example.id} HTML asset differs from its source record`,
    );
    assert.equal(digest(html), asset.htmlSha256, `${family}/${example.id} HTML digest is stale`);
    assert.equal(digest(css), asset.cssSha256, `${family}/${example.id} CSS digest is stale`);
    return { ...example, asset, html, css };
  });

  assert.deepEqual(
    examples.map((example) => example.id),
    Object.keys(FAMILIES[family].spacing ?? FAMILIES[family].padding),
    `${family} preview records are in the accepted sample order`,
  );
  return examples;
}

const CASES = Object.entries(LOCALES).flatMap(([locale, labels]) =>
  Object.keys(FAMILIES).map((family) => {
    const examples = examplesFor(family);
    const route = `docs/zudo-wind/utilities/${family}/`;
    const localePath = `${labels.segment}${route}`;
    const baselineRoute = `${labels.segment}${route}index.html`;
    assert.ok(
      ANCHOR_BASELINE[baselineRoute],
      `built anchor baseline is missing for ${baselineRoute}`,
    );
    return { locale, labels, family, examples, route, localePath, baselineRoute };
  }),
);

function collectPageErrors(page) {
  const errors = [];
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`);
  });
  page.on("pageerror", (error) => errors.push(`pageerror: ${error.message}`));
  page.on("requestfailed", (request) => {
    errors.push(`request failed: ${request.url()} (${request.failure()?.errorText ?? "unknown"})`);
  });
  page.on("response", (response) => {
    if (response.url().startsWith(`http://127.0.0.1:`) && response.status() >= 400) {
      errors.push(`HTTP ${response.status()}: ${response.url()}`);
    }
  });
  return errors;
}

function assertNoPageErrors(errors, context) {
  assert.deepEqual(errors, [], `${context} emitted browser or local-server errors`);
}

function pageUrl(testCase) {
  return publicPath(testCase.localePath.replace(/\/$/, ""));
}

async function findFrame(page, index) {
  const locator = page.locator("iframe").nth(index);
  const element = await locator.elementHandle();
  assert.ok(element, `preview iframe ${index + 1} is missing`);
  const frame = await element.contentFrame();
  assert.ok(frame, `preview iframe ${index + 1} has no browsing context`);
  await frame.waitForLoadState("domcontentloaded");
  return { locator, frame };
}

async function inspectFrame(frame) {
  const demo = frame.locator(".wind-demo");
  await expect
    .poll(() => demo.evaluate((element) => getComputedStyle(element).fontSize))
    .toBe("14px");
  return frame.evaluate(() => {
    const demo = document.querySelector(".wind-demo");
    const style = getComputedStyle(demo);
    const bodyStyle = getComputedStyle(document.body);
    return {
      bodyMargin: bodyStyle.margin,
      columnGap: style.columnGap,
      display: style.display,
      headStyleCount: document.head.querySelectorAll("style").length,
      hasLinkedStylesheet: Boolean(document.head.querySelector('link[rel="stylesheet"]')),
      htmlLang: document.documentElement.lang,
      htmlTheme: document.documentElement.getAttribute("data-theme"),
      scriptCount: document.querySelectorAll("script").length,
      paddingBottom: style.paddingBottom,
      paddingLeft: style.paddingLeft,
      paddingRight: style.paddingRight,
      paddingTop: style.paddingTop,
      rowGap: style.rowGap,
      styleText: document.head.querySelector("style")?.textContent ?? "",
    };
  });
}

async function scrollAndWaitForHydration(page, count) {
  const previews = page.locator(PREVIEW_MARKER);
  await expect(previews).toHaveCount(count);
  for (let index = 0; index < count; index += 1) {
    await page.locator("iframe").nth(index).scrollIntoViewIfNeeded();
    await expect(previews.nth(index)).toHaveAttribute("data-zfb-island-mounted", "");
  }
}

async function captureFreshScreenshot(browser, testCase, width, height) {
  const context = await browser.newContext({
    baseURL: PREVIEW_ORIGIN,
    colorScheme: "light",
    viewport: { width, height },
  });
  const page = await context.newPage();
  const errors = collectPageErrors(page);
  try {
    await page.goto(pageUrl(testCase), { waitUntil: "networkidle" });
    await expect(page.locator("iframe")).toHaveCount(testCase.examples.length);
    const firstMarker = page.locator(PREVIEW_MARKER).first();
    await page.locator("iframe").first().scrollIntoViewIfNeeded();
    await expect(firstMarker).toHaveAttribute("data-zfb-island-mounted", "");
    const first = await findFrame(page, 0);
    const firstFrameStyle = await inspectFrame(first.frame);
    assert.equal(
      firstFrameStyle.styleText,
      testCase.examples[0].css,
      `${testCase.locale}/${testCase.family} screenshot iframe CSS matches its generated asset`,
    );
    const preview = first.locator.locator("xpath=../../..");
    await expect
      .poll(() => preview.locator(".zd-html-preview-code pre.hi-root code").count())
      .toBe(2);
    await expect
      .poll(() => preview.locator(".zd-html-preview-code .code-btn-copy").count())
      .toBe(2);
    await page.waitForLoadState("networkidle");
    mkdirSync(SCREENSHOT_DIR, { recursive: true });
    const filepath = join(
      SCREENSHOT_DIR,
      `${BASE_LABEL}-${testCase.locale}-${testCase.family}-${width}x${height}-light.png`,
    );
    await page.screenshot({ path: filepath, animations: "disabled" });
    assertNoPageErrors(errors, `${testCase.locale}/${testCase.family} screenshot`);
    return filepath;
  } finally {
    await context.close();
  }
}

async function switchLocaleAndCheckRemount(page, testCase) {
  const targetLocale = testCase.locale === "en" ? "ja" : "en";
  const targetLabels = LOCALES[targetLocale];
  const switcher = page.locator("[data-language-switcher]:visible").first();
  await expect(switcher).toBeVisible();
  await switcher.locator("[data-language-toggle]").click();
  const target = switcher.locator(`a[lang="${targetLabels.lang}"]`);
  await expect(target).toBeVisible();
  await expect(target).toHaveAttribute(
    "href",
    pageUrl({
      ...testCase,
      locale: targetLocale,
      labels: targetLabels,
      localePath: `${targetLabels.segment}${testCase.route}`,
    }),
  );

  await page.evaluate(() => {
    window.__windDocAfterSwapCount = 0;
    document.addEventListener("zfb:after-swap", () => {
      window.__windDocAfterSwapCount += 1;
    });
  });
  await target.click();
  const targetPath = pageUrl({
    ...testCase,
    locale: targetLocale,
    labels: targetLabels,
    localePath: `${targetLabels.segment}${testCase.route}`,
  });
  await expect(page).toHaveURL(new RegExp(`${escapeRegExp(targetPath)}$`));
  await expect.poll(() => page.evaluate(() => window.__windDocAfterSwapCount)).toBeGreaterThan(0);
  await expect(page.locator("html")).toHaveAttribute("lang", targetLabels.lang);
  await scrollAndWaitForHydration(page, testCase.examples.length);

  const reverseSwitcher = page.locator("[data-language-switcher]:visible").first();
  await reverseSwitcher.locator("[data-language-toggle]").click();
  const returnLink = reverseSwitcher.locator(`a[lang="${testCase.labels.lang}"]`);
  await expect(returnLink).toBeVisible();
  await returnLink.click();
  await expect(page).toHaveURL(new RegExp(`${escapeRegExp(pageUrl(testCase))}$`));
  await expect.poll(() => page.evaluate(() => window.__windDocAfterSwapCount)).toBeGreaterThan(1);
  await expect(page.locator("html")).toHaveAttribute("lang", testCase.labels.lang);
  await scrollAndWaitForHydration(page, testCase.examples.length);
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

async function checkOldTechnicalHash(page, testCase) {
  const id = testCase.labels.oldHashes[testCase.family];
  await page.goto(`${pageUrl(testCase)}#${encodeURIComponent(id)}`, { waitUntil: "networkidle" });
  const heading = page.locator(`#${id}`);
  const disclosure = heading.locator("xpath=ancestor::details[1]");
  await expect(heading).toHaveCount(1);
  await expect(disclosure).toHaveCount(1);
  await expect(disclosure).toHaveAttribute("open", "");
  await expect(heading).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`#${escapeRegExp(encodeURIComponent(id))}$`));
}

async function checkNativeCopyAfterRemount(page, testCase) {
  const preview = page.locator("iframe").first().locator("xpath=../../..");
  const codeBlocks = preview.locator(".zd-html-preview-code pre.hi-root code");
  const copyButtons = preview.locator(".zd-html-preview-code .code-btn-copy");
  await expect.poll(() => codeBlocks.count()).toBe(2);
  await expect.poll(() => copyButtons.count()).toBe(2);
  const code = codeBlocks.first();
  const copy = copyButtons.first();
  await expect(code).toHaveText(testCase.examples[0].html.trim());
  await code.hover();
  await expect(copy).toBeVisible();
  await copy.click();
  await expect
    .poll(() => page.evaluate(() => navigator.clipboard.readText()))
    .toBe(testCase.examples[0].html.trim());
}

async function checkSharedNativeWrapState(page) {
  const originalViewport = page.viewportSize();
  assert.ok(originalViewport, "the browser page has a configured viewport");
  const staticCssCode = page
    .locator("details pre.hi-root code")
    .filter({ hasText: ".wind-demo-cell {" })
    .first();
  await expect(staticCssCode).toHaveCount(1);
  const staticDetails = staticCssCode.locator("xpath=ancestor::details[1]");
  await staticDetails.locator("summary").click();
  await expect(staticDetails).toHaveAttribute("open", "");

  try {
    await page.setViewportSize({ width: 390, height: originalViewport.height });
    const preview = page.locator("iframe").first().locator("xpath=../../..");
    const previewCssCode = preview.locator(".zd-html-preview-code pre.hi-root code").nth(1);
    const staticPre = staticCssCode.locator("xpath=..");
    const previewPre = previewCssCode.locator("xpath=..");
    const staticWrap = staticCssCode.locator("xpath=../..").locator(".code-btn-wrap");
    const previewWrap = previewCssCode.locator("xpath=../..").locator(".code-btn-wrap");

    await expect.poll(() => previewCssCode.count()).toBe(1);
    await expect.poll(() => staticWrap.count()).toBe(1);
    await expect.poll(() => previewWrap.count()).toBe(1);
    await expect(previewWrap).toBeVisible();
    const storedWrapMode = await page.evaluate(() => sessionStorage.getItem("zudo-doc-code-wrap"));
    if (storedWrapMode === "1") await staticWrap.evaluate((button) => button.click());
    await expect(staticWrap).toHaveAttribute("aria-pressed", "false");
    await expect(previewWrap).toHaveAttribute("aria-pressed", "false");

    await staticWrap.evaluate((button) => button.click());
    await expect(staticWrap).toHaveAttribute("aria-pressed", "true");
    await expect(previewWrap).toHaveAttribute("aria-pressed", "true");
    await expect
      .poll(() => staticPre.evaluate((pre) => pre.classList.contains("word-wrap")))
      .toBe(true);
    await expect
      .poll(() => previewPre.evaluate((pre) => pre.classList.contains("word-wrap")))
      .toBe(true);
    await expect
      .poll(() => page.evaluate(() => sessionStorage.getItem("zudo-doc-code-wrap")))
      .toBe("1");

    await previewWrap.evaluate((button) => button.click());
    await expect(staticWrap).toHaveAttribute("aria-pressed", "false");
    await expect(previewWrap).toHaveAttribute("aria-pressed", "false");
    await expect
      .poll(() => staticPre.evaluate((pre) => pre.classList.contains("word-wrap")))
      .toBe(false);
    await expect
      .poll(() => previewPre.evaluate((pre) => pre.classList.contains("word-wrap")))
      .toBe(false);
    await expect
      .poll(() => page.evaluate(() => sessionStorage.getItem("zudo-doc-code-wrap")))
      .toBe("0");
  } finally {
    await page.setViewportSize(originalViewport);
  }

  await staticDetails.locator("summary").click();
  await expect(staticDetails).not.toHaveAttribute("open", "");
}

async function checkPreviewSource(page, preview, example, index, labels) {
  const sourceToggle = preview.locator("button[aria-expanded]").first();
  await expect(sourceToggle).toHaveAttribute("aria-expanded", "true");
  const codeBlocks = preview.locator("pre code");
  await expect(codeBlocks).toHaveCount(2);
  await expect(preview.getByText("HTML", { exact: true })).toBeVisible();
  await expect(preview.getByText("CSS", { exact: true })).toBeVisible();
  const expectedHtml = example.html.trim();
  const expectedCss = example.css.trim();
  await expect
    .poll(() => codeBlocks.nth(0).evaluate((node) => node.textContent))
    .toBe(expectedHtml);
  await expect.poll(() => codeBlocks.nth(1).evaluate((node) => node.textContent)).toBe(expectedCss);

  if (index === 0) {
    const clipboardExamples = [expectedHtml, expectedCss];
    for (let codeIndex = 0; codeIndex < clipboardExamples.length; codeIndex += 1) {
      const code = codeBlocks.nth(codeIndex);
      await code.hover();
      const copy = preview.locator(".code-btn-copy").nth(codeIndex);
      await expect(copy).toBeVisible();
      await expect(copy).toHaveAttribute("aria-label", "Copy code");
      await copy.click();
      await expect
        .poll(() => page.evaluate(() => navigator.clipboard.readText()))
        .toBe(clipboardExamples[codeIndex]);
    }

    const sourceButton = preview.locator("button[aria-expanded]").first();
    await sourceButton.focus();
    await page.keyboard.press("Space");
    await expect(sourceButton).toHaveAttribute("aria-expanded", "false");
    await expect(sourceButton).toHaveText(new RegExp(escapeRegExp(labels.showCode)));
    await sourceButton.focus();
    await page.keyboard.press("Enter");
    await expect(sourceButton).toHaveAttribute("aria-expanded", "true");

    const controls = preview.getByRole("group", { name: labels.viewportLabel });
    await expect(controls).toBeVisible();
    await expect(controls.getByRole("button", { name: labels.tablet })).toBeVisible();
    const mobile = controls.getByRole("button", { name: labels.mobile });
    const mobileBounds = await mobile.boundingBox();
    assert.ok(mobileBounds && mobileBounds.width >= 44 && mobileBounds.height >= 44);
    await mobile.focus();
    await page.keyboard.press("Enter");
    await expect(mobile).toHaveAttribute("aria-pressed", "true");
    await expect(preview.locator(".resize-x")).toHaveAttribute("style", /width:\s*320px/);
    const full = controls.getByRole("button", { name: labels.full });
    await full.click();
    await expect(full).toHaveAttribute("aria-pressed", "true");
    await expect(preview.locator(".resize-x")).toHaveAttribute("style", /width:\s*100%/);
  }
}

async function readResolvedThemeColors(page) {
  return page.evaluate(() => {
    const probe = document.createElement("div");
    probe.setAttribute("aria-hidden", "true");
    probe.style.cssText =
      "position:fixed;visibility:hidden;pointer-events:none;color:var(--color-fg);background-color:var(--color-bg)";
    document.body.append(probe);
    try {
      const styles = getComputedStyle(probe);
      const foreground = styles.color;
      const background = styles.backgroundColor;
      const canvas = document.createElement("canvas");
      canvas.width = 1;
      canvas.height = 1;
      const context = canvas.getContext("2d", { colorSpace: "srgb" });
      if (!context) throw new Error("sRGB canvas context is unavailable");
      const sample = (color) => {
        context.clearRect(0, 0, 1, 1);
        context.fillStyle = color;
        context.fillRect(0, 0, 1, 1);
        return Array.from(context.getImageData(0, 0, 1, 1).data);
      };
      const foregroundSrgb = sample(foreground);
      const backgroundSrgb = sample(background);
      return {
        background,
        backgroundSrgb,
        colorScheme: getComputedStyle(document.documentElement).colorScheme,
        foreground,
        foregroundSrgb,
        theme: document.documentElement.getAttribute("data-theme"),
      };
    } finally {
      probe.remove();
    }
  });
}

function assertOpaqueThemeColors(colors, context) {
  assert.ok(colors.foreground, `${context} foreground token resolves`);
  assert.ok(colors.background, `${context} background token resolves`);
  assert.equal(colors.foregroundSrgb[3], 255, `${context} foreground is opaque`);
  assert.equal(colors.backgroundSrgb[3], 255, `${context} background is opaque`);
}

async function checkThemeContrast(page, previewFrame, labels) {
  const before = await readResolvedThemeColors(page);
  assertOpaqueThemeColors(before, "light theme");
  assert.notEqual(before.theme, "dark", "light scheme is active before the contrast check");
  assert.ok(
    contrastRatioSrgb(before.foregroundSrgb, before.backgroundSrgb) >= 4.5,
    "light theme foreground/background meet WCAG AA contrast for text",
  );

  const previewColorBefore = await previewFrame.evaluate(
    () => getComputedStyle(document.querySelector(".wind-demo")).color,
  );
  const toggle = page.locator("[data-zd-theme-menu]:visible > button").first();
  await expect(toggle).toBeVisible();
  await toggle.click();
  await page.getByRole("menuitemradio", { name: labels.dark, exact: true }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");

  const after = await readResolvedThemeColors(page);
  assertOpaqueThemeColors(after, "dark theme");
  assert.equal(after.theme, "dark", "dark scheme is active");
  assert.notDeepEqual(
    after.backgroundSrgb,
    before.backgroundSrgb,
    "dark theme changes the docs surface",
  );
  assert.notDeepEqual(
    after.foregroundSrgb,
    before.foregroundSrgb,
    "dark theme changes the docs text color",
  );
  assert.ok(
    contrastRatioSrgb(after.foregroundSrgb, after.backgroundSrgb) >= 4.5,
    "dark theme foreground/background meet WCAG AA contrast for text",
  );
  const previewColorAfter = await previewFrame.evaluate(
    () => getComputedStyle(document.querySelector(".wind-demo")).color,
  );
  assert.equal(
    previewColorAfter,
    previewColorBefore,
    "host theme does not leak into the isolated wind frame",
  );
  assert.equal(
    await previewFrame.evaluate(() => document.documentElement.getAttribute("data-theme")),
    null,
    "the iframe has no host color-scheme attribute",
  );
}

for (const testCase of CASES) {
  test(`${testCase.locale} ${testCase.family}: installed HtmlPreview renders authentic source and wind CSS`, async ({
    browser,
    page,
  }) => {
    const errors = collectPageErrors(page);
    await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
    await page.goto(pageUrl(testCase), { waitUntil: "networkidle" });

    await expect(page.locator("html")).toHaveAttribute("lang", testCase.labels.lang);
    await expect(page.locator(PREVIEW_MARKER)).toHaveCount(testCase.examples.length);
    await expect(page.locator("iframe")).toHaveCount(testCase.examples.length);

    for (let index = 0; index < testCase.examples.length; index += 1) {
      const example = testCase.examples[index];
      const { locator, frame } = await findFrame(page, index);
      await locator.scrollIntoViewIfNeeded();
      await expect(page.locator(PREVIEW_MARKER).nth(index)).toHaveAttribute(
        "data-zfb-island-mounted",
        "",
      );

      const style = await inspectFrame(frame);
      assert.equal(
        style.bodyMargin,
        "8px",
        "preflight=false leaves the browser body margin intact",
      );
      assert.equal(
        style.headStyleCount,
        1,
        "the preview frame only contains its authored wind CSS",
      );
      assert.equal(
        style.hasLinkedStylesheet,
        false,
        "the frame does not load host or Tailwind stylesheets",
      );
      assert.equal(style.scriptCount, 0, "the authored preview document contains no scripts");
      assert.equal(style.htmlLang, testCase.labels.lang);
      assert.equal(style.htmlTheme, null, "the iframe does not inherit host theme attributes");
      assert.equal(
        style.styleText,
        example.css,
        `${example.id} frame CSS matches its generated asset`,
      );

      if (testCase.family === "gap") {
        assert.equal(style.display, "grid", `${example.id} emits a grid container`);
        assert.equal(
          style.columnGap,
          FAMILIES.gap.spacing[example.id][0],
          `${example.id} column gap`,
        );
        assert.equal(style.rowGap, FAMILIES.gap.spacing[example.id][1], `${example.id} row gap`);
      } else {
        const expected = FAMILIES.padding.padding[example.id];
        assert.deepEqual(
          [style.paddingTop, style.paddingRight, style.paddingBottom, style.paddingLeft],
          expected,
          `${example.id} computed padding`,
        );
      }

      const preview = locator.locator("xpath=../../..");
      await checkPreviewSource(page, preview, example, index, testCase.labels);
    }

    const firstFrame = await findFrame(page, 0);
    await checkThemeContrast(page, firstFrame.frame, testCase.labels);
    await checkOldTechnicalHash(page, testCase);
    await switchLocaleAndCheckRemount(page, testCase);
    await checkNativeCopyAfterRemount(page, testCase);
    await checkSharedNativeWrapState(page);

    if (CAPTURE_SCREENSHOTS) {
      await captureFreshScreenshot(browser, testCase, 1440, 1000);
      await captureFreshScreenshot(browser, testCase, 390, 844);
    }

    assertNoPageErrors(errors, `${testCase.locale}/${testCase.family}`);
  });
}
