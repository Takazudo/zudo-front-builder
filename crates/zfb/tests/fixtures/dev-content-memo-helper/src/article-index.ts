import { getCollection, type CollectionEntry } from "@takazudo/zfb/content";

type ArticleData = { title: string };
type Article = CollectionEntry<ArticleData>;
type ArticleIndex = { articles: Article[]; bySlug: Map<string, Article> };

let cached: ArticleIndex | undefined;

export function getIndex(): ArticleIndex {
  if (cached) return cached;
  const articles = getCollection<ArticleData>("articles").sort((a, b) =>
    a.slug.localeCompare(b.slug),
  );
  cached = { articles, bySlug: new Map(articles.map((article) => [article.slug, article])) };
  return cached;
}
