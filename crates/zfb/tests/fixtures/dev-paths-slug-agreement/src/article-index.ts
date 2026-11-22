import { getCollection, type CollectionEntry } from "@takazudo/zfb/content";

type ArticleData = { title: string; slug: string };
type Article = CollectionEntry<ArticleData>;

export function getArticles(): Article[] {
  const articles = getCollection<ArticleData>("articles").sort((a, b) =>
    a.slug.localeCompare(b.slug),
  );
  for (const article of articles) {
    if (article.data.slug !== article.slug) {
      throw new Error(`Article slug mismatch: ${article.slug}`);
    }
  }
  return articles;
}
