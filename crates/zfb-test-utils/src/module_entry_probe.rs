//! Helpers for checking the module entries declared by a served HTML page.
//!
//! A page can return `200` while one of its browser-requested module entries
//! still returns `404`.  The helpers in this module keep the assertion close
//! to the browser's URL resolution rules: callers pass the URL of the served
//! document (including any configured `base` path), and root-absolute sources
//! are resolved against that URL without stripping an already-prefixed path.

use anyhow::{Context, Result};
use html5ever::driver::ParseOpts;
use html5ever::parse_fragment;
use html5ever::tendril::TendrilSink;
use html5ever::{namespace_url, ns, LocalName, QualName};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use std::collections::HashSet;

/// One same-origin module script URL and the status returned by its probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleEntryProbe {
    /// The URL a browser would request for the module entry.
    pub url: reqwest::Url,
    /// The HTTP status returned by the entry probe.
    pub status: reqwest::StatusCode,
}

/// Resolve same-origin module script URLs declared by `html`.
///
/// `document_url` must be the URL that served the HTML, including its path.
/// This matters for a path-mounted dev server: if the HTML contains
/// `src="/docs/assets/client.js"`, resolving it against
/// `http://127.0.0.1:3000/docs/` preserves `/docs/assets/client.js`; it does
/// not treat `/docs` as an origin and accidentally probe `/assets/client.js`.
///
/// Cross-origin, inline, non-module, malformed, and non-HTTP(S) script
/// sources are ignored. The returned URLs are de-duplicated in document
/// order, matching the set of requests needed by a readiness assertion while
/// avoiding duplicate probes for repeated script tags.
pub fn module_entry_urls(html: &str, document_url: &str) -> Result<Vec<reqwest::Url>> {
    let document_url = reqwest::Url::parse(document_url)
        .with_context(|| format!("invalid served document URL: {document_url}"))?;

    let mut seen = HashSet::new();
    Ok(module_script_srcs(html)
        .into_iter()
        .filter_map(|src| resolve_same_origin_module_url(&document_url, &src))
        .filter(|url| seen.insert(url.clone()))
        .collect())
}

/// Probe every same-origin module entry declared by `html`.
///
/// The request method is `GET`, because the helper is intended to check the
/// URL the browser will actually fetch. A transport error includes the URL in
/// its returned error; an HTTP failure such as `404` is represented in the
/// corresponding [`ModuleEntryProbe`] so callers can report every declared
/// entry and its status together.
pub async fn probe_module_entries(
    client: &reqwest::Client,
    html: &str,
    document_url: &str,
) -> Result<Vec<ModuleEntryProbe>> {
    let urls = module_entry_urls(html, document_url)?;
    let mut probes = Vec::with_capacity(urls.len());

    for url in urls {
        let status = client
            .get(url.clone())
            .send()
            .await
            .with_context(|| format!("probe module entry {url}"))?
            .status();
        probes.push(ModuleEntryProbe { url, status });
    }

    Ok(probes)
}

/// Extract the raw `src` values from module script elements in document order.
///
/// This is kept private to the URL-resolving API so callers cannot accidentally
/// probe a raw root path and lose the served document's base context.
fn module_script_srcs(html: &str) -> Vec<String> {
    let context_name = QualName::new(None, ns!(html), LocalName::from("body"));
    let dom = parse_fragment(RcDom::default(), ParseOpts::default(), context_name, vec![])
        .one(html.to_string());
    let mut srcs = Vec::new();
    collect_module_script_srcs(&dom.document, &mut srcs);
    srcs
}

fn collect_module_script_srcs(node: &Handle, srcs: &mut Vec<String>) {
    if let NodeData::Element { name, attrs, .. } = &node.data {
        if name.local.as_ref() == "script" {
            let mut script_type = None;
            let mut src = None;
            for attr in attrs.borrow().iter() {
                match attr.name.local.as_ref() {
                    "type" => script_type = Some(attr.value.to_string()),
                    "src" => src = Some(attr.value.to_string()),
                    _ => {}
                }
            }

            if script_type
                .as_deref()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("module"))
            {
                if let Some(src) = src.map(|value| value.trim().to_owned()) {
                    if !src.is_empty() {
                        srcs.push(src);
                    }
                }
            }
        }
    }

    for child in node.children.borrow().iter() {
        collect_module_script_srcs(child, srcs);
    }
}

