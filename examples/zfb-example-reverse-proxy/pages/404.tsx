import "../styles/global.css";

export default function NotFoundPage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>Not found</title>
      </head>
      <body>
        <main class="shell">
          <header class="page-header">
            <p class="eyebrow">404</p>
            <h1>Route not found</h1>
            <p>The reverse proxy example serves proxied URLs below /proxy/.</p>
          </header>
          <p>
            <a href="/">Back to checks</a>
          </p>
        </main>
      </body>
    </html>
  );
}
