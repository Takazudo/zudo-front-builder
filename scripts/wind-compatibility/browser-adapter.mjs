import { readFile } from "node:fs/promises";
import { release } from "node:os";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { sha256 } from "./reference.mjs";

const cssUrl = "https://wind-fixture.invalid/style.css";
const documentUrl = "https://wind-fixture.invalid/fixture";

function documentFor(candidate, probe) {
  const tag = probe.element ?? "span";
  const safeCandidate = candidate
    .replaceAll("&", "&amp;")
    .replaceAll('"', "&quot;")
    .replaceAll("<", "&lt;");
  const width = probe.containerWidth ?? 300;
  const targetWidth = probe.targetWidth ?? 100;
  return `<!doctype html><html><head><link rel="stylesheet" href="${cssUrl}"><style>${probe.authoredCss ?? ""}</style></head><body style="margin:0"><div id="box" style="width:${width}px;height:${width}px;writing-mode:${probe.writingMode ?? "horizontal-tb"};direction:${probe.direction ?? "ltr"};${probe.scopeVars ?? ""}"><${tag} id="target" class="${safeCandidate}" style="width:${targetWidth}px">probe</${tag}></div></body></html>`;
}

function matchesExpected(actual, expected) {
  if (typeof expected === "string") return actual.value === expected;
  if (expected === null) return true;
  if (typeof expected === "object") {
    if (expected.value !== undefined && actual.value !== expected.value) return false;
    if (expected.contains !== undefined && !actual.value.includes(expected.contains)) return false;
    if (expected.notValue !== undefined && actual.value === expected.notValue) return false;
    if (expected.min !== undefined && !(actual.number >= expected.min)) return false;
    if (expected.max !== undefined && !(actual.number <= expected.max)) return false;
    return true;
  }
  return false;
}

export async function observeIsolated(browser, css, candidate, probe, engine) {
  const context = await browser.newContext({
    viewport: { width: probe.viewportWidth ?? 800, height: 700 },
    reducedMotion: "no-preference",
    colorScheme: "light",
    hasTouch: false,
    isMobile: false,
  });
  try {
    await context.route(cssUrl, (route) =>
      route.fulfill({ status: 200, contentType: "text/css", body: css }),
    );
    await context.route(documentUrl, (route) =>
      route.fulfill({ status: 200, contentType: "text/html", body: documentFor(candidate, probe) }),
    );
    const page = await context.newPage();
    const cssResponse = page.waitForResponse((response) => response.url() === cssUrl);
    await page.goto(documentUrl, { waitUntil: "load" });
    const response = await cssResponse;
    const served = { status: response.status(), body: await response.body() };
    await page.locator("#target").evaluate((el) => document.fonts.ready);
    if (probe.hover) await page.locator("#target").hover();
    const observation = await page.locator("#target").evaluate((el, property) => {
      const style = getComputedStyle(el),
        box = el.getBoundingClientRect();
      const parent = document.querySelector("#box").getBoundingClientRect();
      const value = property.startsWith("geometry:")
        ? String(Math.round((box[property.slice(9)] - parent[property.slice(9)]) * 100) / 100)
        : style.getPropertyValue(property).trim();
      const tree = [];
      const walk = (rules, conditions) => {
        for (const rule of rules) {
          const next = [...conditions];
          if (rule.conditionText) next.push(rule.conditionText);
          if (rule.name && rule.cssRules) next.push(`@layer ${rule.name}`);
          if (rule.selectorText)
            tree.push({
              selector: rule.selectorText,
              conditions: next,
              declarations: [...rule.style].map((name) => [
                name,
                rule.style.getPropertyValue(name).trim(),
                rule.style.getPropertyPriority(name),
              ]),
            });
          if (rule.cssRules) walk(rule.cssRules, next);
        }
      };
      for (const sheet of document.styleSheets) walk(sheet.cssRules, []);
      return {
        value,
        number: Number.parseFloat(value),
        box: {
          x: box.x,
          y: box.y,
          width: box.width,
          height: box.height,
          parentX: parent.x,
          parentY: parent.y,
          parentWidth: parent.width,
          parentHeight: parent.height,
        },
        cssTree: tree,
        cssRules: [...document.styleSheets].flatMap((sheet) =>
          [...sheet.cssRules].map((rule) => rule.cssText),
        ),
      };
    }, probe.property);
    const servedBytes = served?.body;
    const verified = served?.status === 200 && servedBytes && sha256(servedBytes) === sha256(css);
    return {
      engine,
      probe: probe.name,
      verified: Boolean(verified),
      observation,
      expected: probe[engine],
      pass: Boolean(verified) && matchesExpected(observation, probe[engine]),
      stylesheetSha256: sha256(css),
      servedSha256: servedBytes ? sha256(servedBytes) : null,
    };
  } finally {
    await context.close();
  }
}

export async function observePair(browser, windCss, referenceCss, candidate, probes) {
  const results = [];
  for (const probe of probes) {
    // Separate contexts ensure no shared cascade, variables, DOM or stylesheet cache.
    results.push({
      wind: await observeIsolated(browser, windCss, candidate, probe, "wind"),
      reference: await observeIsolated(browser, referenceCss, candidate, probe, "reference"),
    });
  }
  return results;
}

export async function browserIdentity(browser, executable) {
  const require = createRequire(import.meta.url);
  const fromPlaywright = createRequire(require.resolve("@playwright/test/package.json"));
  const playwrightPackage = JSON.parse(
    await readFile(require.resolve("@playwright/test/package.json")),
  );
  const corePackagePath = fromPlaywright.resolve("playwright-core/package.json");
  const corePackage = JSON.parse(await readFile(corePackagePath));
  const manifestBytes = await readFile(resolve(dirname(corePackagePath), "browsers.json"));
  const manifest = JSON.parse(manifestBytes);
  const chromium = manifest.browsers.find((entry) => entry.name === "chromium");
  if (!chromium) throw Error("Playwright Chromium manifest entry absent");
  return {
    name: browser.browserType().name(),
    version: browser.version(),
    executable,
    executableSha256: executable ? sha256(await readFile(executable)) : null,
    platform: process.platform,
    osVersion: release(),
    architecture: process.arch,
    node: process.version,
    playwrightVersion: playwrightPackage.version,
    playwrightCoreVersion: corePackage.version,
    browserManifestSha256: sha256(manifestBytes),
    revision: chromium.revision,
    manifestBrowserVersion: chromium.browserVersion,
    headless: true,
    deviceScaleFactor: 1,
    colorScheme: "light",
    reducedMotion: "no-preference",
    isolation: "new context per engine and probe",
    styleDelivery: "same-origin intercepted 200 text/css response",
  };
}

export function unexpectedSelector(tree, candidate) {
  const escaped = candidate.replaceAll(":", "\\:");
  return tree.some(
    (rule) =>
      !rule.selector.includes(`.${escaped}`) &&
      !rule.selector.includes(":root") &&
      !rule.selector.includes(":host") &&
      !/^&(?::hover)?$/.test(rule.selector),
  );
}
