import { afterEach, describe, expect, it } from "vitest";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import {
  defaultLocaleOnlyPrefixes,
  discoverDocPages,
  docRoute,
  missingTranslations,
} from "../docs-translation-coverage.mjs";

const temporary = [];
afterEach(() => {
  for (const directory of temporary.splice(0)) rmSync(directory, { recursive: true, force: true });
});

function fixture(english, japanese) {
  const root = mkdtempSync(join(tmpdir(), "docs-translation-coverage-"));
  temporary.push(root);
  for (const [locale, files] of [
    ["en", english],
    ["ja", japanese],
  ]) {
    mkdirSync(join(root, locale));
    for (const [file, body] of Object.entries(files)) {
      const path = join(root, locale, file);
      mkdirSync(dirname(path), { recursive: true });
      writeFileSync(path, body);
    }
  }
  return { englishDir: join(root, "en"), japaneseDir: join(root, "ja") };
}

const publishedPolicy = () =>
  defaultLocaleOnlyPrefixes(
    readFileSync(new URL("../../docs/zfb.config.ts", import.meta.url), "utf8"),
  );

describe("current documentation translation coverage", () => {
  it("provides a nonempty Japanese counterpart for every non-excluded English route", () => {
    expect(
      missingTranslations({
        englishDir: fileURLToPath(new URL("../../docs/src/content/docs", import.meta.url)),
        japaneseDir: fileURLToPath(new URL("../../docs/src/content/docs-ja", import.meta.url)),
        excludedPrefixes: publishedPolicy(),
      }),
    ).toEqual([]);
  });

  it("reports an ordinary missing page and an empty frontmatter-only translation", () => {
    const dirs = fixture(
      { "api/missing.mdx": "English", "guide/index.mdx": "English", "mdx-only.mdx": "English" },
      {
        "guide/index.mdx": "---\ntitle: Guide\n---\n<!-- pending -->\n",
        "mdx-only.mdx": "---\ntitle: Pending\n---\n{/* pending */}",
      },
    );
    expect(missingTranslations({ ...dirs, excludedPrefixes: [] })).toEqual([
      { route: "/docs/api/missing/", reason: "missing" },
      { route: "/docs/guide/", reason: "empty" },
      { route: "/docs/mdx-only/", reason: "empty" },
    ]);
  });

  it("honors the actual English-only policy, including category index routes, without excluding similarly named ordinary pages", () => {
    const policy = publishedPolicy();
    expect(policy.length).toBeGreaterThan(0);
    const excluded = policy[0].replace(/^\/docs\//, "").replace(/\/$/, "");
    const dirs = fixture(
      {
        [`${excluded}/index.mdx`]: "History",
        [`${excluded}/old.mdx`]: "History",
        [`${excluded}-guide.mdx`]: "Current guide",
      },
      {},
    );
    expect(missingTranslations({ ...dirs, excludedPrefixes: policy })).toEqual([
      { route: `/docs/${excluded}-guide/`, reason: "missing" },
    ]);
  });

  it("normalizes collection indexes and discovers equivalent Markdown/MDX routes", () => {
    const dirs = fixture({ "guide/index.mdx": "English" }, { "guide.md": "日本語" });
    expect(docRoute("index.mdx")).toBe("/docs/");
    expect(docRoute("guide/index.mdx")).toBe("/docs/guide/");
    expect(missingTranslations({ ...dirs, excludedPrefixes: [] })).toEqual([]);
    writeFileSync(join(dirs.japaneseDir, "guide.mdx"), "Duplicate");
    expect(() => discoverDocPages(dirs.japaneseDir)).toThrow("Duplicate documentation route");
  });

  it("fails visibly if the explicit policy becomes dynamic or ambiguous", () => {
    expect(() =>
      defaultLocaleOnlyPrefixes(
        "const x = { defaultLocaleOnlyPrefixes: [], nested: { defaultLocaleOnlyPrefixes: [] } };",
      ),
    ).toThrow();
    expect(() =>
      defaultLocaleOnlyPrefixes("const x = { defaultLocaleOnlyPrefixes: getPolicy() }; "),
    ).toThrow();
    expect(() =>
      defaultLocaleOnlyPrefixes("const x = { defaultLocaleOnlyPrefixes: [...other] }; "),
    ).toThrow();
  });
});
