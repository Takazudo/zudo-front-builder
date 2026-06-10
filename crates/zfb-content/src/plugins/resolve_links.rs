//! Rewrite `.md` / `.mdx` link targets to their final site URLs.
//!
//! Rust port of zudo-doc's `remarkResolveMarkdownLinks`. This is the
//! second pass of a two-pass design: callers first build a
//! `path → URL` map via [`crate::plugins::util::source_map::build_docs_source_map`],
//! then construct this plugin with that map. As mdast is walked, every
//! [`Link`] node whose `url` ends in `.md` or `.mdx` **or is
//! extensionless** is matched against the map (after best-effort path
//! resolution) and rewritten to the mapped URL.
//!
//! For extensionless links the following candidates are tried in order
//! (first match wins):
//!
//! 1. `{name}.mdx`
//! 2. `{name}.md`
//! 3. `{name}/index.mdx`
//! 4. `{name}/index.md`
//!
//! Resolved URLs always end with `/` (they come directly from the
//! source map), which is shape-compatible with the trailing-slash mode
//! of [`crate::plugins::StripMdExtensionPlugin`] (Sub 6 convergence).
//!
//! [`Link`]: markdown::mdast::Link

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use markdown::mdast::Node as MdastNode;

use crate::pipeline::MdastVisitor;
use crate::plugins::util::url::is_external;

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

/// A broken `.md`/`.mdx` link that could not be resolved via the source map.
///
/// Produced by [`ResolveLinksPlugin`] when a link target that ends in
/// `.md`/`.mdx` is not found in the source map. Callers drain these via
/// [`ResolveLinksPlugin::take_broken_links`] after each file's pipeline run.
#[derive(Debug, Clone, PartialEq)]
pub struct BrokenLinkDiagnostic {
    /// The original (unresolved) link URL, as written by the author.
    pub url: String,
}

/// Visitor that rewrites `.md` / `.mdx` link URLs.
///
/// After each file's pipeline run, call [`ResolveLinksPlugin::take_broken_links`]
/// to drain any broken-link diagnostics accumulated during the visit. A
/// broken link is any `.md`/`.mdx` href (or extensionless href candidate) that
/// was not found in the source map. Non-md links and successfully-resolved
/// links do not produce diagnostics.
#[derive(Debug, Clone)]
pub struct ResolveLinksPlugin {
    options: ResolveMarkdownLinksOptions,
    /// Broken links accumulated since the last [`take_broken_links`] call.
    broken_links: Vec<BrokenLinkDiagnostic>,
}

impl ResolveLinksPlugin {
    /// Construct with the prebuilt `source_map`.
    #[must_use]
    pub fn new(options: ResolveMarkdownLinksOptions) -> Self {
        Self {
            options,
            broken_links: Vec::new(),
        }
    }

    /// Drain accumulated broken-link diagnostics. Call this after each
    /// file's pipeline run to collect any unresolved `.md`/`.mdx` links.
    /// The internal buffer is cleared; subsequent calls return an empty vec
    /// until the next pipeline run accumulates new diagnostics.
    pub fn take_broken_links(&mut self) -> Vec<BrokenLinkDiagnostic> {
        std::mem::take(&mut self.broken_links)
    }

    /// Update the per-file source directory used to resolve relative link
    /// targets (e.g. `./other.mdx`). Call once per MDX file before the
    /// pipeline's mdast visitors run.
    pub fn set_source_dir(&mut self, dir: PathBuf) {
        self.options.source_dir = Some(dir);
    }

    /// Current per-file source directory (`None` until
    /// [`Self::set_source_dir`] is called). Read by the compile-cache
    /// keying surface (`Pipeline::cache_key_context`, zfb#939).
    pub(crate) fn source_dir(&self) -> Option<&Path> {
        self.options.source_dir.as_deref()
    }

    /// Number of diagnostics currently buffered (i.e. accumulated and
    /// not yet drained via [`Self::take_broken_links`]).
    pub(crate) fn broken_links_len(&self) -> usize {
        self.broken_links.len()
    }

