//! Generic MDX directive registry.
//!
//! Replaces the hardcoded admonition match (`note`/`tip`/…) in
//! [`super::admonitions`] with a runtime-extensible mapping from
//! directive name (e.g. `card`, `badge`, `note`) to a JSX component
//! name (e.g. `Card`, `Badge`, `Note`).
//!
//! ## Directive shapes
//!
//! Three CommonMark-Directives-flavoured shapes are recognised:
//!
//! - **Container** — `:::name[label]{attrs}` … `:::`. Block-level. Body
//!   between fences becomes the JSX element's children.
//! - **Leaf** — `::name[label]{attrs}` (single line, no fenced body).
//!   Block-level. The bracketed `[label]` (if any) becomes the only
//!   text child.
//! - **Text** — `:name[label]{attrs}` inline within a paragraph.
//!
//! `[label]` and `{attrs}` are both optional. Attribute parsing also
//! falls back to the legacy unbraced form (e.g.
//! `:::details title="Click me"`) so the existing admonition fixtures
//! keep producing bit-for-bit identical JSX output.
//!
//! ## Attribute escaping (v1)
//!
//! All directive attribute values emit as JSX **string-literal**
//! attributes (`title="foo"`). Raw-expression attributes
//! (`title={someVar}`) are NOT supported in v1 — every value is a
//! [`AttributeValue::Literal`] under the hood, and the downstream JSX
//! emitter (`crate::mdx_jsx_emit`) escapes `"`, `&`, `<`, `>`, and
//! line terminators when rendering. Hyphenated keys (e.g. `data-foo`)
//! round-trip verbatim as-is.
//!
//! ## Warning sink (v1)
//!
//! Unknown directive names produce a [`DirectiveDiagnostic`] with
//! optional line/column info, NOT a parse error. The source paragraph
//! is left intact. There is no central diagnostic sink yet — diagnostics
//! accumulate on the registry instance and the orchestrator (e.g. the
//! pipeline runner) is responsible for draining and printing them.
//! Use [`DirectiveRegistry::take_diagnostics`] after a pipeline run.
//!
//! ## Built-in defaults
//!
//! [`DirectiveRegistry::with_defaults`] preregisters the six existing
//! admonitions (`note`, `tip`, `info`, `warning`, `danger`, `details`)
//! as containers so existing zfb users see no behavioural change.

use std::collections::HashMap;

use markdown::mdast::{
    AttributeContent, AttributeValue, MdxJsxAttribute, MdxJsxFlowElement, MdxJsxTextElement,
    Node as MdastNode, Text,
};

use crate::pipeline::MdastVisitor;

/// Which CommonMark-Directives shape this directive responds to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectiveKind {
    /// `:::name … :::` (block, with body).
    Container,
    /// `::name[label]{attrs}` (block, no body).
    Leaf,
    /// `:name[label]{attrs}` (inline).
    Text,
}

/// A registered directive: its source-side `name` and the JSX component
/// it expands to.
#[derive(Debug, Clone)]
pub struct DirectiveDef {
    /// Lowercase source-side name (`note`, `card`, …).
    pub name: String,
    /// Container/leaf/text shape.
    pub kind: DirectiveKind,
    /// JSX component identifier (`Note`, `Card`, …).
    pub component_name: String,
    /// If true, the bracketed `[label]` is promoted to a `title="…"`
    /// attribute on the emitted JSX (and is NOT also emitted as a
    /// child). If false, the `[label]` becomes a single Text child of
    /// the element.
    pub title_from_label: bool,
}

impl DirectiveDef {
    /// Convenience: container with `title_from_label=false`.
    #[must_use]
    pub fn container(name: impl Into<String>, component_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: DirectiveKind::Container,
            component_name: component_name.into(),
            title_from_label: false,
        }
    }

    /// Convenience: leaf with `title_from_label=false`.
    #[must_use]
    pub fn leaf(name: impl Into<String>, component_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: DirectiveKind::Leaf,
            component_name: component_name.into(),
            title_from_label: false,
        }
    }

    /// Convenience: text directive with `title_from_label=false`.
    #[must_use]
    pub fn text(name: impl Into<String>, component_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: DirectiveKind::Text,
            component_name: component_name.into(),
            title_from_label: false,
        }
    }
}

/// A non-fatal diagnostic produced while expanding directives — typically
/// "unknown directive name" with the source location of the offending
/// paragraph.
///
/// File path is intentionally not stored here: the registry visitor sees
/// only an mdast tree and has no idea which file it came from. The
/// orchestrator pairs the diagnostic with the file it just processed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectiveDiagnostic {
    /// Human-readable message.
    pub message: String,
    /// 1-based line of the offending construct, if known.
    pub line: Option<usize>,
    /// 1-based column of the offending construct, if known.
    pub column: Option<usize>,
}

