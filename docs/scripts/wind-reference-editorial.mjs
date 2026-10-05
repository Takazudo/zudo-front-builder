import { createHash } from "node:crypto";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { exampleAssetPath, exampleSource, mergeConfig } from "./wind-preview-assets.mjs";

const fail = (message) => {
  throw new Error(message);
};
const nonblank = (value) => typeof value === "string" && value.trim().length > 0;

export function loadEditorial(directory) {
  return new Map(
    readdirSync(directory)
      .filter((name) => name.endsWith(".json"))
      .sort()
      .map((name) => {
        const record = JSON.parse(readFileSync(join(directory, name), "utf8"));
        if (name !== `${record.family}.json`) fail(`${name}: editorial filename must match family`);
        return [record.family, record];
      }),
  );
}

// Variant separators inside arbitrary selector/value brackets are literal bytes.
function unvariedCandidate(candidate) {
  let nesting = 0;
  let start = 0;
  let escaped = false;
  for (let index = 0; index < candidate.length; index += 1) {
    const ch = candidate[index];
    if (escaped) {
      escaped = false;
      continue;
    }
    if (ch === "\\") {
      escaped = true;
      continue;
    }
    if (ch === "[" || ch === "(") nesting += 1;
    else if (ch === "]" || ch === ")") nesting -= 1;
    else if (ch === ":" && nesting === 0) start = index + 1;
  }
  return candidate.slice(start);
}

/** Each worker owns one bilingual JSON file; no shared translation edits are needed. */
export function validateEditorial(record, family, examples, entriesById, assets) {
  const label = `Editorial ${family.id}`;
  if (record?.schemaVersion !== 1 || record.family !== family.id)
    fail(`${label}: invalid identity`);
  const byId = new Map((examples?.examples ?? []).map((example) => [example.id, example]));
  if (!Array.isArray(record.lookup) || !record.lookup.length) fail(`${label}: lookup required`);
  const covered = new Set();
  for (const row of record.lookup) {
    const example = byId.get(row.example);
    if (
      !family.entries.includes(row.entry) ||
      !nonblank(row.candidate) ||
      example?.kind !== "positive" ||
      !example.utilities.includes(row.candidate)
    )
      fail(`${label}: lookup must reference a family entry and a positive declared candidate`);
    if (entriesById) {
      const entry = entriesById.get(row.entry);
      const root = entry.root;
      const core = unvariedCandidate(row.candidate);
      const unsigned = core.startsWith("-") ? core.slice(1) : core;
      const exactOnly = entry.grammar.acceptedKinds.every((kind) => kind === "exact");
      if (exactOnly ? core !== root : unsigned !== root && !unsigned.startsWith(`${root}-`))
        fail(`${label}: lookup candidate does not match catalog root ${root}`);
      if (assets) {
        const asset = assets.get(`${family.id}/${row.example}`);
        if (!asset?.css) fail(`${label}: missing lookup CSS for ${row.example}`);
        const declarations = candidateDeclarations(asset.css, row.candidate);
        const properties = new Set(
          [...declarations.matchAll(/(?:^|;)\s*([\w-]+)\s*:/g)].map((match) => match[1]),
        );
        const optional = new Set(
          (entry.declarationTemplates ?? [])
            .filter((template) => template.valueKind === "optionalFontSizeLeading")
            .map((template) => template.property),
        );
        const missing = entry.emitter.filter(
          (property) => !optional.has(property) && !properties.has(property),
        );
        if (missing.length)
          fail(`${label}: ${row.candidate} does not emit catalog properties ${missing.join(", ")}`);
      }
    }
    covered.add(row.entry);
  }
  if (family.entries.some((entry) => !covered.has(entry)))
    fail(`${label}: lookup must cover every family entry`);
  for (const locale of ["en", "ja"]) {
    const content = record.locales?.[locale];
    for (const key of ["purpose", "setup", "customValues"])
      if (!nonblank(content?.[key])) fail(`${label}: missing ${locale}.${key}`);
    if (!Array.isArray(content.examples) || !content.examples.length)
      fail(`${label}: ${locale} examples required`);
    const ids = new Set();
    for (const item of content.examples) {
      if (
        !byId.has(item.id) ||
        ids.has(item.id) ||
        !nonblank(item.title) ||
        !nonblank(item.description)
      )
        fail(`${label}: invalid ${locale} example reference`);
      ids.add(item.id);
    }
    if (record.lookup.some((row) => !ids.has(row.example)))
      fail(`${label}: ${locale} must explain every lookup example`);
  }
  if (
    JSON.stringify(record.locales.en.examples.map((item) => item.id)) !==
    JSON.stringify(record.locales.ja.examples.map((item) => item.id))
  )
    fail(`${label}: locale example order must match`);
  return record;
}

