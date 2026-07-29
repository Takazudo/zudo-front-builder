//! Shared `_`-prefix privacy convention for `pages/`-relative paths.
//!
//! ## Why this lives in `zfb-types`
//!
//! Both the router (`zfb-router`'s `scan_pages`) and the bundler
//! (`zfb-build`'s `derive_route`) treat a leading `_` on a `pages/`-relative
//! path segment as marking a file (or an entire directory of files)
//! "private" — a framework/author convention mirroring Next.js / Astro /
//! Remix, used for internal helpers that must never become a route. Before
//! this module the two layers implemented the rule independently and had
//! drifted (issue #2123 / epic #2145, sub #2148): the router checked BOTH
//! the file stem and every ancestor directory component, while the
//! bundler's `derive_route` checked only the file name. A `prerender:
//! false` page nested under a private directory (e.g.
//! `pages/_components/api.tsx`) was therefore invisible to the
//! router-driven SSG scan yet still reached the bundler's own
//! independently-derived route list — which feeds the compiled
//! `entry.mjs` route table used for SSR/worker dispatch — making it
//! live-servable despite looking private everywhere else.
//!
//! ## Semantics
//!
//! [`path_has_private_prefix_component`] accepts a route-relative path
//! (i.e. relative to `pages_dir`) and returns `true` when:
//!
//! - any **ancestor directory** component's name starts with `_`
//!   (`_components/foo.tsx`, `a/_b/c.tsx`), OR
//! - the file's **stem** (extension stripped) starts with `_`
//!   (`_private.tsx`).
//!
//! The extension itself is never consulted: `foo._bar.tsx`'s stem is
//! `foo._bar` (only the last `.`-suffix is stripped), which does not start
//! with `_`, so it is NOT private.
//!
//! Purely path-based — no filesystem access — mirroring
//! [`crate::is_client_script_file`] / [`crate::is_page_sidecar_file`], the
//! sibling filename contracts this module is modelled on.
//!
//! ## Non-UTF-8 paths are the caller's decision, not this function's
//!
//! The two existing call sites intentionally disagree about what to do
//! when a path segment cannot be decoded as UTF-8: `zfb-router`'s
//! `scan_pages` SKIPS the file entirely via its own pre-check (run before
//! this predicate is ever reached), while `zfb-build`'s `derive_route`
//! falls through to a lossy conversion and keeps treating the file as a
//! page. This function does not referee that disagreement — it answers
//! only the `_`-prefix question, treating an undecodable component as
//! "not private" (the same best-effort default `derive_route` already
//! relied on via `.and_then(|s| s.to_str())...unwrap_or(false)`), and each
//! caller keeps whatever pre-check or fallback its own non-UTF-8 posture
//! needs.
use std::path::{Component, Path};

/// Returns `true` when `path` (relative to `pages_dir`) is private under the
/// `_`-prefix convention — either an ancestor directory component or the
/// file's own stem starts with `_`. See the module docs for the exact
/// semantics and the non-UTF-8 handling contract.
pub fn path_has_private_prefix_component(path: &Path) -> bool {
    let ancestor_private = path
        .parent()
        .into_iter()
        .flat_map(std::path::Path::components)
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .any(|name| name.starts_with('_'));
    if ancestor_private {
        return true;
    }

    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.starts_with('_'))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stem_prefixed_file_is_private() {
        assert!(path_has_private_prefix_component(Path::new("_private.tsx")));
        assert!(path_has_private_prefix_component(Path::new("_app.tsx")));
    }

    #[test]
    fn ancestor_directory_prefixed_is_private() {
        assert!(path_has_private_prefix_component(Path::new(
            "_components/foo.tsx"
        )));
        // Nested deeper than one level — every ancestor is checked, not just
        // the immediate parent.
        assert!(path_has_private_prefix_component(Path::new(
            "a/b/_c/d.tsx"
        )));
        // The private component need not be the LAST ancestor either.
        assert!(path_has_private_prefix_component(Path::new(
            "_lib/nested/leaf.tsx"
        )));
    }

    #[test]
    fn extension_is_not_part_of_the_stem_check() {
        // Stem is `sitemap.xml` (only the last `.`-suffix is stripped) —
        // does not start with `_`, so this must NOT be private.
        assert!(!path_has_private_prefix_component(Path::new(
            "sitemap.xml.tsx"
        )));
        assert!(!path_has_private_prefix_component(Path::new(
            "foo._bar.tsx"
        )));
    }

    #[test]
    fn positive_controls_are_not_private() {
        assert!(!path_has_private_prefix_component(Path::new("index.tsx")));
        assert!(!path_has_private_prefix_component(Path::new("about.tsx")));
        assert!(!path_has_private_prefix_component(Path::new(
            "blog/[slug].tsx"
        )));
        assert!(!path_has_private_prefix_component(Path::new(
            "a/b/c.tsx"
        )));
    }

    #[test]
    fn no_extension_file_stem_is_whole_name() {
        // `file_stem()` on an extension-less name returns the whole name.
        assert!(path_has_private_prefix_component(Path::new("_private")));
        assert!(!path_has_private_prefix_component(Path::new("readme")));
    }
}
