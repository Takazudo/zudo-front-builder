/**
 * DYNAMIC injected readiness-probe route at `/home/[slug]`, used only as
 * this fixture's boot readiness target (see `preset.mjs`'s comment) —
 * this project has no `pages/` directory, so nothing else answers a GET.
 *
 * Deliberately dynamic, not static: a static injected route would land in
 * `injected_static_seeds` and make `mark_injected_seeds_stale` push to
 * `tick_stale` on every route-table swap, vacuously satisfying the Page
 * gate this fixture's real cell is trying to observe (issue #2097 Step 0).
 *
 * Its `paths()` returns one fixed slug and never touches the `posts`
 * collection, so editing `shared-content/posts/alpha.mdx` cannot reach it.
 */
export async function paths() {
  return [{ params: { slug: "ok" } }];
}

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
