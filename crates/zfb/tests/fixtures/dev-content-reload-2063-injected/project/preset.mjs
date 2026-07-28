export default {
  name: "content-reload-2063-injected-preset",
  setup({ injectRoute }) {
    // Readiness-probe route. This fixture deliberately has NO `pages/`
    // directory at all (the #1518 zero-pages consumer shape, proven
    // bootable in `dev_serve_injected_routes_e2e.rs`), so the harness's
    // boot readiness probe needs an injected route to answer it.
    //
    // It MUST be a DYNAMIC pattern, never a static one (issue #2097,
    // Step 0). A static `injectRoute("/")` becomes a member of
    // `injected_static_seeds` (`crates/zfb/src/commands/package_routes.rs`
    // `static_injected_seeds`, which filters on `!is_dynamic_pattern`),
    // and `DevRenderSession::mark_injected_seeds_stale`
    // (`crates/zfb/src/commands/dev.rs`) runs UNCONDITIONALLY after every
    // successful P4 route-table swap — content-independently — pushing to
    // `tick_stale` via `mark_stale`. That drains into
    // `BuildOutcome::pages_stale` and vacuously satisfies
    // `outcome_to_events`'s Page gate on every full-refresh tick, so the
    // cell below would observe a `page` event that says NOTHING about the
    // dynamic injected channel it exists to test. Issue #2094's matrix
    // read exactly that vacuous pass as a real pass; see the correction
    // comment on #2094 and the decision on #2092.
    //
    // A dynamic route never enters `injected_static_seeds`, so with this
    // spelling that set is EMPTY for this project — the precondition the
    // cell needs. Its `paths()` deliberately does not read the `posts`
    // collection, so it stays genuinely inert with respect to the
    // content-edit assertion.
    injectRoute("/home/[slug]", "./pkg/home.tsx");
    // The cell under test (issue #2094, matrix cell (a) combined with
    // (c)'s empty-pages shape): a DYNAMIC injected route — the
    // zudo-doc-style "route-injecting consumer" the epic names as the
    // single most #2063-relevant shape — whose `paths()` reads a
    // collection with NO in-project `pages/` consumer at all, so the
    // dependency graph's known-page universe is empty for this project.
    injectRoute("/injected-posts/[slug]", "./pkg/injected-post.tsx");
  },
};
