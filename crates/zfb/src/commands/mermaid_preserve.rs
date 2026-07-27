//! Extract-and-restore helper that carries `data-mermaid` element bodies
//! across HTML minification untouched.
//!
//! `zfb-md-extras::mermaid` rewrites a `<pre><code class="language-mermaid">`
//! block into `<div class="mermaid" data-mermaid="">{diagram source}</div>`.
//! Mermaid's DSL is newline-significant, but a `<div>` is not a
//! whitespace-sensitive element, so the minifier legitimately collapses the
//! body and the diagram stops parsing. `minify-html` 0.18 exposes no way to
//! nominate extra whitespace-sensitive elements, so the fix has to be a
//! wrapper: [`extract`] swaps every such body for an opaque placeholder,
//! the caller minifies, and [`MermaidPreservation::restore`] puts the
//! original bytes back.
//!
//! # Policy
//!
//! "Handle arbitrary HTML" is not a specification, so each ambiguous case has
//! an explicit, tested answer:
//!
//! * **Nested `data-mermaid`** — *outermost wins*. The scanner jumps over the
//!   whole body of the element it claims, so an inner `data-mermaid` element
//!   is preserved verbatim as part of that body rather than extracted again.
//! * **Malformed HTML** — *the whole document passes through untouched, but
//!   only when a mermaid body is actually at risk*. Whenever the scanner meets
//!   a construct it cannot resolve (an unterminated comment, an unterminated
//!   raw-text element, a `data-mermaid` element with no matching end tag),
//!   extraction is abandoned wholesale: [`extract`] returns zero bodies and the
//!   original bytes. Losing the preservation for a broken document is strictly
//!   better than emitting a corrupted one. Nothing here panics, indexes out of
//!   bounds, or recurses.
//!
//!   The abandonment is only reported as
//!   [`is_malformed`](MermaidPreservation::is_malformed) — the flag that makes
//!   a caller skip minification entirely — when the document contains the
//!   literal bytes `data-mermaid` (case-insensitively) *somewhere*. A real
//!   `data-mermaid` attribute name necessarily contains them, so their absence
//!   proves no mermaid body exists to endanger, and such a document minifies
//!   normally no matter how odd its markup is. The check is deliberately a
//!   substring scan rather than anything smarter: it must over-approximate,
//!   never under-approximate.
//! * **Token collision** — *the input cannot forge a placeholder*.
//!   The placeholder is `ZFB-MERMAID-PRESERVE-<pad>-<index>-END`, where `pad`
//!   is a run of `X` one longer than the longest run of `X` following any
//!   occurrence of `ZFB-MERMAID-PRESERVE-` anywhere in the input. The input
//!   therefore provably contains no substring equal to the placeholder prefix,
//!   including a document that embeds a placeholder from an earlier run
//!   verbatim. It stays deterministic: the pad is a pure function of the
//!   input, so no randomness or global state is involved.
//!
//!   That proof is about the **input**.
//!   [`restore`](MermaidPreservation::restore) reads the **minifier's output**,
//!   which is a different byte string, so the guarantee it delivers is narrower
//!   and worth stating exactly. A minifier that deletes bytes can in principle
//!   *create* a prefix-shaped run the input never contained (by joining two
//!   fragments that were separated in the input). Every such forgery is
//!   rejected — as [`Malformed`](MermaidRestoreError::Malformed),
//!   [`UnknownIndex`](MermaidRestoreError::UnknownIndex),
//!   [`Duplicated`](MermaidRestoreError::Duplicated), or
//!   [`Missing`](MermaidRestoreError::Missing) — *except* in one residual case:
//!   a forged token naming exactly the index of a real placeholder that the
//!   same minification pass also deleted. It is substituted silently, because
//!   the seen-set then looks complete. The blast radius is bounded: the bytes
//!   written are still the correct extracted body (nothing is lost, and nothing
//!   from the document is injected) — only its *position* can be wrong. No
//!   real-world minifier is known to produce this shape, and closing it would
//!   need a second, independent channel for placeholder identity; the honest
//!   claim is the narrower one above, not "unforgeable".
//! * **False positives** — only a genuine *attribute name* counts. The scanner
//!   parses tags properly, so `data-mermaid` in ordinary text, inside another
//!   attribute's value, or as a prefix of a longer attribute name
//!   (`data-mermaid-src`) never triggers extraction. Every context the HTML
//!   tokenizer reads as text rather than markup is skipped rather than scanned
//!   — comments, CDATA sections, the raw-text elements in
//!   [`RAW_TEXT_ELEMENTS`], and everything following `<plaintext>` — so
//!   mermaid-shaped markup quoted inside a script is likewise inert.
//! * **Bytes vs decoded text** — *raw bytes* are what is preserved. No entity
//!   decoding, re-encoding, or UTF-8 validation happens anywhere: the body
//!   `A --&gt; B` is stored and restored as those exact bytes. That is the
//!   only choice that can round-trip exactly, and it matches what the
//!   serializer actually emits (`-->` reaches the minifier as `--&gt;`).
//!
//! A [void element](VOID_ELEMENTS), or an element whose body is empty, is
//! skipped — there are no bytes to protect, and inserting a placeholder into
//! an empty element would change the document for no benefit.
//!
//! The element's end is found the way any HTML parser would find it: the first
//! matching end tag at nesting depth zero. A literal `</div>` inside a mermaid
//! label therefore ends the element here exactly as it would in a browser.
//! (`zfb-md-extras` escapes the body, so a real emitted diagram cannot contain
//! one.) Round-tripping is exact either way.
//!
//! For the same reason a trailing `/` in a start tag is **ignored**, exactly as
//! an HTML parser ignores it: `<div/>` opens a `div`, so
//! `<div data-mermaid>a<div/>b</div>c</div>` has the body `a<div/>b</div>c`,
//! not `a<div/>b`. Honouring `/>` here would leave part of what a browser
//! considers the diagram outside the protected range, where the minifier would
//! collapse it. Foreign (SVG/MathML) content, where `/>` genuinely does
//! self-close, is not tracked as a separate namespace: it could only change the
//! outcome for an element sharing the mermaid host's own tag name, and such a
//! document resolves to the malformed pass-through above rather than to a
//! wrong extraction.