    /// Clone the diagnostics buffered at index `from` onward, without
    /// draining. The compile cache uses this to slice off exactly the
    /// diagnostics ONE compile appended (zfb#939) — the buffer may still
    /// hold earlier files' diagnostics when the caller drains lazily.
    pub(crate) fn broken_links_since(&self, from: usize) -> Vec<BrokenLinkDiagnostic> {
        self.broken_links.get(from..).unwrap_or_default().to_vec()
    }

    /// Cache-hit replay (zfb#939): re-append diagnostics recorded by an
    /// earlier compile of the same `(input, config, source_dir)` so call
    /// sites draining [`Self::take_broken_links`] after a compile-cache
    /// hit observe exactly what a fresh compile would have produced.
    pub(crate) fn replay_broken_links(&mut self, diags: Vec<BrokenLinkDiagnostic>) {
        self.broken_links.extend(diags);
    }

    /// Resolve a link URL against the source map.
    ///
    /// Returns `Ok(Some(rewritten))` when the URL resolved successfully,
    /// `Ok(None)` for URLs that are not md/mdx (nothing to resolve),
    /// and `Err(())` when the URL is an `.md`/`.mdx` href that failed
    /// to resolve (caller should record a [`BrokenLinkDiagnostic`]).
    fn resolve(&self, url: &str) -> Result<Option<String>, ()> {
        // External (scheme-bearing / protocol-relative) hrefs are never local
        // targets — leave them untouched and don't flag a `.md` one as broken.
        // Mirrors the front-guard in StripMdExtensionPlugin.
        if is_external(url) {
            return Ok(None);
        }
        // Split off optional ?query and #fragment so we only resolve
        // the path part and stitch them back on.
        let (path_part, suffix) = split_suffix(url);

        // Bare same-page references (`#fragment`, `?query`) have an empty
        // path component — nothing to resolve. Without this guard the empty
        // path falls through to extensionless probing, where the `/index.mdx`
        // candidate joins to `{source_dir}/index.mdx` and rewrites `#anchor`
        // to `/<parent-dir>/#anchor` (zudolab/zudo-doc#1948).
        if path_part.is_empty() {
            return Ok(None);
        }

        if ends_with_md(path_part) {
            // Explicit .md/.mdx path: try direct and source_dir-relative lookups.
            return match self.lookup_path(path_part, suffix) {
                Some(rewritten) => Ok(Some(rewritten)),
                // Link has a .md/.mdx extension but is not in the map → broken.
                None => Err(()),
            };
        }

        // Extensionless path: skip anything that looks like an external URL
        // or an absolute path with a non-md extension (e.g. ".html", ".png").
        // We only probe when the path has no dot in its last segment (truly
        // extensionless) so we don't accidentally probe "foo.html" as if it
        // were "foo.html.mdx".
        if has_non_md_extension(path_part) {
            return Ok(None);
        }

        // Probe the 4 candidates in precedence order.
        for candidate in extensionless_candidates(path_part) {
            if let Some(result) = self.lookup_path(&candidate, suffix) {
                return Ok(Some(result));
            }
        }
        // No candidate matched — leave extensionless links unchanged (they
        // might be intentional cross-origin or non-doc references).
        Ok(None)
    }

