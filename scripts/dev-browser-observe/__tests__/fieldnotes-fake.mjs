import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { createHash } from "node:crypto";

export const SPEC = {
  siteName: "Field Notes",
  pageSize: 50,
  homeSize: 12,
  categorySize: 30,
  categories: [
    { slug: "architecture", label: "Architecture" },
    { slug: "content", label: "Content" },
    { slug: "design", label: "Design" },
    { slug: "tooling", label: "Tooling" },
    { slug: "performance", label: "Performance" },
    { slug: "testing", label: "Testing" },
    { slug: "accessibility", label: "Accessibility" },
    { slug: "platforms", label: "Platforms" },
    { slug: "workflow", label: "Workflow" },
    { slug: "notes", label: "Notes" },
  ],
};

export function articleRow(n) {
  const c = SPEC.categories[(n - 1) % SPEC.categories.length];
  const slug = `article-${String(n).padStart(5, "0")}`;
  return {
    slug,
    title: `Title ${n} · ${String(n).padStart(5, "0")}`,
    description: `Field note ${n}.`,
    date: new Date(Date.UTC(2026, 0, 1) + ((n - 1) % 300) * 86400000).toISOString().slice(0, 10),
    category: c.slug,
    categoryLabel: c.label,
    author: ["Mika Tanaka", "Alex Morgan", "Sam Rivera", "Robin Chen"][(n - 1) % 4],
    readingMinutes: 4,
  };
}

export function articleText(a) {
  return [
    "---",
    `slug: ${JSON.stringify(a.slug)}`,
    `title: ${JSON.stringify(a.title)}`,
    `description: ${JSON.stringify(a.description)}`,
    `date: ${JSON.stringify(a.date)}`,
    `category: ${JSON.stringify(a.category)}`,
    `categoryLabel: ${JSON.stringify(a.categoryLabel)}`,
    `author: ${JSON.stringify(a.author)}`,
    `readingMinutes: ${a.readingMinutes}`,
    "---",
    "",
    "## Start with a clear question",
    "",
    `Body of ${a.slug} first paragraph.`,
    "",
    "- a list item",
    "",
  ].join("\n");
}

/** A small Field Notes source dir with the original layout (apps/zfb, content, shared). */
export function makeFakeSource(root, count = 20) {
  const app = join(root, "apps", "zfb");
  for (const d of ["components", "lib", "pages/articles", "public"])
    mkdirSync(join(app, d), { recursive: true });
  mkdirSync(join(root, "content", "articles"), { recursive: true });
  mkdirSync(join(root, "shared"), { recursive: true });
  writeFileSync(
    join(app, "components", "Layout.tsx"),
    "<footer><p>Same content. Same design. Three build systems.</p></footer>\n",
  );
  writeFileSync(join(app, "lib", "spec.json"), JSON.stringify(SPEC));
  writeFileSync(join(app, "lib", "content.ts"), "// source helper (replaced by the adapter)\n");
  writeFileSync(join(app, "public", "site.css"), ":root{color-scheme:light;--accent:#df653a}\n");
  writeFileSync(join(app, "zfb.config.ts"), "export default {};\n");
  writeFileSync(join(root, "shared", "site.css"), "/* shared */\n");
  const rows = [];
  const h = createHash("sha256");
  for (let n = 1; n <= count; n++) {
    const a = articleRow(n);
    const text = articleText(a);
    writeFileSync(join(root, "content", "articles", `${a.slug}.md`), text);
    h.update(a.slug).update("\0").update(text);
    rows.push(a);
  }
  writeFileSync(
    join(root, "content", "manifest.json"),
    JSON.stringify({ articleCount: count, articles: rows }),
  );
  return { datasetSha256: h.digest("hex"), rows };
}

/**
 * A fake archive for make-source: the fake source layout without content/, plus a tiny
 * deterministic scripts/generate.mjs; the returned scales declare that generator's sha256.
 */
export async function makeFakeArchive(root, fixtureDir) {
  const { loadFieldNotesFixture, loadFieldNotesScales } = await import("../lib/fieldnotes.mjs");
  const { sha256File } = await import("../lib/util.mjs");
  const archive = join(root, "archive");
  makeFakeSource(archive, 40);
  rmSync(join(archive, "content"), { recursive: true });
  mkdirSync(join(archive, "scripts"));
  const gen = [
    'import fs from "node:fs";',
    "const n = Number(process.argv[2]);",
    'fs.mkdirSync("content/articles", { recursive: true });',
    "const rows = [];",
    "for (let i = 1; i <= n; i++) {",
    '  const slug = `article-${String(i).padStart(5, "0")}`;',
    "  fs.writeFileSync(`content/articles/${slug}.md`, `# ${slug}\\n`);",
    "  rows.push({ slug });",
    "}",
    'fs.writeFileSync("content/manifest.json", JSON.stringify({ articleCount: n, articles: rows }));',
    "console.log(JSON.stringify({ articleCount: n }));",
    "",
  ].join("\n");
  writeFileSync(join(archive, "scripts", "generate.mjs"), gen);
  const manifest = loadFieldNotesFixture(fixtureDir);
  const scales = structuredClone(loadFieldNotesScales(fixtureDir, manifest));
  scales.generator.sha256 = sha256File(join(archive, "scripts", "generate.mjs"));
  return { root, archive, manifest, scales };
}

/**
 * A minimal harness dir to freeze (cli.mjs, observer, one lib file, one fixture) and a fake
 * @playwright/test package dir, so frozen-copy tests need not copy and hash the real harness.
 */
export function makeTinyHarness(root) {
  const harness = join(root, "harness");
  for (const d of ["lib", "fixtures/f"]) mkdirSync(join(harness, d), { recursive: true });
  writeFileSync(join(harness, "cli.mjs"), "// cli\n");
  writeFileSync(join(harness, "observer-init.js"), "// observer\n");
  writeFileSync(join(harness, "lib", "a.mjs"), "export const a = 1;\n");
  writeFileSync(join(harness, "fixtures", "f", "x.txt"), "x\n");
  const playwright = join(root, "pw");
  mkdirSync(playwright);
  writeFileSync(join(playwright, "package.json"), JSON.stringify({ version: "0.0.0-fake" }));
  return { harness, playwright };
}
