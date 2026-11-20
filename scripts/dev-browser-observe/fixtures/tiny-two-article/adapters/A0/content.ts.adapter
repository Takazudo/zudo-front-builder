// A0 is RECONSTRUCTED for dev-browser-observe, not the original helper: the
// original getIndex() memoized its result in a module-level `cached`; A0 keeps
// that exact shape but rebuilds the index from getCollection on every call.
import { getCollection, type CollectionEntry } from "@takazudo/zfb/content";

export type ArticleData = { title: string; category: string };
export type Article = CollectionEntry<ArticleData>;
export type ArticleIndex = { articles: Article[]; bySlug: Map<string, Article> };

export function getIndex(): ArticleIndex {
  const articles = getCollection<ArticleData>("articles").sort((a, b) =>
    a.slug.localeCompare(b.slug),
  );
  return { articles, bySlug: new Map(articles.map((article) => [article.slug, article])) };
}
