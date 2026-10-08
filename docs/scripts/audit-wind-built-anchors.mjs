#!/usr/bin/env node

import { migratedBuiltRoute } from "../src/config/route-migrations.mjs";
import { Config, Parser } from "html-validate";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const WIND_BASELINE = join(SCRIPT_DIR, "__tests__/fixtures/wind-built-anchors.v1.json");
const ADJACENT_BASELINE = join(
  SCRIPT_DIR,
  "__tests__/fixtures/wind-adjacent-built-anchors.v1.json",
);
const HEADING_TAGS = ["h1", "h2", "h3", "h4", "h5", "h6"];

function readBaseline(filepath) {
  return JSON.parse(readFileSync(filepath, "utf8"));
}

function attributeValue(element, name) {
  const attribute = element.getAttribute(name);
  if (typeof attribute === "string") return attribute;
  return attribute?.value ?? null;
}

/** Read ids from actual emitted HTML, including the ids on heading elements. */
export function collectBuiltAnchors(html) {
  const root = new Parser(Config.defaultConfig()).parseHtml(html);
  const ids = root
    .querySelectorAll("[id]")
    .map((element) => attributeValue(element, "id"))
    .filter((value) => value !== null);
  const headingIds = HEADING_TAGS.flatMap((tag) =>
    root
      .getElementsByTagName(tag)
      .map((element) => attributeValue(element, "id"))
      .filter((value) => value !== null),
  );

  return { ids, headingIds };
}

function duplicateValues(values) {
  const seen = new Set();
  const duplicates = new Set();
  for (const value of values) {
    if (seen.has(value)) duplicates.add(value);
    seen.add(value);
  }
  return [...duplicates].sort();
}

function safeRouteFile(distDir, route) {
  if (
    typeof route !== "string" ||
    route.startsWith("/") ||
    route.split("/").some((segment) => segment === ".." || segment === "")
  ) {
    throw new Error(`Invalid baseline route path: ${String(route)}`);
  }
  const filepath = resolve(distDir, route);
  const prefix = `${resolve(distDir)}/`;
  if (!filepath.startsWith(prefix)) throw new Error(`Baseline route escaped dist: ${route}`);
  return filepath;
}

function auditInventory(distDir, baseline, kind, findings) {
  let routeCount = 0;
  let idCount = 0;
  let headingCount = 0;

  for (const [route, expected] of Object.entries(baseline)) {
    routeCount += 1;
    idCount += expected.ids.length;
    headingCount += (expected.headings ?? []).length;
    const filepath = safeRouteFile(distDir, migratedBuiltRoute(route));
    let html;
    try {
      html = readFileSync(filepath, "utf8");
    } catch {
      findings.push({ kind, route, type: "missing-route" });
      continue;
    }

    let actual;
    try {
      actual = collectBuiltAnchors(html);
    } catch (error) {
      findings.push({
        kind,
        route,
        type: "invalid-html",
        message: error instanceof Error ? error.message : String(error),
      });
      continue;
    }

    const actualIds = new Set(actual.ids);
    const actualHeadingIds = new Set(actual.headingIds);
    const expectedHeadings = expected.headings ?? [];

    const missingIds = expected.ids.filter((id) => !actualIds.has(id));
    if (missingIds.length > 0) {
      findings.push({ kind, route, type: "missing-ids", ids: missingIds });
    }

    const missingHeadingIds = expectedHeadings.filter((id) => !actualHeadingIds.has(id));
    if (missingHeadingIds.length > 0) {
      findings.push({ kind, route, type: "missing-heading-ids", ids: missingHeadingIds });
    }

    const duplicateIds = duplicateValues(actual.ids);
    if (duplicateIds.length > 0) {
      findings.push({ kind, route, type: "duplicate-ids", ids: duplicateIds });
    }
  }

  return { routeCount, idCount, headingCount };
}

/** Compare built route files with the captured parent build inventories. */
export function auditBuiltAnchors(
  distDir,
  {
    windBaseline = readBaseline(WIND_BASELINE),
    adjacentBaseline = readBaseline(ADJACENT_BASELINE),
  } = {},
) {
  const findings = [];
  const wind = auditInventory(distDir, windBaseline, "wind", findings);
  const adjacent = auditInventory(distDir, adjacentBaseline, "adjacent", findings);
  return {
    findings,
    summary: {
      windRoutes: wind.routeCount,
      windIds: wind.idCount,
      windHeadingIds: wind.headingCount,
      adjacentRoutes: adjacent.routeCount,
      adjacentIds: adjacent.idCount,
    },
  };
}

function defaultDistDir() {
  const cwdDist = resolve(process.cwd(), "dist");
  if (existsSync(cwdDist)) return cwdDist;
  const repoRoot = resolve(SCRIPT_DIR, "../..");
  return resolve(repoRoot, "docs/dist");
}

export function main(distDir = process.argv[2] ? resolve(process.argv[2]) : defaultDistDir()) {
  if (!existsSync(distDir)) {
    console.error(`Built docs directory is missing: ${distDir}`);
    return 1;
  }

  const { findings, summary } = auditBuiltAnchors(distDir);
  if (findings.length > 0) {
    console.error(`Built route/anchor audit failed for ${distDir}`);
    for (const finding of findings) {
      if (finding.type === "missing-route") {
        console.error(`- [${finding.kind}] missing built route: ${finding.route}`);
      } else if (finding.type === "invalid-html") {
        console.error(`- [${finding.kind}] could not parse ${finding.route}: ${finding.message}`);
      } else {
        console.error(
          `- [${finding.kind}] ${finding.route} ${finding.type}: ${finding.ids.join(", ")}`,
        );
      }
    }
    return 1;
  }

  console.log(
    `Built route/anchor audit passed: ${summary.windRoutes} wind routes, ` +
      `${summary.windHeadingIds} retained heading ids across ${summary.windIds} ids, ` +
      `${summary.adjacentRoutes} adjacent routes and ${summary.adjacentIds} ids.`,
  );
  return 0;
}

const scriptPath = fileURLToPath(import.meta.url);
if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  process.exitCode = main();
}
