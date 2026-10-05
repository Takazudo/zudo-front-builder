#!/usr/bin/env node
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Config, Parser } from "html-validate";
import { collectBuiltAnchors } from "./audit-wind-built-anchors.mjs";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const WIND_BASELINE = join(SCRIPT_DIR, "__tests__/fixtures/wind-built-anchors.v1.json");
const ADJACENT_BASELINE = join(
  SCRIPT_DIR,
  "__tests__/fixtures/wind-adjacent-built-anchors.v1.json",
);
const SAFE_ORIGIN = "https://wind-docs.invalid";
const HTML_PARSER = new Parser(Config.defaultConfig());

function readBaseline(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function attributeValue(element, name) {
  const attribute = element.getAttribute(name);
  if (typeof attribute === "string") return attribute;
  return attribute?.value ?? null;
}

function routeFile(distDir, route) {
  if (
    typeof route !== "string" ||
    route.startsWith("/") ||
    route.split("/").some((segment) => segment === ".." || segment === "")
  )
    throw new Error(`Invalid built route: ${String(route)}`);
  const path = resolve(distDir, route);
  if (!path.startsWith(`${resolve(distDir)}/`))
    throw new Error(`Built route escaped dist: ${route}`);
  return path;
}

function collectLinks(html) {
  const root = HTML_PARSER.parseHtml(html);
  return root
    .getElementsByTagName("a")
    .map((element) => attributeValue(element, "href"))
    .filter((href) => typeof href === "string")
    .map((href) => href.trim())
    .filter(Boolean);
}

function walkHtml(directory) {
  return readdirSync(directory, { withFileTypes: true })
    .sort((left, right) => left.name.localeCompare(right.name, "en"))
    .flatMap((entry) => {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) return walkHtml(path);
      return entry.isFile() && entry.name.endsWith(".html") ? [path] : [];
    });
}

function localTarget(distDir, documentRoute, href) {
  if (/^(?:[a-z][a-z0-9+.-]*:|\/\/)/i.test(href)) return null;
  let url;
  try {
    const publicRoute = `/${documentRoute.replace(/index\.html$/, "")}`;
    url = new URL(href, new URL(publicRoute, SAFE_ORIGIN));
  } catch {
    return { error: "invalid-url" };
  }
  if (url.origin !== SAFE_ORIGIN) return null;

  let rawSegments;
  try {
    rawSegments = url.pathname
      .split("/")
      .filter(Boolean)
      .map((segment) => decodeURIComponent(segment));
  } catch {
    return { error: "invalid-path-encoding" };
  }
  const candidates = [];
  for (let start = 0; start <= rawSegments.length; start += 1) {
    const segments = rawSegments.slice(start);
    const path = resolve(distDir, ...segments);
    if (!path.startsWith(`${resolve(distDir)}/`) && path !== resolve(distDir)) continue;
    if (path.endsWith(".html") || path.endsWith(".htm")) candidates.push(path);
    else if (path.endsWith(".mdx")) {
      candidates.push(`${path.slice(0, -4)}/index.html`);
    } else {
      candidates.push(path, join(path, "index.html"), `${path}.html`);
    }
  }
  const targetPath = candidates.find((candidate) => {
    try {
      return statSync(candidate).isFile();
    } catch {
      return false;
    }
  });
  if (!targetPath) return { error: "missing-local-target", pathname: url.pathname, hash: url.hash };

  let fragment = "";
  try {
    fragment = decodeURIComponent(url.hash.slice(1));
  } catch {
    return { error: "invalid-fragment-encoding", pathname: url.pathname, hash: url.hash };
  }
  return { path: targetPath, fragment, pathname: url.pathname };
}

