import { test, expect } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// C01 Twin Joint identity on the built docs (docs/brand/README.md).
const prefix = process.env.WIND_DOCS_BASE ?? "/";
const FONT = "ZFB Brand Jost";
const FONT_PATH = `${prefix}fonts/jost-600.woff2`;
const FONT_BYTES = 19900;
const LOCALES = {
  en: {
    home: prefix,
    inner: `${prefix}docs/architecture/client-side-routing/`,
    light: "Light",
    dark: "Dark",
  },
  ja: {
    home: `${prefix}ja/`,
    inner: `${prefix}ja/docs/architecture/client-side-routing/`,
    light: "ライト",
    dark: "ダーク",
  },
};
const SYMBOL_SVGS = [
  "img/logo.svg",
  "img/brand/zfb-symbol.svg",
  "img/brand/zfb-symbol-small.svg",
  "img/brand/zfb-symbol-black.svg",
  "img/brand/zfb-symbol-white.svg",
  "favicon.svg",
];

const brandLink = (page) => page.locator("header[data-header] a[data-header-logo]");

// Each page navigates before its font body is read, so read every finished
// response's body as it arrives and await them all before asserting.
function watchFontRequests(page) {
  const pending = [];
  page.on("response", (response) => {
    const url = new URL(response.url());
    if (!/jost/i.test(url.pathname)) return;
    pending.push(
      response.body().then((body) => ({
        path: url.pathname,
        status: response.status(),
        type: response.headers()["content-type"],
        bytes: body.length,
      })),
    );
  });
  return () => Promise.all(pending);
}

// Measure only after the swap-displayed brand face and any resize-driven
// header work (the nav overflow menu) have settled.
async function settle(page) {
  await page.evaluate(async () => {
    await document.fonts.ready;
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  });
}

// The theme menu island stays aria-disabled until it hydrates.
async function themeButton(scope) {
  const button = scope.locator("[data-zd-theme-menu] > button:visible").first();
  await expect(button).not.toHaveAttribute("data-zd-pending");
  return button;
}

async function openDrawer(page) {
  const toggle = page.getByRole("button", { name: /Open sidebar|サイドバーを開く/ });
  await expect(async () => {
    await toggle.click();
    await expect(page.locator("[data-zd-mobile-sidebar]")).toBeInViewport({ timeout: 1000 });
  }).toPass();
}

function watchErrors(page) {
  const failures = [];
  page.on("pageerror", (error) => failures.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") failures.push(message.text());
  });
  return failures;
}

async function capture(page, name, locator) {
  const dir = process.env.WIND_DOCS_SCREENSHOT_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  const shot = { path: join(dir, `branding-${name}.png`) };
  await (locator ?? page).screenshot(shot);
}

// Computed style of the header name, its mask icon, and unrelated chrome text.
async function readBrand(page) {
  return brandLink(page).evaluate((anchor) => {
    const icon = anchor.querySelector("[data-header-brand-icon]");
    const iconStyle = getComputedStyle(icon);
    const anchorStyle = getComputedStyle(anchor);
    const iconBox = icon.getBoundingClientRect();
    const navItem = document.querySelector("header [data-header-nav] a");
    return {
      href: anchor.getAttribute("href"),
      font: anchorStyle.fontFamily,
      weight: anchorStyle.fontWeight,
      transform: anchorStyle.textTransform,
      tracking: anchorStyle.letterSpacing,
      color: anchorStyle.color,
      emblem: getComputedStyle(anchor, "::before").content,
      iconHidden: icon.getAttribute("aria-hidden"),
      iconMask: iconStyle.maskImage || iconStyle.webkitMaskImage,
      iconPaint: iconStyle.backgroundColor,
      iconBox: { width: iconBox.width, height: iconBox.height },
      iconCount: document.querySelectorAll("[data-header-brand-icon]").length,
      bodyFont: getComputedStyle(document.body).fontFamily,
      navFont: navItem ? getComputedStyle(navItem).fontFamily : null,
      theme: document.documentElement.getAttribute("data-theme"),
      pack: document.documentElement.getAttribute("data-theme-pack"),
    };
  });
}

