//! `<head>` asset injection for the build- and dev-side renderers.
//!
//! Originally introduced as a prod-only helper: the production
//! orchestrator hands the renderer a [`ProdHeadAssets`] and `render_all`
//! post-processes each HTML response to splice the stable CSS / islands
//! tags in before atomic-write to disk. The byte-level splicer is
//! deliberately renderer-agnostic, so the dev server now reuses the
//! same function in-flight to inject the islands `<script type="module">`
//! tag on every served HTML page (issue #377). The dev path stays
//! gated by [`crate::pipeline::BuildContext::run_islands`]: when the
//! project has no `"use client"` components there is no bundle to point
//! at, and the dev server simply has no URL to inject — matching the
//! prior contract that ships HTML untouched in that case.
//!
//! ## What this module is, and is not
//!
//! - **Is:** a small, byte-level rewrite that splices a stable CSS
//!   `<link>` tag and zero-or-more islands `<script type="module">` tags
//!   into the rendered page's `<head>`, immediately before `</head>`.
//!   The URLs are taken verbatim from the caller (typically S0's
//!   constants or — once S4 lands — the hashed URLs minted by
//!   [`crate::pipeline::ProductionAssetPipeline`]).
//! - **Is not:** a generic HTML rewriter. Unlike `lol_html`-based passes
//!   in `zfb-islands` and `zfb-server`, this helper does **not** parse
//!   the document. It looks for the literal closing `</head>` tag with
//!   a case-insensitive substring search, splices before it, and bails
//!   out cleanly when the tag is absent (passthrough). That matches the
//!   acceptance criteria — malformed input must round-trip — and avoids
//!   pulling `lol_html` into `zfb-build`'s dependency graph just for a
//!   one-liner injection. It also keeps non-HTML output (like
//!   `feed.xml`) implicitly safe: no `</head>` ⇒ no edit.
//!
//! ## Idempotency
//!
//! Re-running the injection on its own output is a no-op. The caller is
//! free to inject twice (e.g. once per pipeline pass) without creating
//! duplicate `<link>` / `<script>` tags. Two facts make that work:
//!
//! 1. Each candidate tag is rendered once into its final byte form.
//! 2. Before splicing, the helper checks whether that exact byte
//!    sequence already appears between the document start and the first
//!    `</head>`. If so, the tag is skipped.
//!
//! Idempotency is by exact byte match, not by URL semantics — the
//! caller is expected to use stable URLs from
//! [`zfb_types::asset_urls`] (or stable hashed URLs minted exactly
//! once by `ProductionAssetPipeline`). Mixing in differently-spelled
//! URLs for the same asset would produce duplicates, which is the
//! intended behaviour: the caller has changed the contract.

use std::borrow::Cow;

/// Optional head-asset URLs handed to the build-side renderer or
/// spliced into a dev-server HTML response. When `None` or
/// `is_empty()`, the renderer ships HTML untouched (dev /
/// build-without-prod-pipeline parity).
///
/// All URLs are embedded verbatim into the rendered page (after
/// HTML-attribute escaping). The renderer does not validate them —
/// callers are responsible for emitting URLs that match the asset
/// files actually written to `dist/`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProdHeadAssets {
    /// Public URL for the page-wide stylesheet (e.g.
    /// [`zfb_types::STABLE_CSS_URL`] in S1, or
    /// `/assets/styles-<hash>.css` once `ProductionAssetPipeline`
    /// rewrites it). `None` skips the `<link>` tag entirely.
    pub css_url: Option<String>,
    /// Public URLs for client-side island modules. Each becomes one
    /// `<script type="module" src="…"></script>` tag, in this order.
    /// Empty ⇒ no script tags injected.
    pub island_module_urls: Vec<String>,
}

impl ProdHeadAssets {
    /// `true` when this struct would not inject anything. Lets callers
    /// skip the rewrite altogether.
    pub fn is_empty(&self) -> bool {
        self.css_url.is_none() && self.island_module_urls.is_empty()
    }
}

