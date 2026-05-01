//! Wrap block-level paragraph images in an enlargeable `<figure>`.
//!
//! Rust port of zudo-doc's `rehypeImageEnlarge`. Selector and shape
//! match the JS plugin verbatim:
//!
//! - **Selector:** any `<p>` whose only non-whitespace child is `<img>`
//!   (whitespace-only text siblings around the `<img>` are ignored).
//!   `<a>`, `<picture>`, multiple `<img>` siblings, or any non-whitespace
//!   text leave the `<p>` alone.
//! - **Output:** the `<p>` is replaced with
//!   `<figure class="zd-enlargeable"><img …><button class="zd-enlarge-btn">…SVG…</button></figure>`.
//!   The button body is an inline 4-polygon SVG copied byte-for-byte
//!   from the JS reference (the corner-arrow "enlarge" glyph).
//! - **Opt-out:** `title="no-enlarge"` (case-sensitive, exact match) on
//!   the `<img>` skips the wrap AND drops the sentinel from the rendered
//!   `<img>` so it never reaches the DOM.
//! - **Idempotency:** a `<p>` whose only non-whitespace child is an
//!   already-wrapped `<figure class="zd-enlargeable">` is left alone.
//!
//! No behaviour is keyed on the `<img>`'s `width` attribute (that was
//! the pre-Sub-6 Rust selector — removed here per the engine-alignment
//! mandate). The previous `<ImageEnlarge>` JSX-marker emission path is
//! gone too: no consumer depends on it post-cutover.

use crate::pipeline::{HastNode, HastVisitor};

/// Visitor that wraps block-level `<p><img></p>` paragraphs in
/// `<figure class="zd-enlargeable">`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ImageEnlargePlugin;

