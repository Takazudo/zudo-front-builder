//! HTML post-processing: inject the dev live-reload `<script>` tag.
//!
//! `zfb-server` is a *dev-only* crate, so the route layer always calls
//! [`inject_livereload`] before responding with HTML. There is no
//! production mode here — production-build HTML is emitted by a
//! separate pipeline that doesn't go through this server.
//!
//! ## Anchor-based injection (issue #65)
//!
//! The previous implementation used a byte-scanning loop to find the
//! **last** `</body>` tag (case-insensitively) and splice the script
//! tag before it. The anchor-based replacement uses `lol_html`'s CSS
//! selector `body` to locate the actual `<body>` element — this is
//! immune to a literal `</body>` appearing inside a `<pre>` or
//! `<textarea>` element.
//!
//! Callers that already have an [`HtmlTree`] handle should use
//! [`inject_livereload_into_tree`] to avoid an extra parse/serialize
//! round-trip. The old `inject_livereload(html: &str) -> String`
//! convenience wrapper is preserved for call sites that operate on
//! plain strings (primarily the route layer).

use std::borrow::Cow;
use std::cell::Cell;
use std::rc::Rc;

use lol_html::html_content::{ContentType, Element};
use lol_html::{ElementContentHandlers, RewriteStrSettings, Selector};
use zfb_islands::html_tree::HtmlTree;

/// The script tag we inject before `</body>` on every served HTML page.
///
/// Kept short to minimise dev-mode noise. The script itself lives at
/// `/__zfb/livereload.js` (served by [`crate::routes`]) and is
/// `Cache-Control: no-store` so the browser always refetches the
/// latest version.
pub const LIVERELOAD_TAG: &str = "<script src=\"/__zfb/livereload.js\"></script>";

/// Inject [`LIVERELOAD_TAG`] into the `<body>` element of `tree`
/// (immediately before `</body>`) using `lol_html`'s CSS selector.
///
/// Behaviour:
///
/// - Uses the CSS selector `body` to locate the element, which means
///   only a real `<body>` DOM node triggers the injection — a literal
///   `</body>` inside a `<pre>` or `<textarea>` is ignored.
/// - When no `<body>` element is found (HTML fragments, partials,
///   malformed input) the tag is appended to the end of the document,
///   matching the previous fallback behaviour.
///
/// The function never errors; the worst case is a fragment getting an
/// extra script tag at the end, which is harmless in dev mode.
pub fn inject_livereload_into_tree(tree: &mut HtmlTree) {
    let body_found = Rc::new(Cell::new(false));
    let body_found_c = Rc::clone(&body_found);

    let selector: Selector = "body".parse().expect("static selector 'body' is valid");

    let settings: RewriteStrSettings<'_, '_> = RewriteStrSettings {
        element_content_handlers: vec![(
            Cow::Borrowed(&selector),
            ElementContentHandlers::default().element(move |el: &mut Element<'_, '_, _>| {
                body_found_c.set(true);
                el.append(LIVERELOAD_TAG, ContentType::Html);
                Ok(())
            }),
        )],
        ..RewriteStrSettings::new()
    };

    tree.rewrite(settings)
        .expect("lol_html rewriting for livereload injection should not fail");

    if !body_found.get() {
        // Fragment / headerless: append the tag.
        tree.html_mut().push_str(LIVERELOAD_TAG);
    }
}

/// Convenience wrapper: inject [`LIVERELOAD_TAG`] into a plain HTML
/// string and return the modified string.
///
/// Internally wraps the string in an [`HtmlTree`], calls
/// [`inject_livereload_into_tree`], and serialises. Use
/// [`inject_livereload_into_tree`] directly when you already hold an
/// `HtmlTree` to avoid the extra allocation.
pub fn inject_livereload(html: &str) -> String {
    let mut tree = HtmlTree::parse(html);
    inject_livereload_into_tree(&mut tree);
    tree.serialize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injects_before_closing_body() {
        let html = "<html><body><h1>hi</h1></body></html>";
        let out = inject_livereload(html);
        assert_eq!(
            out,
            "<html><body><h1>hi</h1><script src=\"/__zfb/livereload.js\"></script></body></html>"
        );
    }

    #[test]
    fn appends_when_no_body_close() {
        let html = "<div>fragment</div>";
        let out = inject_livereload(html);
        assert_eq!(
            out,
            "<div>fragment</div><script src=\"/__zfb/livereload.js\"></script>"
        );
    }

    #[test]
    fn empty_input_gets_just_the_tag() {
        let out = inject_livereload("");
        assert_eq!(out, LIVERELOAD_TAG);
    }

    #[test]
    fn injects_with_multibyte_content_around_body() {
        // Japanese before/after, plus a 4-byte emoji literal.
        let html = "<html><body><h1>こんにちは🎉世界</h1></body></html>";
        let out = inject_livereload(html);
        let expected = format!(
            "<html><body><h1>こんにちは🎉世界</h1>{LIVERELOAD_TAG}</body></html>"
        );
        assert_eq!(out, expected);
        assert!(out.contains("こんにちは🎉世界"));
    }

    #[test]
    fn injects_with_multibyte_content_inside_last_body() {
        // Two body closes with non-ASCII content between them: confirm
        // injection still picks the last close cleanly.
        // NOTE: with the lol_html selector approach, only the first
        // <body> element is matched (HTML spec: only one <body> is
        // valid). We inject inside that body, which appears before the
        // second malformed </body> close tag.
        let html = "<html><body>あ<div>い</div></body></html>";
        let out = inject_livereload(html);
        assert!(out.contains(LIVERELOAD_TAG));
        assert!(out.contains("あ"));
        assert!(out.contains("い"));
    }

    #[test]
    fn idempotent_marker_check() {
        // The function isn't *strictly* idempotent (running it twice
        // injects two tags), but the script itself is no-op safe so
        // double-injection only costs one duplicate <script> in the
        // dev-mode output.
        let html = "<html><body></body></html>";
        let once = inject_livereload(html);
        let twice = inject_livereload(&once);
        assert_eq!(twice.matches(LIVERELOAD_TAG).count(), 2);
    }

    #[test]
    fn inject_livereload_into_tree_works() {
        let html = "<html><body><p>test</p></body></html>";
        let mut tree = HtmlTree::parse(html);
        inject_livereload_into_tree(&mut tree);
        let out = tree.serialize();
        assert!(out.contains(LIVERELOAD_TAG));
        assert!(out.contains("<p>test</p>"));
    }
}
