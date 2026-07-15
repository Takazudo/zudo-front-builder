//! Code-block enrichment — diff markers, per-line highlighting, and
//! visible-text word emphasis.
//!
//! Rust port of [rehype-pretty-code](https://rehype-pretty-code.netlify.app/)'s
//! diff-marker, `{1,3-5}` line-highlight, and `/word/` emphasis behaviours.
//!
//! # Phase
//!
//! **hast**, running AFTER [`SyntectPlugin`] (wave 5 ordering contract).
//! SyntectPlugin emits `<pre class="syntect-…"><code><span class="line">…</span>…</code></pre>`
//! structured HAST; this visitor walks those `<span class="line">` elements
//! and mutates them in place.
//!
//! # Diff markers
//!
//! Lines whose rendered content contains the comment-wrapped marker
//! `[!code ++]` or `[!code --]` receive `data-line-diff="added"` /
//! `data-line-diff="removed"` on the `<span class="line">` element. The
//! marker text is stripped from the line's `Raw(…)` content so it does not
//! appear in the final HTML.
//!
//! Supported comment styles (matched by checking the raw line HTML for the
//! bracketed marker text):
//!
//! | Style | Marker |
//! |-------|--------|
//! | `//` line comment | `// [!code ++]` / `// [!code --]` |
//! | `#` line comment  | `# [!code ++]` / `# [!code --]`  |
//! | `--` line comment | `-- [!code ++]` / `-- [!code --]` |
//!
//! The visitor searches for the literal marker string `[!code ++]` /
//! `[!code --]` inside the already-highlighted raw HTML. Because syntect
//! typically tokenises an entire `// comment` into a single `<span>`, the
//! whole marker ends up in one `<span>` — a simple string-search on the
//! inner HTML is sufficient. No regex-on-raw-HTML needed for the attribute
//! assignment; the strip step removes only the marker portion from the raw
//! text.
//!
//! # Line highlighting
//!
//! The fence info-string (stored on the `<code>` element as `data-meta`) may
//! carry a brace-delimited range after the language identifier:
//!
//! ````markdown
//! ```js {1,3-5}
//! ````
//!
//! Lines 1, 3, 4, and 5 receive `data-line-highlight="true"` on their
//! `<span class="line">`.
//!
//! # Configuration
//!
//! All three behaviours are on by default (`None` means enabled). Each can be
//! disabled independently via [`CodeEnrichmentConfig`]:
//!
//! ```toml
//! [markdown.features.code_enrichment]
//! diff_markers = false
//! line_highlight = true
//! word_highlight = true
//! ```

use zfb_md_ast::{CodeEnrichmentConfig, HastNode, HastVisitor};

// ── Diff marker constants ────────────────────────────────────────────────────

/// Marker text for "added" lines. Case-sensitive (rehype-pretty-code convention).
const MARKER_ADD: &str = "[!code ++]";
/// Marker text for "removed" lines.
const MARKER_DEL: &str = "[!code --]";

// ── Line-highlight range parsing ─────────────────────────────────────────────

/// Parse a `{1,3-5,8}` style range string into a sorted `Vec` of 1-based
/// line numbers.
///
/// The opening `{` and closing `}` are already stripped by the caller.
/// Individual items may be single numbers (`3`) or ranges (`3-5`).
/// Malformed items are silently skipped.
///
/// `max_lines` caps the upper bound of any range to prevent unbounded Vec
/// allocation from a crafted meta like `{1-100000000}` (build DoS). The cap
/// is the actual number of lines in the code block, threaded in by the
/// caller via [`extract_highlight_range`].
fn parse_line_range(s: &str, max_lines: usize) -> Vec<usize> {
    let mut lines = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if let Some((start, end)) = part.split_once('-') {
            if let (Ok(s), Ok(e)) = (start.trim().parse::<usize>(), end.trim().parse::<usize>()) {
                // Cap the upper bound to the actual line count so a crafted
                // huge range (e.g. {1-100000000}) does not materialize a
                // 100M-element Vec.
                let e_capped = e.min(max_lines);
                for n in s..=e_capped {
                    lines.push(n);
                }
            }
        } else if let Ok(n) = part.parse::<usize>() {
            lines.push(n);
        }
    }
    lines.sort_unstable();
    lines.dedup();
    lines
}

/// Extract the `{…}` range from a `data-meta` value like `"title=\"x\" {1,3-5}"`.
///
/// Returns `None` when no brace-delimited range is present.
///
/// `max_lines` is forwarded to [`parse_line_range`] to cap unbounded range
/// expansion (build DoS guard): a range like `{1-100000000}` is clamped to
/// the actual line count of the code block, not materialised in full.
fn extract_highlight_range(meta: &str, max_lines: usize) -> Option<Vec<usize>> {
    let open = meta.find('{')?;
    let close = meta[open..].find('}').map(|i| open + i)?;
    let inner = &meta[open + 1..close];
    let lines = parse_line_range(inner, max_lines);
    if lines.is_empty() {
        None
    } else {
        Some(lines)
    }
}

// ── Word-highlight metadata and visible-HTML rewriting ───────────────────────

/// Metadata and fragment limits keep this optional presentation pass linear
/// with a small, fixed multiplier even for crafted fence info strings.
const MAX_WORD_META_BYTES: usize = 16 * 1024;
const MAX_WORD_EXPRESSIONS: usize = 32;
const MAX_WORD_PATTERN_BYTES: usize = 256;
const MAX_TOTAL_WORD_PATTERN_BYTES: usize = 4 * 1024;
const MAX_SYNTAX_SPAN_DEPTH: usize = 32;
const MAX_SYNTAX_TAG_BYTES: usize = 4 * 1024;