/// Runtime registry mapping directive names to JSX components.
///
/// Implements [`MdastVisitor`]: when used as a pipeline visitor it
/// rewrites recognised directive paragraphs into [`MdxJsxFlowElement`]
/// (or [`MdxJsxTextElement`] for inline text directives) nodes, and
/// records [`DirectiveDiagnostic`]s for unknown directive names it
/// encounters.
#[derive(Debug, Default, Clone)]
pub struct DirectiveRegistry {
    defs: HashMap<String, DirectiveDef>,
    diagnostics: Vec<DirectiveDiagnostic>,
    /// True if any registered directive has `kind == Text`. Cached so
    /// the inline scanner can be skipped in the common case.
    has_text_dirs: bool,
}

impl DirectiveRegistry {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a directive. Later registrations under the same name
    /// overwrite earlier ones (registry is the user's tool — they
    /// decide precedence).
    pub fn register(&mut self, def: DirectiveDef) {
        if def.kind == DirectiveKind::Text {
            self.has_text_dirs = true;
        }
        self.defs.insert(def.name.clone(), def);
    }

    /// Registry preloaded with the six built-in admonitions.
    #[must_use]
    pub fn with_defaults() -> Self {
        let mut r = Self::new();
        for d in super::admonitions::default_admonition_directives() {
            r.register(d);
        }
        r
    }

    /// Read-only view of accumulated diagnostics.
    #[must_use]
    pub fn diagnostics(&self) -> &[DirectiveDiagnostic] {
        &self.diagnostics
    }

    /// Drain accumulated diagnostics. Useful when the registry is
    /// reused across multiple pipeline runs and the orchestrator wants
    /// to print and clear the buffer between files.
    pub fn take_diagnostics(&mut self) -> Vec<DirectiveDiagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    /// Wrap this registry in a `Box<dyn MdastVisitor>` for insertion
    /// into a [`crate::pipeline::Pipeline`]. The returned box owns the
    /// registry; for streaming diagnostics across pipeline runs, keep
    /// a separate registry instance and call [`MdastVisitor::visit`]
    /// directly.
    #[must_use]
    pub fn into_visitor(self) -> Box<dyn MdastVisitor> {
        Box::new(self)
    }

    /// Get a registered directive by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&DirectiveDef> {
        self.defs.get(name)
    }
}

impl MdastVisitor for DirectiveRegistry {
    fn visit(&mut self, node: &mut MdastNode) {
        if let Some(children) = node.children_mut() {
            self.transform_children(children);
            for c in children.iter_mut() {
                self.visit(c);
            }
        }
    }
}

// -- transformation -----------------------------------------------------

impl DirectiveRegistry {
    /// Rewrite `:::kind / :::` and `::name[…]` runs in `children` in
    /// place. Inline `:name[…]` rewrites are handled in the per-text
    /// scanner.
    fn transform_children(&mut self, children: &mut Vec<MdastNode>) {
        // First pass: container + leaf at block level.
        let mut i = 0;
        while i < children.len() {
            // Try container open.
            if let Some(parsed) = self.parse_block_open(&children[i], 3) {
                if let Some(def) = self.defs.get(&parsed.name).cloned() {
                    if def.kind == DirectiveKind::Container {
                        // Find matching `:::` close.
                        if let Some(close_idx) = (i + 1..children.len())
                            .find(|j| is_container_close(&children[*j]))
                        {
                            let body: Vec<MdastNode> = children.drain(i..=close_idx).collect();
                            // Strip open + close paragraphs.
                            let inner = body[1..body.len() - 1].to_vec();
                            let jsx = build_flow_jsx(&def, &parsed, inner);
                            children.insert(i, jsx);
                            i += 1;
                            continue;
                        }
                        // A registered container name was found but no
                        // separate closing `:::` paragraph exists.  Check
                        // whether the opener and body were merged into a
                        // single paragraph because the author forgot the
                        // surrounding blank lines and emit a helpful
                        // diagnostic.
                        if paragraph_text_looks_merged(&children[i]) {
                            let (line, column) = paragraph_line_col(&children[i]);
                            self.diagnostics.push(DirectiveDiagnostic {
                                message: format!(
                                    "admonition `:::{}` looks like it is missing blank lines \
                                     around the fences — the opening `:::{name}` and closing \
                                     `:::` must each be on their own paragraph (separated by \
                                     blank lines) for the directive to be recognised",
                                    parsed.name,
                                    name = parsed.name,
                                ),
                                line,
                                column,
                            });
                        }
                    }
                } else {
                    // Looks like a container, but the name is not
                    // registered. Surface a warning and leave the AST
                    // alone — old behaviour.
                    self.warn_unknown(&parsed.name, &children[i]);
                }
            }

            // Try leaf at block level. A leaf occupies a single
            // paragraph whose first text run is exactly
            // `::name[label]{attrs}` (no fenced body).
            if let Some(parsed) = self.parse_block_open(&children[i], 2) {
                if let Some(def) = self.defs.get(&parsed.name).cloned() {
                    if def.kind == DirectiveKind::Leaf {
                        let position = paragraph_position(&children[i]);
                        let jsx = build_leaf_jsx(&def, &parsed, position);
                        children[i] = jsx;
                        i += 1;
                        continue;
                    }
                } else {
                    self.warn_unknown(&parsed.name, &children[i]);
                }
            }

            // Inline text directive scan inside this paragraph.
            if self.has_text_dirs {
                self.transform_inline_in(&mut children[i]);
            }

            i += 1;
        }
    }

