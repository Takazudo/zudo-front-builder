//! A pure `url()` byte-span scanner over CSS text (issue #2314, epic #2311).
//!
//! Given a stylesheet, [`scan_css_urls`] reports **every** `url()` occurrence
//! as an exact byte span plus a decoded value — no classification, no
//! filesystem access, no policy. Deciding which references are in scope
//! (relative vs `data:`/absolute/scheme) belongs to the caller; keeping this
//! layer classification-free lets a scanner bug be isolated from a policy bug.
//!
//! ## Input contract
//!
//! The scanner runs on the Tailwind output text *after* the trailing
//! `/*# sourceMappingURL=... */` comment has been stripped (done where the
//! output is read — the comment is trailing, so stripping never shifts
//! earlier byte offsets; scanner spans and sourcemap positions therefore
//! describe the same text).
//!
//! ## Output contract — per occurrence
//!
//! - [`CssUrlOccurrence::value_span`]: the exact byte span of the raw value —
//!   everything between `url(` and its matching `)`, trimmed of surrounding
//!   CSS whitespace, **including** the quotes when present. Splicing a
//!   replacement at this span leaves every other byte of the stylesheet
//!   identical, so a caller can rewrite without reserialising.
//! - [`CssUrlOccurrence::decoded`]: the value with quotes removed and CSS
//!   escapes decoded (hex escapes with their optional whitespace terminator,
//!   literal-character escapes, and string line continuations).
//! - [`CssUrlOccurrence::quote`]: none / single / double, so a rewriter can
//!   preserve the original quoting style.
//!
//! ## Tokenisation rules (CSS Syntax Level 3)
//!
//! This is a state machine, not a regex — a regex cannot correctly skip
//! comment and string contexts. Per the spec:
//!
//! - `/* comments */` are skipped wholesale; `url(` inside one never matches.
//! - String tokens at the top level are skipped; `content: "url(x)"` never
//!   matches.
//! - The function name matches ASCII case-insensitively (`URL(` works) and
//!   must be a standalone ident: `imageurl(` is a function named `imageurl`,
//!   `3url(` is a dimension token, and `@url(`/`#url(` are an at-keyword and
//!   a hash token — none of them matches. An escaped spelling (`\75 rl(`)
//!   decodes to `url` and does match, per ident decoding rules.
//! - `url( "..." )` tokenises as a function containing a string token; the
//!   reported span is the string token's bytes (quotes included).
//! - An unquoted value follows url-token rules: it cannot contain unescaped
//!   whitespace, quotes, parentheses, or non-printable code points — any of
//!   those makes it a bad-url-token, which is consumed and **not** reported.

use std::ops::Range;

/// The quoting style of a `url()` value, preserved so a rewriter can splice a
/// replacement in the same style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlQuote {
    /// `url(foo.png)` — a raw url-token.
    None,
    /// `url('foo.png')`.
    Single,
    /// `url("foo.png")`.
    Double,
}

/// One `url()` occurrence found by [`scan_css_urls`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssUrlOccurrence {
    /// Exact byte span of the raw value between `url(` and the matching `)`,
    /// whitespace-trimmed, including quotes when present.
    pub value_span: Range<usize>,
    /// The value with quotes removed and CSS escapes decoded.
    pub decoded: String,
    /// Quoting style of the original value.
    pub quote: UrlQuote,
}

