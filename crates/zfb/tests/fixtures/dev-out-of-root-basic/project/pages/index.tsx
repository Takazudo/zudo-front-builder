/**
 * Static home page — deliberately does NOT read the `posts` collection, so
 * editing an out-of-root post can never legitimately stale/re-render it.
 * That keeps the narrow-re-render proof in the e2e test unambiguous: any
 * write to this route's disk output after a content edit would be a real
 * fan-out regression, not an artifact of this page depending on the data.
 */
export default function Index() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>dev-out-of-root-basic fixture</title>
      </head>
      <body>
        <h1>dev-out-of-root-basic</h1>
      </body>
    </html>
  );
}
