//! Issue #228: rewrite root-absolute `<a href>` / `<form action>` to
//! sit under the configured `base` prefix in shipped HTML.
//!
//! ## Why this exists
//!
//! `zfb build` already prefixes asset URLs (`<link rel="stylesheet">`,
//! `<script type="module">`) with `cfg.base` via
//! [`super::build::apply_asset_url_base`]. User-authored absolute
//! links — e.g. `<a href="/about">` in page TSX — were left alone, so
//! a sub-path deploy (`https://example.com/pj/zudo-doc/`) styled the
//! pages correctly but every internal click went to `/about` (a 404 /
//! the wrong site) instead of `/pj/zudo-doc/about`.
//!
//! ## Scope (deliberately narrow)
//!
//! We rewrite **only** `href` on `<a>` and `action` on `<form>`. Other
//! navigation-shaped attributes (`<area href>`, `<form formaction>`,
//! `<base href>`, `<link rel="alternate">`, …) are out of scope — the
//! issue calls out the two attributes above and the prod-asset rewrite
//! we mirror only handles `<link>` / `<script>`. Adding more later is
//! cheap; the per-element handler shape extends naturally.
//!
//! ## Skip rules
//!
//! Only ROOT-ABSOLUTE paths get prefixed — values that start with a
//! single `/` not followed by another `/`. Everything else passes
//! through verbatim:
//!
//! - empty string, fragment-only (`#anchor`), protocol-relative
//!   (`//host/x`), schemed URLs (`mailto:`, `tel:`, `javascript:`,
//!   `http:`, `https:`, …) — schemed URLs never start with `/`, so
//!   the leading-`/` gate already catches them.
//! - relative paths (`foo.html`, `./foo`, `../foo`).
//! - already-prefixed absolute paths (`/foo/about` when base is
//!   `/foo`) — the rewrite is **idempotent** so a second build pass
//!   over already-shipped HTML is a no-op.
//!
//! Authors can opt a specific element out of the rewrite by adding the
//! `data-no-base` attribute (any value, including bareword). The
//! attribute is currently **left on the emitted HTML** — browsers
//! ignore unknown `data-*` attributes, so the cost is one harmless
//! attribute per opt-out and the rewriter stays trivially correct on
//! re-runs.
//!
//! ## CDN-shaped base (`base = "https://cdn.example.com/"`)
//!
//! The same `cfg.base` slot accepts an absolute URL for asset hosting
//! (`asset_url_base_prefix` returns e.g. `"https://cdn.example.com"`).
//! User-link rewriting is **disabled** for that shape: clicking
//! `<a href="/about">` should keep navigating inside the current
//! origin, not get redirected to a CDN host that does not serve HTML.
//! Asset URLs continue to point at the CDN via the existing pipeline.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use zfb_build::link_base_rewrite::rewrite_links_in_html;

use crate::config::asset_url_base_prefix;

/// Walk every emitted SSG HTML file under `out_dir` and prefix every
/// root-absolute `<a href>` / `<form action>` with the configured
/// `base`. Re-writes the file in-place only when the rewriter
/// produced different bytes — projects without absolute internal
/// links round-trip the file untouched.
///
/// The rewrite is a **no-op** when:
///
/// - `base` normalises to empty (None / `""` / `"/"`), OR
/// - `base` is an absolute URL (CDN host); see module docs.
///
/// Non-`*.html` / `*.htm` outputs (XML feeds, JSON, …) are skipped
/// even when `base` is set — the lol_html parser is HTML-only and
/// would mangle non-HTML payloads.
pub fn apply_link_base_rewrite(
    out_dir: &Path,
    written_pages: &[PathBuf],
    base: Option<&str>,
    add_trailing_slash: bool,
) -> Result<()> {
    let prefix = asset_url_base_prefix(base);
    if prefix.is_empty() || !prefix.starts_with('/') {
        // Empty prefix → no base configured. Absolute-URL prefix →
        // CDN-style base; user links stay same-origin.
        return Ok(());
    }
    for page in written_pages {
        if !is_html_path(page) {
            continue;
        }
        let bytes = std::fs::read(page).with_context(|| {
            format!(
                "link-base-rewrite: failed to read {}",
                page.display()
            )
        })?;
        // The renderer wrote UTF-8 HTML; bail if a binary-ish file
        // somehow snuck through with a `.html` extension instead of
        // silently corrupting it via lossy decoding.
        let html = std::str::from_utf8(&bytes).with_context(|| {
            format!(
                "link-base-rewrite: {} is not valid UTF-8",
                page.display()
            )
        })?;
        let new_html = rewrite_links_in_html(html, &prefix, add_trailing_slash)
            .with_context(|| {
                format!(
                    "link-base-rewrite: lol_html failed on {} (under {})",
                    page.display(),
                    out_dir.display()
                )
            })?;
        if new_html != html {
            std::fs::write(page, new_html).with_context(|| {
                format!(
                    "link-base-rewrite: failed to write {}",
                    page.display()
                )
            })?;
        }
    }
    Ok(())
}

