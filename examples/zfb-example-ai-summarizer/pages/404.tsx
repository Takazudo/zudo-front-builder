import "../styles/global.css";

export default function NotFoundPage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>Not found · AI Summarizer</title>
      </head>
      <body>
        <main class="page-shell">
          <section class="not-found">
            <p class="eyebrow">404</p>
            <h1>Not found</h1>
            <a href="/">Return to summarizer</a>
          </section>
        </main>
      </body>
    </html>
  );
}
