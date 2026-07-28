export default {
  name: "content-reload-2063-injected-preset",
  setup({ injectRoute }) {
    // EXACTLY ONE injected route, and it is DYNAMIC. Both properties are
    // load-bearing for issue #2097's cell; do not add a second route here
    // for any reason, including "the harness needs something to probe".
    //
    // Why nothing STATIC: a static `injectRoute("/")` becomes a member of
    // `injected_static_seeds` (`crates/zfb/src/commands/package_routes.rs`
    // `static_injected_seeds`, which filters on `!is_dynamic_pattern`), and
    // `DevRenderSession::mark_injected_seeds_stale`
    // (`crates/zfb/src/commands/dev.rs`) runs UNCONDITIONALLY after every
    // successful P4 route-table swap — content-independently — pushing to
    // `tick_stale` via `mark_stale`. That drains into
    // `BuildOutcome::pages_stale` and vacuously satisfies
    // `outcome_to_events`'s Page gate on EVERY full-refresh tick, so the
    // cell would observe a `page` event that says nothing whatsoever about
    // the dynamic injected channel it exists to test. This fixture
    // previously carried exactly such a route as a `GET /` readiness probe,
    // and issue #2094's matrix read the resulting vacuous pass as a real
    // pass. See the correction comment on #2094 and the decision on #2092.
    //
    // Why nothing else DYNAMIC either: any second injected route, however
    // inert its `paths()`, joins `stale.dynamic_injected` the moment it is
    // requested — so a post-fix `page` event would no longer be
    // attributable to the route under test alone. The readiness probe is
    // therefore the route under test itself (`GET /injected-posts/alpha`),
    // which needs no extra route at all.
    //
    // The project has NO `pages/` directory (the #1518 zero-pages consumer
    // shape, proven bootable in `dev_serve_injected_routes_e2e.rs`), so this
    // one route is the entire route universe: `routes_by_source` never
    // carries an entry for the `posts` collection. That is exactly the
    // #2063 shape — a zudo-doc-style route-injecting consumer whose MDX
    // route universe comes from a dynamic injected route rather than from
    // the project's own `pages/`.
    injectRoute("/injected-posts/[slug]", "./pkg/injected-post.tsx");
  },
};
