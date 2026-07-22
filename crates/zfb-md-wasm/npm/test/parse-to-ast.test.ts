// Coverage for the `parseToAst` export (zfb#1857, epic zfb#1854).
//
// Test plan (declared per project testing discipline):
// - Level: 3 (build output that RUNS) — like the sibling test files, this
//   imports the BUILT package (`dist/`) and drives the real wasm artifact
//   through Node, proving the export survives the wasm-bindgen boundary,
//   the glue wiring, and JSON round-trip. The exhaustive position-shift
//   proof (including the UTF-16 conversion itself) lives at level 1 in
//   `crates/zfb-md-wasm/tests/parse_to_ast.rs`; this file re-proves the
//   recursive whole-tree position contract through the REAL artifact
//   (slice-equality against the original source — an independent anchor,
//   not a re-derivation of the Rust arithmetic) and adds the ONE proof that
//   genuinely needs a JS runtime: node-by-node UTF-16 position parity
//   against real `remark-parse`, the actual remark/unist reference
//   implementation this export claims compatibility with.
// - Blind spots: browser execution (the committed browser bench harness,
//   `package-browser.test.ts`'s bundled-entry check below, and the
//   md-wasm-browser-smoke Chromium lane cover that surface).
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { unified } from "unified";
import remarkParse from "remark-parse";
import { describe, it, expect } from "vitest";

import { parseToAst, type Diagnostic } from "../dist/index.js";

const testDir = dirname(fileURLToPath(import.meta.url));
const fixturesDir = join(testDir, "..", "..", "tests", "fixtures", "parse-to-ast");

interface ParseToAstFixture {
  slug: string;
  source: string;
  options: Record<string, unknown>;
}
interface ParseToAstManifest {
  fixtures: ParseToAstFixture[];
}
interface DiagnosticFixture {
  slug: string;
  source: string;
  options: { filename: string };
  line: number;
  column: number;
}
const manifest: ParseToAstManifest = JSON.parse(
  readFileSync(join(fixturesDir, "manifest.json"), "utf8"),
);
const diagnosticFixtures = (
  JSON.parse(readFileSync(join(fixturesDir, "diagnostics.json"), "utf8")) as {
    fixtures: DiagnosticFixture[];
  }
).fixtures;
function fixture(slug: string): ParseToAstFixture {
  const found = manifest.fixtures.find((f) => f.slug === slug);
  if (!found) throw new Error(`no fixture named \`${slug}\` in the parse-to-ast manifest`);
  return found;
}

// Minimal structural view of a serialized mdast node — a superset-safe local
// narrowing over the real `MdastNode` union, matching what this file's
// generic recursive walkers need.
interface MdastNodeish {
  type?: string;
  position?: {
    start: { line: number; column: number; offset: number };
    end: { line: number; column: number; offset: number };
  };
  [key: string]: unknown;
}

// JSX attributes serialize with a `type` tag but no position (markdown-rs
// does not model attribute positions — mirrored from the Rust-side proof).
const POSITIONLESS_TYPES = new Set([
  "mdxJsxAttribute",
  "mdxJsxAttributeValueExpression",
  "mdxJsxExpressionAttribute",
]);

const FRONTMATTER_FIXTURE = [
  "---",
  "title: Browser boundary proof",
  "n: 42",
  "---",
  "",
  "# Shifted heading",
  "",
  "Prose with *emphasis*, `code()` and ~~strike~~.",
  "",
  '<Badge kind="info" count={40 + 2} />',
  "",
  "{1 + 1}",
  "",
].join("\n");

function walk(value: unknown, visit: (node: MdastNodeish, path: string) => void, path = "ast") {
  if (Array.isArray(value)) {
    value.forEach((item, i) => walk(item, visit, `${path}[${i}]`));
    return;
  }
  if (value !== null && typeof value === "object") {
    const node = value as MdastNodeish;
    if (typeof node.type === "string") visit(node, path);
    for (const [key, nested] of Object.entries(node)) {
      walk(nested, visit, `${path}.${key}`);
    }
  }
}

