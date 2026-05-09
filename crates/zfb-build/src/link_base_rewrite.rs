//! Shared HTML link / form rewriter for the configured `base` prefix
//! (issue #228). Lives here so both consumers can call the same code:
//!
//! - **Build pipeline** (`zfb` crate, `commands::link_base_rewrite`)
//!   walks emitted SSG HTML files post-render and rewrites them on
//!   disk via [`apply_link_base_rewrite`]-style file I/O wrapping.
//! - **Dev server** (`zfb-server`, `routes::page_response_bytes`)
//!   applies the rewrite in-flight on cached HTML responses when the
//!   dev server is mounted under a `base` prefix (issue #229's
//!   review caught that the dev server was serving cached HTML
//!   verbatim, so user `<a href="/about">` literals 404'd against
//!   the prefixed dev mount).
//!
//! Both consumers funnel through the same boundary semantics so a
//! page that renders correctly in `zfb build` also renders correctly
//! in `zfb dev` — no drift between modes.
//!
//! ## Scope (deliberately narrow)
//!
//! Rewrite **only** `href` on `<a>` and `action` on `<form>`. Other
//! navigation-shaped attributes (`<area href>`, `<form formaction>`,
//! `<base href>`, `<link rel="alternate">`, …) are out of scope —
//! that mirrors the asset URL rewrite that handles only `<link>` /
//! `<script>`. Adding more later is cheap.
//!
//! ## Skip rules — see [`compute_prefixed`]
//!
//! Authors can opt a specific element out of the rewrite by adding the
//! `data-no-base` attribute (any value, including bareword). The
//! attribute is left on the emitted HTML — browsers ignore unknown
//! `data-*` attributes, so the cost is one harmless attribute per
//! opt-out and the rewriter stays trivially correct on re-runs.

use std::borrow::Cow;

use lol_html::html_content::Element;
use lol_html::{ElementContentHandlers, LocalHandlerTypes, RewriteStrSettings, Selector};

/// Rewrite root-absolute `<a href>` / `<form action>` in `html` by
/// prepending `prefix`. Pure function — no I/O.
///
/// `prefix` MUST be the canonical form: empty (no rewrite), or a path
/// with no trailing slash (`/foo`). Callers that compute the prefix
/// from `cfg.base` should run it through their canonicaliser
/// (`zfb::config::asset_url_base_prefix` for the build pipeline,
/// `zfb_types::dev_mount_prefix` for the dev server) before passing
/// it here. An empty prefix is a no-op via the idempotency check, so
/// callers do not need to short-circuit themselves.
///
/// Absolute-URL prefixes (`https://cdn.example.com`) are accepted but
/// callers should prefer to skip the rewrite — user navigation
/// shouldn't redirect to a CDN host that doesn't serve HTML.
pub fn rewrite_links_in_html(
    html: &str,
    prefix: &str,
) -> Result<String, lol_html::errors::RewritingError> {
    let a_selector: Selector = "a[href]".parse().expect("static selector is valid");
    let form_selector: Selector = "form[action]"
        .parse()
        .expect("static selector is valid");

    // Each handler is `FnMut + Send + Sync`; capture an owned `String`
    // clone of the prefix so the closures don't borrow from the outer
    // scope.
    let prefix_for_a = prefix.to_string();
    let prefix_for_form = prefix.to_string();

    lol_html::rewrite_str(
        html,
        RewriteStrSettings {
            element_content_handlers: vec![
                (
                    Cow::Owned(a_selector),
                    ElementContentHandlers::default().element(move |el: &mut Element<'_, '_, _>| {
                        if has_no_base_optout(el) {
                            return Ok(());
                        }
                        if let Some(href) = el.get_attribute("href") {
                            if let Some(rewritten) = compute_prefixed(&href, &prefix_for_a) {
                                el.set_attribute("href", &rewritten)?;
                            }
                        }
                        Ok(())
                    }),
                ),
                (
                    Cow::Owned(form_selector),
                    ElementContentHandlers::default().element(move |el: &mut Element<'_, '_, _>| {
                        if has_no_base_optout(el) {
                            return Ok(());
                        }
                        if let Some(action) = el.get_attribute("action") {
                            if let Some(rewritten) = compute_prefixed(&action, &prefix_for_form) {
                                el.set_attribute("action", &rewritten)?;
                            }
                        }
                        Ok(())
                    }),
                ),
            ],
            ..RewriteStrSettings::new()
        },
    )
}

