//! Add `id` + permalink anchor to `<h2>`–`<h6>` elements.
//!
//! Rust port of zudo-doc's `rehypeHeadingLinks`. Walks the hast tree
//! and, for each `<h2>` … `<h6>`:
//!
//! - Generates a slug from the heading's text content using a
//!   github-slugger-equivalent algorithm (lowercase, non-alphanumerics
//!   collapsed to `-`, leading/trailing `-` stripped).
//! - Deduplicates slugs within one document by appending `-1`, `-2`, …
//!   to repeated slugs (matching `github-slugger` behaviour).
//! - Adds `id="<slug>"` to the heading element (or replaces an
//!   existing `id` attribute).
//! - Appends an empty anchor element as the last child:
//!   `<a href="#<slug>" class="hash-link"
//!   aria-label="Direct link to {heading-text}"></a>`.
//!   The `#` glyph is rendered via CSS `::after` so the anchor body
//!   stays empty — keeping heading-text extraction (e.g. for TOC) clean.
//!
//! `<h1>` is intentionally left alone — page titles are typically
//! emitted by the layout, not the markdown body.

use std::collections::HashMap;
use std::path::PathBuf;

pub use zfb_md_ast::HeadingIdStrategy;

use crate::heading_registry::HeadingEntry;
use crate::pipeline::{BuildContext, HastNode, HastVisitor};
use crate::plugins::util::hast_text::extract_text;

/// Visitor that adds permalink anchors to headings.
pub struct HeadingLinksPlugin {
    slugs: SlugAllocator,
}

impl HeadingLinksPlugin {
    /// New plugin with the default flat strategy and an empty per-document
    /// slug counter.
    #[must_use]
    pub fn new() -> Self {
        Self::with_strategy(HeadingIdStrategy::Flat)
    }

    /// New plugin with an explicit heading-ID strategy
    /// (`markdown.features.headingIds.strategy`, zfb#871).
    #[must_use]
    pub fn with_strategy(strategy: HeadingIdStrategy) -> Self {
        Self {
            slugs: SlugAllocator::new(strategy),
        }
    }
}

/// Strategy-aware slug allocator shared by [`HeadingLinksPlugin`] and
/// `mdx_jsx_emit::collect_headings` so the rendered `<hN id="…">` and the
/// emitted `headings[i].slug` can never drift (zfb#871).
///
/// - [`HeadingIdStrategy::Flat`] delegates to [`next_slug`] untouched —
///   byte-identical to the pre-#871 scheme.
/// - [`HeadingIdStrategy::Hierarchical`] prefixes each slug with its
///   ancestor chain: the candidate is `{parent_final_id}-{base}` (just
///   `base` for a top-of-outline heading), deduped on the full candidate
///   through the same [`next_slug`] counter. The ancestor stack records the
///   parent's *final* (post-dedup) id, so a child anchor always extends the
///   anchor of the section it actually lives in.
///
/// Empty `base` short-circuits to the empty string in BOTH strategies and
/// leaves all state untouched — empty-text headings get no id and do not
/// participate in the outline.
pub struct SlugAllocator {
    strategy: HeadingIdStrategy,
    seen: HashMap<String, usize>,
    /// Hierarchical ancestor stack of `(depth, final id)`. Unused in flat
    /// mode.
    stack: Vec<(u8, String)>,
}

impl SlugAllocator {
    /// New allocator with empty per-document state.
    #[must_use]
    pub fn new(strategy: HeadingIdStrategy) -> Self {
        Self {
            strategy,
            seen: HashMap::new(),
            stack: Vec::new(),
        }
    }

    /// Allocate the slug for a heading of `depth` (2–6) whose slugified
    /// text is `base`. Returns the empty string for an empty `base`.
    pub fn allocate(&mut self, depth: u8, base: &str) -> String {
        match self.strategy {
            HeadingIdStrategy::Flat => next_slug(&mut self.seen, base),
            HeadingIdStrategy::Hierarchical => {
                if base.is_empty() {
                    return String::new();
                }
                while self.stack.last().is_some_and(|(d, _)| *d >= depth) {
                    self.stack.pop();
                }
                let candidate = match self.stack.last() {
                    Some((_, parent)) => format!("{parent}-{base}"),
                    None => base.to_string(),
                };
                let slug = next_slug(&mut self.seen, &candidate);
                self.stack.push((depth, slug.clone()));
                slug
            }
        }
    }

