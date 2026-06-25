//! Dev-injected-route registry and pattern matching (#255, #1228).
//!
//! Plugins call `injectRoute(pattern, entrypoint)` from the `setup`
//! hook to register a synthetic page route. The build crate owns the
//! canonical [`zfb_build::InjectedRoute`] /
//! [`zfb_build::InjectedRouteList`] types; `zfb-server` depends on
//! `zfb-build` (see `Cargo.toml`) and uses them directly.
//!
//! ## Shipped behaviour (epic #1228)
//!
//! `zfb dev` fully renders package-owned injected routes:
//!
//! - **Static injected routes** (URL == pattern, e.g. `/preset-about`)
//!   are seeded into `DevRouteTables.url_index` at boot and on every
//!   route-table swap (`note_table_swap`). `lookup_by_url` hits them
//!   exactly like a normal static page; the lazy adapter renders them
//!   through `render_one` and writes the result into `html_root`.
//!
//! - **Dynamic injected routes** (e.g. `/preset-docs/[slug]`) have no
//!   concrete URL at boot. On a `url_index` miss, `lazy_render_adapter`
//!   calls [`InjectedRouteSet::find_match`] to check whether an injected
//!   pattern matches the request URL. On a hit it synthesizes a
//!   `RouteUniverseEntry` on the fly (concrete `url_path`, `route_key`
//!   = the pattern, `static_html = false`, `source_path = None`) and
//!   runs it through the unchanged `render_one` → guarded-write →
//!   `html_root` flow. Params are extracted by the Hono router inside
//!   the live bundle; no Rust-side `paths()` enumeration is needed.
//!
//! - **Precedence:** user `pages/` always wins over any injected route
//!   of the same shape (enforced at staging time by reusing
//!   `package_routes::resolve_build_pages_root`'s survivor-selection).
//!   The post-precedence survivor set backs both the `url_index` seeds
//!   (static) and the `InjectedRouteSet` consulted at request time
//!   (dynamic), so the two views never disagree.
//!
//! - **HMR:** content the route reads from watched collection roots
//!   live-refreshes through the existing `with_external_invalidation`
//!   seam. The injected entrypoint itself lives under `node_modules`
//!   (not in `DEFAULT_WATCH_ROOTS`) and is restart-only — editing the
//!   package source requires a `zfb dev` restart.
//!
//! This module's public API (`InjectedRouteSet`, `pattern_matches`) is
//! the lookup layer. The staging, seeding, and render wiring live in
//! `crates/zfb/src/commands/dev.rs` and
//! `crates/zfb/src/lazy_render_adapter.rs`.

use std::sync::Arc;
use zfb_build::InjectedRoute;

/// Bundle of injected-route records passed into the dev server.
/// Cloned cheaply (the underlying `Arc<Vec<_>>` is shared).
#[derive(Clone, Default)]
pub struct InjectedRouteSet {
    pub records: Arc<Vec<InjectedRoute>>,
}