function expectBrand(brand, home) {
  expect(brand.href).toBe(home);
  expect(brand.font.startsWith(`"${FONT}"`) || brand.font.startsWith(FONT)).toBe(true);
  expect(brand.weight).toBe("600");
  expect(brand.transform).toBe("none");
  expect(brand.tracking).toBe("normal");
  expect(["none", "normal"]).toContain(brand.emblem);
  expect(brand.iconCount).toBe(1);
  expect(brand.iconHidden).toBe("true");
  expect(brand.iconMask).toContain(`${prefix}img/brand/zfb-symbol-small.svg`);
  expect(brand.iconPaint).toBe(brand.color);
  expect(brand.iconBox.width).toBeGreaterThan(12);
  expect(Math.abs(brand.iconBox.width - brand.iconBox.height)).toBeLessThan(0.5);
  expect(brand.bodyFont).not.toContain(FONT);
  if (brand.navFont !== null) expect(brand.navFont).not.toContain(FONT);
}

for (const [locale, labels] of Object.entries(LOCALES)) {
  test(`header and hero carry the C01 mark and local Jost ${locale}`, async ({ page }) => {
    const fontResponses = watchFontRequests(page);
    const failures = watchErrors(page);
    await page.goto(labels.home);
    await expect(brandLink(page)).toHaveCount(1);
    await expect(
      page.locator("header[data-header]").getByRole("link", { name: "zfb", exact: true }),
    ).toHaveCount(1);
    await expect(brandLink(page)).toHaveAccessibleName("zfb");

    const loaded = await page.evaluate(async (family) => {
      const faces = await document.fonts.load(`600 16px "${family}"`, "zfb");
      return faces.map((face) => face.status);
    }, FONT);
    expect(loaded.length).toBeGreaterThan(0);
    expect(loaded.every((status) => status === "loaded")).toBe(true);
    expectBrand(await readBrand(page), labels.home);

    const hero = await page.locator("[data-zd-home-hero]").evaluate((root) => {
      const mark = root.querySelector('[aria-hidden="true"]');
      const style = getComputedStyle(mark);
      const box = mark.getBoundingClientRect();
      const name = root.querySelector("[data-zd-site-name]");
      return {
        mask: style.maskImage || style.webkitMaskImage,
        size: style.maskSize || style.webkitMaskSize,
        box: { width: box.width, height: box.height },
        name: name.textContent,
        nameFont: getComputedStyle(name).fontFamily,
        nameWeight: getComputedStyle(name).fontWeight,
      };
    });
    expect(hero.mask).toContain(`${prefix}img/logo.svg`);
    expect(hero.size).toBe("contain");
    expect(hero.box.width).toBeGreaterThan(200);
    expect(hero.name).toBe("zfb");
    expect(hero.nameFont).toContain(FONT);
    expect(hero.nameWeight).toBe("600");

    await page.goto(labels.inner);
    await settle(page);
    expectBrand(await readBrand(page), labels.home);
    const prose = await page
      .locator(".zd-content p")
      .first()
      .evaluate((p) => getComputedStyle(p).fontFamily);
    expect(prose).not.toContain(FONT);

    const fonts = await fontResponses();
    expect(fonts.length).toBeGreaterThan(0);
    for (const request of fonts) {
      expect(request.path).toBe(FONT_PATH);
      expect(request.status).toBe(200);
      expect(request.type).toContain("font/woff2");
      expect(request.bytes).toBe(FONT_BYTES);
    }
    expect(failures).toEqual([]);
  });

  test(`Light, Dark and System keep the mark in the name ink ${locale}`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: "dark" });
    await page.goto(labels.home);
    const system = await readBrand(page);
    expect(system.pack).toBe("graphite");
    expect(system.iconPaint).toBe(system.color);
    await capture(page, `header-${locale}-system-dark`, page.locator("header[data-header]"));

    await page.emulateMedia({ colorScheme: "light" });
    await expect.poll(async () => (await readBrand(page)).color).not.toBe(system.color);
    const light = await readBrand(page);
    expect(light.iconPaint).toBe(light.color);
    await capture(page, `header-${locale}-system-light`, page.locator("header[data-header]"));

    // Explicit choices override the emulated system scheme in both directions.
    const toggle = await themeButton(page.locator("header[data-header]"));
    await toggle.click();
    await page.getByRole("menuitemradio", { name: labels.dark, exact: true }).click();
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
    const dark = await readBrand(page);
    expect(dark.color).toBe(system.color);
    expect(dark.iconPaint).toBe(dark.color);
    await capture(page, `home-${locale}-dark`);

    await page.emulateMedia({ colorScheme: "dark" });
    await toggle.click();
    await page.getByRole("menuitemradio", { name: labels.light, exact: true }).click();
    await expect(page.locator("html")).not.toHaveAttribute("data-theme", "dark");
    const chosenLight = await readBrand(page);
    expect(chosenLight.color).toBe(light.color);
    expect(chosenLight.iconPaint).toBe(chosenLight.color);
  });
}

