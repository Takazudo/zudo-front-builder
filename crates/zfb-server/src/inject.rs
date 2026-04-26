//! HTML post-processing: inject the dev live-reload `<script>` tag.
//!
//! `zfb-server` is a *dev-only* crate, so the route layer always calls
//! [`inject_livereload`] before responding with HTML. There is no
//! production mode here — production-build HTML is emitted by a
//! separate pipeline that doesn't go through this server.

/// The script tag we inject before `</body>` on every served HTML page.
///
/// Kept short to minimise dev-mode noise. The script itself lives at
/// `/__zfb/livereload.js` (served by [`crate::routes`]) and is
/// `Cache-Control: no-store` so the browser always refetches the
/// latest version.
pub const LIVERELOAD_TAG: &str = "<script src=\"/__zfb/livereload.js\"></script>";

/// Insert [`LIVERELOAD_TAG`] immediately before the **last** `</body>`
/// tag in `html`.
///
/// Behaviour:
///
/// - Matching is case-insensitive (`</BODY>`, `</Body>`, `</body>` all
///   work).
/// - When multiple `</body>` tags appear (which is invalid HTML but
///   does happen with malformed input or hand-written fragments) we
///   inject before the **last** one — that's the close that visually
///   matters.
/// - When no `</body>` appears at all (HTML fragments, partials,
///   malformed input) we append the tag to the end of the string.
///
/// The function never errors; the worst case is a fragment getting an
/// extra script tag at the end, which is harmless in dev mode.
pub fn inject_livereload(html: &str) -> String {
    // Find the byte offset of the LAST </body> match, case-insensitive.
    // Walk the lowercase form to keep the match position aligned with
    // the original bytes (ASCII tag, so byte positions match 1:1).
    let needle = "</body>";
    let lower = html.to_ascii_lowercase();

    let mut last: Option<usize> = None;
    let mut search_from = 0usize;
    while let Some(rel) = lower[search_from..].find(needle) {
        let abs = search_from + rel;
        last = Some(abs);
        search_from = abs + needle.len();
    }

    match last {
        Some(idx) => {
            let mut out = String::with_capacity(html.len() + LIVERELOAD_TAG.len());
            out.push_str(&html[..idx]);
            out.push_str(LIVERELOAD_TAG);
            out.push_str(&html[idx..]);
            out
        }
        None => {
            let mut out = String::with_capacity(html.len() + LIVERELOAD_TAG.len());
            out.push_str(html);
            out.push_str(LIVERELOAD_TAG);
            out
        }
    }
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
    fn case_insensitive_match() {
        let html = "<BODY>x</BODY>";
        let out = inject_livereload(html);
        assert!(
            out.contains(LIVERELOAD_TAG),
            "expected script tag, got: {out}"
        );
        // Tag inserted before </BODY> (preserving original case).
        assert!(out.ends_with("</BODY>"));
        // Mixed case too.
        let html2 = "<Body>x</Body>";
        let out2 = inject_livereload(html2);
        assert!(out2.ends_with("</Body>"));
        assert!(out2.contains(LIVERELOAD_TAG));
    }

    #[test]
    fn injects_before_last_body_when_multiple() {
        // Malformed but realistic: editor accidentally pastes two body
        // closes. We must inject before the LAST one so the script is
        // still inside the final body in the rendered DOM.
        let html = "<body>a</body><body>b</body>";
        let out = inject_livereload(html);
        // The first </body> is preserved; injection happens at the
        // last one.
        let expected = format!("<body>a</body><body>b{LIVERELOAD_TAG}</body>");
        assert_eq!(out, expected);
    }

    #[test]
    fn idempotent_marker_check() {
        // The function isn't *strictly* idempotent (running it twice
        // injects two tags), but the script itself is no-op safe so
        // double-injection only costs one duplicate <script> in the
        // dev-mode output. Document that here so future maintainers
        // don't bake-in idempotence assumptions.
        let html = "<body></body>";
        let once = inject_livereload(html);
        let twice = inject_livereload(&once);
        assert_eq!(twice.matches(LIVERELOAD_TAG).count(), 2);
    }
}