impl InjectedRouteSet {
    pub fn new(records: Vec<InjectedRoute>) -> Self {
        Self {
            records: Arc::new(records),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &InjectedRoute> {
        self.records.iter()
    }

    /// Look up the first injected route whose pattern matches
    /// `url_path`. The pattern grammar matches the `pages/`-style
    /// router: a bracketed segment (`[slug]`, `[...rest]`) is a
    /// wildcard, an unbracketed segment is a literal. This is a
    /// linear scan — registration counts are tiny (single-digit per
    /// project in practice) so the simple shape wins.
    pub fn find_match(&self, url_path: &str) -> Option<&InjectedRoute> {
        self.records
            .iter()
            .find(|rec| pattern_matches(&rec.pattern, url_path))
    }
}

/// `true` iff `url_path` matches `pattern`.
///
/// Segment grammar:
///
/// - `foo` matches the literal segment `foo` only.
/// - `[name]` matches exactly one segment (cannot be empty).
/// - `[...rest]` matches one or more segments (catch-all).
/// - `[[...rest]]` matches zero or more segments (optional catch-all).
///   The zero case matches the bare prefix (`/docs` for
///   `/docs/[[...rest]]`) but NOT the trailing-slash form (`/docs/`),
///   mirroring Hono's `:rest{.+}?` behaviour.
///
/// Mirrors the subset of `pages/`-router semantics the spec
/// explicitly calls out (`/blog/[slug]`); full feature parity with
/// `zfb_router::scan` is not required at this layer (the renderer
/// hook will do its own walk).
pub fn pattern_matches(pattern: &str, url_path: &str) -> bool {
    let p_segments: Vec<&str> = pattern.trim_start_matches('/').split('/').collect();
    let u_segments: Vec<&str> = url_path.trim_start_matches('/').split('/').collect();
    let mut pi = 0usize;
    let mut ui = 0usize;
    while pi < p_segments.len() {
        let p = p_segments[pi];
        // Optional catch-all `[[...rest]]` — checked before the
        // single-bracket forms so the doubled brackets are not
        // mis-read as a param literally named `[...rest]`.
        if p.starts_with("[[...") && p.ends_with("]]") {
            if pi != p_segments.len() - 1 {
                return false;
            }
            let remaining = &u_segments[ui..];
            if remaining.iter().any(|s| !s.is_empty()) {
                // One or more segments: same as the required catch-all.
                return true;
            }
            // Zero segments: only the bare prefix (no trailing slash)
            // or the root URL itself. `/docs/` leaves an empty trailing
            // segment which Hono's `:rest{.+}?` also rejects; `/` is
            // special-cased because the root URL always splits to one
            // empty segment.
            return remaining.is_empty() || url_path == "/";
        }
        if let Some(name) = strip_brackets(p) {
            if let Some(rest) = name.strip_prefix("...") {
                // Catch-all `[...rest]` — must be the last pattern
                // segment by convention. "One or more segments" means
                // at least one non-empty remaining url segment; a
                // trailing slash leaves an empty segment that does
                // NOT qualify.
                let _ = rest;
                if pi != p_segments.len() - 1 {
                    return false;
                }
                let remaining: Vec<&str> = u_segments[ui..]
                    .iter()
                    .copied()
                    .filter(|s| !s.is_empty())
                    .collect();
                return !remaining.is_empty();
            }
            // Single-segment param. Must match exactly one
            // non-empty url segment.
            if ui >= u_segments.len() || u_segments[ui].is_empty() {
                return false;
            }
            ui += 1;
            pi += 1;
            continue;
        }
        // Literal segment.
        if ui >= u_segments.len() || u_segments[ui] != p {
            return false;
        }
        ui += 1;
        pi += 1;
    }
    ui == u_segments.len()
}

fn strip_brackets(seg: &str) -> Option<&str> {
    seg.strip_prefix('[').and_then(|s| s.strip_suffix(']'))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn rec(pattern: &str) -> InjectedRoute {
        InjectedRoute {
            pattern: pattern.into(),
            entrypoint: PathBuf::from("/tmp/x.ts"),
            plugin: "p".into(),
            prerender: None,
        }
    }

    #[test]
    fn literal_pattern_matches_exact_url() {
        let s = InjectedRouteSet::new(vec![rec("/api/dev/x")]);
        assert!(s.find_match("/api/dev/x").is_some());
        assert!(s.find_match("/api/dev/y").is_none());
        assert!(s.find_match("/api/dev").is_none());
        assert!(s.find_match("/api/dev/x/extra").is_none());
    }

    #[test]
    fn bracketed_segment_matches_single_segment() {
        let s = InjectedRouteSet::new(vec![rec("/blog/[slug]")]);
        assert!(s.find_match("/blog/hello").is_some());
        assert!(s.find_match("/blog/hello-world").is_some());
        assert!(s.find_match("/blog/").is_none());
        assert!(s.find_match("/blog").is_none());
        assert!(s.find_match("/blog/a/b").is_none());
    }

    #[test]
    fn catchall_matches_one_or_more_segments() {
        let s = InjectedRouteSet::new(vec![rec("/docs/[...rest]")]);
        assert!(s.find_match("/docs/a").is_some());
        assert!(s.find_match("/docs/a/b/c").is_some());
        assert!(s.find_match("/docs").is_none());
        assert!(s.find_match("/docs/").is_none());
    }

    #[test]
    fn optional_catchall_matches_zero_or_more_segments() {
        let s = InjectedRouteSet::new(vec![rec("/docs/[[...rest]]")]);
        // Zero segments: the bare prefix matches…
        assert!(s.find_match("/docs").is_some());
        // …but the trailing-slash form does not (Hono `:rest{.+}?` parity).
        assert!(s.find_match("/docs/").is_none());
        // One or more segments: same as the required catch-all.
        assert!(s.find_match("/docs/a").is_some());
        assert!(s.find_match("/docs/a/b/c").is_some());
        // Different prefix never matches.
        assert!(s.find_match("/other").is_none());
        assert!(s.find_match("/other/a").is_none());
    }

    #[test]
    fn root_optional_catchall_matches_root_url() {
        let s = InjectedRouteSet::new(vec![rec("/[[...rest]]")]);
        assert!(s.find_match("/").is_some());
        assert!(s.find_match("/a").is_some());
        assert!(s.find_match("/a/b").is_some());
    }

    #[test]
    fn optional_catchall_must_be_last_pattern_segment() {
        let s = InjectedRouteSet::new(vec![rec("/docs/[[...rest]]/edit")]);
        assert!(s.find_match("/docs/a/edit").is_none());
        assert!(s.find_match("/docs/edit").is_none());
    }

    #[test]
    fn no_match_when_set_is_empty() {
        let s = InjectedRouteSet::default();
        assert!(s.find_match("/anything").is_none());
    }

    #[test]
    fn first_registered_wins_when_two_patterns_overlap() {
        // Patterns are declaration-ordered upstream; a literal
        // registered before a wildcard should be preferred.
        let s = InjectedRouteSet::new(vec![rec("/blog/feed"), rec("/blog/[slug]")]);
        let m = s.find_match("/blog/feed").unwrap();
        assert_eq!(m.pattern, "/blog/feed");
    }
}
