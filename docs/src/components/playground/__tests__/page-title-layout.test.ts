import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";
import zfbConfig from "../../../../zfb.config";

interface HeaderNavTestItem {
  label: string;
  path: string;
  categoryMatch?: string;
  children?: HeaderNavTestItem[];
}

const playgroundPagePaths = [
  "../../../content/docs/playground/index.mdx",
  "../../../content/docs/playground/render.mdx",
  "../../../content/docs/playground/compile.mdx",
  "../../../content/docs/playground/parse.mdx",
  "../../../content/docs/playground/highlight.mdx",
  "../../../content/docs-ja/playground/index.mdx",
  "../../../content/docs-ja/playground/render.mdx",
  "../../../content/docs-ja/playground/compile.mdx",
  "../../../content/docs-ja/playground/parse.mdx",
  "../../../content/docs-ja/playground/highlight.mdx",
] as const;

function readFrontmatter(path: string): string {
  const source = readFileSync(new URL(path, import.meta.url), "utf8");
  const match = source.match(/^---\n([\s\S]*?)\n---/);
  if (match === null) throw new Error(`Missing frontmatter in ${path}`);
  return match[1];
}

function readFrontmatterTitle(path: string): string {
  const match = readFrontmatter(path).match(/^title:\s*(.+)$/m);
  if (match === null) throw new Error(`Missing title in ${path}`);
  return match[1].trim();
}

describe("playground page layout", () => {
  it.each(playgroundPagePaths)("hides both documentation rails in %s", (path) => {
    const frontmatter = readFrontmatter(path);

    expect(frontmatter).toMatch(/^hide_sidebar:\s*true$/m);
    expect(frontmatter).toMatch(/^hide_toc:\s*true$/m);
  });

  it("groups every playground under the parent header item", () => {
    const routesPlugin = zfbConfig.plugins?.find(
      (plugin) => plugin.name === "@takazudo/zudo-doc/plugins/routes",
    );
    const settings = routesPlugin?.options?.settings as
      | { headerNav?: HeaderNavTestItem[] }
      | undefined;
    const playgroundNav = settings?.headerNav?.find((item) => item.label === "Playground");

    expect(playgroundNav).toEqual({
      label: "Playground",
      path: "/docs/playground",
      categoryMatch: "playground",
      children: [
        { label: "Index", path: "/docs/playground" },
        { label: "renderHtml", path: "/docs/playground/render" },
        { label: "compile", path: "/docs/playground/compile" },
        { label: "parseToAst", path: "/docs/playground/parse" },
        { label: "highlightCode", path: "/docs/playground/highlight" },
      ],
    });
  });

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
