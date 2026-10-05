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
const SITE_ORIGIN = "https://zfb.takazudomodular.com";
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

function normalizeBase(base) {
  if (
    typeof base !== "string" ||
    !base.startsWith("/") ||
    !base.endsWith("/") ||
    base.includes("\\") ||
    /[?#]/.test(base) ||
    (base !== "/" &&
      base
        .split("/")
        .slice(1, -1)
        .some(
          (segment) =>
            !segment || segment === "." || segment === ".." || !/^[a-zA-Z0-9._~-]+$/.test(segment),
        ))
  )
    throw new Error(`Invalid docs URL base: ${String(base)}`);
  return base;
}

function mappedFile(distDir, pathname) {
  let rawSegments;
  try {
    rawSegments = pathname
      .split("/")
      .filter(Boolean)
      .map((segment) => decodeURIComponent(segment));
  } catch {
    return { error: "invalid-path-encoding" };
  }
  if (rawSegments.some((segment) => /[\\/]/.test(segment) || segment === "." || segment === ".."))
    return { error: "invalid-local-path" };
  const path = resolve(distDir, ...rawSegments);
  if (!path.startsWith(`${resolve(distDir)}/`) && path !== resolve(distDir))
    return { error: "invalid-local-path" };
  const candidates = [path, join(path, "index.html")];
  const targetPath = candidates.find((candidate) => {
    try {
      return statSync(candidate).isFile();
    } catch {
      return false;
    }
  });
  return targetPath ? { path: targetPath } : { error: "missing-local-target" };
}

function localTarget(distDir, documentRoute, href, base) {
  let url;
  try {
    const publicRoute = documentRoute.replace(/index\.html$/, "");
    const documentUrl = new URL(`${base}${publicRoute}`, SAFE_ORIGIN);
    url = new URL(href, documentUrl);
  } catch {
    return { error: "invalid-url" };
  }
  // Canonical site links are local. Other absolute/protocol-relative URLs are
  // intentionally outside this static-bundle audit.
  if (url.origin !== SAFE_ORIGIN && url.origin !== SITE_ORIGIN) return null;

  if (!url.pathname.startsWith(base)) {
    // Preserve inbound coverage for an unprefixed link only when its exact
    // root-relative path maps to a built page. Never strip arbitrary segments.
    const unprefixed = mappedFile(distDir, url.pathname);
    return {
      error: "wrong-base-prefix",
      pathname: url.pathname,
      hash: url.hash,
      likelyTargetPath: unprefixed.path,
    };
  }

  const mapped = mappedFile(distDir, url.pathname.slice(base.length));
  if (mapped.error) return { error: mapped.error, pathname: url.pathname, hash: url.hash };

  let fragment = "";
  try {
    fragment = decodeURIComponent(url.hash.slice(1));
  } catch {
    return { error: "invalid-fragment-encoding", pathname: url.pathname, hash: url.hash };
  }
  return { path: mapped.path, fragment, pathname: url.pathname };
}

function checkTarget(distDir, sourceRoute, href, expectedTargets, base, findings, anchorCache) {
  const target = localTarget(distDir, sourceRoute, href, base);
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
  let parsed = anchorCache.get(target.path);
  if (!parsed) {
    let html;
    try {
      html = readFileSync(target.path, "utf8");
    } catch (error) {
      parsed = {
        type: "unreadable-local-target",
        error: error instanceof Error ? error.message : String(error),
      };
    }
    if (!parsed) {
      if (!/<html[\s>]/i.test(html) && !target.path.endsWith(".html")) parsed = { skip: true };
      else {
        try {
          parsed = { anchors: collectBuiltAnchors(html) };
        } catch (error) {
          parsed = {
            type: "invalid-target-html",
            error: error instanceof Error ? error.message : String(error),
          };
        }
      }
    }
    anchorCache.set(target.path, parsed);
  }
  if (parsed.skip) return { local: true, inScope: false };
  if (parsed.error) {
    findings.push({
      source: sourceRoute,
      href,
      type: parsed.type,
      target: target.path,
      message: parsed.error,
    });
    return { local: true, inScope };
  }
  if (!parsed.anchors.ids.includes(target.fragment)) {
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
    base = "/",
  } = {},
) {
  base = normalizeBase(base);
  const findings = [];
  const anchorCache = new Map();
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
      const checked = checkTarget(distDir, route, href, scopedFiles, base, findings, anchorCache);
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
      const target = localTarget(distDir, sourceRoute, href, base);
      if (!target) continue;
      if (target.error) {
        if (target.likelyTargetPath && scopedFiles.has(target.likelyTargetPath))
          checkTarget(distDir, sourceRoute, href, scopedFiles, base, findings, anchorCache);
        continue;
      }
      if (!scopedFiles.has(target.path)) continue;
      const before = findings.length;
      const checked = checkTarget(
        distDir,
        sourceRoute,
        href,
        scopedFiles,
        base,
        findings,
        anchorCache,
      );
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

export function main(
  distDir = process.argv[2] ? resolve(process.argv[2]) : defaultDistDir(),
  base = process.argv.includes("--base") ? process.argv[process.argv.indexOf("--base") + 1] : "/",
) {
  if (!existsSync(distDir)) {
    console.error(`Built docs directory is missing: ${distDir}`);
    return 1;
  }
  let result;
  try {
    result = auditBuiltMarkdownLinks(distDir, { base });
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    return 1;
  }
  const { findings, summary } = result;
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