    /// Try the given `path_str` against the source map directly, then
    /// via `source_dir`-relative join. Returns `Some(url + suffix)` on
    /// the first match, `None` if no entry is found.
    fn lookup_path(&self, path_str: &str, suffix: &str) -> Option<String> {
        // Try direct lookup first (caller may have stored relative
        // paths in the map).
        let direct = PathBuf::from(path_str);
        if let Some(target) = self.options.source_map.get(&direct) {
            return Some(format!("{target}{suffix}"));
        }

        // Try resolving against source_dir.
        if let Some(dir) = &self.options.source_dir {
            let joined = normalize_join(dir, path_str);
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
            match self.resolve(&l.url) {
                Ok(Some(rewritten)) => l.url = rewritten,
                Err(()) => {
                    // .md/.mdx link that is not in the source map → broken.
                    self.broken_links
                        .push(BrokenLinkDiagnostic { url: l.url.clone() });
                }
                Ok(None) => {}
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

/// Return `true` when the last path segment contains a `.` — meaning
/// the path already carries a non-md extension and should not be
/// probed as extensionless.
fn has_non_md_extension(path: &str) -> bool {
    let last_segment = path.rsplit('/').next().unwrap_or(path);
    // If the last segment has a dot and the path does NOT end in .md/.mdx
    // (already handled above), it has some other extension.
    last_segment.contains('.')
}

/// Yield the 4 extensionless candidate paths for `name`, in probe order:
///
/// 1. `{name}.mdx`
/// 2. `{name}.md`
/// 3. `{name}/index.mdx`
/// 4. `{name}/index.md`
fn extensionless_candidates(name: &str) -> [String; 4] {
    [
        format!("{name}.mdx"),
        format!("{name}.md"),
        format!("{name}/index.mdx"),
        format!("{name}/index.md"),
    ]
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
        let MdastNode::Root(r) = node else {
            unreachable!("expected MdastNode::Root")
        };
        let MdastNode::Paragraph(p) = &r.children[0] else {
            unreachable!("expected MdastNode::Paragraph")
        };
        let MdastNode::Link(l) = &p.children[0] else {
            unreachable!("expected MdastNode::Link")
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

    // ---------- extensionless candidate probing ----------

    #[test]
    fn extensionless_resolves_name_mdx_candidate() {
        // Candidate 1: {name}.mdx
        let dir = PathBuf::from("/site/docs");
        let mut map = HashMap::new();
        map.insert(
            PathBuf::from("/site/docs/guide.mdx"),
            "/docs/guide/".to_string(),
        );
        let mut plugin = ResolveLinksPlugin::new(ResolveMarkdownLinksOptions {
            source_map: map,
            source_dir: Some(dir),
        });
        let mut root = root_with_link("./guide");
        plugin.visit(&mut root);
        assert_eq!(link_url(&root), "/docs/guide/");
    }

    #[test]
    fn extensionless_resolves_name_md_candidate() {
        // Candidate 2: {name}.md (only .md present, not .mdx)
        let dir = PathBuf::from("/site/docs");
        let mut map = HashMap::new();
        map.insert(
            PathBuf::from("/site/docs/guide.md"),
            "/docs/guide/".to_string(),
        );
        let mut plugin = ResolveLinksPlugin::new(ResolveMarkdownLinksOptions {
            source_map: map,
            source_dir: Some(dir),
        });
        let mut root = root_with_link("./guide");
        plugin.visit(&mut root);
        assert_eq!(link_url(&root), "/docs/guide/");
    }

    #[test]
    fn extensionless_resolves_index_mdx_candidate() {
        // Candidate 3: {name}/index.mdx
        let dir = PathBuf::from("/site/docs");
        let mut map = HashMap::new();
        map.insert(
            PathBuf::from("/site/docs/guide/index.mdx"),
            "/docs/index/".to_string(),
        );
        let mut plugin = ResolveLinksPlugin::new(ResolveMarkdownLinksOptions {
            source_map: map,
            source_dir: Some(dir),
        });
        let mut root = root_with_link("./guide");
        plugin.visit(&mut root);
        assert_eq!(link_url(&root), "/docs/index/");
    }

    #[test]
    fn extensionless_resolves_index_md_candidate() {
        // Candidate 4: {name}/index.md
        let dir = PathBuf::from("/site/docs");
        let mut map = HashMap::new();
        map.insert(
            PathBuf::from("/site/docs/guide/index.md"),
            "/docs/index/".to_string(),
        );
        let mut plugin = ResolveLinksPlugin::new(ResolveMarkdownLinksOptions {
            source_map: map,
            source_dir: Some(dir),
        });
        let mut root = root_with_link("./guide");
        plugin.visit(&mut root);
        assert_eq!(link_url(&root), "/docs/index/");
    }

    #[test]
    fn extensionless_precedence_name_mdx_beats_index_md() {
        // When both candidate 1 ({name}.mdx) and candidate 4 ({name}/index.md)
        // exist, {name}.mdx wins (higher precedence).
        let dir = PathBuf::from("/site/docs");
        let mut map = HashMap::new();
        map.insert(
            PathBuf::from("/site/docs/guide.mdx"),
            "/docs/guide-mdx/".to_string(),
        );
        map.insert(
            PathBuf::from("/site/docs/guide/index.md"),
            "/docs/guide-index-md/".to_string(),
        );
        let mut plugin = ResolveLinksPlugin::new(ResolveMarkdownLinksOptions {
            source_map: map,
            source_dir: Some(dir),
        });
        let mut root = root_with_link("./guide");
        plugin.visit(&mut root);
        assert_eq!(link_url(&root), "/docs/guide-mdx/");
    }

    #[test]
    fn extensionless_all_four_miss_left_alone() {
        // No candidates match — link is left unchanged.
        let dir = PathBuf::from("/site/docs");
        let mut plugin = ResolveLinksPlugin::new(ResolveMarkdownLinksOptions {
            source_map: HashMap::new(),
            source_dir: Some(dir),
        });
        let mut root = root_with_link("./missing");
        plugin.visit(&mut root);
        assert_eq!(link_url(&root), "./missing");
    }

    #[test]
    fn extensionless_preserves_query_and_fragment() {
        // Query + fragment are preserved when resolving an extensionless link.
        let dir = PathBuf::from("/site/docs");
        let mut map = HashMap::new();
        map.insert(
            PathBuf::from("/site/docs/guide.mdx"),
            "/docs/guide/".to_string(),
        );
        let mut plugin = ResolveLinksPlugin::new(ResolveMarkdownLinksOptions {
            source_map: map,
            source_dir: Some(dir),
        });
        let mut root = root_with_link("./guide#section");
        plugin.visit(&mut root);
        assert_eq!(link_url(&root), "/docs/guide/#section");
    }

    #[test]
    fn bare_fragment_left_alone() {
        // Regression test for zudolab/zudo-doc#1948: a same-page `#anchor`
        // link must not resolve against the page directory even when the
        // parent category index is present in the source map.
        let dir = PathBuf::from("/site/docs/guides");
        let mut map = HashMap::new();
        map.insert(
            PathBuf::from("/site/docs/guides/index.mdx"),
            "/docs/guides/".to_string(),
        );
        let mut plugin = ResolveLinksPlugin::new(ResolveMarkdownLinksOptions {
            source_map: map,
            source_dir: Some(dir),
        });
        let mut root = root_with_link("#per-page-visibility");
        plugin.visit(&mut root);
        assert_eq!(link_url(&root), "#per-page-visibility");
        assert!(plugin.take_broken_links().is_empty());
    }

    #[test]
    fn bare_query_left_alone() {
        // Query-only links also have an empty path component — same guard.
        let dir = PathBuf::from("/site/docs/guides");
        let mut map = HashMap::new();
        map.insert(
            PathBuf::from("/site/docs/guides/index.mdx"),
            "/docs/guides/".to_string(),
        );
        let mut plugin = ResolveLinksPlugin::new(ResolveMarkdownLinksOptions {
            source_map: map,
            source_dir: Some(dir),
        });
        let mut root = root_with_link("?x=1");
        plugin.visit(&mut root);
        assert_eq!(link_url(&root), "?x=1");
        assert!(plugin.take_broken_links().is_empty());
    }

    #[test]
    fn non_md_extension_left_alone() {
        // A link like "./image.png" is not extensionless — it must not be probed.
        let dir = PathBuf::from("/site/docs");
        let mut plugin = ResolveLinksPlugin::new(ResolveMarkdownLinksOptions {
            source_map: HashMap::new(),
            source_dir: Some(dir),
        });
        let mut root = root_with_link("./image.png");
        plugin.visit(&mut root);
        assert_eq!(link_url(&root), "./image.png");
    }
}
