#!/usr/bin/env node

import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Keep the route/marker contract in one place. Adding another playground is a
// single-line change here; both the default locale and `ja` are expanded below.
export const EXPECTED_ISLANDS = [
  { route: "docs/playground/render", marker: "RenderPlayground" },
  { route: "docs/playground/compile", marker: "CompilePlayground" },
  { route: "docs/playground/parse", marker: "ParsePlayground" },
  { route: "docs/playground/highlight", marker: "HighlightPlayground" },
];

export const EXPECTED_LOCALES = ["", "ja/"];

const ELEMENT_TAG = /<[A-Za-z][^>]*>/g;
const ISLAND_ATTRIBUTE =
  /\sdata-zfb-island(?:-skip-ssr)?\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))/gi;
const ISLAND_ASSET = /^islands(?:-[^/]+)?\.js$/;

/**
 * Return the non-empty island marker values in an emitted HTML document.
 *
 * zfb's production HTML is minified, so values can be either quoted or
 * unquoted. This deliberately inspects element attributes rather than doing a
 * plain `html.includes()` check: a code sample containing the marker spelling
 * must not make the marker half green.
 */
export function collectIslandMarkers(html) {
  const markers = new Set();
  for (const tag of html.matchAll(ELEMENT_TAG)) {
    for (const match of tag[0].matchAll(ISLAND_ATTRIBUTE)) {
      const marker = match[1] ?? match[2] ?? match[3] ?? "";
      if (marker !== "") markers.add(marker);
    }
  }
  return markers;
}

/**
 * Find the generated islands JavaScript assets under `dist/assets/`.
 *
 * The production entry is `islands-<hash>.js`; dev fixtures and older builds
 * may use `islands.js`. Code-split `islands-chunk-*.js` files are included as
 * well because a future bundler may place the registration table in a chunk.
 */
export function findIslandAssets(distDir) {
  const assetsDir = join(distDir, "assets");
  if (!existsSync(assetsDir)) return [];
  return readdirSync(assetsDir)
    .filter((name) => ISLAND_ASSET.test(name))
    .sort()
    .map((name) => join(assetsDir, name));
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * Check that `marker` is used as a generated registry key, rather than merely
 * appearing somewhere in component source or a source-map-like string.
 *
 * The first expression is the stable generated shape before minification:
 * `__zfb_register(namespace, exportName, markerName, moduleLabel)`. The same
 * shape is emitted as `R(ns, "default", "Marker", "...")` in production.
 * The object/bracket forms cover runtime/fixture manifests that are not passed
 * through the call-based shared-bundle generator.
 */
export function hasIslandManifestEntry(bundleText, marker) {
  const escapedMarker = escapeRegExp(marker);
  const literal = `"${escapedMarker}"`;
  const registrationCall = new RegExp(
    `\\b[A-Za-z_$][\\w$]*\\(\\s*[^,()]+\\s*,\\s*["'][^"']+["']\\s*,\\s*${literal}\\s*,`,
  );
  if (registrationCall.test(bundleText)) return true;

  const objectKey = new RegExp(`(?:["'])${escapedMarker}(?:["'])\\s*:`);
  if (objectKey.test(bundleText)) return true;

  const bracketKey = new RegExp(`\\[\\s*(?:["'])${escapedMarker}(?:["'])\\s*\\]\\s*[:=]`);
  return bracketKey.test(bundleText);
}

function routeLabel(route) {
  return `/${route.replace(/^\/+/, "")}`;
}

function expectedRoutes() {
  return EXPECTED_LOCALES.flatMap((localePrefix) =>
    EXPECTED_ISLANDS.map(({ route, marker }) => ({
      route: `${localePrefix}${route}`,
      marker,
    })),
  );
}

function finding(half, route, marker, message) {
  return { half, route: routeLabel(route), marker, message };
}

/**
 * Validate the built docs tree. Findings are returned to make the guard easy
 * to exercise against focused synthetic fixtures; the CLI below turns any
 * finding into a non-zero process exit.
 */
export function checkIslandMarkers(distDir) {
  const findings = [];
  const assets = findIslandAssets(distDir);
  const bundleText = assets.map((asset) => readFileSync(asset, "utf8")).join("\n");

  for (const { route, marker: expectedMarker } of expectedRoutes()) {
    const htmlPath = join(distDir, route, "index.html");
    let html;
    try {
      html = readFileSync(htmlPath, "utf8");
    } catch {
      findings.push(
        finding(
          "marker",
          route,
          expectedMarker,
          `marker half failed: emitted HTML is missing (${htmlPath})`,
        ),
      );
      if (assets.length === 0 || !hasIslandManifestEntry(bundleText, expectedMarker)) {
        findings.push(
          finding(
            "manifest",
            route,
            expectedMarker,
            assets.length === 0
              ? "manifest half failed: no islands JavaScript asset exists under dist/assets/"
              : "manifest half failed: marker is not registered in the emitted islands bundle",
          ),
        );
      }
      continue;
    }

    const markers = collectIslandMarkers(html);
    if (!markers.has(expectedMarker)) {
      findings.push(
        finding(
          "marker",
          route,
          expectedMarker,
          `marker half failed: index.html has no data-zfb-island="${expectedMarker}" ` +
            `or data-zfb-island-skip-ssr="${expectedMarker}" attribute`,
        ),
      );
    }

    // Check the declared marker even when its HTML half is missing (a stale
    // registry entry must not hide the missing page marker). Also check every
    // marker actually emitted on these pages: this catches a renamed
    // displayName whose new SSR marker is not registered under the old key.
    const markersToCheck = new Set([expectedMarker, ...markers]);
    for (const marker of markersToCheck) {
      if (assets.length === 0 || !hasIslandManifestEntry(bundleText, marker)) {
        findings.push(
          finding(
            "manifest",
            route,
            marker,
            assets.length === 0
              ? "manifest half failed: no islands JavaScript asset exists under dist/assets/"
              : "manifest half failed: marker is not registered in the emitted islands bundle",
          ),
        );
      }
    }
  }

  return { assets, findings };
}

function defaultDistDir() {
  const cwdDist = resolve(process.cwd(), "dist");
  if (existsSync(cwdDist)) return cwdDist;
  const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
  return resolve(repoRoot, "docs", "dist");
}

export function main(distDir = process.argv[2] ? resolve(process.argv[2]) : defaultDistDir()) {
  const { assets, findings } = checkIslandMarkers(distDir);
  if (findings.length > 0) {
    console.error(`Island marker guard failed for ${distDir}`);
    if (assets.length > 0) {
      console.error(
        `Checked islands assets: ${assets.map((asset) => asset.split("/").pop()).join(", ")}`,
      );
    }
    for (const item of findings) {
      console.error(`- [${item.half}] ${item.route} marker "${item.marker}": ${item.message}`);
    }
    return 1;
  }

  console.log(
    `Island marker guard passed: ${expectedRoutes().length} route/marker pairs; ` +
      `${assets.length} islands asset(s) checked`,
  );
  return 0;
}

const scriptPath = fileURLToPath(import.meta.url);
if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  process.exitCode = main();
}
