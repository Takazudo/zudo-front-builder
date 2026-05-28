//! TOC-export visitor (hast phase, runs after `HeadingLinksPlugin`).
//!
//! When enabled, walks the hast tree and collects all headings that have an
//! `id` attribute (placed by `HeadingLinksPlugin`), then injects an MDX named
//! export at the top of the document:
//!
//! ```js
//! export const toc = [{"depth":2,"id":"intro","text":"Introduction","children":[]}];
//! ```
//!
//! Frameworks (e.g. Fumadocs, zudo-doc) consume `entry.toc` to render
//! sidebars and floating-TOC components without scraping rendered HTML.
//!
//! # Phase choice
//!
//! Runs in the **hast phase** (not mdast) so that `HeadingLinksPlugin`-assigned
//! IDs are already final and deduplicated. The visitor is registered
//! **after** `HeadingLinksPlugin` in `register_features` (pipeline.rs).
//!
//! # Why MDX named export, not frontmatter?
//!
//! Wave 5 does not thread a `BuildContext` to all visitors in the standard
//! `Pipeline::run` path, so writing into `frontmatter` (a `BuildContext`
//! field) is not universally available. Emitting `export const toc = [...]` as
//! a `JsxRaw` node at the front of `HastNode::Root` works on both the HTML
//! serializer path (verbatim passthrough) and the JSX-emit path (inlined into
//! the MDX module), matching how other named exports (e.g. page `meta`) are
//! handled in this codebase.
//!
//! # Depth semantics
//!
//! `max_depth` is the **absolute heading depth** (2–6), unlike
//! `TocConfig::max_depth` (heading_marker_toc), which counts levels from h2.
//! With the default `max_depth: 3`, h2 and h3 are included; h1 is always
//! excluded because `HeadingLinksPlugin` only IDs h2–h6 (and h1 is the page
//! title by convention). See `TocExportConfig` in `zfb-md-ast` for the full
//! field reference.
//!
//! Wire via `markdown.features.tocExport` in `zfb.config.ts`:
//!
//! ```json
//! { "markdown": { "features": { "tocExport": {} } } }
//! { "markdown": { "features": { "tocExport": { "maxDepth": 2 } } } }
//! ```
//!
//! Ported in Wave 5 (#578). Separate from `heading_marker_toc.rs` (#570)
//! so users can enable data-export without the in-body heading-marker insertion.

use zfb_md_ast::{HastNode, HastVisitor, TocExportConfig, extract_text};

/// Hast visitor that injects `export const toc = [...]` at the top of the
/// document root.
///
/// Must be registered **after** `HeadingLinksPlugin` in the hast phase so
/// that `id` attributes are already final.
pub struct TocExportPlugin {
    config: TocExportConfig,
}

impl TocExportPlugin {
    /// Create a new plugin with the given configuration.
    #[must_use]
    pub fn new(config: TocExportConfig) -> Self {
        Self { config }
    }
}

impl HastVisitor for TocExportPlugin {
    fn visit(&mut self, node: &mut HastNode) {
        let HastNode::Root { children } = node else {
            return;
        };

        // Collect flat heading entries from the root's direct children only.
        // Nested headings (inside blockquotes, divs, etc.) are intentionally
        // excluded — only top-level document headings form the TOC.
        let max_depth = self.config.max_depth.clamp(2, 6);
        let flat: Vec<FlatEntry> = children
            .iter()
            .filter_map(|c| flat_entry(c, max_depth))
            .collect();

        let nested = build_nested(&flat, 2);
        let json = render_toc_json(&nested);

        // Emit as a JsxRaw node so both the HTML serializer (verbatim
        // passthrough) and the JSX-emit bridge (inlined into the MDX module)
        // handle it correctly — consistent with how other MDX named exports
        // (e.g. `export const meta`) are emitted in this codebase.
        let export_node = HastNode::JsxRaw(format!("export const toc = {json};\n"));

        // Insert at the front so the export appears before document HTML,
        // matching the ESM convention that exports appear at module top level.
        children.insert(0, export_node);
    }
}

// ── Internal types ────────────────────────────────────────────────────────────

