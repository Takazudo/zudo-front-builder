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
//!
//! ## Trailing-slash mode
//!
//! When constructed with [`StripMdExtensionPlugin::with_trailing_slash`]
//! (or via [`Pipeline::with_defaults`] / the `add_trailing_slash`
//! pipeline option, which defaults to `true`), the plugin also appends
//! a `/` to extensionless internal relative-path hrefs that don't
//! already end in `/`. This converges the URL shape with
//! [`crate::plugins::ResolveLinksPlugin`], whose mapped output always
//! ends in `/`, so authors writing `[x](./guide.md)` get the same
//! `./guide/` URL whether the link is mapped or not.

use crate::pipeline::{HastNode, HastVisitor};
use crate::plugins::util::url::is_external;

/// Visitor that strips `.md` / `.mdx` from internal anchor hrefs.
#[derive(Debug, Default, Clone, Copy)]
pub struct StripMdExtensionPlugin {
    add_trailing_slash: bool,
}

impl StripMdExtensionPlugin {
    /// New plugin with the legacy "strip only" behaviour
    /// (`add_trailing_slash = false`).
    #[must_use]
    pub fn new() -> Self {
        Self {
            add_trailing_slash: false,
        }
    }

    /// New plugin with the JS-aligned behaviour
    /// (`add_trailing_slash = true`).
    ///
    /// After stripping `.md`/`.mdx`, this mode appends `/` to the path
    /// part of internal relative hrefs that have no other file
    /// extension and don't already end in `/`. The query/fragment
    /// suffix (if any) is preserved AFTER the slash:
    /// `./foo.md#sec` → `./foo/#sec`.
    #[must_use]
    pub fn with_trailing_slash() -> Self {
        Self {
            add_trailing_slash: true,
        }
    }
}

