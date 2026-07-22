// PROTOTYPE coverage (zfb#1855, epic zfb#1854 — parseToAst go/no-go spike;
// pruned on a Wave-2 no-go).
//
// Test plan (declared per project testing discipline):
// - Level: 3 (build output that RUNS) — like the sibling test files, this
//   imports the BUILT package (`dist/`) and drives the real wasm artifact
//   through Node, proving the export survives the wasm-bindgen boundary,
//   the glue wiring, and JSON round-trip. The exhaustive position-shift
//   proof lives at level 1 in `crates/zfb-md-wasm/tests/parse_to_ast.rs`;
//   this file re-proves the recursive whole-tree position contract through
//   the REAL artifact (slice-equality against the original source — an
//   independent anchor, not a re-derivation of the Rust arithmetic).
// - Blind spots: browser execution (the committed browser bench harness +
//   the md-wasm-browser-smoke Chromium lane cover that surface).
import { describe, it, expect } from "vitest";

import { parseToAst, type Diagnostic } from "../dist/index.js";

// Minimal structural view of a serialized mdast node — the public type is
// deliberately `unknown` (prototype), so the test narrows locally.
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

describe("parseToAst (PROTOTYPE, raw mdast export)", () => {
  it("returns the raw tree, frontmatter values, and zero diagnostics", async () => {
    const out = await parseToAst(FRONTMATTER_FIXTURE, { filename: "post.mdx" });
    expect(out.diagnostics).toHaveLength(0);
    expect(out.frontmatter).toEqual({ title: "Browser boundary proof", n: 42 });
    const ast = out.ast as MdastNodeish;
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