use std::fmt;
use std::ops::Range;

/// Literal stem of every placeholder. Never used on its own — see [`extract`].
const MARKER_CORE: &[u8] = b"ZFB-MERMAID-PRESERVE-";
/// The byte the collision-proof pad is built from.
const PAD_BYTE: u8 = b'X';
/// Trailer that closes a placeholder after its decimal index.
const PLACEHOLDER_SUFFIX: &[u8] = b"-END";

/// Elements whose content the HTML tokenizer treats as raw (or escapable-raw)
/// text rather than markup, so this scanner skips over it too.
///
/// `noscript` is raw text whenever scripting is enabled, which is the only
/// case that matters for output zfb ships to a browser.
const RAW_TEXT_ELEMENTS: [&[u8]; 9] = [
    b"script",
    b"style",
    b"textarea",
    b"title",
    b"xmp",
    b"iframe",
    b"noembed",
    b"noframes",
    b"noscript",
];

/// The bytes any genuine `data-mermaid` attribute name must contain.
const MERMAID_ATTR: &[u8] = b"data-mermaid";

/// `<plaintext>` has no end tag at all: everything after it is text forever.
const PLAINTEXT_ELEMENT: &[u8] = b"plaintext";

/// Elements that never have a body, so never have a mermaid body either.
const VOID_ELEMENTS: [&[u8]; 14] = [
    b"area", b"base", b"br", b"col", b"embed", b"hr", b"img", b"input", b"link", b"meta", b"param",
    b"source", b"track", b"wbr",
];

/// The result of an extraction pass: rewritten HTML plus the bodies it removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MermaidPreservation {
    html: Vec<u8>,
    bodies: Vec<Vec<u8>>,
    /// `MARKER_CORE` + pad + `-`, i.e. everything before a placeholder's index.
    prefix: Vec<u8>,
    /// `true` when the scanner met an unresolvable construct and abandoned
    /// extraction wholesale (see the module docs' "Malformed HTML" policy)
    /// *and* the document could still contain a `data-mermaid` element, as
    /// opposed to simply finding no mermaid content at all. Both cases leave
    /// [`bodies`](Self::bodies) empty, but a caller must not treat them the
    /// same way: minifying the original bytes directly is safe when there was
    /// never any mermaid content, but can corrupt an unresolved mermaid body
    /// sitting elsewhere in a malformed document.
    malformed: bool,
}

/// Why a [`MermaidPreservation::restore`] call could not put the bodies back.
///
/// Every variant means the minified document no longer carries the exact
/// placeholders that were handed to it. The caller's only safe response is to
/// discard the minified output and fall back to the original HTML.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MermaidRestoreError {
    /// A placeholder prefix was found, but its index/trailer was mangled.
    Malformed,
    /// A placeholder named an index that no extracted body corresponds to.
    UnknownIndex { index: usize },
    /// One placeholder appeared more than once in the minified output.
    Duplicated { index: usize },
    /// A placeholder handed to the minifier is absent from its output.
    Missing { index: usize },
}

impl fmt::Display for MermaidRestoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => {
                write!(f, "a mermaid placeholder was mangled by minification")
            }
            Self::UnknownIndex { index } => {
                write!(f, "mermaid placeholder {index} has no extracted body")
            }
            Self::Duplicated { index } => {
                write!(f, "mermaid placeholder {index} appeared more than once")
            }
            Self::Missing { index } => {
                write!(f, "mermaid placeholder {index} is missing from the output")
            }
        }
    }
}

impl std::error::Error for MermaidRestoreError {}

impl MermaidPreservation {
    /// The HTML to hand the minifier: bodies swapped for placeholders.
    pub(crate) fn html(&self) -> &[u8] {
        &self.html
    }

    /// `true` when nothing was extracted — [`html`](Self::html) is the input.
    pub(crate) fn is_empty(&self) -> bool {
        self.bodies.is_empty()
    }

    /// `true` when extraction was abandoned because the scanner met an
    /// unresolvable construct *and* the bytes `data-mermaid` occur somewhere
    /// in the document, per the module docs' "Malformed HTML" policy.
    /// A caller must not minify [`html`](Self::html) in this case: an
    /// unresolved mermaid body may still be sitting in it. `false` (with
    /// [`is_empty`](Self::is_empty) also `true`) means no mermaid body can be
    /// at risk — either the document resolved cleanly with no `data-mermaid`
    /// element, or it did not resolve but provably has no mermaid content to
    /// protect — which is safe to minify normally.
    pub(crate) fn is_malformed(&self) -> bool {
        self.malformed
    }

    /// How many mermaid bodies were taken out.
    #[allow(dead_code)] // exercised by this module's own tests, not by `html_minify`
    pub(crate) fn len(&self) -> usize {
        self.bodies.len()
    }

