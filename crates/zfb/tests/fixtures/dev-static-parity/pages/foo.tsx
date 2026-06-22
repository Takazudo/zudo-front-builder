/**
 * Route /foo — used by the route-vs-static collision test (issue #1167).
 * A `public/foo` file also exists in this fixture; the page cache MUST win
 * over the public file in both dev and build (precedence: page > static).
 */
export default function FooPage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>Foo page</title>
      </head>
      <body>
        <h1>PARITY-FOO-ROUTE-MARKER</h1>
        <p>This is the rendered /foo route, not the public/foo file.</p>
      </body>
    </html>
  );
}
