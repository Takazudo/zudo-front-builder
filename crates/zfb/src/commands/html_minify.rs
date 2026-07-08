//! Conservative HTML minification helper for production build output.
//!
//! This module intentionally exposes only a narrow "minify this rendered
//! HTML" API. Build/config wiring can decide whether to call it; callers
//! should not thread through the upstream option matrix.

// This sub-issue adds the helper before the later production build wiring
// starts calling it.
#![allow(dead_code)]

use minify_html::{minify, Cfg};

pub(crate) fn minify_rendered_html_bytes(html: &[u8]) -> Vec<u8> {
    minify(html, &conservative_cfg())
}

pub(crate) fn minify_rendered_html_string(html: &str) -> String {
    String::from_utf8(minify_rendered_html_bytes(html.as_bytes()))
        .expect("minify-html returned non-UTF-8 for UTF-8 HTML input")
}

fn conservative_cfg() -> Cfg {
    let mut cfg = Cfg::new();
    cfg.keep_comments = true;
    cfg.keep_closing_tags = true;
    cfg.keep_html_and_head_opening_tags = true;
    cfg.keep_input_type_text_attr = true;
    cfg.minify_css = false;
    cfg.minify_js = false;
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_minify_reduces_basic_whitespace() {
        let input = "<main>\n  <h1> Hello </h1>\n  <p>  Welcome   home. </p>\n</main>";

        let output = minify_rendered_html_string(input);

        assert!(output.len() < input.len(), "{output}");
        assert_eq!(output, "<main><h1>Hello</h1><p>Welcome home.</p></main>");
    }

    #[test]
    fn html_minify_preserves_comments() {
        let output =
            minify_rendered_html_string("<main>  <!-- keep this comment -->  <p>Hi</p></main>");

        assert!(output.contains("<!-- keep this comment -->"), "{output}");
    }

    #[test]
    fn html_minify_preserves_html_and_head_opening_tags() {
        let output = minify_rendered_html_string(
            "<!doctype html><html><head><title>Hi</title></head><body>Hello</body></html>",
        );

        assert!(output.contains("<html><head>"), "{output}");
    }

    #[test]
    fn html_minify_preserves_optional_closing_tags() {
        let output = minify_rendered_html_string("<ul>\n  <li>One</li>\n  <li>Two</li>\n</ul>");

        assert!(output.contains("</li>"), "{output}");
        assert_eq!(output.matches("</li>").count(), 2, "{output}");
    }

    #[test]
    fn html_minify_keeps_input_type_text_attr() {
        let output = minify_rendered_html_string(r#"<form><input type="text" value="x"></form>"#);

        assert!(output.contains(r#"<input type=text value=x>"#), "{output}");
    }

    #[test]
    fn html_minify_does_not_corrupt_raw_text_contexts() {
        let input = concat!(
            "<pre>  keep\n    spacing  </pre>",
            "<script>if (a < b) { console.log(\"  keep js spacing  \"); }</script>",
            "<style>.x > .y { content: \"  keep css spacing  \"; }</style>"
        );

        let output = minify_rendered_html_string(input);

        assert!(
            output.contains("<pre>  keep\n    spacing  </pre>"),
            "{output}"
        );
        assert!(
            output
                .contains(r#"<script>if (a < b) { console.log("  keep js spacing  "); }</script>"#),
            "{output}"
        );
        assert!(
            output.contains(r#"<style>.x > .y { content: "  keep css spacing  "; }</style>"#),
            "{output}"
        );
    }

    #[test]
    fn html_minify_is_deterministic_for_fixed_input() {
        let input = "<html><head><title>Hi</title></head><body><p> Hello </p></body></html>";

        let first = minify_rendered_html_bytes(input.as_bytes());
        let second = minify_rendered_html_bytes(input.as_bytes());

        assert_eq!(first, second);
    }
}
