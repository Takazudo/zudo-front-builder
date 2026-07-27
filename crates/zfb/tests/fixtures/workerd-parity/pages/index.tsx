// Trivial SSG page so `zfb build` has a static route to prerender
// alongside the `prerender = false` API routes below (issue #2020,
// V8 Request Time Parity epic #2012, Wave 8 — workerd/wrangler parity).
// Its content is never inspected by the parity test.
export default function IndexPage() {
  return (
    <html lang="en">
      <head>
        <title>workerd parity fixture</title>
      </head>
      <body>index</body>
    </html>
  );
}
