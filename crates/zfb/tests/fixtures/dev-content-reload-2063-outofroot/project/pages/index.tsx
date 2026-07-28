/**
 * Static home page — deliberately does NOT read the `posts` collection
 * (mirrors `dev-out-of-root-basic/project/pages/index.tsx`, #1552). Only
 * used here as the harness's `GET /` readiness probe; the exactly-one-
 * `page`-event assertion below targets `/posts/alpha`, not this route.
 */
export default function Index() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>dev-content-reload-2063-outofroot fixture</title>
      </head>
      <body>
        <h1>dev-content-reload-2063-outofroot</h1>
      </body>
    </html>
  );
}
