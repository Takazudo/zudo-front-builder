//! Transclusion of other markdown files via `:::include{file="./path.md"}`.
//!
//! # Syntax
//!
//! ```text
//! :::include{file="./snippet.md"}
//! ```
//!
//! Inline a file's entire parsed mdast at the include site. The `file`
//! attribute is resolved relative to the **current source file** (from
//! `BuildContext.source_path`).
//!
//! ## Line-range option
//!
//! ```text
//! :::include{file="./foo.rs" lines="10-30"}
//! ```
//!
//! Slice only lines 10–30 (1-indexed, inclusive) of the file before parsing.
//! Works for both `.md`/`.mdx` content and arbitrary text files paired with
//! `code=true`.
//!
//! ## Code-fence wrap option
//!
//! ```text
//! :::include{file="./foo.rs" code=true lang="rust"}
//! ```
//!
//! Instead of parsing the included content as Markdown, wrap it verbatim in a
//! fenced `Code` node (`~~~lang … ~~~`). Useful for embedding source files
//! as highlighted code blocks. `lang` defaults to `""` when absent.
//!
//! # Parse shape
//!
//! The MDX parser renders `:::include{file="./x.md"}` as a `Paragraph` with
//! exactly two inline children:
//!
//! 1. `Text { value: ":::include" }`
//! 2. `MdxTextExpression { value: "file=\"./x.md\" …" }`
//!
//! This module matches that two-child pattern and replaces the parent
//! `Paragraph` with the spliced nodes from the included file.
//!
//! # Security
//!
//! - Absolute `file` paths are rejected with an error diagnostic.
//! - Paths that escape `project_root` (after canonicalization) are rejected.
//! - Symlink chains that point outside `project_root` are rejected.
//!
//! # Cycle detection
//!
//! The visitor maintains a set of currently-being-transcluded absolute paths.
//! If A→B→A is detected, an Error-severity diagnostic is emitted and the
//! second inclusion is skipped (the `:::include` node is left in the tree as
//! an empty paragraph rather than causing infinite recursion).
//!
//! # Config
//!
//! Wire via `features.transclude: {}` or `features.transclude: { maxDepth: N }`
//! in `zfb.config.ts`. Phase: **mdast** (runs early so included content is
//! processed by subsequent mdast visitors).
//!
//! # Wave 6 (#581)

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use markdown::mdast::Node as MdastNode;
use zfb_md_ast::{AttrSchema, AttrType, BuildContext, DirectiveDef, MdastVisitor};

use crate::TranscludeConfig;

// ── Public API ──────────────────────────────────────────────────────────────

/// Return the `DirectiveDef` for the `:::include` directive.
///
/// Declares the required `file` attribute (string) and optional `lines`,
/// `code`, and `lang` attributes.
#[must_use]
pub fn include_directive_def() -> DirectiveDef {
    DirectiveDef::container("include", "include").with_attrs(vec![
        AttrSchema {
            name: "file".to_string(),
            ty: AttrType::String,
            default: None,
            required: true,
        },
        AttrSchema {
            name: "lines".to_string(),
            ty: AttrType::String,
            default: None,
            required: false,
        },
        AttrSchema {
            name: "code".to_string(),
            ty: AttrType::Boolean,
            default: None,
            required: false,
        },
        AttrSchema {
            name: "lang".to_string(),
            ty: AttrType::String,
            default: None,
            required: false,
        },
    ])
}

/// Mdast visitor that replaces `:::include{file="…"}` directive paragraphs
/// with the parsed mdast nodes from the referenced file.
///
/// Requires a `BuildContext` with `source_path` (or falls back gracefully
/// with a diagnostic when absent). Override `visit_with_context` for the
/// context-aware path; the base `visit` is a no-op (cannot resolve without
/// paths).
///
/// Wire via `features.transclude: {}` in `zfb.config.ts`.
#[derive(Debug)]
pub struct TranscludePlugin {
    config: TranscludeConfig,
}

impl TranscludePlugin {
    /// Create a new plugin from the given config.
    #[must_use]
    pub fn new(config: TranscludeConfig) -> Self {
        Self { config }
    }
}

