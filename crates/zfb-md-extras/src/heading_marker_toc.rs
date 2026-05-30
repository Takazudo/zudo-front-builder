//! Table-of-contents visitor (hast phase, runs after `HeadingLinksPlugin`).
//!
//! Rust port of `remark-toc`. When the hast tree contains a heading whose
//! text matches the configured anchor string (default `"TOC"`,
//! case-insensitive), a nested `<ul>/<li>` list of the *subsequent* headings
//! is inserted as the next sibling.
//!
//! **Phase choice:** hast (not mdast). Running after `HeadingLinksPlugin`
//! lets this visitor read the final `id` attributes that plugin already
//! placed on every `<h2>`–`<h6>`, so we never need to re-run the slugger or
//! deduplicate. Each `<a href="#<id>">` in the TOC simply mirrors whatever
//! `id` the heading has.
//!
//! ## Configuration
//!
//! Wire via `markdown.features.headingMarkerToc` in `zfb.config.ts`:
//!
//! ```json
//! { "markdown": { "features": { "headingMarkerToc": { "heading": "TOC", "maxDepth": 2 } } } }
//! ```
//!
//! - `heading` — the heading text that triggers TOC insertion (default
//!   `"TOC"`, matched case-insensitively after whitespace trimming).
//! - `maxDepth` — how many depth levels to include starting from `<h2>`.
//!   `1` → h2 only, `2` → h2+h3 (default), `3` → h2+h3+h4, etc.
//!   Maximum is 5 (h2 through h6).
//!
//! Moved from `zfb-content::plugins::toc` in Wave 3 (#570).
//! Wire via `features.headingMarkerToc = { heading: "TOC", maxDepth: 2 }`
//! in `zfb.config.ts`.

use zfb_md_ast::{HastNode, HastVisitor, TocConfig, extract_text};

/// Hast visitor that inserts a `<ul>/<li>` table of contents after the
/// TOC anchor heading.
///
/// Must be registered **after** `HeadingLinksPlugin` in the hast phase
/// so that the `id` attributes it reads are the final, deduplicated slugs.
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

fn rewrite_children(children: &mut Vec<HastNode>, cfg: &TocConfig) {
    let Some(anchor_idx) = find_anchor(children, cfg) else {
        return;
    };

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

fn find_anchor(children: &[HastNode], cfg: &TocConfig) -> Option<usize> {
    let target = cfg.heading.trim().to_lowercase();
    for (i, node) in children.iter().enumerate() {
        let HastNode::Element { tag, .. } = node else {
            continue;
        };
        if !is_heading_tag(tag) {
            continue;
        }
        let text = heading_text_without_anchor(node);
        if text.trim().to_lowercase() == target {
            return Some(i);
        }
    }
    None
}

#[derive(Debug)]
struct TocEntry {
    level: u8,
    text: String,
    id: String,
}

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

fn heading_text_without_anchor(node: &HastNode) -> String {
    let HastNode::Element { children, .. } = node else {
        return extract_text(node);
    };
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

fn build_nested_list(entries: &[TocEntry], base_level: u8) -> HastNode {
    build_list_at_level(entries, base_level)
}

fn build_list_at_level(entries: &[TocEntry], current_level: u8) -> HastNode {
    let mut list_children: Vec<HastNode> = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        let entry = &entries[i];
        if entry.level < current_level {
            i += 1;
            continue;
        }
        if entry.level == current_level {
            let link = HastNode::Element {
                tag: "a".to_string(),
                attrs: vec![("href".to_string(), format!("#{}", entry.id))],
                children: vec![HastNode::Text(entry.text.clone())],
                void: false,
            };
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

    fn h_with_id(level: u8, text: &str, id: &str) -> HastNode {
        HastNode::Element {
            tag: format!("h{level}"),
            attrs: vec![("id".to_string(), id.to_string())],
            children: vec![
                HastNode::Text(text.to_string()),
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

    fn collect_hrefs(node: &HastNode) -> Vec<String> {
        let mut out = Vec::new();
        collect_hrefs_inner(node, &mut out);
        out
    }

    fn collect_hrefs_inner(node: &HastNode, out: &mut Vec<String>) {
        if let HastNode::Element { tag, attrs, children, .. } = node {
            if tag == "a" {
                if let Some((_, v)) = attrs.iter().find(|(k, _)| k == "href") {
                    out.push(v.clone());
                }
            }
            for c in children {
                collect_hrefs_inner(c, out);
            }
        }
    }

    fn root_children(tree: &HastNode) -> &Vec<HastNode> {
        let HastNode::Root { children } = tree else {
            panic!("expected Root");
        };
        children
    }

    #[test]
    fn inserts_toc_after_anchor() {
        let mut tree = root(vec![
            h_toc(2),
            h_with_id(2, "Introduction", "introduction"),
            h_with_id(2, "Conclusion", "conclusion"),
        ]);
        TocPlugin::new(default_cfg()).visit(&mut tree);

        let children = root_children(&tree);
        assert_eq!(children.len(), 4, "TOC list must be inserted");
        let toc_list = &children[1];
        let HastNode::Element { tag, children: ul_children, .. } = toc_list else {
            panic!("expected <ul>");
        };
        assert_eq!(tag, "ul");
        assert_eq!(ul_children.len(), 2, "two h2 headings → two <li> items");

        let hrefs = collect_hrefs(toc_list);
        assert_eq!(hrefs, vec!["#introduction", "#conclusion"]);
    }

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
        assert_eq!(hrefs, vec!["#alpha", "#beta"]);
    }

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
        let HastNode::Element { children: ul_children, .. } = toc_list else {
            panic!("expected <ul>");
        };
        assert_eq!(ul_children.len(), 2, "two top-level items");

        let HastNode::Element { children: li1_children, .. } = &ul_children[0] else {
            panic!("expected <li>");
        };
        assert_eq!(li1_children.len(), 2, "alpha <li> must have link + nested ul");
        let hrefs = collect_hrefs(&li1_children[1]);
        assert_eq!(hrefs, vec!["#sub"]);
    }
}
