//! Rewrite `.md` / `.mdx` link targets to their final site URLs.
//!
//! Rust port of zudo-doc's `remarkResolveMarkdownLinks`. This is the
//! second pass of a two-pass design: callers first build a
//! `path → URL` map via [`crate::plugins::util::source_map::build_docs_source_map`],
//! then construct this plugin with that map. As mdast is walked, every
//! [`Link`] node whose `url` ends in `.md` or `.mdx` is matched
//! against the map (after best-effort path resolution) and rewritten
//! to the mapped URL.
//!
//! [`Link`]: markdown::mdast::Link

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use markdown::mdast::Node as MdastNode;

use crate::pipeline::MdastVisitor;

/// Options for [`ResolveLinksPlugin`].
#[derive(Debug, Clone, Default)]
pub struct ResolveMarkdownLinksOptions {
    /// `absolute_path → site_url` map — typically built by
    /// [`crate::plugins::util::source_map::build_docs_source_map`].
    pub source_map: HashMap<PathBuf, String>,
    /// The directory of the source file currently being rendered.
    /// Used to resolve relative `./foo.md` link targets against the
    /// `source_map`. If `None`, only absolute paths are resolved.
    pub source_dir: Option<PathBuf>,
}

/// Visitor that rewrites `.md` / `.mdx` link URLs.
#[derive(Debug, Clone)]
pub struct ResolveLinksPlugin {
    options: ResolveMarkdownLinksOptions,
}

impl ResolveLinksPlugin {
    /// Construct with the prebuilt `source_map`.
    #[must_use]
    pub fn new(options: ResolveMarkdownLinksOptions) -> Self {
        Self { options }
    }

    fn resolve(&self, url: &str) -> Option<String> {
        // Split off optional ?query and #fragment so we only resolve
        // the path part and stitch them back on.
        let (path_part, suffix) = split_suffix(url);
        if !ends_with_md(path_part) {
            return None;
        }

        // Try direct lookup first (caller may have stored relative
        // paths in the map).
        let direct = PathBuf::from(path_part);
        if let Some(target) = self.options.source_map.get(&direct) {
            return Some(format!("{target}{suffix}"));
        }

        // Try resolving against source_dir.
        if let Some(dir) = &self.options.source_dir {
            let joined = normalize_join(dir, path_part);
            if let Some(target) = self.options.source_map.get(&joined) {
                return Some(format!("{target}{suffix}"));
            }
        }
        None
    }
}

impl MdastVisitor for ResolveLinksPlugin {
    fn visit(&mut self, node: &mut MdastNode) {
        if let MdastNode::Link(l) = node {
            if let Some(rewritten) = self.resolve(&l.url) {
                l.url = rewritten;
            }
        }
        if let Some(children) = node.children_mut() {
            for c in children {
                self.visit(c);
            }
        }
    }
}

fn ends_with_md(path: &str) -> bool {
    path.ends_with(".md") || path.ends_with(".mdx")
}

fn split_suffix(url: &str) -> (&str, &str) {
    if let Some(i) = url.find(['?', '#']) {
        (&url[..i], &url[i..])
    } else {
        (url, "")
    }
}

/// Normalize `dir.join(rel)` by resolving `./` and `../` segments
/// against `dir`. We use a string-level walk because `Path::canonicalize`
/// requires the file to exist.
fn normalize_join(dir: &Path, rel: &str) -> PathBuf {
    let mut buf = dir.to_path_buf();
    for component in rel.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                buf.pop();
            }
            other => buf.push(other),
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use markdown::mdast::{Link, Paragraph, Root, Text};

    fn link(url: &str) -> MdastNode {
        MdastNode::Link(Link {
            url: url.to_string(),
            title: None,
            children: vec![MdastNode::Text(Text {
                value: "x".into(),
                position: None,
            })],
            position: None,
        })
    }

    fn root_with_link(url: &str) -> MdastNode {
        MdastNode::Root(Root {
            children: vec![MdastNode::Paragraph(Paragraph {
                children: vec![link(url)],
                position: None,
            })],
            position: None,
        })
    }

    fn link_url(node: &MdastNode) -> String {
        let MdastNode::Root(r) = node else { panic!() };
        let MdastNode::Paragraph(p) = &r.children[0] else {
            panic!()
        };
        let MdastNode::Link(l) = &p.children[0] else {
            panic!()
        };
        l.url.clone()
    }

    #[test]
    fn rewrites_md_link_via_direct_map() {
        let mut map = HashMap::new();
        map.insert(PathBuf::from("foo.md"), "/docs/foo/".to_string());
        let mut plugin = ResolveLinksPlugin::new(ResolveMarkdownLinksOptions {
            source_map: map,
            source_dir: None,
        });
        let mut root = root_with_link("foo.md");
        plugin.visit(&mut root);
        assert_eq!(link_url(&root), "/docs/foo/");
    }

    #[test]
    fn rewrites_via_source_dir() {
        let dir = PathBuf::from("/site/docs");
        let mut map = HashMap::new();
        map.insert(
            PathBuf::from("/site/docs/intro.md"),
            "/docs/intro/".to_string(),
        );
        let mut plugin = ResolveLinksPlugin::new(ResolveMarkdownLinksOptions {
            source_map: map,
            source_dir: Some(dir),
        });
        let mut root = root_with_link("./intro.md");
        plugin.visit(&mut root);
        assert_eq!(link_url(&root), "/docs/intro/");
    }

    #[test]
    fn preserves_query_and_fragment() {
        let mut map = HashMap::new();
        map.insert(PathBuf::from("foo.md"), "/docs/foo/".to_string());
        let mut plugin = ResolveLinksPlugin::new(ResolveMarkdownLinksOptions {
            source_map: map,
            source_dir: None,
        });
        let mut root = root_with_link("foo.md#section?x=1");
        plugin.visit(&mut root);
        // Note: '#' comes first in our test url so suffix = "#section?x=1".
        assert_eq!(link_url(&root), "/docs/foo/#section?x=1");
    }

    #[test]
    fn leaves_non_md_alone() {
        let plugin_opts = ResolveMarkdownLinksOptions::default();
        let mut plugin = ResolveLinksPlugin::new(plugin_opts);
        let mut root = root_with_link("https://example.com");
        plugin.visit(&mut root);
        assert_eq!(link_url(&root), "https://example.com");
    }

    #[test]
    fn unmapped_md_left_alone() {
        let plugin_opts = ResolveMarkdownLinksOptions::default();
        let mut plugin = ResolveLinksPlugin::new(plugin_opts);
        let mut root = root_with_link("missing.md");
        plugin.visit(&mut root);
        assert_eq!(link_url(&root), "missing.md");
    }
}
