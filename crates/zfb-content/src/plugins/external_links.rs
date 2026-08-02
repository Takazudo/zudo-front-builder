//! Mark external links with `target` and `rel` attributes.
//!
//! Rust port of npm `rehype-external-links`. Two visitors implement the
//! same classification + rewrite policy over disjoint halves of a
//! document's links (see "Coverage decisions" below for the partition):
//!
//! - [`ExternalLinksPlugin`] (hast phase) walks the hast tree and treats
//!   every top-level, structurally-visible `<a href="...">` — the shape
//!   `mdast_to_hast` renders an ordinary markdown link as.
//! - [`JsxNestedExternalLinks`] (mdast phase) walks the mdast tree and
//!   treats every markdown `[label](url)` link reached under an MDX-JSX
//!   ancestor — the exact complement `ExternalLinksPlugin` cannot see.
//!
//! For every external link, both visitors:
//!
//! - Set `target` (default `"_blank"`).
//! - Merge `rel` tokens (default `["noopener", "noreferrer"]`) with any
//!   existing `rel` attribute, deduplicating and preserving order. A
//!   markdown link can never carry a pre-existing `rel` (unlike a hast
//!   `<a>`, which might come from raw HTML), so [`JsxNestedExternalLinks`]
//!   always merges against an empty existing set.
//!
//! ## Same-origin classification
//!
//! An href is **external** when the href parses as an absolute URL *and*:
//!
//! - `site` is absent → any HTTP/HTTPS absolute URL is external.
//! - `site` is present → URL origin (scheme + host + port) differs from
//!   `site`'s origin. Absolute URLs whose origin matches `site` are
//!   **internal** and left unchanged.
//!
//! `mailto:`, `tel:`, `javascript:`, and other non-HTTP(S) absolute URLs
//! are **always left unchanged** — only `http:` and `https:` links are
//! candidates.
//!
//! Relative URLs (`/internal/`, `./relative.mdx`, `#anchor`) are always
//! internal. Both visitors share this classification via [`is_external`].
//!
//! ## Coverage decisions
//!
//! - **Markdown links only.** `[label](url)` reached under JSX — via a
//!   literal MDX-JSX element (`<Note>…</Note>`) or a container directive
//!   that expands to one (`:::note … :::`; directive expansion runs
//!   before this pass) — is covered by [`JsxNestedExternalLinks`]. An
//!   author-written literal JSX anchor (`<a href="...">`) is **not**
//!   covered, anywhere — top-level or nested. This matches
//!   `rehype-external-links` in JS (which also "cannot inspect JSX"),
//!   #2224's authored-JSX-out-of-scope ruling, and keeps top-level
//!   output byte-identical: treating a nested literal `<a>` while
//!   leaving a top-level one alone would be incoherent, and silently
//!   overriding an author's explicit `target=` would be hostile. Because
//!   markdown `Link.url` is always a plain string, an expression-valued
//!   `href` never arises for this pass — documented non-goal.
//! - **HTML-path blind spot, refined.** `Pipeline::run` /
//!   `run_with_context` (the HTML-serializer path) never apply
//!   [`JsxNestedExternalLinks`] — nested markdown links render there as
//!   lossy TEXT (the `reconstruct_jsx` fallback), so there is no `<a>` to
//!   protect. Epic #2222 originally speculated the HTML path was the
//!   affected surface; the real gap is the JSX path (MDX compile /
//!   `mdx_to_jsx_module_with_pipeline`), where nested links DO render as
//!   anchors — this module's split targets that corrected surface.
//!   Documented blind spot, not a bug.
//! - **Footnote-definition partition.** A `FootnoteDefinition`'s body
//!   renders once, in the document-level footnote section — a
//!   structurally hast-visible location `ExternalLinksPlugin` reaches
//!   regardless of where the definition was authored. So
//!   [`JsxNestedExternalLinks`] (via `rewrite_jsx_nested`) resets
//!   JSX-nested tracking at a `FootnoteDefinition` boundary: an ordinary
//!   link in the body is left untouched here, for the hast plugin to
//!   treat exactly once, while JSX authored INSIDE the body re-arms (it
//!   renders nowhere else — the hast walk never reaches it).