/// Scan `css` and report every `url()` occurrence in source order.
///
/// Pure function: no filesystem access, no classification of the values.
/// Comments and string tokens are skipped per CSS Syntax Level 3, so a
/// `url(` inside either never matches; bad url-tokens (unquoted values
/// containing unescaped whitespace/quotes/parens) are consumed but not
/// reported.
pub fn scan_css_urls(css: &str) -> Vec<CssUrlOccurrence> {
    let bytes = css.as_bytes();
    let len = bytes.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < len {
        let c = bytes[i];
        if c == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            i = skip_comment(bytes, i);
        } else if c == b'"' || c == b'\'' {
            i = consume_string_token(css, i).end;
        } else if (c == b'@' || c == b'#') && starts_word(bytes, i + 1) {
            // At-keyword (`@url(...)`) and hash (`#url(...)`) tokens absorb
            // the following ident sequence, so no url() function can start
            // inside one — consume the whole token to avoid a false match.
            let (_, end) = consume_ident_sequence(css, i + 1);
            i = end;
        } else if is_word_byte(c) || (c == b'\\' && is_valid_escape(bytes, i)) {
            let (name, end) = consume_ident_sequence(css, i);
            if end < len && bytes[end] == b'(' && name.eq_ignore_ascii_case("url") {
                let (occurrence, next) = consume_url_value(css, end + 1);
                if let Some(occurrence) = occurrence {
                    out.push(occurrence);
                }
                i = next;
            } else {
                i = end;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Bytes that participate in the ident-ish run used for function-name
/// detection. Digits are deliberately included even though they cannot start
/// an ident: consuming `3url` as one run is what keeps a dimension token's
/// unit (`3url(` per spec) from being misread as a `url(` function.
fn is_word_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c >= 0x80
}

/// Whether a word run ([`is_word_byte`] chars / escapes) starts at `i`.
fn starts_word(bytes: &[u8], i: usize) -> bool {
    match bytes.get(i) {
        Some(&c) => is_word_byte(c) || (c == b'\\' && is_valid_escape(bytes, i)),
        None => false,
    }
}

fn is_css_whitespace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'\x0C')
}

fn is_css_newline(c: u8) -> bool {
    matches!(c, b'\n' | b'\r' | b'\x0C')
}

/// Non-printable code points that turn an unquoted url into a bad-url-token
/// (CSS Syntax §4.3.6).
fn is_non_printable(c: u8) -> bool {
    matches!(c, 0x00..=0x08 | 0x0B | 0x0E..=0x1F | 0x7F)
}

/// A `\` at `i` starts a valid escape iff it is not followed by a newline
/// (an EOF after `\` still decodes, to U+FFFD).
fn is_valid_escape(bytes: &[u8], i: usize) -> bool {
    debug_assert_eq!(bytes[i], b'\\');
    match bytes.get(i + 1) {
        Some(&next) => !is_css_newline(next),
        None => true,
    }
}

/// Skip a `/* ... */` comment starting at `i`; unterminated comments run to
/// EOF per spec.
fn skip_comment(bytes: &[u8], i: usize) -> usize {
    debug_assert!(bytes[i] == b'/' && bytes[i + 1] == b'*');
    let mut k = i + 2;
    while k + 1 < bytes.len() {
        if bytes[k] == b'*' && bytes[k + 1] == b'/' {
            return k + 2;
        }
        k += 1;
    }
    bytes.len()
}

/// How a string token ended.
#[derive(PartialEq, Eq)]
enum StringEnd {
    /// The matching close quote was found.
    Quote,
    /// EOF before the close quote (parse error; the token is still returned).
    Eof,
    /// An unescaped newline — a bad-string-token. The newline itself is not
    /// consumed (`end` points at it).
    Newline,
}

struct StringToken {
    decoded: String,
    /// Byte index one past the token (past the close quote for `Quote`; at
    /// the newline for `Newline`; `len` for `Eof`).
    end: usize,
    terminator: StringEnd,
}

/// Consume a string token whose open quote sits at `open` (CSS Syntax §4.3.5),
/// decoding escapes and `\`-newline line continuations.
fn consume_string_token(css: &str, open: usize) -> StringToken {
    let bytes = css.as_bytes();
    let quote = bytes[open];
    let mut decoded = String::new();
    let mut k = open + 1;
    loop {
        let Some(&c) = bytes.get(k) else {
            return StringToken {
                decoded,
                end: css.len(),
                terminator: StringEnd::Eof,
            };
        };
        if c == quote {
            return StringToken {
                decoded,
                end: k + 1,
                terminator: StringEnd::Quote,
            };
        }
        if is_css_newline(c) {
            return StringToken {
                decoded,
                end: k,
                terminator: StringEnd::Newline,
            };
        }
        if c == b'\\' {
            match bytes.get(k + 1) {
                // EOF after `\`: per spec, append nothing; the next loop
                // iteration reports the EOF.
                None => {
                    k += 1;
                }
                // `\` + newline inside a string is a line continuation —
                // both characters are consumed, nothing is appended.
                Some(&next) if is_css_newline(next) => {
                    k += 2;
                    if next == b'\r' && bytes.get(k) == Some(&b'\n') {
                        k += 1;
                    }
                }
                Some(_) => {
                    let (ch, next_index) = consume_escape(css, k);
                    decoded.push(ch);
                    k = next_index;
                }
            }
            continue;
        }
        let ch = char_at(css, k);
        decoded.push(ch);
        k += ch.len_utf8();
    }
}

