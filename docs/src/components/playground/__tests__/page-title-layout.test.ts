import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

function readFrontmatterTitle(path: string): string {
  const source = readFileSync(new URL(path, import.meta.url), "utf8");
  const match = source.match(/^title:\s*(.+)$/m);
  if (match === null) throw new Error(`Missing title in ${path}`);
  return match[1].trim();
}

describe("playground page titles", () => {
  it("keeps highlightCode as the narrow-screen-safe API title in both locales", () => {
    expect(readFrontmatterTitle("../../../content/docs/playground/highlight.mdx")).toBe(
      "highlightCode",
    );
    expect(readFrontmatterTitle("../../../content/docs-ja/playground/highlight.mdx")).toBe(
      "highlightCode",
    );
  });
});