    /// Put the extracted bodies back into `minified`.
    ///
    /// Single-pass: restored bytes are never rescanned, so a body that happens
    /// to look like a placeholder cannot be substituted a second time.
    pub(crate) fn restore(&self, minified: &[u8]) -> Result<Vec<u8>, MermaidRestoreError> {
        if self.bodies.is_empty() {
            return Ok(minified.to_vec());
        }

        let mut out = Vec::with_capacity(minified.len());
        let mut seen = vec![false; self.bodies.len()];
        let mut cursor = 0usize;

        while cursor < minified.len() {
            let Some(offset) = find_sub(&minified[cursor..], &self.prefix) else {
                break;
            };
            let token_start = cursor + offset;
            out.extend_from_slice(&minified[cursor..token_start]);

            let digits_start = token_start + self.prefix.len();
            let mut p = digits_start;
            while minified.get(p).is_some_and(u8::is_ascii_digit) {
                p += 1;
            }
            if p == digits_start || !minified[p..].starts_with(PLACEHOLDER_SUFFIX) {
                return Err(MermaidRestoreError::Malformed);
            }
            let index = std::str::from_utf8(&minified[digits_start..p])
                .ok()
                .and_then(|digits| digits.parse::<usize>().ok())
                .ok_or(MermaidRestoreError::Malformed)?;
            let body = self
                .bodies
                .get(index)
                .ok_or(MermaidRestoreError::UnknownIndex { index })?;
            if seen[index] {
                return Err(MermaidRestoreError::Duplicated { index });
            }
            seen[index] = true;

            out.extend_from_slice(body);
            cursor = p + PLACEHOLDER_SUFFIX.len();
        }

        out.extend_from_slice(&minified[cursor..]);

        if let Some(index) = seen.iter().position(|restored| !restored) {
            return Err(MermaidRestoreError::Missing { index });
        }
        Ok(out)
    }
}

/// Replace every `data-mermaid` element's body with an opaque placeholder.
///
/// Deterministic and total: any input produces exactly one output, and
/// unresolvable markup yields the input unchanged (see the module docs).
pub(crate) fn extract(html: &[u8]) -> MermaidPreservation {
    let prefix = collision_proof_prefix(html);

    let Some(ranges) = collect_mermaid_bodies(html) else {
        return MermaidPreservation {
            html: html.to_vec(),
            bodies: Vec::new(),
            prefix,
            // Unresolvable markup only endangers a mermaid body if there is
            // one. A genuine `data-mermaid` attribute name cannot exist
            // without these bytes, so their absence is proof of safety.
            malformed: contains_mermaid_attr_bytes(html),
        };
    };
    if ranges.is_empty() {
        return MermaidPreservation {
            html: html.to_vec(),
            bodies: Vec::new(),
            prefix,
            malformed: false,
        };
    }

    let mut out = Vec::with_capacity(html.len());
    let mut bodies = Vec::with_capacity(ranges.len());
    let mut last = 0usize;
    for range in ranges {
        out.extend_from_slice(&html[last..range.start]);
        out.extend_from_slice(&placeholder(&prefix, bodies.len()));
        bodies.push(html[range.clone()].to_vec());
        last = range.end;
    }
    out.extend_from_slice(&html[last..]);

    MermaidPreservation {
        html: out,
        bodies,
        prefix,
        malformed: false,
    }
}

fn placeholder(prefix: &[u8], index: usize) -> Vec<u8> {
    let mut token = prefix.to_vec();
    token.extend_from_slice(index.to_string().as_bytes());
    token.extend_from_slice(PLACEHOLDER_SUFFIX);
    token
}

/// Build a placeholder prefix that provably does not occur in `html`.
///
/// One pass: find the longest run of `X` immediately following any occurrence
/// of `MARKER_CORE`, then pad with one more. No occurrence of `MARKER_CORE` in
/// the input can be followed by that many `X`, so the prefix cannot be forged
/// by the document — including a document that pastes in a placeholder from a
/// previous run.
fn collision_proof_prefix(html: &[u8]) -> Vec<u8> {
    let mut longest_run = 0usize;
    let mut cursor = 0usize;
    while let Some(offset) = find_sub(&html[cursor..], MARKER_CORE) {
        let after = cursor + offset + MARKER_CORE.len();
        let mut run = 0usize;
        while html.get(after + run) == Some(&PAD_BYTE) {
            run += 1;
        }
        longest_run = longest_run.max(run);
        cursor = cursor + offset + 1;
    }

    let mut prefix = MARKER_CORE.to_vec();
    prefix.extend(std::iter::repeat_n(PAD_BYTE, longest_run + 1));
    prefix.push(b'-');
    prefix
}

/// Byte ranges of the bodies of every outermost `data-mermaid` element.
///
/// `None` means the document could not be resolved and must be left alone.
fn collect_mermaid_bodies(html: &[u8]) -> Option<Vec<Range<usize>>> {
    let mut bodies = Vec::new();
    let mut i = 0usize;

    while i < html.len() {
        if html[i] != b'<' {
            i += 1;
            continue;
        }
        match markup_at(html, i)? {
            Markup::Skipped { next } => i = next,
            // A stray end tag with nothing open: nothing to claim, move past it.
            Markup::End { next, .. } => i = next,
            Markup::Text => i += 1,
            Markup::Start(tag) => {
                let name = &html[tag.name.clone()];
                if name.eq_ignore_ascii_case(PLAINTEXT_ELEMENT) {
                    // Nothing after this can be an element, so stop scanning
                    // and keep whatever was already claimed.
                    break;
                }
                if is_raw_text(name) {
                    i = skip_raw_text(html, tag.end, name)?;
                } else if tag.has_mermaid && !is_void(name) {
                    let (body_end, after_close) = find_matching_close(html, tag.end, name)?;
                    if body_end > tag.end {
                        bodies.push(tag.end..body_end);
                    }
                    i = after_close;
                } else {
                    i = tag.end;
                }
            }
        }
    }

    Some(bodies)
}

