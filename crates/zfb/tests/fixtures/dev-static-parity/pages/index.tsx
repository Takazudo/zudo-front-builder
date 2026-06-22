/**
 * Home page for the dev-static-parity fixture.
 * Used by dev↔build static-path parity tests (issue #1167):
 * - /  resolves here (rendered HTML contains PARITY-INDEX-MARKER)
 * - /foo is a route that wins over public/foo in collision tests
 */
export default function HomePage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>dev-static-parity fixture</title>
      </head>
      <body>
        <h1>PARITY-INDEX-MARKER</h1>
        <p>Static parity test home page</p>
      </body>
    </html>
  );
}
