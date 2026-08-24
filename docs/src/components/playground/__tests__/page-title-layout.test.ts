import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

function readFrontmatterTitle(path: string): string {
  const source = readFileSync(new URL(path, import.meta.url), "utf8");
  const match = source.match(/^title:\s*(.+)$/m);
  if (match === null) throw new Error(`Missing title in ${path}`);
  return match[1].trim();
}

describe("playground page titles", () => {
  it("keeps the required locale titles and pins their page-scoped wrapping rule", () => {
    expect(readFrontmatterTitle("../../../content/docs/playground/highlight.mdx")).toBe(
      "highlightCode Playground",
    );
    expect(readFrontmatterTitle("../../../content/docs-ja/playground/highlight.mdx")).toBe(
      "highlightCode プレイグラウンド",
    );

    const globalStyles = readFileSync(
      new URL("../../../styles/global.css", import.meta.url),
      "utf8",
    );
    expect(globalStyles).toMatch(
      /\.zd-content:has\(\[data-zfb-island="HighlightPlayground"\]\) > h1\.text-heading\s*\{\s*overflow-wrap:\s*anywhere;/,
    );
  });
});
