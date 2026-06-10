//! Internal link and anchor validation (hast phase, runs after `HeadingLinksPlugin`).
//!
//! When enabled, walks every `<a href>` and `<img src>` in the build and
//! validates that:
//!
//! - **Bare anchor fragments** (`#section`) match a heading ID in the current
//!   file's heading-ID registry entry.
//! - **File-relative links with anchor** (`./other.md#section`) resolve to an
//!   existing source file whose registry entry contains that heading ID.
//! - **File-relative links without anchor** (`./other.md`) resolve to an
//!   existing file on disk (under `project_root`).
//! - **External URLs** (`http://`, `https://`, `mailto:`, etc.) are always
//!   skipped silently — network validation is out of scope.
//!
//! Broken links are emitted as [`MarkdownDiagnostic::BrokenLink`] through
//! `ctx.diagnostics`. Severity is `Warning` (default) or `Error` when
//! `failOnBroken: true` is set in the config — the orchestrator decides
//! whether `Error`-level diagnostics abort the build.
//!
//! # Phase
//!
//! Runs in the **hast phase**, very late — registered in `register_features`
//! after all heading-mutating visitors (heading marker TOC, toc export, etc.)
//! so `HeadingLinksPlugin` has already populated `ctx.heading_registry` with
//! the current file's entries.
//!
//! # Context dependency
//!
//! This plugin only validates when called via
//! `Pipeline::run_with_context` with a `BuildContext` that has both
//! `source_path` and `heading_registry` populated. When the context is absent
//! (bare `visit` call), the plugin is a no-op — matching the backwards-compat
//! contract of all wave-6 context-aware visitors.
//!
//! Wire via `markdown.features.linkValidation` in `zfb.config.ts`:
//!
//! ```json
//! { "markdown": { "features": { "linkValidation": {} } } }
//! { "markdown": { "features": { "linkValidation": { "failOnBroken": true } } } }
//! ```
//!
//! Ported in Wave 6 (#580). Reference: [`remark-validate-links`](https://www.npmjs.com/package/remark-validate-links).

use std::path::{Path, PathBuf};

use zfb_md_ast::{
    diagnostics::{DiagnosticSeverity, MarkdownDiagnostic, SourceLocation},
    BuildContext, HastNode, HastVisitor, LinkValidationConfig,
};
use zfb_types::normalize_path_lexical;

// ── External URL detection ────────────────────────────────────────────────────

/// Returns true if the href should be skipped as an external URL.
///
/// Matches `http://`, `https://`, `mailto:`, `ftp://`, `data:`, `javascript:`,
/// and any other scheme-prefixed URL. Fragment-only links (`#anchor`) and
/// relative file paths are NOT external.
fn is_external_url(href: &str) -> bool {
    // Scheme detection: contains "://" (http, https, ftp, …) or starts with a
    // known schemeless prefix (mailto:, tel:, data:, javascript:). Checking for
    // "://" covers the vast majority of external URLs; the explicit prefixes
    // cover the common non-"//" schemes used in markdown documents.
    href.contains("://")
        || href.starts_with("mailto:")
        || href.starts_with("tel:")
        || href.starts_with("data:")
        || href.starts_with("javascript:")
}

// ── Link parsing ──────────────────────────────────────────────────────────────

/// Parsed representation of an href or src attribute.
#[derive(Debug, PartialEq, Eq)]
enum ParsedLink {
    /// External URL — skip validation.
    External,
    /// Bare anchor fragment (e.g. `#intro`). Path part is empty.
    BareFragment(String),
    /// File path without an anchor (e.g. `./other.md`).
    FilePath(String),
    /// File path with an anchor (e.g. `./other.md#intro`).
    FileWithFragment { path: String, fragment: String },
}

fn parse_link(href: &str) -> ParsedLink {
    if is_external_url(href) {
        return ParsedLink::External;
    }
    if let Some(fragment) = href.strip_prefix('#') {
        return ParsedLink::BareFragment(fragment.to_string());
    }
    // Split on `#` for file-relative links.
    if let Some(pos) = href.find('#') {
        let (path, rest) = href.split_at(pos);
        let fragment = &rest[1..]; // strip the `#`
        if path.is_empty() {
            // Shouldn't happen here (bare `#` already handled above), but
            // defensively treat empty path as bare fragment.
            return ParsedLink::BareFragment(fragment.to_string());
        }
        return ParsedLink::FileWithFragment {
            path: path.to_string(),
            fragment: fragment.to_string(),
        };
    }
    ParsedLink::FilePath(href.to_string())
}

// ── Path resolution ───────────────────────────────────────────────────────────

