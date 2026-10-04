import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { format } from "@takazudo/mdx-formatter";
import { describe, expect, it } from "vite-plus/test";

import viteConfig from "../../vite.config.mjs";

// Formatter ownership contract (#3558): Oxfmt (`vp fmt`) owns the ordinary-code
// extensions Prettier formatted before; @takazudo/mdx-formatter 1.2.1 alone
// owns .md and .mdx. The Markdown cases pin 1.2.1's output on this repo's
// supported syntax, so a formatter drift or a lost `.mdx-formatter.json`
// setting fails here instead of as a repository-wide reformat.

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const read = (path) => readFileSync(join(rootDir, path), "utf8");
const packageJson = JSON.parse(read("package.json"));
const lefthook = read("lefthook.yml");
const mdxConfigPath = join(rootDir, ".mdx-formatter.json");
const mdxConfig = JSON.parse(read(".mdx-formatter.json"));
const fmt = viteConfig.fmt;

const ORDINARY_GLOB = "**/*.{js,mjs,cjs,ts,tsx,json,yml,yaml}";
const MARKDOWN_GLOB = "'**/*.{md,mdx}'";

function hookCommand(name) {
  const match = lefthook.match(
    new RegExp(`\\n    ${name}:\\n      glob: "([^"]+)"\\n      run: "([^"]+)"\\n`),
  );
  if (!match) throw new Error(`missing lefthook command: ${name}`);
  return { glob: match[1], run: match[2] };
}

const formatMarkdown = (content) => format(content, { config: mdxConfigPath });

describe("ordinary-code formatting is Oxfmt through vp fmt", () => {
  it("keeps the former Prettier settings and disables sorting", () => {
    expect(fmt).toMatchObject({
      semi: true,
      singleQuote: false,
      trailingComma: "all",
      printWidth: 100,
      sortPackageJson: false,
    });
    expect(fmt.sortImports ?? false).toBe(false);
    expect(fmt.sortTailwindcss ?? false).toBe(false);
  });

  it("excludes Markdown, foreign formats and the protected goldens", () => {
    expect(fmt.ignorePatterns).toEqual(
      expect.arrayContaining([
        "*.md",
        "*.mdx",
        "*.html",
        "*.css",
        "pnpm-lock.yaml",
        "crates/zfb-content/tests/fixtures/two-collections-schemas/expected.d.ts",
        "packages/zfb/src/__tests__/zudo-react/fixtures/md-roundtrip/**",
      ]),
    );
  });

  it("runs the package scripts over the former Prettier glob", () => {
    expect(packageJson.scripts["format:ts"]).toBe(`vp fmt --write "${ORDINARY_GLOB}"`);
    expect(packageJson.scripts["format:check:ts"]).toBe(`vp fmt --check "${ORDINARY_GLOB}"`);
  });

  it("has no direct Prettier dependency, config or caller", () => {
    expect(packageJson.devDependencies).not.toHaveProperty("prettier");
    expect(existsSync(join(rootDir, ".prettierrc.json"))).toBe(false);
    expect(existsSync(join(rootDir, ".prettierignore"))).toBe(false);
    expect(JSON.stringify(packageJson.scripts)).not.toMatch(/prettier/);
    expect(lefthook).not.toMatch(/prettier/);
  });
});

