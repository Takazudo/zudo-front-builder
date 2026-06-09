//! Dev-only injected-route plumbing (#255).
//!
//! Plugins call `injectRoute(pattern, entrypoint)` from the new
//! `setup` hook to register a synthetic page route the dev server
//! evaluates through the page rendering pipeline. The build crate
//! owns the canonical [`zfb_build::InjectedRoute`] /
//! [`zfb_build::InjectedRouteList`] types; `zfb-server` already
//! depends on `zfb-build` (see `Cargo.toml`), so this module uses
//! them directly — no local mirror needed.
//!
//! ## v1 scope (Wave 1, #255)
//!
//! This sub-issue ships the **registry plumbing**: the dev router
//! can look up an incoming URL against the injected-route patterns
//! and identify the matching entrypoint. Full evaluation of the
//! TSX/TS entrypoint through the page renderer is a follow-up — the
//! renderer's `pages/`-walk machinery already wires through
//! `zfb-render` and `zfb-router` and integrating a per-route synthetic
//! entry requires touching both. Today the dev router matches the
//! pattern and surfaces the matched [`zfb_build::InjectedRoute`] so
//! the caller can decide what to do (typically: fall through to the
//! existing page cache pipeline, which is the path Wave 1 leaves
//! intact, while emitting a structured log so the user can see the
//! injection landed).
//!
//! Once the renderer-side hook is wired up (planned follow-up), the
//! dev router will route matched patterns into the page pipeline by
//! evaluating `record.entrypoint` like any other `pages/*.tsx` file
//! and inserting the result into the cache under the request URL.
//! The wire shape and the lookup API here are stable so that work
//! does not need to revisit this module.

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