fn resolve_same_origin_module_url(document_url: &reqwest::Url, src: &str) -> Option<reqwest::Url> {
    let mut url = document_url.join(src).ok()?;
    if !matches!(url.scheme(), "http" | "https") || !same_origin(document_url, &url) {
        return None;
    }

    // Fragments are part of a DOM URL but never part of an HTTP request.
    url.set_fragment(None);
    Some(url)
}

fn same_origin(left: &reqwest::Url, right: &reqwest::Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCUMENT_URL: &str = "http://127.0.0.1:4173/docs/guide/";

    #[test]
    fn resolves_base_prefixed_module_entries_as_browser_requests() {
        let html = r#"
            <html><head>
              <script type="module" src="/docs/assets/islands.js"></script>
              <script type="module" src="/docs/assets/client/search.js?generation=2#entry"></script>
            </head></html>
        "#;

        let urls = module_entry_urls(html, DOCUMENT_URL).unwrap();

        assert_eq!(
            urls.iter().map(reqwest::Url::as_str).collect::<Vec<_>>(),
            [
                "http://127.0.0.1:4173/docs/assets/islands.js",
                "http://127.0.0.1:4173/docs/assets/client/search.js?generation=2",
            ]
        );
    }

    #[test]
    fn resolves_relative_entries_against_the_served_document_path() {
        let html = r#"<script type="module" src="../assets/relative.js"></script>"#;

        let urls = module_entry_urls(html, DOCUMENT_URL).unwrap();

        assert_eq!(
            urls.iter().map(reqwest::Url::as_str).collect::<Vec<_>>(),
            ["http://127.0.0.1:4173/docs/assets/relative.js"]
        );
    }

    #[test]
    fn ignores_cross_origin_non_module_and_inline_scripts() {
        let html = r#"
            <script type="module" src="https://cdn.example.test/vendor.js"></script>
            <script type="module" src="//other.example.test/remote.js"></script>
            <script type="text/javascript" src="/assets/classic.js"></script>
            <script src="/assets/no-type.js"></script>
            <script type="module">console.log("inline")</script>
            <script type="module" src="data:text/javascript,export default 1"></script>
            <script type="module" src="/assets/local.js"></script>
        "#;

        let urls = module_entry_urls(html, DOCUMENT_URL).unwrap();

        assert_eq!(
            urls.iter().map(reqwest::Url::as_str).collect::<Vec<_>>(),
            ["http://127.0.0.1:4173/assets/local.js"]
        );
    }

    #[test]
    fn accepts_html_attribute_order_and_module_type_whitespace() {
        let html = r#"
            <script src="/assets/first.js" data-test="1" TYPE=" module "></script>
            <script SRC="/assets/second.js" type="MODULE"></script>
        "#;

        let urls = module_entry_urls(html, DOCUMENT_URL).unwrap();

        assert_eq!(
            urls.iter().map(reqwest::Url::as_str).collect::<Vec<_>>(),
            [
                "http://127.0.0.1:4173/assets/first.js",
                "http://127.0.0.1:4173/assets/second.js",
            ]
        );
    }

    #[test]
    fn de_duplicates_repeated_module_entries_without_changing_order() {
        let html = r#"
            <script type="module" src="/assets/a.js"></script>
            <script type="module" src="/assets/a.js#second-tag"></script>
            <script type="module" src="/assets/b.js"></script>
        "#;

        let urls = module_entry_urls(html, DOCUMENT_URL).unwrap();

        assert_eq!(
            urls.iter().map(reqwest::Url::as_str).collect::<Vec<_>>(),
            [
                "http://127.0.0.1:4173/assets/a.js",
                "http://127.0.0.1:4173/assets/b.js",
            ]
        );
    }

    #[test]
    fn rejects_invalid_document_urls_before_parsing() {
        let error = module_entry_urls("<script type=module src=/assets/a.js>", "not a URL")
            .expect_err("invalid document URL must be reported");

        assert!(error.to_string().contains("invalid served document URL"));
    }
}
