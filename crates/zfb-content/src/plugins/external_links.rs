//! Mark external links with `target` and `rel` attributes.
//!
//! Rust port of npm `rehype-external-links`. Walks the hast tree and, for
//! every `<a href="...">` whose href is classified as **external**:
//!
//! - Sets `target` (default `"_blank"`).
//! - Merges `rel` tokens (default `["noopener", "noreferrer"]`) with any
//!   existing `rel` attribute, deduplicating and preserving order.
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
//! internal.

use crate::pipeline::{HastNode, HastVisitor};

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
        // Note: MDX JSX anchors (`<a href="...">` written inline in .mdx files)
        // are lowered to `HastNode::JsxRaw` (opaque strings) before this visitor
        // runs, so they are not rewritten. Only standard-markdown links
        // (`[label](https://...)`) — which become `HastNode::Element { tag: "a", … }`
        // — are in scope. This matches `rehype-external-links` in JS, which also
        // only sees the hast layer and cannot inspect JSX.
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
    let authority_end = rest
        .find(['/', '?', '#'])
        .unwrap_or(rest.len());
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
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https"
    )
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
        attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
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
        assert_eq!(attr(link, "target"), None, "same-origin must not get target");
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
        assert_eq!(
            attr(first_child(&tree), "rel"),
            Some("noopener noreferrer")
        );
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
}
