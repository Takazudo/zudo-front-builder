import { getIndex } from "../../src/article-index";

export function paths() {
  return getIndex().articles.map((e) => ({ params: { slug: e.slug } }));
}

export default function ArticlePage({ params }: { params: { slug: string } }) {
  const article = getIndex().bySlug.get(params.slug);
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>{article ? article.data.title : "missing"}</title>
      </head>
      <body>
        <h1>{article ? article.data.title : "missing"}</h1>
      </body>
    </html>
  );
}
