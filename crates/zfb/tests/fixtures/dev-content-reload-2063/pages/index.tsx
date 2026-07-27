/**
 * Static home page — no collection reads. This fixture's boot-readiness
 * handshake (`GET /`) only needs a 200, not aggregate fan-out, so this
 * stays intentionally minimal (mirrors
 * `dev-content-aggregate-cold-boot/pages/index.tsx`, #1598).
 */
export default function HomePage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>dev-content-reload-2063 fixture</title>
      </head>
      <body>
        <a href="/posts/alpha">Alpha</a>
      </body>
    </html>
  );
}