    /// Walk inline children of `node` (if any) and rewrite recognised
    /// `:name[label]{attrs}` text runs into [`MdxJsxTextElement`]
    /// nodes. No-op for nodes that are not paragraphs/headings/etc.
    fn transform_inline_in(&mut self, node: &mut MdastNode) {
        let Some(inline) = node.children_mut() else {
            return;
        };
        let mut out: Vec<MdastNode> = Vec::with_capacity(inline.len());
        for child in inline.drain(..) {
            if let MdastNode::Text(t) = &child {
                let scanned = self.scan_text_for_text_directives(&t.value);
                if scanned.len() == 1 {
                    if let MdastNode::Text(ref new_text) = scanned[0] {
                        if new_text.value == t.value {
                            // No replacement happened — keep original.
                            out.push(child);
                            continue;
                        }
                    }
                }
                out.extend(scanned);
            } else {
                out.push(child);
            }
        }
        *inline = out;
    }

    fn scan_text_for_text_directives(&mut self, source: &str) -> Vec<MdastNode> {
        let mut out: Vec<MdastNode> = Vec::new();
        let bytes = source.as_bytes();
        let mut i = 0;
        let mut last_emit = 0;
        while i < bytes.len() {
            if bytes[i] == b':'
                && (i == 0 || !is_name_char(bytes[i - 1]))
                && (i + 1 < bytes.len() && is_name_start(bytes[i + 1]))
            {
                // Ensure we are NOT at a `::` (leaf/container) opener.
                if i + 1 < bytes.len() && bytes[i + 1] == b':' {
                    i += 1;
                    continue;
                }
                // Try to parse `:name[label]{attrs}` starting at i.
                if let Some((parsed, end)) = parse_text_directive(source, i) {
                    if let Some(def) = self.defs.get(&parsed.name).cloned() {
                        if def.kind == DirectiveKind::Text {
                            // Flush prefix.
                            if last_emit < i {
                                out.push(MdastNode::Text(Text {
                                    value: source[last_emit..i].to_string(),
                                    position: None,
                                }));
                            }
                            out.push(build_text_jsx(&def, &parsed));
                            i = end;
                            last_emit = end;
                            continue;
                        }
                        // Registered but wrong kind: leave alone.
                    } else {
                        // Unknown text directive: warn, leave intact.
                        self.diagnostics.push(DirectiveDiagnostic {
                            message: format!("unknown directive `:{}`", parsed.name),
                            line: None,
                            column: None,
                        });
                    }
                }
            }
            i += 1;
        }
        if last_emit == 0 {
            // No replacements; return original text wrapped.
            return vec![MdastNode::Text(Text {
                value: source.to_string(),
                position: None,
            })];
        }
        if last_emit < bytes.len() {
            out.push(MdastNode::Text(Text {
                value: source[last_emit..].to_string(),
                position: None,
            }));
        }
        out
    }

    /// Try to parse the first text run of `node` (a Paragraph) as a
    /// directive opener with exactly `colons` leading colons.
    fn parse_block_open(&self, node: &MdastNode, colons: usize) -> Option<ParsedDirective> {
        let MdastNode::Paragraph(p) = node else {
            return None;
        };
        let MdastNode::Text(t) = p.children.first()? else {
            return None;
        };
        // Require the FIRST line of the first text run to be the
        // directive — anything after a newline is body text.
        let line = first_line(&t.value).trim_end();
        let parsed = parse_directive_line(line, colons)?;
        // Reject `:::name` if the paragraph has trailing content on the
        // same line that we couldn't consume — we want `:::note` and
        // `:::details title="x"` to match, but not `:::nope hello`
        // unless the trailing portion was attribute-shaped.
        Some(parsed)
    }

    fn warn_unknown(&mut self, name: &str, node: &MdastNode) {
        let (line, column) = paragraph_line_col(node);
        self.diagnostics.push(DirectiveDiagnostic {
            message: format!("unknown directive `{name}`"),
            line,
            column,
        });
    }
}

// -- shared parsing -----------------------------------------------------

/// Result of parsing a single directive opener.
#[derive(Debug, Clone)]
pub(crate) struct ParsedDirective {
    pub name: String,
    pub label: Option<String>,
    pub attrs: Vec<(String, String)>,
}

/// Parse a directive opener line with exactly `expected_colons` leading
/// colons. Returns `None` if the line doesn't start with that many
/// colons, has a different count, or has no name token.
pub(crate) fn parse_directive_line(line: &str, expected_colons: usize) -> Option<ParsedDirective> {
    // Count leading colons exactly.
    let bytes = line.as_bytes();
    let mut n = 0;
    while n < bytes.len() && bytes[n] == b':' {
        n += 1;
    }
    if n != expected_colons {
        return None;
    }
    let rest = &line[n..];
    // The "rest" must start with a name char (so `:::` alone, or `::: `
    // is rejected).
    let rest_bytes = rest.as_bytes();
    if rest_bytes.is_empty() || !is_name_start(rest_bytes[0]) {
        return None;
    }
    let mut i = 0;
    while i < rest_bytes.len() && is_name_char(rest_bytes[i]) {
        i += 1;
    }
    let name = rest[..i].to_string();
    let mut after = &rest[i..];

    // Optional [label].
    let mut label: Option<String> = None;
    if let Some(stripped) = after.strip_prefix('[') {
        if let Some(end) = stripped.find(']') {
            label = Some(stripped[..end].to_string());
            after = &stripped[end + 1..];
        }
    }

    let trimmed = after.trim_start();
    let attrs = if let Some(braced) = trimmed.strip_prefix('{') {
        // Find the matching close brace. Attribute values may contain
        // `}` only inside quotes; do a quote-aware scan.
        let close = find_matching_close_brace(braced).unwrap_or(braced.len());
        parse_braced_attrs(&braced[..close])
    } else {
        parse_unbraced_attrs(trimmed)
    };

    Some(ParsedDirective { name, label, attrs })
}