/// Return the prefixed value, or `None` to leave the attribute alone.
///
/// Skip rules:
///
/// - Empty string: pass through.
/// - Schemed / absolute URLs (`mailto:`, `tel:`, `javascript:`,
///   `http:`, `https:`, `data:`, `blob:`, `ws:`, `wss:`, `file:`,
///   `ftp:`, …): never start with `/`, so the leading-`/` gate
///   already catches them.
/// - Protocol-relative `//host/...`: starts with two slashes.
/// - Fragment-only `#anchor`: doesn't start with `/`.
/// - Relative paths (`foo.html`, `./foo`, `../foo`): no leading `/`.
/// - Already-prefixed (`/foo` when prefix is `/foo`): pass through —
///   the rewrite is idempotent. The boundary after the prefix must be
///   end-of-string, `/`, `?`, or `#`. A trailing-character mismatch
///   like `/foobar` is NOT considered already-prefixed by `/foo` and
///   IS rewritten.
pub fn compute_prefixed(value: &str, prefix: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    let bytes = value.as_bytes();
    if bytes[0] != b'/' {
        return None;
    }
    if bytes.len() >= 2 && bytes[1] == b'/' {
        return None;
    }
    // Idempotency: if the value is already mounted under `prefix`,
    // leave it alone. The boundary after the prefix must be one of:
    // end-of-string, `/` (path segment), `?` (query suffix), or `#`
    // (fragment suffix). So `/foobar` is NOT considered already-prefixed
    // by `/foo`, but `/foo`, `/foo/x`, `/foo?x=1`, `/foo#top` are.
    if value.starts_with(prefix) {
        let rest = &value[prefix.len()..];
        if rest.is_empty()
            || rest.starts_with('/')
            || rest.starts_with('?')
            || rest.starts_with('#')
        {
            return None;
        }
    }
    Some(format!("{prefix}{value}"))
}

