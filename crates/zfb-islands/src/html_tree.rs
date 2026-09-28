//! Anchor-based HTML parse handle.
//!
//! [`HtmlTree`] stores markup for the server HTML injection pass. Callers
//! parse once, apply mutations, then serialize the result:
//!
//! ```rust,ignore
//! use zfb_islands::HtmlTree;
//! let mut tree = HtmlTree::parse("<html><head></head><body></body></html>");
//! // zfb-server::inject mutates the tree here.
//! let output = tree.serialize();
//! ```

use lol_html::errors::RewritingError;

/// An HTML document parse handle.
///
/// Construct with [`HtmlTree::parse`], mutate with [`zfb_server::inject`], then retrieve the
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
    /// This is the primary mutation seam used by helpers in `zfb-server::inject`. Callers build a [`lol_html::RewriteStrSettings`] with
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