/// Extract whitespace-delimited `/literal phrase/` tokens from fence metadata.
///
/// Expressions inside quoted title metadata or brace-delimited line ranges are
/// ignored. `\/` represents a literal slash. Empty, malformed, unterminated,
/// overlong, and excess expressions are silently ignored; unrelated metadata
/// is never mutated.
fn extract_word_phrases(meta: &str) -> Vec<String> {
    let capped_len = floor_char_boundary(meta, meta.len().min(MAX_WORD_META_BYTES));
    let meta = &meta[..capped_len];
    let bytes = meta.as_bytes();
    let mut phrases = Vec::new();
    let mut total_bytes = 0usize;
    let mut quote: Option<u8> = None;
    let mut brace_depth = 0usize;
    let mut i = 0usize;

    while i < bytes.len() && phrases.len() < MAX_WORD_EXPRESSIONS {
        let byte = bytes[i];
        if let Some(delimiter) = quote {
            if byte == b'\\' && i + 1 < bytes.len() {
                i += 2;
            } else {
                if byte == delimiter {
                    quote = None;
                }
                i += 1;
            }
            continue;
        }

        match byte {
            b'\'' | b'"' => {
                quote = Some(byte);
                i += 1;
            }
            b'{' => {
                brace_depth = brace_depth.saturating_add(1);
                i += 1;
            }
            b'}' => {
                brace_depth = brace_depth.saturating_sub(1);
                i += 1;
            }
            b'/' if brace_depth == 0 && is_word_token_start(bytes, i) => {
                let Some((end, phrase)) = parse_word_expression(meta, i) else {
                    i += 1;
                    continue;
                };
                i = end;
                if phrase.is_empty()
                    || phrase.len() > MAX_WORD_PATTERN_BYTES
                    || total_bytes.saturating_add(phrase.len()) > MAX_TOTAL_WORD_PATTERN_BYTES
                {
                    continue;
                }
                total_bytes += phrase.len();
                phrases.push(phrase);
            }
            _ => i += 1,
        }
    }

    phrases
}

fn floor_char_boundary(value: &str, mut index: usize) -> usize {
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn is_word_token_start(bytes: &[u8], index: usize) -> bool {
    index == 0 || bytes[index - 1].is_ascii_whitespace() || bytes[index - 1] == b'}'
}

fn is_word_token_end(bytes: &[u8], index: usize) -> bool {
    index == bytes.len() || bytes[index].is_ascii_whitespace() || bytes[index] == b'{'
}

/// Parse one expression beginning at `opening`, returning the byte position
/// immediately after its closing slash and the unescaped literal phrase.
fn parse_word_expression(meta: &str, opening: usize) -> Option<(usize, String)> {
    let bytes = meta.as_bytes();
    let mut phrase = String::new();
    let mut segment_start = opening + 1;
    let mut i = segment_start;

    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                phrase.push_str(&meta[segment_start..i]);
                phrase.push('/');
                i += 2;
                segment_start = i;
            }
            b'/' => {
                if !is_word_token_end(bytes, i + 1) {
                    if is_word_token_start(bytes, i) {
                        // A new slash token began before this expression was
                        // closed. Reject the malformed expression so the
                        // outer scanner can recover and parse the new token.
                        return None;
                    }
                    // The Markdown parser normalizes `\/` in fence metadata
                    // to `/` before this hast-phase visitor runs. A slash that
                    // is not followed by a token boundary therefore remains
                    // part of the literal phrase; only the boundary slash can
                    // close it.
                    i += 1;
                    continue;
                }
                phrase.push_str(&meta[segment_start..i]);
                return Some((i + 1, phrase));
            }
            _ => i += 1,
        }
    }
    None
}

/// Select non-overlapping matches from left to right. At a shared start,
/// metadata order wins because `phrases` is checked in order. Advancing to a
/// winning match's end makes that visible range unavailable to later matches.
fn find_word_matches(visible: &str, phrases: &[String]) -> Vec<(usize, usize)> {
    let mut matches = Vec::new();
    let mut pos = 0usize;
    while pos < visible.len() {
        let tail = &visible[pos..];
        if let Some(phrase) = phrases
            .iter()
            .find(|phrase| tail.starts_with(phrase.as_str()))
        {
            let end = pos + phrase.len();
            matches.push((pos, end));
            pos = end;
        } else {
            let Some(ch) = tail.chars().next() else {
                break;
            };
            pos += ch.len_utf8();
        }
    }
    matches
}

#[derive(Debug, Clone)]
struct SyntaxSpan<'a> {
    id: usize,
    open: &'a str,
}

#[derive(Debug)]
struct VisibleUnit<'a> {
    raw: &'a str,
    start: usize,
    end: usize,
}

#[derive(Debug)]
struct TextRun<'a> {
    path: Vec<SyntaxSpan<'a>>,
    units: Vec<VisibleUnit<'a>>,
}

/// Wrap matches in serialized Syntect HTML without flattening syntax spans.
///
/// The highlighter emits only nested `<span ...>` elements plus escaped text
/// in each line fragment. We validate that contract, decode text entities for
/// matching, then close/reopen the active syntax path at word boundaries. The
/// original opening tags, attributes, text entity spellings, and closing tags
/// are retained byte-for-byte on every resulting fragment.
fn highlight_words_in_html(raw_html: &str, phrases: &[String]) -> Option<String> {
    if phrases.is_empty() {
        return None;
    }
    let (visible, runs) = tokenize_visible_html(raw_html)?;
    let matches = find_word_matches(&visible, phrases);
    if matches.is_empty() {
        return None;
    }

    // Start at input size and let the buffer grow with actual wrappers. A
    // crafted input with one match per character must not trigger a large
    // speculative allocation before any output is produced.
    let mut out = String::with_capacity(raw_html.len());
    let mut current_path: Vec<SyntaxSpan<'_>> = Vec::new();
    let mut current_highlight = false;
    let mut match_index = 0usize;

    for run in &runs {
        for unit in &run.units {
            while match_index < matches.len() && matches[match_index].1 <= unit.start {
                match_index += 1;
            }
            let highlighted = matches
                .get(match_index)
                .is_some_and(|&(start, end)| start <= unit.start && unit.end <= end);
            transition_render_path(
                &mut out,
                &mut current_path,
                &run.path,
                &mut current_highlight,
                highlighted,
            );
            out.push_str(unit.raw);
        }
    }

    close_syntax_path(&mut out, &current_path, 0);
    if current_highlight {
        out.push_str("</span>");
    }
    Some(out)
}

