import { mkdirSync, writeFileSync } from "node:fs";
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