describe("Markdown and MDX belong to the local mdx-formatter 1.2.1", () => {
  it("is an exact devDependency invoked through its local bin", () => {
    expect(packageJson.devDependencies["@takazudo/mdx-formatter"]).toBe("1.2.1");
    const installed = JSON.parse(read("node_modules/@takazudo/mdx-formatter/package.json"));
    expect(installed.version).toBe("1.2.1");
    expect(installed.bin).toHaveProperty("mdx-formatter");
    expect(packageJson.scripts["format:mdx"]).toBe(`mdx-formatter --write ${MARKDOWN_GLOB}`);
    expect(packageJson.scripts["format:check:mdx"]).toBe(`mdx-formatter --check ${MARKDOWN_GLOB}`);
    expect(JSON.stringify(packageJson.scripts)).not.toMatch(/dlx/);
  });

  it("keeps the formatter split in format and format:check", () => {
    expect(packageJson.scripts.format).toBe("pnpm format:ts && pnpm format:mdx");
    expect(packageJson.scripts["format:check"]).toBe(
      "pnpm format:check:ts && pnpm format:check:mdx",
    );
  });

  it("gives the two pre-commit hooks disjoint globs", () => {
    const ts = hookCommand("format-ts");
    const mdx = hookCommand("format-mdx");
    expect(ts.glob).toBe("*.{js,mjs,cjs,ts,tsx,json,yml,yaml}");
    expect(ts.run).toBe("pnpm exec vp fmt --write --no-error-on-unmatched-pattern {staged_files}");
    expect(mdx.glob).toBe("*.{md,mdx}");
    expect(mdx.run).toBe("pnpm exec mdx-formatter --write {staged_files}");
    expect(lefthook.match(/stage_fixed: true/g)).toHaveLength(2);
  });

  it("keeps the block components and the HtmlPreview multiline exclusion", () => {
    expect(mdxConfig.addEmptyLinesInBlockJsx.blockComponents).toEqual([
      "Note",
      "Tip",
      "Info",
      "Warning",
      "Danger",
      "Details",
      "Tabs",
      "HtmlPreview",
    ]);
    expect(mdxConfig.formatMultiLineJsx.ignoreComponents).toEqual(["HtmlPreview"]);
    expect(mdxConfig.exclude).toContain(
      "packages/zfb/src/__tests__/zudo-react/fixtures/md-roundtrip/**",
    );
  });

  it.each([
    // :::note, titled admonitions, <Details>/<Note>, frontmatter, fences
    "docs/src/content/docs/recipes/admonitions.mdx",
    // the same in Japanese
    "docs/src/content/docs-ja/recipes/admonitions.mdx",
    // nested ::::/::: directives
    "docs/src/content/docs/markdown-features/directives.mdx",
    // <Tabs>
    "docs/src/content/docs/getting-started/installation.mdx",
    "crates/zfb-content/tests/fixtures/03-admonitions.mdx",
  ])("leaves the formatted %s unchanged", async (path) => {
    const source = read(path);
    expect(await formatMarkdown(source)).toBe(source);
  });

  it.each([
    ["<Note>", "<Note>\nBody.\n</Note>\n", "<Note>\n\nBody.\n\n</Note>\n"],
    [
      "titled <Note>",
      '<Note title="Heads up">\nBody.\n</Note>\n',
      '<Note title="Heads up">\n\nBody.\n\n</Note>\n',
    ],
    [
      "<Tabs>",
      '<Tabs>\n<TabItem label="pnpm">\nA\n</TabItem>\n</Tabs>\n',
      '<Tabs>\n\n<TabItem label="pnpm">\nA\n</TabItem>\n\n</Tabs>\n',
    ],
    [
      "<Details>",
      '<Details summary="More">\nHidden.\n</Details>\n',
      '<Details summary="More">\n\nHidden.\n\n</Details>\n',
    ],
    [
      "HtmlPreview raw HTML (excluded from multiline JSX)",
      "<HtmlPreview html={`<div>\n      <p>x</p>\n</div>`}>\n</HtmlPreview>\n",
      "<HtmlPreview html={`<div>\n      <p>x</p>\n</div>`}>\n</HtmlPreview>\n",
    ],
    [
      "HtmlPreview attributes (excluded from multiline JSX)",
      '<HtmlPreview\n      title="x"\n  height={200}\n>\n<div>\n<p>x</p>\n</div>\n</HtmlPreview>\n',
      '<HtmlPreview\n      title="x"\n  height={200}\n>\n<div>\n  <p>x</p>\n</div>\n</HtmlPreview>\n',
    ],
    ["titled directive", ":::warning[Careful]\nBody\n:::\n", ":::warning[Careful]\nBody\n:::\n"],
    [
      "nested directives",
      "::::note\nOuter\n:::tip\nInner\n:::\n::::\n",
      "::::note\nOuter\n:::tip\nInner\n:::\n::::\n",
    ],
    [
      "Japanese text",
      "# 見出し\n日本語のテキストとEnglish混在。\n- 項目\n",
      "# 見出し\n\n日本語のテキストとEnglish混在。\n\n- 項目\n",
    ],
    [
      "frontmatter and a fence",
      "---\ntitle:   Foo\n---\n# Hi\n```js\nconst a  =  1\n```\n",
      "---\ntitle: Foo\n---\n# Hi\n\n```js\nconst a  =  1\n```\n",
    ],
  ])("formats %s as 1.2.1 does, idempotently", async (_name, input, expected) => {
    const once = await formatMarkdown(input);
    expect(once).toBe(expected);
    expect(await formatMarkdown(once)).toBe(once);
  });
});