/// Decode one escape whose `\` sits at `backslash`, which the caller has
/// already checked is a valid escape (not `\`+newline). Returns the decoded
/// character and the index one past the escape (CSS Syntax §4.3.7).
fn consume_escape(css: &str, backslash: usize) -> (char, usize) {
    let bytes = css.as_bytes();
    debug_assert_eq!(bytes[backslash], b'\\');
    let Some(&first) = bytes.get(backslash + 1) else {
        return ('\u{FFFD}', css.len());
    };
    if first.is_ascii_hexdigit() {
        let mut k = backslash + 1;
        let mut value: u32 = 0;
        let mut digits = 0;
        while digits < 6 {
            let Some(&c) = bytes.get(k) else { break };
            let Some(d) = (c as char).to_digit(16) else {
                break;
            };
            value = value * 16 + d;
            digits += 1;
            k += 1;
        }
        // One trailing whitespace character terminates the escape and is
        // consumed with it (`\r\n` counts as a single character).
        if let Some(&c) = bytes.get(k) {
            if is_css_whitespace(c) {
                k += 1;
                if c == b'\r' && bytes.get(k) == Some(&b'\n') {
                    k += 1;
                }
            }
        }
        let ch = match char::from_u32(value) {
            Some(ch) if value != 0 && !(0xD800..=0xDFFF).contains(&value) => ch,
            _ => '\u{FFFD}',
        };
        return (ch, k);
    }
    let ch = char_at(css, backslash + 1);
    (ch, backslash + 1 + ch.len_utf8())
}

/// The char starting at byte `i` (must be a char boundary).
fn char_at(css: &str, i: usize) -> char {
    css[i..].chars().next().expect("index within bounds")
}

/// Consume a run of ident-ish characters and escapes starting at `start`,
/// returning the decoded text and the index one past the run. Used only for
/// function-name detection, so the run is deliberately broader than a strict
/// ident (digits may lead — see [`is_word_byte`]).
fn consume_ident_sequence(css: &str, start: usize) -> (String, usize) {
    let bytes = css.as_bytes();
    let mut decoded = String::new();
    let mut k = start;
    while let Some(&c) = bytes.get(k) {
        if c == b'\\' && is_valid_escape(bytes, k) {
            let (ch, next) = consume_escape(css, k);
            decoded.push(ch);
            k = next;
        } else if is_word_byte(c) {
            let ch = char_at(css, k);
            decoded.push(ch);
            k += ch.len_utf8();
        } else {
            break;
        }
    }
    debug_assert!(k > start, "caller only enters on a word/escape byte");
    (decoded, k)
}

