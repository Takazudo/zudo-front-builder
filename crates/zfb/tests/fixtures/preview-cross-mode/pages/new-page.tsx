// Target of the plain-redirect and splat `_redirects` rules (see
// public/_redirects).
export default function NewPage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>New page</title>
      </head>
      <body>
        <h1>CROSS_MODE_NEW_PAGE_MARKER</h1>
      </body>
    </html>
  );
}
