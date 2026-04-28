//! Anchor-based HTML parse handle.
//!
//! [`HtmlTree`] is the central type introduced by the anchor-splicing
//! refactor (issue #65). Callers parse the page HTML **once** and then
//! pass a `&mut HtmlTree` to each of the rewrite/inject helpers:
//!
//! ```rust,ignore
//! use zfb_islands::html_tree::HtmlTree;
//! use zfb_islands::hydration::{rewrite_islands, inject_runtime_script_into_head};
//!
//! let mut tree = HtmlTree::parse(page_html);
//! rewrite_islands(&mut tree, &islands)?;
//! inject_runtime_script_into_head(&mut tree, runtime_url);
//! let output = tree.serialize();
//! ```
//!
//! Internally each mutation method drives a single `lol_html` rewriting
//! pass on the stored HTML, updating the buffer in place. Using lol_html's
//! CSS-selector-based element handlers is more robust than the previous
//! substring / regex scans:
//!
//! - `inject_runtime_script_into_head` uses the CSS selector `head` to
//!   locate the `<head>` element, so it never fires on a literal
//!   `</head>` string inside a code block.
//! - `rewrite_islands_in_attr_skeleton` uses the selector
//!   `div[data-zfb-island=""]` — it matches *elements*, not text content,
//!   so a Markdown code fence that happens to contain the literal string
//!   `data-zfb-island=""` will not be miscounted.
//!
//! The type is intentionally kept opaque so the implementation can
//! evolve (e.g. to a full in-memory DOM tree) without touching callers.

use lol_html::errors::RewritingError;

/// An HTML document parse handle.
///
/// Construct with [`HtmlTree::parse`], mutate with the helpers in
/// [`crate::hydration`] and [`zfb_server::inject`], then retrieve the
/// final markup with [`HtmlTree::serialize`].
///
/// The handle currently stores the document as a `String` and applies
/// each mutation via a `lol_html` rewriting pass. This is deliberately
/// left as an implementation detail so future optimisations (pooled
/// buffers, a single multi-handler pass) can land without breaking
/// callers.
#[derive(Debug)]
pub struct HtmlTree {
    pub(crate) html: String,
}

impl HtmlTree {
    /// Parse (wrap) an HTML string into a tree handle.
    ///
    /// `html` is moved in; no copy is made. The parse handle owns the
    /// document for the lifetime of the rewrite pipeline.
    pub fn parse(html: impl Into<String>) -> Self {
        Self { html: html.into() }
    }

    /// Consume the handle and return the (potentially rewritten) HTML
    /// string.
    pub fn serialize(self) -> String {
        self.html
    }

    /// Run a `lol_html` rewriting pass on the stored HTML.
    ///
    /// This is the primary mutation seam used by helpers in
    /// `hydration` (in this crate) and `zfb-server::inject` (external
    /// crate). Callers build a [`lol_html::RewriteStrSettings`] with
    /// their element and document content handlers and pass it here;
    /// the existing HTML is replaced with the rewriter's output.
    pub fn rewrite(
        &mut self,
        settings: lol_html::RewriteStrSettings<'_, '_>,
    ) -> Result<(), RewritingError> {
        let out = lol_html::rewrite_str(&self.html, settings)?;
        self.html = out;
        Ok(())
    }

    /// Mutable access to the raw HTML buffer.
    ///
    /// Exposed for callers (e.g. `zfb-server::inject`) that need to append
    /// to the document when no suitable DOM element is found (fragment /
    /// headerless HTML fallback path). Prefer the `rewrite` method for
    /// structured mutations.
    pub fn html_mut(&mut self) -> &mut String {
        &mut self.html
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_serialize_round_trips_html() {
        let html = "<!doctype html><html><body><p>hi</p></body></html>";
        let tree = HtmlTree::parse(html);
        assert_eq!(tree.serialize(), html);
    }

    #[test]
    fn parse_accepts_string_and_str() {
        let from_str = HtmlTree::parse("<p>a</p>");
        let from_string = HtmlTree::parse(String::from("<p>a</p>"));
        assert_eq!(from_str.serialize(), from_string.serialize());
    }
}