fn is_html_path(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|s| s.to_str()),
        Some("html") | Some("htm")
    )
}

// `rewrite_links_in_html`, `compute_prefixed`, and `has_no_base_optout`
// live in `zfb_build::link_base_rewrite` so the dev server (issue
// #229's `routes::page_response_bytes`) and this build-pass call the
// same code. The dev server applies the rewrite in-flight on cached
// HTML responses; the build pass applies it on disk via
// [`apply_link_base_rewrite`] above.

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;
    use zfb_build::link_base_rewrite::{compute_prefixed, rewrite_links_in_html};

    // --- compute_prefixed -----------------------------------------------------
    //
    // The pure-string pieces are tested in zfb_build::link_base_rewrite
    // alongside their implementation; the smoke tests here keep a
    // few cases pinned at this layer to prevent silent drift if a
    // future refactor extracts/re-exports them differently.

    #[test]
    fn compute_prefixed_root_absolute_gets_prefix() {
        assert_eq!(
            compute_prefixed("/about", "/foo").as_deref(),
            Some("/foo/about")
        );
        assert_eq!(
            compute_prefixed("/api/users", "/foo").as_deref(),
            Some("/foo/api/users")
        );
    }

    #[test]
    fn compute_prefixed_root_alone_becomes_base_root() {
        assert_eq!(compute_prefixed("/", "/foo").as_deref(), Some("/foo/"));
    }

    #[test]
    fn compute_prefixed_preserves_query_and_hash() {
        assert_eq!(
            compute_prefixed("/about?x=1", "/foo").as_deref(),
            Some("/foo/about?x=1")
        );
        assert_eq!(
            compute_prefixed("/about#section", "/foo").as_deref(),
            Some("/foo/about#section")
        );
    }

    #[test]
    fn compute_prefixed_idempotent_on_already_prefixed() {
        // Boundary cases: exactly `prefix`, prefix + `/`, prefix +
        // `/something` all yield None (no double-prefix).
        assert_eq!(compute_prefixed("/foo", "/foo"), None);
        assert_eq!(compute_prefixed("/foo/", "/foo"), None);
        assert_eq!(compute_prefixed("/foo/about", "/foo"), None);
        assert_eq!(compute_prefixed("/foo/api/x", "/foo"), None);
    }

    #[test]
    fn compute_prefixed_idempotent_on_prefixed_with_query_or_fragment() {
        // Codex review caught these: a prefixed URL with a `?query` or
        // `#fragment` suffix is still under the prefix and must NOT be
        // double-prefixed. Without the `?` / `#` boundary, the previous
        // logic would rewrite `/foo?tab=1` → `/foo/foo?tab=1` because
        // it failed the "rest starts with /" check.
        assert_eq!(compute_prefixed("/foo?tab=1", "/foo"), None);
        assert_eq!(compute_prefixed("/foo?", "/foo"), None);
        assert_eq!(compute_prefixed("/foo#top", "/foo"), None);
        assert_eq!(compute_prefixed("/foo#", "/foo"), None);
        assert_eq!(compute_prefixed("/foo?x=1#y", "/foo"), None);
        // The boundary still rejects partial matches: `/foobar?` is NOT
        // under prefix `/foo` and should still be rewritten.
        assert_eq!(
            compute_prefixed("/foobar?x=1", "/foo").as_deref(),
            Some("/foo/foobar?x=1"),
        );
    }

    #[test]
    fn compute_prefixed_partial_prefix_still_rewrites() {
        // `/foobar` does NOT match prefix `/foo` because the next
        // char after `/foo` is `b`, not `/` or end-of-string.
        assert_eq!(
            compute_prefixed("/foobar", "/foo").as_deref(),
            Some("/foo/foobar")
        );
    }

    #[test]
    fn compute_prefixed_skips_empty() {
        assert_eq!(compute_prefixed("", "/foo"), None);
    }

    #[test]
    fn compute_prefixed_skips_fragment() {
        assert_eq!(compute_prefixed("#anchor", "/foo"), None);
    }

    #[test]
    fn compute_prefixed_skips_protocol_relative() {
        assert_eq!(compute_prefixed("//cdn.example.com/x", "/foo"), None);
    }

    #[test]
    fn compute_prefixed_skips_schemed_urls() {
        for v in [
            "mailto:x@y.com",
            "tel:+15555550100",
            "javascript:void(0)",
            "data:text/plain,hi",
            "blob:https://example.com/abc",
            "ws://example.com/",
            "wss://example.com/",
            "file:///etc/passwd",
            "ftp://example.com/x",
            "http://example.com/x",
            "https://example.com/x",
        ] {
            assert_eq!(
                compute_prefixed(v, "/foo"),
                None,
                "expected skip for {v}"
            );
        }
    }

    #[test]
    fn compute_prefixed_skips_relative_paths() {
        for v in ["foo.html", "./foo", "../foo", "page", "page/sub.html"] {
            assert_eq!(
                compute_prefixed(v, "/foo"),
                None,
                "expected skip for {v}"
            );
        }
    }

    #[test]
    fn compute_prefixed_empty_prefix_is_noop_via_idempotency() {
        // Defensive: caller short-circuits empty prefix already, but
        // if it slipped through we still leave values alone (the
        // idempotency check sees `value.starts_with("")` == true and
        // bails).
        assert_eq!(compute_prefixed("/about", ""), None);
        assert_eq!(compute_prefixed("/", ""), None);
    }

    // --- rewrite_links_in_html -----------------------------------------------

    #[test]
    fn rewrite_html_a_href_root_absolute() {
        let html = r#"<a href="/about">About</a>"#;
        let out = rewrite_links_in_html(html, "/foo", false).unwrap();
        assert!(
            out.contains(r#"href="/foo/about""#),
            "expected /foo/about; got: {out}"
        );
    }

    #[test]
    fn rewrite_html_form_action_root_absolute() {
        let html = r#"<form action="/submit"><input/></form>"#;
        let out = rewrite_links_in_html(html, "/foo", false).unwrap();
        assert!(
            out.contains(r#"action="/foo/submit""#),
            "expected /foo/submit; got: {out}"
        );
    }

    #[test]
    fn rewrite_html_data_no_base_optout_a() {
        let html = r#"<a href="/about" data-no-base>About</a>"#;
        let out = rewrite_links_in_html(html, "/foo", false).unwrap();
        // Original href preserved.
        assert!(
            out.contains(r#"href="/about""#),
            "expected unchanged href; got: {out}"
        );
        assert!(
            !out.contains("/foo/about"),
            "did not expect /foo/about; got: {out}"
        );
    }

    #[test]
    fn rewrite_html_data_no_base_optout_form() {
        let html = r#"<form action="/submit" data-no-base><input/></form>"#;
        let out = rewrite_links_in_html(html, "/foo", false).unwrap();
        assert!(out.contains(r#"action="/submit""#), "got: {out}");
        assert!(!out.contains("/foo/submit"), "got: {out}");
    }

    #[test]
    fn rewrite_html_data_no_base_with_value_also_optout() {
        // The opt-out is presence-based, like other boolean-ish
        // data attributes; any value (including bareword, `"true"`,
        // empty) skips.
        for snippet in [
            r#"<a href="/x" data-no-base>x</a>"#,
            r#"<a href="/x" data-no-base="">x</a>"#,
            r#"<a href="/x" data-no-base="true">x</a>"#,
            r#"<a href="/x" data-no-base="anything">x</a>"#,
        ] {
            let out = rewrite_links_in_html(snippet, "/foo", false).unwrap();
            assert!(
                out.contains(r#"href="/x""#) && !out.contains("/foo/x"),
                "expected unchanged for: {snippet}; got: {out}"
            );
        }
    }

    #[test]
    fn rewrite_html_attribute_order_does_not_matter() {
        // data-no-base could appear before href.
        let html = r#"<a data-no-base href="/about">About</a>"#;
        let out = rewrite_links_in_html(html, "/foo", false).unwrap();
        assert!(out.contains(r#"href="/about""#), "got: {out}");
        assert!(!out.contains("/foo/about"), "got: {out}");
    }

    #[test]
    fn rewrite_html_skips_external_relative_and_schemed() {
        // NB: outer raw-string delimiter is `r##"..."##` because the
        // inner literal contains `"#anchor"` — `r#"..."#` would close
        // at `"#` and split the test data in half.
        let html = r##"<a href="//cdn.example.com/x">cdn</a>
            <a href="#anchor">anchor</a>
            <a href="mailto:x@y">mail</a>
            <a href="http://example.com/">http</a>
            <a href="https://example.com/">https</a>
            <a href="javascript:void(0)">js</a>
            <a href="foo.html">rel</a>
            <a href="./foo">dot</a>
            <a href="../foo">dotdot</a>"##;
        let out = rewrite_links_in_html(html, "/foo", false).unwrap();
        // None of the originals should have been touched.
        for original in [
            r#"href="//cdn.example.com/x""#,
            r##"href="#anchor""##,
            r#"href="mailto:x@y""#,
            r#"href="http://example.com/""#,
            r#"href="https://example.com/""#,
            r#"href="javascript:void(0)""#,
            r#"href="foo.html""#,
            r#"href="./foo""#,
            r#"href="../foo""#,
        ] {
            assert!(
                out.contains(original),
                "expected to find {original} in: {out}"
            );
        }
        // And nothing got prefixed.
        assert!(
            !out.contains("/foo/"),
            "no rewrites expected; got: {out}"
        );
    }

    #[test]
    fn rewrite_html_idempotent_on_already_prefixed() {
        // Already-prefixed values stay byte-identical.
        let html = r#"<a href="/foo/about">about</a>"#;
        let out = rewrite_links_in_html(html, "/foo", false).unwrap();
        assert_eq!(out, html);
    }

    #[test]
    fn rewrite_html_handles_multiple_links_in_one_doc() {
        let html = r#"<!doctype html><html><body>
            <a href="/about">About</a>
            <a href="/contact">Contact</a>
            <form action="/login"><input/></form>
            <a href="/foo/already">Already</a>
            <a href="https://example.com/">Out</a>
        </body></html>"#;
        let out = rewrite_links_in_html(html, "/foo", false).unwrap();
        assert!(out.contains(r#"href="/foo/about""#), "got: {out}");
        assert!(out.contains(r#"href="/foo/contact""#), "got: {out}");
        assert!(out.contains(r#"action="/foo/login""#), "got: {out}");
        // Already-prefixed stays as-is.
        assert!(out.contains(r#"href="/foo/already""#), "got: {out}");
        // External stays as-is.
        assert!(
            out.contains(r#"href="https://example.com/""#),
            "got: {out}"
        );
    }

    #[test]
    fn rewrite_html_root_alone_gets_base_root() {
        let html = r#"<a href="/">home</a>"#;
        let out = rewrite_links_in_html(html, "/foo", false).unwrap();
        assert!(out.contains(r#"href="/foo/""#), "got: {out}");
    }

    // --- apply_link_base_rewrite (file I/O) ----------------------------------

    fn write_file(dir: &Path, rel: &str, body: &str) -> PathBuf {
        let abs = dir.join(rel);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&abs, body).unwrap();
        abs
    }

    #[test]
    fn apply_no_base_is_noop_on_disk() {
        let tmp = tempdir().unwrap();
        let body = r#"<a href="/about">about</a>"#;
        let p = write_file(tmp.path(), "index.html", body);
        apply_link_base_rewrite(tmp.path(), std::slice::from_ref(&p), None, false).unwrap();
        let after = fs::read_to_string(&p).unwrap();
        assert_eq!(after, body);
    }

    #[test]
    fn apply_root_base_is_noop_on_disk() {
        // base = "/" normalises to "" → no-op
        let tmp = tempdir().unwrap();
        let body = r#"<a href="/about">about</a>"#;
        let p = write_file(tmp.path(), "index.html", body);
        apply_link_base_rewrite(tmp.path(), std::slice::from_ref(&p), Some("/"), false).unwrap();
        let after = fs::read_to_string(&p).unwrap();
        assert_eq!(after, body);
    }

    #[test]
    fn apply_subpath_base_with_trailing_slash() {
        let tmp = tempdir().unwrap();
        let body = r#"<!doctype html><html><body><a href="/about">About</a></body></html>"#;
        let p = write_file(tmp.path(), "index.html", body);
        apply_link_base_rewrite(tmp.path(), std::slice::from_ref(&p), Some("/foo/"), false).unwrap();
        let after = fs::read_to_string(&p).unwrap();
        assert!(
            after.contains(r#"href="/foo/about""#),
            "expected /foo/about; got: {after}"
        );
    }

    #[test]
    fn apply_subpath_base_without_trailing_slash_is_canonical() {
        let tmp = tempdir().unwrap();
        let body = r#"<a href="/about">about</a>"#;
        let p = write_file(tmp.path(), "index.html", body);
        apply_link_base_rewrite(tmp.path(), std::slice::from_ref(&p), Some("/foo"), false).unwrap();
        let after = fs::read_to_string(&p).unwrap();
        assert!(
            after.contains(r#"href="/foo/about""#),
            "expected /foo/about; got: {after}"
        );
    }

    #[test]
    fn apply_absolute_url_base_skips_user_links() {
        // CDN-shaped base does NOT rewrite user links.
        let tmp = tempdir().unwrap();
        let body = r#"<a href="/about">about</a>"#;
        let p = write_file(tmp.path(), "index.html", body);
        apply_link_base_rewrite(tmp.path(), std::slice::from_ref(&p), Some("https://cdn.example.com/"), false).unwrap();
        let after = fs::read_to_string(&p).unwrap();
        assert_eq!(after, body, "user links should stay same-origin");
    }

    #[test]
    fn apply_skips_non_html_files() {
        let tmp = tempdir().unwrap();
        let xml_body = r#"<?xml version="1.0"?><feed><link href="/post/1"/></feed>"#;
        let xml = write_file(tmp.path(), "feed.xml", xml_body);
        apply_link_base_rewrite(tmp.path(), std::slice::from_ref(&xml), Some("/foo"), false).unwrap();
        let after = fs::read_to_string(&xml).unwrap();
        assert_eq!(
            after, xml_body,
            "non-HTML file should not be touched"
        );
    }

    #[test]
    fn apply_handles_form_action_in_real_doc() {
        let tmp = tempdir().unwrap();
        let body = r#"<!doctype html><html><body>
            <form action="/submit" method="post"><button>go</button></form>
        </body></html>"#;
        let p = write_file(tmp.path(), "index.html", body);
        apply_link_base_rewrite(tmp.path(), std::slice::from_ref(&p), Some("/foo/"), false).unwrap();
        let after = fs::read_to_string(&p).unwrap();
        assert!(
            after.contains(r#"action="/foo/submit""#),
            "expected /foo/submit; got: {after}"
        );
    }

    #[test]
    fn apply_idempotent_on_second_run() {
        // Run the pass twice; the second run is a no-op (no
        // double-prefix).
        let tmp = tempdir().unwrap();
        let body = r#"<a href="/about">about</a>"#;
        let p = write_file(tmp.path(), "index.html", body);
        apply_link_base_rewrite(tmp.path(), std::slice::from_ref(&p), Some("/foo/"), false).unwrap();
        let after_first = fs::read_to_string(&p).unwrap();
        apply_link_base_rewrite(tmp.path(), std::slice::from_ref(&p), Some("/foo/"), false).unwrap();
        let after_second = fs::read_to_string(&p).unwrap();
        assert_eq!(after_first, after_second);
        assert!(after_second.contains(r#"href="/foo/about""#));
        assert!(!after_second.contains("/foo/foo/"));
    }

    #[test]
    fn apply_handles_data_no_base_end_to_end() {
        let tmp = tempdir().unwrap();
        let body = r#"<!doctype html><html><body>
            <a href="/about">about</a>
            <a href="/legal" data-no-base>legal-stays-bare</a>
        </body></html>"#;
        let p = write_file(tmp.path(), "index.html", body);
        apply_link_base_rewrite(tmp.path(), std::slice::from_ref(&p), Some("/foo/"), false).unwrap();
        let after = fs::read_to_string(&p).unwrap();
        assert!(after.contains(r#"href="/foo/about""#), "got: {after}");
        assert!(after.contains(r#"href="/legal""#), "got: {after}");
        assert!(!after.contains("/foo/legal"), "got: {after}");
    }

    #[test]
    fn is_html_path_extension_check() {
        assert!(is_html_path(Path::new("a.html")));
        assert!(is_html_path(Path::new("nested/dir/a.html")));
        assert!(is_html_path(Path::new("a.htm")));
        assert!(!is_html_path(Path::new("a.xml")));
        assert!(!is_html_path(Path::new("a.json")));
        assert!(!is_html_path(Path::new("a.txt")));
        assert!(!is_html_path(Path::new("noext")));
    }
}