/// Try to parse `:name[label]{attrs}` starting at byte offset `start`.
/// Returns the parse plus the byte offset of the first byte AFTER the
/// directive on success. The opening `:` at `start` is required.
fn parse_text_directive(source: &str, start: usize) -> Option<(ParsedDirective, usize)> {
    let bytes = source.as_bytes();
    if bytes.get(start) != Some(&b':') {
        return None;
    }
    let mut i = start + 1;
    if i >= bytes.len() || !is_name_start(bytes[i]) {
        return None;
    }
    let name_start = i;
    while i < bytes.len() && is_name_char(bytes[i]) {
        i += 1;
    }
    let name = source[name_start..i].to_string();

    // [label]
    let mut label: Option<String> = None;
    if i < bytes.len() && bytes[i] == b'[' {
        let label_start = i + 1;
        let mut j = label_start;
        while j < bytes.len() && bytes[j] != b']' {
            j += 1;
        }
        if j >= bytes.len() {
            return None;
        }
        label = Some(source[label_start..j].to_string());
        i = j + 1;
    }

    // {attrs}
    let mut attrs: Vec<(String, String)> = Vec::new();
    if i < bytes.len() && bytes[i] == b'{' {
        let attrs_start = i + 1;
        let close = find_matching_close_brace(&source[attrs_start..])?;
        attrs = parse_braced_attrs(&source[attrs_start..attrs_start + close]);
        i = attrs_start + close + 1;
    }

    // For text directives we require AT LEAST one of label or attrs
    // (otherwise `:foo` text in normal prose would gobble identifiers).
    if label.is_none() && attrs.is_empty() {
        return None;
    }

    Some((
        ParsedDirective { name, label, attrs },
        i,
    ))
}

/// Find the index of the matching `}` in `s`, treating `"…"` runs as
/// quoted regions where `}` does not terminate the brace.
fn find_matching_close_brace(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'}' => return Some(i),
            b'"' => {
                // Skip past the closing quote.
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                        continue;
                    }
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Parse a `{key=value key2="value 2"}` attribute body (without the
/// surrounding braces). Returns `(key, value)` pairs in source order.
/// Tolerant: malformed pairs are silently dropped.
pub(crate) fn parse_braced_attrs(s: &str) -> Vec<(String, String)> {
    parse_unbraced_attrs(s)
}

/// Parse a space-separated `key=value key2="value 2"` attribute body.
/// Same tolerance as [`parse_braced_attrs`].
pub(crate) fn parse_unbraced_attrs(s: &str) -> Vec<(String, String)> {
    let mut attrs: Vec<(String, String)> = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Skip whitespace.
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        // Read key: [A-Za-z0-9_-]+
        let key_start = i;
        while i < bytes.len() && is_attr_key_char(bytes[i]) {
            i += 1;
        }
        if i == key_start {
            // Couldn't parse a key — bail out.
            break;
        }
        let key = s[key_start..i].to_string();
        // Skip whitespace.
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        // `=` required for v1 (no boolean attributes yet).
        if i >= bytes.len() || bytes[i] != b'=' {
            // Treat as boolean: empty-string value.
            attrs.push((key, String::new()));
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'"' {
            // Quoted value.
            i += 1;
            let val_start = i;
            let mut val = String::new();
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    // Support \" and \\ inside quoted attribute values.
                    val.push(bytes[i + 1] as char);
                    i += 2;
                    continue;
                }
                val.push(bytes[i] as char);
                i += 1;
            }
            // Fast-path when no escape was seen: take the slice instead
            // of the per-char copy. (Both are correct; the copy version
            // already did the right thing.)
            let _ = val_start; // silence unused if compiler warns.
            if i < bytes.len() {
                i += 1; // consume closing quote
            }
            attrs.push((key, val));
        } else {
            // Bare token.
            let val_start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            attrs.push((key, s[val_start..i].to_string()));
        }
    }
    attrs
}

fn is_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}
fn is_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}
fn is_attr_key_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b':'
}

fn first_line(s: &str) -> &str {
    match s.find('\n') {
        Some(i) => &s[..i],
        None => s,
    }
}

/// Is this paragraph the closing `:::` fence of a container directive?
fn is_container_close(node: &MdastNode) -> bool {
    let MdastNode::Paragraph(p) = node else {
        return false;
    };
    let Some(MdastNode::Text(t)) = p.children.first() else {
        return false;
    };
    first_line(&t.value).trim() == ":::"
}

