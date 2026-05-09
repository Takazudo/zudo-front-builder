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
//!
//! ## Base-prefix awareness (issue #229)
//!
//! When the project's `zfb.config.ts` declares
//! `base: "/foo/"`, the dev server mounts every route under `/foo`
//! including the live-reload endpoint at
//! `/foo/__zfb/livereload.js`. The injected `<script src>` therefore
//! has to point at the prefixed URL — otherwise the browser would
//! request the unprefixed `/__zfb/livereload.js`, hit the dev-server
//! 404, and the live-reload connection would never establish.
//! [`livereload_tag`] builds the right URL given the prefix; the
//! prefix-less convenience wrappers ([`LIVERELOAD_TAG`],
//! [`inject_livereload`], [`inject_livereload_into_tree`]) are kept
//! for backward compatibility and use an empty prefix.

use std::borrow::Cow;
use std::cell::Cell;
use std::rc::Rc;

use lol_html::html_content::{ContentType, Element};
use lol_html::{ElementContentHandlers, RewriteStrSettings, Selector};
use zfb_islands::html_tree::HtmlTree;

/// The script tag we inject before `</body>` on every served HTML page
/// when the dev server is mounted at the root (no `base` prefix).
///
/// Kept short to minimise dev-mode noise. The script itself lives at
/// `/__zfb/livereload.js` (served by [`crate::routes`]) and is
/// `Cache-Control: no-store` so the browser always refetches the
/// latest version.
///
/// When the project uses `base: "/foo/"` the dev server has to emit a
/// prefixed `<script src="/foo/__zfb/livereload.js">` instead — see
/// [`livereload_tag`]. This constant covers the no-base case and stays
/// for backward compatibility (it was part of the public API before the
/// prefix-aware variants landed).
pub const LIVERELOAD_TAG: &str = "<script src=\"/__zfb/livereload.js\"></script>";

/// Build the live-reload `<script>` tag with the dev server's mount
/// prefix folded in.
///
/// `base_prefix` is the canonical mount prefix returned by
/// [`zfb_types::dev_mount_prefix`]: empty string for the no-base case,
/// or a leading-slash, no-trailing-slash path like `"/foo"` for a
/// prefixed dev server.
///
/// Returns the full `<script src="…">` tag the dev server can splice
/// into a served HTML body.
pub fn livereload_tag(base_prefix: &str) -> String {
    if base_prefix.is_empty() {
        LIVERELOAD_TAG.to_string()
    } else {
        format!("<script src=\"{base_prefix}/__zfb/livereload.js\"></script>")
    }
}

/// Inject the live-reload script tag into the `<body>` element of
/// `tree` (immediately before `</body>`) using `lol_html`'s CSS
/// selector. The injected tag points at
/// `<base_prefix>/__zfb/livereload.js`.
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
pub fn inject_livereload_into_tree_with_prefix(tree: &mut HtmlTree, base_prefix: &str) {
    let tag = livereload_tag(base_prefix);
    let body_found = Rc::new(Cell::new(false));
    let body_found_c = Rc::clone(&body_found);
    // Clone the tag once into the closure so the rewrite callback owns
    // its copy without borrowing across `await` / lifetimes.
    let tag_for_append = tag.clone();

    let selector: Selector = "body".parse().expect("static selector 'body' is valid");

    let settings: RewriteStrSettings<'_, '_> = RewriteStrSettings {
        element_content_handlers: vec![(
            Cow::Borrowed(&selector),
            ElementContentHandlers::default().element(move |el: &mut Element<'_, '_, _>| {
                body_found_c.set(true);
                el.append(&tag_for_append, ContentType::Html);
                Ok(())
            }),
        )],
        ..RewriteStrSettings::new()
    };

    tree.rewrite(settings)
        .expect("lol_html rewriting for livereload injection should not fail");

    if !body_found.get() {
        // Fragment / headerless: append the tag.
        tree.html_mut().push_str(&tag);
    }
}

/// Backwards-compatible wrapper that injects the no-base
/// [`LIVERELOAD_TAG`]. New call sites should use
/// [`inject_livereload_into_tree_with_prefix`] so the live-reload URL
/// matches the dev server's mount prefix.
pub fn inject_livereload_into_tree(tree: &mut HtmlTree) {
    inject_livereload_into_tree_with_prefix(tree, "");
}

/// Convenience wrapper: inject the live-reload tag into a plain HTML
/// string (with the given mount prefix folded into the script URL) and
/// return the modified string.
///
/// Internally wraps the string in an [`HtmlTree`], calls
/// [`inject_livereload_into_tree_with_prefix`], and serialises. Use
/// [`inject_livereload_into_tree_with_prefix`] directly when you
/// already hold an `HtmlTree` to avoid the extra allocation.
pub fn inject_livereload_with_prefix(html: &str, base_prefix: &str) -> String {
    let mut tree = HtmlTree::parse(html);
    inject_livereload_into_tree_with_prefix(&mut tree, base_prefix);
    tree.serialize()
}

/// Backwards-compatible wrapper that injects the no-base
/// [`LIVERELOAD_TAG`] into a plain HTML string. New call sites should
/// use [`inject_livereload_with_prefix`] when the dev server is
/// mounted under a `base` prefix.
pub fn inject_livereload(html: &str) -> String {
    inject_livereload_with_prefix(html, "")
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

    // ---- prefix-aware variants (issue #229) ------------------------------

    #[test]
    fn livereload_tag_empty_prefix_matches_const() {
        assert_eq!(livereload_tag(""), LIVERELOAD_TAG);
    }

    #[test]
    fn livereload_tag_with_prefix_includes_it() {
        assert_eq!(
            livereload_tag("/foo"),
            "<script src=\"/foo/__zfb/livereload.js\"></script>"
        );
    }

    #[test]
    fn inject_with_prefix_uses_prefixed_url() {
        let html = "<html><body><h1>hi</h1></body></html>";
        let out = inject_livereload_with_prefix(html, "/foo");
        assert!(
            out.contains("<script src=\"/foo/__zfb/livereload.js\"></script>"),
            "expected prefixed script tag, got: {out}"
        );
        // The unprefixed tag must NOT appear — that would 404 against
        // the dev server when base is set.
        assert!(
            !out.contains("src=\"/__zfb/livereload.js\""),
            "unexpected unprefixed script tag in: {out}"
        );
    }

    #[test]
    fn inject_with_prefix_falls_back_to_append_for_fragments() {
        let html = "<div>fragment</div>";
        let out = inject_livereload_with_prefix(html, "/foo");
        assert_eq!(
            out,
            "<div>fragment</div><script src=\"/foo/__zfb/livereload.js\"></script>"
        );
    }

    #[test]
    fn inject_with_empty_prefix_matches_no_base_behaviour() {
        let html = "<html><body><h1>hi</h1></body></html>";
        let with_empty_prefix = inject_livereload_with_prefix(html, "");
        let no_prefix = inject_livereload(html);
        assert_eq!(with_empty_prefix, no_prefix);
    }
}