/// Consume the value of a `url(` whose opening paren has just been passed
/// (`start` is the index right after `(`). Returns the occurrence (or `None`
/// for a bad url) and the index scanning should resume from.
fn consume_url_value(css: &str, start: usize) -> (Option<CssUrlOccurrence>, usize) {
    let bytes = css.as_bytes();
    let len = bytes.len();
    let mut j = start;
    while j < len && is_css_whitespace(bytes[j]) {
        j += 1;
    }

    // Quoted form: `url(` + whitespace + a quote tokenises as a function
    // containing a string token (CSS Syntax §4.3.4).
    if j < len && (bytes[j] == b'"' || bytes[j] == b'\'') {
        let quote = if bytes[j] == b'"' {
            UrlQuote::Double
        } else {
            UrlQuote::Single
        };
        let token = consume_string_token(css, j);
        if token.terminator == StringEnd::Newline {
            // Bad-string inside url() — the whole thing is invalid; report
            // nothing and let the main loop resume at the newline.
            return (None, token.end);
        }
        let occurrence = CssUrlOccurrence {
            value_span: j..token.end,
            decoded: token.decoded,
            quote,
        };
        // Consume the well-formed tail (`)` after optional whitespace) so the
        // caller resumes past the function. Anything else — url-modifiers or
        // stray tokens — is left for the main loop to tokenise normally.
        let mut k = token.end;
        while k < len && is_css_whitespace(bytes[k]) {
            k += 1;
        }
        if k < len && bytes[k] == b')' {
            return (Some(occurrence), k + 1);
        }
        return (Some(occurrence), token.end);
    }

    // Unquoted url-token (CSS Syntax §4.3.6).
    let value_start = j;
    let mut decoded = String::new();
    let mut value_end = j; // one past the last value byte consumed
    let mut k = j;
    loop {
        let Some(&c) = bytes.get(k) else {
            // EOF in a url-token is a parse error but still yields the token.
            let occurrence = CssUrlOccurrence {
                value_span: value_start..value_end,
                decoded,
                quote: UrlQuote::None,
            };
            return (Some(occurrence), len);
        };
        if c == b')' {
            let occurrence = CssUrlOccurrence {
                value_span: value_start..value_end,
                decoded,
                quote: UrlQuote::None,
            };
            return (Some(occurrence), k + 1);
        }
        if is_css_whitespace(c) {
            // Trailing whitespace before `)`/EOF is fine; whitespace followed
            // by anything else makes this a bad-url.
            let mut w = k;
            while w < len && is_css_whitespace(bytes[w]) {
                w += 1;
            }
            if w >= len || bytes[w] == b')' {
                k = w;
                continue;
            }
            return (None, consume_bad_url_remnants(bytes, w));
        }
        if c == b'"' || c == b'\'' || c == b'(' || is_non_printable(c) {
            return (None, consume_bad_url_remnants(bytes, k));
        }
        if c == b'\\' {
            if !is_valid_escape(bytes, k) {
                return (None, consume_bad_url_remnants(bytes, k));
            }
            let (ch, next) = consume_escape(css, k);
            decoded.push(ch);
            k = next;
            value_end = k;
            continue;
        }
        let ch = char_at(css, k);
        decoded.push(ch);
        k += ch.len_utf8();
        value_end = k;
    }
}

