//! Conservative HTML minification helper for production build output.
//!
//! This module intentionally exposes only a narrow "minify this rendered
//! HTML" API. Build/config wiring can decide whether to call it; callers
//! should not thread through the upstream option matrix.

use minify_html::{minify, Cfg};

pub(crate) fn minify_rendered_html_bytes(html: &[u8]) -> Vec<u8> {
    minify(html, &conservative_cfg())
}

#[cfg(test)]
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
    #[ignore = "pending-feature: https://github.com/Takazudo/zudo-front-builder/issues/2033"]
    fn html_minify_preserves_mermaid_source_whitespace() {
        // `zfb-md-extras::mermaid` emits this exact shape (see
        // `crates/zfb-content/tests/fixtures/snapshots/08-mermaid.html` for the real
        // serializer's output): a `<div class="mermaid" data-mermaid="">` whose text
        // content is the mermaid DSL body run through the ordinary HTML text-node
        // escaper — `-->` serializes as `--&gt;`, matching hast-util-to-html's default
        // for an empty attribute value and for `>` in text. Mermaid's grammar is
        // newline-significant, so the minifier must not collapse whitespace inside it
        // the way it would for an ordinary `<div>`.
        //
        // `subgraph build["…"]` is the deliberately chosen shape: zudo-doc's client-side
        // `normalizeCollapsedMermaidSource` repair heuristic requires whitespace between a
        // subgraph id and what follows it, and here the id (`build`) is immediately
        // followed by `[` with no space — so a fix that only reinserts newlines between
        // top-level statements (but not inside/around bracketed labels sitting flush
        // against an id) would still leave this exact line unparseable. A weaker fixture
        // using only simple `A --> B` node declarations would pass under such a fix.
        let mermaid_body = concat!(
            "graph TD;\n",
            "  subgraph build[\"zfb build — your machine\"]\n",
            "    A[Content] --&gt; B[Bundle]\n",
            "  end\n",
            "  subgraph deploy[\"edge\"]\n",
            "    C[Deploy]\n",
            "  end\n"
        );
        let input = format!(
            "<div class=\"mermaid\" data-mermaid=\"\">{mermaid_body}</div>",
            mermaid_body = mermaid_body
        );

        let output = minify_rendered_html_string(&input);

        assert_eq!(
            output, input,
            "mermaid body must survive minification byte-identically \
             (newline positions preserved), got: {output}"
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
