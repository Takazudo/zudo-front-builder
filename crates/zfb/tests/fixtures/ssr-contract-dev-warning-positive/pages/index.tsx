// Healthy sibling route (#2357): proves the boot-time SSR route-contract
// warning for `pages/api/broken.tsx` does not degrade the dev server —
// this static page must keep serving normally alongside it.
export default function HomePage() {
  return (
    <html lang="en">
      <head>
        <title>ssr contract dev warning fixture</title>
      </head>
      <body>POSITIVE_FIXTURE_HOME_OK</body>
    </html>
  );
}