function checkTarget(distDir, sourceRoute, href, expectedTargets, findings) {
  const target = localTarget(distDir, sourceRoute, href);
  if (!target) return null;
  if (target.error) {
    findings.push({
      source: sourceRoute,
      href,
      type: target.error,
      target: target.pathname ?? "",
    });
    return { local: true, inScope: false };
  }
  const inScope = expectedTargets.has(target.path);
  if (!target.fragment) return { local: true, inScope };
  let html;
  try {
    html = readFileSync(target.path, "utf8");
  } catch {
    findings.push({
      source: sourceRoute,
      href,
      type: "unreadable-local-target",
      target: target.path,
    });
    return { local: true, inScope };
  }
  if (!/<html[\s>]/i.test(html) && !target.path.endsWith(".html"))
    return { local: true, inScope: false };
  let anchors;
  try {
    anchors = collectBuiltAnchors(html);
  } catch (error) {
    findings.push({
      source: sourceRoute,
      href,
      type: "invalid-target-html",
      target: target.path,
      message: error instanceof Error ? error.message : String(error),
    });
    return { local: true, inScope };
  }
  if (!anchors.ids.includes(target.fragment)) {
    findings.push({
      source: sourceRoute,
      href,
      type: "missing-fragment",
      fragment: target.fragment,
      target: relative(distDir, target.path),
    });
  }
  return { local: true, inScope };
}

function sourceRouteForFile(distDir, path) {
  return relative(distDir, path).split("\\").join("/");
}

/** Check every local link in the captured wind/adjacent pages and all inbound fragments. */
export function auditBuiltMarkdownLinks(
  distDir,
  {
    windBaseline = readBaseline(WIND_BASELINE),
    adjacentBaseline = readBaseline(ADJACENT_BASELINE),
  } = {},
) {
  const findings = [];
  const scopedRoutes = [...Object.keys(windBaseline), ...Object.keys(adjacentBaseline)];
  const scopedFiles = new Set(scopedRoutes.map((route) => routeFile(distDir, route)));
  let checkedScopedLinks = 0;
  let checkedInboundLinks = 0;

  for (const route of scopedRoutes) {
    let html;
    try {
      html = readFileSync(routeFile(distDir, route), "utf8");
    } catch {
      findings.push({ source: route, href: "", type: "missing-scoped-route" });
      continue;
    }
    const links = collectLinks(html);
    for (const href of links) {
      const checked = checkTarget(distDir, route, href, scopedFiles, findings);
      if (checked?.local) checkedScopedLinks += 1;
    }
  }

  for (const file of walkHtml(distDir)) {
    if (scopedFiles.has(file)) continue;
    const sourceRoute = sourceRouteForFile(distDir, file);
    let links;
    try {
      links = collectLinks(readFileSync(file, "utf8"));
    } catch {
      continue;
    }
    for (const href of links) {
      const target = localTarget(distDir, sourceRoute, href);
      if (!target || target.error || !scopedFiles.has(target.path)) continue;
      const before = findings.length;
      const checked = checkTarget(distDir, sourceRoute, href, scopedFiles, findings);
      if (checked?.inScope && findings.length === before) checkedInboundLinks += 1;
    }
  }

  return {
    findings,
    summary: {
      scopedRoutes: scopedRoutes.length,
      checkedScopedLinks,
      checkedInboundLinks,
    },
  };
}

function defaultDistDir() {
  const cwdDist = resolve(process.cwd(), "dist");
  if (existsSync(cwdDist)) return cwdDist;
  return resolve(SCRIPT_DIR, "../dist");
}

export function main(distDir = process.argv[2] ? resolve(process.argv[2]) : defaultDistDir()) {
  if (!existsSync(distDir)) {
    console.error(`Built docs directory is missing: ${distDir}`);
    return 1;
  }
  const { findings, summary } = auditBuiltMarkdownLinks(distDir);
  if (findings.length) {
    console.error(`Built Markdown link audit failed for ${distDir}`);
    for (const item of findings) {
      if (item.type === "missing-fragment") {
        console.error(
          `- ${item.source}: #${item.fragment} is missing from ${item.target} (href ${item.href})`,
        );
      } else {
        console.error(
          `- ${item.source}: ${item.type} for ${item.href}${item.target ? ` -> ${item.target}` : ""}`,
        );
      }
    }
    return 1;
  }
  console.log(
    `Built Markdown link audit passed: ${summary.checkedScopedLinks} local links from ` +
      `${summary.scopedRoutes} captured routes, plus ${summary.checkedInboundLinks} inbound links to those routes.`,
  );
  return 0;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = main();
}
