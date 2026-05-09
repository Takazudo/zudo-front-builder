//! Helpers for normalising the user-supplied `base` config value into
//! the URL-path mount prefix the dev server should mount everything
//! under.
//!
//! ## Why this lives in `zfb-types`
//!
//! Multiple crates need a single, shared interpretation of `base`:
//!
//! - `zfb-server` mounts pages, assets, livereload, and plugin
//!   middleware under the prefix during `zfb dev` (issue #229).
//! - the build pipeline (issue #228) rewrites `<a href>` and
//!   `<form action>` URLs to include the same prefix.
//!
//! Both must agree on the canonical normal form to avoid drift between
//! "what the dev server serves" and "what the rendered HTML asks for".
//! `zfb-types` is the lowest-dep leaf crate every consumer can already
//! depend on, so the helper lives here rather than in `zfb` (which is
//! a high-level crate the renderer cannot import) or `zfb-server`
//! (which is dev-only and is a sibling, not an ancestor, of the build
//! pipeline).

/// Normalise a `base` config value into the URL-path mount prefix the
/// dev server uses to mount pages, assets, livereload, and plugin
/// middleware.
///
/// Returns:
///
/// - `None` for `None`, `""`, `"/"`, or any absolute-URL form (e.g.
///   `"https://cdn.example.com/"`). The dev server should mount
///   everything at the root in these cases — the absolute-URL case
///   means the assets are served from a different origin (typically a
///   CDN) and the dev server cannot help with them.
/// - `Some("/foo")` — leading slash, no trailing slash — for any
///   path-shaped value like `"/foo"`, `"/foo/"`, `"/foo/bar/"`. Callers
///   can concatenate `"<prefix>/<path>"` cleanly knowing the prefix has
///   exactly one leading slash and no trailing slash.
///
/// Mirrors the input matrix accepted by
/// [`zfb::config::asset_url_base_prefix`] except the absolute-URL case
/// returns `None` (the dev server cannot mount on a different origin)
/// and `Some` is reserved for the path-shaped case the dev server can
/// actually serve.
pub fn dev_mount_prefix(base: Option<&str>) -> Option<String> {
    let raw = base?;
    let trimmed = raw.trim_end_matches('/');
    if trimmed.is_empty() {
        // `""` or `"/"` (or `"//"`, …) — none of these mount the site
        // under a sub-path on the dev server.
        return None;
    }
    if trimmed.contains("://") {
        // Absolute URL — dev server can't mount on a different origin.
        // The build emits absolute URLs for assets in this case; there
        // is nothing for the dev server to do beyond serving the page
        // HTML at the root (which is the no-base behaviour).
        return None;
    }
    if !trimmed.starts_with('/') {
        // Malformed path-shaped value (no leading slash). Treat as
        // no-prefix rather than mounting under a relative-style path
        // that axum cannot represent.
        return None;
    }
    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_returns_none() {
        assert_eq!(dev_mount_prefix(None), None);
    }

    #[test]
    fn empty_string_returns_none() {
        assert_eq!(dev_mount_prefix(Some("")), None);
    }

    #[test]
    fn root_slash_returns_none() {
        assert_eq!(dev_mount_prefix(Some("/")), None);
    }

    #[test]
    fn multiple_trailing_slashes_collapse_to_root() {
        assert_eq!(dev_mount_prefix(Some("//")), None);
        assert_eq!(dev_mount_prefix(Some("///")), None);
    }

    #[test]
    fn path_with_trailing_slash_strips_it() {
        assert_eq!(dev_mount_prefix(Some("/foo/")), Some("/foo".to_string()));
        assert_eq!(
            dev_mount_prefix(Some("/foo/bar/")),
            Some("/foo/bar".to_string())
        );
    }

    #[test]
    fn path_without_trailing_slash_is_idempotent() {
        assert_eq!(dev_mount_prefix(Some("/foo")), Some("/foo".to_string()));
        assert_eq!(
            dev_mount_prefix(Some("/foo/bar")),
            Some("/foo/bar".to_string())
        );
    }

    #[test]
    fn absolute_url_returns_none() {
        // CDN-style bases — the dev server doesn't mount on a different
        // origin, so it should keep the no-base behaviour.
        assert_eq!(dev_mount_prefix(Some("https://cdn.example.com/")), None);
        assert_eq!(dev_mount_prefix(Some("http://localhost:8080/")), None);
        assert_eq!(dev_mount_prefix(Some("https://cdn.example.com")), None);
    }

    #[test]
    fn missing_leading_slash_returns_none() {
        // Malformed path — no leading slash means axum can't mount it.
        assert_eq!(dev_mount_prefix(Some("foo")), None);
        assert_eq!(dev_mount_prefix(Some("foo/bar")), None);
    }
}