/// Inject `<link>` / `<script>` tags into `html` immediately before
/// the first `</head>` close tag.
///
/// Behaviour:
///
/// - When `assets.is_empty()` or `html` contains no `</head>`, the
///   input is returned unchanged. The acceptance criterion is that a
///   missing close tag is a passthrough, not an error: the route
///   universe contains non-HTML outputs (`feed.xml`, etc.) and we must
///   not corrupt them.
/// - Tags are appended in this order: CSS link first, then island
///   module scripts in the order given. The exact byte sequence for
///   each candidate tag is checked against the existing pre-`</head>`
///   region and skipped on hit, so a second pass over the same HTML
///   does not produce duplicates.
/// - URLs are HTML-attribute escaped — see [`escape_attr`].
pub fn inject_prod_head_assets<'a>(html: &'a str, assets: &ProdHeadAssets) -> Cow<'a, str> {
    if assets.is_empty() {
        return Cow::Borrowed(html);
    }

    let close_at = match find_close_head(html) {
        Some(at) => at,
        // Malformed / non-HTML output (no `</head>` to anchor to).
        // Passthrough — caller's responsibility to pre-filter if they
        // want stricter handling.
        None => return Cow::Borrowed(html),
    };

    // Slice the document into "before close-head" / "from close-head".
    // Idempotency is decided against `before`: a tag is skipped when
    // its exact byte form already appears anywhere up to (and not
    // including) the `</head>` close.
    let before = &html[..close_at];
    let after = &html[close_at..];

    let mut tags: Vec<String> = Vec::with_capacity(1 + assets.island_module_urls.len());
    if let Some(css) = assets.css_url.as_deref() {
        let tag = css_link_tag(css);
        if !before.contains(&tag) {
            tags.push(tag);
        }
    }
    for url in &assets.island_module_urls {
        let tag = island_module_script_tag(url);
        if !before.contains(&tag) {
            tags.push(tag);
        }
    }

    if tags.is_empty() {
        // Every candidate tag was already present — pure no-op.
        return Cow::Borrowed(html);
    }

    let mut out = String::with_capacity(html.len() + tags.iter().map(|t| t.len()).sum::<usize>());
    out.push_str(before);
    for tag in &tags {
        out.push_str(tag);
    }
    out.push_str(after);
    Cow::Owned(out)
}

/// The HTML5 doctype prefix prepended to documents that lack one. The
/// trailing newline is cosmetic (no browser-visible effect); it keeps the
/// emitted `<html>` on its own line.
pub const HTML5_DOCTYPE_PREFIX: &str = "<!doctype html>\n";

/// `true` when `html` is an HTML document that should have
/// [`HTML5_DOCTYPE_PREFIX`] prepended.
///
/// `preact-render-to-string` renders the page's `<html>…</html>` shell
/// verbatim and (correctly) does not emit a doctype — a doctype is not a
/// valid JSX node. Without `<!doctype html>` browsers fall back to quirks
/// mode, which silently changes the box model and CSS the template relies
/// on (issue #524). The renderer prepends the doctype host-side because it
/// is the same constant byte string for every route.
///
/// Conservative, non-parsing rules (mirroring the rest of this module):
///
/// - Leading ASCII whitespace is ignored when sniffing the document start.
///   A leading UTF-8 BOM is *not* whitespace, so a BOM-prefixed document
///   does not match `<html` and is left untouched — this deliberately
///   avoids inserting the doctype ahead of a BOM.
/// - Returns `false` when the document already starts with `<!doctype`
///   (case-insensitive), so the prefix is never doubled.
/// - Returns `true` only when the document starts with `<html`
///   (case-insensitive). Non-HTML output (`feed.xml`'s `<rss/>`, fragments,
///   binary payloads) is left untouched, matching the `</head>`-passthrough
///   contract of [`inject_prod_head_assets`].
pub fn needs_html5_doctype(html: &str) -> bool {
    let sniff = html.trim_start();
    let starts_with_ci = |prefix: &str| {
        sniff.len() >= prefix.len()
            && sniff.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
    };
    !starts_with_ci("<!doctype") && starts_with_ci("<html")
}

/// Build the canonical `<link rel="stylesheet" href="…">` tag form
/// this module injects. Exposed so callers and tests can compare
/// against an authoritative byte string.
pub fn css_link_tag(href: &str) -> String {
    format!(
        "<link rel=\"stylesheet\" href=\"{href}\">",
        href = escape_attr(href),
    )
}

/// Build the canonical `<script type="module" src="…"></script>` tag
/// form this module injects.
pub fn island_module_script_tag(src: &str) -> String {
    format!(
        "<script type=\"module\" src=\"{src}\"></script>",
        src = escape_attr(src),
    )
}

