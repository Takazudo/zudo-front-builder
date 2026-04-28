//! Replace `<img width="…">` with the `<ImageEnlarge>` MDX
//! component marker.
//!
//! Rust port of zudo-doc's `rehypeImageEnlarge`. Only images that
//! have an explicit `width` attribute are wrapped — bare `<img>`
//! elements are left alone. The actual `<ImageEnlarge>` component is
//! provided downstream in Epic 3; here we just emit the marker as
//! [`HastNode::Raw`] so the serializer passes it through verbatim.

use crate::pipeline::{HastNode, HastVisitor};

/// Visitor that swaps `<img>` for `<ImageEnlarge>` markers.
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
        if let Some(raw) = build_image_enlarge(child) {
            *child = HastNode::Raw(raw);
        }
    }
}

fn build_image_enlarge(node: &HastNode) -> Option<String> {
    let HastNode::Element { tag, attrs, .. } = node else {
        return None;
    };
    if tag != "img" {
        return None;
    }
    if !attrs.iter().any(|(k, _)| k == "width") {
        return None;
    }

    let mut buf = String::from("<ImageEnlarge");
    for (k, v) in attrs {
        // Render as JSX-style literal. Escape only the double quote
        // and `&` to keep the marker safe to round-trip through the
        // serializer.
        buf.push(' ');
        buf.push_str(k);
        buf.push_str("=\"");
        buf.push_str(&jsx_attr_escape(v));
        buf.push('"');
    }
    buf.push_str(" />");
    Some(buf)
}

fn jsx_attr_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
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

    #[test]
    fn wraps_img_with_width() {
        let mut tree = HastNode::Root {
            children: vec![img(&[
                ("src", "pic.png"),
                ("alt", "x"),
                ("width", "120"),
                ("height", "80"),
            ])],
        };
        ImageEnlargePlugin::new().visit(&mut tree);
        let HastNode::Root { children } = tree else {
            unreachable!("expected HastNode::Root after visit")
        };
        let HastNode::Raw(raw) = &children[0] else {
            unreachable!("expected Raw, got {:?}", children[0])
        };
        assert!(raw.starts_with("<ImageEnlarge"));
        assert!(raw.contains("src=\"pic.png\""));
        assert!(raw.contains("width=\"120\""));
        assert!(raw.contains("height=\"80\""));
        assert!(raw.ends_with("/>"));
    }

    #[test]
    fn leaves_img_without_width() {
        let original = HastNode::Root {
            children: vec![img(&[("src", "pic.png"), ("alt", "x")])],
        };
        let mut tree = original.clone();
        ImageEnlargePlugin::new().visit(&mut tree);
        assert_eq!(tree, original);
    }

    #[test]
    fn escapes_special_chars_in_attrs() {
        let mut tree = HastNode::Root {
            children: vec![img(&[
                ("src", "pic.png"),
                ("alt", "a&b\"c"),
                ("width", "100"),
            ])],
        };
        ImageEnlargePlugin::new().visit(&mut tree);
        let HastNode::Root { children } = tree else {
            unreachable!("expected HastNode::Root after visit")
        };
        let HastNode::Raw(raw) = &children[0] else {
            unreachable!("expected Raw, got {:?}", children[0])
        };
        assert!(raw.contains("alt=\"a&amp;b&quot;c\""));
    }
}