    /// Clear all per-document state (dedup counters AND the hierarchical
    /// ancestor stack) — called between documents (zfb#187).
    pub fn reset(&mut self) {
        self.seen.clear();
        self.stack.clear();
    }
}

/// Allocate the next slug for `base` against a per-document counter map.
///
/// Mirrors github-slugger's repeat-numbering: first occurrence returns
/// `base`, subsequent occurrences return `base-1`, `base-2`, ...
/// Empty `base` short-circuits to the empty string and leaves the
/// counter map untouched, matching heading_links' "skip empty-text
/// headings" behaviour.
///
/// Shared with `mdx_jsx_emit::collect_headings` so the JSX-emitted
/// `headings[i].slug` cannot drift from the rendered `<hN id="...">`.
#[must_use]
pub fn next_slug(seen: &mut HashMap<String, usize>, base: &str) -> String {
    if base.is_empty() {
        return String::new();
    }
    let count = seen.entry(base.to_string()).or_insert(0);
    let slug = if *count == 0 {
        base.to_string()
    } else {
        format!("{base}-{count}")
    };
    *count += 1;
    slug
}

impl Default for HeadingLinksPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl HastVisitor for HeadingLinksPlugin {
    /// Reset the per-document slug state (dedup counter + hierarchical
    /// ancestor stack).
    ///
    /// Called by [`Pipeline::reset_per_entry`] between documents so the
    /// same `## Basic Usage` heading always produces `basic-usage` in each
    /// file rather than `basic-usage-7` when the pipeline is reused across
    /// multiple entries (zfb#187).
    fn reset(&mut self) {
        self.slugs.reset();
    }

    fn visit(&mut self, node: &mut HastNode) {
        self.visit_node(node, None, None);
    }

    /// Context-aware variant: populates the heading-ID registry when one is
    /// available in `ctx.heading_registry`.
    ///
    /// Zero-cost when `ctx.heading_registry` is `None` — no registry writes
    /// are performed.
    fn visit_with_context(&mut self, node: &mut HastNode, ctx: &mut BuildContext<'_>) {
        self.visit_node(node, ctx.heading_registry.as_deref_mut(), ctx.source_path.clone());
    }
}

impl HeadingLinksPlugin {
    /// Shared traversal used by both `visit` and `visit_with_context`.
    ///
    /// When `registry` and `source_path` are provided, a `HeadingEntry` is
    /// pushed for each heading processed. When `registry` is `None` the
    /// behaviour is identical to the pre-wave-6 `visit` method.
    fn visit_node(
        &mut self,
        node: &mut HastNode,
        mut registry: Option<&mut crate::heading_registry::HeadingRegistry>,
        source_path: Option<PathBuf>,
    ) {
        // Compute slug + heading text first (immutable borrow only).
        let mut slug_and_text: Option<(String, String, u8)> = None;
        if let HastNode::Element { tag, .. } = node {
            if let Some(depth) = heading_depth(tag) {
                let text = extract_text(node);
                let base = slugify(&text);
                if !base.is_empty() {
                    slug_and_text = Some((self.slugs.allocate(depth, &base), text, depth));
                }
            }
        }
        if let Some((slug, text, depth)) = slug_and_text {
            if let HastNode::Element {
                attrs, children, ..
            } = node
            {
                set_attr(attrs, "id", &slug);
                children.push(anchor(&slug, &text));
            }
            // Populate the registry when wired — zero cost when None.
            if let (Some(reg), Some(ref path)) = (registry.as_deref_mut(), &source_path) {
                reg.insert(
                    path.clone(),
                    HeadingEntry {
                        id: slug,
                        text,
                        depth,
                    },
                );
            }
        }

        // We cannot hold `registry` across the recursive calls because we
        // need to pass it by mutable reference on each child. Build a list
        // of child indices first, then recurse.
        match node {
            HastNode::Root { children } | HastNode::Element { children, .. } => {
                for c in children {
                    self.visit_node(c, registry.as_deref_mut(), source_path.clone());
                }
            }
            _ => {}
        }
    }
}

