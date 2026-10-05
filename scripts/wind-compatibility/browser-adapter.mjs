import { readFile } from "node:fs/promises";
import { release } from "node:os";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { sha256 } from "./reference.mjs";

const cssUrl = "https://wind-fixture.invalid/style.css";
const documentUrl = "https://wind-fixture.invalid/fixture";

function documentFor(candidate, probe) {
  if (
    typeof probe.documentHtml !== "string" ||
    !probe.documentHtml.includes("</head>") ||
    !probe.documentHtml.includes('id="target"') ||
    !probe.documentHtml.includes('id="box"')
  )
    throw Error(`Missing explicit probe DOM for ${probe.name}`);
  const width = probe.containerWidth ?? 300;
  const setup = `#box{width:${width}px;height:${width}px;writing-mode:${probe.writingMode ?? "horizontal-tb"};direction:${probe.direction ?? "ltr"};${probe.scopeVars ?? ""}}`;
  return probe.documentHtml.replace(
    "</head>",
    `<link rel="stylesheet" href="${cssUrl}"><style>${setup}${probe.authoredCss ?? ""}</style></head>`,
  );
}

export function matchesExpected(actual, expected) {
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
  const html = documentFor(candidate, probe);
  const selector = probe.selector ?? "#target";
  const settings = {
    viewportWidth: probe.viewportWidth ?? 800,
    viewportHeight: 700,
    hover: Boolean(probe.hover),
    hoverSelector: probe.hoverSelector ?? "#target",
    keyboardFocus: Boolean(probe.keyboardFocus),
    focusSelector: probe.focusSelector ?? null,
    targetAttributes: probe.targetAttributes ?? {},
    ancestorAttributes: probe.ancestorAttributes ?? {},
    pseudo: probe.pseudo ?? null,
    writingMode: probe.writingMode ?? "horizontal-tb",
    direction: probe.direction ?? "ltr",
    selector,
    relativeTo: probe.relativeTo ?? "#box",
    documentSha256: sha256(probe.documentHtml),
    servedDocumentSha256: sha256(html),
    authoredCssSha256: sha256(probe.authoredCss ?? ""),
    scopeVars: probe.scopeVars ?? "",
    hasTouch: probe.hoverCapability === "none",
    isMobile: probe.hoverCapability === "none",
    deviceScaleFactor: 1,
    colorScheme: "light",
    reducedMotion: "no-preference",
    hoverCapability: probe.hoverCapability ?? "desktop-hover",
  };
  const context = await browser.newContext({
    viewport: { width: settings.viewportWidth, height: settings.viewportHeight },
    reducedMotion: "no-preference",
    colorScheme: "light",
    hasTouch: settings.hasTouch,
    isMobile: settings.isMobile,
  });
  try {
    await context.route(cssUrl, (route) =>
      route.fulfill({ status: 200, contentType: "text/css; charset=utf-8", body: css }),
    );
    await context.route(documentUrl, (route) =>
      route.fulfill({ status: 200, contentType: "text/html; charset=utf-8", body: html }),
    );
    const page = await context.newPage();
    const cssResponse = page.waitForResponse((response) => response.url() === cssUrl);
    await page.goto(documentUrl, { waitUntil: "load" });
    for (const [name, value] of Object.entries(probe.targetAttributes ?? {}))
      await page
        .locator("#target")
        .evaluate((el, [key, val]) => el.setAttribute(key, val), [name, value]);
    for (const [name, value] of Object.entries(probe.ancestorAttributes ?? {}))
      await page
        .locator("#ancestor")
        .evaluate((el, [key, val]) => el.setAttribute(key, val), [name, value]);
    const response = await cssResponse;
    const served = { status: response.status(), body: await response.body() };
    const targetClasses = await page.locator("#target").evaluate((el) => [...el.classList]);
    if (!candidate.split(/\s+/).every((name) => targetClasses.includes(name)))
      throw Error(`Probe DOM candidate mismatch: ${probe.name}`);
    await page.locator(selector).evaluate(() => document.fonts.ready);
    if (probe.hover) await page.locator(probe.hoverSelector ?? "#target").hover();
    if (probe.keyboardFocus) {
      await page.keyboard.press("Tab");
      if (
        probe.focusSelector &&
        !(await page.locator(probe.focusSelector).evaluate((el) => el === document.activeElement))
      )
        throw Error(`Keyboard focus missed ${probe.focusSelector}`);
    } else if (probe.focusSelector) await page.locator(probe.focusSelector).focus();
    const observation = await page.locator(selector).evaluate(
      (el, input) => {
        const { property, relativeTo, pseudo } = input;
        const style = getComputedStyle(el, pseudo ?? null),
          box = el.getBoundingClientRect();
        const parent = document.querySelector(relativeTo).getBoundingClientRect();
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
      },
      { property: probe.property, relativeTo: settings.relativeTo, pseudo: probe.pseudo },
    );
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
      settings,
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
  const browserName = browser.browserType().name();
  const browserEntry = manifest.browsers.find((entry) => entry.name === browserName);
  if (!browserEntry) throw Error(`Playwright ${browserName} manifest entry absent`);
  let releaseText = "";
  if (process.platform === "linux") {
    try {
      releaseText = await readFile("/etc/os-release", "utf8");
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
  }
  const host = hostPlatformIdentity(process.platform, process.arch, releaseText);
  return {
    name: browser.browserType().name(),
    version: browser.version(),
    executable,
    executableSha256: executable ? sha256(await readFile(executable)) : null,
    launchExecutableKind: `${browserName}-full-explicit`,
    platform: process.platform,
    hostPlatform: host.hostPlatform,
    linuxDistribution: host.linuxDistribution,
    osVersion: release(),
    architecture: process.arch,
    node: process.version,
    playwrightVersion: playwrightPackage.version,
    playwrightCoreVersion: corePackage.version,
    browserManifestSha256: sha256(manifestBytes),
    revision: browserEntry.revision,
    manifestBrowserVersion: browserEntry.browserVersion,
    headless: true,
    deviceScaleFactor: 1,
    colorScheme: "light",
    reducedMotion: "no-preference",
    isolation: "new context per engine and probe",
    styleDelivery: "same-origin intercepted 200 text/css response",
  };
}

export function hostPlatformIdentity(platform, architecture, releaseText = "") {
  if (platform !== "linux") return { hostPlatform: null, linuxDistribution: null };
  const fields = Object.fromEntries(
    releaseText.split(/\r?\n/).flatMap((line) => {
      const match = /^([A-Z_]+)=(.*)$/.exec(line);
      if (!match) return [];
      let value = match[2];
      if (
        (value.startsWith('"') && value.endsWith('"')) ||
        (value.startsWith("'") && value.endsWith("'"))
      )
        value = value.slice(1, -1);
      return [[match[1], value]];
    }),
  );
  const id = fields.ID?.toLowerCase();
  const versionId = fields.VERSION_ID;
  const valid = /^[a-z0-9.-]+$/;
  return {
    hostPlatform:
      id && versionId && valid.test(id) && valid.test(versionId) && valid.test(architecture)
        ? `${id}${versionId}-${architecture}`
        : null,
    linuxDistribution: id && versionId ? { id, versionId } : null,
  };
}

export function requiredMatrixMember(profile, environment) {
  const policy = profile.browserPolicy;
  if (
    environment.playwrightVersion !== policy.playwrightTestVersion ||
    environment.playwrightCoreVersion !== policy.playwrightCoreVersion ||
    environment.browserManifestSha256 !== policy.browserManifest.sha256 ||
    !environment.hostPlatform ||
    !environment.linuxDistribution ||
    environment.hostPlatform !==
      `${environment.linuxDistribution.id}${environment.linuxDistribution.versionId}-${environment.architecture}`
  )
    return false;
  return policy.requiredMatrix.some(
    (member) =>
      member.os === environment.platform &&
      member.hostPlatform === environment.hostPlatform &&
      member.browser === environment.name &&
      member.revision === environment.revision &&
      member.browserVersion === environment.version &&
      member.browserVersion === environment.manifestBrowserVersion,
  );
}
