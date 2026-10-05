import { describe, expect, it } from "vite-plus/test";
import {
  compareGeneratedPages,
  escapeTableCell,
  renderReferencePages,
} from "../../docs/scripts/generate-wind-reference.mjs";

const catalog = {
  schemaVersion: 1,
  specVersion: 1,
  entries: [
    {
      id: "v1.flex",
      root: "flex",
      grammar: {
        acceptedKinds: ["exact"],
        arbitraryProperty: null,
        allowsFractionSlash: false,
        allowsColorOpacity: false,
      },
      acceptedValues: [{ kind: "exact", suffix: null, emittedValue: "flex" }],
      tokenCategories: [],
      negativePolicy: "forbidden",
      conflictGroup: "display",
      conflictGroupRank: 1,
      orderRank: 0,
      selectorShape: "ownElement",
      emitter: ["display"],
      declarationTemplates: [{ property: "display", valueKind: "resolved", fixedValue: null }],
      specVersion: 1,
      examples: [{ candidate: "flex", declarations: [{ property: "display", value: "flex" }] }],
    },
    {
      id: "v1.margin",
      root: "m",
      grammar: {
        acceptedKinds: ["scale"],
        arbitraryProperty: null,
        allowsFractionSlash: false,
        allowsColorOpacity: false,
      },
      acceptedValues: [{ kind: "scale", suffix: "2", emittedValue: "0.5rem" }],
      tokenCategories: ["spacing"],
      negativePolicy: "allowed",
      conflictGroup: "margin",
      conflictGroupRank: 10,
      orderRank: 0,
      selectorShape: "ownElement",
      emitter: ["margin-top"],
      declarationTemplates: [{ property: "margin-top", valueKind: "resolved", fixedValue: null }],
      specVersion: 1,
      examples: [
        {
          candidate: "m|<{x}>",
          declarations: [{ property: "margin-top", value: "-0.5rem" }],
        },
      ],
    },
  ],
};

const families = [
  { id: "display", entries: ["v1.flex"] },
  { id: "margin", entries: ["v1.margin"] },
];

const strings = {
  index: {
    title: "Utility reference",
    description: "Browse the utility catalog.",
    intro: "Browse generated entries.",
    specVersionLabel: "Catalog spec version",
    familyColumn: "Family",
    entryCountColumn: "Entries",
  },
  labels: {
    field: "Field",
    value: "Value",
    catalogIdentifier: "Catalog identifier",
    classPattern: "Class pattern",
    grammar: "Grammar",
    acceptedValues: "Accepted values",
    tokenCategories: "Token categories",
    negativePolicy: "Negative policy",
    conflictGroup: "Conflict group and rank",
    orderRank: "Order rank",
    selectorShape: "Selector shape",
    specVersion: "Spec version",
    emitterProperties: "Emitter properties",
    noAcceptedValues: "No separate accepted values.",
    noTokenCategories: "No token category.",
    noDeclarations: "No declaration records.",
    noExamples: "No catalog examples.",
  },
  headings: {
    acceptedValues: "Accepted values",
    declarationTemplates: "Declaration templates",
    examples: "Catalog examples",
    relatedGuides: "Related guides",
  },
  columns: {
    kind: "Kind",
    suffix: "Suffix",
    emittedValue: "Emitted value",
    property: "Property",
    valueKind: "Value kind",
    fixedValue: "Fixed value",
    candidate: "Example class",
    expectedDeclarations: "Expected declarations",
  },
  links: { utilityGrammar: "Utility grammar", variants: "Variants" },
  families: {
    display: { title: "Display", description: "Control display." },
    margin: { title: "Margin", description: "Control outer spacing." },
  },
};