impl HastVisitor for StripMdExtensionPlugin {
    fn visit(&mut self, node: &mut HastNode) {
        if let HastNode::Element { tag, attrs, .. } = node {
            if tag == "a" {
                for (k, v) in attrs.iter_mut() {
                    if k == "href" {
                        if let Some(rewritten) = rewrite_href(v, self.add_trailing_slash) {
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

/// Rewrite a single href, returning `Some` only if the value changes.
fn rewrite_href(url: &str, add_trailing_slash: bool) -> Option<String> {
    if is_external(url) {
        return None;
    }
    // Anchor-only links (`#frag`) are not internal paths in any
    // meaningful sense — leave them alone.
    if url.starts_with('#') {
        return None;
    }

    let (path, suffix) = split_suffix(url);

    // Step 1: strip `.mdx` / `.md` from the path part if present.
    let stripped_path: Option<&str> = path
        .strip_suffix(".mdx")
        .or_else(|| path.strip_suffix(".md"));

    if let Some(stripped) = stripped_path {
        let mut new_path = stripped.to_string();
        if add_trailing_slash && !new_path.ends_with('/') {
            new_path.push('/');
        }
        return Some(format!("{new_path}{suffix}"));
    }

    // Step 2 (trailing-slash mode only): if no `.md`/`.mdx` was
    // stripped, still append `/` to extensionless relative hrefs that
    // don't already end in `/`. This handles links the author already
    // wrote without the extension (e.g. `./guide`), which
    // `ResolveLinksPlugin` would not see (it only matches `.md`/`.mdx`
    // targets). Mirrors the "Astro already stripped .md" branch in the
    // JS plugin so the URL shape matches.
    if !add_trailing_slash {
        return None;
    }
    if !(url.starts_with("./") || url.starts_with("../")) {
        return None;
    }
    if path.ends_with('/') {
        return None;
    }
    let last_segment = path.rsplit('/').next().unwrap_or("");
    if has_file_extension(last_segment) {
        return None;
    }
    Some(format!("{path}/{suffix}"))
}

/// Detect a "file extension" on the last path segment (e.g. `.png`,
/// `.pdf`). Mirrors the JS plugin's `/\.[a-zA-Z]\w*$/` heuristic.
fn has_file_extension(segment: &str) -> bool {
    let Some(dot) = segment.rfind('.') else {
        return false;
    };
    let ext = &segment[dot + 1..];
    if ext.is_empty() {
        return false;
    }
    let mut chars = ext.chars();
    let first = chars.next().expect("ext is non-empty above");
    if !first.is_ascii_alphabetic() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
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

    fn run(plugin: &mut StripMdExtensionPlugin, href_in: &str) -> String {
        let mut t = root_anchor(href_in);
        plugin.visit(&mut t);
        href(&t)
    }

    // ---------- legacy mode (no trailing slash) ----------

    #[test]
    fn legacy_strips_md_from_internal_link() {
        let mut p = StripMdExtensionPlugin::new();
        assert_eq!(run(&mut p, "./foo.md"), "./foo");
    }

    #[test]
    fn legacy_strips_mdx_from_internal_link() {
        let mut p = StripMdExtensionPlugin::new();
        assert_eq!(run(&mut p, "nested/bar.mdx"), "nested/bar");
    }

    #[test]
    fn legacy_preserves_query_and_fragment() {
        let mut p = StripMdExtensionPlugin::new();
        assert_eq!(run(&mut p, "foo.md#sec"), "foo#sec");
        assert_eq!(run(&mut p, "foo.mdx?x=1"), "foo?x=1");
    }

    #[test]
    fn legacy_leaves_external_md_alone() {
        let mut p = StripMdExtensionPlugin::new();
        assert_eq!(
            run(&mut p, "https://example.com/foo.md"),
            "https://example.com/foo.md",
        );
    }

    #[test]
    fn legacy_leaves_non_md_alone() {
        let mut p = StripMdExtensionPlugin::new();
        assert_eq!(run(&mut p, "/docs/intro/"), "/docs/intro/");
    }

    // ---------- trailing-slash mode (JS-aligned) ----------

    #[test]
    fn trailing_slash_appends_after_md_strip() {
        let mut p = StripMdExtensionPlugin::with_trailing_slash();
        assert_eq!(run(&mut p, "./other-doc.md"), "./other-doc/");
        assert_eq!(run(&mut p, "./other-doc.mdx"), "./other-doc/");
        assert_eq!(run(&mut p, "../guides/setup.md"), "../guides/setup/");
    }

    #[test]
    fn trailing_slash_preserves_fragment_after_slash() {
        let mut p = StripMdExtensionPlugin::with_trailing_slash();
        assert_eq!(run(&mut p, "./other-doc.md#sec"), "./other-doc/#sec");
    }

    #[test]
    fn trailing_slash_added_to_extensionless_relative() {
        // The JS plugin also appends `/` to relative hrefs that already
        // lost their `.md` extension upstream.
        let mut p = StripMdExtensionPlugin::with_trailing_slash();
        assert_eq!(run(&mut p, "./other-doc"), "./other-doc/");
        assert_eq!(run(&mut p, "../guides/setup"), "../guides/setup/");
    }

    #[test]
    fn trailing_slash_preserves_extensionless_query_and_fragment() {
        let mut p = StripMdExtensionPlugin::with_trailing_slash();
        assert_eq!(run(&mut p, "./other-doc?tab=api"), "./other-doc/?tab=api");
        assert_eq!(
            run(&mut p, "./other-doc?tab=api#section"),
            "./other-doc/?tab=api#section",
        );
        assert_eq!(run(&mut p, "./other-doc#section"), "./other-doc/#section");
    }

    #[test]
    fn trailing_slash_preserves_file_extensions() {
        let mut p = StripMdExtensionPlugin::with_trailing_slash();
        // `.png` is a real file extension — leave the href alone even
        // in trailing-slash mode.
        assert_eq!(run(&mut p, "./image.png"), "./image.png");
    }

    #[test]
    fn trailing_slash_skips_already_trailing_slash() {
        let mut p = StripMdExtensionPlugin::with_trailing_slash();
        assert_eq!(run(&mut p, "./dir/"), "./dir/");
    }

    #[test]
    fn trailing_slash_skips_non_relative_paths() {
        // Absolute paths (`/foo`) are treated as already canonical —
        // the plugin only reshapes `./` and `../` style relative
        // references in this mode (matching the JS plugin).
        let mut p = StripMdExtensionPlugin::with_trailing_slash();
        assert_eq!(run(&mut p, "/absolute/path"), "/absolute/path");
    }

    #[test]
    fn trailing_slash_skips_external_and_anchor() {
        let mut p = StripMdExtensionPlugin::with_trailing_slash();
        assert_eq!(run(&mut p, "https://example.com"), "https://example.com");
        assert_eq!(run(&mut p, "#section"), "#section");
        assert_eq!(
            run(&mut p, "mailto:test@example.com"),
            "mailto:test@example.com",
        );
    }

    #[test]
    fn trailing_slash_md_with_query_unchanged_by_strip() {
        // `.md?tab=api` does not match the strip path (the suffix
        // splits on `?` so the path part is `./doc.md` — but the JS
        // regex `/\.mdx?(#.*)?$/` only allows an optional `#` after the
        // extension, not `?`. Mirror that behaviour: the path WITH a
        // query string is left alone by the strip step. Then the
        // already-extensionless branch sees `.md` as a file extension
        // and skips, so the URL is unchanged.
        let mut p = StripMdExtensionPlugin::with_trailing_slash();
        // Our split_suffix treats both `?` and `#` as suffix starts —
        // that means we DO see `./doc.md` here and strip it. This is a
        // pragmatic divergence from the JS regex shape; it matters
        // only for the rare `.md?query` author-written link, and the
        // trailing-slash mode produces a more consistent URL.
        assert_eq!(run(&mut p, "./doc.md?tab=api"), "./doc/?tab=api");
    }
}
