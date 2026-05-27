//! Table-of-contents visitor (hast phase, runs after `HeadingLinksPlugin`).
//!
//! Rust port of `remark-toc`. When the hast tree contains a heading whose
//! text matches the configured anchor string (default `"TOC"`,
//! case-insensitive), a nested `<ul>/<li>` list of the *subsequent* headings
//! is inserted as the next sibling.
//!
//! **Phase choice:** hast (not mdast). Running after [`HeadingLinksPlugin`]
//! lets this visitor read the final `id` attributes that plugin already
//! placed on every `<h2>`–`<h6>`, so we never need to re-run the slugger or
//! deduplicate. Each `<a href="#<id>">` in the TOC simply mirrors whatever
//! `id` the heading has.
//!
//! ## Configuration
//!
//! Wire via `markdown.toc` in `zfb.config.ts`:
//!
//! ```json
//! { "markdown": { "toc": { "heading": "TOC", "maxDepth": 2 } } }
//! ```
//!
//! - `heading` — the heading text that triggers TOC insertion (default
//!   `"TOC"`, matched case-insensitively after whitespace trimming).
//! - `maxDepth` — how many depth levels to include starting from `<h2>`.
//!   `1` → h2 only, `2` → h2+h3 (default), `3` → h2+h3+h4, etc.
//!   Maximum is 5 (h2 through h6).

use crate::pipeline::{HastNode, HastVisitor};
use crate::plugins::util::hast_text::extract_text;

// TocConfig moved to zfb-md-ast so MarkdownFeaturesConfig.heading_marker_toc
// can carry the rich shape from the canonical visitor-contract crate without
// a dep cycle. Re-exported below so the historical
// zfb_content::plugins::toc::TocConfig path still resolves.
pub use zfb_md_ast::TocConfig;

/// Hast visitor that inserts a `<ul>/<li>` table of contents after the
/// TOC anchor heading.
///
/// Must be registered **after** [`HeadingLinksPlugin`] in the hast phase
/// so that the `id` attributes it reads are the final, deduplicated slugs.
///
/// [`HeadingLinksPlugin`]: crate::plugins::HeadingLinksPlugin
pub struct TocPlugin {
    config: TocConfig,
}

impl TocPlugin {
    /// New plugin with the given configuration.
    #[must_use]
    pub fn new(config: TocConfig) -> Self {
        Self { config }
    }
}

impl HastVisitor for TocPlugin {
    fn visit(&mut self, node: &mut HastNode) {
        match node {
            HastNode::Root { children } => {
                rewrite_children(children, &self.config);
                // Recurse into children (TOC anchor might be nested, but
                // standard remark-toc only acts on top-level headings).
                for c in children {
                    self.visit(c);
                }
            }
            HastNode::Element { children, .. } => {
                rewrite_children(children, &self.config);
                for c in children {
                    self.visit(c);
                }
            }
            _ => {}
        }
    }
}

/// Scan `children` for a TOC anchor heading. When found, collect
/// subsequent headings within the configured depth range and splice a
/// `<ul>` TOC list at `anchor_index + 1`.
fn rewrite_children(children: &mut Vec<HastNode>, cfg: &TocConfig) {
    // Find the anchor heading index.
    let Some(anchor_idx) = find_anchor(children, cfg) else {
        return;
    };

    // Collect headings that follow the anchor.
    let max_level = 2u8.saturating_add(cfg.max_depth.min(5).saturating_sub(1));
    let entries: Vec<TocEntry> = children[anchor_idx + 1..]
        .iter()
        .filter_map(|n| heading_entry(n, max_level))
        .collect();

    if entries.is_empty() {
        return;
    }

    let toc_list = build_nested_list(&entries, 2);
    children.insert(anchor_idx + 1, toc_list);
}

/// Find the index of the TOC anchor heading in `children`.
fn find_anchor(children: &[HastNode], cfg: &TocConfig) -> Option<usize> {
    let target = cfg.heading.trim().to_lowercase();
    for (i, node) in children.iter().enumerate() {
        let HastNode::Element { tag, .. } = node else {
            continue;
        };
        // Accept any heading level (h1–h6) as a potential TOC anchor.
        if !is_heading_tag(tag) {
            continue;
        }
        // Extract the visible text, stripping the hash-link anchor that
        // HeadingLinksPlugin appended as the last empty child.
        let text = heading_text_without_anchor(node);
        if text.trim().to_lowercase() == target {
            return Some(i);
        }
    }
    None
}

