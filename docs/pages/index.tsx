/** @jsxRuntime automatic */
/** @jsxImportSource preact */
// Default-locale (EN) site index — the ONE route the package does not inject.
//
// zudo-doc packageOwnedRoutes injects /[locale] but not the root "/", so
// the host must own this page. We use the package's default home verbatim: it
// rebuilds the same hero (logo, siteName, description, SiteTreeNav grid, tags)
// from `settings` via the virtual:zudo-doc-route-context payload the routes
// plugin emits at build. See @takazudo/zudo-doc/routes/index.
export { default, frontmatter } from "@takazudo/zudo-doc/routes/index";