impl MdastVisitor for TranscludePlugin {
    /// No-op when called without context — cannot resolve file paths.
    fn visit(&mut self, _node: &mut MdastNode) {}

    /// Context-aware path: resolves include directives using `ctx.source_path`
    /// and `ctx.project_root`.
    fn visit_with_context(&mut self, node: &mut MdastNode, ctx: &mut BuildContext<'_>) {
        let Some(source_path) = ctx.source_path.clone() else {
            // No source path — cannot resolve includes. Emit a warning if a
            // sink is wired, then return.
            if let Some(sink) = ctx.diagnostics.as_mut() {
                sink.emit(zfb_md_ast::diagnostics::MarkdownDiagnostic::warning(
                    "transclude: no source_path in BuildContext; includes cannot be resolved",
                ));
            }
            return;
        };

        let source_dir = match source_path.parent() {
            Some(d) => d.to_path_buf(),
            None => PathBuf::from("."),
        };
        let project_root = ctx.project_root.clone();
        let max_depth = self.config.max_depth;

        let mut visited: HashSet<PathBuf> = HashSet::new();
        // Mark the current file as "in the stack" to detect direct cycles.
        if let Ok(canonical_self) = source_path.canonicalize() {
            visited.insert(canonical_self);
        }

        expand_includes_in_node(
            node,
            &source_dir,
            &project_root,
            &mut visited,
            0,
            max_depth,
            ctx,
        );
    }
}

// ── Recursive expansion ─────────────────────────────────────────────────────

/// Recursively scan `node`'s children for `:::include` paragraphs and
/// replace each one with the parsed nodes from the referenced file.
///
/// Dispatches to [`expand_includes_in_children`] on the appropriate child
/// vec for each container node type. Non-container nodes are skipped.
fn expand_includes_in_node(
    node: &mut MdastNode,
    source_dir: &Path,
    project_root: &Path,
    visited: &mut HashSet<PathBuf>,
    depth: u8,
    max_depth: u8,
    ctx: &mut BuildContext<'_>,
) {
    let children = match node {
        MdastNode::Root(r) => &mut r.children,
        MdastNode::ListItem(li) => &mut li.children,
        MdastNode::Blockquote(bq) => &mut bq.children,
        MdastNode::MdxJsxFlowElement(j) => &mut j.children,
        _ => return,
    };
    expand_includes_in_children(children, source_dir, project_root, visited, depth, max_depth, ctx);
}

/// Scan a `Vec<MdastNode>` (the children of a container) for `:::include`
/// paragraphs, replacing each with the spliced nodes from the referenced file.
///
/// Also recurses into nested container nodes so includes nested inside
/// blockquotes, list items, etc. are found.
///
/// `depth` is the current nesting level; `max_depth` is the configured bound.
fn expand_includes_in_children(
    children: &mut Vec<MdastNode>,
    source_dir: &Path,
    project_root: &Path,
    visited: &mut HashSet<PathBuf>,
    depth: u8,
    max_depth: u8,
    ctx: &mut BuildContext<'_>,
) {
    let mut i = 0;
    while i < children.len() {
        if let Some(attrs) = extract_include_attrs(&children[i]) {
            // Found an `:::include` paragraph at `i`.
            let replacement = resolve_and_expand(
                &attrs,
                source_dir,
                project_root,
                visited,
                depth,
                max_depth,
                ctx,
            );

            // Remove the directive paragraph.
            children.remove(i);

            // Insert the replacement nodes at `i`.
            let insert_count = replacement.len();
            for (offset, new_node) in replacement.into_iter().enumerate() {
                children.insert(i + offset, new_node);
            }
            i += insert_count;
        } else {
            // Not an include directive — recurse into container children.
            expand_includes_in_node(
                &mut children[i],
                source_dir,
                project_root,
                visited,
                depth,
                max_depth,
                ctx,
            );
            i += 1;
        }
    }
}

