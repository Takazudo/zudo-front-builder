//! Internal link and anchor validation (hast phase, runs after `HeadingLinksPlugin`).
//!
//! When enabled, walks every `<a href>` and `<img src>` in the build and
//! validates that:
//!
//! - **Bare anchor fragments** (`#section`) match a heading ID in the current
//!   file's heading-ID registry entry. Skipped silently when the registry has
//!   no entry for the source file (distinguishes "no headings tracked" from
//!   "file not tracked" — entry-presence contract). Empty and percent-encoded
//!   fragments (`#`, `#a%20b`) are also skipped (dummy links / undecodable).
//! - **File-relative links with anchor** (`./other.md#section`) resolve to an
//!   existing source file whose registry entry contains that heading ID.
//!   Degrades to existence-only when the registry has no entry for the target
//!   file (mirrors the bare-anchor contract; kills false positives until a
//!   build-scoped cross-file registry lands in #960).
//! - **File-relative links without anchor** (`./other.md`) resolve to an
//!   existing file on disk (under `project_root`). Query strings
//!   (`./other.md?x=1`) are stripped before the path check.
//! - **URL-space hrefs** — site-absolute paths (`/docs/intro/`),
//!   protocol-relative URLs (`//host/x`), and bare-query/empty hrefs (`?x=1`,
//!   ``) — are skipped silently. These are rewrite products of
//!   `ResolveLinksPlugin` or hand-authored site URLs, never on-disk file
//!   references. Site-absolute `<img src="/img/x.png">` public-dir assets are
//!   also skipped — their validation belongs to `imageDimensions`, not here.
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
//! # Read recording (zfb#944) — filesystem-read audit
//!
//! The plugin touches the filesystem in exactly TWO places, both probing
//! the **directly linked file**: the existence checks in
//! [`validate_file_exists`] and [`validate_file_with_fragment`]. Anchor /
//! heading extraction performs **no filesystem reads** — fragments are
//! validated against the in-memory `ctx.heading_registry` only (whose
//! contents derive from the target files; the full-content hash recorded
//! for each linked file is what invalidates a cached entry when a target
//! file's headings change). When a [`ReadRecorder`] is attached
//! ([`LinkValidationPlugin::with_recorder`]), both probe sites record the
//! linked file's full-content state — `Missing` for absent targets (so a
//! later-created target invalidates), `Error` for unreadable ones
//! (including directory targets, which therefore never cache — see the
//! note in [`validate_file_exists`]).
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
use std::sync::Arc;

use zfb_md_ast::{
    diagnostics::{DiagnosticSeverity, MarkdownDiagnostic, SourceLocation},
    BuildContext, CrossFileLinkCandidate, HastNode, HastVisitor, LinkValidationConfig,
    ReadRecorder,
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
    /// URL-space href — site-absolute path (`/docs/intro/`), protocol-relative
    /// (`//host/x`), or empty-path query-only href. These are rewrite products
    /// (ResolveLinksPlugin) or hand-authored site URLs, never on-disk file
    /// references — skip validation.
    UrlSpace,
    /// Bare anchor fragment (e.g. `#intro`). Path part is empty.
    BareFragment(String),
    /// File path without an anchor (e.g. `./other.md`).
    FilePath(String),
    /// File path with an anchor (e.g. `./other.md#intro`).
    FileWithFragment { path: String, fragment: String },
}

fn parse_link(href: &str) -> ParsedLink {
    // 1. External URL (scheme-prefixed) — skip validation.
    if is_external_url(href) {
        return ParsedLink::External;
    }
    // 2. URL-space: site-absolute paths (`/docs/x/`) and protocol-relative
    //    URLs (`//host/x`) can never be on-disk relative references.
    if href.starts_with('/') {
        return ParsedLink::UrlSpace;
    }
    // 3. Bare fragment — everything after the leading `#` is the fragment.
    if let Some(fragment) = href.strip_prefix('#') {
        return ParsedLink::BareFragment(fragment.to_string());
    }
    // 4. Split on the first `#` to separate path+query from fragment.
    let (path_and_query, fragment_opt) = match href.find('#') {
        Some(pos) => (&href[..pos], Some(&href[pos + 1..])),
        None => (href, None),
    };
    // 5. Strip query from the path part (query is never part of an on-disk path).
    let path = match path_and_query.find('?') {
        Some(pos) => &path_and_query[..pos],
        None => path_and_query,
    };
    // 6. Classify by (path, fragment).
    match (path.is_empty(), fragment_opt) {
        // Empty path with fragment — defensive bare-fragment (mirrors existing behaviour).
        (true, Some(f)) => ParsedLink::BareFragment(f.to_string()),
        // Empty path, no fragment — bare-query href (`?x=1`) or empty href.
        (true, None) => ParsedLink::UrlSpace,
        // Non-empty path with fragment.
        (false, Some(f)) => ParsedLink::FileWithFragment {
            path: path.to_string(),
            fragment: f.to_string(),
        },
        // Non-empty path, no fragment.
        (false, None) => ParsedLink::FilePath(path.to_string()),
    }
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
    /// Optional read-recorder (zfb#944): when present, every linked-file
    /// existence probe — including missing targets — is reported so the
    /// MDX compile cache can validate a dependency manifest. `None`
    /// keeps the pre-#944 behaviour exactly.
    recorder: Option<Arc<ReadRecorder>>,
}