/// A single TOC entry: heading level (2–6), display text, and anchor id.
#[derive(Debug)]
struct TocEntry {
    level: u8,
    text: String,
    id: String,
}

/// Extract a `TocEntry` from a heading element if it is within the allowed
/// depth range and has a non-empty `id` attribute.
fn heading_entry(node: &HastNode, max_level: u8) -> Option<TocEntry> {
    let HastNode::Element { tag, attrs, .. } = node else {
        return None;
    };
    let level = heading_level(tag)?;
    if level < 2 || level > max_level {
        return None;
    }
    let id = attrs
        .iter()
        .find(|(k, _)| k == "id")
        .map(|(_, v)| v.clone())?;
    if id.is_empty() {
        return None;
    }
    let text = heading_text_without_anchor(node);
    if text.is_empty() {
        return None;
    }
    Some(TocEntry { level, text, id })
}

/// Extract visible heading text, skipping the hash-link anchor that
/// `HeadingLinksPlugin` appended as an empty `<a class="hash-link">`.
fn heading_text_without_anchor(node: &HastNode) -> String {
    let HastNode::Element { children, .. } = node else {
        return extract_text(node);
    };
    // Filter out the trailing hash-link anchor before extracting text.
    let filtered: Vec<&HastNode> = children
        .iter()
        .filter(|c| !is_hash_link(c))
        .collect();
    let mut out = String::new();
    for c in filtered {
        out.push_str(&extract_text(c));
    }
    out
}

/// True for `<a class="hash-link">` anchors appended by `HeadingLinksPlugin`.
fn is_hash_link(node: &HastNode) -> bool {
    let HastNode::Element { tag, attrs, children, .. } = node else {
        return false;
    };
    tag == "a"
        && children.is_empty()
        && attrs.iter().any(|(k, v)| k == "class" && v == "hash-link")
}

