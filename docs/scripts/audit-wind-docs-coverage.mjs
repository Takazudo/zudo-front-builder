#!/usr/bin/env node
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { parseToAst } from "@takazudo/zfb-md-wasm/parse";
import en from "./wind-reference-strings/en.mjs";
import ja from "./wind-reference-strings/ja.mjs";
import { WIND_GUIDE_PAGES } from "./generate-wind-guide-previews.mjs";
import { validateCatalogFamilies, validateLocaleStrings } from "./generate-wind-reference.mjs";
import { WIND_REFERENCE_FAMILIES } from "./wind-reference-families.mjs";
import { exampleSource, loadRecords, REPO_ROOT } from "./wind-preview-assets.mjs";
import {
  loadEditorial,
  loadPreviewContext,
  validateEditorial,
} from "./wind-reference-editorial.mjs";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const EXPECTED = Object.freeze({
  adjacentRoutes: 42,
  adjacentSources: 43,
  catalogEntries: 197,
  diagnosticExamples: 5,
  editorialFamilies: 47,
  guidePages: 10,
  guideRecords: 9,
  historicalWindRoutes: 116,
  negativeExamples: 30,
  positiveExamples: 138,
  records: 57,
  utilityFamilies: 47,
  utilityPositiveExamples: 122,
  windExamples: 168,
});

const fail = (message) => {
  throw new Error(message);
};

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function requireCount(values, expected, label) {
  const count = values.length ?? values.size ?? Object.keys(values).length;
  if (count !== expected) fail(`${label}: expected ${expected}, found ${count}`);
}

function assertMdxInventory(directory, expectedNames, label) {
  const actual = readdirSync(directory, { withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith(".mdx"))
    .map((entry) => entry.name)
    .sort();
  const expected = [...expectedNames].sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected))
    fail(`${label}: expected [${expected.join(", ")}], found [${actual.join(", ")}]`);
}