#[derive(Debug)]
struct FlatEntry {
    depth: u8,
    id: String,
    text: String,
}

#[derive(Debug)]
struct TocEntry {
    depth: u8,
    id: String,
    text: String,
    children: Vec<TocEntry>,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract a `FlatEntry` from a hast node if it is an h2–h{max_depth} heading
/// with a non-empty `id` attribute.
fn flat_entry(node: &HastNode, max_depth: u8) -> Option<FlatEntry> {
    let HastNode::Element { tag, attrs, .. } = node else {
        return None;
    };
    let depth = heading_level(tag)?;
    if depth < 2 || depth > max_depth {
        return None;
    }
    let id = attrs
        .iter()
        .find(|(k, _)| k == "id")
        .map(|(_, v)| v.clone())?;
    if id.is_empty() {
        return None;
    }
    let text = heading_text_without_hash_link(node);
    if text.is_empty() {
        return None;
    }
    Some(FlatEntry { depth, id, text })
}

/// Strip the hash-link anchor appended by `HeadingLinksPlugin` and return
/// the heading's plain-text content. Mirrors the same helper in
/// `heading_marker_toc.rs`.
fn heading_text_without_hash_link(node: &HastNode) -> String {
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

/// Build a nested `TocEntry` tree from a flat list, grouping children of
/// each heading under their nearest parent.
///
/// The algorithm mirrors the nesting logic in `heading_marker_toc.rs`.
fn build_nested(flat: &[FlatEntry], base_depth: u8) -> Vec<TocEntry> {
    let mut result: Vec<TocEntry> = Vec::new();
    let mut i = 0;
    while i < flat.len() {
        let entry = &flat[i];
        if entry.depth < base_depth {
            // Shallower than expected — skip (shouldn't happen with h2 base).
            i += 1;
            continue;
        }
        if entry.depth == base_depth {
            // Gather all subsequent deeper entries as children of this one.
            let child_start = i + 1;
            let mut child_end = child_start;
            while child_end < flat.len() && flat[child_end].depth > base_depth {
                child_end += 1;
            }
            let children = build_nested(&flat[child_start..child_end], base_depth + 1);
            result.push(TocEntry {
                depth: entry.depth,
                id: entry.id.clone(),
                text: entry.text.clone(),
                children,
            });
            i = child_end;
        } else {
            // Deeper entry without a same-level parent — skip (malformed input).
            i += 1;
        }
    }
    result
}

/// Render the TOC tree as a compact JSON array literal.
///
/// Key order is fixed (depth, id, text, children) for stable output.
/// String values are JSON-escaped to handle quotes, backslashes, and
/// control characters in heading text.
fn render_toc_json(entries: &[TocEntry]) -> String {
    let items: Vec<String> = entries.iter().map(render_entry).collect();
    format!("[{}]", items.join(","))
}

fn render_entry(entry: &TocEntry) -> String {
    let children_json = render_toc_json(&entry.children);
    format!(
        "{{\"depth\":{},\"id\":{},\"text\":{},\"children\":{}}}",
        entry.depth,
        json_string(&entry.id),
        json_string(&entry.text),
        children_json,
    )
}

/// Minimal JSON string escaping: `"`, `\`, and control characters.
///
/// This is safe for heading text and slugified IDs; we don't need full
/// Unicode escape support because heading text is UTF-8 and IDs are
/// ASCII slugs from `HeadingLinksPlugin`.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                // Escape other control characters as \uXXXX.
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn h_with_id(level: u8, text: &str, id: &str) -> HastNode {
        HastNode::Element {
            tag: format!("h{level}"),
            attrs: vec![("id".to_string(), id.to_string())],
            children: vec![
                HastNode::Text(text.to_string()),
                // Hash-link appended by HeadingLinksPlugin.
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

    fn root(children: Vec<HastNode>) -> HastNode {
        HastNode::Root { children }
    }

    fn run(mut tree: HastNode, cfg: TocExportConfig) -> HastNode {
        TocExportPlugin::new(cfg).visit(&mut tree);
        tree
    }

    fn extract_export(tree: &HastNode) -> String {
        let HastNode::Root { children } = tree else {
            panic!("expected Root");
        };
        match children.first() {
            Some(HastNode::JsxRaw(s)) => s.clone(),
            other => panic!("expected JsxRaw as first child, got: {:?}", other),
        }
    }

    #[test]
    fn empty_document_emits_empty_toc() {
        let tree = run(root(vec![]), TocExportConfig::default());
        let export = extract_export(&tree);
        assert_eq!(export, "export const toc = [];\n");
    }

    #[test]
    fn single_h2_no_children() {
        let tree = run(
            root(vec![h_with_id(2, "Hello", "hello")]),
            TocExportConfig::default(),
        );
        let export = extract_export(&tree);
        assert_eq!(
            export,
            "export const toc = [{\"depth\":2,\"id\":\"hello\",\"text\":\"Hello\",\"children\":[]}];\n"
        );
    }

    #[test]
    fn h2_with_nested_h3() {
        let tree = run(
            root(vec![
                h_with_id(2, "Alpha", "alpha"),
                h_with_id(3, "Sub", "sub"),
                h_with_id(2, "Beta", "beta"),
            ]),
            TocExportConfig::default(),
        );
        let export = extract_export(&tree);
        assert_eq!(
            export,
            "export const toc = [\
{\"depth\":2,\"id\":\"alpha\",\"text\":\"Alpha\",\"children\":[{\"depth\":3,\"id\":\"sub\",\"text\":\"Sub\",\"children\":[]}]},\
{\"depth\":2,\"id\":\"beta\",\"text\":\"Beta\",\"children\":[]}\
];\n"
        );
    }

    #[test]
    fn max_depth_2_excludes_h3() {
        let tree = run(
            root(vec![
                h_with_id(2, "Alpha", "alpha"),
                h_with_id(3, "Sub", "sub"),
                h_with_id(2, "Beta", "beta"),
            ]),
            TocExportConfig { max_depth: 2 },
        );
        let export = extract_export(&tree);
        // Only h2 entries; h3 is filtered out.
        assert_eq!(
            export,
            "export const toc = [\
{\"depth\":2,\"id\":\"alpha\",\"text\":\"Alpha\",\"children\":[]},\
{\"depth\":2,\"id\":\"beta\",\"text\":\"Beta\",\"children\":[]}\
];\n"
        );
    }

    #[test]
    fn heading_without_id_is_skipped() {
        let no_id_heading = HastNode::Element {
            tag: "h2".to_string(),
            attrs: vec![],
            children: vec![HastNode::Text("No ID".to_string())],
            void: false,
        };
        let tree = run(
            root(vec![no_id_heading, h_with_id(2, "Has ID", "has-id")]),
            TocExportConfig::default(),
        );
        let export = extract_export(&tree);
        assert_eq!(
            export,
            "export const toc = [{\"depth\":2,\"id\":\"has-id\",\"text\":\"Has ID\",\"children\":[]}];\n"
        );
    }

    #[test]
    fn text_with_special_chars_is_json_escaped() {
        let tree = run(
            root(vec![h_with_id(2, r#"Say "hello" & <bye>"#, "say-hello")]),
            TocExportConfig::default(),
        );
        let export = extract_export(&tree);
        // Double-quote must be escaped; angle brackets are safe in JSON strings.
        assert!(export.contains(r#"\"hello\""#), "quotes must be escaped: {export}");
    }

    #[test]
    fn hash_link_text_is_stripped_from_heading_text() {
        // HeadingLinksPlugin adds an empty <a class="hash-link"> child.
        // Its text (empty) must not bleed into the exported text.
        let tree = run(
            root(vec![h_with_id(2, "Clean", "clean")]),
            TocExportConfig::default(),
        );
        let export = extract_export(&tree);
        assert!(export.contains("\"text\":\"Clean\""), "text must be clean: {export}");
    }

    #[test]
    fn non_root_call_is_noop() {
        // Visiting a non-Root node must not mutate anything.
        let mut node = h_with_id(2, "Title", "title");
        TocExportPlugin::new(TocExportConfig::default()).visit(&mut node);
        // If the node is unchanged (still an Element), the visitor was a no-op.
        assert!(matches!(node, HastNode::Element { .. }));
    }
}
