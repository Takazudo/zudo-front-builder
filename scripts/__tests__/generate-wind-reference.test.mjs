import { describe, expect, it } from "vitest";
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