/// Index just past the end tag that closes the element opened at `start`.
///
/// Returns `(body_end, after_close)`. `None` when the element is never closed.
fn find_matching_close(html: &[u8], start: usize, name: &[u8]) -> Option<(usize, usize)> {
    let mut depth = 1usize;
    let mut i = start;

    while i < html.len() {
        if html[i] != b'<' {
            i += 1;
            continue;
        }
        match markup_at(html, i)? {
            Markup::Skipped { next } => i = next,
            Markup::Text => i += 1,
            Markup::Start(tag) => {
                let tag_name = &html[tag.name.clone()];
                if tag_name.eq_ignore_ascii_case(PLAINTEXT_ELEMENT) {
                    // The open element can never be closed after this point.
                    return None;
                }
                if is_raw_text(tag_name) {
                    i = skip_raw_text(html, tag.end, tag_name)?;
                    continue;
                }
                if !is_void(tag_name) && tag_name.eq_ignore_ascii_case(name) {
                    depth += 1;
                }
                i = tag.end;
            }
            Markup::End {
                name: end_name,
                next,
            } => {
                if html[end_name].eq_ignore_ascii_case(name) {
                    depth -= 1;
                    if depth == 0 {
                        return Some((i, next));
                    }
                }
                i = next;
            }
        }
    }

    None
}

/// What the `<` at `i` opens.
enum Markup {
    /// A comment / doctype / stray end tag: nothing to inspect, resume at `next`.
    Skipped {
        next: usize,
    },
    /// Not markup at all — a literal `<` in text.
    Text,
    Start(StartTag),
    End {
        name: Range<usize>,
        next: usize,
    },
}

struct StartTag {
    name: Range<usize>,
    has_mermaid: bool,
    /// Index just past the start tag's `>`.
    end: usize,
}

/// Classify the `<` at `i`. `None` when the construct is unterminated.
fn markup_at(html: &[u8], i: usize) -> Option<Markup> {
    debug_assert_eq!(html[i], b'<');

    if html[i..].starts_with(b"<!--") {
        let rest = &html[i + 4..];
        let offset = find_sub(rest, b"-->")?;
        return Some(Markup::Skipped {
            next: i + 4 + offset + 3,
        });
    }
    // A CDATA section (legal inside inline SVG/MathML) ends at `]]>`, not at
    // the first `>` — a `>` in its text would otherwise leave the remainder
    // being scanned as markup.
    if html[i..].starts_with(b"<![CDATA[") {
        let rest = &html[i + 9..];
        let offset = find_sub(rest, b"]]>")?;
        return Some(Markup::Skipped {
            next: i + 9 + offset + 3,
        });
    }
    match html.get(i + 1) {
        // Doctype or processing instruction: runs to the next `>`.
        Some(b'!' | b'?') => {
            let offset = find_sub(&html[i..], b">")?;
            Some(Markup::Skipped {
                next: i + offset + 1,
            })
        }
        Some(b'/') => {
            let (name, next) = parse_end_tag(html, i)?;
            Some(Markup::End { name, next })
        }
        Some(byte) if byte.is_ascii_alphabetic() => Some(Markup::Start(parse_start_tag(html, i)?)),
        // `a < b` and friends: a bare `<` that opens nothing.
        _ => Some(Markup::Text),
    }
}

/// Parse `<name attr=… >`, recording whether a `data-mermaid` attribute
/// *name* (never a value, never a longer name that merely starts with it) is
/// present. `None` when the tag never closes.
///
/// A trailing `/` ends the tag but is otherwise ignored, matching an HTML
/// parser: `<div/>` opens a `div` (see the module docs).
fn parse_start_tag(html: &[u8], i: usize) -> Option<StartTag> {
    let mut p = i + 1;
    let name_start = p;
    while p < html.len() && !is_name_terminator(html[p]) {
        p += 1;
    }
    let name = name_start..p;
    if name.is_empty() {
        return None;
    }

    let mut has_mermaid = false;

    loop {
        while html.get(p).is_some_and(u8::is_ascii_whitespace) {
            p += 1;
        }
        match html.get(p) {
            None => return None,
            Some(b'>') => {
                p += 1;
                break;
            }
            Some(b'/') => {
                if html.get(p + 1) == Some(&b'>') {
                    p += 2;
                    break;
                }
                p += 1;
            }
            Some(_) => {
                let attr_start = p;
                while p < html.len() && !is_attr_name_terminator(html[p]) {
                    p += 1;
                }
                if p == attr_start {
                    // A lone `=` where a name was expected; consume it so the
                    // loop always makes progress.
                    p += 1;
                    continue;
                }
                let attr_name = &html[attr_start..p];

                let mut q = p;
                while html.get(q).is_some_and(u8::is_ascii_whitespace) {
                    q += 1;
                }
                if html.get(q) == Some(&b'=') {
                    q += 1;
                    while html.get(q).is_some_and(u8::is_ascii_whitespace) {
                        q += 1;
                    }
                    match html.get(q) {
                        None => return None,
                        Some(&quote @ (b'"' | b'\'')) => {
                            q += 1;
                            while html.get(q).is_some_and(|byte| *byte != quote) {
                                q += 1;
                            }
                            if q >= html.len() {
                                return None;
                            }
                            q += 1;
                        }
                        Some(_) => {
                            while html
                                .get(q)
                                .is_some_and(|byte| !byte.is_ascii_whitespace() && *byte != b'>')
                            {
                                q += 1;
                            }
                        }
                    }
                    p = q;
                }

                if attr_name.eq_ignore_ascii_case(MERMAID_ATTR) {
                    has_mermaid = true;
                }
            }
        }
    }

    Some(StartTag {
        name,
        has_mermaid,
        end: p,
    })
}

