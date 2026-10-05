#!/usr/bin/env node
// Generate committed zudo-wind utility reference pages from the versioned catalog export.
import { existsSync, readFileSync, readdirSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";
import { loadRecords, exampleSource } from "./wind-preview-assets.mjs";
import {
  loadEditorial,
  validateEditorial,
  loadPreviewContext,
  candidateDeclarations,
} from "./wind-reference-editorial.mjs";
import { WIND_REFERENCE_FAMILIES } from "./wind-reference-families.mjs";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(SCRIPT_DIR, "../..");
const DEFAULT_CATALOG = "crates/zudo-wind/catalog/zudo-wind-catalog.v1.json";

const REQUIRED_STRING_PATHS = [
  "index.title",
  "index.description",
  "index.intro",
  "index.specVersionLabel",
  "index.familyColumn",
  "index.entryCountColumn",
  "labels.field",
  "labels.value",
  "labels.catalogIdentifier",
  "labels.classPattern",
  "labels.grammar",
  "labels.acceptedValues",
  "labels.tokenCategories",
  "labels.negativePolicy",
  "labels.conflictGroup",
  "labels.orderRank",
  "labels.selectorShape",
  "labels.specVersion",
  "labels.emitterProperties",
  "labels.noAcceptedValues",
  "labels.noTokenCategories",
  "labels.noDeclarations",
  "labels.noExamples",
  "headings.acceptedValues",
  "headings.declarationTemplates",
  "headings.examples",
  "headings.relatedGuides",
  "columns.kind",
  "columns.suffix",
  "columns.emittedValue",
  "columns.property",
  "columns.valueKind",
  "columns.fixedValue",
  "columns.candidate",
  "columns.expectedDeclarations",
  "links.utilityGrammar",
  "links.variants",
];

function getPath(value, path) {
  return path.split(".").reduce((current, part) => current?.[part], value);
}

function compareText(left, right) {
  return left < right ? -1 : left > right ? 1 : 0;
}

function codeSpan(value) {
  const text = String(value);
  const longestTicks = Math.max(0, ...[...text.matchAll(/`+/g)].map(([run]) => run.length));
  const fence = "`".repeat(longestTicks + 1);
  const padded = text.startsWith("`") || text.endsWith("`") || /^\s|\s$/.test(text);
  const content = text;
  return `${fence}${padded ? " " : ""}${content}${padded ? " " : ""}${fence}`;
}

function escapeMdxText(value) {
  return String(value)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/{/g, "&#123;")
    .replace(/}/g, "&#125;");
}

export function escapeTableCell(value) {
  return String(value).replace(/\|/g, "\\|").replace(/\r?\n/g, " ");
}

function table(headers, rows) {
  const lines = [
    `| ${headers.map((value) => escapeTableCell(value)).join(" | ")} |`,
    `| ${headers.map(() => "---").join(" | ")} |`,
  ];
  for (const row of rows) {
    lines.push(`| ${row.map((value) => escapeTableCell(value)).join(" | ")} |`);
  }
  return lines.join("\n");
}

function frontmatter({ title, description, sidebarPosition }) {
  const yamlString = (value) => {
    const text = String(value);
    const safePlain =
      /^[\p{L}\p{N}][\p{L}\p{N} .,;!?()/、。・-]*$/u.test(text) &&
      !/^(?:true|false|null|yes|no|on|off|~|[0-9]+(?:\.[0-9]+)?)$/i.test(text);
    return safePlain ? text : JSON.stringify(text);
  };
  return [
    "---",
    `title: ${yamlString(title)}`,
    `description: ${yamlString(description)}`,
    `sidebar_position: ${sidebarPosition}`,
    "generated: true",
    "---",
  ].join("\n");
}

function classPattern(entry) {
  const hasBareForm = entry.acceptedValues?.some((value) => value.suffix === null);
  const hasSuffixForm = (entry.grammar?.acceptedKinds ?? []).some((kind) => kind !== "exact");
  const forms = [];
  if (hasBareForm) forms.push(codeSpan(entry.root));
  if (hasSuffixForm) forms.push(codeSpan(`${entry.root}-<suffix>`));
  return forms.length > 0 ? forms.join(" or ") : codeSpan(entry.root);
}

function renderGrammar(entry) {
  const grammar = entry.grammar;
  return codeSpan(
    JSON.stringify({
      acceptedKinds: grammar.acceptedKinds,
      arbitraryProperty: grammar.arbitraryProperty,
      allowsFractionSlash: grammar.allowsFractionSlash,
      allowsColorOpacity: grammar.allowsColorOpacity,
    }),
  );
}

function renderValueRows(values, columns) {
  return table(
    [columns.kind, columns.suffix, columns.emittedValue].map(escapeMdxText),
    values.map((value) => [
      codeSpan(value.kind),
      codeSpan(JSON.stringify(value.suffix)),
      codeSpan(JSON.stringify(value.emittedValue)),
    ]),
  );
}

function renderTemplateRows(templates, columns) {
  return table(
    [columns.property, columns.valueKind, columns.fixedValue].map(escapeMdxText),
    templates.map((template) => [
      codeSpan(template.property),
      codeSpan(template.valueKind),
      codeSpan(JSON.stringify(template.fixedValue)),
    ]),
  );
}

function renderExampleRows(examples, columns, noDeclarations) {
  return table(
    [columns.candidate, columns.expectedDeclarations].map(escapeMdxText),
    examples.map((example) => [
      codeSpan(example.candidate),
      example.declarations.length > 0
        ? example.declarations
            .map((declaration) => codeSpan(`${declaration.property}: ${declaration.value}`))
            .join("; ")
        : escapeMdxText(noDeclarations),
    ]),
  );
}

export function validateCatalogFamilies(catalog, families) {
  if (!Array.isArray(catalog?.entries)) {
    throw new Error("Catalog must contain an entries array.");
  }
  if (!Array.isArray(families)) {
    throw new Error("Utility families must be an array.");
  }

  const entriesById = new Map();
  for (const entry of catalog.entries) {
    if (typeof entry?.id !== "string" || entry.id.length === 0) {
      throw new Error("Every catalog entry must have a non-empty id.");
    }
    if (entriesById.has(entry.id)) {
      throw new Error(`Catalog entry "${entry.id}" appears more than once.`);
    }
    entriesById.set(entry.id, entry);
  }

  const assignedTo = new Map();
  const familyIds = new Set();
  let previousFamilyRank = 0;
  for (const family of families) {
    if (typeof family?.id !== "string" || !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(family.id)) {
      throw new Error(`Invalid utility family id "${family?.id}".`);
    }
    if (familyIds.has(family.id)) {
      throw new Error(`Utility family "${family.id}" appears more than once.`);
    }
    familyIds.add(family.id);
    if (!Array.isArray(family.entries)) {
      throw new Error(`Utility family "${family.id}" must contain an entries array.`);
    }
    if (family.entries.length === 0) {
      throw new Error(`Utility family "${family.id}" must name at least one catalog entry.`);
    }
    const familyOrderRanks = new Set();
    for (const entryId of family.entries) {
      if (!entriesById.has(entryId)) {
        throw new Error(`Utility family "${family.id}" names missing catalog entry "${entryId}".`);
      }
      const catalogEntry = entriesById.get(entryId);
      if (catalogEntry.conflictGroup !== family.id) {
        throw new Error(
          `Catalog entry "${entryId}" declares conflict group "${catalogEntry.conflictGroup}" but is assigned to family "${family.id}".`,
        );
      }
      if (!Number.isInteger(catalogEntry.conflictGroupRank) || catalogEntry.conflictGroupRank < 1) {
        throw new Error(`Catalog entry "${entryId}" has an invalid conflict-group rank.`);
      }
      familyOrderRanks.add(catalogEntry.conflictGroupRank);
      const previousFamilies = assignedTo.get(entryId) ?? [];
      previousFamilies.push(family.id);
      assignedTo.set(entryId, previousFamilies);
    }
    if (familyOrderRanks.size > 1) {
      throw new Error(
        `Utility family "${family.id}" has inconsistent catalog conflict-group ranks.`,
      );
    }
    const familyRank = familyOrderRanks.values().next().value;
    if (familyRank <= previousFamilyRank) {
      throw new Error(`Utility family "${family.id}" is out of catalog conflict-group order.`);
    }
    previousFamilyRank = familyRank;
  }

  for (const entry of catalog.entries) {
    const owningFamilies = assignedTo.get(entry.id) ?? [];
    if (owningFamilies.length === 0) {
      throw new Error(`Catalog entry "${entry.id}" belongs to no utility family.`);
    }
    if (owningFamilies.length > 1) {
      throw new Error(
        `Catalog entry "${entry.id}" belongs to more than one utility family: ${owningFamilies.join(", ")}.`,
      );
    }
  }

  return entriesById;
}

export function validateLocaleStrings(strings, families) {
  for (const path of REQUIRED_STRING_PATHS) {
    if (typeof getPath(strings, path) !== "string") {
      throw new Error(`Missing locale string key "${path}".`);
    }
  }
  if (!strings.families || typeof strings.families !== "object") {
    throw new Error('Missing locale string key "families".');
  }
  for (const family of families) {
    for (const key of ["title", "description"]) {
      if (typeof strings.families[family.id]?.[key] !== "string") {
        throw new Error(`Missing locale string key "families.${family.id}.${key}".`);
      }
    }
  }
}

function renderEntry(entry, strings) {
  const labels = strings.labels;
  const rows = [
    [escapeMdxText(labels.catalogIdentifier), codeSpan(entry.id)],
    [escapeMdxText(labels.classPattern), classPattern(entry)],
    [escapeMdxText(labels.grammar), renderGrammar(entry)],
    [
      escapeMdxText(labels.acceptedValues),
      entry.acceptedValues.length
        ? codeSpan(entry.acceptedValues.length)
        : escapeMdxText(labels.noAcceptedValues),
    ],
    [
      escapeMdxText(labels.tokenCategories),
      entry.tokenCategories.length
        ? entry.tokenCategories.map(codeSpan).join(", ")
        : escapeMdxText(labels.noTokenCategories),
    ],
    [escapeMdxText(labels.negativePolicy), codeSpan(entry.negativePolicy)],
    [
      escapeMdxText(labels.conflictGroup),
      `${codeSpan(entry.conflictGroup)} (${codeSpan(entry.conflictGroupRank)})`,
    ],
    [escapeMdxText(labels.orderRank), codeSpan(entry.orderRank)],
    [escapeMdxText(labels.selectorShape), codeSpan(entry.selectorShape)],
    [escapeMdxText(labels.specVersion), codeSpan(entry.specVersion)],
    [
      escapeMdxText(labels.emitterProperties),
      entry.emitter.length
        ? entry.emitter.map(codeSpan).join(", ")
        : escapeMdxText(labels.noDeclarations),
    ],
  ];

  return [
    `## ${escapeMdxText(entry.root)}`,
    "",
    table([escapeMdxText(labels.field), escapeMdxText(labels.value)], rows),
    "",
    `### ${escapeMdxText(strings.headings.acceptedValues)}`,
    "",
    entry.acceptedValues.length
      ? renderValueRows(entry.acceptedValues, strings.columns)
      : escapeMdxText(labels.noAcceptedValues),
    "",
    `### ${escapeMdxText(strings.headings.declarationTemplates)}`,
    "",
    entry.declarationTemplates.length
      ? renderTemplateRows(entry.declarationTemplates, strings.columns)
      : escapeMdxText(labels.noDeclarations),
    "",
    `### ${escapeMdxText(strings.headings.examples)}`,
    "",
    entry.examples.length
      ? renderExampleRows(entry.examples, strings.columns, labels.noDeclarations)
      : escapeMdxText(labels.noExamples),
  ].join("\n");
}

// Deliberate rollout allowance; #3708 removes it after all 47 records are authored.
export const TRANSITIONAL_ALLOW_MISSING_EDITORIAL = true;

function expression(value) {
  // JSON literals preserve exact bytes and keep user text inside an MDX expression.
  return JSON.stringify(value)
    .replace(/</g, "\\u003c")
    .replace(/>/g, "\\u003e")
    .replace(/&/g, "\\u0026");
}

function fenced(value, language) {
  const fence = "`".repeat(
    Math.max(3, ...[...String(value).matchAll(/`+/g)].map(([run]) => run.length + 1)),
  );
  return `${fence}${language}\n${String(value).trimEnd()}\n${fence}`;
}

function renderEditorial(family, record, strings, locale, preview, entriesById) {
  validateEditorial(record, family, preview.examples.get(family.id), entriesById, preview.assets);
  const content = record.locales[locale];
  if (!content) throw new Error(`Editorial ${family.id}: unsupported locale ${locale}`);
  const labels = strings.reader;
  if (
    !labels ||
    [
      "lookup",
      "setup",
      "workedExamples",
      "customValues",
      "technical",
      "config",
      "scaffold",
      "diagnostics",
      "taskColumn",
      "browse",
    ].some((key) => typeof labels[key] !== "string")
  )
    throw new Error("Missing reader locale labels");
  const examples = new Map(
    preview.examples.get(family.id).examples.map((example) => [example.id, example]),
  );
  const rows = record.lookup.map((row) => {
    const asset = preview.assets.get(`${family.id}/${row.example}`);
    if (!asset) throw new Error(`Missing preview asset ${family.id}/${row.example}`);
    return [codeSpan(row.candidate), codeSpan(candidateDeclarations(asset.css, row.candidate))];
  });
  return [
    escapeMdxText(content.purpose),
    "",
    `## ${escapeMdxText(labels.lookup)}`,
    "",
    table([strings.columns.candidate, strings.columns.expectedDeclarations], rows),
    "",
    `<Details title={${expression(labels.setup)}}>`,
    "",
    escapeMdxText(content.setup),
    "",
    "</Details>",
    "",
    `## ${escapeMdxText(labels.workedExamples)}`,
    "",
    "<WindPreviewEnhancer />",
    "",
    ...content.examples.flatMap((item) => {
      const example = examples.get(item.id);
      const asset = preview.assets.get(`${family.id}/${item.id}`);
      const source = exampleSource(example);
      return [
        `### ${escapeMdxText(item.title)}`,
        "",
        escapeMdxText(item.description),
        "",
        ...(example.kind === "positive"
          ? [
              `<HtmlPreview html={${expression(source)}} css={${expression(asset.css)}} title={${expression(item.title)}} lang=${expression(locale)} preflight={false} defaultOpen={true} showSource={true} showViewportControls={true} />`,
            ]
          : [
              fenced(source, "html"),
              "",
              `${escapeMdxText(labels.diagnostics)}: ${example.expectedDiagnostics.map((item) => codeSpan(`${item.code} (${item.severity})`)).join(", ")}`,
            ]),
        "",
        `<Details title={${expression(labels.config)}}>`,
        "",
        fenced(JSON.stringify(asset?.config ?? preview.baseConfig, null, 2), "json"),
        "",
        "</Details>",
        "",
        `<Details title={${expression(labels.scaffold)}}>`,
        "",
        fenced(example.scaffoldCss, "css"),
        "",
        "</Details>",
        "",
      ];
    }),
    `## ${escapeMdxText(labels.customValues)}`,
    "",
    escapeMdxText(content.customValues),
  ].join("\n");
}

export function renderReferencePages(catalog, strings, families, options = {}) {
  const entriesById = validateCatalogFamilies(catalog, families);
  validateLocaleStrings(strings, families);

  const { editorial, preview, locale = "en", allowMissingEditorial = false } = options;
  if (editorial) {
    for (const id of editorial.keys())
      if (!families.some((family) => family.id === id))
        throw new Error(`Unknown editorial family ${id}`);
    if (!allowMissingEditorial)
      for (const family of families)
        if (!editorial.has(family.id)) throw new Error(`Missing editorial family ${family.id}`);
  }
  const output = new Map();
  const indexRows = [];
  for (const family of families) {
    const localized = strings.families[family.id];
    const familyRank = entriesById.get(family.entries[0]).conflictGroupRank;
    indexRows.push([
      `[${escapeMdxText(localized.title)}](${family.id}.mdx)`,
      codeSpan(family.entries.length),
    ]);

    const entries = family.entries
      .map((entryId) => entriesById.get(entryId))
      .sort(
        (left, right) =>
          left.orderRank - right.orderRank ||
          compareText(left.root, right.root) ||
          compareText(left.id, right.id),
      );
    const record = editorial?.get(family.id);
    const page = [
      frontmatter({
        title: localized.title,
        description: localized.description,
        sidebarPosition: 20 + familyRank,
      }),
      "",
      record
        ? renderEditorial(family, record, strings, locale, preview, entriesById)
        : escapeMdxText(localized.description),
      "",
      `**${escapeMdxText(strings.headings.relatedGuides)}:** [${escapeMdxText(strings.links.utilityGrammar)}](../utility-grammar.mdx) · [${escapeMdxText(strings.links.variants)}](../variants.mdx)`,
      "",
      ...(record ? [`<Details title={${expression(strings.reader.technical)}}>`, ""] : []),
      ...entries.flatMap((entry, entryIndex) => [
        ...(entryIndex > 0 ? [""] : []),
        renderEntry(entry, strings),
        ...(record && entry.registrations?.length
          ? ["", codeSpan(JSON.stringify({ registrations: entry.registrations }))]
          : []),
      ]),
      "",
      ...(record ? ["</Details>", ""] : []),
    ].join("\n");
    output.set(`${family.id}.mdx`, page);
  }

  const index = [
    frontmatter({
      title: strings.index.title,
      description: strings.index.description,
      sidebarPosition: 20,
    }),
    "",
    escapeMdxText(strings.index.intro),
    "",
    ...(strings.reader ? [`## ${escapeMdxText(strings.reader.browse)}`, ""] : []),
    table(
      [
        strings.index.familyColumn,
        ...(strings.reader ? [strings.reader.taskColumn] : []),
        strings.index.entryCountColumn,
      ].map(escapeMdxText),
      indexRows.map((row, index) =>
        strings.reader
          ? [row[0], escapeMdxText(strings.families[families[index].id].description), row[1]]
          : row,
      ),
    ),
    "",
    `${escapeMdxText(strings.index.specVersionLabel)}: ${codeSpan(catalog.specVersion)}.`,
    "",
    '<CategoryNav category="zudo-wind/utilities" />',
    "",
  ].join("\n");
  output.set("index.mdx", index);

  return new Map([...output.entries()].sort(([left], [right]) => compareText(left, right)));
}

export function compareGeneratedPages(expectedPages, existingFiles) {
  const changed = [];
  for (const [name, content] of expectedPages) {
    if (existingFiles.get(name) !== content) changed.push(name);
  }
  const stale = [];
  for (const [name, content] of existingFiles) {
    if (!expectedPages.has(name) && hasGeneratedFrontmatter(content)) stale.push(name);
  }
  return {
    changed: changed.sort(compareText),
    stale: stale.sort(compareText),
  };
}

function hasGeneratedFrontmatter(content) {
  const match = String(content).match(/^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/);
  return Boolean(match && /^generated:\s*true\s*$/m.test(match[1]));
}

function readExistingPages(outputDir) {
  const files = new Map();
  if (!existsSync(outputDir)) return files;
  for (const entry of readdirSync(outputDir, { withFileTypes: true })) {
    if (!entry.isFile() || !entry.name.endsWith(".mdx")) continue;
    files.set(entry.name, readFileSync(join(outputDir, entry.name), "utf8"));
  }
  return files;
}

function writeGeneratedPages(outputDir, expectedPages, existingFiles) {
  mkdirSync(outputDir, { recursive: true });
  const { stale } = compareGeneratedPages(expectedPages, existingFiles);
  for (const name of stale) rmSync(join(outputDir, name));
  for (const [name, content] of expectedPages) {
    writeFileSync(join(outputDir, name), content);
  }
}

function parseArgs(args) {
  const options = { locale: undefined, catalog: DEFAULT_CATALOG, check: false };
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--check") {
      options.check = true;
    } else if (arg === "--locale" || arg === "--catalog") {
      const value = args[index + 1];
      if (!value || value.startsWith("--")) {
        throw new Error(`Option ${arg} requires a value.`);
      }
      index += 1;
      options[arg.slice(2)] = value;
    } else {
      throw new Error(`Unknown option "${arg}".`);
    }
  }
  if (!options.locale) throw new Error('Missing required option "--locale <code>".');
  if (!/^[a-z]{2}(?:-[A-Z]{2})?$/.test(options.locale)) {
    throw new Error(`Invalid locale code "${options.locale}".`);
  }
  return options;
}

async function runCli(args) {
  const options = parseArgs(args);
  const catalogPath = resolve(REPO_ROOT, options.catalog);
  const catalog = JSON.parse(readFileSync(catalogPath, "utf8"));
  validateCatalogFamilies(catalog, WIND_REFERENCE_FAMILIES);
  console.log(`entries=${catalog.entries.length} families=${WIND_REFERENCE_FAMILIES.length}`);

  const stringsPath = join(SCRIPT_DIR, "wind-reference-strings", `${options.locale}.mjs`);
  if (!existsSync(stringsPath)) {
    throw new Error(
      `Missing string table for locale "${options.locale}" at ${relative(REPO_ROOT, stringsPath)}.`,
    );
  }
  const module = await import(pathToFileURL(stringsPath).href);
  const strings = module.default ?? module.strings;
  const editorial = loadEditorial(join(SCRIPT_DIR, "wind-reference-editorial"));
  const preview = loadPreviewContext(REPO_ROOT, loadRecords());
  const pages = renderReferencePages(catalog, strings, WIND_REFERENCE_FAMILIES, {
    editorial,
    preview,
    locale: options.locale,
    allowMissingEditorial: TRANSITIONAL_ALLOW_MISSING_EDITORIAL,
  });
  const outputDir = join(
    REPO_ROOT,
    options.locale === "en"
      ? "docs/src/content/docs/zudo-wind/utilities"
      : `docs/src/content/docs-${options.locale}/zudo-wind/utilities`,
  );
  const existingFiles = readExistingPages(outputDir);
  if (options.check) {
    const diff = compareGeneratedPages(pages, existingFiles);
    for (const name of diff.changed)
      console.error(`would change: ${relative(REPO_ROOT, join(outputDir, name))}`);
    for (const name of diff.stale)
      console.error(
        `would remove stale generated page: ${relative(REPO_ROOT, join(outputDir, name))}`,
      );
    if (diff.changed.length || diff.stale.length) process.exitCode = 1;
    return;
  }

  writeGeneratedPages(outputDir, pages, existingFiles);
}

const scriptPath = fileURLToPath(import.meta.url);
if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  runCli(process.argv.slice(2)).catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