// CSSOM identifier serialization, matching zudo-wind's public selector contract.
function escapeClass(value) {
  return [...value]
    .map((ch, index) => {
      const cp = ch.codePointAt(0);
      if (cp === 0) return "\ufffd";
      if (
        cp <= 31 ||
        cp === 127 ||
        (/[0-9]/.test(ch) && (index === 0 || (index === 1 && value.startsWith("-"))))
      )
        return `\\${cp.toString(16)} `;
      if (ch === "-" && value === "-") return "\\-";
      return cp >= 128 || /[a-zA-Z0-9_-]/.test(ch) ? ch : `\\${ch}`;
    })
    .join("");
}

/** Read declarations from genuine compiler rules, including child/pseudo selectors. */
export function candidateDeclarations(css, candidate) {
  const selector = `.${escapeClass(candidate)}`;
  const declarations = [];
  for (const [, selectors, body] of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    // Escaped candidate bytes must be followed by a selector boundary.
    let offset = selectors.indexOf(selector);
    while (offset !== -1) {
      const after = selectors.slice(offset + selector.length);
      if (!after || /^[\s:>+~,.#[]/.test(after)) {
        declarations.push(body.trim().replace(/\s+/g, " "));
        break;
      }
      offset = selectors.indexOf(selector, offset + selector.length);
    }
  }
  if (!declarations.length) fail(`No compiled declaration for ${candidate}`);
  return [...new Set(declarations)].join(" ");
}

/** Fail closed on missing/stale HTML or CSS without invoking a compiler in docs generation. */
export function loadPreviewContext(root, exampleRecords) {
  const publicDir = join(root, "docs/public");
  const manifest = JSON.parse(readFileSync(join(publicDir, "wind-examples/manifest.json"), "utf8"));
  const baseConfig = JSON.parse(
    readFileSync(join(root, "docs/wind-examples/base-config.json"), "utf8"),
  );
  const examples = new Map(exampleRecords.map((record) => [record.family, record]));
  const assets = new Map();
  const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
  for (const record of exampleRecords)
    for (const example of record.examples) {
      const config = mergeConfig(baseConfig, example.config);
      if (example.kind === "positive") config.wind.strict = true;
      config.wind.authoredClasses = Object.fromEntries(
        example.authoredClasses.map((name) => [name, true]),
      );
      config.wind.safelist =
        example.kind === "positive" || example.candidateOrigin !== "source"
          ? { "docs-preview": example.utilities }
          : {};
      const html = exampleSource(example);
      if (example.kind !== "positive") {
        assets.set(`${record.family}/${example.id}`, { html, config });
        continue;
      }
      const css = readFileSync(
        join(publicDir, exampleAssetPath(record.family, example.id)),
        "utf8",
      );
      const storedHtml = readFileSync(
        join(publicDir, exampleAssetPath(record.family, example.id, "html")),
        "utf8",
      );
      const provenance = manifest.examples.find(
        (item) => item.family === record.family && item.id === example.id,
      );
      if (
        !provenance ||
        storedHtml !== html ||
        provenance.htmlSha256 !== hash(html) ||
        provenance.cssSha256 !== hash(css) ||
        provenance.inputSha256 !== hash(`${example.scaffoldCss.trimEnd()}\n`) ||
        provenance.configSha256 !== hash(`${JSON.stringify(config, null, 2)}\n`)
      )
        fail(`Stale preview source/CSS: ${record.family}/${example.id}; regenerate preview assets`);
      assets.set(`${record.family}/${example.id}`, { html, css, config });
    }
  return { examples, assets, baseConfig };
}