fn transition_render_path<'a>(
    out: &mut String,
    current_path: &mut Vec<SyntaxSpan<'a>>,
    desired_path: &[SyntaxSpan<'a>],
    current_highlight: &mut bool,
    desired_highlight: bool,
) {
    if *current_highlight != desired_highlight {
        close_syntax_path(out, current_path, 0);
        if *current_highlight {
            out.push_str("</span>");
        }
        if desired_highlight {
            out.push_str("<span class=\"highlighted-word\">");
        }
        for span in desired_path {
            out.push_str(span.open);
        }
    } else {
        let common = current_path
            .iter()
            .zip(desired_path)
            .take_while(|(left, right)| left.id == right.id)
            .count();
        close_syntax_path(out, current_path, common);
        for span in &desired_path[common..] {
            out.push_str(span.open);
        }
    }
    current_path.clear();
    current_path.extend_from_slice(desired_path);
    *current_highlight = desired_highlight;
}

fn close_syntax_path(out: &mut String, path: &[SyntaxSpan<'_>], keep: usize) {
    for _ in path[keep..].iter().rev() {
        out.push_str("</span>");
    }
}

/// Tokenize the narrowly-defined Syntect line fragment. Any unexpected or
/// malformed markup returns `None`, making word enrichment fail closed while
/// leaving the original HTML untouched.
fn tokenize_visible_html(raw_html: &str) -> Option<(String, Vec<TextRun<'_>>)> {
    let mut visible = String::new();
    let mut runs = Vec::new();
    let mut stack: Vec<SyntaxSpan<'_>> = Vec::new();
    let mut next_id = 0usize;
    let mut i = 0usize;

    while i < raw_html.len() {
        if raw_html[i..].starts_with("</span>") {
            stack.pop()?;
            i += "</span>".len();
        } else if raw_html[i..].starts_with("<span")
            && raw_html
                .as_bytes()
                .get(i + "<span".len())
                .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'>')
        {
            if stack.len() >= MAX_SYNTAX_SPAN_DEPTH {
                return None;
            }
            let end = find_html_tag_end(raw_html, i)?;
            if end - i > MAX_SYNTAX_TAG_BYTES {
                return None;
            }
            let open = &raw_html[i..end];
            stack.push(SyntaxSpan { id: next_id, open });
            next_id = next_id.checked_add(1)?;
            i = end;
        } else if raw_html.as_bytes()[i] == b'<' {
            return None;
        } else {
            let text_end = raw_html[i..]
                .find('<')
                .map_or(raw_html.len(), |offset| i + offset);
            let text = &raw_html[i..text_end];
            let mut units = Vec::new();
            let mut text_pos = 0usize;
            while text_pos < text.len() {
                let (raw_len, decoded) = decode_visible_unit(&text[text_pos..]);
                let start = visible.len();
                visible.push(decoded);
                let end = visible.len();
                units.push(VisibleUnit {
                    raw: &text[text_pos..text_pos + raw_len],
                    start,
                    end,
                });
                text_pos += raw_len;
            }
            if !units.is_empty() {
                runs.push(TextRun {
                    path: stack.clone(),
                    units,
                });
            }
            i = text_end;
        }
    }

    if stack.is_empty() {
        Some((visible, runs))
    } else {
        None
    }
}

fn find_html_tag_end(html: &str, opening: usize) -> Option<usize> {
    let bytes = html.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = opening + 1;
    while i < bytes.len() {
        match (quote, bytes[i]) {
            (Some(delimiter), byte) if byte == delimiter => quote = None,
            (None, b'\'' | b'"') => quote = Some(bytes[i]),
            (None, b'>') => return Some(i + 1),
            _ => {}
        }
        i += 1;
    }
    None
}

/// Decode exactly the entity spellings emitted by the shared HTML escaper,
/// plus numeric entities. Unknown named entities remain visible as a literal
/// `&` followed by their ordinary characters.
fn decode_visible_unit(input: &str) -> (usize, char) {
    for (entity, decoded) in [
        ("&amp;", '&'),
        ("&lt;", '<'),
        ("&gt;", '>'),
        ("&quot;", '"'),
        ("&#39;", '\''),
        ("&apos;", '\''),
    ] {
        if input.starts_with(entity) {
            return (entity.len(), decoded);
        }
    }
    if let Some(rest) = input.strip_prefix("&#") {
        if let Some(semicolon) = rest.find(';').filter(|&index| index <= 8) {
            let number = &rest[..semicolon];
            let parsed = number
                .strip_prefix(['x', 'X'])
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| number.parse::<u32>().ok());
            if let Some(decoded) = parsed.and_then(char::from_u32) {
                return (2 + semicolon + 1, decoded);
            }
        }
    }
    let ch = input.chars().next().expect("input is non-empty");
    (ch.len_utf8(), ch)
}

// ── Diff marker detection & stripping ────────────────────────────────────────

/// Diff classification for a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineDiff {
    Added,
    Removed,
}

