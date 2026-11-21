import { getIndex } from "../src/article-index";

export default function HomePage() {
  const { articles } = getIndex();
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>memo helper home</title>
      </head>
      <body>
        <h1>MEMO-HOME</h1>
        <ul>
          {articles.map((article) => (
            <li key={article.slug}>{article.data.title}</li>
          ))}
        </ul>
      </body>
    </html>
  );
}