/// Parse `</name …>`, returning the name's range and the index past the `>`.
fn parse_end_tag(html: &[u8], i: usize) -> Option<(Range<usize>, usize)> {
    let mut p = i + 2;
    let name_start = p;
    while p < html.len() && !is_name_terminator(html[p]) {
        p += 1;
    }
    let name = name_start..p;
    let offset = find_sub(&html[p..], b">")?;
    Some((name, p + offset + 1))
}

/// Index past the end tag of the raw-text element whose content starts at `i`.
fn skip_raw_text(html: &[u8], i: usize, name: &[u8]) -> Option<usize> {
    let mut p = i;
    loop {
        let offset = find_sub(&html[p..], b"</")?;
        let candidate = p + offset;
        let name_start = candidate + 2;
        let name_end = name_start + name.len();
        let matches = html
            .get(name_start..name_end)
            .is_some_and(|found| found.eq_ignore_ascii_case(name))
            && html
                .get(name_end)
                .is_none_or(|byte| is_name_terminator(*byte));
        if matches {
            let close = find_sub(&html[name_end..], b">")?;
            return Some(name_end + close + 1);
        }
        p = candidate + 2;
    }
}

fn is_name_terminator(byte: u8) -> bool {
    byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>')
}

fn is_attr_name_terminator(byte: u8) -> bool {
    byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>' | b'=')
}

