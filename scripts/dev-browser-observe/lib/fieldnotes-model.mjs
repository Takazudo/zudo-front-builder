// Pure model of the Field Notes dataset and the app's index rules, used to
// derive expected page states. The rules mirror apps/zfb/lib/content.ts and
// apps/zfb/pages/* of the original benchmark app (sort by slug with "en"
// collation, categories in spec order, related = next 3 in the category,
// cyclic; home 12, archive 50 per page, category first 30).

export function createModel(manifest, spec) {
  return {
    spec: {
      categories: spec.categories.map((c) => ({ slug: c.slug, label: c.label })),
      homeSize: spec.homeSize,
      pageSize: spec.pageSize,
      categorySize: spec.categorySize,
    },
    articles: manifest.articles.map((a) => ({
      slug: a.slug,
      title: a.title,
      description: a.description,
      date: a.date,
      category: a.category,
      categoryLabel: a.categoryLabel,
      author: a.author,
      readingMinutes: a.readingMinutes,
    })),
  };
}

export function cloneModel(model) {
  return { spec: model.spec, articles: model.articles.map((a) => ({ ...a })) };
}

export function labelOf(model, category) {
  const c = model.spec.categories.find((x) => x.slug === category);
  if (!c) throw new Error(`unknown category ${category}`);
  return c.label;
}

export function computeIndex(model) {
  const articles = [...model.articles].sort((a, b) => a.slug.localeCompare(b.slug, "en"));
  const categories = new Map(model.spec.categories.map((c) => [c.slug, []]));
  for (const a of articles) {
    const list = categories.get(a.category);
    if (!list) throw new Error(`Unknown article category: ${a.category}`);
    list.push(a);
  }
  const related = new Map();
  for (const list of categories.values()) {
    list.forEach((a, position) => {
      const next = [];
      for (let offset = 1; offset <= Math.min(3, list.length - 1); offset++)
        next.push(list[(position + offset) % list.length]);
      related.set(a.slug, next);
    });
  }
  return { articles, categories, related, bySlug: new Map(articles.map((a) => [a.slug, a])) };
}

export const href = (slug) => `/articles/${slug}/`;

/** Where an article appears: home card?, archive page number, category position. */
export function placement(model, slug) {
  const idx = computeIndex(model);
  const i = idx.articles.findIndex((a) => a.slug === slug);
  if (i < 0) return null;
  const a = idx.articles[i];
  const catPos = idx.categories.get(a.category).findIndex((x) => x.slug === slug);
  return {
    onHome: i < model.spec.homeSize,
    archivePage: Math.floor(i / model.spec.pageSize) + 1,
    category: a.category,
    onCategoryPage: catPos >= 0 && catPos < model.spec.categorySize,
  };
}

/** Articles whose related list contains `slug`. */
export function referrersOf(model, slug) {
  const idx = computeIndex(model);
  return [...idx.related.entries()]
    .filter(([, list]) => list.some((a) => a.slug === slug))
    .map(([s]) => s);
}

export function relatedHrefs(model, slug) {
  return (computeIndex(model).related.get(slug) ?? []).map((a) => href(a.slug));
}

// Mutations return a new model; they never edit the input.
export function setTitle(model, slug, title) {
  const m = cloneModel(model);
  m.articles.find((a) => a.slug === slug).title = title;
  return m;
}

/** Set one plain frontmatter field the model carries (e.g. description). */
export function setField(model, slug, key, value) {
  const m = cloneModel(model);
  const a = m.articles.find((x) => x.slug === slug);
  if (!(key in a)) throw new Error(`model has no field ${key}`);
  a[key] = value;
  return m;
}

export function setCategory(model, slug, category) {
  const m = cloneModel(model);
  const a = m.articles.find((x) => x.slug === slug);
  a.category = category;
  a.categoryLabel = labelOf(model, category);
  return m;
}

export function addArticle(model, article) {
  const m = cloneModel(model);
  m.articles.push({ ...article });
  return m;
}

export function removeArticle(model, slug) {
  const m = cloneModel(model);
  m.articles = m.articles.filter((a) => a.slug !== slug);
  return m;
}

export function renameSlug(model, slug, to) {
  const m = cloneModel(model);
  m.articles.find((a) => a.slug === slug).slug = to;
  return m;
}