use markdown::mdast::Node as MdastNode;

use zfb_md_ast::mdx_jsx::{jsx_text_element, rewrite_jsx_nested};

use crate::pipeline::{HastNode, HastVisitor, MdastVisitor};

/// Configuration passed to [`ExternalLinksPlugin`] at construction time.
///
/// Mirrors `markdown.externalLinks` in `zfb.config.ts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalLinksConfig {
    /// `rel` tokens to set on external links.
    /// Default: `["noopener", "noreferrer"]`.
    pub rel: Vec<String>,
    /// `target` value for external links.
    /// Default: `"_blank"`.
    pub target: String,
}

impl Default for ExternalLinksConfig {
    fn default() -> Self {
        Self {
            rel: vec!["noopener".to_string(), "noreferrer".to_string()],
            target: "_blank".to_string(),
        }
    }
}

/// Hast visitor that rewrites external `<a>` elements.
///
/// Construct via [`ExternalLinksPlugin::new`].
pub struct ExternalLinksPlugin {
    config: ExternalLinksConfig,
    /// Parsed origin from the configured `site` URL, if provided.
    /// `None` when `site` is absent — any HTTP/HTTPS URL is treated as
    /// external (graceful fallback documented in issue #257).
    site_origin: Option<String>,
}

impl ExternalLinksPlugin {
    /// Create a new plugin.
    ///
    /// `config` controls `target` and `rel`. `site` is the canonical
    /// site URL (e.g. `"https://example.com"`); when `None`, every
    /// absolute HTTP/HTTPS href is treated as external.
    #[must_use]
    pub fn new(config: ExternalLinksConfig, site: Option<&str>) -> Self {
        let site_origin = site.and_then(extract_origin);
        Self {
            config,
            site_origin,
        }
    }
}