impl ImageEnlargePlugin {
    /// New plugin (stateless).
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl HastVisitor for ImageEnlargePlugin {
    fn visit(&mut self, node: &mut HastNode) {
        match node {
            HastNode::Root { children } | HastNode::Element { children, .. } => {
                rewrite_children(children);
                for c in children {
                    self.visit(c);
                }
            }
            _ => {}
        }
    }
}

fn rewrite_children(children: &mut [HastNode]) {
    for child in children.iter_mut() {
        try_rewrite_paragraph(child);
    }
}

fn try_rewrite_paragraph(node: &mut HastNode) {
    let HastNode::Element { tag, children, .. } = node else {
        return;
    };
    if tag != "p" {
        return;
    }

    // Collect indices of non-whitespace children so we can decide
    // shape AND mutate the matched <img> in place if the opt-out fires.
    let non_ws_indices: Vec<usize> = children
        .iter()
        .enumerate()
        .filter(|(_, c)| !is_whitespace_text(c))
        .map(|(i, _)| i)
        .collect();
    if non_ws_indices.len() != 1 {
        return;
    }
    let only = non_ws_indices[0];

    // Idempotency: <p>'s sole non-whitespace child is an already-wrapped
    // figure → leave it alone.
    if is_enlargeable_figure(&children[only]) {
        return;
    }

    // Selector: the sole non-whitespace child must be an <img>. An
    // <a><img></a> wrapper makes <a> the only child, not <img>, so the
    // wrap does not apply (locked decision per Sub 6 spec).
    let HastNode::Element {
        tag: child_tag,
        attrs: img_attrs,
        ..
    } = &children[only]
    else {
        return;
    };
    if child_tag != "img" {
        return;
    }

    // Opt-out: title="no-enlarge" (case-sensitive, exact). Strip the
    // title sentinel from the <img> attrs and skip the wrap.
    if img_attrs
        .iter()
        .any(|(k, v)| k == "title" && v == "no-enlarge")
    {
        let HastNode::Element {
            attrs: img_attrs_mut,
            ..
        } = &mut children[only]
        else {
            return;
        };
        img_attrs_mut.retain(|(k, _)| k != "title");
        return;
    }

    // Wrap: replace the <p>'s entire children with an enlargeable
    // <figure>. We move the matched <img> out of the paragraph to
    // preserve attribute identity.
    let img = std::mem::replace(&mut children[only], HastNode::Text(String::new()));

    *node = HastNode::Element {
        tag: "figure".to_string(),
        attrs: vec![("class".to_string(), "zd-enlargeable".to_string())],
        children: vec![img, enlarge_button()],
        void: false,
    };
}

/// True for a [`HastNode::Text`] whose value is whitespace-only (or
/// empty). Mirrors the JS `isWhitespaceText` predicate.
fn is_whitespace_text(node: &HastNode) -> bool {
    let HastNode::Text(s) = node else {
        return false;
    };
    s.chars().all(char::is_whitespace)
}

/// True for a `<figure class="zd-enlargeable">` produced by a previous
/// run of this plugin (idempotency guard).
fn is_enlargeable_figure(node: &HastNode) -> bool {
    let HastNode::Element { tag, attrs, .. } = node else {
        return false;
    };
    if tag != "figure" {
        return false;
    }
    attrs
        .iter()
        .any(|(k, v)| k == "class" && class_contains(v, "zd-enlargeable"))
}

fn class_contains(value: &str, class: &str) -> bool {
    value.split_ascii_whitespace().any(|c| c == class)
}

/// Build the `<button>` payload. The SVG points are copied
/// byte-for-byte from `rehype-image-enlarge.ts` so the rendered DOM
/// matches the JS plugin shape.
fn enlarge_button() -> HastNode {
    HastNode::Element {
        tag: "button".to_string(),
        // Attribute order matters for byte-for-byte parity with the
        // hast-util-to-html serialization of the JS plugin's properties
        // map (insertion order: type, class, aria-label, hidden).
        attrs: vec![
            ("type".to_string(), "button".to_string()),
            ("class".to_string(), "zd-enlarge-btn".to_string()),
            ("aria-label".to_string(), "Enlarge image".to_string()),
            ("hidden".to_string(), String::new()),
        ],
        children: vec![enlarge_svg()],
        void: false,
    }
}

fn enlarge_svg() -> HastNode {
    HastNode::Element {
        tag: "svg".to_string(),
        attrs: vec![
            ("viewBox".to_string(), "0 0 38.99 38.99".to_string()),
            ("fill".to_string(), "currentColor".to_string()),
            ("focusable".to_string(), "false".to_string()),
            ("aria-hidden".to_string(), "true".to_string()),
        ],
        children: vec![
            polygon(
                "16.2 13.74 5.92 3.47 11.2 3.47 11.2 0 3.47 0 0 0 0 3.47 0 11.2 3.47 11.2 3.47 5.92 13.74 16.2 16.2 13.74",
            ),
            polygon(
                "25.24 16.2 35.52 5.92 35.52 11.2 38.99 11.2 38.99 3.47 38.99 0 35.52 0 27.79 0 27.79 3.47 33.07 3.47 22.79 13.74 25.24 16.2",
            ),
            polygon(
                "22.79 25.24 33.07 35.52 27.79 35.52 27.79 38.99 35.52 38.99 38.99 38.99 38.99 35.52 38.99 27.79 35.52 27.79 35.52 33.07 25.24 22.79 22.79 25.24",
            ),
            polygon(
                "13.74 22.79 3.47 33.07 3.47 27.79 0 27.79 0 35.52 0 38.99 3.47 38.99 11.2 38.99 11.2 35.52 5.92 35.52 16.2 25.24 13.74 22.79",
            ),
        ],
        void: false,
    }
}

fn polygon(points: &str) -> HastNode {
    HastNode::Element {
        tag: "polygon".to_string(),
        attrs: vec![("points".to_string(), points.to_string())],
        children: vec![],
        void: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(attrs: &[(&str, &str)]) -> HastNode {
        HastNode::Element {
            tag: "img".to_string(),
            attrs: attrs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            children: vec![],
            void: true,
        }
    }

    fn p(children: Vec<HastNode>) -> HastNode {
        HastNode::Element {
            tag: "p".to_string(),
            attrs: vec![],
            children,
            void: false,
        }
    }

    fn root(children: Vec<HastNode>) -> HastNode {
        HastNode::Root { children }
    }

    fn first_child(node: &HastNode) -> &HastNode {
        let HastNode::Root { children } = node else {
            unreachable!("expected root");
        };
        &children[0]
    }

    fn assert_wrapped(node: &HastNode) {
        let HastNode::Element { tag, attrs, children, .. } = node else {
            panic!("expected element, got {node:?}");
        };
        assert_eq!(tag, "figure");
        assert!(attrs.contains(&("class".to_string(), "zd-enlargeable".to_string())));
        assert_eq!(children.len(), 2, "figure must have img + button");
        let HastNode::Element { tag: img_tag, .. } = &children[0] else {
            panic!("first figure child must be element");
        };
        assert_eq!(img_tag, "img");
        let HastNode::Element {
            tag: btn_tag,
            attrs: btn_attrs,
            children: btn_children,
            ..
        } = &children[1]
        else {
            panic!("second figure child must be element");
        };
        assert_eq!(btn_tag, "button");
        assert!(btn_attrs.contains(&("type".to_string(), "button".to_string())));
        assert!(btn_attrs.contains(&("class".to_string(), "zd-enlarge-btn".to_string())));
        assert!(btn_attrs.iter().any(|(k, _)| k == "hidden"));
        let HastNode::Element {
            tag: svg_tag,
            children: svg_children,
            ..
        } = &btn_children[0]
        else {
            panic!("button must contain svg");
        };
        assert_eq!(svg_tag, "svg");
        let polygon_count = svg_children
            .iter()
            .filter(|c| matches!(c, HastNode::Element { tag, .. } if tag == "polygon"))
            .count();
        assert_eq!(polygon_count, 4, "svg must have 4 polygon children");
    }

    // <p><img></p> with no surrounding text — wrap.
    #[test]
    fn wraps_block_image() {
        let mut tree = root(vec![p(vec![img(&[("src", "pic.png"), ("alt", "x")])])]);
        ImageEnlargePlugin::new().visit(&mut tree);
        assert_wrapped(first_child(&tree));
    }

    // Whitespace-only siblings around the <img> — wrap.
    #[test]
    fn wraps_block_image_with_whitespace_siblings() {
        let mut tree = root(vec![p(vec![
            HastNode::Text("\n  ".into()),
            img(&[("src", "pic.png"), ("alt", "x")]),
            HastNode::Text("\n".into()),
        ])]);
        ImageEnlargePlugin::new().visit(&mut tree);
        assert_wrapped(first_child(&tree));
    }

    // Non-whitespace text before/after the img — leave alone.
    #[test]
    fn leaves_inline_image_with_text_alone() {
        let original = root(vec![p(vec![
            HastNode::Text("text ".into()),
            img(&[("src", "pic.png"), ("alt", "x")]),
        ])]);
        let mut tree = original.clone();
        ImageEnlargePlugin::new().visit(&mut tree);
        assert_eq!(tree, original);
    }

    // Multiple <img> children — leave alone.
    #[test]
    fn leaves_multiple_images_alone() {
        let original = root(vec![p(vec![
            img(&[("src", "a.png"), ("alt", "a")]),
            img(&[("src", "b.png"), ("alt", "b")]),
        ])]);
        let mut tree = original.clone();
        ImageEnlargePlugin::new().visit(&mut tree);
        assert_eq!(tree, original);
    }

    // <p><a><img></a></p> — anchor-wrapped image is NOT in scope.
    #[test]
    fn leaves_anchor_wrapped_image_alone() {
        let a = HastNode::Element {
            tag: "a".to_string(),
            attrs: vec![("href".to_string(), "/x".to_string())],
            children: vec![img(&[("src", "pic.png"), ("alt", "x")])],
            void: false,
        };
        let original = root(vec![p(vec![a])]);
        let mut tree = original.clone();
        ImageEnlargePlugin::new().visit(&mut tree);
        assert_eq!(tree, original);
    }

    // <p><img title="no-enlarge"></p> — leave the <p> untouched but
    // strip the title from the <img>.
    #[test]
    fn opt_out_strips_title_and_does_not_wrap() {
        let mut tree = root(vec![p(vec![img(&[
            ("src", "pic.png"),
            ("alt", "x"),
            ("title", "no-enlarge"),
        ])])]);
        ImageEnlargePlugin::new().visit(&mut tree);
        let HastNode::Element {
            tag,
            children: p_children,
            ..
        } = first_child(&tree)
        else {
            panic!("expected element");
        };
        assert_eq!(tag, "p");
        let HastNode::Element {
            tag: img_tag,
            attrs,
            ..
        } = &p_children[0]
        else {
            panic!("expected img");
        };
        assert_eq!(img_tag, "img");
        assert!(
            !attrs.iter().any(|(k, _)| k == "title"),
            "title attr must be stripped",
        );
        assert!(attrs.iter().any(|(k, v)| k == "src" && v == "pic.png"));
        assert!(attrs.iter().any(|(k, v)| k == "alt" && v == "x"));
    }

    // Case mismatch — opt-out is case-sensitive, so this still wraps.
    #[test]
    fn opt_out_is_case_sensitive() {
        let mut tree = root(vec![p(vec![img(&[
            ("src", "pic.png"),
            ("title", "No-Enlarge"),
        ])])]);
        ImageEnlargePlugin::new().visit(&mut tree);
        assert_wrapped(first_child(&tree));
        // The title attr survives onto the wrapped <img>.
        let HastNode::Element { children, .. } = first_child(&tree) else {
            panic!("expected figure");
        };
        let HastNode::Element { attrs, .. } = &children[0] else {
            panic!("expected img inside figure");
        };
        assert!(attrs.iter().any(|(k, v)| k == "title" && v == "No-Enlarge"));
    }

    // Lazy-loaded images wrap as normal.
    #[test]
    fn wraps_lazy_loaded_image() {
        let mut tree = root(vec![p(vec![img(&[
            ("src", "pic.png"),
            ("loading", "lazy"),
        ])])]);
        ImageEnlargePlugin::new().visit(&mut tree);
        assert_wrapped(first_child(&tree));
    }

    // SVG <img> follows the standard <img> path.
    #[test]
    fn wraps_svg_img() {
        let mut tree = root(vec![p(vec![img(&[("src", "x.svg"), ("alt", "v")])])]);
        ImageEnlargePlugin::new().visit(&mut tree);
        assert_wrapped(first_child(&tree));
    }

    // Already-wrapped figure inside a <p> — idempotent: leave alone.
    #[test]
    fn idempotent_when_paragraph_already_holds_figure() {
        let figure = HastNode::Element {
            tag: "figure".to_string(),
            attrs: vec![("class".to_string(), "zd-enlargeable".to_string())],
            children: vec![
                img(&[("src", "pic.png"), ("alt", "x")]),
                enlarge_button(),
            ],
            void: false,
        };
        let original = root(vec![p(vec![figure])]);
        let mut tree = original.clone();
        ImageEnlargePlugin::new().visit(&mut tree);
        assert_eq!(tree, original);
    }

    // <picture> as the only child — out of scope (only <img> matches).
    #[test]
    fn leaves_picture_element_alone() {
        let picture = HastNode::Element {
            tag: "picture".to_string(),
            attrs: vec![],
            children: vec![img(&[("src", "pic.png"), ("alt", "x")])],
            void: false,
        };
        let original = root(vec![p(vec![picture])]);
        let mut tree = original.clone();
        ImageEnlargePlugin::new().visit(&mut tree);
        assert_eq!(tree, original);
    }
}
