export default {
  name: "content-reload-2063-injected-preset",
  setup({ injectRoute }) {
    // Static injected root route. This fixture deliberately has NO
    // `pages/` directory at all (the #1518 zero-pages consumer shape,
    // proven bootable in `dev_serve_injected_routes_e2e.rs`), so the
    // harness's `GET /` readiness probe needs an injected route to
    // answer it. This route never reads the `posts` collection, so it
    // stays inert with respect to the content-edit assertion below.
    injectRoute("/", "./pkg/home.tsx");
    // The cell under test (issue #2094, matrix cell (a) combined with
    // (c)'s empty-pages shape): a DYNAMIC injected route — the
    // zudo-doc-style "route-injecting consumer" the epic names as the
    // single most #2063-relevant shape — whose `paths()` reads a
    // collection with NO in-project `pages/` consumer at all, so the
    // dependency graph's known-page universe is empty for this project.
    injectRoute("/injected-posts/[slug]", "./pkg/injected-post.tsx");
  },
};