describe("parseToAst (raw mdast export)", () => {
  it("returns the raw tree, frontmatter values, and zero diagnostics", async () => {
    const out = await parseToAst(FRONTMATTER_FIXTURE, { filename: "post.mdx" });
    expect(out.diagnostics).toHaveLength(0);
    expect(out.frontmatter).toEqual({ title: "Browser boundary proof", n: 42 });
    const ast = out.ast as unknown as MdastNodeish;
    expect(ast.type).toBe("root");
    const types = new Set<string>();
    walk(out.ast, (node) => types.add(node.type as string));
    // Contract shape: MDX element + expressions survive as raw mdx nodes,
    // pre-visitor (no zfb-synthesized node types anywhere).
    for (const required of [
      "heading",
      "emphasis",
      "inlineCode",
      "delete",
      "mdxJsxFlowElement",
      "mdxFlowExpression",
    ]) {
      expect(types, `tree must contain a \`${required}\` node`).toContain(required);
    }
  });

  it("recursively carries frontmatter-shifted positions on EVERY node (slice-equality proof)", async () => {
    const out = await parseToAst(FRONTMATTER_FIXTURE, { filename: "post.mdx" });
    const bodyOffset = FRONTMATTER_FIXTURE.indexOf("\n# Shifted heading") - 0;
    expect(bodyOffset).toBeGreaterThan(0);
    let nodes = 0;
    walk(out.ast, (node, path) => {
      if (POSITIONLESS_TYPES.has(node.type as string)) return;
      nodes += 1;
      expect(node.position, `${path} (\`${node.type}\`) must carry a position`).toBeDefined();
      const { start, end } = node.position!;
      // Offsets index the ORIGINAL source (frontmatter included): the text
      // slice they select must exist past the frontmatter block.
      expect(start.offset, `${path} start.offset must sit past the frontmatter`).toBeGreaterThan(
        node.type === "root" ? 0 : bodyOffset - 1,
      );
      expect(end.offset).toBeGreaterThanOrEqual(start.offset);
      expect(end.offset).toBeLessThanOrEqual(FRONTMATTER_FIXTURE.length);
      // Line numbers index the original source too: the 4 frontmatter
      // lines push every body line past 4.
      expect(start.line, `${path} start.line must sit past the frontmatter`).toBeGreaterThan(4);
      // Independent anchor: a text node's value must literally be the
      // slice its shifted offsets select from the ORIGINAL source.
      if (node.type === "text" || node.type === "inlineCode") {
        const slice = FRONTMATTER_FIXTURE.slice(start.offset, end.offset);
        if (node.type === "text") expect(slice).toBe(node.value);
        if (node.type === "inlineCode") expect(slice).toBe(`\`${node.value as string}\``);
      }
    });
    expect(nodes).toBeGreaterThan(10);
  });

  it("surfaces parse errors as markdown diagnostics with original-source lines", async () => {
    const out = await parseToAst("---\ntitle: x\n---\n<Card>\n", { filename: "bad.mdx" });
    expect(out.ast).toBeNull();
    expect(out.frontmatter).toEqual({ title: "x" });
    const diag: Diagnostic = out.diagnostics[0]!;
    expect(diag.source).toBe("markdown");
    expect(diag.message).toContain("closing tag");
    expect(diag.line).toBe(5);
  });
});