/** The generator's rule for article n (scripts/generate.mjs). */
export function generatedMetadata(model, n) {
  const authors = ["Mika Tanaka", "Alex Morgan", "Sam Rivera", "Robin Chen"];
  const category = model.spec.categories[(n - 1) % model.spec.categories.length];
  const date = new Date(Date.UTC(2026, 0, 1) + ((n - 1) % 300) * 86400000)
    .toISOString()
    .slice(0, 10);
  return {
    slug: `article-${String(n).padStart(5, "0")}`,
    date,
    category: category.slug,
    categoryLabel: category.label,
    author: authors[(n - 1) % authors.length],
    readingMinutes: 4,
  };
}

// ---- Markdown/frontmatter transforms (byte-level, one field at a time) ----

function fieldRe(key) {
  return new RegExp(`^${key}: .*$`, "m");
}

/** Replace one frontmatter field, JSON-quoting strings the way the generator does. */
export function setFrontmatter(text, key, value) {
  const re = fieldRe(key);
  if (!re.test(text)) throw new Error(`frontmatter has no ${key} field`);
  const v = typeof value === "number" ? String(value) : JSON.stringify(value);
  return text.replace(re, `${key}: ${v}`);
}

export function getFrontmatter(text, key) {
  const m = text.match(fieldRe(key));
  if (!m) return null;
  const raw = m[0].slice(key.length + 2);
  try {
    return JSON.parse(raw);
  } catch {
    return raw;
  }
}

/** Put `token` at the start of the first body paragraph (first non-heading, non-list line after the frontmatter). */
export function tokenInFirstParagraph(text, token) {
  const end = text.indexOf("\n---\n", 4);
  if (!text.startsWith("---\n") || end < 0) throw new Error("no frontmatter");
  const head = text.slice(0, end + 5);
  const lines = text.slice(end + 5).split("\n");
  const i = lines.findIndex((l) => l.trim() !== "" && !/^(#|-|\*|>|\d+\.|```)/.test(l.trim()));
  if (i < 0) throw new Error("no body paragraph");
  lines[i] = `${token} ${lines[i]}`;
  return head + lines.join("\n");
}

/** Break the YAML frontmatter (an unterminated quoted title). */
/**
 * Invalid-frontmatter breakages, by versioned id. The first is the original and stays the
 * default, so existing results keep their meaning. js-yaml 3.15.2 (gray-matter, the Next app)
 * accepts it as a multi-line quoted title; the second is an unclosed flow sequence that zfb's
 * noyalib 0.0.58, js-yaml 3.15.2 and yaml 2.9.1 all reject
 * (fixtures/field-notes/evidence/frontmatter-breakage-verification.json).
 */
export const FRONTMATTER_BREAKAGES = {
  "title-unbalanced-quote-v1": (line) => line.replace(/"$/, "") + "\n  : [unbalanced",
  "unclosed-flow-sequence-v1": () => "title: [unbalanced",
};
export const DEFAULT_FRONTMATTER_BREAKAGE = "title-unbalanced-quote-v1";

export function breakFrontmatter(text, breakage = DEFAULT_FRONTMATTER_BREAKAGE) {
  const re = fieldRe("title");
  if (!re.test(text)) throw new Error("frontmatter has no title field");
  const f = FRONTMATTER_BREAKAGES[breakage];
  if (!f) throw new Error(`unknown frontmatter breakage ${breakage}`);
  return text.replace(re, (line) => f(line));
}

export function newArticleText(meta, token) {
  const fm = [
    "---",
    `slug: ${JSON.stringify(meta.slug)}`,
    `title: ${JSON.stringify(meta.title)}`,
    `description: ${JSON.stringify(meta.description)}`,
    `date: ${JSON.stringify(meta.date)}`,
    `category: ${JSON.stringify(meta.category)}`,
    `categoryLabel: ${JSON.stringify(meta.categoryLabel)}`,
    `author: ${JSON.stringify(meta.author)}`,
    `readingMinutes: ${meta.readingMinutes}`,
    "---",
    "",
    "## Added during observation",
    "",
    `${token} This field note was created by dev-browser-observe to check that a new collection member is discovered.`,
    "",
  ];
  return fm.join("\n");
}
