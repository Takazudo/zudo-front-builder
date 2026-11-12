import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";
export function defaultLocaleOnlyPrefixes(configSource) {
  const declarations = [...configSource.matchAll(/\bdefaultLocaleOnlyPrefixes\s*:/g)];
  const declaration = declarations[0];
  if (declarations.length !== 1)
    throw new Error("Expected one literal defaultLocaleOnlyPrefixes policy in docs/zfb.config.ts");
  const rest = configSource.slice(declaration.index + declaration[0].length);
  const literal = rest.match(/^\s*(\[[\s\S]*?\])/);
  if (!literal) throw new Error("Expected a literal defaultLocaleOnlyPrefixes array");
  const prefixes = JSON.parse(literal[1].replace(/,\s*\]$/, "]"));
  if (!prefixes.every((prefix) => typeof prefix === "string" && prefix.startsWith("/")))
    throw new Error("Expected explicit default-locale-only paths");
  return prefixes.map((prefix) => prefix.replace(/\/+$/, "") + "/");
}

export function docRoute(file) {
  const slug = file
    .replaceAll("\\", "/")
    .replace(/\.mdx?$/, "")
    .replace(/(^|\/)index$/, "");
  return `/docs/${slug}`.replace(/\/+$/, "") + "/";
}

export function discoverDocPages(directory) {
  const pages = new Map();
  function walk(current) {
    for (const entry of readdirSync(current, { withFileTypes: true }).sort((left, right) =>
      left.name.localeCompare(right.name, "en"),
    )) {
      const path = join(current, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (entry.isFile() && /\.mdx?$/.test(entry.name)) {
        const route = docRoute(relative(directory, path));
        if (pages.has(route)) throw new Error(`Duplicate documentation route: ${route}`);
        pages.set(route, path);
      }
    }
  }
  walk(directory);
  return pages;
}

export function hasDocBody(source) {
  return (
    source
      .replace(/^\uFEFF/, "")
      .replace(/^---\r?\n[\s\S]*?\r?\n---(?:\r?\n|$)/, "")
      .replace(/<!--[\s\S]*?-->/g, "")
      .replace(/\{\/\*[\s\S]*?\*\/\}/g, "")
      .trim().length > 0
  );
}

export function missingTranslations({ englishDir, japaneseDir, excludedPrefixes }) {
  const english = discoverDocPages(englishDir);
  const japanese = discoverDocPages(japaneseDir);
  const findings = [];
  for (const route of english.keys()) {
    if (excludedPrefixes.some((prefix) => route.startsWith(prefix))) continue;
    const path = japanese.get(route);
    if (!path) findings.push({ route, reason: "missing" });
    else if (!hasDocBody(readFileSync(path, "utf8"))) findings.push({ route, reason: "empty" });
  }
  return findings;
}
