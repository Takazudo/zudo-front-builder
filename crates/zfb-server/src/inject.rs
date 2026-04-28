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
///
/// This is a convenience wrapper around [`inject_livereload_into`]
/// that allocates and returns a new `String`. Prefer
/// [`inject_livereload_into`] when you already hold a buffer you want
/// to reuse across calls.
pub fn inject_livereload(html: &str) -> String {
    let mut buf = String::with_capacity(html.len() + LIVERELOAD_TAG.len());
    inject_livereload_into(html, &mut buf);
    buf
}

/// In-place variant of [`inject_livereload`]: writes the result into
/// `buf` instead of allocating a new `String`.
///
/// `buf` is **cleared** before writing so the caller does not need to
/// reset it between calls. After this function returns `buf` contains
/// exactly the same content that [`inject_livereload`] would return.
///
/// # Example
///
/// ```
/// use zfb_server::inject::{inject_livereload_into, LIVERELOAD_TAG};
///
/// let mut buf = String::new();
/// inject_livereload_into("<html><body>hi</body></html>", &mut buf);
/// assert!(buf.contains(LIVERELOAD_TAG));
///
/// // Reuse the buffer: buf is cleared before each call.
/// inject_livereload_into("<html><body>world</body></html>", &mut buf);
/// assert!(buf.contains("world"));
/// assert_eq!(buf.matches(LIVERELOAD_TAG).count(), 1);
/// ```
pub fn inject_livereload_into(html: &str, buf: &mut String) {
    buf.clear();
    buf.reserve(html.len() + LIVERELOAD_TAG.len());

    // Find the byte offset of the LAST </body> match, case-insensitive.
    // We scan the bytes directly rather than allocating a full
    // lowercase copy of the page — the previous implementation cost
    // one full copy per page render in the dev server.
    //
    // The needle (`</body>`) is pure ASCII so byte-wise comparison is
    // safe even when the surrounding HTML contains UTF-8: we never
    // start a match in the middle of a multi-byte sequence (none of
    // the bytes `<`, `/`, ASCII letters, `>` appear as continuation
    // bytes in valid UTF-8).
    let needle: &[u8] = b"</body>";
    let bytes = html.as_bytes();

    let mut last: Option<usize> = None;
    if bytes.len() >= needle.len() {
        let upper_bound = bytes.len() - needle.len();
        let mut i = 0;
        while i <= upper_bound {
            if bytes[i] == b'<' {
                let mut matches = true;
                for (j, nb) in needle.iter().enumerate() {
                    let hb = bytes[i + j];
                    let hb_lc = if hb.is_ascii_uppercase() { hb + 32 } else { hb };
                    if hb_lc != *nb {
                        matches = false;
                        break;
                    }
                }
                if matches {
                    last = Some(i);
                    i += needle.len();
                    continue;
                }
            }
            i += 1;
        }
    }

    match last {
        Some(idx) => {
            buf.push_str(&html[..idx]);
            buf.push_str(LIVERELOAD_TAG);
            buf.push_str(&html[idx..]);
        }
        None => {
            buf.push_str(html);
            buf.push_str(LIVERELOAD_TAG);
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

    /// Round 2: prove the byte-scan splice point is a UTF-8 char
    /// boundary even when multi-byte content (e.g. Japanese, emoji)
    /// surrounds the `</body>` needle. The needle itself is pure ASCII
    /// and none of its bytes are UTF-8 continuation bytes (0x80-0xBF),
    /// so the scanner can never start a match inside a multi-byte
    /// sequence — but verify the resulting String contains exactly the
    /// expected bytes.
    #[test]
    fn injects_with_multibyte_content_around_body() {
        // Japanese before/after, plus a 4-byte emoji literal.
        let html = "<html><body><h1>こんにちは🎉世界</h1></body></html>";
        let out = inject_livereload(html);
        let expected = format!(
            "<html><body><h1>こんにちは🎉世界</h1>{LIVERELOAD_TAG}</body></html>"
        );
        assert_eq!(out, expected);
        // Byte-level: the result must still be valid UTF-8 (String
        // construction would have panicked otherwise) and the original
        // multi-byte sequences must survive verbatim.
        assert!(out.contains("こんにちは🎉世界"));
    }

    #[test]
    fn injects_with_multibyte_content_inside_last_body() {
        // Two body closes with non-ASCII content between them: confirm
        // injection still picks the last close cleanly.
        let html = "<body>あ</body><body>い</body>";
        let out = inject_livereload(html);
        let expected =
            format!("<body>あ</body><body>い{LIVERELOAD_TAG}</body>");
        assert_eq!(out, expected);
    }

    // ---- inject_livereload_into -------------------------------------------------

    #[test]
    fn into_variant_produces_same_result_as_allocating_variant() {
        let cases = [
            "<html><body><h1>hi</h1></body></html>",
            "<div>fragment</div>",
            "",
            "<BODY>x</BODY>",
            "<body>a</body><body>b</body>",
        ];
        for html in &cases {
            let expected = inject_livereload(html);
            let mut buf = String::new();
            inject_livereload_into(html, &mut buf);
            assert_eq!(buf, expected, "mismatch for input: {html:?}");
        }
    }

    #[test]
    fn into_variant_clears_existing_buffer_contents() {
        let mut buf = String::from("stale content that should be overwritten");
        inject_livereload_into("<body></body>", &mut buf);
        // Must not contain "stale".
        assert!(!buf.contains("stale"), "buf was not cleared: {buf}");
        assert!(buf.contains(LIVERELOAD_TAG));
    }

    #[test]
    fn into_variant_reusable_across_calls() {
        let mut buf = String::new();
        inject_livereload_into("<body>first</body>", &mut buf);
        assert!(buf.contains("first"));
        assert_eq!(buf.matches(LIVERELOAD_TAG).count(), 1);

        inject_livereload_into("<body>second</body>", &mut buf);
        assert!(buf.contains("second"));
        assert!(!buf.contains("first"), "old content leaked into reused buf");
        assert_eq!(buf.matches(LIVERELOAD_TAG).count(), 1);
    }
}
