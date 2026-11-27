import assert from "node:assert/strict";
import { isDeepStrictEqual } from "node:util";
import { computeIndex, href, relatedHrefs, setCategory } from "./fieldnotes-model.mjs";
import { astroTextPredicate } from "./astro-text.mjs";

export const CATEGORY_ARTICLE = "article-00015";
export const CATEGORY_DETAIL = href(CATEGORY_ARTICLE);
const relatedArticles = [15, 5, 85, 95, 6, 86, 96].map(
  (i) => "article-" + String(i).padStart(5, "0"),
);
const relatedSelector = "section.related .post-card h3 a";
const cards = ".card-grid .post-card h3 a";
const card = cards + '[href="' + CATEGORY_DETAIL + '"]';
const meta = '.post-card:has(h3 a[href="' + CATEGORY_DETAIL + '"]) .card-meta a';
const list = (selector, values) => ({
  kind: "attribute-list",
  selector,
  name: "href",
  equals: values,
});
const count = (selector, equals) => ({ kind: "count", selector, equals });
const text = (selector, equals) => ({ kind: "text", selector, equals });
const all = (...of) => ({ kind: "all", of });
const detail = (title, label) =>
  all(
    astroTextPredicate(title, ".article-header h1"),
    astroTextPredicate(label, ".article-header .eyebrow"),
    count('[data-article-body="article-00015"]', 1),
  );

export function astroPredicateValue(p) {
  return p.kind === "all" ? p.of.map(astroPredicateValue) : p.equals;
}
export function matchesAstroPredicate(actual, p) {
  return !!p && isDeepStrictEqual(actual, astroPredicateValue(p));
}

/** Build both oracles from the pinned original articles. Preserve model order and repeated target paths. */
export function fixedCategoryContract(model) {
  const index = computeIndex(model),
    article = index.bySlug.get(CATEGORY_ARTICLE);
  assert.equal(index.articles.length, 100);
  assert.equal(article.category, "performance");
  assert.equal(article.categoryLabel, "Performance");
  assert.equal(article.title, "Finding useful signals in build output · 00015");
  const edited = setCategory(model, CATEGORY_ARTICLE, "testing"),
    next = computeIndex(edited);
  const selected = [
    { path: "/category/performance/", predicate: count(card, 0) },
    { path: "/category/testing/", predicate: count(card, 1) },
    { path: "/category/testing/", predicate: text(meta, "Testing") },
    ...relatedArticles.map((s) => ({
      path: href(s),
      predicate: list(relatedSelector, relatedHrefs(edited, s)),
    })),
  ];
  function group(idx, slug, originalPredicate) {
    const members = idx.categories
      .get(slug)
      .slice(0, model.spec.categorySize)
      .map((a) => href(a.slug));
    assert(members.length > 0);
    return all(
      originalPredicate,
      astroTextPredicate(slug === "testing" ? "Testing" : "Performance", ".listing-header h1"),
      list(cards, members),
    );
  }
  const supplemental = selected.map((q, i) =>
    i >= 3
      ? q
      : {
          ...q,
          predicate: group(
            next,
            i === 0 ? "performance" : "testing",
            i === 2
              ? all(astroTextPredicate("Testing", meta), {
                  kind: "attribute",
                  selector: meta,
                  name: "href",
                  equals: "/category/testing/",
                })
              : q.predicate,
          ),
        },
  );
  const restore = relatedArticles.map((s) => list(relatedSelector, relatedHrefs(model, s)));
  assert.notDeepEqual(selected.at(-1).predicate, restore.at(-1));
  return {
    original: detail(article.title, "Performance"),
    edited: detail(article.title, "Testing"),
    selected,
    supplemental,
    finalPath: href(relatedArticles.at(-1)),
    restore: [
      group(index, "performance", count(card, 1)),
      group(index, "testing", count(card, 0)),
      group(index, "testing", count(card, 0)),
      ...restore,
    ],
  };
}