function sourceForBuiltRoute(root, route) {
  const japanese = route.startsWith("ja/");
  const unprefixed = route.replace(/^ja\//, "");
  if (!unprefixed.startsWith("docs/")) fail(`Unexpected docs route in coverage fixture: ${route}`);
  const page = unprefixed.slice("docs/".length).replace(/\/index\.html$/, "");
  const contentRoot = join(root, "docs/src/content", japanese ? "docs-ja" : "docs");
  const candidates = [join(contentRoot, `${page}.mdx`), join(contentRoot, page, "index.mdx")];
  const existing = candidates.filter((path) => {
    try {
      readFileSync(path);
      return true;
    } catch {
      return false;
    }
  });
  if (existing.length !== 1) {
    fail(
      `${route}: expected one matching source page, found ${existing.length} ` +
        `(${candidates.map((path) => relative(root, path)).join(", ")})`,
    );
  }
  return existing[0];
}

function collectGuideMarkers(source) {
  return [
    ...source.matchAll(
      /\{\/\* wind-preview: ([a-z0-9-]+)\/([a-z0-9-]+) ("(?:[^"\\]|\\.)*") \*\/\}/g,
    ),
  ].map(([, family, id]) => ({ family, id }));
}

async function parseSource(path, root) {
  const source = readFileSync(path, "utf8");
  const filename = relative(root, path).replaceAll("\\", "/");
  const dialect = path.endsWith(".md") ? "markdown" : "mdx";
  const result = await parseToAst(source, { filename, dialect });
  const errors = (result.diagnostics ?? []).filter((item) => item.severity === "error");
  if (!result.ast || errors.length) {
    const details = errors
      .map((item) => `${item.line ?? "?"}:${item.column ?? "?"} ${item.message}`)
      .join("\n");
    fail(`${filename}: Markdown parser rejected the source${details ? `\n${details}` : ""}`);
  }
  return result;
}

function collectMarkdownLinks(ast, found = []) {
  if (!ast || typeof ast !== "object") return found;
  if ((ast.type === "link" || ast.type === "image") && typeof ast.url === "string")
    found.push(ast.url);
  for (const [key, value] of Object.entries(ast)) {
    if (key === "position") continue;
    if (Array.isArray(value)) value.forEach((child) => collectMarkdownLinks(child, found));
    else collectMarkdownLinks(value, found);
  }
  return found;
}

async function auditReadmeLinks(path, root) {
  const parsed = await parseSource(path, root);
  for (const href of collectMarkdownLinks(parsed.ast)) {
    if (/^(?:[a-z][a-z0-9+.-]*:|\/\/)/i.test(href)) continue;
    let target;
    try {
      target = fileURLToPath(new URL(href, pathToFileURL(path)));
    } catch {
      fail(`${relative(root, path)}: invalid local Markdown link ${href}`);
    }
    if (!existsSync(target))
      fail(`${relative(root, path)}: local Markdown target is missing: ${href}`);
  }
}

/** Enforce the settled content, record, source, and captured-route inventories. */
export async function auditWindDocsCoverage(root = REPO_ROOT) {
  const catalog = readJson(join(root, "crates/zudo-wind/catalog/zudo-wind-catalog.v1.json"));
  const records = loadRecords(root);
  const editorial = loadEditorial(join(root, "docs/scripts/wind-reference-editorial"));
  const preview = loadPreviewContext(root, records);
  const entriesById = validateCatalogFamilies(catalog, WIND_REFERENCE_FAMILIES);
  validateLocaleStrings(en, WIND_REFERENCE_FAMILIES);
  validateLocaleStrings(ja, WIND_REFERENCE_FAMILIES);

  requireCount(WIND_REFERENCE_FAMILIES, EXPECTED.utilityFamilies, "Utility family inventory");
  const familyEntries = WIND_REFERENCE_FAMILIES.flatMap((family) => family.entries);
  requireCount(familyEntries, EXPECTED.catalogEntries, "Utility entry membership");
  if (new Set(familyEntries).size !== EXPECTED.catalogEntries)
    fail("Utility entries must belong to exactly one family");
  requireCount(catalog.entries, EXPECTED.catalogEntries, "Catalog entry inventory");
  requireCount(editorial, EXPECTED.editorialFamilies, "Bilingual editorial inventory");

  for (const family of WIND_REFERENCE_FAMILIES) {
    const record = editorial.get(family.id);
    if (!record) fail(`Missing editorial family ${family.id}`);
    validateEditorial(record, family, preview.examples.get(family.id), entriesById, preview.assets);
  }

  const utilityIds = new Set(WIND_REFERENCE_FAMILIES.map((family) => family.id));
  const guideNames = WIND_GUIDE_PAGES.filter((name) => name !== "index");
  requireCount(WIND_GUIDE_PAGES, EXPECTED.guidePages, "Guide page inventory");
  requireCount(guideNames, EXPECTED.guideRecords, "Guide example record inventory");
  const guideIds = new Set(guideNames.map((name) => `guide-${name}`));
  const recordFamilies = new Set(records.map((record) => record.family));
  const expectedRecordFamilies = new Set([...utilityIds, ...guideIds, "diagnostics"]);
  if (recordFamilies.size !== records.length) fail("Example record family names must be unique");
  if (
    recordFamilies.size !== expectedRecordFamilies.size ||
    [...expectedRecordFamilies].some((family) => !recordFamilies.has(family))
  )
    fail("Example records must be the 47 utility families, 9 guide records, and diagnostics");
  requireCount(records, EXPECTED.records, "Example record inventory");

  const exampleByKey = new Map();
  const counts = { positive: 0, negative: 0, utilityPositive: 0, diagnostic: 0 };
  for (const record of records) {
    for (const example of record.examples) {
      const key = `${record.family}/${example.id}`;
      if (exampleByKey.has(key)) fail(`Duplicate example record ${key}`);
      exampleByKey.set(key, { ...example, family: record.family });
      if (example.kind === "positive") {
        counts.positive += 1;
        if (utilityIds.has(record.family)) counts.utilityPositive += 1;
      } else {
        counts.negative += 1;
      }
      if (record.family === "diagnostics") counts.diagnostic += 1;
    }
  }
  requireCount(exampleByKey, EXPECTED.windExamples, "Example inventory");
  if (counts.positive !== EXPECTED.positiveExamples)
    fail(
      `Runnable example inventory: expected ${EXPECTED.positiveExamples}, found ${counts.positive}`,
    );
  if (counts.negative !== EXPECTED.negativeExamples)
    fail(
      `Expected-diagnostic inventory: expected ${EXPECTED.negativeExamples}, found ${counts.negative}`,
    );
  if (counts.utilityPositive !== EXPECTED.utilityPositiveExamples)
    fail(
      `Utility runnable examples: expected ${EXPECTED.utilityPositiveExamples}, found ${counts.utilityPositive}`,
    );
  if (counts.diagnostic !== EXPECTED.diagnosticExamples)
    fail(
      `Pipeline-only diagnostics: expected ${EXPECTED.diagnosticExamples}, found ${counts.diagnostic}`,
    );

  const manifest = readJson(join(root, "docs/public/wind-examples/manifest.json"));
  requireCount(manifest.examples, EXPECTED.windExamples, "Preview manifest inventory");
  const manifestKeys = new Set();
  for (const entry of manifest.examples) {
    const key = `${entry.family}/${entry.id}`;
    if (manifestKeys.has(key)) fail(`Duplicate preview manifest entry ${key}`);
    manifestKeys.add(key);
    const example = exampleByKey.get(key);
    if (!example || entry.kind !== example.kind) fail(`Unexpected preview manifest entry ${key}`);
    if (entry.kind === "positive" && (!entry.cssPath || !entry.htmlPath))
      fail(`Positive preview manifest entry lacks HTML/CSS assets: ${key}`);
    if (entry.kind !== "positive" && (entry.cssPath || entry.htmlPath))
      fail(`Diagnostic manifest entry must not have runnable assets: ${key}`);
  }
  if (
    manifestKeys.size !== exampleByKey.size ||
    [...exampleByKey.keys()].some((key) => !manifestKeys.has(key))
  )
    fail("Preview manifest must cover every positive and expected-diagnostic record exactly once");

  const guideMarkers = { en: new Map(), ja: new Map() };
  for (const locale of ["en", "ja"]) {
    const contentRoot = join(root, "docs/src/content", locale === "en" ? "docs" : "docs-ja");
    const windRoot = join(contentRoot, "zudo-wind");
    assertMdxInventory(
      windRoot,
      WIND_GUIDE_PAGES.map((page) => `${page}.mdx`),
      `${locale} guide source inventory`,
    );
    assertMdxInventory(
      join(windRoot, "utilities"),
      ["index.mdx", ...WIND_REFERENCE_FAMILIES.map(({ id }) => `${id}.mdx`)],
      `${locale} utility output inventory`,
    );
    const localeMarkers = guideMarkers[locale];
    for (const page of WIND_GUIDE_PAGES) {
      const path = join(windRoot, `${page}.mdx`);
      const source = readFileSync(path, "utf8");
      const markers = collectGuideMarkers(source);
      const expectedFamily = page === "index" ? null : `guide-${page}`;
      for (const marker of markers) {
        if (marker.family !== expectedFamily)
          fail(
            `${relative(root, path)}: preview ${marker.family}/${marker.id} belongs on its paired guide page`,
          );
        const record = records.find((item) => item.family === marker.family);
        if (!record?.examples.some((example) => example.id === marker.id))
          fail(`${relative(root, path)}: unknown guide preview ${marker.family}/${marker.id}`);
        const key = `${marker.family}/${marker.id}`;
        if (localeMarkers.has(key)) fail(`${locale}: duplicate guide preview ${key}`);
        localeMarkers.set(key, page);
      }
    }
  }

  for (const family of guideIds) {
    const record = records.find((item) => item.family === family);
    for (const example of record.examples) {
      const key = `${family}/${example.id}`;
      if (!guideMarkers.en.has(key) || !guideMarkers.ja.has(key))
        fail(`${key} must be referenced by both localized guide pages`);
      if (guideMarkers.en.get(key) !== guideMarkers.ja.get(key))
        fail(`${key} must stay on the same guide page in both locales`);
    }
  }
  if (guideMarkers.en.size !== guideMarkers.ja.size || guideMarkers.en.size !== 41)
    fail("English and Japanese guide blocks must pair all 41 guide examples");

  const windBaseline = readJson(
    join(root, "docs/scripts/__tests__/fixtures/wind-built-anchors.v1.json"),
  );
  const adjacentBaseline = readJson(
    join(root, "docs/scripts/__tests__/fixtures/wind-adjacent-built-anchors.v1.json"),
  );
  requireCount(
    Object.keys(windBaseline),
    EXPECTED.historicalWindRoutes,
    "Historical wind route baseline",
  );
  requireCount(Object.keys(adjacentBaseline), EXPECTED.adjacentRoutes, "Adjacent route baseline");
  const windSources = Object.keys(windBaseline).map((route) => sourceForBuiltRoute(root, route));
  const adjacentSources = Object.keys(adjacentBaseline).map((route) =>
    sourceForBuiltRoute(root, route),
  );
  adjacentSources.push(join(root, "crates/zudo-wind/README.md"));
  requireCount(windSources, EXPECTED.historicalWindRoutes, "Wind source pages");
  requireCount(adjacentSources, EXPECTED.adjacentSources, "Adjacent audited source files");
  if (
    new Set([...windSources, ...adjacentSources]).size !==
    windSources.length + adjacentSources.length
  )
    fail("Wind and adjacent audit sources must be distinct");

  const sources = [...windSources, ...adjacentSources];
  const adjacentReadme = join(root, "crates/zudo-wind/README.md");
  for (const source of sources) {
    if (source === adjacentReadme) await auditReadmeLinks(source, root);
    else await parseSource(source, root);
  }

  // Keep a direct provenance assertion beside the count contract: displayed
  // source, compiled HTML, and the record bytes use the same trailing newline.
  for (const family of WIND_REFERENCE_FAMILIES) {
    for (const example of preview.examples.get(family.id).examples) {
      const asset = preview.assets.get(`${family.id}/${example.id}`);
      if (asset.html !== exampleSource(example))
        fail(`Source bytes differ for ${family.id}/${example.id}`);
    }
  }

  return {
    adjacentRoutes: Object.keys(adjacentBaseline).length,
    adjacentSources: adjacentSources.length,
    catalogEntries: catalog.entries.length,
    editorialFamilies: editorial.size,
    examples: exampleByKey.size,
    guidePages: WIND_GUIDE_PAGES.length,
    guideRecords: guideIds.size,
    historicalWindRoutes: Object.keys(windBaseline).length,
    negativeExamples: counts.negative,
    parsedSources: sources.length,
    positiveExamples: counts.positive,
    utilityFamilies: WIND_REFERENCE_FAMILIES.length,
  };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  auditWindDocsCoverage().then(
    (summary) => {
      console.log(
        `Wind docs coverage passed: ${summary.utilityFamilies} families, ${summary.catalogEntries} entries, ` +
          `${summary.examples} examples (${summary.positiveExamples} positive, ${summary.negativeExamples} diagnostic), ` +
          `${summary.guidePages} bilingual guide pages, ${summary.historicalWindRoutes} historical routes, ` +
          `${summary.adjacentSources} adjacent source files, ${summary.parsedSources} source pages parsed.`,
      );
    },
    (error) => {
      console.error(error instanceof Error ? error.message : String(error));
      process.exitCode = 1;
    },
  );
}
