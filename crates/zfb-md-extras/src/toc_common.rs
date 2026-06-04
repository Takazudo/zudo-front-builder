//! Shared pure helpers used by both `toc_export` and `heading_marker_toc`.
//!
//! These three functions are logically identical in both visitors; they live
//! here to avoid byte-for-byte duplication (source finding #704).

use zfb_md_ast::{extract_text, HastNode};

/// Return `true` if `node` is the empty `<a class="hash-link">` anchor
/// appended by `HeadingLinksPlugin`.
pub(crate) fn is_hash_link(node: &HastNode) -> bool {
    let HastNode::Element {
        tag,
        attrs,
        children,
        ..
    } = node
    else {
        return false;
    };
    tag == "a" && children.is_empty() && attrs.iter().any(|(k, v)| k == "class" && v == "hash-link")
}

/// Map an HTML heading tag name (`"h1"`…`"h6"`) to its numeric depth.
/// Returns `None` for any other tag.
pub(crate) fn heading_level(tag: &str) -> Option<u8> {
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

/// Strip the hash-link anchor appended by `HeadingLinksPlugin` and return
/// the heading's plain-text content.
pub(crate) fn heading_text_without_hash_link(node: &HastNode) -> String {
    let HastNode::Element { children, .. } = node else {
        return extract_text(node);
    };
    let filtered: Vec<&HastNode> = children.iter().filter(|c| !is_hash_link(c)).collect();
    let mut out = String::new();
    for c in filtered {
        out.push_str(&extract_text(c));
    }
    out
}