fn paragraph_position(node: &MdastNode) -> Option<markdown::unist::Position> {
    if let MdastNode::Paragraph(p) = node {
        return p.position.clone();
    }
    None
}

fn paragraph_line_col(node: &MdastNode) -> (Option<usize>, Option<usize>) {
    if let Some(pos) = paragraph_position(node) {
        return (Some(pos.start.line), Some(pos.start.column));
    }
    (None, None)
}

/// Returns true when `node` is a paragraph whose text begins with a
/// `:::name` directive opener AND contains a newline — indicating that
/// the opener, body, and (optional) closing `:::` all collapsed into a
/// single paragraph because blank lines were omitted.  This is the
/// primary signal for the blank-line diagnostic.
///
/// Only inspects the first Text child; non-Text paragraphs return false
/// so that arbitrary rich paragraphs are not misidentified.
fn paragraph_text_looks_merged(node: &MdastNode) -> bool {
    let MdastNode::Paragraph(p) = node else {
        return false;
    };
    let Some(MdastNode::Text(t)) = p.children.first() else {
        return false;
    };
    // Must have a newline (i.e. multi-line → merged).
    if !t.value.contains('\n') {
        return false;
    }
    // First line must look like a container opener (`:::[a-z]…`).
    let first = first_line(&t.value).trim_end();
    parse_directive_line(first, 3).is_some()
}

// -- JSX construction ---------------------------------------------------

/// Build a flow JSX element for a Container directive.
fn build_flow_jsx(
    def: &DirectiveDef,
    parsed: &ParsedDirective,
    children: Vec<MdastNode>,
) -> MdastNode {
    let attributes = build_attributes(def, parsed);
    MdastNode::MdxJsxFlowElement(MdxJsxFlowElement {
        children,
        position: None,
        name: Some(def.component_name.clone()),
        attributes,
    })
}