describe("generate-wind-reference", () => {
  it("writes Japanese frontmatter without unnecessary quotes", () => {
    const localized = structuredClone(strings);
    localized.index.title = "ユーティリティリファレンス";
    localized.index.description = "有限のカタログを参照します。";
    localized.families.display.title = "表示";
    const pages = renderReferencePages(catalog, localized, families);
    expect(pages.get("index.mdx")).toContain("title: ユーティリティリファレンス\n");
    expect(pages.get("display.mdx")).toContain("title: 表示\n");
    expect(pages.get("index.mdx")).toContain("description: 有限のカタログを参照します。\n");
  });
  it("escapes table delimiters and newlines", () => {
    expect(escapeTableCell("first|second\nthird")).toBe("first\\|second third");
  });

  it("renders exact index MDX and documents negative-value policy", () => {
    const pages = renderReferencePages(catalog, strings, families);
    const expectedIndex = [
      "---",
      "title: Utility reference",
      "description: Browse the utility catalog.",
      "sidebar_position: 20",
      "generated: true",
      "---",
      "",
      "Browse generated entries.",
      "",
      "| Family | Entries |",
      "| --- | --- |",
      "| [Display](display.mdx) | `1` |",
      "| [Margin](margin.mdx) | `1` |",
      "",
      "Catalog spec version: `1`.",
      "",
      '<CategoryNav category="zudo-wind/utilities" />',
      "",
    ].join("\n");
    expect(pages.get("index.mdx")).toBe(expectedIndex);

    const expectedDisplay = [
      "---",
      "title: Display",
      "description: Control display.",
      "sidebar_position: 21",
      "generated: true",
      "---",
      "",
      "Control display.",
      "",
      "**Related guides:** [Utility grammar](../utility-grammar.mdx) · [Variants](../variants.mdx)",
      "",
      "## flex",
      "",
      "| Field | Value |",
      "| --- | --- |",
      "| Catalog identifier | `v1.flex` |",
      "| Class pattern | `flex` |",
      '| Grammar | `{"acceptedKinds":["exact"],"arbitraryProperty":null,"allowsFractionSlash":false,"allowsColorOpacity":false}` |',
      "| Accepted values | `1` |",
      "| Token categories | No token category. |",
      "| Negative policy | `forbidden` |",
      "| Conflict group and rank | `display` (`1`) |",
      "| Order rank | `0` |",
      "| Selector shape | `ownElement` |",
      "| Spec version | `1` |",
      "| Emitter properties | `display` |",
      "",
      "### Accepted values",
      "",
      "| Kind | Suffix | Emitted value |",
      "| --- | --- | --- |",
      '| `exact` | `null` | `"flex"` |',
      "",
      "### Declaration templates",
      "",
      "| Property | Value kind | Fixed value |",
      "| --- | --- | --- |",
      "| `display` | `resolved` | `null` |",
      "",
      "### Catalog examples",
      "",
      "| Example class | Expected declarations |",
      "| --- | --- |",
      "| `flex` | `display: flex` |",
      "",
    ].join("\n");
    expect(pages.get("display.mdx")).toBe(expectedDisplay);
    const margin = pages.get("margin.mdx");
    expect(margin).toContain("sidebar_position: 30");
    expect(margin).toContain("| Negative policy | `allowed` |");
    expect(margin).toContain("| `m\\|<{x}>` |");
    expect(margin).toContain("`margin-top: -0.5rem`");
  });

  it("requires every locale family key instead of falling back", () => {
    const incomplete = structuredClone(strings);
    delete incomplete.families.margin.description;

    expect(() => renderReferencePages(catalog, incomplete, families)).toThrow(
      'Missing locale string key "families.margin.description".',
    );
  });

  it("names catalog entries that belong to no family", () => {
    const incomplete = {
      ...catalog,
      entries: [...catalog.entries, { ...catalog.entries[0], id: "v1.orphan" }],
    };

    expect(() => renderReferencePages(incomplete, strings, families)).toThrow(
      'Catalog entry "v1.orphan" belongs to no utility family.',
    );
  });

  it("rejects family maps that name an entry missing from the catalog", () => {
    const incompleteFamilies = [...families, { id: "unknown", entries: ["v1.missing"] }];

    expect(() => renderReferencePages(catalog, strings, incompleteFamilies)).toThrow(
      'Utility family "unknown" names missing catalog entry "v1.missing".',
    );
  });

  it("reports stale generated pages but leaves hand-authored pages alone", () => {
    const expected = new Map([["current.mdx", "current page"]]);
    const existing = new Map([
      ["current.mdx", "current page"],
      ["stale.mdx", "---\ntitle: Stale\ngenerated: true\n---\n"],
      ["hand-authored.mdx", "---\ntitle: Hand authored\n---\n"],
    ]);

    expect(compareGeneratedPages(expected, existing)).toEqual({
      changed: [],
      stale: ["stale.mdx"],
    });
  });

  it("is deterministic when catalog entries and family entry lists are shuffled", () => {
    const expected = renderReferencePages(catalog, strings, families);
    const shuffledCatalog = { ...catalog, entries: [...catalog.entries].reverse() };
    const shuffledFamilies = families.map((family) => ({
      ...family,
      entries: [...family.entries].reverse(),
    }));

    expect(renderReferencePages(shuffledCatalog, strings, shuffledFamilies)).toEqual(expected);
  });
});