/// Consume the remnants of a bad url (CSS Syntax §4.3.14): everything up to
/// and including the closing `)` (or EOF), with valid escapes consumed so an
/// escaped `\)` does not end the token early.
fn consume_bad_url_remnants(bytes: &[u8], start: usize) -> usize {
    let mut k = start;
    while let Some(&c) = bytes.get(k) {
        if c == b')' {
            return k + 1;
        }
        if c == b'\\' && is_valid_escape(bytes, k) {
            // Skip the backslash and its escaped character; hex-escape
            // internals are hex digits / whitespace and cannot be `)`.
            k += 2;
        } else {
            k += 1;
        }
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single(css: &str) -> CssUrlOccurrence {
        let found = scan_css_urls(css);
        assert_eq!(found.len(), 1, "expected exactly one url() in {css:?}");
        found.into_iter().next().unwrap()
    }

    /// The raw slice at the reported span.
    fn raw<'a>(css: &'a str, occurrence: &CssUrlOccurrence) -> &'a str {
        &css[occurrence.value_span.clone()]
    }

    // ---- positive forms -------------------------------------------------

    #[test]
    fn unquoted_relative_value() {
        let css = ".a{background:url(./files/x.woff2)}";
        let occ = single(css);
        assert_eq!(raw(css, &occ), "./files/x.woff2");
        assert_eq!(occ.decoded, "./files/x.woff2");
        assert_eq!(occ.quote, UrlQuote::None);
    }

    #[test]
    fn double_quoted_value_span_includes_quotes() {
        let css = ".a{background:url(\"./x.png\")}";
        let occ = single(css);
        assert_eq!(raw(css, &occ), "\"./x.png\"");
        assert_eq!(occ.decoded, "./x.png");
        assert_eq!(occ.quote, UrlQuote::Double);
    }

    #[test]
    fn single_quoted_value_span_includes_quotes() {
        let css = ".a{background:url('./x.png')}";
        let occ = single(css);
        assert_eq!(raw(css, &occ), "'./x.png'");
        assert_eq!(occ.decoded, "./x.png");
        assert_eq!(occ.quote, UrlQuote::Single);
    }

    #[test]
    fn function_name_matches_case_insensitively() {
        for css in [
            ".a{background:URL(./x.png)}",
            ".a{background:Url(./x.png)}",
            ".a{background:uRl(\"./x.png\")}",
        ] {
            let occ = single(css);
            assert_eq!(occ.decoded, "./x.png", "in {css:?}");
        }
    }

    #[test]
    fn escaped_ident_spelling_of_url_matches() {
        // `\75 rl` decodes to `url` per ident escape rules.
        let css = ".a{background:\\75 rl(./x.png)}";
        let occ = single(css);
        assert_eq!(occ.decoded, "./x.png");
    }

    #[test]
    fn relative_parent_and_bare_forms() {
        let css = ".a{a:url(../up.png);b:url(files/x.png)}";
        let found = scan_css_urls(css);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].decoded, "../up.png");
        assert_eq!(found[1].decoded, "files/x.png");
    }

    #[test]
    fn absolute_protocol_relative_and_scheme_forms() {
        let css = ".a{a:url(/img/x.png);b:url(//host/x.png);c:url(https://example.com/x.png)}";
        let found = scan_css_urls(css);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].decoded, "/img/x.png");
        assert_eq!(found[1].decoded, "//host/x.png");
        assert_eq!(found[2].decoded, "https://example.com/x.png");
    }

    #[test]
    fn data_uri_is_reported_not_special_cased() {
        let css = ".a{background:url(data:image/png;base64,iVBORw0KGgo=)}";
        let occ = single(css);
        assert_eq!(occ.decoded, "data:image/png;base64,iVBORw0KGgo=");
        assert_eq!(occ.quote, UrlQuote::None);

        let css = ".a{background:url(\"data:image/svg+xml,<svg xmlns='x'/>\")}";
        let occ = single(css);
        assert_eq!(occ.decoded, "data:image/svg+xml,<svg xmlns='x'/>");
        assert_eq!(occ.quote, UrlQuote::Double);
    }

    #[test]
    fn fragment_only_value() {
        let css = ".a{filter:url(#blur)}";
        let occ = single(css);
        assert_eq!(occ.decoded, "#blur");
    }

    #[test]
    fn empty_url_reports_empty_value() {
        let css = ".a{background:url()}";
        let occ = single(css);
        assert_eq!(occ.decoded, "");
        assert_eq!(occ.quote, UrlQuote::None);
        assert_eq!(raw(css, &occ), "");
        // The empty span sits between `(` and `)` so a splice inserts there.
        assert_eq!(occ.value_span.start, css.find("()").unwrap() + 1);
        assert_eq!(occ.value_span.start, occ.value_span.end);
    }

    #[test]
    fn whitespace_only_url_reports_empty_value_inside_parens() {
        let css = ".a{background:url(   )}";
        let occ = single(css);
        assert_eq!(occ.decoded, "");
        assert!(occ.value_span.start > css.find('(').unwrap());
        assert!(occ.value_span.end < css.find(')').unwrap() + 1);
    }

    #[test]
    fn query_string_and_fragment_stay_part_of_the_value() {
        let css = "@font-face{src:url(\"./font.woff2?v=1#x\")}";
        let occ = single(css);
        assert_eq!(occ.decoded, "./font.woff2?v=1#x");

        let css = "@font-face{src:url(./font.woff2?v=1#x)}";
        let occ = single(css);
        assert_eq!(occ.decoded, "./font.woff2?v=1#x");
    }

    #[test]
    fn surrounding_whitespace_is_trimmed_from_the_span() {
        let css = ".a{background:url(  ./x.png\t)}";
        let occ = single(css);
        assert_eq!(raw(css, &occ), "./x.png");

        let css = ".a{background:url( \"./x.png\" )}";
        let occ = single(css);
        assert_eq!(raw(css, &occ), "\"./x.png\"");
    }

    #[test]
    fn font_face_src_list_matches_urls_but_not_format_tech_or_local() {
        let css = "@font-face{src:local(\"Arial\"),url(a.woff2) format(\"woff2\") \
                   tech(color-COLRv1),url(\"b.woff\") format('woff');}";
        let found = scan_css_urls(css);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].decoded, "a.woff2");
        assert_eq!(found[0].quote, UrlQuote::None);
        assert_eq!(found[1].decoded, "b.woff");
        assert_eq!(found[1].quote, UrlQuote::Double);
    }

    // ---- escape decoding ------------------------------------------------

    #[test]
    fn unquoted_escapes_are_decoded() {
        // `\ ` escapes a space; `\28`/`\29` are `(`/`)`.
        let css = ".a{background:url(fo\\ o\\28 x\\29.png)}";
        let occ = single(css);
        assert_eq!(occ.decoded, "fo o(x).png");
        assert_eq!(raw(css, &occ), "fo\\ o\\28 x\\29.png");
    }

    #[test]
    fn hex_escape_whitespace_terminator_is_consumed() {
        // `\66 ` is `f` with its terminating space consumed — not "f oo".
        let css = ".a{background:url(\\66 oo.png)}";
        let occ = single(css);
        assert_eq!(occ.decoded, "foo.png");
    }

    #[test]
    fn quoted_escapes_are_decoded() {
        let css = ".a{background:url(\"a\\\"b\\66 .png\")}";
        let occ = single(css);
        assert_eq!(occ.decoded, "a\"bf.png");
    }

    #[test]
    fn quoted_line_continuation_joins_lines() {
        let css = ".a{background:url(\"a\\\nb.png\")}";
        let occ = single(css);
        assert_eq!(occ.decoded, "ab.png");
    }

    #[test]
    fn invalid_hex_code_points_decode_to_replacement_char() {
        // `\0` and a surrogate escape both decode to U+FFFD.
        let css = ".a{background:url(a\\0 b\\d800 c.png)}";
        let occ = single(css);
        assert_eq!(occ.decoded, "a\u{FFFD}b\u{FFFD}c.png");
    }

    #[test]
    fn multi_byte_utf8_values_round_trip() {
        let css = ".a{background:url(./\u{65E5}\u{672C}.png)}";
        let occ = single(css);
        assert_eq!(occ.decoded, "./\u{65E5}\u{672C}.png");
        assert_eq!(raw(css, &occ), "./\u{65E5}\u{672C}.png");
    }

    // ---- negative cases -------------------------------------------------

    #[test]
    fn url_inside_comment_does_not_match() {
        let css = "/* url(ghost.png) */.a{color:red}";
        assert!(scan_css_urls(css).is_empty());
    }

    #[test]
    fn url_inside_string_tokens_does_not_match() {
        let css = ".a{content:\"url(ghost.png)\"}.b{content:'url(ghost.png)'}";
        assert!(scan_css_urls(css).is_empty());
    }

    #[test]
    fn function_names_merely_ending_in_url_do_not_match() {
        for css in [
            ".a{background:imageurl(x.png)}",
            ".a{background:-url(x.png)}",
            ".a{background:my-url(x.png)}",
            ".a{width:3url(x.png)}",
        ] {
            assert!(scan_css_urls(css).is_empty(), "false match in {css:?}");
        }
    }

    #[test]
    fn at_keyword_and_hash_prefixed_url_do_not_match() {
        // `@url(...)` is an at-keyword token and `#url(...)` a hash token —
        // the ident sequence is absorbed, no url() function exists.
        for css in [
            "@url(x.png){color:red}",
            ".a{color:#url(x.png)}",
            "@\\75 rl(x.png){}",
            ".a{color:#\\75 rl(x.png)}",
        ] {
            assert!(scan_css_urls(css).is_empty(), "false match in {css:?}");
        }
    }

    #[test]
    fn at_rule_prelude_url_still_matches() {
        // `@import` is its own at-keyword token; the following `url(...)`
        // after whitespace is an ordinary url token.
        let css = "@import url(./theme.css);";
        let occ = single(css);
        assert_eq!(occ.decoded, "./theme.css");
    }

    #[test]
    fn delim_prefixed_url_still_matches() {
        // Per CSS Syntax, `.url(` is a `.` delim token followed by a url
        // token — unlike `@`/`#`, the delim does not absorb the ident.
        let css = ".url(x.png)";
        let occ = single(css);
        assert_eq!(occ.decoded, "x.png");
    }

    #[test]
    fn real_url_after_comment_and_string_still_matches() {
        let css = "/* url(no.png) */.a{content:\"url(no.png)\";background:url(yes.png)}";
        let occ = single(css);
        assert_eq!(occ.decoded, "yes.png");
    }

    #[test]
    fn bad_url_with_interior_whitespace_is_not_reported() {
        let css = ".a{background:url(a b.png)}.b{background:url(ok.png)}";
        let found = scan_css_urls(css);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].decoded, "ok.png");
    }

    #[test]
    fn bad_url_with_quote_or_paren_is_not_reported() {
        for css in [
            ".a{background:url(a\"b.png)}",
            ".a{background:url(a'b.png)}",
            ".a{background:url(a(b.png)}",
        ] {
            assert!(scan_css_urls(css).is_empty(), "false match in {css:?}");
        }
    }

    #[test]
    fn bad_url_remnants_do_not_derail_later_urls() {
        // The escaped `\)` inside the bad url must not end it early; scanning
        // resumes cleanly and finds the later occurrence.
        let css = ".a{background:url(bad\"x\\).png)}.b{background:url(ok.png)}";
        let found = scan_css_urls(css);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].decoded, "ok.png");
    }

    #[test]
    fn unterminated_comment_swallows_the_rest() {
        let css = ".a{color:red}/* url(ghost.png)";
        assert!(scan_css_urls(css).is_empty());
    }

    // ---- span exactness (the splice contract) ---------------------------

    /// Splice a replacement at every reported span (back to front) and assert
    /// the result equals the same stylesheet with the values swapped by hand —
    /// i.e. every byte outside the spans is untouched.
    #[test]
    fn splicing_at_reported_spans_leaves_every_other_byte_identical() {
        let css = "@font-face{font-family:A;src:url(./a.woff2?v=1#x) format(\"woff2\"),\
                   url(\"./b\\\".woff\") format('woff');unicode-range:U+0-10FFFF}\
                   /* url(no.png) */.c{background:URL( '../c c.png' );mask:url(#frag)}";
        let found = scan_css_urls(css);
        assert_eq!(found.len(), 4);
        assert_eq!(raw(css, &found[0]), "./a.woff2?v=1#x");
        assert_eq!(raw(css, &found[1]), "\"./b\\\".woff\"");
        assert_eq!(raw(css, &found[2]), "'../c c.png'");
        assert_eq!(raw(css, &found[3]), "#frag");

        let mut spliced = css.to_string();
        for occ in found.iter().rev() {
            spliced.replace_range(occ.value_span.clone(), "SPLICED");
        }
        let expected = "@font-face{font-family:A;src:url(SPLICED) format(\"woff2\"),\
                   url(SPLICED) format('woff');unicode-range:U+0-10FFFF}\
                   /* url(no.png) */.c{background:URL( SPLICED );mask:url(SPLICED)}";
        assert_eq!(spliced, expected);
    }

    #[test]
    fn spans_are_exact_byte_offsets() {
        let css = ".a{background:url(x.png)}";
        let occ = single(css);
        let start = css.find("x.png").unwrap();
        assert_eq!(occ.value_span, start..start + "x.png".len());
    }
}