/// Resolve a relative file path reference against the source file's directory,
/// returning a normalized (`.`/`..`-collapsed) path.
///
/// Returns `None` if `source_path` has no parent (shouldn't happen in
/// practice). Uses [`normalize_path`] to collapse `..` and `.` components
/// without requiring the path to exist on disk — so the project-root boundary
/// check works before the filesystem-existence check.
fn resolve_relative(source_path: &Path, file_ref: &str) -> Option<PathBuf> {
    let dir = source_path.parent()?;
    let joined = dir.join(file_ref);
    Some(normalize_path_lexical(&joined))
}

// ── Visitor ───────────────────────────────────────────────────────────────────

/// Hast visitor that validates internal links and anchor fragments.
///
/// Operates only when called via `visit_with_context` with a `BuildContext`
/// that provides both `source_path` and `heading_registry`. The bare `visit`
/// method is a no-op (backwards-compat contract for wave-6 context-aware
/// visitors).
pub struct LinkValidationPlugin {
    config: LinkValidationConfig,
}

impl LinkValidationPlugin {
    /// Create a new plugin with the given configuration.
    #[must_use]
    pub fn new(config: LinkValidationConfig) -> Self {
        Self { config }
    }

    /// Determine the diagnostic severity based on the `failOnBroken` flag.
    fn severity(&self) -> DiagnosticSeverity {
        if self.config.fail_on_broken.unwrap_or(false) {
            DiagnosticSeverity::Error
        } else {
            DiagnosticSeverity::Warning
        }
    }
}

impl HastVisitor for LinkValidationPlugin {
    /// No-op — context is required for validation.
    fn visit(&mut self, _node: &mut HastNode) {}

    /// Context-aware traversal: validates every `<a href>` and `<img src>`
    /// in the hast tree against the heading registry and filesystem.
    fn visit_with_context(&mut self, node: &mut HastNode, ctx: &mut BuildContext<'_>) {
        let source_path = match ctx.source_path.clone() {
            Some(p) => p,
            None => return, // no source path → cannot resolve relative links
        };
        // Walk the tree collecting (href, severity) diagnostics.
        let severity = self.severity();
        let project_root = ctx.project_root.clone();
        collect_diagnostics(
            node,
            &source_path,
            &project_root,
            ctx,
            severity,
        );
    }
}

// ── Tree walk ─────────────────────────────────────────────────────────────────

/// Recursively walk the hast tree and emit diagnostics for broken links.
fn collect_diagnostics(
    node: &HastNode,
    source_path: &Path,
    project_root: &Path,
    ctx: &mut BuildContext<'_>,
    severity: DiagnosticSeverity,
) {
    match node {
        HastNode::Element {
            tag,
            attrs,
            children,
            ..
        } => {
            // Extract the link target: href for <a>, src for <img>.
            // Track whether the element is an <img> so fragment validation
            // can be skipped — images use fragment syntax for SVG sprites
            // (e.g. sprite.svg#icon-x) which are not heading anchors.
            let (href_opt, is_img) = if tag == "a" {
                (
                    attrs
                        .iter()
                        .find(|(k, _)| k == "href")
                        .map(|(_, v)| v.clone()),
                    false,
                )
            } else if tag == "img" {
                (
                    attrs
                        .iter()
                        .find(|(k, _)| k == "src")
                        .map(|(_, v)| v.clone()),
                    true,
                )
            } else {
                (None, false)
            };

            if let Some(href) = href_opt {
                validate_link(
                    &href,
                    source_path,
                    project_root,
                    ctx,
                    severity,
                    is_img,
                );
            }

            // Recurse into children.
            for child in children {
                collect_diagnostics(
                    child,
                    source_path,
                    project_root,
                    ctx,
                    severity,
                );
            }
        }
        HastNode::Root { children } => {
            for child in children {
                collect_diagnostics(
                    child,
                    source_path,
                    project_root,
                    ctx,
                    severity,
                );
            }
        }
        // Leaf nodes.
        HastNode::Text(_) | HastNode::Raw(_) | HastNode::JsxRaw(_) | HastNode::Comment(_) => {}
    }
}