/// Resolve the file and return the replacement nodes.
///
/// On error (path escape, cycle, read failure, depth exceeded), emits a
/// diagnostic and returns an empty vec (the directive paragraph is removed,
/// leaving nothing in its place).
fn resolve_and_expand(
    attrs: &IncludeAttrs,
    source_dir: &Path,
    project_root: &Path,
    visited: &mut HashSet<PathBuf>,
    depth: u8,
    max_depth: u8,
    ctx: &mut BuildContext<'_>,
) -> Vec<MdastNode> {
    // ── Depth check ────────────────────────────────────────────────────────
    if depth >= max_depth {
        emit_error(
            ctx,
            format!(
                "transclude: max depth ({max_depth}) exceeded while including \"{}\"; chain is too deep",
                attrs.file
            ),
        );
        return Vec::new();
    }

    // ── Reject absolute paths ───────────────────────────────────────────────
    let raw_path = Path::new(&attrs.file);
    if raw_path.is_absolute() {
        emit_error(
            ctx,
            format!(
                "transclude: absolute file paths are not allowed; got \"{}\"",
                attrs.file
            ),
        );
        return Vec::new();
    }

    // ── Resolve relative to source dir ──────────────────────────────────────
    let resolved = source_dir.join(raw_path);

    // ── Canonicalize + project-root check ─────────────────────────────────
    let canonical = match resolved.canonicalize() {
        Ok(c) => c,
        Err(e) => {
            emit_error(
                ctx,
                format!(
                    "transclude: cannot read \"{}\": {e}",
                    resolved.display()
                ),
            );
            return Vec::new();
        }
    };

    let canonical_root = project_root.canonicalize().unwrap_or_else(|_| project_root.to_path_buf());
    if !canonical.starts_with(&canonical_root) {
        emit_error(
            ctx,
            format!(
                "transclude: \"{}\" escapes project root \"{}\"",
                canonical.display(),
                canonical_root.display()
            ),
        );
        return Vec::new();
    }

    // ── Cycle detection ────────────────────────────────────────────────────
    if visited.contains(&canonical) {
        emit_error(
            ctx,
            format!(
                "transclude: cycle detected — \"{}\" is already in the include stack",
                canonical.display()
            ),
        );
        return Vec::new();
    }

    // ── Read file ─────────────────────────────────────────────────────────
    let raw_content = match std::fs::read_to_string(&canonical) {
        Ok(c) => c,
        Err(e) => {
            emit_error(
                ctx,
                format!(
                    "transclude: cannot read \"{}\": {e}",
                    canonical.display()
                ),
            );
            return Vec::new();
        }
    };

    // ── Apply line-range slice ────────────────────────────────────────────
    let content = if let Some(ref lines_spec) = attrs.lines {
        match parse_line_range(lines_spec) {
            Some((start, end)) => slice_lines(&raw_content, start, end),
            None => {
                emit_error(
                    ctx,
                    format!(
                        "transclude: invalid lines attribute \"{lines_spec}\"; expected \"N-M\" (e.g. \"10-30\")"
                    ),
                );
                return Vec::new();
            }
        }
    } else {
        raw_content.clone()
    };

    // ── code=true wrap ───────────────────────────────────────────────────
    if attrs.code {
        let lang = attrs.lang.clone().unwrap_or_default();
        let code_node = MdastNode::Code(markdown::mdast::Code {
            lang: if lang.is_empty() { None } else { Some(lang) },
            meta: None,
            value: content,
            position: None,
        });
        return vec![code_node];
    }

    // ── Parse as mdast ────────────────────────────────────────────────────
    let opts = markdown::ParseOptions::mdx();
    let included_mdast = match markdown::to_mdast(&content, &opts) {
        Ok(tree) => tree,
        Err(e) => {
            emit_error(
                ctx,
                format!(
                    "transclude: failed to parse \"{}\": {e}",
                    canonical.display()
                ),
            );
            return Vec::new();
        }
    };

    let MdastNode::Root(root) = included_mdast else {
        return Vec::new();
    };

    let mut nodes = root.children;

    // ── Recurse into the included file's nodes ──────────────────────────
    // Use `expand_includes_in_children` (not `expand_includes_in_node`) so
    // top-level `:::include` directives in the included file are found
    // directly in the root's children vec rather than being missed by the
    // container-dispatch in `expand_includes_in_node` (which skips Paragraph
    // nodes — the `:::include` pattern appears as a top-level Paragraph).
    let included_source_dir = canonical.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
    visited.insert(canonical.clone());

    expand_includes_in_children(
        &mut nodes,
        &included_source_dir,
        project_root,
        visited,
        depth + 1,
        max_depth,
        ctx,
    );

    visited.remove(&canonical);

    nodes
}