// Every control rendered in the header bar itself (the mobile drawer also lives
// in the header, off-canvas, and is checked separately once opened).
async function readHeaderLayout(page) {
  return page.evaluate(() => {
    const rect = (el) => el.getBoundingClientRect().toJSON();
    const shown = (el) => el.checkVisibility() && el.getBoundingClientRect().width > 0;
    const header = document.querySelector("header[data-header]");
    const anchor = header.querySelector("a[data-header-logo]");
    return {
      overflow: document.documentElement.scrollWidth - innerWidth,
      header: rect(header),
      anchor: rect(anchor),
      icon: rect(anchor.querySelector("[data-header-brand-icon]")),
      controls: [...header.querySelectorAll("a[href], button")]
        .filter((el) => el !== anchor && shown(el) && !el.closest("[data-zd-mobile-sidebar]"))
        .map((el) => ({
          label: el.getAttribute("aria-label") ?? el.textContent.trim(),
          ...rect(el),
        })),
    };
  });
}

// The header nav measures its overflow menu on resize, as after a real setting change.
async function enlargeText(page, width) {
  await page.addStyleTag({ content: "html { font-size: 200% !important; }" });
  await page.setViewportSize({ width: width - 1, height: 900 });
  await page.setViewportSize({ width, height: 900 });
}
// 720 CSS px at 2x is the 1440px desktop at 200% browser zoom; 320 covers 400%.
const LAYOUTS = [
  { name: "320", width: 320 },
  { name: "390", width: 390 },
  { name: "1440", width: 1440 },
  { name: "1440 at 200% zoom", width: 720, scale: 2 },
  { name: "390 with 200% text", width: 390, enlarged: true },
  { name: "1440 with 200% text", width: 1440, enlarged: true },
];

for (const { name, width, scale = 1, enlarged = false } of LAYOUTS)
  for (const [locale, labels] of Object.entries(LOCALES))
    test(`header fits ${locale} ${name}`, async ({ browser }) => {
      const context = await browser.newContext({
        viewport: { width, height: 900 },
        deviceScaleFactor: scale,
      });
      const page = await context.newPage();
      try {
        for (const route of [labels.home, labels.inner]) {
          await page.goto(route);
          if (enlarged) await enlargeText(page, width);
          await settle(page);
          const layout = await readHeaderLayout(page);
          // Text-only enlargement also widens package content (tables, the rem-sized
          // drawer); this spec owns the header, so page overflow is asserted at zoom only.
          if (!enlarged) expect(layout.overflow, route).toBeLessThanOrEqual(0);
          expect(layout.header.right).toBeLessThanOrEqual(width + 0.5);
          expect(layout.icon.width).toBeGreaterThan(12);
          expect(layout.icon.left).toBeGreaterThanOrEqual(layout.anchor.left - 0.5);
          expect(layout.icon.right).toBeLessThanOrEqual(layout.anchor.right + 0.5);
          expect(layout.icon.top).toBeGreaterThanOrEqual(layout.header.top);
          expect(layout.icon.bottom).toBeLessThanOrEqual(layout.header.bottom);
          const shown = layout.controls.map((control) => control.label);
          expect(
            shown.some((label) => /^Search$|^検索$/.test(label)),
            route,
          ).toBe(true);
          for (const control of layout.controls) {
            expect(control.left, control.label).toBeGreaterThanOrEqual(-0.5);
            expect(control.right, control.label).toBeLessThanOrEqual(width + 0.5);
            const overlaps =
              control.left < layout.anchor.right - 0.5 && control.right > layout.anchor.left + 0.5;
            expect(overlaps, control.label).toBe(false);
          }

          // Theme and language controls: header bar on desktop, the drawer below lg.
          const mobile = width < 1024;
          if (mobile) await openDrawer(page);
          const scope = page.locator(mobile ? "[data-zd-mobile-sidebar]" : "header[data-header]");
          for (const control of [
            await themeButton(scope),
            scope.locator(mobile ? "a[lang]:visible" : "[data-language-toggle]:visible").first(),
          ]) {
            await expect(control).toBeVisible();
            await control.scrollIntoViewIfNeeded();
            await expect(control).toBeInViewport({ ratio: 1 });
          }
        }
        if (name !== "390") {
          await page.goto(labels.home);
          if (enlarged) await enlargeText(page, width);
          await settle(page);
          await capture(page, `home-${locale}-${name.replaceAll(/[^0-9a-z]+/gi, "-")}`);
        }
      } finally {
        await context.close();
      }
    });

