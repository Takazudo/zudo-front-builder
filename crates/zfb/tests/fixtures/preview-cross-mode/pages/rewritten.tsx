// Target of the 200-rewrite `_redirects` rule (see public/_redirects). A
// real prerendered page, not a raw static asset — proves a rewrite target
// resolves through the same page-serving waterfall a direct request would.
export default function RewrittenPage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>Rewritten</title>
      </head>
      <body>
        <h1>CROSS_MODE_REWRITTEN_MARKER</h1>
      </body>
    </html>
  );
}