// ── Attribute extraction ────────────────────────────────────────────────────

/// Parsed attributes from a `:::include{...}` directive.
#[derive(Debug, Clone)]
struct IncludeAttrs {
    /// Required: relative path to the file to include.
    file: String,
    /// Optional line range `"N-M"` (1-indexed, inclusive).
    lines: Option<String>,
    /// When true, wrap the file's content in a code block rather than parsing.
    code: bool,
    /// Language identifier for the code block (only relevant when `code: true`).
    lang: Option<String>,
}

/// Check if `node` is an `:::include` paragraph and extract its attributes.
///
/// Returns `Some(attrs)` iff the node is a `Paragraph` with exactly two
/// children: `Text(":::include")` + `MdxTextExpression(attrs_str)`.
fn extract_include_attrs(node: &MdastNode) -> Option<IncludeAttrs> {
    let MdastNode::Paragraph(p) = node else {
        return None;
    };
    if p.children.len() != 2 {
        return None;
    }
    let MdastNode::Text(t) = &p.children[0] else {
        return None;
    };
    if first_line(&t.value).trim() != ":::include" {
        return None;
    }
    let MdastNode::MdxTextExpression(expr) = &p.children[1] else {
        return None;
    };

    parse_attrs_from_expression(&expr.value)
}

/// Parse key="value" / key=value / key=true/false attributes from an
/// `MdxTextExpression` value string.
///
/// The MDX parser stores the raw attr string without the surrounding `{}`,
/// e.g. `file="./snippet.md" lines="10-30" code=true lang="rust"`.
fn parse_attrs_from_expression(expr_value: &str) -> Option<IncludeAttrs> {
    let attrs = parse_attr_string(expr_value);

    let file = attrs.get("file").cloned()?;
    if file.is_empty() {
        return None;
    }

    let lines = attrs.get("lines").cloned();
    let code = attrs.get("code").map(|v| v == "true").unwrap_or(false);
    let lang = attrs.get("lang").cloned().filter(|l| !l.is_empty());

    Some(IncludeAttrs { file, lines, code, lang })
}

/// Parse a key="value" / key=value / key=true / key=false string into a map.
///
/// Handles:
/// - Quoted values: `key="val"` or `key='val'`
/// - Unquoted values: `key=value` (no whitespace)
/// - Boolean shorthands: `key=true`, `key=false`
/// - Keys without values (treated as boolean true): `key`
fn parse_attr_string(s: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut rest = s.trim();

    while !rest.is_empty() {
        // Skip leading whitespace
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }

        // Read key
        let key_end = rest.find(|c: char| c == '=' || c.is_whitespace()).unwrap_or(rest.len());
        let key = rest[..key_end].to_string();
        rest = &rest[key_end..];

        if key.is_empty() {
            // Skip malformed token
            rest = &rest[1.min(rest.len())..];
            continue;
        }

        if rest.starts_with('=') {
            rest = &rest[1..]; // consume '='
            // Read value
            if rest.starts_with('"') {
                // Quoted with "
                rest = &rest[1..];
                let end = rest.find('"').unwrap_or(rest.len());
                let value = rest[..end].to_string();
                rest = if end < rest.len() { &rest[end + 1..] } else { &rest[end..] };
                map.insert(key, value);
            } else if rest.starts_with('\'') {
                // Quoted with '
                rest = &rest[1..];
                let end = rest.find('\'').unwrap_or(rest.len());
                let value = rest[..end].to_string();
                rest = if end < rest.len() { &rest[end + 1..] } else { &rest[end..] };
                map.insert(key, value);
            } else {
                // Unquoted value (stops at whitespace)
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                let value = rest[..end].to_string();
                rest = &rest[end..];
                map.insert(key, value);
            }
        } else {
            // No '=' — boolean shorthand (key without value = true)
            map.insert(key, "true".to_string());
        }
    }

    map
}

// ── Line-range helpers ──────────────────────────────────────────────────────