/// Search `raw_html` for a diff marker (`[!code ++]` or `[!code --]`).
///
/// Returns the classification and the raw HTML with the marker stripped,
/// or `None` if no marker is found.
///
/// **The marker MUST be preceded by a recognised comment prefix** (`// `,
/// `# `, or `-- `). A bare `[!code ++]` occurrence inside a string literal
/// such as `const s = "[!code ++]";` is NOT treated as a marker — this
/// prevents false positives when the marker text appears as code content
/// rather than a comment annotation.
///
/// The strip logic removes the smallest span that contained the marker. If
/// the marker occupies an entire `<span>…</span>` node (common when syntect
/// groups the whole comment), that span is removed. Otherwise the comment
/// prefix plus the marker text are erased from the raw text. This ensures
/// that even when syntect's tokenisation does not isolate the comment, the
/// visible output does not retain a dangling `//` (or `#`, `--`) prefix.
///
/// After stripping, trailing whitespace runs at the end of the line HTML
/// are trimmed so the diff-marked line does not gain an extra blank column.
fn detect_and_strip_marker(raw_html: &str) -> Option<(LineDiff, String)> {
    // Comment prefixes recognised in source code. Order matters only for the
    // strip pass — detection treats them as alternatives.
    let comment_prefixes = ["// ", "# ", "-- "];

    // Detect: search for any `<prefix><marker>` combination. Bare markers
    // (without a comment prefix) are deliberately NOT treated as diff
    // markers — see the function-level doc above.
    let mut found: Option<(LineDiff, &'static str)> = None;
    for &marker in &[MARKER_ADD, MARKER_DEL] {
        for &prefix in &comment_prefixes {
            let combined = format!("{prefix}{marker}");
            if raw_html.contains(&combined) {
                let diff = if marker == MARKER_ADD {
                    LineDiff::Added
                } else {
                    LineDiff::Removed
                };
                found = Some((diff, marker));
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }
    let (diff, marker) = found?;

    // Strip: prefer removing the entire `<span>PREFIX MARKER</span>` when
    // syntect tokenised the whole comment into one span. Fallback path
    // removes `PREFIX MARKER` text (preserving surrounding markup) so
    // languages with coarser tokenisation also strip cleanly.
    //
    // Use `rfind` (last occurrence) rather than `replace` (all occurrences)
    // to avoid incorrectly stripping a marker that appears inside a string
    // literal earlier on the same line when the real trailing comment marker
    // also exists. Only the trailing comment annotation should be stripped.
    let stripped = try_strip_whole_span(raw_html, marker, &comment_prefixes).unwrap_or_else(|| {
        let mut out = raw_html.to_string();
        for &prefix in &comment_prefixes {
            let combined = format!("{prefix}{marker}");
            if let Some(pos) = out.rfind(&combined) {
                out.drain(pos..pos + combined.len());
                return out.trim_end().to_string();
            }
        }
        out
    });

    // Trim trailing whitespace so that a line like `  x  ` after stripping
    // the trailing ` // [!code ++]` does not leave a dangling space run.
    let stripped = stripped.trim_end().to_string();

    Some((diff, stripped))
}

/// Attempt to remove an entire `<span …>PREFIX MARKER</span>` from `html`.
///
/// Returns `Some(html_without_span)` if such a span was found and removed,
/// or `None` otherwise.
fn try_strip_whole_span(html: &str, marker: &str, prefixes: &[&str]) -> Option<String> {
    // Find the marker position.
    let marker_start = html.find(marker)?;

    // Walk backwards from `marker_start` to find the opening `<span` tag.
    // We look for `>PREFIX` immediately before `marker_start`.
    for &prefix in prefixes {
        if !html[marker_start..].starts_with(marker) {
            continue;
        }
        // Check that the text before the marker (within the same span) is
        // exactly `prefix`. `ends_with` avoids slicing at a byte offset that
        // may fall inside a multi-byte character directly before the marker
        // (e.g. CJK text preceding a bare marker), which would panic.
        if !html[..marker_start].ends_with(prefix) {
            continue;
        }
        let text_start = marker_start - prefix.len();
        // `text_start` is just after `>`. Find the `>` character.
        if text_start == 0 || html.as_bytes()[text_start - 1] != b'>' {
            continue;
        }
        let gt_pos = text_start - 1;
        // Walk backwards to find the matching `<span`.
        if let Some(span_start) = find_span_open_start(html, gt_pos) {
            // The closing `</span>` must follow the marker (possibly after a newline).
            let after_marker = marker_start + marker.len();
            // Strip optional trailing whitespace (e.g. `\n`) between the marker
            // and `</span>` — syntect appends a newline to the last token in a
            // line's comment span.
            let after_trim = html[after_marker..].trim_start_matches('\n');
            let trimmed_len = html[after_marker..].len() - after_trim.len();
            if after_trim.starts_with("</span>") {
                let span_end = after_marker + trimmed_len + "</span>".len();
                let mut result = String::new();
                result.push_str(&html[..span_start]);
                result.push_str(&html[span_end..]);
                return Some(result);
            }
        }
    }
    None
}

/// Walk backwards from `gt_pos` (a `>` that closes the opening span tag) to
/// find the position of the `<` that opened the `<span …>` tag.
fn find_span_open_start(html: &str, gt_pos: usize) -> Option<usize> {
    // Scan backwards from `gt_pos`.
    let bytes = html.as_bytes();
    let mut i = gt_pos;
    loop {
        if i == 0 {
            return None;
        }
        i -= 1;
        if bytes[i] == b'<' {
            // Verify it is a `<span` tag.
            if html[i..].starts_with("<span") {
                return Some(i);
            }
            return None;
        }
    }
}

// ── Visitor ──────────────────────────────────────────────────────────────────

/// Hast visitor that enriches code blocks with diff markers, line
/// highlighting, and visible-text word emphasis.
///
/// Wired into the pipeline by `register_post_syntect_features` (in
/// `zfb-content::pipeline`) AFTER [`SyntectPlugin`] has run.
///
/// The visitor walks `<pre><code>` subtrees, reading:
///
/// - The `data-meta` attribute on the `<code>` element to extract a
///   `{1,3-5}` line-highlight range and `/word/` emphasis phrases.
/// - The `Raw(…)` content of each `<span class="line">` child to detect and
///   strip diff markers.
///
/// All three behaviours can be toggled independently via
/// [`CodeEnrichmentConfig`].
/// `None` means enabled (the default when the feature is on).
pub struct CodeEnrichmentPlugin {
    diff_markers: bool,
    line_highlight: bool,
    word_highlight: bool,
}

impl CodeEnrichmentPlugin {
    /// Create a new plugin from a [`CodeEnrichmentConfig`].
    ///
    /// Every subfeature defaults to `true` when its respective `Option<bool>`
    /// field is `None`.
    #[must_use]
    pub fn new(cfg: CodeEnrichmentConfig) -> Self {
        Self {
            diff_markers: cfg.diff_markers.unwrap_or(true),
            line_highlight: cfg.line_highlight.unwrap_or(true),
            word_highlight: cfg.word_highlight.unwrap_or(true),
        }
    }
}

impl HastVisitor for CodeEnrichmentPlugin {
    fn visit(&mut self, node: &mut HastNode) {
        // When all features are off this visitor is a no-op.
        if !self.diff_markers && !self.line_highlight && !self.word_highlight {
            return;
        }
        match node {
            HastNode::Root { children } | HastNode::Element { children, .. } => {
                enrich_children(
                    children,
                    self.diff_markers,
                    self.line_highlight,
                    self.word_highlight,
                );
                for c in children {
                    self.visit(c);
                }
            }
            _ => {}
        }
    }
}

/// Walk `children` and enrich any `<pre><code>…</code></pre>` subtrees.
///
/// The outer `<pre>` may be a direct child of the current element (after
/// `SyntectPlugin` wraps it in `<pre class="syntect-…">`), or it may be
/// nested inside a `<div class="code-block-container">` wrapper added by
/// `CodeTitlePlugin`. The recursion in `CodeEnrichmentPlugin::visit` handles
/// the nested case; this function handles the direct case.
fn enrich_children(
    children: &mut [HastNode],
    diff_markers: bool,
    line_highlight: bool,
    word_highlight: bool,
) {
    for child in children.iter_mut() {
        if let Some((meta, line_spans)) = pre_code_meta_and_lines(child) {
            // Pass the actual line count as the cap for range parsing so a
            // crafted meta like `{1-100000000}` cannot materialize a
            // 100M-element Vec (build DoS fix).
            let line_count = line_spans.len();
            let highlight_set: Vec<usize> = if line_highlight {
                meta.as_deref()
                    .and_then(|m| extract_highlight_range(m, line_count))
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let word_phrases = if word_highlight {
                meta.as_deref()
                    .map(extract_word_phrases)
                    .unwrap_or_default()
            } else {
                Vec::new()
            };

            for (idx, span) in line_spans.iter_mut().enumerate() {
                let line_num = idx + 1; // 1-based
                enrich_line_span(span, line_num, &highlight_set, diff_markers, &word_phrases);
            }
        }
    }
}

/// If `node` is `<pre …><code …>…</code></pre>` after syntect processing
/// (i.e. `<code>` has `<span class="line">` children), return a mutable
/// reference to the `<code>` element's children (the line spans) plus the
/// `data-meta` value of the `<code>` element.
///
/// Returns `None` if the node does not match the expected shape.
fn pre_code_meta_and_lines(node: &mut HastNode) -> Option<(Option<String>, &mut Vec<HastNode>)> {
    let HastNode::Element {
        tag: pre_tag,
        children: pre_children,
        ..
    } = node
    else {
        return None;
    };
    if pre_tag != "pre" {
        return None;
    }
    let HastNode::Element {
        tag: code_tag,
        attrs: code_attrs,
        children: code_children,
        ..
    } = pre_children.first_mut()?
    else {
        return None;
    };
    if code_tag != "code" {
        return None;
    }
    // Verify there is at least one <span class="line"> child — this
    // distinguishes a post-syntect code block from a plain <pre><code>.
    let has_line_span = code_children.iter().any(|c| {
        matches!(c, HastNode::Element { tag, attrs, .. }
            if tag == "span" && attrs.iter().any(|(k, v)| k == "class" && v == "line"))
    });
    if !has_line_span {
        return None;
    }
    let meta = code_attrs
        .iter()
        .find(|(k, _)| k == "data-meta")
        .map(|(_, v)| v.clone());
    Some((meta, code_children))
}

/// Mutate a single `<span class="line">` element in place.
///
/// - If `line_num` is in `highlight_set`, add `data-line-highlight="true"`.
/// - If diff markers are enabled and the line's `Raw(…)` content contains
///   a marker, add `data-line-diff="added"` / `"removed"` and strip the
///   marker from the raw content.
/// - Wrap configured visible-text matches in `.highlighted-word` while
///   preserving syntax span attributes and escaped text.
fn enrich_line_span(
    span: &mut HastNode,
    line_num: usize,
    highlight_set: &[usize],
    diff_markers: bool,
    word_phrases: &[String],
) {
    let HastNode::Element {
        tag,
        attrs,
        children,
        ..
    } = span
    else {
        return;
    };
    if tag != "span" {
        return;
    }
    // Only process elements that have class="line".
    let is_line_span = attrs.iter().any(|(k, v)| k == "class" && v == "line");
    if !is_line_span {
        return;
    }

    // ── Diff markers ─────────────────────────────────────────────────────────
    if diff_markers {
        // Collect raw HTML content from Raw children to scan for markers.
        // In the normal syntect output there is exactly one Raw child per line
        // span, but we handle multiple to be robust.
        let raw_html: String = children
            .iter()
            .filter_map(|c| {
                if let HastNode::Raw(s) = c {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .collect();

        if let Some((diff, stripped)) = detect_and_strip_marker(&raw_html) {
            // Apply the attribute.
            let diff_value = match diff {
                LineDiff::Added => "added",
                LineDiff::Removed => "removed",
            };
            attrs.push(("data-line-diff".to_string(), diff_value.to_string()));

            // Replace Raw children with the stripped version.
            // Simplification: replace ALL Raw children with a single Raw(stripped).
            children.retain(|c| !matches!(c, HastNode::Raw(_)));
            children.insert(0, HastNode::Raw(stripped));
        }
    }

    // ── Word highlighting ────────────────────────────────────────────
    if !word_phrases.is_empty() {
        let raw_html: String = children
            .iter()
            .filter_map(|child| match child {
                HastNode::Raw(raw) => Some(raw.as_str()),
                _ => None,
            })
            .collect();
        if let Some(highlighted) = highlight_words_in_html(&raw_html, word_phrases) {
            children.retain(|child| !matches!(child, HastNode::Raw(_)));
            children.insert(0, HastNode::Raw(highlighted));
        }
    }

    // ── Line highlighting ─────────────────────────────────────────────────────
    if !highlight_set.is_empty() && highlight_set.binary_search(&line_num).is_ok() {
        attrs.push(("data-line-highlight".to_string(), "true".to_string()));
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_line_range ──────────────────────────────────────────────────────

    #[test]
    fn parse_single_number() {
        assert_eq!(parse_line_range("3", usize::MAX), vec![3]);
    }

    #[test]
    fn parse_range() {
        assert_eq!(parse_line_range("3-5", usize::MAX), vec![3, 4, 5]);
    }

    #[test]
    fn parse_mixed() {
        let mut r = parse_line_range("1,3-5,8", usize::MAX);
        r.sort_unstable();
        assert_eq!(r, vec![1, 3, 4, 5, 8]);
    }

    #[test]
    fn parse_with_spaces() {
        assert_eq!(parse_line_range(" 1 , 2 - 3 ", usize::MAX), vec![1, 2, 3]);
    }

    #[test]
    fn parse_empty_string_returns_empty() {
        assert!(parse_line_range("", usize::MAX).is_empty());
    }

    #[test]
    fn parse_malformed_skips() {
        // "abc" is not a valid number — silently skip.
        assert!(parse_line_range("abc", usize::MAX).is_empty());
    }

    /// An out-of-range upper bound is clamped to `max_lines` rather than
    /// materialising a massive Vec (build DoS guard).
    #[test]
    fn parse_range_capped_to_max_lines() {
        // Only 10 lines in the block, but the meta says {1-100000000}.
        // The result must be at most 10 elements, not 100M.
        let result = parse_line_range("1-100000000", 10);
        assert_eq!(result, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    }

    // ── extract_highlight_range ───────────────────────────────────────────────

    #[test]
    fn extract_from_plain_meta() {
        let r = extract_highlight_range("{1,3-5}", usize::MAX).unwrap();
        assert_eq!(r, vec![1, 3, 4, 5]);
    }

    #[test]
    fn extract_from_meta_with_title() {
        let r = extract_highlight_range("title=\"foo\" {2,4}", usize::MAX).unwrap();
        assert_eq!(r, vec![2, 4]);
    }

    #[test]
    fn extract_from_meta_no_range() {
        assert!(extract_highlight_range("title=\"foo\"", usize::MAX).is_none());
    }

    #[test]
    fn extract_empty_braces_returns_none() {
        assert!(extract_highlight_range("{}", usize::MAX).is_none());
    }

    // ── Word metadata and rewriting ────────────────────────────────────────

    #[test]
    fn extracts_one_and_multiple_word_expressions_around_other_meta() {
        assert_eq!(extract_word_phrases("/answer/"), vec!["answer"]);
        assert_eq!(
            extract_word_phrases("title=\"demo/example\" {1,3} /first/ /second phrase/"),
            vec!["first", "second phrase"]
        );
    }

    #[test]
    fn word_expression_supports_escaped_slash() {
        assert_eq!(extract_word_phrases(r"/path\/name/"), vec!["path/name"]);
        // This is the shape received after the Markdown parser normalizes the
        // author's `\/` escape in fence metadata.
        assert_eq!(extract_word_phrases("/path/name/"), vec!["path/name"]);
    }

    #[test]
    fn empty_malformed_and_unterminated_expressions_are_ignored() {
        assert!(extract_word_phrases("//").is_empty());
        assert!(extract_word_phrases("/unterminated").is_empty());
        assert!(extract_word_phrases("/bad/adjacent").is_empty());
        assert!(extract_word_phrases("title=\"/not an expression/\"").is_empty());
        assert_eq!(extract_word_phrases("/unterminated /word/"), vec!["word"]);
    }

    #[test]
    fn word_matching_repeats_and_resolves_overlaps_by_start_then_meta_order() {
        assert_eq!(
            find_word_matches("answer answer", &["answer".into()]),
            vec![(0, 6), (7, 13)]
        );
        // Earliest start wins even though the later-starting pattern is first.
        assert_eq!(
            find_word_matches("answer", &["swer".into(), "answer".into()]),
            vec![(0, 6)]
        );
        // At the same start, metadata order wins and consumes the range.
        assert_eq!(
            find_word_matches("answer", &["ans".into(), "answer".into()]),
            vec![(0, 3)]
        );
    }

    #[test]
    fn highlights_visible_text_without_matching_tags_or_attributes() {
        let html = r#"<span class="answer">plain</span>"#;
        assert!(highlight_words_in_html(html, &["answer".into()]).is_none());
        assert_eq!(
            highlight_words_in_html(html, &["plain".into()]).unwrap(),
            r#"<span class="highlighted-word"><span class="answer">plain</span></span>"#
        );
    }

    #[test]
    fn highlights_decoded_html_sensitive_text_and_preserves_entities() {
        let html = r#"<span class="hi-str">&lt;tag&gt; &amp; &#39;q&#39;</span>"#;
        assert_eq!(
            highlight_words_in_html(html, &["<tag> & 'q'".into()]).unwrap(),
            r#"<span class="highlighted-word"><span class="hi-str">&lt;tag&gt; &amp; &#39;q&#39;</span></span>"#
        );
    }

    #[test]
    fn phrase_crossing_syntax_spans_clones_semantic_markup_at_boundaries() {
        let html = r#"<span class="hi-var">an</span><span style="color:#123">swer</span> = 1"#;
        assert_eq!(
            highlight_words_in_html(html, &["answer".into()]).unwrap(),
            r#"<span class="highlighted-word"><span class="hi-var">an</span><span style="color:#123">swer</span></span> = 1"#
        );
    }

    #[test]
    fn phrase_inside_nested_syntax_spans_preserves_the_full_role_path() {
        let html = r#"<span class="outer">an<span class="inner">sw</span>er</span>"#;
        assert_eq!(
            highlight_words_in_html(html, &["answer".into()]).unwrap(),
            r#"<span class="highlighted-word"><span class="outer">an<span class="inner">sw</span>er</span></span>"#
        );
    }

    #[test]
    fn repeated_visible_matches_each_receive_a_wrapper() {
        let highlighted = highlight_words_in_html("answer answer", &["answer".into()]).unwrap();
        assert_eq!(highlighted.matches("highlighted-word").count(), 2);
        assert_eq!(
            highlighted,
            r#"<span class="highlighted-word">answer</span> <span class="highlighted-word">answer</span>"#
        );
    }

    #[test]
    fn phrase_boundaries_split_a_syntax_span_without_losing_attributes() {
        let html = r#"<span class="hi-var" style="color:#123">the answer value</span>"#;
        assert_eq!(
            highlight_words_in_html(html, &["answer".into()]).unwrap(),
            r#"<span class="hi-var" style="color:#123">the </span><span class="highlighted-word"><span class="hi-var" style="color:#123">answer</span></span><span class="hi-var" style="color:#123"> value</span>"#
        );
    }

    #[test]
    fn malformed_highlight_html_fails_closed() {
        assert!(highlight_words_in_html("<span>answer", &["answer".into()]).is_none());
        assert!(highlight_words_in_html("<em>answer</em>", &["answer".into()]).is_none());
    }

    // ── detect_and_strip_marker ───────────────────────────────────────────────

    #[test]
    fn detect_add_marker_plain() {
        let html = "const x = 1; // [!code ++]";
        let (diff, stripped) = detect_and_strip_marker(html).unwrap();
        assert_eq!(diff, LineDiff::Added);
        assert!(
            !stripped.contains("[!code ++]"),
            "marker should be stripped"
        );
    }

    #[test]
    fn detect_del_marker_plain() {
        let html = "old line // [!code --]";
        let (diff, _) = detect_and_strip_marker(html).unwrap();
        assert_eq!(diff, LineDiff::Removed);
    }

    #[test]
    fn no_marker_returns_none() {
        let html = "const x = 1;";
        assert!(detect_and_strip_marker(html).is_none());
    }

    #[test]
    fn detect_marker_in_span() {
        // Simulate syntect-highlighted HTML where the comment is in a <span>.
        let html = r#"<span style="color:#65737e">// [!code ++]</span>"#;
        let (diff, stripped) = detect_and_strip_marker(html).unwrap();
        assert_eq!(diff, LineDiff::Added);
        assert!(
            !stripped.contains("[!code ++]"),
            "marker should be stripped: {stripped}"
        );
    }

    #[test]
    fn strip_whole_span_when_marker_is_only_content() {
        let html = r#"<span style="color:#c0c5ce">x = 1</span><span style="color:#65737e">// [!code ++]</span>"#;
        let (_diff, stripped) = detect_and_strip_marker(html).unwrap();
        // The second span should be entirely removed.
        assert!(!stripped.contains("[!code ++]"));
        assert!(stripped.contains("x = 1"), "code content must remain");
    }

    /// A bare marker (no comment prefix) inside a string literal must NOT
    /// be treated as a diff marker. This guards against false positives in
    /// code that happens to contain the literal marker text.
    #[test]
    fn bare_marker_in_string_literal_is_not_detected() {
        let html = r#"const s = "[!code ++]";"#;
        assert!(
            detect_and_strip_marker(html).is_none(),
            "bare [!code ++] without comment prefix must not be detected"
        );
    }

    /// A multi-byte (CJK) character directly before a bare marker literal
    /// must not panic the prefix check (regression: slicing at
    /// `marker_start - prefix.len()` landed mid-character and panicked).
    #[test]
    fn cjk_before_bare_marker_does_not_panic() {
        // The first marker occurrence is prefix-less and preceded by a
        // 3-byte CJK char; the real commented marker comes later, so the
        // line is still detected via the fallback path.
        let html = "あ[!code ++] // [!code ++]";
        let (diff, _stripped) = detect_and_strip_marker(html).unwrap();
        assert_eq!(diff, LineDiff::Added);
    }

    /// When the marker text appears inside a string literal earlier on the
    /// line (prefixed by a comment prefix) AND also as a real trailing
    /// comment, only the TRAILING comment (last occurrence) must be stripped.
    /// The earlier in-string occurrence must be preserved.
    ///
    /// Before the fix, `out.replace(&combined, "")` stripped ALL occurrences,
    /// mangling the string literal content.
    #[test]
    fn marker_in_string_literal_earlier_on_line_is_preserved() {
        // The string literal contains `// [!code ++]` verbatim; the real
        // trailing comment also contains it. Only the trailing one should go.
        let html = r#"const s = "// [!code ++]"; // [!code ++]"#;
        let (diff, stripped) = detect_and_strip_marker(html).unwrap();
        assert_eq!(diff, LineDiff::Added);
        // The marker inside the string literal must survive.
        assert!(
            stripped.contains(r#""// [!code ++]""#),
            "in-string occurrence must be preserved: {stripped}"
        );
        // The trailing real marker must be gone.
        // After stripping `// [!code ++]` at the end, only one occurrence remains.
        assert_eq!(
            stripped.matches("[!code ++]").count(),
            1,
            "only the trailing marker should be stripped; one in-string occurrence must remain: {stripped}"
        );
    }

    /// Hash-style comment prefix (`# `) is also recognised — covers Python,
    /// Ruby, shell scripts.
    #[test]
    fn detect_hash_comment_prefix() {
        let html = "x = 1  # [!code ++]";
        let (diff, stripped) = detect_and_strip_marker(html).unwrap();
        assert_eq!(diff, LineDiff::Added);
        assert!(!stripped.contains("[!code ++]"));
        assert!(
            !stripped.contains("# "),
            "comment prefix must be stripped: {stripped}"
        );
    }

    /// Double-dash comment prefix (`-- `) is recognised — covers SQL, Lua.
    #[test]
    fn detect_double_dash_comment_prefix() {
        let html = "x = 1 -- [!code --]";
        let (diff, stripped) = detect_and_strip_marker(html).unwrap();
        assert_eq!(diff, LineDiff::Removed);
        assert!(!stripped.contains("[!code --]"));
        assert!(
            !stripped.contains("-- "),
            "comment prefix must be stripped: {stripped}"
        );
    }

    /// Fallback path (no whole-span match): the comment prefix is removed
    /// alongside the marker, so the visible output does not retain a
    /// dangling `// ` / `# ` / `-- ` prefix.
    #[test]
    fn fallback_strip_removes_prefix_too() {
        // No `<span>` wrapping around the marker — forces the fallback
        // path. The whole `// [!code ++]` must be removed (prefix + marker).
        let html = "x = 1; // [!code ++]";
        let (_diff, stripped) = detect_and_strip_marker(html).unwrap();
        assert!(!stripped.contains("[!code ++]"));
        assert!(
            !stripped.contains("// "),
            "fallback must strip prefix: {stripped}"
        );
    }

    // ── enrich_line_span ──────────────────────────────────────────────────────

    fn make_line_span(raw_html: &str) -> HastNode {
        HastNode::Element {
            tag: "span".to_string(),
            attrs: vec![("class".to_string(), "line".to_string())],
            children: vec![HastNode::Raw(raw_html.to_string())],
            void: false,
        }
    }

    #[test]
    fn enrich_highlight_adds_attribute() {
        let mut span = make_line_span("const x = 1;");
        enrich_line_span(&mut span, 1, &[1, 3, 5], false, &[]);
        let HastNode::Element { attrs, .. } = &span else {
            panic!()
        };
        assert!(
            attrs
                .iter()
                .any(|(k, v)| k == "data-line-highlight" && v == "true"),
            "line 1 should be highlighted: {attrs:?}"
        );
    }

    #[test]
    fn enrich_no_highlight_when_not_in_set() {
        let mut span = make_line_span("const x = 1;");
        enrich_line_span(&mut span, 2, &[1, 3, 5], false, &[]);
        let HastNode::Element { attrs, .. } = &span else {
            panic!()
        };
        assert!(
            !attrs.iter().any(|(k, _)| k == "data-line-highlight"),
            "line 2 should NOT be highlighted"
        );
    }

    #[test]
    fn enrich_diff_marker_adds_attribute_and_strips() {
        let mut span = make_line_span("x = 1 // [!code ++]");
        enrich_line_span(&mut span, 1, &[], true, &[]);
        let HastNode::Element {
            attrs, children, ..
        } = &span
        else {
            panic!()
        };
        assert!(
            attrs
                .iter()
                .any(|(k, v)| k == "data-line-diff" && v == "added"),
            "should have data-line-diff=added: {attrs:?}"
        );
        // Marker must be stripped from children.
        let raw_content: String = children
            .iter()
            .filter_map(|c| {
                if let HastNode::Raw(s) = c {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(
            !raw_content.contains("[!code ++]"),
            "marker must be stripped: {raw_content}"
        );
    }

    #[test]
    fn both_diff_and_highlight_can_apply_to_same_line() {
        let mut span = make_line_span("x = 1 // [!code ++]");
        enrich_line_span(&mut span, 1, &[1], true, &[]);
        let HastNode::Element { attrs, .. } = &span else {
            panic!()
        };
        assert!(attrs
            .iter()
            .any(|(k, v)| k == "data-line-diff" && v == "added"));
        assert!(attrs
            .iter()
            .any(|(k, v)| k == "data-line-highlight" && v == "true"));
    }

    #[test]
    fn no_op_when_all_features_off() {
        // The visitor should return early without touching the tree.
        let mut plugin = CodeEnrichmentPlugin {
            diff_markers: false,
            line_highlight: false,
            word_highlight: false,
        };
        let mut root = HastNode::Root {
            children: vec![make_line_span("x = 1 // [!code ++]")],
        };
        plugin.visit(&mut root);
        // The line span should have no extra attributes.
        let HastNode::Root { children } = &root else {
            panic!()
        };
        let HastNode::Element { attrs, .. } = &children[0] else {
            panic!()
        };
        assert_eq!(attrs.len(), 1, "only class attribute expected: {attrs:?}");
    }
}