// The reader-first contract exercises the committed compiler assets, without
// invoking Rust or building the docs site in the content worker's unit lane.
import { readFileSync, mkdtempSync, cpSync, rmSync, writeFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { tmpdir } from "node:os";
import { WIND_REFERENCE_FAMILIES } from "../../docs/scripts/wind-reference-families.mjs";
import { loadRecords, exampleSource } from "../../docs/scripts/wind-preview-assets.mjs";
import {
  candidateDeclarations,
  loadEditorial,
  loadPreviewContext,
  validateEditorial,
} from "../../docs/scripts/wind-reference-editorial.mjs";
import en from "../../docs/scripts/wind-reference-strings/en.mjs";
import ja from "../../docs/scripts/wind-reference-strings/ja.mjs";
import { extractAllHeadingIds } from "../../docs/node_modules/@takazudo/zudo-doc/dist/extract-headings/index.js";

const root = resolve(import.meta.dirname, "../..");
const realCatalog = JSON.parse(
  readFileSync(join(root, "crates/zudo-wind/catalog/zudo-wind-catalog.v1.json"), "utf8"),
);
const editorial = loadEditorial(join(root, "docs/scripts/wind-reference-editorial"));
const anchorContract = JSON.parse(
  readFileSync(join(root, "scripts/__tests__/fixtures/wind-reference-anchors.v1.json"), "utf8"),
);
const records = loadRecords(root);
const preview = loadPreviewContext(root, records);
const options = { editorial, preview, allowMissingEditorial: true };

describe("reader-first wind pages", () => {
  it("preserves all family/entry membership, old root and nested anchors, and nonpilot bytes in both locales", () => {
    expect(WIND_REFERENCE_FAMILIES).toHaveLength(47);
    expect(WIND_REFERENCE_FAMILIES.flatMap((family) => family.entries)).toHaveLength(187);
    for (const [locale, localized] of [
      ["en", en],
      ["ja", ja],
    ]) {
      const before = renderReferencePages(realCatalog, localized, WIND_REFERENCE_FAMILIES);
      const after = renderReferencePages(realCatalog, localized, WIND_REFERENCE_FAMILIES, {
        ...options,
        locale,
      });
      expect(after.size).toBe(48);
      for (const family of WIND_REFERENCE_FAMILIES) {
        const name = `${family.id}.mdx`;
        const newIds = extractAllHeadingIds(after.get(name));
        for (const id of anchorContract.locales[locale][name]) expect(newIds).toContain(id);
        expect(new Set(newIds).size).toBe(newIds.length);
        if (!editorial.has(family.id)) expect(after.get(name)).toBe(before.get(name));
        for (const entry of family.entries) expect(after.get(name)).toContain(`\`${entry}\``);
        expect(after.get("index.mdx")).toContain(`](${name})`);
      }
    }
  });

  it("uses exact compiled/displayed HTML and inline isolated styles with visible native controls", () => {
    const pages = renderReferencePages(realCatalog, en, WIND_REFERENCE_FAMILIES, options);
    for (const family of ["gap", "padding"]) {
      const page = pages.get(`${family}.mdx`);
      for (const example of preview.examples.get(family).examples) {
        const encoded = JSON.stringify(exampleSource(example))
          .replace(/</g, "\\u003c")
          .replace(/>/g, "\\u003e")
          .replace(/&/g, "\\u0026");
        expect(page).toContain(`html={${encoded}}`);
      }
      expect(page).toContain(
        "preflight={false} defaultOpen={true} showSource={true} showViewportControls={true}",
      );
      expect(page).not.toContain("externalStyles");
      expect(page.indexOf("## Quick reference")).toBeLessThan(
        page.indexOf('title={"Example setup"}'),
      );
      expect(page.indexOf('title={"Example setup"}')).toBeLessThan(page.indexOf("<HtmlPreview"));
      expect(page.indexOf("<HtmlPreview")).toBeLessThan(page.indexOf("## Custom and named values"));
      expect(page.indexOf("## Custom and named values")).toBeLessThan(
        page.indexOf('title={"Catalog details"}'),
      );
    }
    expect(pages.get("gap.mdx")).toContain("column-gap: var(--zw-spacing-gutter)");
    expect(pages.get("padding.mdx")).toContain("padding-left: 2rem");
  });

  it("requires exhaustive bilingual schemas and supports explicit transitional missing records only", () => {
    expect(() =>
      renderReferencePages(realCatalog, en, WIND_REFERENCE_FAMILIES, { editorial, preview }),
    ).toThrow("Missing editorial family display");
    const gap = WIND_REFERENCE_FAMILIES.find((family) => family.id === "gap");
    const record = structuredClone(editorial.get("gap"));
    record.lookup = record.lookup.filter((row) => row.entry !== "v1.gap-y");
    expect(() => validateEditorial(record, gap, preview.examples.get("gap"))).toThrow(
      "lookup must cover every family entry",
    );
    const missingLocale = structuredClone(editorial.get("gap"));
    delete missingLocale.locales.ja.setup;
    expect(() => validateEditorial(missingLocale, gap, preview.examples.get("gap"))).toThrow(
      "missing ja.setup",
    );
    const wrongRoot = structuredClone(editorial.get("gap"));
    wrongRoot.lookup[0].entry = "v1.gap-y";
    expect(() =>
      validateEditorial(
        wrongRoot,
        gap,
        preview.examples.get("gap"),
        new Map(realCatalog.entries.map((entry) => [entry.id, entry])),
      ),
    ).toThrow("does not match catalog root");
    const invalidCandidate = structuredClone(editorial.get("gap"));
    invalidCandidate.lookup[0].candidate = "unknown";
    expect(() => validateEditorial(invalidCandidate, gap, preview.examples.get("gap"))).toThrow(
      "positive declared candidate",
    );
  });
  it("passes verified fragment metadata to the native Head source panel", () => {
    const context = structuredClone(preview);
    context.examples.get("gap").examples[0].head = '<base href="about:srcdoc">';
    context.assets.get("gap/grid-gap").head = '<base href="about:srcdoc">';
    const page = renderReferencePages(realCatalog, en, WIND_REFERENCE_FAMILIES, {
      ...options,
      preview: context,
    }).get("gap.mdx");
    expect(page).toContain('head={"\\u003cbase href=\\"about:srcdoc\\"\\u003e"}');
    expect(page.match(/ head=/g)).toHaveLength(1);
  });

  it("binds exact roots and shared roots to their actual catalog properties", () => {
    function validate(entry, candidate, css) {
      const family = { id: entry.conflictGroup, entries: [entry.id] };
      const localized = {
        purpose: "Purpose",
        setup: "Explicit setup",
        customValues: "Supported values",
        examples: [{ id: "sample", title: "Sample", description: "Sample behavior" }],
      };
      const record = {
        schemaVersion: 1,
        family: family.id,
        lookup: [{ entry: entry.id, candidate, example: "sample" }],
        locales: { en: localized, ja: localized },
      };
      const example = { id: "sample", kind: "positive", utilities: [candidate] };
      return validateEditorial(
        record,
        family,
        { examples: [example] },
        new Map([[entry.id, entry]]),
        new Map([[`${family.id}/sample`, { css }]]),
      );
    }
    const entry = (id) => realCatalog.entries.find((item) => item.id === id);
    expect(() =>
      validate(
        entry("v1.grid"),
        "grid-cols-3",
        ".grid-cols-3 { grid-template-columns: repeat(3,minmax(0,1fr)); }",
      ),
    ).toThrow("does not match catalog root");
    expect(() =>
      validate(entry("v1.flex"), "flex-col", ".flex-col { flex-direction: column; }"),
    ).toThrow("does not match catalog root");
    expect(() =>
      validate(entry("v1.font.family"), "font-[400]", ".font-\\[400\\] { font-weight: 400; }"),
    ).toThrow("does not emit catalog properties font-family");
    expect(() =>
      validate(entry("v1.grid"), "hover:grid", ".hover\\:grid:hover { display: grid; }"),
    ).not.toThrow();
    expect(() =>
      validate(
        entry("v1.font.family"),
        "font-[serif]",
        ".font-\\[serif\\] { font-family: serif; font-weight: 400; }",
      ),
    ).not.toThrow();
    expect(() =>
      validate(
        entry("v1.m"),
        "hover:-m-2",
        ".hover\\:-m-2:hover { margin-top: -0.5rem; margin-right: -0.5rem; margin-bottom: -0.5rem; margin-left: -0.5rem; }",
      ),
    ).not.toThrow();
    expect(() =>
      validate(
        entry("v1.gap"),
        "gap-[var(--x:spacing)]",
        ".gap-\\[var\\(--x\\:spacing\\)\\] { column-gap: var(--x); row-gap: var(--x); }",
      ),
    ).not.toThrow();
  });

  it("shows diagnostic teaching source and configured severity without creating a runnable iframe", () => {
    const diagnostic = structuredClone(
      records.find((record) => record.family === "diagnostics").examples[0],
    );
    const changedRecords = structuredClone(records);
    changedRecords.find((record) => record.family === "gap").examples.push(diagnostic);
    const changedEditorial = structuredClone(editorial);
    for (const locale of ["en", "ja"])
      changedEditorial.get("gap").locales[locale].examples.push({
        id: diagnostic.id,
        title: "Unsupported input",
        description: "Expected compiler diagnostic",
      });
    const context = loadPreviewContext(root, changedRecords);
    const page = renderReferencePages(realCatalog, en, WIND_REFERENCE_FAMILIES, {
      ...options,
      editorial: changedEditorial,
      preview: context,
    }).get("gap.mdx");
    expect(page).toContain(diagnostic.html);
    expect(page).toContain(
      `${diagnostic.expectedDiagnostics[0].code} (${diagnostic.expectedDiagnostics[0].severity})`,
    );
    expect(page.match(/<HtmlPreview /g)).toHaveLength(4);
  });

  it("escapes authored text and JSX strings, including source-like delimiters", () => {
    const changed = structuredClone(editorial);
    changed.get("gap").locales.en.purpose = "<script>{secret}</script> & |";
    changed.get("gap").locales.en.examples[0].title = 'A </script> "quoted" {value}';
    const page = renderReferencePages(realCatalog, en, WIND_REFERENCE_FAMILIES, {
      ...options,
      editorial: changed,
    }).get("gap.mdx");
    expect(page).toContain("&lt;script&gt;&#123;secret&#125;&lt;/script&gt; &amp;");
    expect(page).toContain('title={"A \\u003c/script\\u003e \\"quoted\\" {value}"}');
  });

  it("reads escaped candidate and child selectors while rejecting longer candidate prefixes", () => {
    const css =
      ".gap-40 { column-gap: 10rem; }\n.gap-4 { column-gap: 1rem; }\n.space-x-4 > :not([hidden]) ~ :not([hidden]) { margin-left: 1rem; }\n.gap-\\[18px\\] { column-gap: 18px; }";
    expect(candidateDeclarations(css, "gap-4")).toBe("column-gap: 1rem;");
    expect(candidateDeclarations(css, "space-x-4")).toBe("margin-left: 1rem;");
    expect(candidateDeclarations(css, "gap-[18px]")).toBe("column-gap: 18px;");
    expect(() => candidateDeclarations(css, "gap-3")).toThrow("No compiled declaration");
  });

  it("fails on stale HTML, CSS, scaffold input or merged configuration", () => {
    const temporary = mkdtempSync(join(tmpdir(), "wind-editorial-test-"));
    try {
      cpSync(join(root, "docs/wind-examples"), join(temporary, "docs/wind-examples"), {
        recursive: true,
      });
      cpSync(
        join(root, "docs/public/wind-examples"),
        join(temporary, "docs/public/wind-examples"),
        { recursive: true },
      );
      const modified = structuredClone(records);
      modified.find((record) => record.family === "gap").examples[0].scaffoldCss +=
        "\n/* changed */";
      expect(() => loadPreviewContext(temporary, modified)).toThrow("Stale preview source/CSS");
      const configChanged = structuredClone(records);
      configChanged.find((record) => record.family === "gap").examples[0].config = {
        wind: { tokens: { spacingUnit: "1rem" } },
      };
      expect(() => loadPreviewContext(temporary, configChanged)).toThrow(
        "Stale preview source/CSS",
      );
      const headChanged = structuredClone(records);
      headChanged.find((record) => record.family === "gap").examples[0].head =
        '<base href="about:srcdoc">';
      expect(() => loadPreviewContext(temporary, headChanged)).toThrow("Stale preview source/CSS");
      const htmlPath = join(temporary, "docs/public/wind-examples/gap/grid-gap.html");
      const originalHtml = readFileSync(htmlPath, "utf8");
      writeFileSync(htmlPath, originalHtml + "<!-- stale -->");
      expect(() => loadPreviewContext(temporary, records)).toThrow("Stale preview source/CSS");
      writeFileSync(htmlPath, originalHtml);
      const cssPath = join(temporary, "docs/public/wind-examples/gap/grid-gap.css");
      writeFileSync(cssPath, readFileSync(cssPath, "utf8") + "/* stale */");
      expect(() => loadPreviewContext(temporary, records)).toThrow("Stale preview source/CSS");
    } finally {
      rmSync(temporary, { recursive: true, force: true });
    }
  });
});