/// Validate a single link `href` and emit a diagnostic if broken.
///
/// `is_img` must be `true` when the link comes from an `<img src>` attribute.
/// Images support fragment syntax for SVG sprites (e.g. `sprite.svg#icon-x`),
/// so `FileWithFragment` and `BareFragment` skip heading-anchor validation and
/// only confirm file existence (or skip bare fragments entirely).
fn validate_link(
    href: &str,
    source_path: &Path,
    project_root: &Path,
    ctx: &mut BuildContext<'_>,
    severity: DiagnosticSeverity,
    is_img: bool,
) {
    let parsed = parse_link(href);
    match parsed {
        ParsedLink::External => {
            // External URLs are always skipped — network validation is out of scope.
        }
        ParsedLink::BareFragment(fragment) => {
            if is_img {
                // Bare fragment on an <img> (e.g. `#icon`) — no file to check,
                // no heading registry applies; skip silently.
                let _ = fragment;
            } else {
                // Check fragment against the current file's heading entries.
                validate_fragment_in_file(href, &fragment, source_path, ctx, severity);
            }
        }
        ParsedLink::FilePath(path) => {
            // Check that the file exists on disk relative to source_path,
            // and that the resolved path stays within project_root.
            validate_file_exists(href, &path, source_path, project_root, ctx, severity);
        }
        ParsedLink::FileWithFragment { path, fragment } => {
            if is_img {
                // For <img>, fragments are SVG sprite IDs — not heading anchors.
                // Only validate file existence; ignore the fragment part.
                validate_file_exists(href, &path, source_path, project_root, ctx, severity);
            } else {
                // Resolve the target file, then check the fragment in that file.
                validate_file_with_fragment(
                    href,
                    &path,
                    &fragment,
                    source_path,
                    project_root,
                    ctx,
                    severity,
                );
            }
        }
    }
}

/// Validate a bare `#fragment` against the current file's heading registry.
fn validate_fragment_in_file(
    raw_href: &str,
    fragment: &str,
    source_path: &Path,
    ctx: &mut BuildContext<'_>,
    severity: DiagnosticSeverity,
) {
    let registry = match ctx.heading_registry.as_ref() {
        Some(r) => r,
        None => return, // no registry → skip validation
    };
    let known = registry
        .get(source_path)
        .map(|entries| entries.iter().any(|e| e.id == fragment))
        .unwrap_or(false);

    if !known {
        emit_broken_link(raw_href, source_path, ctx, severity);
    }
}

/// Validate that a file-only reference (`./other.md`) exists on disk and
/// does not escape the project root via path traversal (`../outside.md`).
fn validate_file_exists(
    raw_href: &str,
    file_ref: &str,
    source_path: &Path,
    project_root: &Path,
    ctx: &mut BuildContext<'_>,
    severity: DiagnosticSeverity,
) {
    let resolved = match resolve_relative(source_path, file_ref) {
        Some(p) => p,
        None => return,
    };
    // Reject path traversal that escapes the project root. `resolved` is
    // already lexically normalized by `resolve_relative`; any remaining `..`
    // components would be past the root and `starts_with` reliably rejects them.
    if !resolved.starts_with(project_root) {
        emit_broken_link(raw_href, source_path, ctx, severity);
        return;
    }
    // Check filesystem existence.
    if !resolved.exists() {
        emit_broken_link(raw_href, source_path, ctx, severity);
    }
}

/// Validate a `./other.md#fragment` link: file must exist within the project
/// root and the fragment must be in the target file's heading registry.
fn validate_file_with_fragment(
    raw_href: &str,
    file_ref: &str,
    fragment: &str,
    source_path: &Path,
    project_root: &Path,
    ctx: &mut BuildContext<'_>,
    severity: DiagnosticSeverity,
) {
    let resolved = match resolve_relative(source_path, file_ref) {
        Some(p) => p,
        None => return,
    };

    // Reject path traversal that escapes the project root.
    if !resolved.starts_with(project_root) {
        emit_broken_link(raw_href, source_path, ctx, severity);
        return;
    }

    // Check filesystem existence first.
    if !resolved.exists() {
        emit_broken_link(raw_href, source_path, ctx, severity);
        return;
    }

    // Check heading fragment in the target file's registry.
    let known = ctx
        .heading_registry
        .as_ref()
        .and_then(|r| r.get(&resolved))
        .map(|entries| entries.iter().any(|e| e.id == fragment))
        .unwrap_or(false);

    if !known {
        emit_broken_link(raw_href, source_path, ctx, severity);
    }
}

/// Emit a `BrokenLink` diagnostic through `ctx.diagnostics`.
fn emit_broken_link(
    url: &str,
    source_path: &Path,
    ctx: &mut BuildContext<'_>,
    severity: DiagnosticSeverity,
) {
    if let Some(sink) = ctx.diagnostics.as_deref_mut() {
        sink.emit(MarkdownDiagnostic::BrokenLink {
            severity,
            url: url.to_string(),
            location: Some(SourceLocation::from_path(source_path.to_path_buf())),
        });
    }
}