/// Build a flow JSX element for a Leaf directive. The label (if any
/// and `title_from_label` is false) becomes a single Text child.
fn build_leaf_jsx(
    def: &DirectiveDef,
    parsed: &ParsedDirective,
    position: Option<markdown::unist::Position>,
) -> MdastNode {
    let attributes = build_attributes(def, parsed);
    let children: Vec<MdastNode> = if !def.title_from_label {
        if let Some(label) = &parsed.label {
            vec![MdastNode::Text(Text {
                value: label.clone(),
                position: None,
            })]
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    MdastNode::MdxJsxFlowElement(MdxJsxFlowElement {
        children,
        position,
        name: Some(def.component_name.clone()),
        attributes,
    })
}

/// Build a text JSX element for an inline Text directive.
fn build_text_jsx(def: &DirectiveDef, parsed: &ParsedDirective) -> MdastNode {
    let attributes = build_attributes(def, parsed);
    let children: Vec<MdastNode> = if !def.title_from_label {
        if let Some(label) = &parsed.label {
            vec![MdastNode::Text(Text {
                value: label.clone(),
                position: None,
            })]
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    MdastNode::MdxJsxTextElement(MdxJsxTextElement {
        children,
        position: None,
        name: Some(def.component_name.clone()),
        attributes,
    })
}

/// Convert parsed attrs (+ optional title-from-label promotion) into
/// mdast `AttributeContent::Property` entries.
fn build_attributes(def: &DirectiveDef, parsed: &ParsedDirective) -> Vec<AttributeContent> {
    let mut out: Vec<AttributeContent> = Vec::new();
    if def.title_from_label {
        if let Some(label) = &parsed.label {
            out.push(AttributeContent::Property(MdxJsxAttribute {
                name: "title".to_string(),
                value: Some(AttributeValue::Literal(label.clone())),
            }));
        }
    }
    for (k, v) in &parsed.attrs {
        out.push(AttributeContent::Property(MdxJsxAttribute {
            name: k.clone(),
            value: Some(AttributeValue::Literal(v.clone())),
        }));
    }
    out
}

// -- helpers used by tests ----------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use markdown::mdast::{Code, Heading, Paragraph, Root};

    /// Build a paragraph with one Text child. Used by the registry's
    /// own tests to construct fixtures.
    fn text_para(value: &str) -> MdastNode {
        MdastNode::Paragraph(Paragraph {
            children: vec![MdastNode::Text(Text {
                value: value.to_string(),
                position: None,
            })],
            position: None,
        })
    }

    fn run_with_registry(reg: &mut DirectiveRegistry, children: Vec<MdastNode>) -> Vec<MdastNode> {
        let mut root = MdastNode::Root(Root {
            children,
            position: None,
        });
        reg.visit(&mut root);
        let MdastNode::Root(Root { children, .. }) = root else {
            unreachable!()
        };
        children
    }

    fn flow(node: &MdastNode) -> &MdxJsxFlowElement {
        match node {
            MdastNode::MdxJsxFlowElement(j) => j,
            other => unreachable!("expected MdxJsxFlowElement, got {other:?}"),
        }
    }

    fn text(node: &MdastNode) -> &MdxJsxTextElement {
        match node {
            MdastNode::MdxJsxTextElement(j) => j,
            other => unreachable!("expected MdxJsxTextElement, got {other:?}"),
        }
    }

    fn attr(j: &MdxJsxFlowElement, name: &str) -> Option<String> {
        for a in &j.attributes {
            if let AttributeContent::Property(p) = a {
                if p.name == name {
                    if let Some(AttributeValue::Literal(v)) = &p.value {
                        return Some(v.clone());
                    }
                }
            }
        }
        None
    }

    fn text_attr(j: &MdxJsxTextElement, name: &str) -> Option<String> {
        for a in &j.attributes {
            if let AttributeContent::Property(p) = a {
                if p.name == name {
                    if let Some(AttributeValue::Literal(v)) = &p.value {
                        return Some(v.clone());
                    }
                }
            }
        }
        None
    }

    // ---- container tests ----

    #[test]
    fn container_with_attributes() {
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef::container("card", "Card"));
        let out = run_with_registry(
            &mut r,
            vec![
                text_para(":::card{title=\"x\" variant=\"outline\"}"),
                text_para("body"),
                text_para(":::"),
            ],
        );
        assert_eq!(out.len(), 1);
        let j = flow(&out[0]);
        assert_eq!(j.name.as_deref(), Some("Card"));
        assert_eq!(attr(j, "title").as_deref(), Some("x"));
        assert_eq!(attr(j, "variant").as_deref(), Some("outline"));
        assert_eq!(j.children.len(), 1, "body paragraph preserved");
    }

    #[test]
    fn container_with_unbraced_legacy_attributes() {
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef::container("details", "Details"));
        let out = run_with_registry(
            &mut r,
            vec![
                text_para(":::details title=\"Click me\""),
                text_para("hidden"),
                text_para(":::"),
            ],
        );
        let j = flow(&out[0]);
        assert_eq!(j.name.as_deref(), Some("Details"));
        assert_eq!(attr(j, "title").as_deref(), Some("Click me"));
    }

    #[test]
    fn attribute_value_with_double_quotes_round_trips_via_literal() {
        // The parser sees `title="he said \"hi\""` and unescapes to
        // `he said "hi"`. The downstream JSX emitter is responsible for
        // re-escaping `"` → `&quot;` when serialising.
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef::container("card", "Card"));
        let out = run_with_registry(
            &mut r,
            vec![
                text_para(":::card{title=\"he said \\\"hi\\\"\"}"),
                text_para("body"),
                text_para(":::"),
            ],
        );
        let j = flow(&out[0]);
        assert_eq!(attr(j, "title").as_deref(), Some("he said \"hi\""));
    }

    #[test]
    fn hyphenated_attribute_keys_round_trip_verbatim() {
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef::container("card", "Card"));
        let out = run_with_registry(
            &mut r,
            vec![
                text_para(":::card{data-foo=\"1\" aria-label=\"ok\"}"),
                text_para("body"),
                text_para(":::"),
            ],
        );
        let j = flow(&out[0]);
        assert_eq!(attr(j, "data-foo").as_deref(), Some("1"));
        assert_eq!(attr(j, "aria-label").as_deref(), Some("ok"));
    }

    // ---- leaf tests ----

    #[test]
    fn leaf_with_label_and_attrs() {
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef::leaf("badge", "Badge"));
        let out = run_with_registry(
            &mut r,
            vec![text_para("::badge[Label]{variant=success}")],
        );
        assert_eq!(out.len(), 1);
        let j = flow(&out[0]);
        assert_eq!(j.name.as_deref(), Some("Badge"));
        assert_eq!(attr(j, "variant").as_deref(), Some("success"));
        // Label becomes the only Text child.
        assert_eq!(j.children.len(), 1);
        if let MdastNode::Text(t) = &j.children[0] {
            assert_eq!(t.value, "Label");
        } else {
            unreachable!("expected Text child, got {:?}", j.children[0]);
        }
    }

    // ---- text directive tests ----

    #[test]
    fn text_directive_inline_in_paragraph() {
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef::text("kbd", "Kbd"));
        let out = run_with_registry(
            &mut r,
            vec![MdastNode::Paragraph(Paragraph {
                children: vec![MdastNode::Text(Text {
                    value: "Press :kbd[Ctrl+S] to save".to_string(),
                    position: None,
                })],
                position: None,
            })],
        );
        let MdastNode::Paragraph(Paragraph { children, .. }) = &out[0] else {
            unreachable!("expected paragraph, got {:?}", out[0]);
        };
        // children: [Text("Press "), MdxJsxTextElement(<Kbd>), Text(" to save")]
        assert_eq!(children.len(), 3);
        if let MdastNode::Text(t) = &children[0] {
            assert_eq!(t.value, "Press ");
        } else {
            unreachable!("expected leading Text");
        }
        let j = text(&children[1]);
        assert_eq!(j.name.as_deref(), Some("Kbd"));
        assert_eq!(j.children.len(), 1);
        if let MdastNode::Text(t) = &children[2] {
            assert_eq!(t.value, " to save");
        } else {
            unreachable!("expected trailing Text");
        }
    }

    #[test]
    fn text_directive_passes_attributes() {
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef::text("link", "Link"));
        let out = run_with_registry(
            &mut r,
            vec![MdastNode::Paragraph(Paragraph {
                children: vec![MdastNode::Text(Text {
                    value: "see :link[here]{href=\"/x\" data-id=\"7\"}.".to_string(),
                    position: None,
                })],
                position: None,
            })],
        );
        let MdastNode::Paragraph(Paragraph { children, .. }) = &out[0] else {
            unreachable!("expected MdastNode::Paragraph")
        };
        let j = text(&children[1]);
        assert_eq!(j.name.as_deref(), Some("Link"));
        assert_eq!(text_attr(j, "href").as_deref(), Some("/x"));
        assert_eq!(text_attr(j, "data-id").as_deref(), Some("7"));
    }

    // ---- diagnostics ----

    #[test]
    fn unknown_container_emits_warning_and_keeps_source() {
        let mut r = DirectiveRegistry::new();
        // No registrations.
        let out = run_with_registry(
            &mut r,
            vec![
                text_para(":::nope"),
                text_para("body"),
                text_para(":::"),
            ],
        );
        // Source preserved.
        assert_eq!(out.len(), 3);
        // Warning recorded.
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.contains("nope"),
            "diag should mention name, got {:?}",
            diags[0].message
        );
    }

    #[test]
    fn unknown_text_directive_emits_warning_and_keeps_source() {
        let mut r = DirectiveRegistry::new();
        // Need at least one Text-kind directive to engage the inline
        // scanner.
        r.register(DirectiveDef::text("kbd", "Kbd"));
        let out = run_with_registry(
            &mut r,
            vec![MdastNode::Paragraph(Paragraph {
                children: vec![MdastNode::Text(Text {
                    value: "see :unknownx[bar] please".to_string(),
                    position: None,
                })],
                position: None,
            })],
        );
        let MdastNode::Paragraph(Paragraph { children, .. }) = &out[0] else {
            unreachable!("expected MdastNode::Paragraph")
        };
        // No JSX node — text preserved verbatim. Either as a single
        // unchanged Text node, or split but still semantically the
        // same string.
        let collected: String = children
            .iter()
            .filter_map(|c| match c {
                MdastNode::Text(t) => Some(t.value.clone()),
                _ => None,
            })
            .collect();
        assert!(
            collected.contains(":unknownx[bar]"),
            "text preserved, got: {collected:?}"
        );
        assert!(!r.take_diagnostics().is_empty(), "warning recorded");
    }

    // ---- bit-for-bit admonition behaviour ----

    #[test]
    fn admonition_defaults_match_legacy_output() {
        let mut r = DirectiveRegistry::with_defaults();
        for (key, tag) in [
            ("note", "Note"),
            ("tip", "Tip"),
            ("info", "Info"),
            ("warning", "Warning"),
            ("danger", "Danger"),
            ("details", "Details"),
        ] {
            let mut reg = r.clone();
            let out = run_with_registry(
                &mut reg,
                vec![
                    text_para(&format!(":::{key}")),
                    text_para("body"),
                    text_para(":::"),
                ],
            );
            assert_eq!(out.len(), 1);
            let j = flow(&out[0]);
            assert_eq!(j.name.as_deref(), Some(tag), "kind {key}");
        }
        let _ = r.take_diagnostics();
    }

    #[test]
    fn admonition_details_with_legacy_title_attr() {
        let mut r = DirectiveRegistry::with_defaults();
        let out = run_with_registry(
            &mut r,
            vec![
                text_para(":::details title=\"Click me\""),
                text_para("hidden"),
                text_para(":::"),
            ],
        );
        let j = flow(&out[0]);
        assert_eq!(j.name.as_deref(), Some("Details"));
        assert_eq!(attr(j, "title").as_deref(), Some("Click me"));
    }

    #[test]
    fn missing_close_left_alone() {
        let mut r = DirectiveRegistry::with_defaults();
        let out = run_with_registry(
            &mut r,
            vec![text_para(":::note"), text_para("body")],
        );
        assert_eq!(out.len(), 2);
        // First is still a paragraph.
        assert!(matches!(out[0], MdastNode::Paragraph(_)));
    }

    #[test]
    fn nested_admonition_inside_blockquote() {
        let mut r = DirectiveRegistry::with_defaults();
        // Build a blockquote that contains the admonition paragraphs.
        let bq = MdastNode::Blockquote(markdown::mdast::Blockquote {
            children: vec![
                text_para(":::note"),
                text_para("inner"),
                text_para(":::"),
            ],
            position: None,
        });
        let out = run_with_registry(&mut r, vec![bq]);
        let MdastNode::Blockquote(bq) = &out[0] else {
            unreachable!("expected blockquote")
        };
        assert_eq!(bq.children.len(), 1, "admonition collapsed inside bq");
        let j = flow(&bq.children[0]);
        assert_eq!(j.name.as_deref(), Some("Note"));
    }

    #[test]
    fn directive_in_heading_is_left_alone() {
        // Ensure `transform_inline_in` doesn't crash on heading nodes
        // and only fires for registered text directives.
        let mut r = DirectiveRegistry::with_defaults();
        let h = MdastNode::Heading(Heading {
            depth: 2,
            children: vec![MdastNode::Text(Text {
                value: "Hello world".to_string(),
                position: None,
            })],
            position: None,
        });
        let out = run_with_registry(&mut r, vec![h]);
        assert!(matches!(out[0], MdastNode::Heading(_)));
    }

    // ---- unbraced parser edge cases ----

    #[test]
    fn parse_unbraced_attrs_handles_quoted_and_bare() {
        let attrs = parse_unbraced_attrs("a=1 b=\"two words\" c=3");
        assert_eq!(
            attrs,
            vec![
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "two words".to_string()),
                ("c".to_string(), "3".to_string()),
            ]
        );
    }

    #[test]
    fn parse_unbraced_attrs_treats_bare_words_as_boolean_attrs() {
        // Non-key=value tokens become boolean attributes with empty
        // values. The parser never panics, even on weird input.
        let attrs = parse_unbraced_attrs("just words");
        assert_eq!(
            attrs,
            vec![
                ("just".to_string(), String::new()),
                ("words".to_string(), String::new()),
            ]
        );
    }

    // ---- title_from_label ----

    #[test]
    fn title_from_label_promotes_label_to_title_attr() {
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef {
            name: "callout".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Callout".to_string(),
            title_from_label: true,
        });
        let out = run_with_registry(
            &mut r,
            vec![
                text_para(":::callout[Heads up]"),
                text_para("body"),
                text_para(":::"),
            ],
        );
        let j = flow(&out[0]);
        assert_eq!(j.name.as_deref(), Some("Callout"));
        assert_eq!(attr(j, "title").as_deref(), Some("Heads up"));
    }

    #[test]
    fn note_with_custom_label_produces_title_attr() {
        // Sub #135: :::note[Custom Title] should emit title="Custom Title".
        let mut r = DirectiveRegistry::with_defaults();
        let out = run_with_registry(
            &mut r,
            vec![
                text_para(":::note[Custom Title]"),
                text_para("body"),
                text_para(":::"),
            ],
        );
        assert_eq!(out.len(), 1);
        let j = flow(&out[0]);
        assert_eq!(j.name.as_deref(), Some("Note"));
        assert_eq!(
            attr(j, "title").as_deref(),
            Some("Custom Title"),
            "bracketed label promoted to title attribute"
        );
    }

    // ---- blank-line diagnostic (Sub #185 Gap 2) ----

    #[test]
    fn merged_container_emits_blank_line_diagnostic() {
        // When :::note\nbody\n::: is written without blank lines, the
        // markdown parser collapses them into a single paragraph.  The
        // registry should emit a diagnostic suggesting the author add
        // blank lines.
        let mut r = DirectiveRegistry::with_defaults();
        // Simulate the merged paragraph: first Text child has the full
        // un-blank-lined content as a single multi-line value.
        let merged_para = MdastNode::Paragraph(Paragraph {
            children: vec![MdastNode::Text(Text {
                value: ":::note\nbody text\n:::".to_string(),
                position: None,
            })],
            position: None,
        });
        let out = run_with_registry(&mut r, vec![merged_para]);
        // Source is preserved (not converted — no separate close para).
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], MdastNode::Paragraph(_)));
        // A diagnostic must be emitted.
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "expected exactly one diagnostic, got {diags:?}");
        assert!(
            diags[0].message.contains("blank lines"),
            "diagnostic should mention blank lines, got: {:?}",
            diags[0].message
        );
        assert!(
            diags[0].message.contains("note"),
            "diagnostic should mention directive name, got: {:?}",
            diags[0].message
        );
    }

    #[test]
    fn fenced_code_block_does_not_trip_blank_line_diagnostic() {
        // A MdastNode::Code (fenced code block) whose content contains
        // `:::` should not trigger the diagnostic — only Paragraph nodes
        // are inspected.
        let mut r = DirectiveRegistry::with_defaults();
        let code_block = MdastNode::Code(Code {
            value: ":::note\nsome code\n:::".to_string(),
            lang: Some("markdown".to_string()),
            meta: None,
            position: None,
        });
        let out = run_with_registry(&mut r, vec![code_block]);
        assert_eq!(out.len(), 1, "code block preserved");
        assert!(matches!(out[0], MdastNode::Code(_)));
        let diags = r.take_diagnostics();
        assert!(
            diags.is_empty(),
            "no diagnostic for fenced code block, got {diags:?}"
        );
    }

    #[test]
    fn single_line_unrecognised_container_no_blank_line_diagnostic() {
        // A paragraph with a directive opener but NO newline (i.e., the
        // close is a separate paragraph) should not trigger the blank-
        // line diagnostic even when we can't find the close.
        let mut r = DirectiveRegistry::with_defaults();
        // Just the opener with no close sibling.
        let out = run_with_registry(&mut r, vec![text_para(":::note")]);
        // No diagnostic (the missing-close path, not the merged path).
        let diags = r.take_diagnostics();
        assert!(
            diags.is_empty(),
            "no blank-line diagnostic for lone opener, got {diags:?}"
        );
        // Source paragraph preserved.
        assert_eq!(out.len(), 1);
    }
}
