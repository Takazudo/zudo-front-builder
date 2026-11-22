import { getArticles } from "../src/article-index";

export default function HomePage() {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>slug agreement home</title>
      </head>
      <body>
        <h1>SLUG-HOME</h1>
        <ul>
          {getArticles().map((article) => (
            <li key={article.slug}>{article.data.title}</li>
          ))}
        </ul>
      </body>
    </html>
  );
}
