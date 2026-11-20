import { getIndex } from "../lib/content";

export default function Home() {
  const { articles } = getIndex();
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>tiny-two-article</title>
      </head>
      <body>
        <h1 data-home-heading="">tiny-two-article</h1>
        <ul data-home-list="">
          {articles.map((article) => (
            <li key={article.slug} data-home-item={article.slug}>
              <a href={`/articles/${article.slug}`} data-home-title={article.slug}>
                {article.data.title}
              </a>{" "}
              <span data-home-category={article.slug}>{article.data.category}</span>
            </li>
          ))}
        </ul>
        <a href="/independent" id="independent-link">
          independent
        </a>
      </body>
    </html>
  );
}
