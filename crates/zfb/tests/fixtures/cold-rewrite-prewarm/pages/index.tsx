// The unrelated, repeatedly-edited page used to prove genuine watcher
// ticks occur during the test (see `cold_rewrite_prewarm_e2e.rs`'s header
// comment). Never the `_redirects` rewrite target — that is
// `pages/target.tsx`.
export default function HomePage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>cold-rewrite-prewarm fixture</title>
      </head>
      <body>
        <h1>COLD_REWRITE_PREWARM_HOME_MARKER_V0</h1>
      </body>
    </html>
  );
}
