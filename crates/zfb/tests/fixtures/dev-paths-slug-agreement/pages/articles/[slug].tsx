import { getArticles } from "../../src/article-index";

export function paths() {
  return getArticles().map((article) => ({ params: { slug: article.slug } }));
}

export default function ArticlePage({ params }: { params: { slug: string } }) {
  const article = getArticles().find((entry) => entry.slug === params.slug);
  const title = article ? article.data.title : "missing";
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>{title}</title>
      </head>
      <body>
        <h1>{title}</h1>
      </body>
    </html>
  );
}