/// Parse `"N-M"` into `(start, end)` (1-indexed, inclusive).
/// Returns `None` if the format is invalid.
fn parse_line_range(spec: &str) -> Option<(usize, usize)> {
    let (start_str, end_str) = spec.split_once('-')?;
    let start: usize = start_str.trim().parse().ok()?;
    let end: usize = end_str.trim().parse().ok()?;
    if start == 0 || end == 0 || start > end {
        return None;
    }
    Some((start, end))
}

/// Slice `content` to lines `start..=end` (1-indexed, inclusive).
///
/// Lines outside the range are silently dropped. If `end` exceeds the
/// actual line count, only available lines are returned.
fn slice_lines(content: &str, start: usize, end: usize) -> String {
    content
        .lines()
        .enumerate()
        .filter(|(i, _)| {
            let line_num = i + 1; // 1-indexed
            line_num >= start && line_num <= end
        })
        .map(|(_, l)| l)
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Diagnostic helper ───────────────────────────────────────────────────────

fn emit_error(ctx: &mut BuildContext<'_>, message: String) {
    if let Some(sink) = ctx.diagnostics.as_mut() {
        sink.emit(zfb_md_ast::diagnostics::MarkdownDiagnostic::error(message));
    }
}

// ── Node predicate helpers ─────────────────────────────────────────────────

fn first_line(s: &str) -> &str {
    match s.find('\n') {
        Some(i) => &s[..i],
        None => s,
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use markdown::mdast::{Paragraph, Text};

    // ── parse_attr_string ──────────────────────────────────────────────────

    #[test]
    fn parse_attrs_quoted_double() {
        let m = parse_attr_string(r#"file="./foo.md""#);
        assert_eq!(m.get("file").map(String::as_str), Some("./foo.md"));
    }

    #[test]
    fn parse_attrs_multiple_attrs() {
        let m = parse_attr_string(r#"file="./foo.rs" code=true lang="rust""#);
        assert_eq!(m.get("file").map(String::as_str), Some("./foo.rs"));
        assert_eq!(m.get("code").map(String::as_str), Some("true"));
        assert_eq!(m.get("lang").map(String::as_str), Some("rust"));
    }

    #[test]
    fn parse_attrs_lines_attr() {
        let m = parse_attr_string(r#"file="./foo.md" lines="10-30""#);
        assert_eq!(m.get("lines").map(String::as_str), Some("10-30"));
    }

    #[test]
    fn parse_attrs_code_without_lang() {
        let m = parse_attr_string(r#"file="./foo.txt" code=true"#);
        assert_eq!(m.get("code").map(String::as_str), Some("true"));
        assert!(m.get("lang").is_none());
    }

    // ── parse_line_range ──────────────────────────────────────────────────

    #[test]
    fn line_range_valid() {
        assert_eq!(parse_line_range("10-30"), Some((10, 30)));
    }

    #[test]
    fn line_range_single_line() {
        assert_eq!(parse_line_range("5-5"), Some((5, 5)));
    }

    #[test]
    fn line_range_invalid_reversed() {
        assert!(parse_line_range("30-10").is_none());
    }

    #[test]
    fn line_range_invalid_format() {
        assert!(parse_line_range("abc").is_none());
        assert!(parse_line_range("").is_none());
    }

    #[test]
    fn line_range_zero_start_rejected() {
        assert!(parse_line_range("0-5").is_none());
    }

    // ── slice_lines ───────────────────────────────────────────────────────

    #[test]
    fn slice_lines_basic() {
        let content = "line1\nline2\nline3\nline4\nline5";
        assert_eq!(slice_lines(content, 2, 4), "line2\nline3\nline4");
    }

    #[test]
    fn slice_lines_single() {
        let content = "a\nb\nc";
        assert_eq!(slice_lines(content, 2, 2), "b");
    }

    #[test]
    fn slice_lines_beyond_end() {
        let content = "a\nb";
        // end=10 exceeds line count — only available lines returned
        assert_eq!(slice_lines(content, 1, 10), "a\nb");
    }

    #[test]
    fn slice_lines_from_start() {
        let content = "a\nb\nc\nd";
        assert_eq!(slice_lines(content, 1, 2), "a\nb");
    }

    // ── extract_include_attrs ─────────────────────────────────────────────

    fn make_include_paragraph(expr_value: &str) -> MdastNode {
        MdastNode::Paragraph(Paragraph {
            children: vec![
                MdastNode::Text(Text {
                    value: ":::include".to_string(),
                    position: None,
                }),
                MdastNode::MdxTextExpression(markdown::mdast::MdxTextExpression {
                    value: expr_value.to_string(),
                    position: None,
                    stops: Vec::new(),
                }),
            ],
            position: None,
        })
    }

    #[test]
    fn extract_basic_include() {
        let node = make_include_paragraph(r#"file="./snippet.md""#);
        let attrs = extract_include_attrs(&node).expect("should match");
        assert_eq!(attrs.file, "./snippet.md");
        assert!(!attrs.code);
        assert!(attrs.lines.is_none());
        assert!(attrs.lang.is_none());
    }

    #[test]
    fn extract_include_with_lines() {
        let node = make_include_paragraph(r#"file="./foo.md" lines="10-30""#);
        let attrs = extract_include_attrs(&node).expect("should match");
        assert_eq!(attrs.file, "./foo.md");
        assert_eq!(attrs.lines.as_deref(), Some("10-30"));
    }

    #[test]
    fn extract_include_with_code_and_lang() {
        let node = make_include_paragraph(r#"file="./foo.rs" code=true lang="rust""#);
        let attrs = extract_include_attrs(&node).expect("should match");
        assert_eq!(attrs.file, "./foo.rs");
        assert!(attrs.code);
        assert_eq!(attrs.lang.as_deref(), Some("rust"));
    }

    #[test]
    fn non_include_paragraph_returns_none() {
        let node = MdastNode::Paragraph(Paragraph {
            children: vec![MdastNode::Text(Text {
                value: "just a paragraph".to_string(),
                position: None,
            })],
            position: None,
        });
        assert!(extract_include_attrs(&node).is_none());
    }

    #[test]
    fn non_include_directive_returns_none() {
        // `:::note` is not an include directive
        let node = make_include_paragraph_with_text(":::note", r#"file="./foo.md""#);
        assert!(extract_include_attrs(&node).is_none());
    }

    fn make_include_paragraph_with_text(text: &str, expr_value: &str) -> MdastNode {
        MdastNode::Paragraph(Paragraph {
            children: vec![
                MdastNode::Text(Text {
                    value: text.to_string(),
                    position: None,
                }),
                MdastNode::MdxTextExpression(markdown::mdast::MdxTextExpression {
                    value: expr_value.to_string(),
                    position: None,
                    stops: Vec::new(),
                }),
            ],
            position: None,
        })
    }

    // ── include_directive_def ─────────────────────────────────────────────

    #[test]
    fn directive_def_validates_file_attr() {
        let def = include_directive_def();
        let (result, warnings) = def.validate_attrs(&[("file".to_string(), "./foo.md".to_string())]);
        assert!(result.is_ok(), "valid file attr must pass: {result:?}");
        assert!(warnings.is_empty());
    }

    #[test]
    fn directive_def_rejects_missing_required_file() {
        let def = include_directive_def();
        let (result, _) = def.validate_attrs(&[]);
        assert!(result.is_err(), "missing required `file` must fail");
    }

    #[test]
    fn directive_def_accepts_all_attrs() {
        let def = include_directive_def();
        let attrs = vec![
            ("file".to_string(), "./foo.rs".to_string()),
            ("lines".to_string(), "1-10".to_string()),
            ("code".to_string(), "true".to_string()),
            ("lang".to_string(), "rust".to_string()),
        ];
        let (result, warnings) = def.validate_attrs(&attrs);
        assert!(result.is_ok(), "all valid attrs must pass: {result:?}");
        assert!(warnings.is_empty());
    }

    // ── Integration: file resolution (using tempdir) ──────────────────────

    use tempdir::TempDir;

    fn run_transclude(
        input_md: &str,
        source_path: PathBuf,
        project_root: PathBuf,
        config: TranscludeConfig,
    ) -> (MdastNode, Vec<zfb_md_ast::diagnostics::MarkdownDiagnostic>) {
        let opts = markdown::ParseOptions::mdx();
        let mut tree = markdown::to_mdast(input_md, &opts).expect("parse ok");
        let mut sink = zfb_md_ast::diagnostics::CollectingSink::new();
        let mut ctx = BuildContext {
            source_path: Some(source_path),
            project_root,
            public_dir: PathBuf::from("/tmp"),
            heading_registry: None,
            diagnostics: Some(&mut sink),
        };
        let mut plugin = TranscludePlugin::new(config);
        plugin.visit_with_context(&mut tree, &mut ctx);
        let diags = sink.take();
        (tree, diags)
    }

    #[test]
    fn simple_include_inlines_content() {
        let dir = TempDir::new("transclude_test").unwrap();
        let snippet_path = dir.path().join("snippet.md");
        std::fs::write(&snippet_path, "# Hello\n\nWorld.\n").unwrap();

        let input = r#":::include{file="./snippet.md"}"#;
        let source = dir.path().join("input.md");
        std::fs::write(&source, input).unwrap();

        let (tree, diags) = run_transclude(
            input,
            source,
            dir.path().to_path_buf(),
            TranscludeConfig::default(),
        );
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");

        let MdastNode::Root(root) = &tree else { panic!("expected Root") };
        // snippet.md has h1 + paragraph → 2 nodes spliced in
        assert_eq!(root.children.len(), 2, "expected 2 spliced nodes, got: {root:?}");
        assert!(matches!(root.children[0], MdastNode::Heading(_)));
        assert!(matches!(root.children[1], MdastNode::Paragraph(_)));
    }

    #[test]
    fn include_with_code_true_wraps_in_code_block() {
        let dir = TempDir::new("transclude_code").unwrap();
        let snippet_path = dir.path().join("foo.rs");
        std::fs::write(&snippet_path, "fn main() {}\n").unwrap();

        let input = r#":::include{file="./foo.rs" code=true lang="rust"}"#;
        let source = dir.path().join("input.md");
        std::fs::write(&source, input).unwrap();

        let (tree, diags) = run_transclude(
            input,
            source,
            dir.path().to_path_buf(),
            TranscludeConfig::default(),
        );
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let MdastNode::Root(root) = &tree else { panic!("expected Root") };
        assert_eq!(root.children.len(), 1);
        let MdastNode::Code(code) = &root.children[0] else { panic!("expected Code, got: {:?}", root.children[0]) };
        assert_eq!(code.lang.as_deref(), Some("rust"));
        assert!(code.value.contains("fn main()"));
    }

    #[test]
    fn include_with_lines_slices_correctly() {
        let dir = TempDir::new("transclude_lines").unwrap();
        let snippet_path = dir.path().join("nums.md");
        std::fs::write(&snippet_path, "line1\nline2\nline3\nline4\nline5\n").unwrap();

        let input = r#":::include{file="./nums.md" lines="2-4" code=true}"#;
        let source = dir.path().join("input.md");
        std::fs::write(&source, input).unwrap();

        let (tree, diags) = run_transclude(
            input,
            source,
            dir.path().to_path_buf(),
            TranscludeConfig::default(),
        );
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");
        let MdastNode::Root(root) = &tree else { panic!("expected Root") };
        let MdastNode::Code(code) = &root.children[0] else { panic!("expected Code") };
        assert_eq!(code.value, "line2\nline3\nline4");
    }

    #[test]
    fn cycle_detection_emits_error() {
        // A→A direct self-reference.
        let dir = TempDir::new("transclude_cycle").unwrap();
        let input = r#":::include{file="./cycle.md"}"#;
        // cycle.md includes itself
        let cycle_content = r#":::include{file="./cycle.md"}"#;
        std::fs::write(dir.path().join("cycle.md"), cycle_content).unwrap();

        let source = dir.path().join("cycle.md");
        let (_, diags) = run_transclude(
            input,
            source.clone(),
            dir.path().to_path_buf(),
            TranscludeConfig::default(),
        );
        let has_cycle_error = diags.iter().any(|d| {
            matches!(d, zfb_md_ast::diagnostics::MarkdownDiagnostic::Generic {
                severity: zfb_md_ast::diagnostics::DiagnosticSeverity::Error,
                message,
                ..
            } if message.contains("cycle"))
        });
        assert!(has_cycle_error, "expected cycle error diagnostic: {diags:?}");
    }

    #[test]
    fn max_depth_exceeded_emits_error() {
        // With maxDepth=1 a chain A→B→C is rejected at B→C level.
        let dir = TempDir::new("transclude_depth").unwrap();
        // c.md is a leaf (no includes)
        std::fs::write(dir.path().join("c.md"), "leaf\n").unwrap();
        // b.md includes c.md (depth 1 — allowed)
        std::fs::write(dir.path().join("b.md"), r#":::include{file="./c.md"}"#).unwrap();
        // a.md includes b.md (depth 0 — allowed), which includes c.md (depth 1 — allowed)
        let input = r#":::include{file="./b.md"}"#;
        let source = dir.path().join("a.md");
        std::fs::write(&source, input).unwrap();

        // maxDepth=1: b.md can include nothing (depth 1 >= maxDepth 1)
        let config = TranscludeConfig { max_depth: 1 };
        let (_, diags) = run_transclude(input, source, dir.path().to_path_buf(), config);
        let has_depth_error = diags.iter().any(|d| {
            matches!(d, zfb_md_ast::diagnostics::MarkdownDiagnostic::Generic {
                severity: zfb_md_ast::diagnostics::DiagnosticSeverity::Error,
                message,
                ..
            } if message.contains("max depth"))
        });
        assert!(has_depth_error, "expected depth error: {diags:?}");
    }

    #[test]
    fn absolute_path_rejected() {
        let dir = TempDir::new("transclude_abs").unwrap();
        let input = r#":::include{file="/etc/passwd"}"#;
        let source = dir.path().join("input.md");
        std::fs::write(&source, input).unwrap();

        let (_, diags) = run_transclude(input, source, dir.path().to_path_buf(), TranscludeConfig::default());
        let has_err = diags.iter().any(|d| {
            matches!(d, zfb_md_ast::diagnostics::MarkdownDiagnostic::Generic {
                severity: zfb_md_ast::diagnostics::DiagnosticSeverity::Error,
                message,
                ..
            } if message.contains("absolute"))
        });
        assert!(has_err, "expected absolute path error: {diags:?}");
    }

    #[test]
    fn path_outside_project_root_rejected() {
        let outer = TempDir::new("transclude_outer").unwrap();
        let inner = TempDir::new("transclude_inner").unwrap();
        // outer/secret.md exists but project_root is inner/
        std::fs::write(outer.path().join("secret.md"), "secret\n").unwrap();

        let input = format!(r#":::include{{file="{}"}}"#, outer.path().join("secret.md").display());
        let source = inner.path().join("input.md");
        std::fs::write(&source, &input).unwrap();

        // project_root = inner dir, but file resolves to outer dir (absolute → rejected first)
        let (_, diags) = run_transclude(&input, source, inner.path().to_path_buf(), TranscludeConfig::default());
        let has_err = diags.iter().any(|d| {
            matches!(d, zfb_md_ast::diagnostics::MarkdownDiagnostic::Generic {
                severity: zfb_md_ast::diagnostics::DiagnosticSeverity::Error,
                ..
            })
        });
        assert!(has_err, "expected path error: {diags:?}");
    }

    #[test]
    fn two_level_chain_inlines_both() {
        // A → B → (nothing), verify both levels resolve.
        let dir = TempDir::new("transclude_chain").unwrap();
        std::fs::write(dir.path().join("b.md"), "From B.\n").unwrap();
        std::fs::write(dir.path().join("a.md"), r#":::include{file="./b.md"}"#).unwrap();

        let input = r#":::include{file="./a.md"}"#;
        let source = dir.path().join("root.md");
        std::fs::write(&source, input).unwrap();

        let (tree, diags) = run_transclude(input, source, dir.path().to_path_buf(), TranscludeConfig::default());
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");
        let MdastNode::Root(root) = &tree else { panic!("expected Root") };
        // a.md inlines b.md which is "From B." → a paragraph
        assert!(!root.children.is_empty(), "expected inlined content");
    }
}