impl LinkValidationPlugin {
    /// Create a new plugin with the given configuration.
    #[must_use]
    pub fn new(config: LinkValidationConfig) -> Self {
        Self {
            config,
            recorder: None,
        }
    }

    /// Attach the read-recorder this plugin reports linked-file probes
    /// through (zfb#944). The SAME `Arc` must also be set on the
    /// pipeline via `Pipeline::set_read_recorder` so the compile-cache
    /// choke point can scope the recording per compile.
    #[must_use]
    pub fn with_recorder(mut self, recorder: Arc<ReadRecorder>) -> Self {
        self.recorder = Some(recorder);
        self
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
        let project_root = ctx.project_root.clone();
        let env = ValidationEnv {
            source_path: &source_path,
            project_root: &project_root,
            severity: self.severity(),
            recorder: self.recorder.as_deref(),
        };
        collect_diagnostics(node, &env, ctx);
    }
}

// ── Tree walk ─────────────────────────────────────────────────────────────────

/// Per-visit immutable validation environment, shared by the whole walk.
struct ValidationEnv<'a> {
    source_path: &'a Path,
    project_root: &'a Path,
    severity: DiagnosticSeverity,
    /// Read-recorder for the compile-cache dependency manifest
    /// (zfb#944); `None` when the plugin was built without one.
    recorder: Option<&'a ReadRecorder>,
}