// ── Unit tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_urls_are_detected() {
        assert!(is_external_url("https://example.com"));
        assert!(is_external_url("http://example.com/path"));
        assert!(is_external_url("mailto:user@example.com"));
        assert!(is_external_url("ftp://files.example.com"));
        assert!(is_external_url("tel:+1234567890"));
        assert!(is_external_url("data:image/png;base64,abc"));
        assert!(is_external_url("javascript:void(0)"));
        assert!(!is_external_url("#fragment"));
        assert!(!is_external_url("./other.md"));
        assert!(!is_external_url("other.md#anchor"));
    }

    #[test]
    fn parse_bare_fragment() {
        assert_eq!(
            parse_link("#intro"),
            ParsedLink::BareFragment("intro".to_string())
        );
        assert_eq!(parse_link("#"), ParsedLink::BareFragment(String::new()));
    }

    #[test]
    fn parse_external_link() {
        assert_eq!(parse_link("https://example.com"), ParsedLink::External);
        assert_eq!(parse_link("mailto:a@b.com"), ParsedLink::External);
    }

    #[test]
    fn parse_file_path() {
        assert_eq!(
            parse_link("./other.md"),
            ParsedLink::FilePath("./other.md".to_string())
        );
        assert_eq!(
            parse_link("subdir/page.md"),
            ParsedLink::FilePath("subdir/page.md".to_string())
        );
    }

    #[test]
    fn parse_file_with_fragment() {
        assert_eq!(
            parse_link("./other.md#intro"),
            ParsedLink::FileWithFragment {
                path: "./other.md".to_string(),
                fragment: "intro".to_string(),
            }
        );
    }

    #[test]
    fn resolve_relative_strips_dot_prefix() {
        let source = PathBuf::from("/docs/guide/page.md");
        let resolved = resolve_relative(&source, "./other.md").unwrap();
        assert_eq!(resolved, PathBuf::from("/docs/guide/other.md"));
    }

    #[test]
    fn resolve_relative_normalizes_parent_traversal() {
        let source = PathBuf::from("/project/docs/page.md");
        let resolved = resolve_relative(&source, "../outside.md").unwrap();
        // `..` should collapse: /project/docs/../outside.md → /project/outside.md
        assert_eq!(resolved, PathBuf::from("/project/outside.md"));
    }

    #[test]
    fn normalize_path_collapses_dot_dot() {
        let p = PathBuf::from("/a/b/../c");
        assert_eq!(normalize_path_lexical(&p), PathBuf::from("/a/c"));
    }

    #[test]
    fn normalize_path_collapses_dot() {
        let p = PathBuf::from("/a/./b");
        assert_eq!(normalize_path_lexical(&p), PathBuf::from("/a/b"));
    }

    #[test]
    fn normalize_path_double_parent() {
        let p = PathBuf::from("/a/b/c/../../d");
        assert_eq!(normalize_path_lexical(&p), PathBuf::from("/a/d"));
    }

    // ── Fix #733: img-src fragment should not trigger heading-anchor check ────

    /// An `<img src="sprite.svg#icon-x">` where the file exists must NOT emit a
    /// BrokenLink diagnostic even when `icon-x` is not a heading in any registry.
    #[test]
    fn img_src_with_svg_sprite_fragment_no_broken_link() {
        use tempdir::TempDir;
        use zfb_md_ast::{diagnostics::CollectingSink, HastVisitor};

        let dir = TempDir::new("link_val_img_fragment").unwrap();
        let sprite_path = dir.path().join("sprite.svg");
        // File must exist so validate_file_exists passes.
        std::fs::write(&sprite_path, "<svg/>").unwrap();
        let source_path = dir.path().join("page.mdx");
        std::fs::write(&source_path, "").unwrap();

        // Build a hast tree: <img src="sprite.svg#icon-x">
        let img_node = HastNode::Element {
            tag: "img".to_string(),
            attrs: vec![("src".to_string(), "sprite.svg#icon-x".to_string())],
            children: vec![],
            void: true,
        };
        let mut root = HastNode::Root {
            children: vec![img_node],
        };

        let mut sink = CollectingSink::new();
        let mut ctx = BuildContext {
            source_path: Some(source_path),
            project_root: dir.path().to_path_buf(),
            public_dir: dir.path().to_path_buf(),
            heading_registry: None, // no headings registered → fragment would fail if checked
            diagnostics: Some(&mut sink),
        };
        let mut plugin = LinkValidationPlugin::new(LinkValidationConfig::default());
        plugin.visit_with_context(&mut root, &mut ctx);

        let diags = sink.take();
        assert!(
            diags.is_empty(),
            "img src with svg sprite fragment must not emit BrokenLink: {diags:?}"
        );
    }
}