fn is_heading_tag(tag: &str) -> bool {
    matches!(tag, "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
}

fn heading_level(tag: &str) -> Option<u8> {
    match tag {
        "h1" => Some(1),
        "h2" => Some(2),
        "h3" => Some(3),
        "h4" => Some(4),
        "h5" => Some(5),
        "h6" => Some(6),
        _ => None,
    }
}

/// Build a nested `<ul>` structure from a flat list of TOC entries.
///
/// Entries with `level == base_level` become top-level `<li>` items.
/// Entries with deeper levels are grouped into nested `<ul>` children of
/// the nearest shallower `<li>`.
///
/// Example for maxDepth=2 (h2+h3):
///
/// ```html
/// <ul>
///   <li><a href="#intro">Intro</a>
///     <ul>
///       <li><a href="#sub">Sub</a></li>
///     </ul>
///   </li>
///   <li><a href="#outro">Outro</a></li>
/// </ul>
/// ```
fn build_nested_list(entries: &[TocEntry], base_level: u8) -> HastNode {
    build_list_at_level(entries, base_level)
}

/// Recursively build a `<ul>` for items at `current_level`, grouping
/// deeper-level items as nested `<ul>` children of the nearest shallower item.
fn build_list_at_level(entries: &[TocEntry], current_level: u8) -> HastNode {
    let mut list_children: Vec<HastNode> = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        let entry = &entries[i];
        if entry.level < current_level {
            // Should not happen in well-formed documents; skip.
            i += 1;
            continue;
        }
        if entry.level == current_level {
            // Build a <li> for this entry.
            let link = HastNode::Element {
                tag: "a".to_string(),
                attrs: vec![("href".to_string(), format!("#{}", entry.id))],
                children: vec![HastNode::Text(entry.text.clone())],
                void: false,
            };
            // Collect consecutive deeper entries that belong to this item.
            let sub_start = i + 1;
            let mut sub_end = sub_start;
            while sub_end < entries.len() && entries[sub_end].level > current_level {
                sub_end += 1;
            }
            let mut li_children: Vec<HastNode> = vec![link];
            if sub_end > sub_start {
                let nested = build_list_at_level(&entries[sub_start..sub_end], current_level + 1);
                li_children.push(nested);
            }
            list_children.push(HastNode::Element {
                tag: "li".to_string(),
                attrs: vec![],
                children: li_children,
                void: false,
            });
            i = sub_end;
        } else {
            // Deeper item encountered without a parent at current_level;
            // skip to avoid orphaned nesting.
            i += 1;
        }
    }
    HastNode::Element {
        tag: "ul".to_string(),
        attrs: vec![],
        children: list_children,
        void: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ─────────────────────────────────────────────────────────────

    /// Build a heading element with `id` already set (as HeadingLinksPlugin
    /// would do), plus a trailing hash-link anchor (empty `<a class="hash-link">`).
    fn h_with_id(level: u8, text: &str, id: &str) -> HastNode {
        HastNode::Element {
            tag: format!("h{level}"),
            attrs: vec![("id".to_string(), id.to_string())],
            children: vec![
                HastNode::Text(text.to_string()),
                // The hash-link anchor HeadingLinksPlugin appends.
                HastNode::Element {
                    tag: "a".to_string(),
                    attrs: vec![
                        ("href".to_string(), format!("#{id}")),
                        ("class".to_string(), "hash-link".to_string()),
                    ],
                    children: vec![],
                    void: false,
                },
            ],
            void: false,
        }
    }

    fn h_toc(level: u8) -> HastNode {
        h_with_id(level, "TOC", "toc")
    }

    fn root(children: Vec<HastNode>) -> HastNode {
        HastNode::Root { children }
    }

    fn default_cfg() -> TocConfig {
        TocConfig::default()
    }

    fn cfg_with_depth(max_depth: u8) -> TocConfig {
        TocConfig {
            max_depth,
            ..TocConfig::default()
        }
    }

    fn cfg_with_heading(heading: &str) -> TocConfig {
        TocConfig {
            heading: heading.to_string(),
            ..TocConfig::default()
        }
    }

    /// Extract `href` values from a `<ul>` list recursively (depth-first).
    fn collect_hrefs(node: &HastNode) -> Vec<String> {
        let mut out = Vec::new();
        collect_hrefs_inner(node, &mut out);
        out
    }

    fn collect_hrefs_inner(node: &HastNode, out: &mut Vec<String>) {
        match node {
            HastNode::Element { tag, attrs, children, .. } => {
                if tag == "a" {
                    if let Some((_, v)) = attrs.iter().find(|(k, _)| k == "href") {
                        out.push(v.clone());
                    }
                }
                for c in children {
                    collect_hrefs_inner(c, out);
                }
            }
            _ => {}
        }
    }

    /// Get the `children` Vec from a Root or panic.
    fn root_children(tree: &HastNode) -> &Vec<HastNode> {
        let HastNode::Root { children } = tree else {
            panic!("expected Root");
        };
        children
    }

    // ── tests ────────────────────────────────────────────────────────────────

    /// Basic: TOC anchor present, two subsequent h2s → `<ul>` inserted.
    #[test]
    fn inserts_toc_after_anchor() {
        let mut tree = root(vec![
            h_toc(2),
            h_with_id(2, "Introduction", "introduction"),
            h_with_id(2, "Conclusion", "conclusion"),
        ]);
        TocPlugin::new(default_cfg()).visit(&mut tree);

        let children = root_children(&tree);
        // Original: [toc, intro, conclusion] → after: [toc, <ul>, intro, conclusion].
        assert_eq!(children.len(), 4, "TOC list must be inserted");
        let toc_list = &children[1];
        let HastNode::Element { tag, children: ul_children, .. } = toc_list else {
            panic!("expected <ul>");
        };
        assert_eq!(tag, "ul");
        assert_eq!(ul_children.len(), 2, "two h2 headings → two <li> items");

        // Each <li> contains an <a href="#...">
        let hrefs = collect_hrefs(toc_list);
        assert_eq!(hrefs, vec!["#introduction", "#conclusion"]);
    }

    /// Anchor absent → no mutation.
    #[test]
    fn no_anchor_no_op() {
        let original = root(vec![
            h_with_id(2, "Introduction", "introduction"),
            h_with_id(2, "Conclusion", "conclusion"),
        ]);
        let mut tree = original.clone();
        TocPlugin::new(default_cfg()).visit(&mut tree);
        assert_eq!(tree, original);
    }

    /// maxDepth=1 → only h2s, h3s excluded.
    #[test]
    fn max_depth_cutoff() {
        let mut tree = root(vec![
            h_toc(2),
            h_with_id(2, "Alpha", "alpha"),
            h_with_id(3, "Sub-Alpha", "sub-alpha"),
            h_with_id(2, "Beta", "beta"),
        ]);
        TocPlugin::new(cfg_with_depth(1)).visit(&mut tree);

        let children = root_children(&tree);
        let toc_list = &children[1];
        let hrefs = collect_hrefs(toc_list);
        // Only h2s
        assert_eq!(hrefs, vec!["#alpha", "#beta"]);
    }

    /// maxDepth=3 → h2, h3, h4 included.
    #[test]
    fn max_depth_3_includes_h4() {
        let mut tree = root(vec![
            h_toc(2),
            h_with_id(2, "A", "a"),
            h_with_id(3, "B", "b"),
            h_with_id(4, "C", "c"),
            h_with_id(5, "D", "d"), // h5 → excluded at maxDepth=3
        ]);
        TocPlugin::new(cfg_with_depth(3)).visit(&mut tree);

        let children = root_children(&tree);
        let toc_list = &children[1];
        let hrefs = collect_hrefs(toc_list);
        assert_eq!(hrefs, vec!["#a", "#b", "#c"]);
    }

    /// Duplicate heading text: TOC reads `id` from the element, so
    /// deduplicated ids (`slug`, `slug-1`, `slug-2`) link correctly.
    #[test]
    fn duplicate_heading_ids_link_correctly() {
        let mut tree = root(vec![
            h_toc(2),
            h_with_id(2, "Item", "item"),
            h_with_id(2, "Item", "item-1"),
            h_with_id(2, "Item", "item-2"),
        ]);
        TocPlugin::new(default_cfg()).visit(&mut tree);

        let children = root_children(&tree);
        let toc_list = &children[1];
        let hrefs = collect_hrefs(toc_list);
        assert_eq!(hrefs, vec!["#item", "#item-1", "#item-2"]);
    }

    /// Custom heading string (Japanese 目次), matched case-insensitively.
    #[test]
    fn custom_heading_anchor() {
        let mut tree = root(vec![
            // Anchor is "目次" with id already set.
            h_with_id(2, "目次", "目次"),
            h_with_id(2, "はじめに", "はじめに"),
        ]);
        TocPlugin::new(cfg_with_heading("目次")).visit(&mut tree);

        let children = root_children(&tree);
        assert_eq!(children.len(), 3, "TOC list must be inserted");
        let hrefs = collect_hrefs(&children[1]);
        assert_eq!(hrefs, vec!["#はじめに"]);
    }

    /// Case-insensitive anchor match: "toc" matches default "TOC".
    #[test]
    fn case_insensitive_anchor_match() {
        let mut tree = root(vec![
            h_with_id(2, "toc", "toc"),
            h_with_id(2, "Section", "section"),
        ]);
        TocPlugin::new(default_cfg()).visit(&mut tree);

        let children = root_children(&tree);
        // "toc" (lowercase) should have matched the default "TOC" anchor.
        assert_eq!(children.len(), 3);
    }

    /// Nesting: h2 + h3 produces nested `<ul>` under the h2 `<li>`.
    #[test]
    fn nesting_h2_h3() {
        let mut tree = root(vec![
            h_toc(2),
            h_with_id(2, "Alpha", "alpha"),
            h_with_id(3, "Sub", "sub"),
            h_with_id(2, "Beta", "beta"),
        ]);
        TocPlugin::new(default_cfg()).visit(&mut tree);

        let children = root_children(&tree);
        let toc_list = &children[1];
        // Top-level <ul> has 2 <li> items (alpha + beta).
        let HastNode::Element { children: ul_children, .. } = toc_list else {
            panic!("expected <ul>");
        };
        assert_eq!(ul_children.len(), 2, "two top-level items");

        // First <li> for alpha has a nested <ul> for sub.
        let HastNode::Element { children: li1_children, .. } = &ul_children[0] else {
            panic!("expected <li>");
        };
        // li1_children: [<a href="#alpha">, <ul>]
        assert_eq!(li1_children.len(), 2, "alpha <li> must have link + nested ul");
        let HastNode::Element { tag: nested_tag, children: nested_children, .. } =
            &li1_children[1]
        else {
            panic!("expected nested <ul>");
        };
        assert_eq!(nested_tag, "ul");
        assert_eq!(nested_children.len(), 1, "one h3 entry under alpha");
        let hrefs = collect_hrefs(&li1_children[1]);
        assert_eq!(hrefs, vec!["#sub"]);

        // Second <li> for beta has no nested ul.
        let HastNode::Element { children: li2_children, .. } = &ul_children[1] else {
            panic!("expected <li>");
        };
        assert_eq!(li2_children.len(), 1, "beta <li> has only link, no nested ul");
    }
}