describe("parseToAst custom/unrecognized node survival (zfb#1828 requirement 3)", () => {
  it("round-trips an MDX-JSX custom component with type/name/attributes intact", async () => {
    const { source, options } = fixture("mdx-custom-component");
    const out = await parseToAst(source, options);
    expect(out.diagnostics).toHaveLength(0);
    const ast = out.ast as unknown as MdastNodeish & { children: MdastNodeish[] };
    const element = ast.children.find((n) => n.type === "mdxJsxFlowElement");
    expect(element, "fixture contains an mdxJsxFlowElement").toBeDefined();
    expect(element!.name).toBe("CustomComponent");
    expect(element!.position).toBeDefined();

    const attributes = element!.attributes as Array<Record<string, unknown>>;
    const prop = attributes.find((a) => a.name === "prop");
    expect(prop).toMatchObject({ type: "mdxJsxAttribute", name: "prop", value: "x" });

    const count = attributes.find((a) => a.name === "count");
    expect(count).toMatchObject({
      type: "mdxJsxAttribute",
      name: "count",
      value: { type: "mdxJsxAttributeValueExpression", value: "2 + 3" },
    });

    const paragraph = (element!.children as MdastNodeish[]).find((c) => c.type === "paragraph");
    expect(paragraph, "nested content forms a paragraph child").toBeDefined();
    const inline = paragraph!.children as MdastNodeish[];
    expect(inline.some((c) => c.type === "text")).toBe(true);
    expect(inline.some((c) => c.type === "emphasis")).toBe(true);
  });

  it("keeps `:::note` directive-convention text raw (pre-visitors, from the shared fixture)", async () => {
    const { source, options } = fixture("directive-convention");
    const out = await parseToAst(source, options);
    expect(out.diagnostics).toHaveLength(0);
    const ast = out.ast as unknown as MdastNodeish & { children: MdastNodeish[] };
    expect(ast.children[0]!.type).toBe("paragraph");
    const text = ast.children[0]!.children as MdastNodeish[];
    expect(text[0]!.value).toContain(":::note");
  });
});

describe("parseToAst markdown diagnostics use original-source UTF-16 coordinates", () => {
  for (const fixture of diagnosticFixtures) {
    it(fixture.slug, async () => {
      const out = await parseToAst(fixture.source, fixture.options);
      expect(out.ast).toBeNull();
      expect(out.diagnostics).toHaveLength(1);
      expect(out.diagnostics[0]).toMatchObject({
        source: "markdown",
        line: fixture.line,
        column: fixture.column,
      });
    });
  }
});

describe("parseToAst UTF-16 position parity with remark-parse (zfb#1856)", () => {
  interface FlatNode {
    type: string;
    start: { line: number; column: number; offset: number };
    end: { line: number; column: number; offset: number };
  }

  // Flattens a tree into pre-order (type, position) pairs -- deliberately a
  // DIFFERENT walking strategy than `walk()` above (that one visits every
  // object with a `type`; this one only records nodes carrying `position`,
  // matching what both mdast implementations actually attach it to).
  function flattenPositions(value: unknown, out: FlatNode[] = []): FlatNode[] {
    if (Array.isArray(value)) {
      for (const item of value) flattenPositions(item, out);
      return out;
    }
    if (value !== null && typeof value === "object") {
      const node = value as Record<string, unknown>;
      if (typeof node.type === "string" && node.position && typeof node.position === "object") {
        const position = node.position as { start: FlatNode["start"]; end: FlatNode["end"] };
        out.push({ type: node.type, start: position.start, end: position.end });
      }
      for (const nested of Object.values(node)) flattenPositions(nested, out);
    }
    return out;
  }

  it("matches remark-parse node-by-node on a CJK + emoji fixture (surrogate pairs included)", async () => {
    // The fixture is shared with the Rust-side
    // `utf16_positions_are_shifted_on_non_ascii_fixture` test; this is the
    // ONE proof that genuinely needs a JS runtime -- comparing against the
    // real remark/unist reference implementation, not a re-derivation of
    // the Rust conversion arithmetic.
    const { source } = fixture("non-ascii-cjk-emoji");
    const zfbOut = await parseToAst(source, { filename: "cjk.mdx" });
    expect(zfbOut.diagnostics).toHaveLength(0);

    const remarkTree = unified().use(remarkParse).parse(source);

    const zfbFlat = flattenPositions(zfbOut.ast);
    const remarkFlat = flattenPositions(remarkTree);

    // Structural parity first: if the node sequences themselves diverge,
    // the position-by-position comparison below would silently compare the
    // wrong pairs.
    expect(zfbFlat.map((n) => n.type)).toEqual(remarkFlat.map((n) => n.type));
    expect(zfbFlat.length).toBeGreaterThan(5);

    for (const [i, zfbNode] of zfbFlat.entries()) {
      const remarkNode = remarkFlat[i]!;
      expect(zfbNode.start, `${zfbNode.type}[${i}] start (UTF-16 units)`).toEqual(remarkNode.start);
      expect(zfbNode.end, `${zfbNode.type}[${i}] end (UTF-16 units)`).toEqual(remarkNode.end);
    }
  });
});
