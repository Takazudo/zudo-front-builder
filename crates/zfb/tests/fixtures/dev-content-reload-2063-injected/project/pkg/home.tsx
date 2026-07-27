/**
 * Static injected root route, used only as this fixture's `GET /`
 * readiness probe target (see `preset.mjs`'s comment) — this project has
 * no `pages/` directory, so nothing else answers `/`.
 */
export default function InjectedHome() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>dev-content-reload-2063-injected fixture</title>
      </head>
      <body>
        <h1>INJECTED_ROOT_OK</h1>
      </body>
    </html>
  );
}