fn is_raw_text(name: &[u8]) -> bool {
    RAW_TEXT_ELEMENTS
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

fn is_void(name: &[u8]) -> bool {
    VOID_ELEMENTS
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

/// Could `html` contain a `data-mermaid` attribute name at all?
///
/// A deliberate over-approximation: it says `true` for the string in a comment,
/// in text, or as part of a longer name. What matters is that it can never say
/// `false` for a document that really carries the attribute, since a `false`
/// lets the caller minify markup the scanner could not resolve.
fn contains_mermaid_attr_bytes(html: &[u8]) -> bool {
    html.windows(MERMAID_ATTR.len())
        .any(|window| window.eq_ignore_ascii_case(MERMAID_ATTR))
}

fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extracted_html(preservation: &MermaidPreservation) -> String {
        String::from_utf8(preservation.html().to_vec()).expect("extraction stays UTF-8")
    }

    /// Extract, hand the rewritten HTML straight back (no minifier), restore.
    /// The identity round trip every policy case below is measured against.
    fn round_trip(input: &str) -> String {
        let preservation = extract(input.as_bytes());
        let restored = preservation
            .restore(preservation.html())
            .expect("identity round trip restores");
        String::from_utf8(restored).expect("restoration stays UTF-8")
    }

    #[test]
    fn extract_swaps_the_body_for_a_placeholder_and_restore_puts_it_back() {
        let input = "<div class=\"mermaid\" data-mermaid=\"\">graph TD;\n  A --&gt; B\n</div>";

        let preservation = extract(input.as_bytes());

        assert_eq!(preservation.len(), 1);
        let staged = extracted_html(&preservation);
        assert!(
            !staged.contains("graph TD;"),
            "the body must leave the document: {staged}"
        );
        assert!(
            staged.starts_with("<div class=\"mermaid\" data-mermaid=\"\">")
                && staged.ends_with("</div>"),
            "the wrapper element itself must be untouched: {staged}"
        );
        assert_eq!(round_trip(input), input);
    }

    #[test]
    fn raw_bytes_are_preserved_rather_than_decoded_text() {
        // The serializer hands the minifier escaped text: `-->` arrives as
        // `--&gt;`. Preserving *bytes* is what makes the round trip exact; a
        // decode/re-encode pass would be free to normalise these.
        let body = "graph TD;\n  A[x] --&gt; B[&amp;y]\n  C[&#39;z&#39;]\n";
        let input = format!("<div data-mermaid>{body}</div>");

        let preservation = extract(input.as_bytes());

        assert_eq!(preservation.bodies, vec![body.as_bytes().to_vec()]);
        assert_eq!(round_trip(&input), input);
    }

    #[test]
    fn multibyte_utf8_in_a_body_round_trips_byte_identically() {
        let input = "<div data-mermaid>subgraph build[\"zfb build — your machine\"]\n  A\n</div>";

        assert_eq!(round_trip(input), input);
    }

    #[test]
    fn nested_data_mermaid_elements_are_claimed_by_the_outermost() {
        let input = concat!(
            "<div data-mermaid>outer\n",
            "<div data-mermaid>inner\n  kept\n</div>\n",
            "tail\n</div>"
        );

        let preservation = extract(input.as_bytes());

        assert_eq!(
            preservation.len(),
            1,
            "outermost wins: exactly one body is extracted"
        );
        let body = String::from_utf8(preservation.bodies[0].clone()).unwrap();
        assert!(
            body.contains("<div data-mermaid>inner\n  kept\n</div>"),
            "the inner element survives verbatim inside the outer body: {body}"
        );
        assert_eq!(round_trip(input), input);
    }

    #[test]
    fn several_mermaid_elements_each_get_their_own_placeholder() {
        let input = "<div data-mermaid>one\nA\n</div><p>x</p><div data-mermaid>two\nB\n</div>";

        let preservation = extract(input.as_bytes());

        assert_eq!(preservation.len(), 2);
        assert_eq!(preservation.bodies[0], b"one\nA\n");
        assert_eq!(preservation.bodies[1], b"two\nB\n");
        let staged = extracted_html(&preservation);
        assert!(
            staged.contains("-0-END") && staged.contains("-1-END"),
            "{staged}"
        );
        assert_eq!(round_trip(input), input);
    }

    // --- token collision -------------------------------------------------

    #[test]
    fn a_document_containing_a_placeholder_verbatim_cannot_forge_the_token() {
        // Both a plausible previous-run token and a longer-padded one, placed
        // in ordinary text AND inside the mermaid body itself.
        let forged = "ZFB-MERMAID-PRESERVE-X-0-END";
        let longer = "ZFB-MERMAID-PRESERVE-XXXXX-3-END";
        let input = format!(
            "<p>{forged}</p><div data-mermaid>graph TD;\n  A[\"{longer}\"]\n</div><p>{longer}</p>"
        );

        let preservation = extract(input.as_bytes());

        assert_eq!(preservation.len(), 1);
        let prefix = String::from_utf8(preservation.prefix.clone()).unwrap();
        assert_eq!(
            prefix, "ZFB-MERMAID-PRESERVE-XXXXXX-",
            "the pad must be one longer than the longest run in the document"
        );
        assert!(
            !input.contains(&prefix),
            "the chosen prefix must not occur in the input at all"
        );
        // The forged tokens are still sitting in the staged document; they must
        // not be mistaken for real placeholders on the way back.
        let staged = extracted_html(&preservation);
        assert!(
            staged.contains(forged) && staged.contains(longer),
            "{staged}"
        );
        assert_eq!(round_trip(&input), input);
    }

    #[test]
    fn the_pad_grows_only_as_far_as_the_document_forces_it() {
        let plain = extract(b"<div data-mermaid>a\nb\n</div>");

        assert_eq!(plain.prefix, b"ZFB-MERMAID-PRESERVE-X-".to_vec());
    }

    // --- false positives -------------------------------------------------

    #[test]
    fn data_mermaid_outside_an_attribute_name_never_triggers_extraction() {
        for input in [
            // ordinary text
            "<p>use data-mermaid on the div</p>",
            // another attribute's value, both quote styles
            "<div title=\"data-mermaid\">a\nb</div>",
            "<div title='data-mermaid'>a\nb</div>",
            // unquoted attribute value
            "<div title=data-mermaid>a\nb</div>",
            // a longer attribute name that merely starts with it
            "<div data-mermaid-src=\"x\">a\nb</div>",
            // a longer attribute name that merely ends with it
            "<div x-data-mermaid>a\nb</div>",
            // the string inside a comment
            "<!-- <div data-mermaid>a\nb</div> -->",
        ] {
            let preservation = extract(input.as_bytes());
            assert!(preservation.is_empty(), "must not extract from: {input}");
            assert_eq!(extracted_html(&preservation), input);
        }
    }

    #[test]
    fn raw_text_element_contents_are_not_scanned() {
        let input = concat!(
            "<script>const s = \"<div data-mermaid>a\\nb</div>\";</script>",
            "<style>/* <div data-mermaid>a</div> */</style>",
            "<textarea><div data-mermaid>a\nb</div></textarea>"
        );

        let preservation = extract(input.as_bytes());

        assert!(preservation.is_empty(), "{}", extracted_html(&preservation));
        assert_eq!(extracted_html(&preservation), input);
    }

    #[test]
    fn markup_shaped_text_in_the_other_raw_text_elements_is_inert_too() {
        // A short raw-text list would scan this text as markup, hit an
        // unmatched `data-mermaid` element, and abandon extraction — silently
        // dropping preservation for the real diagram that follows.
        // `noscript` belongs here too: with scripting enabled — the only case
        // that matters for output served to a browser — its content is raw
        // text, so markup-shaped text inside it must not be scanned.
        for host in ["xmp", "iframe", "noembed", "noframes", "noscript"] {
            let input = format!(
                "<{host}><div data-mermaid>not markup</{host}>\
                 <div data-mermaid>graph TD;\n  A\n</div>"
            );

            let preservation = extract(input.as_bytes());

            assert_eq!(
                preservation.bodies,
                vec![b"graph TD;\n  A\n".to_vec()],
                "the real diagram after <{host}> must still be extracted"
            );
            assert_eq!(round_trip(&input), input);
        }
    }

    #[test]
    fn a_cdata_section_is_skipped_through_its_own_terminator() {
        // The `>` inside the CDATA text must not be mistaken for the end of a
        // declaration, or the fake element after it would abort extraction.
        let input = concat!(
            "<svg><script type=\"application/xml\"></script></svg>",
            "<p><![CDATA[ a > b <div data-mermaid>not markup ]]></p>",
            "<div data-mermaid>graph TD;\n  A\n</div>"
        );

        let preservation = extract(input.as_bytes());

        assert_eq!(preservation.bodies, vec![b"graph TD;\n  A\n".to_vec()]);
        assert_eq!(round_trip(input), input);
    }

    #[test]
    fn everything_after_plaintext_is_text_and_extraction_stops_there() {
        let input = "<div data-mermaid>graph TD;\n  A\n</div>\
                     <plaintext><div data-mermaid>not markup</div>";

        let preservation = extract(input.as_bytes());

        assert_eq!(
            preservation.bodies,
            vec![b"graph TD;\n  A\n".to_vec()],
            "the diagram before <plaintext> is kept; nothing after it is scanned"
        );
        assert_eq!(round_trip(input), input);
    }

    #[test]
    fn a_mermaid_element_that_plaintext_swallows_is_left_untouched() {
        let input = "<div data-mermaid>graph TD;\n<plaintext></div>";

        let preservation = extract(input.as_bytes());

        assert!(preservation.is_empty());
        assert_eq!(extracted_html(&preservation), input);
    }

    #[test]
    fn a_void_data_mermaid_element_has_no_body_to_extract() {
        for input in [
            "<img data-mermaid>text\nhere",
            "<br data-mermaid>text\nhere",
            // A void element's trailing `/` is ignored by an HTML parser too,
            // and changes nothing here either.
            "<br data-mermaid/>text\nhere",
        ] {
            let preservation = extract(input.as_bytes());
            assert!(preservation.is_empty(), "must not extract from: {input}");
            assert_eq!(extracted_html(&preservation), input);
        }
    }

    #[test]
    fn a_trailing_slash_does_not_self_close_an_ordinary_element() {
        // An HTML parser ignores `/` in the start tag of an HTML-namespace
        // element, so `<div/>` OPENS a div. Treating it as self-closing would
        // end a mermaid body at the first `</div>` instead of the second,
        // leaving `b</div>c` outside the protected range — where a minifier
        // would collapse whitespace a browser considers part of the diagram.
        let input = "<div data-mermaid>a\n<div/>b\n</div>c\n</div>";

        let preservation = extract(input.as_bytes());

        assert_eq!(
            preservation.bodies,
            vec![b"a\n<div/>b\n</div>c\n".to_vec()],
            "the inner `<div/>` opens an element, so the FIRST `</div>` closes it"
        );
        assert_eq!(round_trip(input), input);

        // The same rule applied to the mermaid element itself: `<div .../>` is
        // an ordinary start tag, so this document has no end tag at all and
        // falls into the malformed pass-through rather than being read as an
        // empty, bodiless element.
        let unclosed = extract(b"<div data-mermaid/>text\nhere");
        assert!(unclosed.is_empty());
        assert!(unclosed.is_malformed());
        assert_eq!(extracted_html(&unclosed), "<div data-mermaid/>text\nhere");
    }

    #[test]
    fn an_empty_mermaid_body_is_left_alone() {
        let input = "<div class=\"mermaid\" data-mermaid=\"\"></div>";

        let preservation = extract(input.as_bytes());

        assert!(preservation.is_empty());
        assert_eq!(extracted_html(&preservation), input);
    }

    #[test]
    fn a_non_div_element_carrying_data_mermaid_is_handled_the_same_way() {
        let input = "<section data-mermaid>graph TD;\n  A\n</section>";

        let preservation = extract(input.as_bytes());

        assert_eq!(preservation.bodies, vec![b"graph TD;\n  A\n".to_vec()]);
        assert_eq!(round_trip(input), input);
    }

    // --- malformed input -------------------------------------------------

    #[test]
    fn an_unclosed_mermaid_element_leaves_the_whole_document_untouched() {
        let input = "<p>before</p><div data-mermaid>graph TD;\n  A --&gt; B\n";

        let preservation = extract(input.as_bytes());

        assert!(preservation.is_empty());
        assert_eq!(extracted_html(&preservation), input);
        assert_eq!(round_trip(input), input);
    }

    #[test]
    fn other_unresolvable_constructs_also_leave_the_document_untouched() {
        for input in [
            // unterminated comment
            "<div data-mermaid>a\nb\n</div><!-- dangling",
            // unterminated raw-text element
            "<div data-mermaid>a\nb\n</div><script>never closed",
            // unterminated start tag
            "<div data-mermaid>a\nb\n</div><div class=\"x",
        ] {
            let preservation = extract(input.as_bytes());
            assert!(
                preservation.is_empty(),
                "malformed input must pass through untouched: {input}"
            );
            assert_eq!(extracted_html(&preservation), input);
        }
    }

    #[test]
    fn unresolvable_markup_is_only_reported_as_malformed_when_mermaid_content_exists() {
        // `is_malformed` is what makes a caller skip minification for the
        // WHOLE page, so it must mean "a mermaid body may be at risk", not
        // merely "the scanner gave up". A document with no `data-mermaid`
        // anywhere has nothing to protect no matter how odd its markup is.
        for input in [
            "<p>fine</p><script>never closed",
            "<script src=\"/a.js\" /><p>after</p>",
            "<p>fine</p><div class=\"x",
            "<p>fine</p><!-- dangling",
            "<section><p>orphan</p>",
        ] {
            let preservation = extract(input.as_bytes());
            assert!(
                !preservation.is_malformed(),
                "no mermaid content, so nothing is endangered: {input}"
            );
            assert!(preservation.is_empty());
            assert_eq!(extracted_html(&preservation), input);
        }

        // The same constructs WITH mermaid content present stay malformed.
        for input in [
            "<div data-mermaid>a\nb\n</div><script>never closed",
            "<div data-mermaid>a\nb\n</div><!-- dangling",
        ] {
            assert!(
                extract(input.as_bytes()).is_malformed(),
                "a mermaid body IS at risk here: {input}"
            );
        }
    }

    #[test]
    fn the_mermaid_content_probe_over_approximates_rather_than_parsing() {
        // The probe only has to be safe, not precise: it may say "malformed"
        // for a document whose only `data-mermaid` is inert text. Pinning that
        // keeps a future "smarter" probe from quietly becoming an
        // under-approximation, which is the direction that loses data.
        let preservation = extract(b"<p>write data-mermaid on the div</p><script>never closed");

        assert!(preservation.is_malformed());
        assert!(preservation.is_empty());
    }

    #[test]
    fn a_stray_less_than_in_text_is_treated_as_text_and_does_not_derail_extraction() {
        let input = "<p>a < b and 3<4</p><div data-mermaid>graph TD;\n  A\n</div>";

        let preservation = extract(input.as_bytes());

        assert_eq!(preservation.bodies, vec![b"graph TD;\n  A\n".to_vec()]);
        assert_eq!(round_trip(input), input);
    }

    #[test]
    fn a_literal_end_tag_inside_a_label_ends_the_element_exactly_as_html_says() {
        // `zfb-md-extras` escapes the body, so production output cannot contain
        // this — but if it ever does, the first matching `</div>` at depth zero
        // closes the element, which is what a browser does too. The round trip
        // stays exact either way.
        let input = "<div data-mermaid>graph TD;\n  A[\"</div>\"]\n</div>";

        let preservation = extract(input.as_bytes());

        assert_eq!(preservation.bodies, vec![b"graph TD;\n  A[\"".to_vec()]);
        assert_eq!(round_trip(input), input);
    }

    #[test]
    fn extraction_never_panics_on_hostile_fragments() {
        for input in [
            "<",
            "<>",
            "</",
            "</>",
            "<!",
            "<!-",
            "<!--",
            "<?",
            "<div",
            "<div ",
            "<div data-mermaid",
            "<div data-mermaid=",
            "<div data-mermaid=\"",
            "<div data-mermaid=''",
            "< div>",
            "<div/",
            "</div>",
            "<div data-mermaid></div",
            "<script",
            "</script>",
            "<div data-mermaid>x</DIV>",
            "<DIV DATA-MERMAID>x\ny</DIV>",
        ] {
            let preservation = extract(input.as_bytes());
            let restored = preservation
                .restore(preservation.html())
                .expect("identity round trip restores");
            assert_eq!(
                restored,
                input.as_bytes(),
                "hostile fragment must round trip: {input:?}"
            );
        }
    }

    #[test]
    fn tag_and_attribute_matching_is_case_insensitive() {
        let input = "<DIV CLASS=mermaid DATA-MERMAID>graph TD;\n  A\n</DIV>";

        let preservation = extract(input.as_bytes());

        assert_eq!(preservation.bodies, vec![b"graph TD;\n  A\n".to_vec()]);
        assert_eq!(round_trip(input), input);
    }

    // --- determinism -----------------------------------------------------

    #[test]
    fn extraction_is_deterministic_for_fixed_input() {
        let input = "<p>ZFB-MERMAID-PRESERVE-XX</p><div data-mermaid>graph TD;\n  A\n</div>";

        let first = extract(input.as_bytes());
        let second = extract(input.as_bytes());

        assert_eq!(first, second);
    }

    // --- restoration failure modes ---------------------------------------

    #[test]
    fn restore_reports_a_placeholder_the_minifier_dropped() {
        let input = "<div data-mermaid>a\nb\n</div>";
        let preservation = extract(input.as_bytes());

        let err = preservation
            .restore(b"<div data-mermaid></div>")
            .unwrap_err();

        assert_eq!(err, MermaidRestoreError::Missing { index: 0 });
    }

    #[test]
    fn restore_reports_a_placeholder_the_minifier_duplicated() {
        let input = "<div data-mermaid>a\nb\n</div>";
        let preservation = extract(input.as_bytes());
        let doubled = [preservation.html(), preservation.html()].concat();

        let err = preservation.restore(&doubled).unwrap_err();

        assert_eq!(err, MermaidRestoreError::Duplicated { index: 0 });
    }

    #[test]
    fn restore_reports_a_mangled_placeholder() {
        let input = "<div data-mermaid>a\nb\n</div>";
        let preservation = extract(input.as_bytes());
        let mut mangled = preservation.html().to_vec();
        let suffix_at = find_sub(&mangled, PLACEHOLDER_SUFFIX).expect("suffix present");
        mangled.drain(suffix_at..suffix_at + PLACEHOLDER_SUFFIX.len());

        let err = preservation.restore(&mangled).unwrap_err();

        assert_eq!(err, MermaidRestoreError::Malformed);
    }

    #[test]
    fn restore_reports_a_placeholder_index_it_never_issued() {
        let input = "<div data-mermaid>a\nb\n</div>";
        let preservation = extract(input.as_bytes());
        let mut forged = preservation.prefix.clone();
        forged.extend_from_slice(b"7");
        forged.extend_from_slice(PLACEHOLDER_SUFFIX);

        let err = preservation.restore(&forged).unwrap_err();

        assert_eq!(err, MermaidRestoreError::UnknownIndex { index: 7 });
    }

    #[test]
    fn restore_on_an_empty_extraction_is_the_identity() {
        let preservation = extract(b"<p>nothing to do</p>");

        assert_eq!(
            preservation.restore(b"<p>nothing to do</p>").unwrap(),
            b"<p>nothing to do</p>"
        );
    }

    // --- composition with the real minifier ------------------------------

    #[test]
    fn extract_minify_restore_preserves_a_real_mermaid_body() {
        // Proof the helper composes with the minifier it was built for. The
        // RAW `minify_html::minify` is called here, with the same config the
        // production wrapper uses — never `minify_rendered_html_bytes`, which
        // already performs this extract/restore internally and would make the
        // test prove only that the wrapper composes with itself. Bypassing the
        // wrapper is what makes the extract/restore calls below load-bearing:
        // drop either one and the mermaid body's newlines are collapsed.
        let mermaid_body = concat!(
            "graph TD;\n",
            "  subgraph build[\"zfb build — your machine\"]\n",
            "    A[Content] --&gt; B[Bundle]\n",
            "  end\n",
        );
        let input = format!(
            "<main>\n  <h1> Title </h1>\n  \
             <div class=\"mermaid\" data-mermaid=\"\">{mermaid_body}</div>\n</main>"
        );

        let preservation = extract(input.as_bytes());
        let minified = minify_html::minify(
            preservation.html(),
            &crate::commands::html_minify::conservative_cfg(),
        );
        let restored = preservation.restore(&minified).expect("restores");
        let output = String::from_utf8(restored).expect("UTF-8");

        // The raw minifier really did collapse the staged document — without
        // this, "the body survived" could just mean nothing was minified.
        assert!(
            minified.len() < preservation.html().len(),
            "the raw minifier must have done something"
        );

        assert!(
            output.contains(mermaid_body),
            "mermaid body must survive minification byte-identically: {output}"
        );
        assert!(
            output.contains("<h1>Title</h1>"),
            "the rest of the document must still be minified: {output}"
        );
    }
}