/// Case-insensitive check: is `data-no-base` present on this element?
/// `lol_html`'s element selectors and `get_attribute` /
/// `has_attribute` are already case-insensitive on attribute names,
/// so a literal lookup suffices.
pub fn has_no_base_optout(el: &Element<'_, '_, LocalHandlerTypes>) -> bool {
    el.has_attribute("data-no-base")
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- compute_prefixed -------------------------------------------------

    #[test]
    fn root_absolute_gets_prefix() {
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
    fn root_alone_becomes_base_root() {
        assert_eq!(compute_prefixed("/", "/foo").as_deref(), Some("/foo/"));
    }

    #[test]
    fn preserves_query_and_hash() {
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
    fn idempotent_on_already_prefixed() {
        assert_eq!(compute_prefixed("/foo", "/foo"), None);
        assert_eq!(compute_prefixed("/foo/", "/foo"), None);
        assert_eq!(compute_prefixed("/foo/about", "/foo"), None);
        assert_eq!(compute_prefixed("/foo/api/x", "/foo"), None);
    }

    #[test]
    fn idempotent_on_prefixed_with_query_or_fragment() {
        // The boundary admits `?` and `#` so a prefixed URL with a
        // suffix doesn't get double-prefixed (codex review caught
        // this).
        assert_eq!(compute_prefixed("/foo?tab=1", "/foo"), None);
        assert_eq!(compute_prefixed("/foo?", "/foo"), None);
        assert_eq!(compute_prefixed("/foo#top", "/foo"), None);
        assert_eq!(compute_prefixed("/foo#", "/foo"), None);
        assert_eq!(compute_prefixed("/foo?x=1#y", "/foo"), None);
    }

    #[test]
    fn partial_prefix_still_rewrites() {
        assert_eq!(
            compute_prefixed("/foobar", "/foo").as_deref(),
            Some("/foo/foobar")
        );
        // `/foobar?x=1` is NOT under `/foo` — boundary char after the
        // candidate prefix must be `/` / `?` / `#` / end-of-string.
        assert_eq!(
            compute_prefixed("/foobar?x=1", "/foo").as_deref(),
            Some("/foo/foobar?x=1")
        );
    }

    #[test]
    fn skips_empty() {
        assert_eq!(compute_prefixed("", "/foo"), None);
    }

    #[test]
    fn skips_fragment_only() {
        assert_eq!(compute_prefixed("#anchor", "/foo"), None);
    }

    #[test]
    fn skips_protocol_relative() {
        assert_eq!(compute_prefixed("//cdn.example.com/x", "/foo"), None);
    }

    #[test]
    fn skips_schemed_urls() {
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
    fn skips_relative_paths() {
        for v in ["foo.html", "./foo", "../foo", "page", "page/sub.html"] {
            assert_eq!(
                compute_prefixed(v, "/foo"),
                None,
                "expected skip for {v}"
            );
        }
    }

    #[test]
    fn empty_prefix_is_noop_via_idempotency() {
        assert_eq!(compute_prefixed("/about", ""), None);
        assert_eq!(compute_prefixed("/", ""), None);
    }

    // --- rewrite_links_in_html -------------------------------------------

    #[test]
    fn rewrite_a_href_root_absolute() {
        let html = r#"<a href="/about">About</a>"#;
        let out = rewrite_links_in_html(html, "/foo").unwrap();
        assert!(
            out.contains(r#"href="/foo/about""#),
            "expected /foo/about; got: {out}"
        );
    }

    #[test]
    fn rewrite_form_action_root_absolute() {
        let html = r#"<form action="/submit"><input/></form>"#;
        let out = rewrite_links_in_html(html, "/foo").unwrap();
        assert!(
            out.contains(r#"action="/foo/submit""#),
            "expected /foo/submit; got: {out}"
        );
    }

    #[test]
    fn rewrite_data_no_base_optout_a() {
        let html = r#"<a href="/about" data-no-base>About</a>"#;
        let out = rewrite_links_in_html(html, "/foo").unwrap();
        assert!(out.contains(r#"href="/about""#), "got: {out}");
        assert!(!out.contains("/foo/about"), "got: {out}");
    }

    #[test]
    fn rewrite_data_no_base_with_value_also_optout() {
        for snippet in [
            r#"<a href="/x" data-no-base>x</a>"#,
            r#"<a href="/x" data-no-base="">x</a>"#,
            r#"<a href="/x" data-no-base="true">x</a>"#,
        ] {
            let out = rewrite_links_in_html(snippet, "/foo").unwrap();
            assert!(
                out.contains(r#"href="/x""#) && !out.contains("/foo/x"),
                "expected unchanged for: {snippet}; got: {out}"
            );
        }
    }

    #[test]
    fn rewrite_idempotent_on_already_prefixed() {
        let html = r#"<a href="/foo/about">about</a>"#;
        let out = rewrite_links_in_html(html, "/foo").unwrap();
        assert_eq!(out, html);
    }

    #[test]
    fn rewrite_idempotent_with_query() {
        // Regression for the fragment/query boundary fix — once-rewritten
        // HTML with prefixed URLs that carry queries / fragments must
        // stay byte-identical on a second pass.
        let html = r#"<a href="/foo/about?tab=1#top">x</a>"#;
        let out = rewrite_links_in_html(html, "/foo").unwrap();
        assert_eq!(out, html);
    }

    #[test]
    fn rewrite_skips_external_relative_and_schemed() {
        let html = r##"<a href="//cdn.example.com/x">cdn</a>
            <a href="#anchor">anchor</a>
            <a href="mailto:x@y">mail</a>
            <a href="http://example.com/">http</a>
            <a href="javascript:void(0)">js</a>
            <a href="foo.html">rel</a>"##;
        let out = rewrite_links_in_html(html, "/foo").unwrap();
        for original in [
            r#"href="//cdn.example.com/x""#,
            r##"href="#anchor""##,
            r#"href="mailto:x@y""#,
            r#"href="http://example.com/""#,
            r#"href="javascript:void(0)""#,
            r#"href="foo.html""#,
        ] {
            assert!(out.contains(original), "expected to find {original}");
        }
        assert!(!out.contains("/foo/"), "no rewrites expected; got: {out}");
    }

    #[test]
    fn rewrite_attribute_order_does_not_matter() {
        let html = r#"<a data-no-base href="/about">About</a>"#;
        let out = rewrite_links_in_html(html, "/foo").unwrap();
        assert!(out.contains(r#"href="/about""#), "got: {out}");
        assert!(!out.contains("/foo/about"), "got: {out}");
    }

    #[test]
    fn rewrite_handles_multiple_links_in_one_doc() {
        let html = r#"<!doctype html><html><body>
            <a href="/about">About</a>
            <a href="/contact">Contact</a>
            <form action="/login"><input/></form>
            <a href="/foo/already">Already</a>
            <a href="/foo/already?tab=1">AlreadyQuery</a>
            <a href="https://example.com/">Out</a>
        </body></html>"#;
        let out = rewrite_links_in_html(html, "/foo").unwrap();
        assert!(out.contains(r#"href="/foo/about""#), "got: {out}");
        assert!(out.contains(r#"href="/foo/contact""#), "got: {out}");
        assert!(out.contains(r#"action="/foo/login""#), "got: {out}");
        assert!(out.contains(r#"href="/foo/already""#), "got: {out}");
        assert!(out.contains(r#"href="/foo/already?tab=1""#), "got: {out}");
        assert!(out.contains(r#"href="https://example.com/""#), "got: {out}");
    }

    #[test]
    fn rewrite_root_alone_gets_base_root() {
        let html = r#"<a href="/">home</a>"#;
        let out = rewrite_links_in_html(html, "/foo").unwrap();
        assert!(out.contains(r#"href="/foo/""#), "got: {out}");
    }

    #[test]
    fn rewrite_empty_prefix_is_noop() {
        let html = r#"<a href="/about">About</a><form action="/x"></form>"#;
        let out = rewrite_links_in_html(html, "").unwrap();
        // With empty prefix the idempotency check sees `value.starts_with("")`
        // == true and bails, so nothing changes.
        assert!(out.contains(r#"href="/about""#), "got: {out}");
        assert!(out.contains(r#"action="/x""#), "got: {out}");
    }
}
