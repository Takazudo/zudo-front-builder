//! Strip `.md` / `.mdx` from internal `<a href>` paths.
//!
//! Rust port of zudo-doc's `rehypeStripMdExtension`. Only internal
//! links (per [`crate::plugins::util::url::is_external`]) are
//! rewritten — external URLs are left alone. The query string and
//! fragment are preserved across the rewrite.
//!
//! Typically chained AFTER [`crate::plugins::ResolveLinksPlugin`]:
//! that one rewrites mapped links to their final URLs (which usually
//! end in `/`); this one cleans up any unmapped internal links the
//! author wrote with explicit `.md` extensions.

use crate::pipeline::{HastNode, HastVisitor};
use crate::plugins::util::url::is_external;

/// Visitor that strips `.md` / `.mdx` from internal anchor hrefs.
#[derive(Debug, Default, Clone, Copy)]
pub struct StripMdExtensionPlugin;

impl StripMdExtensionPlugin {
    /// New plugin (stateless).
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl HastVisitor for StripMdExtensionPlugin {
    fn visit(&mut self, node: &mut HastNode) {
        if let HastNode::Element { tag, attrs, .. } = node {
            if tag == "a" {
                for (k, v) in attrs.iter_mut() {
                    if k == "href" {
                        if let Some(rewritten) = strip_md(v) {
                            *v = rewritten;
                        }
                    }
                }
            }
        }
        match node {
            HastNode::Root { children } | HastNode::Element { children, .. } => {
                for c in children {
                    self.visit(c);
                }
            }
            _ => {}
        }
    }
}

fn strip_md(url: &str) -> Option<String> {
    if is_external(url) {
        return None;
    }
    let (path, suffix) = split_suffix(url);
    let stripped = path
        .strip_suffix(".mdx")
        .or_else(|| path.strip_suffix(".md"))?;
    Some(format!("{stripped}{suffix}"))
}

fn split_suffix(url: &str) -> (&str, &str) {
    if let Some(i) = url.find(['?', '#']) {
        (&url[..i], &url[i..])
    } else {
        (url, "")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(href: &str) -> HastNode {
        HastNode::Element {
            tag: "a".to_string(),
            attrs: vec![("href".to_string(), href.to_string())],
            children: vec![HastNode::Text("link".into())],
            void: false,
        }
    }

    fn root_anchor(href: &str) -> HastNode {
        HastNode::Root {
            children: vec![anchor(href)],
        }
    }

    fn href(node: &HastNode) -> String {
        let HastNode::Root { children } = node else {
            unreachable!("expected HastNode::Root")
        };
        let HastNode::Element { attrs, .. } = &children[0] else {
            unreachable!("expected HastNode::Element")
        };
        attrs.iter().find(|(k, _)| k == "href").unwrap().1.clone()
    }

    #[test]
    fn strips_md_from_internal_link() {
        let mut t = root_anchor("./foo.md");
        StripMdExtensionPlugin::new().visit(&mut t);
        assert_eq!(href(&t), "./foo");
    }

    #[test]
    fn strips_mdx_from_internal_link() {
        let mut t = root_anchor("nested/bar.mdx");
        StripMdExtensionPlugin::new().visit(&mut t);
        assert_eq!(href(&t), "nested/bar");
    }

    #[test]
    fn preserves_query_and_fragment() {
        let mut t = root_anchor("foo.md#sec");
        StripMdExtensionPlugin::new().visit(&mut t);
        assert_eq!(href(&t), "foo#sec");

        let mut t = root_anchor("foo.mdx?x=1");
        StripMdExtensionPlugin::new().visit(&mut t);
        assert_eq!(href(&t), "foo?x=1");
    }

    #[test]
    fn leaves_external_md_alone() {
        let mut t = root_anchor("https://example.com/foo.md");
        StripMdExtensionPlugin::new().visit(&mut t);
        assert_eq!(href(&t), "https://example.com/foo.md");
    }

    #[test]
    fn leaves_non_md_alone() {
        let mut t = root_anchor("/docs/intro/");
        StripMdExtensionPlugin::new().visit(&mut t);
        assert_eq!(href(&t), "/docs/intro/");
    }
}
