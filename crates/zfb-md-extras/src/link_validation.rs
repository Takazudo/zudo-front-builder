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
//! - **External URLs** (`http://`, `https://`, `mailto:`, etc.) are skipped
//!   silently when `allowExternal` is `true` (the default).
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
    BuildContext, HastNode, HastVisitor, LinkValidationConfig,
    diagnostics::{DiagnosticSeverity, MarkdownDiagnostic, SourceLocation},
};

// ── External URL detection ────────────────────────────────────────────────────

/// Returns true if the href should be skipped as an external URL.
///
/// Matches `http://`, `https://`, `mailto:`, `ftp://`, and any other
/// scheme-prefixed URL (contains `://` or starts with `mailto:`). Fragment-only
/// links (`#anchor`) and relative file paths are NOT external.
fn is_external_url(href: &str) -> bool {
    // Scheme detection: contains "://" (http, https, ftp, …) or starts with
    // "mailto:", "tel:", "data:", etc. (scheme without "//").
    href.contains("://") || href.starts_with("mailto:") || href.starts_with("tel:")
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

/// Resolve a relative file path reference against the source file's directory.
///
/// Returns `None` if `source_path` has no parent (shouldn't happen in
/// practice), or the resolved `PathBuf` otherwise. The path is not
/// canonicalized — callers should use `exists()` / registry lookup.
fn resolve_relative(source_path: &Path, file_ref: &str) -> Option<PathBuf> {
    let dir = source_path.parent()?;
    let resolved = dir.join(file_ref);
    // Normalize `./` prefixes by using the path as-is; std `PathBuf::join`
    // handles `./` correctly on all platforms.
    Some(resolved)
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

    /// Returns true when external URLs should be silently skipped (default).
    fn allow_external(&self) -> bool {
        self.config.allow_external.unwrap_or(true)
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
        let allow_external = self.allow_external();
        collect_diagnostics(node, &source_path, ctx, severity, allow_external);
    }
}

// ── Tree walk ─────────────────────────────────────────────────────────────────

/// Recursively walk the hast tree and emit diagnostics for broken links.
fn collect_diagnostics(
    node: &HastNode,
    source_path: &Path,
    ctx: &mut BuildContext<'_>,
    severity: DiagnosticSeverity,
    allow_external: bool,
) {
    match node {
        HastNode::Element { tag, attrs, children, .. } => {
            // Extract the link target: href for <a>, src for <img>.
            let href_opt = if tag == "a" {
                attrs.iter().find(|(k, _)| k == "href").map(|(_, v)| v.clone())
            } else if tag == "img" {
                attrs.iter().find(|(k, _)| k == "src").map(|(_, v)| v.clone())
            } else {
                None
            };

            if let Some(href) = href_opt {
                validate_link(
                    &href,
                    source_path,
                    ctx,
                    severity,
                    allow_external,
                );
            }

            // Recurse into children.
            for child in children {
                collect_diagnostics(child, source_path, ctx, severity, allow_external);
            }
        }
        HastNode::Root { children } => {
            for child in children {
                collect_diagnostics(child, source_path, ctx, severity, allow_external);
            }
        }
        // Leaf nodes.
        HastNode::Text(_) | HastNode::Raw(_) | HastNode::JsxRaw(_) | HastNode::Comment(_) => {}
    }
}

/// Validate a single link `href` and emit a diagnostic if broken.
fn validate_link(
    href: &str,
    source_path: &Path,
    ctx: &mut BuildContext<'_>,
    severity: DiagnosticSeverity,
    allow_external: bool,
) {
    let parsed = parse_link(href);
    match parsed {
        ParsedLink::External => {
            // Skip external URLs when `allowExternal` is true (default).
            if allow_external {
                return;
            }
            // When allowExternal is false, we still skip actual validation of
            // external URLs in this implementation — network checks are out of
            // scope. Emit nothing.
        }
        ParsedLink::BareFragment(fragment) => {
            // Check fragment against the current file's heading entries.
            validate_fragment_in_file(href, &fragment, source_path, ctx, severity);
        }
        ParsedLink::FilePath(path) => {
            // Check that the file exists on disk relative to source_path.
            validate_file_exists(href, &path, source_path, ctx, severity);
        }
        ParsedLink::FileWithFragment { path, fragment } => {
            // Resolve the target file, then check the fragment in that file.
            validate_file_with_fragment(href, &path, &fragment, source_path, ctx, severity);
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

/// Validate that a file-only reference (`./other.md`) exists on disk.
fn validate_file_exists(
    raw_href: &str,
    file_ref: &str,
    source_path: &Path,
    ctx: &mut BuildContext<'_>,
    severity: DiagnosticSeverity,
) {
    let resolved = match resolve_relative(source_path, file_ref) {
        Some(p) => p,
        None => return,
    };
    // Check filesystem existence.
    if !resolved.exists() {
        emit_broken_link(raw_href, source_path, ctx, severity);
    }
}

/// Validate a `./other.md#fragment` link: file must exist and the fragment
/// must be in the target file's heading registry.
fn validate_file_with_fragment(
    raw_href: &str,
    file_ref: &str,
    fragment: &str,
    source_path: &Path,
    ctx: &mut BuildContext<'_>,
    severity: DiagnosticSeverity,
) {
    let resolved = match resolve_relative(source_path, file_ref) {
        Some(p) => p,
        None => return,
    };

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
        assert!(!is_external_url("#fragment"));
        assert!(!is_external_url("./other.md"));
        assert!(!is_external_url("other.md#anchor"));
    }

    #[test]
    fn parse_bare_fragment() {
        assert_eq!(parse_link("#intro"), ParsedLink::BareFragment("intro".to_string()));
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
}
