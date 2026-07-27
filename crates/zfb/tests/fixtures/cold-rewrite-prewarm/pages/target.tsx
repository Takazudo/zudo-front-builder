// The `_redirects` 200-rewrite target (see public/_redirects: `/alias
// /target 200`). This test's whole premise is that this route is NEVER
// requested directly and NEVER rendered by any other means, so its
// content never matters — only whether `/alias` ever becomes servable
// through the rewrite.
export default function TargetPage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>Target</title>
      </head>
      <body>
        <h1>COLD_REWRITE_PREWARM_TARGET_MARKER</h1>
      </body>
    </html>
  );
}