/// HTML-attribute escape — same character set as
/// `zfb_islands::hydration::escape_attr`. Kept local rather than
/// re-exported across the crate boundary because it is a five-line
/// helper and the prod-pipeline contract for islands hashing already
/// avoided introducing such a re-export between `zfb-islands` and
/// `zfb-build`.
fn escape_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Find the byte offset of the first `</head>` tag in `html`,
/// case-insensitive on the tag name. Returns `None` for documents
/// that have no closing tag — those are passed through unchanged.
///
/// Implementation note: a substring scan is sufficient for the
/// acceptance criteria. We deliberately do *not* try to ignore
/// `</head>` strings inside `<pre>` / `<code>` blocks here — the
/// renderer feeds us the raw HTML the page handler returned, and
/// in the prod pipeline a stray `</head>` literal inside body
/// content would be an authoring bug we want to surface, not
/// silently mask. The `lol_html`-based helper in
/// `zfb_islands::hydration` makes the opposite trade-off because it
/// is parsing user-authored island content at runtime; here we
/// inject deterministic SSG output where that contention does not
/// arise in practice.
fn find_close_head(html: &str) -> Option<usize> {
    // Hot path: most pages spell the tag in lowercase.
    if let Some(at) = html.find("</head>") {
        return Some(at);
    }
    // Cold path: HTML is case-insensitive; scan once with a manual
    // case-fold comparison so we don't allocate a lower-cased copy of
    // the whole document on every render.
    let bytes = html.as_bytes();
    const NEEDLE: &[u8] = b"</head>";
    if bytes.len() < NEEDLE.len() {
        return None;
    }
    'outer: for i in 0..=bytes.len() - NEEDLE.len() {
        for (j, n) in NEEDLE.iter().enumerate() {
            let b = bytes[i + j];
            if b.eq_ignore_ascii_case(n) {
                continue;
            }
            continue 'outer;
        }
        return Some(i);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use zfb_types::asset_urls::{STABLE_CSS_URL, STABLE_ISLANDS_URL};

    fn assets_with_css() -> ProdHeadAssets {
        ProdHeadAssets {
            css_url: Some(STABLE_CSS_URL.into()),
            island_module_urls: vec![],
        }
    }

    fn assets_full() -> ProdHeadAssets {
        ProdHeadAssets {
            css_url: Some(STABLE_CSS_URL.into()),
            island_module_urls: vec![STABLE_ISLANDS_URL.into()],
        }
    }

    #[test]
    fn populated_head_gets_link_immediately_before_close_head() {
        let html = "<!doctype html><html><head><title>Hi</title></head><body>x</body></html>";
        let out = inject_prod_head_assets(html, &assets_with_css());
        let out = out.as_ref();
        let close_at = out.find("</head>").unwrap();
        let link_at = out.find("<link rel=\"stylesheet\"").unwrap();
        // Tag sits between the existing <title> and </head>.
        assert!(link_at < close_at);
        assert!(out.contains("<title>Hi</title>"));
        // Body content is preserved.
        assert!(out.contains("<body>x</body>"));
    }

    #[test]
    fn empty_head_still_gets_a_link() {
        let html = "<html><head></head><body></body></html>";
        let out = inject_prod_head_assets(html, &assets_with_css());
        assert_eq!(
            out.as_ref(),
            "<html><head><link rel=\"stylesheet\" href=\"/assets/styles.css\"></head><body></body></html>"
        );
    }

    #[test]
    fn island_modules_appear_after_css_link_in_order() {
        let html = "<html><head></head><body/></html>";
        let assets = ProdHeadAssets {
            css_url: Some("/assets/styles.css".into()),
            island_module_urls: vec![
                "/assets/islands.js".into(),
                "/assets/extra.js".into(),
            ],
        };
        let out = inject_prod_head_assets(html, &assets);
        let out = out.as_ref();
        let css_at = out.find("rel=\"stylesheet\"").unwrap();
        let i1 = out.find("src=\"/assets/islands.js\"").unwrap();
        let i2 = out.find("src=\"/assets/extra.js\"").unwrap();
        assert!(css_at < i1);
        assert!(i1 < i2);
    }

    #[test]
    fn malformed_html_without_close_head_passes_through_unchanged() {
        let html = "<html><body>no head tag here</body></html>";
        let out = inject_prod_head_assets(html, &assets_full());
        assert_eq!(out.as_ref(), html);
    }

    #[test]
    fn non_html_output_passes_through_unchanged() {
        // feed.xml-style payload: no `</head>` ⇒ no edit.
        let xml = "<?xml version=\"1.0\"?><rss><channel><title>T</title></channel></rss>";
        let out = inject_prod_head_assets(xml, &assets_full());
        assert_eq!(out.as_ref(), xml);
    }

    #[test]
    fn empty_assets_pass_through_borrowed() {
        let html = "<html><head></head><body/></html>";
        let assets = ProdHeadAssets::default();
        let out = inject_prod_head_assets(html, &assets);
        // Should be Borrowed (no allocation).
        match out {
            Cow::Borrowed(_) => {}
            Cow::Owned(_) => panic!("empty assets must short-circuit to a borrowed passthrough"),
        }
    }

    #[test]
    fn idempotent_double_injection_does_not_duplicate_tags() {
        let html = "<html><head><title>X</title></head><body/></html>";
        let pass1 = inject_prod_head_assets(html, &assets_full()).into_owned();
        let pass2 = inject_prod_head_assets(&pass1, &assets_full());
        assert_eq!(pass2.as_ref(), pass1);
        // Sanity: each tag appears exactly once.
        assert_eq!(pass1.matches("rel=\"stylesheet\"").count(), 1);
        assert_eq!(pass1.matches("/assets/islands.js").count(), 1);
    }

    #[test]
    fn case_insensitive_close_head_is_anchored() {
        // Some upstream renderers emit `</HEAD>` (legacy / serializer
        // quirks). Inject before it just like the lowercase form.
        let html = "<HTML><HEAD><TITLE>X</TITLE></HEAD><BODY/></HTML>";
        let out = inject_prod_head_assets(html, &assets_with_css());
        let out = out.as_ref();
        let close_at = out.to_ascii_lowercase().find("</head>").unwrap();
        let link_at = out.find("<link rel=\"stylesheet\"").unwrap();
        assert!(link_at < close_at);
    }

    #[test]
    fn url_with_special_chars_is_attribute_escaped() {
        let html = "<html><head></head><body/></html>";
        let assets = ProdHeadAssets {
            css_url: Some("/a?x=1&y=\"2\"".into()),
            island_module_urls: vec![],
        };
        let out = inject_prod_head_assets(html, &assets);
        let out = out.as_ref();
        // & ⇒ &amp;, " ⇒ &quot;
        assert!(out.contains("href=\"/a?x=1&amp;y=&quot;2&quot;\""));
    }

    #[test]
    fn empty_assets_returns_borrowed_for_html_with_close_head() {
        let html = "<html><head><title>T</title></head><body/></html>";
        let out = inject_prod_head_assets(html, &ProdHeadAssets::default());
        match out {
            Cow::Borrowed(s) => assert_eq!(s, html),
            Cow::Owned(_) => panic!("must passthrough as borrowed when no assets requested"),
        }
    }

    #[test]
    fn css_link_tag_uses_canonical_form() {
        // Locks the byte form S4 will assert against once it wires the
        // pipeline up.
        assert_eq!(
            css_link_tag(STABLE_CSS_URL),
            "<link rel=\"stylesheet\" href=\"/assets/styles.css\">"
        );
    }

    #[test]
    fn island_module_script_tag_uses_canonical_form() {
        assert_eq!(
            island_module_script_tag(STABLE_ISLANDS_URL),
            "<script type=\"module\" src=\"/assets/islands.js\"></script>"
        );
    }

    #[test]
    fn needs_doctype_for_bare_html_document() {
        // The preact-render-to-string shell: starts with `<html …>`, no doctype.
        assert!(needs_html5_doctype("<html lang=\"en\"><head></head><body></body></html>"));
        assert!(needs_html5_doctype("<html><body>x</body></html>"));
    }

    #[test]
    fn no_doctype_when_already_present() {
        // Case-insensitive; never doubles an existing doctype.
        assert!(!needs_html5_doctype("<!doctype html><html></html>"));
        assert!(!needs_html5_doctype("<!DOCTYPE HTML><html></html>"));
        assert!(!needs_html5_doctype("<!Doctype html>\n<html lang=\"en\"></html>"));
    }

    #[test]
    fn no_doctype_for_non_html_output() {
        // feed.xml and other non-`<html>` payloads must round-trip untouched.
        assert!(!needs_html5_doctype("<rss/>"));
        assert!(!needs_html5_doctype("<?xml version=\"1.0\"?><feed></feed>"));
        assert!(!needs_html5_doctype("{\"json\":true}"));
        assert!(!needs_html5_doctype(""));
    }

    #[test]
    fn leading_whitespace_is_ignored_when_sniffing() {
        assert!(needs_html5_doctype("\n\t  <html></html>"));
        assert!(!needs_html5_doctype("  <!doctype html><html></html>"));
    }

    #[test]
    fn leading_bom_is_left_untouched() {
        // A BOM is not ASCII whitespace, so a BOM-prefixed doc does not match
        // `<html` — we never insert a doctype ahead of a BOM.
        assert!(!needs_html5_doctype("\u{feff}<html></html>"));
    }

    #[test]
    fn doctype_prefix_is_the_lowercase_html5_form() {
        assert_eq!(HTML5_DOCTYPE_PREFIX, "<!doctype html>\n");
    }
}