/// Returns `Some(depth)` if `tag` is one of `h2`–`h6`, or `None` otherwise.
fn heading_depth(tag: &str) -> Option<u8> {
    match tag {
        "h2" => Some(2),
        "h3" => Some(3),
        "h4" => Some(4),
        "h5" => Some(5),
        "h6" => Some(6),
        _ => None,
    }
}

fn anchor(slug: &str, heading_text: &str) -> HastNode {
    HastNode::Element {
        tag: "a".to_string(),
        attrs: vec![
            ("href".to_string(), format!("#{slug}")),
            ("class".to_string(), "hash-link".to_string()),
            (
                "aria-label".to_string(),
                format!("Direct link to {heading_text}"),
            ),
        ],
        children: vec![],
        void: false,
    }
}

fn set_attr(attrs: &mut Vec<(String, String)>, key: &str, val: &str) {
    for (k, v) in attrs.iter_mut() {
        if k == key {
            *v = val.to_string();
            return;
        }
    }
    attrs.push((key.to_string(), val.to_string()));
}

/// github-slugger-equivalent slugifier.
///
/// Strips control chars and a specific ASCII punctuation set (matching the
/// npm `github-slugger` regex). Whitespace collapses to a single `-`.
/// Everything else — Unicode letters, numbers, combining marks, symbols,
/// emoji, CJK ideographs, kana, hangul — passes through, lowercased where
/// casing applies.
#[must_use]
pub fn slugify(input: &str) -> String {
    fn is_stripped(ch: char) -> bool {
        ch.is_control()
            || ch == '!'
            || ch == '"'
            || ch == '#'
            || ch == '$'
            || ch == '%'
            || ch == '&'
            || ch == '\''
            || ch == '('
            || ch == ')'
            || ch == '*'
            || ch == '+'
            || ch == ','
            || ch == '.'
            || ch == '/'
            || ch == ':'
            || ch == ';'
            || ch == '<'
            || ch == '='
            || ch == '>'
            || ch == '?'
            || ch == '@'
            || ch == '['
            || ch == '\\'
            || ch == ']'
            || ch == '^'
            || ch == '`'
            || ch == '{'
            || ch == '|'
            || ch == '}'
            || ch == '~'
    }
    let mut out = String::with_capacity(input.len());
    let mut last_dash = true;
    for ch in input.chars() {
        if ch.is_whitespace() || is_stripped(ch) {
            if !last_dash {
                out.push('-');
                last_dash = true;
            }
        } else {
            for c in ch.to_lowercase() {
                out.push(c);
            }
            last_dash = false;
        }
    }
    if out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(level: u8, text: &str) -> HastNode {
        HastNode::Element {
            tag: format!("h{level}"),
            attrs: vec![],
            children: vec![HastNode::Text(text.to_string())],
            void: false,
        }
    }

    fn root(children: Vec<HastNode>) -> HastNode {
        HastNode::Root { children }
    }

    fn first_attr<'a>(node: &'a HastNode, key: &str) -> Option<&'a str> {
        let HastNode::Element { attrs, .. } = node else {
            return None;
        };
        attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn adds_id_and_anchor_to_h2() {
        let mut tree = root(vec![h(2, "Hello World")]);
        HeadingLinksPlugin::new().visit(&mut tree);
        let HastNode::Root { children } = tree else {
            unreachable!("expected HastNode::Root")
        };
        assert_eq!(first_attr(&children[0], "id"), Some("hello-world"));

        let HastNode::Element { children: hc, .. } = &children[0] else {
            unreachable!("expected HastNode::Element")
        };
        // Anchor is appended as the LAST child, after the heading text.
        assert_eq!(hc.len(), 2);
        assert_eq!(hc[0], HastNode::Text("Hello World".into()));
        let HastNode::Element {
            tag,
            attrs,
            children: anchor_children,
            ..
        } = &hc[1]
        else {
            unreachable!("expected HastNode::Element")
        };
        assert_eq!(tag, "a");
        assert!(anchor_children.is_empty(), "anchor body must be empty");
        assert!(attrs.contains(&("href".to_string(), "#hello-world".to_string())));
        assert!(attrs.contains(&("class".to_string(), "hash-link".to_string())));
        assert!(attrs.contains(&(
            "aria-label".to_string(),
            "Direct link to Hello World".to_string(),
        )));
    }

    #[test]
    fn h1_is_left_alone() {
        let mut tree = root(vec![h(1, "Page Title")]);
        let before = tree.clone();
        HeadingLinksPlugin::new().visit(&mut tree);
        assert_eq!(tree, before);
    }

    #[test]
    fn duplicate_slugs_are_numbered() {
        let mut tree = root(vec![h(2, "A"), h(2, "A"), h(2, "A")]);
        HeadingLinksPlugin::new().visit(&mut tree);
        let HastNode::Root { children } = tree else {
            unreachable!("expected HastNode::Root")
        };
        assert_eq!(first_attr(&children[0], "id"), Some("a"));
        assert_eq!(first_attr(&children[1], "id"), Some("a-1"));
        assert_eq!(first_attr(&children[2], "id"), Some("a-2"));
    }

    #[test]
    fn slugify_handles_punctuation_and_case() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
        // `-` is NOT in the stripped set per npm github-slugger semantics;
        // pre-existing dashes survive. Leading/trailing whitespace collapses
        // via the last_dash flag but the surrounding dashes themselves stay.
        assert_eq!(slugify("  --weird-- "), "--weird--");
        assert_eq!(slugify("MixedCase 123"), "mixedcase-123");
    }

    #[test]
    fn empty_text_heading_skipped() {
        let mut tree = root(vec![h(3, "")]);
        let before = tree.clone();
        HeadingLinksPlugin::new().visit(&mut tree);
        assert_eq!(tree, before, "empty text → no slug → no mutation");
    }

    #[test]
    fn slugify_empty_input_returns_empty() {
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn slugify_all_stripped_returns_empty() {
        assert_eq!(slugify("!@#$%"), "");
    }

    #[test]
    fn slugify_whitespace_only_returns_empty() {
        assert_eq!(slugify("   \t\n  "), "");
    }

    #[test]
    fn slugify_preserves_japanese() {
        assert_eq!(slugify("コンポーネント構文"), "コンポーネント構文");
    }

    #[test]
    fn slugify_preserves_chinese() {
        assert_eq!(slugify("中文标题"), "中文标题");
    }

    #[test]
    fn slugify_preserves_korean() {
        assert_eq!(slugify("한국어 제목"), "한국어-제목");
    }

    #[test]
    fn slugify_mixed_ascii_and_cjk() {
        assert_eq!(slugify("Section 一"), "section-一");
    }

    #[test]
    fn slugify_preserves_emoji() {
        // Emoji are symbols (Symbol_Other), not in the stripped set.
        // Whitespace between emoji and text collapses to `-`.
        assert_eq!(slugify("🚀 Launch"), "🚀-launch");
    }

    #[test]
    fn slugify_preserves_accented_latin_precomposed() {
        // Precomposed é (U+00E9) is a single code point that passes through.
        assert_eq!(slugify("Café au lait"), "café-au-lait");
    }

    #[test]
    fn slugify_preserves_accented_latin_decomposed() {
        // Decomposed: e (U+0065) + combining acute accent (U+0301).
        // Both code points are preserved — combining marks are not stripped.
        let input = "Cafe\u{0301} au lait";
        let result = slugify(input);
        assert!(
            result.contains("cafe\u{0301}"),
            "decomposed accent must survive: {result:?}"
        );
        assert!(
            result.contains("au"),
            "rest of text must survive: {result:?}"
        );
    }
}