test("the brand link is keyboard-focusable", async ({ page }) => {
  await page.goto(prefix);
  await themeButton(page.locator("header[data-header]"));
  for (let step = 0; step < 8; step++) {
    await page.keyboard.press("Tab");
    if (await brandLink(page).evaluate((el) => el === document.activeElement)) break;
  }
  await expect(brandLink(page)).toBeFocused();
  expect(await brandLink(page).evaluate((el) => el.matches(":focus-visible"))).toBe(true);
});

test("favicons, social card and brand SVGs are served under the base", async ({
  page,
  request,
}) => {
  await page.goto(prefix);
  const head = await page.evaluate(() => ({
    icons: [...document.querySelectorAll('link[rel="icon"]')].map((l) => l.getAttribute("href")),
    og: document.querySelector('meta[property="og:image"]')?.getAttribute("content"),
    twitter: document.querySelector('meta[name="twitter:image"]')?.getAttribute("content"),
    card: document.querySelector('meta[name="twitter:card"]')?.getAttribute("content"),
  }));
  expect(head.icons.sort()).toEqual(
    ["favicon-16x16.png", "favicon-32x32.png", "favicon.ico", "favicon.svg"].map(
      (file) => `${prefix}${file}`,
    ),
  );
  expect(head.card).toBe("summary_large_image");
  for (const url of [head.og, head.twitter]) {
    expect(new URL(url).pathname).toBe(`${prefix}img/ogp.png`);
  }

  const pngSize = (body) => [body.readUInt32BE(16), body.readUInt32BE(20)];
  for (const [file, size] of [
    ["img/ogp.png", [1200, 630]],
    ["favicon-16x16.png", [16, 16]],
    ["favicon-32x32.png", [32, 32]],
  ]) {
    const response = await request.get(`${prefix}${file}`);
    expect(response.status(), file).toBe(200);
    expect(pngSize(await response.body()), file).toEqual(size);
  }
  const ico = await request.get(`${prefix}favicon.ico`);
  expect(ico.status()).toBe(200);
  expect((await ico.body()).readUInt16LE(4)).toBe(3);

  for (const file of [
    ...SYMBOL_SVGS,
    "img/brand/zfb-lockup.svg",
    "img/brand/zfb-lockup-white.svg",
  ]) {
    const response = await request.get(`${prefix}${file}`);
    expect(response.status(), file).toBe(200);
    const svg = await response.text();
    expect(svg, file).not.toMatch(/<(image|script|text|foreignObject|rect|mask|filter)\b|href=/);
    if (SYMBOL_SVGS.includes(file)) expect(svg.match(/<path\b/g), file).toHaveLength(2);
  }
});

// The joint must survive real rasterization: the alpha of each mark has exactly
// two opaque regions (the two fitted pieces), so neither piece breaks apart.
test("both pieces stay connected at small and large sizes", async ({ page }) => {
  await page.goto(prefix);
  const counts = await page.evaluate(async (base) => {
    async function regions(src, size) {
      const image = new Image();
      image.src = src;
      await image.decode();
      const canvas = document.createElement("canvas");
      canvas.width = canvas.height = size;
      const context = canvas.getContext("2d");
      context.drawImage(image, 0, 0, size, size);
      const { data } = context.getImageData(0, 0, size, size);
      const seen = new Uint8Array(size * size);
      let count = 0;
      for (let start = 0; start < size * size; start++) {
        if (seen[start] || data[start * 4 + 3] < 128) continue;
        count++;
        const stack = [start];
        seen[start] = 1;
        while (stack.length) {
          const index = stack.pop();
          const x = index % size;
          const y = (index - x) / size;
          for (const [dx, dy] of [
            [1, 0],
            [-1, 0],
            [0, 1],
            [0, -1],
          ]) {
            const nx = x + dx;
            const ny = y + dy;
            if (nx < 0 || ny < 0 || nx >= size || ny >= size) continue;
            const next = ny * size + nx;
            if (!seen[next] && data[next * 4 + 3] >= 128) {
              seen[next] = 1;
              stack.push(next);
            }
          }
        }
      }
      return count;
    }
    const result = {};
    for (const size of [16, 20, 24, 32])
      result[`small-${size}`] = await regions(`${base}img/brand/zfb-symbol-small.svg`, size);
    for (const size of [128, 320])
      result[`standard-${size}`] = await regions(`${base}img/brand/zfb-symbol.svg`, size);
    return result;
  }, prefix);
  for (const [label, count] of Object.entries(counts)) expect(count, label).toBe(2);
});
