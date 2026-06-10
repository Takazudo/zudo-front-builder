//! Code-block enrichment — diff markers and per-line line highlighting.
//!
//! Rust port of [rehype-pretty-code](https://rehype-pretty-code.netlify.app/)'s
//! diff-marker and `{1,3-5}` line-highlight behaviours.
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
//! Both behaviours are on by default (`None` means enabled). Each can be
//! disabled independently via [`CodeEnrichmentConfig`]:
//!
//! ```toml
//! [markdown.features.code_enrichment]
//! diff_markers = false
//! line_highlight = true
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
fn parse_line_range(s: &str) -> Vec<usize> {
    let mut lines = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if let Some((start, end)) = part.split_once('-') {
            if let (Ok(s), Ok(e)) = (start.trim().parse::<usize>(), end.trim().parse::<usize>()) {
                for n in s..=e {
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
fn extract_highlight_range(meta: &str) -> Option<Vec<usize>> {
    let open = meta.find('{')?;
    let close = meta[open..].find('}').map(|i| open + i)?;
    let inner = &meta[open + 1..close];
    let lines = parse_line_range(inner);
    if lines.is_empty() {
        None
    } else {
        Some(lines)
    }
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
    let stripped = try_strip_whole_span(raw_html, marker, &comment_prefixes)
        .unwrap_or_else(|| {
            let mut out = raw_html.to_string();
            for &prefix in &comment_prefixes {
                let combined = format!("{prefix}{marker}");
                if out.contains(&combined) {
                    out = out.replace(&combined, "");
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

/// Hast visitor that enriches code blocks with diff markers and line
/// highlighting.
///
/// Wired into the pipeline by `register_post_syntect_features` (in
/// `zfb-content::pipeline`) AFTER [`SyntectPlugin`] has run.
///
/// The visitor walks `<pre><code>` subtrees, reading:
///
/// - The `data-meta` attribute on the `<code>` element to extract a
///   `{1,3-5}` line-highlight range.
/// - The `Raw(…)` content of each `<span class="line">` child to detect and
///   strip diff markers.
///
/// Both behaviours can be toggled independently via [`CodeEnrichmentConfig`].
/// `None` means enabled (the default when the feature is on).
pub struct CodeEnrichmentPlugin {
    diff_markers: bool,
    line_highlight: bool,
}

impl CodeEnrichmentPlugin {
    /// Create a new plugin from a [`CodeEnrichmentConfig`].
    ///
    /// `diff_markers` and `line_highlight` default to `true` when their
    /// respective `Option<bool>` field is `None`.
    #[must_use]
    pub fn new(cfg: CodeEnrichmentConfig) -> Self {
        Self {
            diff_markers: cfg.diff_markers.unwrap_or(true),
            line_highlight: cfg.line_highlight.unwrap_or(true),
        }
    }
}

impl HastVisitor for CodeEnrichmentPlugin {
    fn visit(&mut self, node: &mut HastNode) {
        // When both features are off this visitor is a no-op.
        if !self.diff_markers && !self.line_highlight {
            return;
        }
        match node {
            HastNode::Root { children } | HastNode::Element { children, .. } => {
                enrich_children(children, self.diff_markers, self.line_highlight);
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
fn enrich_children(children: &mut [HastNode], diff_markers: bool, line_highlight: bool) {
    for child in children.iter_mut() {
        if let Some((meta, line_spans)) = pre_code_meta_and_lines(child) {
            let highlight_set: Vec<usize> = if line_highlight {
                meta.as_deref()
                    .and_then(extract_highlight_range)
                    .unwrap_or_default()
            } else {
                Vec::new()
            };

            for (idx, span) in line_spans.iter_mut().enumerate() {
                let line_num = idx + 1; // 1-based
                enrich_line_span(span, line_num, &highlight_set, diff_markers);
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
fn enrich_line_span(
    span: &mut HastNode,
    line_num: usize,
    highlight_set: &[usize],
    diff_markers: bool,
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
    let is_line_span = attrs
        .iter()
        .any(|(k, v)| k == "class" && v == "line");
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
        assert_eq!(parse_line_range("3"), vec![3]);
    }

    #[test]
    fn parse_range() {
        assert_eq!(parse_line_range("3-5"), vec![3, 4, 5]);
    }

    #[test]
    fn parse_mixed() {
        let mut r = parse_line_range("1,3-5,8");
        r.sort_unstable();
        assert_eq!(r, vec![1, 3, 4, 5, 8]);
    }

    #[test]
    fn parse_with_spaces() {
        assert_eq!(parse_line_range(" 1 , 2 - 3 "), vec![1, 2, 3]);
    }

    #[test]
    fn parse_empty_string_returns_empty() {
        assert!(parse_line_range("").is_empty());
    }

    #[test]
    fn parse_malformed_skips() {
        // "abc" is not a valid number — silently skip.
        assert!(parse_line_range("abc").is_empty());
    }

    // ── extract_highlight_range ───────────────────────────────────────────────

    #[test]
    fn extract_from_plain_meta() {
        let r = extract_highlight_range("{1,3-5}").unwrap();
        assert_eq!(r, vec![1, 3, 4, 5]);
    }

    #[test]
    fn extract_from_meta_with_title() {
        let r = extract_highlight_range("title=\"foo\" {2,4}").unwrap();
        assert_eq!(r, vec![2, 4]);
    }

    #[test]
    fn extract_from_meta_no_range() {
        assert!(extract_highlight_range("title=\"foo\"").is_none());
    }

    #[test]
    fn extract_empty_braces_returns_none() {
        assert!(extract_highlight_range("{}").is_none());
    }

    // ── detect_and_strip_marker ───────────────────────────────────────────────

    #[test]
    fn detect_add_marker_plain() {
        let html = "const x = 1; // [!code ++]";
        let (diff, stripped) = detect_and_strip_marker(html).unwrap();
        assert_eq!(diff, LineDiff::Added);
        assert!(!stripped.contains("[!code ++]"), "marker should be stripped");
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
        assert!(!stripped.contains("[!code ++]"), "marker should be stripped: {stripped}");
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

    /// Hash-style comment prefix (`# `) is also recognised — covers Python,
    /// Ruby, shell scripts.
    #[test]
    fn detect_hash_comment_prefix() {
        let html = "x = 1  # [!code ++]";
        let (diff, stripped) = detect_and_strip_marker(html).unwrap();
        assert_eq!(diff, LineDiff::Added);
        assert!(!stripped.contains("[!code ++]"));
        assert!(!stripped.contains("# "), "comment prefix must be stripped: {stripped}");
    }

    /// Double-dash comment prefix (`-- `) is recognised — covers SQL, Lua.
    #[test]
    fn detect_double_dash_comment_prefix() {
        let html = "x = 1 -- [!code --]";
        let (diff, stripped) = detect_and_strip_marker(html).unwrap();
        assert_eq!(diff, LineDiff::Removed);
        assert!(!stripped.contains("[!code --]"));
        assert!(!stripped.contains("-- "), "comment prefix must be stripped: {stripped}");
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
        assert!(!stripped.contains("// "), "fallback must strip prefix: {stripped}");
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
        enrich_line_span(&mut span, 1, &[1, 3, 5], false);
        let HastNode::Element { attrs, .. } = &span else { panic!() };
        assert!(
            attrs.iter().any(|(k, v)| k == "data-line-highlight" && v == "true"),
            "line 1 should be highlighted: {attrs:?}"
        );
    }

    #[test]
    fn enrich_no_highlight_when_not_in_set() {
        let mut span = make_line_span("const x = 1;");
        enrich_line_span(&mut span, 2, &[1, 3, 5], false);
        let HastNode::Element { attrs, .. } = &span else { panic!() };
        assert!(
            !attrs.iter().any(|(k, _)| k == "data-line-highlight"),
            "line 2 should NOT be highlighted"
        );
    }

    #[test]
    fn enrich_diff_marker_adds_attribute_and_strips() {
        let mut span = make_line_span("x = 1 // [!code ++]");
        enrich_line_span(&mut span, 1, &[], true);
        let HastNode::Element { attrs, children, .. } = &span else { panic!() };
        assert!(
            attrs.iter().any(|(k, v)| k == "data-line-diff" && v == "added"),
            "should have data-line-diff=added: {attrs:?}"
        );
        // Marker must be stripped from children.
        let raw_content: String = children
            .iter()
            .filter_map(|c| if let HastNode::Raw(s) = c { Some(s.as_str()) } else { None })
            .collect();
        assert!(!raw_content.contains("[!code ++]"), "marker must be stripped: {raw_content}");
    }

    #[test]
    fn both_diff_and_highlight_can_apply_to_same_line() {
        let mut span = make_line_span("x = 1 // [!code ++]");
        enrich_line_span(&mut span, 1, &[1], true);
        let HastNode::Element { attrs, .. } = &span else { panic!() };
        assert!(attrs.iter().any(|(k, v)| k == "data-line-diff" && v == "added"));
        assert!(attrs.iter().any(|(k, v)| k == "data-line-highlight" && v == "true"));
    }

    #[test]
    fn no_op_when_both_features_off() {
        // The visitor should return early without touching the tree.
        let mut plugin = CodeEnrichmentPlugin {
            diff_markers: false,
            line_highlight: false,
        };
        let mut root = HastNode::Root {
            children: vec![make_line_span("x = 1 // [!code ++]")],
        };
        plugin.visit(&mut root);
        // The line span should have no extra attributes.
        let HastNode::Root { children } = &root else { panic!() };
        let HastNode::Element { attrs, .. } = &children[0] else { panic!() };
        assert_eq!(attrs.len(), 1, "only class attribute expected: {attrs:?}");
    }
}
