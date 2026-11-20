import { getIndex } from "../../lib/content";

export function paths() {
  return getIndex().articles.map((entry) => ({ params: { slug: entry.slug } }));
}

export default function ArticlePage({ params }: { params: { slug: string } }) {
  const slug = params.slug;
  const article = getIndex().bySlug.get(slug);
  if (!article) {
    return (
      <html lang="en">
        <body>
          <p data-article-missing={slug}>missing</p>
        </body>
      </html>
    );
  }
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>{article.data.title}</title>
      </head>
      <body>
        <h1 data-article-title={slug}>{article.data.title}</h1>
        <p data-article-category={slug}>{article.data.category}</p>
        <div data-article-body={slug}>
          <article.Content components={{}} />
        </div>
        <a href="/" id="home-link">
          home
        </a>
      </body>
    </html>
  );
}