impl HastVisitor for ExternalLinksPlugin {
    fn visit(&mut self, node: &mut HastNode) {
        // Partition (see module docs "Coverage decisions"): this hast
        // walk only ever reaches TOP-LEVEL, structurally-visible `<a>`
        // elements. A markdown link reached under an MDX-JSX ancestor
        // (`<Note>[label](url)</Note>`, or a `:::note` directive that
        // expanded to one) is lowered to an opaque `HastNode::JsxRaw`
        // string before this visitor runs — that half is covered by
        // [`JsxNestedExternalLinks`] (mdast phase) instead. An
        // author-written literal JSX anchor (`<a href="...">`) never
        // becomes a hast `<a>` element at all and is out of scope
        // everywhere, matching `rehype-external-links` in JS, which
        // also only sees the hast layer and cannot inspect JSX.
        match node {
            HastNode::Root { children } | HastNode::Element { children, .. } => {
                // First recurse into children so inner <a> elements are
                // processed even when nested inside another element.
                for c in children.iter_mut() {
                    self.visit(c);
                }
            }
            _ => return,
        }
        // After recursion, apply the rewrite to the current node if it is
        // an external <a>.
        if let HastNode::Element { tag, attrs, .. } = node {
            if tag != "a" {
                return;
            }
            let href = attrs
                .iter()
                .find(|(k, _)| k == "href")
                .map(|(_, v)| v.as_str());
            let Some(href) = href else { return };
            if !is_external(href, self.site_origin.as_deref()) {
                return;
            }
            // Apply target.
            set_attr(attrs, "target", &self.config.target);
            // Merge rel tokens.
            let existing: Vec<String> = attrs
                .iter()
                .find(|(k, _)| k == "rel")
                .map(|(_, v)| {
                    v.split_ascii_whitespace()
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let merged = merge_rel_tokens(&existing, &self.config.rel);
            set_attr(attrs, "rel", &merged);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the origin (`scheme://host[:port]`) from an absolute URL string,
/// or `None` when the string is not a valid absolute URL with an HTTP(S)
/// scheme.
fn extract_origin(url: &str) -> Option<String> {
    let (scheme, rest) = split_scheme(url)?;
    if !is_http_scheme(scheme) {
        return None;
    }
    // rest starts with `//`; skip it.
    let rest = rest.strip_prefix("//")?;
    // authority ends at the first `/`, `?`, `#`, or end of string.
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    // Strip userinfo (`user:pass@`) if present — origin never includes it.
    let host_and_port = if let Some(at) = authority.rfind('@') {
        &authority[at + 1..]
    } else {
        authority
    };
    Some(format!("{}://{}", scheme, host_and_port))
}

/// Split `"scheme://..."` into `("scheme", "//...")`.
/// Returns `None` when no `:` is present or the part before `:` is empty.
fn split_scheme(url: &str) -> Option<(&str, &str)> {
    let colon = url.find(':')?;
    let scheme = &url[..colon];
    if scheme.is_empty() {
        return None;
    }
    Some((scheme, &url[colon + 1..]))
}

fn is_http_scheme(scheme: &str) -> bool {
    // Case-insensitive per RFC 3986 §3.1.
    matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https")
}

/// Classify an href as external given an optional site origin.
///
/// Rules (from issue #257):
/// - Relative URLs → always internal.
/// - Non-HTTP(S) absolute URLs → always internal (mailto:, tel:, …).
/// - `site` absent → any HTTP/HTTPS absolute URL is external.
/// - `site` present → external iff the URL's origin ≠ site origin.
fn is_external(href: &str, site_origin: Option<&str>) -> bool {
    // Relative hrefs start with `/`, `./`, `../`, `#`, or are bare paths.
    // A reliable signal for an absolute URL is the presence of `://` after
    // a non-empty scheme that starts before any `/`.
    let (scheme, _rest) = match split_scheme(href) {
        Some(pair) => pair,
        // No colon → relative URL.
        None => return false,
    };
    // A relative path like `/foo:bar` would have scheme `"/foo"` which
    // starts with `/` — guard against that.
    if scheme.starts_with('/') {
        return false;
    }
    if !is_http_scheme(scheme) {
        return false;
    }
    // It's an absolute HTTP(S) URL. Now apply site-origin check.
    match site_origin {
        None => true,
        Some(origin) => {
            // Compare origins case-insensitively (host names are
            // case-insensitive per RFC 1034).
            let href_origin = extract_origin(href);
            href_origin
                .map(|o| !origins_equal(&o, origin))
                .unwrap_or(false)
        }
    }
}

/// Compare two origin strings case-insensitively.
///
/// Note: default-port equivalence (`https://example.com:443` ==
/// `https://example.com`) is **not** normalized here. In practice,
/// content authors almost never write explicit default ports in
/// same-origin self-links, so the omission is acceptable for now.
/// If this becomes a real issue, strip default ports before comparison.
fn origins_equal(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Merge `existing` rel tokens with `configured` tokens, deduplicating.
///
/// Order: existing tokens first (preserving author intent), then any
/// configured tokens not already present. Result is joined with a single
/// space, matching the JS `rehype-external-links` output.
fn merge_rel_tokens(existing: &[String], configured: &[String]) -> String {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<&str> = Vec::new();
    for t in existing {
        let lower = t.to_ascii_lowercase();
        if seen.insert(lower) {
            out.push(t.as_str());
        }
    }
    for t in configured {
        let lower = t.to_ascii_lowercase();
        if seen.insert(lower) {
            out.push(t.as_str());
        }
    }
    out.join(" ")
}

/// Set or replace an attribute value on an element's attribute list.
fn set_attr(attrs: &mut Vec<(String, String)>, key: &str, val: &str) {
    for (k, v) in attrs.iter_mut() {
        if k == key {
            *v = val.to_string();
            return;
        }
    }
    attrs.push((key.to_string(), val.to_string()));
}

/// JSX-nested-phase visitor for the external-links pass (zfb#2249).
///
/// Registered as an [`MdastVisitor`] in the pipeline's mdast phase —
/// `Pipeline::apply_mdast_visitors_with_context` /
/// `Pipeline::apply_mdast_visitors` in `zfb-content::pipeline` — LAST,
/// after `jsx_nested_image_dimensions` (wiring landed in #2247). For
/// every JSX-nested markdown `Link` whose `url` is external (the same
/// [`is_external`] classification the paired hast plugin uses), replaces
/// the node in place with an `<a>` [`MdxJsxTextElement`](markdown::mdast::MdxJsxTextElement)
/// carrying `href`, an optional `title`, `target`, and merged `rel` — see
/// the module docs' "Coverage decisions" for the exact partition and
/// rationale. Internal / relative / same-origin / non-HTTP(S) links are
/// left as ordinary `Link` nodes, so they emit byte-identically via the
/// native `Link` arm in `mdx_jsx_emit.rs`.
///
/// Constructed from the SAME `config` + `site` as the paired
/// [`ExternalLinksPlugin`] (see `Pipeline::add_external_links`) —
/// `site_origin` reuses this module's own [`extract_origin`], since both
/// types live in the same module.
pub struct JsxNestedExternalLinks {
    config: ExternalLinksConfig,
    site_origin: Option<String>,
}

impl JsxNestedExternalLinks {
    /// Create a new stub. `site` mirrors [`ExternalLinksPlugin::new`]'s
    /// `site` parameter — the canonical site URL, or `None` when every
    /// absolute HTTP/HTTPS href should be treated as external.
    #[must_use]
    pub fn new(config: ExternalLinksConfig, site: Option<&str>) -> Self {
        let site_origin = site.and_then(extract_origin);
        Self {
            config,
            site_origin,
        }
    }
}

impl MdastVisitor for JsxNestedExternalLinks {
    /// Context-free by construction (mirrors the hast sibling's
    /// availability): this plugin needs no `BuildContext`, so the
    /// default `visit_with_context` → `visit` delegation
    /// (`MdastVisitor`'s trait default) is exactly right and is not
    /// overridden here.
    fn visit(&mut self, node: &mut MdastNode) {
        // Precompute the per-call constants once: `target`/`rel` never
        // vary per link, and a markdown `Link` can never carry a
        // pre-existing `rel` to merge against (unlike the hast plugin's
        // possibly-raw-HTML `<a>`), so the merge is always against an
        // empty existing set.
        let target = self.config.target.clone();
        let rel = merge_rel_tokens(&[], &self.config.rel);
        let site_origin = self.site_origin.clone();
        rewrite_jsx_nested(node, &mut |n| {
            let MdastNode::Link(link) = n else { return };
            if !is_external(&link.url, site_origin.as_deref()) {
                return;
            }
            // Attr order: href, title (when Some), target, rel — matches
            // the treated top-level hast output order exactly (href,
            // title?, then set_attr-appended target, rel).
            let mut attrs = vec![("href".to_string(), link.url.clone())];
            if let Some(title) = &link.title {
                attrs.push(("title".to_string(), title.clone()));
            }
            attrs.push(("target".to_string(), target.clone()));
            attrs.push(("rel".to_string(), rel.clone()));
            let children = std::mem::take(&mut link.children);
            let position = link.position.clone();
            *n = jsx_text_element("a", attrs, children, position);
        });
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn a(href: &str) -> HastNode {
        HastNode::Element {
            tag: "a".to_string(),
            attrs: vec![("href".to_string(), href.to_string())],
            children: vec![HastNode::Text("link".to_string())],
            void: false,
        }
    }

    fn a_with_rel(href: &str, rel: &str) -> HastNode {
        HastNode::Element {
            tag: "a".to_string(),
            attrs: vec![
                ("href".to_string(), href.to_string()),
                ("rel".to_string(), rel.to_string()),
            ],
            children: vec![HastNode::Text("link".to_string())],
            void: false,
        }
    }

    fn root(children: Vec<HastNode>) -> HastNode {
        HastNode::Root { children }
    }

    fn first_child(node: &HastNode) -> &HastNode {
        let HastNode::Root { children } = node else {
            unreachable!("expected root");
        };
        &children[0]
    }

    fn attr<'a>(node: &'a HastNode, key: &str) -> Option<&'a str> {
        let HastNode::Element { attrs, .. } = node else {
            return None;
        };
        attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn plugin_no_site() -> ExternalLinksPlugin {
        ExternalLinksPlugin::new(ExternalLinksConfig::default(), None)
    }

    fn plugin_with_site(site: &str) -> ExternalLinksPlugin {
        ExternalLinksPlugin::new(ExternalLinksConfig::default(), Some(site))
    }

    // --- no-site mode (any HTTP/HTTPS is external) -------------------------

    #[test]
    fn rewrites_http_link_without_site() {
        let mut tree = root(vec![a("https://other.com/foo")]);
        plugin_no_site().visit(&mut tree);
        let link = first_child(&tree);
        assert_eq!(attr(link, "target"), Some("_blank"));
        assert_eq!(attr(link, "rel"), Some("noopener noreferrer"));
    }

    #[test]
    fn rewrites_http_link_without_site_http_scheme() {
        let mut tree = root(vec![a("http://other.com/foo")]);
        plugin_no_site().visit(&mut tree);
        let link = first_child(&tree);
        assert_eq!(attr(link, "target"), Some("_blank"));
    }

    #[test]
    fn leaves_relative_link_unchanged() {
        let before = root(vec![a("/internal/page")]);
        let mut tree = before.clone();
        plugin_no_site().visit(&mut tree);
        assert_eq!(tree, before);
    }

    #[test]
    fn leaves_relative_dotslash_link_unchanged() {
        let before = root(vec![a("./relative.mdx")]);
        let mut tree = before.clone();
        plugin_no_site().visit(&mut tree);
        assert_eq!(tree, before);
    }

    #[test]
    fn leaves_anchor_link_unchanged() {
        let before = root(vec![a("#anchor")]);
        let mut tree = before.clone();
        plugin_no_site().visit(&mut tree);
        assert_eq!(tree, before);
    }

    #[test]
    fn leaves_mailto_unchanged() {
        let before = root(vec![a("mailto:foo@bar.com")]);
        let mut tree = before.clone();
        plugin_no_site().visit(&mut tree);
        assert_eq!(tree, before);
    }

    #[test]
    fn leaves_tel_unchanged() {
        let before = root(vec![a("tel:+1234567890")]);
        let mut tree = before.clone();
        plugin_no_site().visit(&mut tree);
        assert_eq!(tree, before);
    }

    // --- site set → same-origin is internal --------------------------------

    #[test]
    fn same_origin_not_rewritten() {
        let mut tree = root(vec![a("https://example.com/foo")]);
        plugin_with_site("https://example.com").visit(&mut tree);
        let link = first_child(&tree);
        assert_eq!(
            attr(link, "target"),
            None,
            "same-origin must not get target"
        );
        assert_eq!(attr(link, "rel"), None, "same-origin must not get rel");
    }

    #[test]
    fn different_origin_is_rewritten() {
        let mut tree = root(vec![a("https://other.com/foo")]);
        plugin_with_site("https://example.com").visit(&mut tree);
        let link = first_child(&tree);
        assert_eq!(attr(link, "target"), Some("_blank"));
        assert_eq!(attr(link, "rel"), Some("noopener noreferrer"));
    }

    #[test]
    fn same_origin_with_path_not_rewritten() {
        let mut tree = root(vec![a("https://example.com/docs/page")]);
        plugin_with_site("https://example.com").visit(&mut tree);
        assert_eq!(attr(first_child(&tree), "target"), None);
    }

    // --- config options -----------------------------------------------------

    #[test]
    fn custom_target() {
        let cfg = ExternalLinksConfig {
            target: "_self".to_string(),
            rel: vec!["noopener".to_string()],
        };
        let mut p = ExternalLinksPlugin::new(cfg, None);
        let mut tree = root(vec![a("https://other.com/")]);
        p.visit(&mut tree);
        assert_eq!(attr(first_child(&tree), "target"), Some("_self"));
    }

    #[test]
    fn custom_rel_tokens() {
        let cfg = ExternalLinksConfig {
            target: "_blank".to_string(),
            rel: vec!["nofollow".to_string(), "noopener".to_string()],
        };
        let mut p = ExternalLinksPlugin::new(cfg, None);
        let mut tree = root(vec![a("https://other.com/")]);
        p.visit(&mut tree);
        assert_eq!(attr(first_child(&tree), "rel"), Some("nofollow noopener"));
    }

    // --- existing rel merging -----------------------------------------------

    #[test]
    fn existing_rel_merged_without_duplicates() {
        let mut tree = root(vec![a_with_rel("https://other.com/", "nofollow")]);
        plugin_no_site().visit(&mut tree);
        // nofollow (existing) + noopener + noreferrer (configured), no dupes.
        assert_eq!(
            attr(first_child(&tree), "rel"),
            Some("nofollow noopener noreferrer")
        );
    }

    #[test]
    fn no_duplicate_when_existing_overlaps_configured() {
        let mut tree = root(vec![a_with_rel(
            "https://other.com/",
            "noopener noreferrer",
        )]);
        plugin_no_site().visit(&mut tree);
        // Configured tokens already present → no duplication.
        assert_eq!(attr(first_child(&tree), "rel"), Some("noopener noreferrer"));
    }

    // --- origin helpers ----------------------------------------------------

    #[test]
    fn extract_origin_http() {
        assert_eq!(
            extract_origin("http://example.com/path"),
            Some("http://example.com".to_string())
        );
    }

    #[test]
    fn extract_origin_with_port() {
        assert_eq!(
            extract_origin("https://example.com:8080/path"),
            Some("https://example.com:8080".to_string())
        );
    }

    #[test]
    fn extract_origin_mailto_is_none() {
        assert_eq!(extract_origin("mailto:foo@bar.com"), None);
    }

    #[test]
    fn is_external_relative_is_false() {
        assert!(!is_external("/internal/", None));
        assert!(!is_external("./relative.mdx", None));
        assert!(!is_external("#anchor", None));
    }

    #[test]
    fn is_external_mailto_is_false() {
        assert!(!is_external("mailto:foo@bar.com", None));
    }

    #[test]
    fn is_external_http_no_site_is_true() {
        assert!(is_external("https://example.com/", None));
    }

    #[test]
    fn is_external_same_origin_is_false() {
        assert!(!is_external(
            "https://example.com/foo",
            Some("https://example.com")
        ));
    }

    #[test]
    fn is_external_different_origin_is_true() {
        assert!(is_external(
            "https://other.com/",
            Some("https://example.com")
        ));
    }

    #[test]
    fn merge_rel_deduplicates_case_insensitive() {
        let existing = vec!["Noopener".to_string()];
        let configured = vec!["noopener".to_string(), "noreferrer".to_string()];
        // "Noopener" and "noopener" are the same token (case-insensitive).
        assert_eq!(
            merge_rel_tokens(&existing, &configured),
            "Noopener noreferrer"
        );
    }

    // --- edge cases in URL parsing -----------------------------------------

    #[test]
    fn uppercase_scheme_treated_as_external() {
        // RFC 3986 §3.1: scheme is case-insensitive; HTTPS == https.
        let mut tree = root(vec![a("HTTPS://other.com/")]);
        plugin_no_site().visit(&mut tree);
        assert_eq!(attr(first_child(&tree), "target"), Some("_blank"));
    }

    #[test]
    fn protocol_relative_url_is_not_rewritten() {
        // `//example.com/path` has no scheme colon → treated as relative.
        let before = root(vec![a("//example.com/path")]);
        let mut tree = before.clone();
        plugin_no_site().visit(&mut tree);
        assert_eq!(tree, before);
    }

    #[test]
    fn url_with_userinfo_classifies_correctly() {
        // `http://user:pass@host/` — same host as site origin.
        let mut tree = root(vec![a("http://user:pass@example.com/path")]);
        plugin_with_site("http://example.com").visit(&mut tree);
        // userinfo is stripped before origin comparison; should be same origin.
        assert_eq!(
            attr(first_child(&tree), "target"),
            None,
            "userinfo-prefixed same-origin must not be rewritten"
        );
    }

    #[test]
    fn data_url_is_not_rewritten() {
        let before = root(vec![a("data:text/html,<h1>hi</h1>")]);
        let mut tree = before.clone();
        plugin_no_site().visit(&mut tree);
        assert_eq!(tree, before);
    }

    // --- JsxNestedExternalLinks (zfb#2249) ----------------------------------
    //
    // Level 1/unit coverage for the mdast-phase visitor. End-to-end MDX
    // compile coverage (directive expansion, resolve_links ordering,
    // link-validation interaction, footnote partition) lives in
    // `tests/external_links_jsx_descent_mdx_path.rs`.

    use markdown::mdast::{AttributeContent, AttributeValue, Link, MdxJsxFlowElement, Text};

    use crate::pipeline::BuildContext;

    /// `<Note>[label](url)</Note>` — a single markdown link nested one
    /// level under an MDX-JSX flow element, the minimal shape
    /// `rewrite_jsx_nested` fires on.
    fn note_wrapping_link(url: &str, title: Option<&str>) -> MdastNode {
        MdastNode::MdxJsxFlowElement(MdxJsxFlowElement {
            children: vec![MdastNode::Link(Link {
                children: vec![MdastNode::Text(Text {
                    value: "label".to_string(),
                    position: None,
                })],
                position: None,
                url: url.to_string(),
                title: title.map(str::to_string),
            })],
            position: None,
            name: Some("Note".to_string()),
            attributes: vec![],
        })
    }

    /// The sole nested child of a `note_wrapping_link` tree, after a
    /// visit — either still a `Link` (untouched) or the synthesized `a`
    /// `MdxJsxTextElement` (treated).
    fn nested_child(tree: &MdastNode) -> &MdastNode {
        let MdastNode::MdxJsxFlowElement(el) = tree else {
            unreachable!("expected MdxJsxFlowElement root, got {tree:?}");
        };
        &el.children[0]
    }

    /// Read a `Literal`-valued JSX attribute by name from an
    /// `MdxJsxTextElement`. Returns `None` for a non-JSX node, a missing
    /// attribute, or a non-literal value.
    fn jsx_attr<'a>(node: &'a MdastNode, key: &str) -> Option<&'a str> {
        let MdastNode::MdxJsxTextElement(el) = node else {
            return None;
        };
        el.attributes.iter().find_map(|a| {
            let AttributeContent::Property(p) = a else {
                return None;
            };
            if p.name != key {
                return None;
            }
            match &p.value {
                Some(AttributeValue::Literal(s)) => Some(s.as_str()),
                _ => None,
            }
        })
    }

    fn jsx_nested_plugin_no_site() -> JsxNestedExternalLinks {
        JsxNestedExternalLinks::new(ExternalLinksConfig::default(), None)
    }

    fn jsx_nested_plugin_with_site(site: &str) -> JsxNestedExternalLinks {
        JsxNestedExternalLinks::new(ExternalLinksConfig::default(), Some(site))
    }

    #[test]
    fn jsx_nested_external_link_without_site_gets_target_and_rel() {
        let mut tree = note_wrapping_link("https://other.com/foo", None);
        jsx_nested_plugin_no_site().visit(&mut tree);
        let child = nested_child(&tree);
        assert_eq!(jsx_attr(child, "href"), Some("https://other.com/foo"));
        assert_eq!(jsx_attr(child, "target"), Some("_blank"));
        assert_eq!(jsx_attr(child, "rel"), Some("noopener noreferrer"));
    }

    #[test]
    fn jsx_nested_same_origin_link_untouched_when_site_configured() {
        let before = note_wrapping_link("https://example.com/foo", None);
        let mut tree = before.clone();
        jsx_nested_plugin_with_site("https://example.com").visit(&mut tree);
        assert_eq!(
            tree, before,
            "same-origin nested link must stay an untouched Link node"
        );
    }

    #[test]
    fn jsx_nested_different_origin_link_treated_when_site_configured() {
        let mut tree = note_wrapping_link("https://other.com/foo", None);
        jsx_nested_plugin_with_site("https://example.com").visit(&mut tree);
        let child = nested_child(&tree);
        assert_eq!(jsx_attr(child, "target"), Some("_blank"));
        assert_eq!(jsx_attr(child, "rel"), Some("noopener noreferrer"));
    }

    #[test]
    fn jsx_nested_relative_link_untouched() {
        let before = note_wrapping_link("./relative.mdx", None);
        let mut tree = before.clone();
        jsx_nested_plugin_no_site().visit(&mut tree);
        assert_eq!(tree, before);
    }

    #[test]
    fn jsx_nested_mailto_untouched() {
        let before = note_wrapping_link("mailto:foo@bar.com", None);
        let mut tree = before.clone();
        jsx_nested_plugin_no_site().visit(&mut tree);
        assert_eq!(tree, before);
    }

    #[test]
    fn jsx_nested_custom_target_and_rel_honored() {
        let cfg = ExternalLinksConfig {
            target: "_self".to_string(),
            rel: vec!["nofollow".to_string(), "noopener".to_string()],
        };
        let mut tree = note_wrapping_link("https://other.com/", Some("A title"));
        JsxNestedExternalLinks::new(cfg, None).visit(&mut tree);
        let child = nested_child(&tree);
        assert_eq!(jsx_attr(child, "href"), Some("https://other.com/"));
        assert_eq!(jsx_attr(child, "title"), Some("A title"));
        assert_eq!(jsx_attr(child, "target"), Some("_self"));
        assert_eq!(jsx_attr(child, "rel"), Some("nofollow noopener"));
    }

    #[test]
    fn jsx_nested_attr_order_is_href_title_target_rel() {
        let mut tree = note_wrapping_link("https://other.com/", Some("T"));
        jsx_nested_plugin_no_site().visit(&mut tree);
        let MdastNode::MdxJsxTextElement(a) = nested_child(&tree) else {
            panic!("expected the nested Link to be replaced with an MdxJsxTextElement");
        };
        let names: Vec<&str> = a
            .attributes
            .iter()
            .map(|attr| {
                let AttributeContent::Property(p) = attr else {
                    panic!("expected a Property attribute, got {attr:?}");
                };
                p.name.as_str()
            })
            .collect();
        assert_eq!(names, vec!["href", "title", "target", "rel"]);
    }

    #[test]
    fn jsx_nested_treated_link_preserves_children_and_position() {
        let pos = markdown::unist::Position::new(1, 1, 0, 1, 20, 19);
        let mut tree = MdastNode::MdxJsxFlowElement(MdxJsxFlowElement {
            children: vec![MdastNode::Link(Link {
                children: vec![MdastNode::Text(Text {
                    value: "click here".to_string(),
                    position: None,
                })],
                position: Some(pos.clone()),
                url: "https://other.com/".to_string(),
                title: None,
            })],
            position: None,
            name: Some("Note".to_string()),
            attributes: vec![],
        });
        jsx_nested_plugin_no_site().visit(&mut tree);
        let MdastNode::MdxJsxTextElement(a) = nested_child(&tree) else {
            panic!("expected the nested Link to be replaced with an MdxJsxTextElement");
        };
        assert_eq!(a.position, Some(pos));
        assert_eq!(a.children.len(), 1);
        assert!(matches!(&a.children[0], MdastNode::Text(t) if t.value == "click here"));
    }

    #[test]
    fn jsx_nested_visit_is_a_pure_delegate_of_visit_with_context() {
        // Context-free by construction (module docs / struct doc):
        // `MdastVisitor::visit_with_context`'s trait default delegates to
        // `visit`, and this plugin does not override it. Exercise that
        // default path directly so a future accidental override is
        // caught by this test rather than silently changing behavior.
        let mut tree = note_wrapping_link("https://other.com/foo", None);
        let mut ctx = BuildContext::for_paths("/proj/a.mdx", "/proj", "/proj/public");
        jsx_nested_plugin_no_site().visit_with_context(&mut tree, &mut ctx);
        let child = nested_child(&tree);
        assert_eq!(jsx_attr(child, "target"), Some("_blank"));
    }
}