/// Recursively walk the hast tree and emit diagnostics for broken links.
fn collect_diagnostics(node: &HastNode, env: &ValidationEnv<'_>, ctx: &mut BuildContext<'_>) {
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
                validate_link(&href, env, ctx, is_img);
            }

            // Recurse into children.
            for child in children {
                collect_diagnostics(child, env, ctx);
            }
        }
        HastNode::Root { children } => {
            for child in children {
                collect_diagnostics(child, env, ctx);
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
fn validate_link(href: &str, env: &ValidationEnv<'_>, ctx: &mut BuildContext<'_>, is_img: bool) {
    let parsed = parse_link(href);
    match parsed {
        ParsedLink::External => {
            // External URLs are always skipped — network validation is out of scope.
        }
        ParsedLink::UrlSpace => {
            // Site-absolute and protocol-relative hrefs are rewrite products or
            // hand-authored site URLs — never on-disk references. Site-absolute
            // img srcs (`/img/x.png`) resolve against `public_dir` in URL space;
            // their validation is imageDimensions' job, not linkValidation's.
        }
        ParsedLink::BareFragment(fragment) => {
            if is_img {
                // Bare fragment on an <img> (e.g. `#icon`) — no file to check,
                // no heading registry applies; skip silently.
                let _ = fragment;
            } else {
                // Check fragment against the current file's heading entries.
                validate_fragment_in_file(href, &fragment, env, ctx);
            }
        }
        ParsedLink::FilePath(path) => {
            // Check that the file exists on disk relative to source_path,
            // and that the resolved path stays within project_root.
            validate_file_exists(href, &path, env, ctx);
        }
        ParsedLink::FileWithFragment { path, fragment } => {
            if is_img {
                // For <img>, fragments are SVG sprite IDs — not heading anchors.
                // Only validate file existence; ignore the fragment part.
                validate_file_exists(href, &path, env, ctx);
            } else {
                // Resolve the target file, then check the fragment in that file.
                validate_file_with_fragment(href, &path, &fragment, env, ctx);
            }
        }
    }
}

/// Validate a bare `#fragment` against the current file's heading registry
/// and explicit-anchor-id store.
///
/// No filesystem read happens here (zfb#944 audit): the fragment is
/// checked against the in-memory registry, whose entries for the CURRENT
/// file derive from the compile input itself — already covered by the
/// compile cache's input hash — so there is nothing to record.
///
/// A `BrokenLink` is emitted only when BOTH the heading-id lookup AND the
/// explicit-anchor-id lookup miss, so legitimate non-heading targets
/// (e.g. `<div id="foo">`, `<a name="foo">`) recorded by
/// `HeadingLinksPlugin` do not produce false positives (#1095).
fn validate_fragment_in_file(
    raw_href: &str,
    fragment: &str,
    env: &ValidationEnv<'_>,
    ctx: &mut BuildContext<'_>,
) {
    if fragment.is_empty() || fragment.contains('%') {
        return; // `href="#"` dummy links; percent-encoded fragments (registry stores raw text)
    }
    let entries = match ctx
        .heading_registry
        .as_ref()
        .and_then(|r| r.get(env.source_path))
    {
        Some(e) => e,
        None => return, // no registry entry for this file → cannot distinguish
                        // "file has no headings" from "file not tracked" → skip
    };
    // Accept the fragment if it matches any heading id OR any explicit anchor id.
    if entries.iter().any(|e| e.id == fragment) {
        return;
    }
    if ctx
        .heading_registry
        .as_ref()
        .is_some_and(|r| r.has_anchor(env.source_path, fragment))
    {
        return;
    }
    emit_broken_link(raw_href, env, ctx);
}

/// Validate that a file-only reference (`./other.md`) exists on disk and
/// does not escape the project root via path traversal (`../outside.md`).
///
/// Read recording (zfb#944): the linked file's full-content state is
/// recorded before the existence check — `Content` keeps the cached
/// entry valid until the target changes, `Missing` invalidates it the
/// moment the target appears. A directory target records `Error`
/// (directories cannot be content-hashed), which makes the source file
/// permanently uncacheable — conservative and correct, and rare enough
/// for markdown link targets not to special-case.
fn validate_file_exists(
    raw_href: &str,
    file_ref: &str,
    env: &ValidationEnv<'_>,
    ctx: &mut BuildContext<'_>,
) {
    // A percent-encoded path (`./my%20file.md`) probed verbatim against the
    // filesystem false-positives as broken — the on-disk name is decoded
    // (`my file.md`), not the raw href. Skip rather than decode, mirroring
    // the percent-encoded-fragment rule used elsewhere in this module
    // (`validate_fragment_in_file`, `validate_file_with_fragment`) rather
    // than adding a decode dependency for this one probe site (#1392).
    if file_ref.contains('%') {
        return;
    }
    let resolved = match resolve_relative(env.source_path, file_ref) {
        Some(p) => p,
        None => return,
    };
    // Reject path traversal that escapes the project root. `resolved` is
    // already lexically normalized by `resolve_relative`; any remaining `..`
    // components would be past the root and `starts_with` reliably rejects them.
    // No filesystem access has happened for escaping refs — nothing to record.
    if !resolved.starts_with(env.project_root) {
        emit_broken_link(raw_href, env, ctx);
        return;
    }
    if let Some(r) = env.recorder {
        let _ = r.record_file(&resolved);
    }
    // Check filesystem existence.
    if !resolved.exists() {
        emit_broken_link(raw_href, env, ctx);
    }
}

/// Validate a `./other.md#fragment` link: file must exist within the project
/// root and the fragment must be in the target file's heading registry.
///
/// Read recording (zfb#944): same contract as [`validate_file_exists`].
/// The fragment check itself reads no files — it consults the in-memory
/// registry — but the recorded full-content hash of the TARGET file is
/// exactly what invalidates a cached entry when the target's headings
/// (and therefore the registry-derived verdict) change.
fn validate_file_with_fragment(
    raw_href: &str,
    file_ref: &str,
    fragment: &str,
    env: &ValidationEnv<'_>,
    ctx: &mut BuildContext<'_>,
) {
    // Same false-positive as `validate_file_exists`: a percent-encoded path
    // (`./my%20file.md#anchor`) probed verbatim never matches the decoded
    // on-disk name. Skip rather than decode, mirroring the percent-encoded
    // rule already applied to the fragment part below (#1392).
    if file_ref.contains('%') {
        return;
    }
    let resolved = match resolve_relative(env.source_path, file_ref) {
        Some(p) => p,
        None => return,
    };

    // Reject path traversal that escapes the project root.
    if !resolved.starts_with(env.project_root) {
        emit_broken_link(raw_href, env, ctx);
        return;
    }

    if let Some(r) = env.recorder {
        let _ = r.record_file(&resolved);
    }

    // Check filesystem existence first.
    if !resolved.exists() {
        emit_broken_link(raw_href, env, ctx);
        return;
    }

    if fragment.is_empty() || fragment.contains('%') {
        return; // existence already validated above
    }
    let entries = match ctx.heading_registry.as_ref().and_then(|r| r.get(&resolved)) {
        Some(e) => e,
        None => {
            // No entry for the target file → existence-only degrade
            // (mirrors the bare-anchor contract; kills the
            // `./other.md#frag` unwrap_or(false) false positive).
            //
            // Note: a target compiled in THIS build is always `Some(..)` here
            // (HeadingLinksPlugin calls `mark_tracked` for every file it
            // walks, yielding `Some(&[])` even when headingless), so explicit
            // anchor ids ARE consulted below for same-build targets. Only a
            // not-yet-compiled target falls here, and its explicit anchor ids
            // are settled by the post-compile cross-file check (#960/#977),
            // not this in-compile path (#1095).
            //
            // Cross-file candidate recording (#960 / #977): exactly the
            // links reaching this branch — FileWithFragment, non-img,
            // already past containment + existence — are the ones the
            // post-compile cross-file check can settle once every
            // file's headings are known. `resolved` is already
            // normalised by `resolve_relative` with the shared
            // `zfb_types::normalize_path_lexical` helper; re-applied
            // here (idempotent) so the candidate-target key contract
            // survives any refactor of the resolution path.
            if let Some(out) = ctx.cross_file_links.as_deref_mut() {
                out.push(CrossFileLinkCandidate {
                    source_path: env.source_path.to_path_buf(),
                    target_path: normalize_path_lexical(&resolved),
                    fragment: fragment.to_string(),
                    raw_href: raw_href.to_string(),
                    severity: env.severity,
                });
            }
            return;
        }
    };
    // Accept the fragment if it matches any heading id OR any explicit anchor id.
    if entries.iter().any(|e| e.id == fragment) {
        return;
    }
    if ctx
        .heading_registry
        .as_ref()
        .is_some_and(|r| r.has_anchor(&resolved, fragment))
    {
        return;
    }
    emit_broken_link(raw_href, env, ctx);
}

/// Emit a `BrokenLink` diagnostic through `ctx.diagnostics`.
fn emit_broken_link(url: &str, env: &ValidationEnv<'_>, ctx: &mut BuildContext<'_>) {
    if let Some(sink) = ctx.diagnostics.as_deref_mut() {
        sink.emit(MarkdownDiagnostic::BrokenLink {
            severity: env.severity,
            url: url.to_string(),
            location: Some(SourceLocation::from_path(env.source_path.to_path_buf())),
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
        use zfb_md_ast::{diagnostics::CollectingSink, HastVisitor};

        let dir = tempfile::Builder::new()
            .prefix("link_val_img_fragment")
            .tempdir()
            .unwrap();
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
            cross_file_links: None,
        };
        let mut plugin = LinkValidationPlugin::new(LinkValidationConfig::default());
        plugin.visit_with_context(&mut root, &mut ctx);

        let diags = sink.take();
        assert!(
            diags.is_empty(),
            "img src with svg sprite fragment must not emit BrokenLink: {diags:?}"
        );
    }

    // ── #1392: percent-encoded paths must not false-positive as broken ────

    /// `[x](./my%20file.md)` probed the raw `%20`-encoded path against the
    /// filesystem, so a real file named `my file.md` (decoded) was reported
    /// as a broken link. Percent-containing file paths are now skipped —
    /// mirroring the existing percent-encoded-fragment rule — rather than
    /// false-positiving.
    #[test]
    fn percent_encoded_file_path_is_not_reported_broken() {
        use zfb_md_ast::{diagnostics::CollectingSink, HastVisitor};

        let dir = tempfile::Builder::new()
            .prefix("link_val_percent_path")
            .tempdir()
            .unwrap();
        // The real on-disk file has a space in its name — the href encodes
        // it as `%20`, which the pre-fix code probed verbatim (a file
        // literally named "my%20file.md" does not exist).
        std::fs::write(dir.path().join("my file.md"), "# Hi\n").unwrap();
        let source_path = dir.path().join("page.md");
        std::fs::write(&source_path, "").unwrap();

        let mut root = HastNode::Root {
            children: vec![HastNode::Element {
                tag: "a".to_string(),
                attrs: vec![("href".to_string(), "./my%20file.md".to_string())],
                children: vec![],
                void: false,
            }],
        };

        let mut sink = CollectingSink::new();
        let mut ctx = BuildContext {
            source_path: Some(source_path),
            project_root: dir.path().to_path_buf(),
            public_dir: dir.path().to_path_buf(),
            heading_registry: None,
            diagnostics: Some(&mut sink),
            cross_file_links: None,
        };
        let mut plugin = LinkValidationPlugin::new(LinkValidationConfig::default());
        plugin.visit_with_context(&mut root, &mut ctx);

        let diags = sink.take();
        assert!(
            diags.is_empty(),
            "percent-encoded file path must not be reported broken: {diags:?}"
        );
    }

    /// Same guard on the `path#fragment` variant: `./my%20file.md#anchor`
    /// probes the same decoded on-disk file, so the `%`-containing path must
    /// be skipped in `validate_file_with_fragment` too, not just
    /// `validate_file_exists`.
    #[test]
    fn percent_encoded_file_path_with_fragment_is_not_reported_broken() {
        use zfb_md_ast::{diagnostics::CollectingSink, HastVisitor};

        let dir = tempfile::Builder::new()
            .prefix("link_val_percent_path_frag")
            .tempdir()
            .unwrap();
        std::fs::write(dir.path().join("my file.md"), "# Hi\n").unwrap();
        let source_path = dir.path().join("page.md");
        std::fs::write(&source_path, "").unwrap();

        let mut root = HastNode::Root {
            children: vec![HastNode::Element {
                tag: "a".to_string(),
                attrs: vec![("href".to_string(), "./my%20file.md#hi".to_string())],
                children: vec![],
                void: false,
            }],
        };

        let mut sink = CollectingSink::new();
        let mut ctx = BuildContext {
            source_path: Some(source_path),
            project_root: dir.path().to_path_buf(),
            public_dir: dir.path().to_path_buf(),
            heading_registry: None,
            diagnostics: Some(&mut sink),
            cross_file_links: None,
        };
        let mut plugin = LinkValidationPlugin::new(LinkValidationConfig::default());
        plugin.visit_with_context(&mut root, &mut ctx);

        let diags = sink.take();
        assert!(
            diags.is_empty(),
            "percent-encoded file path with fragment must not be reported broken: {diags:?}"
        );
    }

    // ── Read recording (zfb#944) ──────────────────────────────────────────

    /// Run the plugin (with a recorder) over a single `<a href>` and
    /// return the recorded reads.
    fn record_reads_for_href(
        href: &str,
        dir: &Path,
        source_path: &Path,
    ) -> std::collections::BTreeMap<PathBuf, zfb_md_ast::ReadOutcome> {
        use zfb_md_ast::HastVisitor;

        let recorder = Arc::new(ReadRecorder::new());
        let mut plugin = LinkValidationPlugin::new(LinkValidationConfig::default())
            .with_recorder(Arc::clone(&recorder));
        let mut root = HastNode::Root {
            children: vec![HastNode::Element {
                tag: "a".to_string(),
                attrs: vec![("href".to_string(), href.to_string())],
                children: vec![],
                void: false,
            }],
        };
        let mut ctx = BuildContext {
            source_path: Some(source_path.to_path_buf()),
            project_root: dir.to_path_buf(),
            public_dir: dir.to_path_buf(),
            heading_registry: None,
            diagnostics: None,
            cross_file_links: None,
        };
        plugin.visit_with_context(&mut root, &mut ctx);
        recorder.take_reads()
    }

    #[test]
    fn linked_file_probe_records_full_content_hash() {
        let dir = tempfile::Builder::new()
            .prefix("linkval_rec_ok")
            .tempdir()
            .unwrap();
        std::fs::write(dir.path().join("other.md"), b"# Other\n").unwrap();
        let source = dir.path().join("page.md");

        let reads = record_reads_for_href("./other.md", dir.path(), &source);
        assert_eq!(
            reads.get(&dir.path().join("other.md")),
            Some(&zfb_md_ast::ReadOutcome::of_bytes(b"# Other\n")),
            "the existence probe must record the linked file's FULL hash: {reads:?}"
        );
    }

    #[test]
    fn missing_link_target_records_missing_outcome() {
        let dir = tempfile::Builder::new()
            .prefix("linkval_rec_missing")
            .tempdir()
            .unwrap();
        let source = dir.path().join("page.md");

        let reads = record_reads_for_href("./absent.md", dir.path(), &source);
        assert_eq!(
            reads.get(&dir.path().join("absent.md")),
            Some(&zfb_md_ast::ReadOutcome::Missing),
            "a missing link target must record Missing so creating it later \
             invalidates a cached entry: {reads:?}"
        );
    }

    #[test]
    fn file_with_fragment_records_the_target_file() {
        let dir = tempfile::Builder::new()
            .prefix("linkval_rec_frag")
            .tempdir()
            .unwrap();
        std::fs::write(dir.path().join("other.md"), b"# Heading\n").unwrap();
        let source = dir.path().join("page.md");

        let reads = record_reads_for_href("./other.md#heading", dir.path(), &source);
        assert!(
            reads.contains_key(&dir.path().join("other.md")),
            "a file#fragment link must record the TARGET file (its content \
             carries the headings the fragment is validated against): {reads:?}"
        );
    }

    #[test]
    fn non_filesystem_hrefs_record_nothing() {
        let dir = tempfile::Builder::new()
            .prefix("linkval_rec_none")
            .tempdir()
            .unwrap();
        let source = dir.path().join("page.md");

        for href in [
            "https://example.com/x",
            "mailto:a@b.com",
            "#fragment-only",
            "/docs/intro/", // URL-space — never a filesystem read
            "../escape.md", // project-root escape rejected before any fs access
        ] {
            let reads = record_reads_for_href(href, dir.path(), &source);
            assert!(
                reads.is_empty(),
                "href {href:?} performs no filesystem read — nothing to record: {reads:?}"
            );
        }
    }

    // ── New tests: URL-space classification ───────────────────────────────

    #[test]
    fn parse_site_absolute_is_url_space() {
        assert_eq!(parse_link("/docs/intro/"), ParsedLink::UrlSpace);
        assert_eq!(parse_link("/docs/intro/#frag"), ParsedLink::UrlSpace);
        assert_eq!(parse_link("//cdn.example.com/x"), ParsedLink::UrlSpace);
    }

    #[test]
    fn parse_strips_query_string() {
        assert_eq!(
            parse_link("./other.md?x=1"),
            ParsedLink::FilePath("./other.md".to_string())
        );
        assert_eq!(
            parse_link("./other.md?x=1#frag"),
            ParsedLink::FileWithFragment {
                path: "./other.md".to_string(),
                fragment: "frag".to_string(),
            }
        );
        // bare-query href → URL-space (no path component)
        assert_eq!(parse_link("?x=1"), ParsedLink::UrlSpace);
    }

    // ── New test: cross-file fragment with None registry ──────────────────

    /// `./other.md#frag` with `heading_registry: None` must not report a
    /// BrokenLink when the target exists (existence-only degradation), but
    /// MUST report when the target is missing.
    #[test]
    fn cross_file_fragment_with_none_registry_checks_existence_only() {
        use zfb_md_ast::{diagnostics::CollectingSink, HastVisitor};

        let dir = tempfile::Builder::new()
            .prefix("lv_none_reg")
            .tempdir()
            .unwrap();
        let source_path = dir.path().join("page.md");
        std::fs::write(&source_path, "").unwrap();
        let other_path = dir.path().join("other.md");
        std::fs::write(&other_path, "# Other\n").unwrap();

        // Scenario A: target exists, registry None → no diagnostic (existence-only).
        {
            let node = HastNode::Root {
                children: vec![HastNode::Element {
                    tag: "a".to_string(),
                    attrs: vec![("href".to_string(), "./other.md#frag".to_string())],
                    children: vec![],
                    void: false,
                }],
            };
            let mut root = node;
            let mut sink = CollectingSink::new();
            let mut ctx = BuildContext {
                source_path: Some(source_path.clone()),
                project_root: dir.path().to_path_buf(),
                public_dir: dir.path().to_path_buf(),
                heading_registry: None,
                diagnostics: Some(&mut sink),
                cross_file_links: None,
            };
            let mut plugin = LinkValidationPlugin::new(LinkValidationConfig::default());
            plugin.visit_with_context(&mut root, &mut ctx);
            let diags = sink.take();
            assert!(
                diags.is_empty(),
                "existing target with None registry must not report broken: {diags:?}"
            );
        }

        // Scenario B: target missing, registry None → BrokenLink (file not found).
        {
            let node = HastNode::Root {
                children: vec![HastNode::Element {
                    tag: "a".to_string(),
                    attrs: vec![("href".to_string(), "./missing.md#frag".to_string())],
                    children: vec![],
                    void: false,
                }],
            };
            let mut root = node;
            let mut sink = CollectingSink::new();
            let mut ctx = BuildContext {
                source_path: Some(source_path.clone()),
                project_root: dir.path().to_path_buf(),
                public_dir: dir.path().to_path_buf(),
                heading_registry: None,
                diagnostics: Some(&mut sink),
                cross_file_links: None,
            };
            let mut plugin = LinkValidationPlugin::new(LinkValidationConfig::default());
            plugin.visit_with_context(&mut root, &mut ctx);
            let diags = sink.take();
            assert_eq!(
                diags.len(),
                1,
                "missing target must report BrokenLink: {diags:?}"
            );
            assert!(
                matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "./missing.md#frag"),
                "url must be the raw href: {diags:?}"
            );
        }
    }

    // ── Cross-file candidate recording (#960 / #977) ──────────────────────

    /// Run the plugin over a single `<a href>` / `<img src>` with the
    /// candidates channel wired, returning what it recorded and emitted.
    fn record_candidates_for_href(
        href: &str,
        dir: &Path,
        source_path: &Path,
        registry: Option<&mut zfb_md_ast::heading_registry::HeadingRegistry>,
        config: LinkValidationConfig,
        is_img: bool,
    ) -> (Vec<CrossFileLinkCandidate>, Vec<MarkdownDiagnostic>) {
        use zfb_md_ast::{diagnostics::CollectingSink, HastVisitor};

        let mut sink = CollectingSink::new();
        let mut candidates: Vec<CrossFileLinkCandidate> = Vec::new();
        let (tag, attr) = if is_img {
            ("img", "src")
        } else {
            ("a", "href")
        };
        let mut root = HastNode::Root {
            children: vec![HastNode::Element {
                tag: tag.to_string(),
                attrs: vec![(attr.to_string(), href.to_string())],
                children: vec![],
                void: is_img,
            }],
        };
        let mut ctx = BuildContext {
            source_path: Some(source_path.to_path_buf()),
            project_root: dir.to_path_buf(),
            public_dir: dir.to_path_buf(),
            heading_registry: registry,
            diagnostics: Some(&mut sink),
            cross_file_links: Some(&mut candidates),
        };
        let mut plugin = LinkValidationPlugin::new(config);
        plugin.visit_with_context(&mut root, &mut ctx);
        drop(ctx);
        (candidates, sink.take())
    }

    /// The degrade branch — existing target, no registry entry for it —
    /// must record exactly one candidate carrying the normalised target,
    /// the fragment, the raw href, and the default Warning severity, and
    /// must emit NO diagnostic (the verdict is deferred, not broken).
    #[test]
    fn degrade_branch_records_cross_file_candidate() {
        let dir = tempfile::Builder::new()
            .prefix("cand_degrade")
            .tempdir()
            .unwrap();
        std::fs::write(dir.path().join("other.md"), b"## Target\n").unwrap();
        // `sub/..` in the source spelling collapses lexically during
        // target resolution — the candidate target key must come out in
        // the shared-helper (`zfb_types::normalize_path_lexical`) form.
        let source = dir.path().join("sub").join("..").join("page.md");

        let mut registry = zfb_md_ast::heading_registry::HeadingRegistry::new();
        let (candidates, diags) = record_candidates_for_href(
            "./other.md#target",
            dir.path(),
            &source,
            Some(&mut registry),
            LinkValidationConfig::default(),
            false,
        );
        assert!(
            diags.is_empty(),
            "deferred verdict must not emit: {diags:?}"
        );
        assert_eq!(candidates.len(), 1, "exactly one candidate: {candidates:?}");
        let c = &candidates[0];
        assert_eq!(c.target_path, dir.path().join("other.md"));
        assert_eq!(c.fragment, "target");
        assert_eq!(c.raw_href, "./other.md#target");
        assert_eq!(
            c.source_path, source,
            "source stays as authored (diagnostic location, not a key)"
        );
        assert_eq!(c.severity, DiagnosticSeverity::Warning);
    }

    /// `failOnBroken: true` must stamp `Error` severity into the
    /// candidate so the post-compile check can fail the build exactly as
    /// an in-compile verdict would have.
    #[test]
    fn fail_on_broken_candidate_carries_error_severity() {
        let dir = tempfile::Builder::new()
            .prefix("cand_severity")
            .tempdir()
            .unwrap();
        std::fs::write(dir.path().join("other.md"), b"x\n").unwrap();
        let source = dir.path().join("page.md");

        let (candidates, _) = record_candidates_for_href(
            "./other.md#frag",
            dir.path(),
            &source,
            None,
            LinkValidationConfig {
                fail_on_broken: Some(true),
            },
            false,
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].severity, DiagnosticSeverity::Error);
    }

    /// A fragment the per-compile registry CAN settle (entry present for
    /// the target) must not record a candidate — valid and broken alike.
    #[test]
    fn locally_settled_fragment_records_no_candidate() {
        let dir = tempfile::Builder::new()
            .prefix("cand_local")
            .tempdir()
            .unwrap();
        std::fs::write(dir.path().join("other.md"), b"## Target\n").unwrap();
        let source = dir.path().join("page.md");

        let mut registry = zfb_md_ast::heading_registry::HeadingRegistry::new();
        registry.insert(
            dir.path().join("other.md"),
            zfb_md_ast::heading_registry::HeadingEntry {
                id: "target".to_string(),
                text: "Target".to_string(),
                depth: 2,
            },
        );

        // Valid fragment → settled locally, silent.
        let (candidates, diags) = record_candidates_for_href(
            "./other.md#target",
            dir.path(),
            &source,
            Some(&mut registry),
            LinkValidationConfig::default(),
            false,
        );
        assert!(candidates.is_empty(), "settled verdict: {candidates:?}");
        assert!(diags.is_empty(), "valid fragment: {diags:?}");

        // Broken fragment → settled locally as BrokenLink, still no candidate.
        let (candidates, diags) = record_candidates_for_href(
            "./other.md#nope",
            dir.path(),
            &source,
            Some(&mut registry),
            LinkValidationConfig::default(),
            false,
        );
        assert!(candidates.is_empty(), "settled verdict: {candidates:?}");
        assert_eq!(diags.len(), 1, "broken fragment emits locally: {diags:?}");
    }

    /// Links that never reach the degrade branch must record nothing:
    /// missing targets (existence verdict), img sprite fragments, bare
    /// fragments, fragment-less file links, root-escaping refs, and
    /// empty / percent-encoded fragments.
    #[test]
    fn non_degrading_hrefs_record_no_candidate() {
        let dir = tempfile::Builder::new()
            .prefix("cand_none")
            .tempdir()
            .unwrap();
        std::fs::write(dir.path().join("other.md"), b"x\n").unwrap();
        std::fs::write(dir.path().join("sprite.svg"), b"<svg/>").unwrap();
        let source = dir.path().join("page.md");

        for (href, is_img) in [
            ("./missing.md#frag", false),         // existence verdict (BrokenLink)
            ("./sprite.svg#icon-x", true),        // img sprite fragment
            ("#local", false),                    // bare fragment (same-file contract)
            ("./other.md", false),                // no fragment at all
            ("../escape.md#frag", false),         // containment verdict (BrokenLink)
            ("./other.md#", false),               // empty fragment (dummy link)
            ("./other.md#a%20b", false),          // percent-encoded fragment
            ("https://example.com/#frag", false), // external
        ] {
            let (candidates, _) = record_candidates_for_href(
                href,
                dir.path(),
                &source,
                None,
                LinkValidationConfig::default(),
                is_img,
            );
            assert!(
                candidates.is_empty(),
                "href {href:?} must not record a candidate: {candidates:?}"
            );
        }
    }

    /// A `None` channel keeps the degrade branch a silent no-op — the
    /// pre-#977 unarmed behaviour, byte-for-byte.
    #[test]
    fn missing_channel_keeps_degrade_branch_silent() {
        use zfb_md_ast::{diagnostics::CollectingSink, HastVisitor};

        let dir = tempfile::Builder::new()
            .prefix("cand_no_channel")
            .tempdir()
            .unwrap();
        std::fs::write(dir.path().join("other.md"), b"x\n").unwrap();
        let source = dir.path().join("page.md");

        let mut sink = CollectingSink::new();
        let mut root = HastNode::Root {
            children: vec![HastNode::Element {
                tag: "a".to_string(),
                attrs: vec![("href".to_string(), "./other.md#frag".to_string())],
                children: vec![],
                void: false,
            }],
        };
        let mut ctx = BuildContext {
            source_path: Some(source),
            project_root: dir.path().to_path_buf(),
            public_dir: dir.path().to_path_buf(),
            heading_registry: None,
            diagnostics: Some(&mut sink),
            cross_file_links: None,
        };
        let mut plugin = LinkValidationPlugin::new(LinkValidationConfig::default());
        plugin.visit_with_context(&mut root, &mut ctx);
        assert!(
            sink.take().is_empty(),
            "existence-only degrade with no channel must stay silent"
        );
    }
}
