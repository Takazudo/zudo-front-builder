//! Generic MDX directive registry.
//!
//! A runtime-extensible mapping from directive name (e.g. `card`, `badge`,
//! `note`) to a JSX component name (e.g. `Card`, `Badge`, `Note`). Core seeds
//! ZERO directive names — the entire `:::name` → `<Component>` vocabulary is
//! supplied by the user's `features.directives` config / docs recipes.
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
//! ## No built-in defaults
//!
//! [`DirectiveRegistry::new`] starts empty. Callers register exactly the
//! directives they want via [`DirectiveRegistry::register`]; there is no
//! preset of `note`/`tip`/… in core.

use std::collections::HashMap;

use markdown::mdast::{
    AttributeContent, AttributeValue, MdxJsxAttribute, MdxJsxFlowElement, MdxJsxTextElement,
    Node as MdastNode, Text,
};

use crate::pipeline::MdastVisitor;

// Directive definition types moved to `zfb-md-ast` so `zfb-md-extras`
// (which cannot depend on `zfb-content`) can produce `Vec<DirectiveDef>`
// for preset functions. Re-export them here so all existing
// `zfb_content::plugins::directives::*` import paths keep compiling.
pub use zfb_md_ast::{
    AttrSchema, AttrType, AttrValidationResult, DirectiveDef, DirectiveDiagnostic, DirectiveKind,
    ValidatedAttrValue,
};

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
                        if let Some(close_idx) =
                            (i + 1..children.len()).find(|j| is_container_close(&children[*j]))
                        {
                            let (line, column) = paragraph_line_col(&children[i]);
                            let validated_opt = self.run_validation(&def, &parsed, line, column);
                            let body: Vec<MdastNode> = children.drain(i..=close_idx).collect();
                            // Strip open + close paragraphs.
                            let inner = body[1..body.len() - 1].to_vec();
                            let jsx = build_flow_jsx(&def, &parsed, inner, validated_opt.as_ref());
                            children.insert(i, jsx);
                            i += 1;
                            continue;
                        }
                        // No separate closing `:::` paragraph exists. In
                        // real markdown, when the fences are NOT surrounded
                        // by blank lines, `markdown::to_mdast` collapses the
                        // opener, body, and closing `:::` into a SINGLE
                        // multi-line Paragraph (the opener is the first line
                        // of the first Text child and the `:::` closer is the
                        // last line of the last Text child). This is the
                        // common real-world shape (issue #1090) — detect it
                        // and transform the collapsed paragraph just like the
                        // blank-line-separated form, matching what
                        // `githubAlerts` does end-to-end.
                        if let Some(inner) = collapsed_container_body(&children[i]) {
                            let (line, column) = paragraph_line_col(&children[i]);
                            let validated_opt = self.run_validation(&def, &parsed, line, column);
                            let jsx = build_flow_jsx(&def, &parsed, inner, validated_opt.as_ref());
                            children[i] = jsx;
                            i += 1;
                            continue;
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
                        let (line, column) = paragraph_line_col(&children[i]);
                        let validated_opt = self.run_validation(&def, &parsed, line, column);
                        let jsx = build_leaf_jsx(&def, &parsed, position, validated_opt.as_ref());
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
                            // Inline text directives have no source position
                            // from the text scanner — pass None for both.
                            let validated_opt = self.run_validation(&def, &parsed, None, None);
                            out.push(build_text_jsx(&def, &parsed, validated_opt.as_ref()));
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

    /// Run `validate_attrs` on `def` against the raw attrs in `parsed`.
    /// Appends any resulting diagnostics (annotated with position) to
    /// `self.diagnostics`. Returns `Some(validated_map)` on success
    /// (or when the schema is empty), `None` on validation error.
    fn run_validation(
        &mut self,
        def: &DirectiveDef,
        parsed: &ParsedDirective,
        line: Option<usize>,
        column: Option<usize>,
    ) -> Option<HashMap<String, ValidatedAttrValue>> {
        let (result, warnings) = def.validate_attrs(&parsed.attrs);

        // Append warnings (unknown attrs) — always, regardless of Ok/Err.
        for mut w in warnings {
            w.line = line;
            w.column = column;
            self.diagnostics.push(w);
        }

        match result {
            Ok(validated) => {
                // Schema is empty or all attrs passed — return validated map.
                Some(validated)
            }
            Err(errors) => {
                // Hard errors: append with position and fall back to raw attrs.
                for mut e in errors {
                    e.line = line;
                    e.column = column;
                    self.diagnostics.push(e);
                }
                None
            }
        }
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

    Some((ParsedDirective { name, label, attrs }, i))
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
            let mut val = String::new();
            // Char-aware copy: slice `s` at byte index `i` so multibyte UTF-8
            // (CJK, accented Latin, emoji) stays intact. `bytes[i] as char`
            // would map each byte 0x80-0xFF to a lone code point and corrupt
            // non-ASCII attribute values.
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    // Support \" and \\ inside quoted attribute values.
                    let c = s[i + 1..].chars().next().unwrap();
                    val.push(c);
                    i += 1 + c.len_utf8();
                    continue;
                }
                let c = s[i..].chars().next().unwrap();
                val.push(c);
                i += c.len_utf8();
            }
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

/// Strip a leading directive-opener line (and the `\n`/`\r\n` that
/// follows it) from `value`. The opener line is the substring up to the
/// first newline. Returns the remaining body text.
fn strip_opener_line(value: &str) -> &str {
    match value.find('\n') {
        // Everything after the first `\n` is the body. Any trailing `\r`
        // (CRLF input) belonged to the opener line, so nothing extra to
        // strip here.
        Some(nl) => &value[nl + 1..],
        // No newline: the whole value was the opener — no body.
        None => "",
    }
}

/// Strip a trailing standalone `:::` line (and the `\n`/`\r\n` before it)
/// from `value`. Returns `Some(remaining)` when the last line trims to
/// exactly `:::`, else `None`.
fn strip_closer_line(value: &str) -> Option<&str> {
    let last_nl = value.rfind('\n');
    match last_nl {
        Some(nl) => {
            let last = &value[nl + 1..];
            if last.trim() == ":::" {
                // Drop the `\n` and a preceding `\r` (CRLF) too.
                let before = &value[..nl];
                Some(before.strip_suffix('\r').unwrap_or(before))
            } else {
                None
            }
        }
        None => {
            // Single line: it must itself be the closer.
            if value.trim() == ":::" {
                Some("")
            } else {
                None
            }
        }
    }
}

/// When `node` is a single Paragraph that collapsed a container directive
/// (`:::name[...]` … `:::` written WITHOUT surrounding blank lines, so the
/// markdown parser merged the opener, body, and closing `:::` into one
/// multi-line Paragraph — issue #1090), return the directive body wrapped
/// as JSX flow children: a `Vec` holding one body `Paragraph` (or empty
/// when the body is blank), matching the shape the blank-line-separated
/// form and `githubAlerts` produce. Returns `None` when `node` is not a
/// collapsed container.
///
/// Detection: the FIRST child is a Text whose first line parses as a
/// `:::name` opener, the value spans multiple lines, and the LAST child is
/// a Text whose last line is exactly `:::`.
fn collapsed_container_body(node: &MdastNode) -> Option<Vec<MdastNode>> {
    let MdastNode::Paragraph(p) = node else {
        return None;
    };
    // First child: Text whose first line is a `:::name` opener.
    let MdastNode::Text(first) = p.children.first()? else {
        return None;
    };
    // Must be multi-line (i.e. the parser merged the fences in).
    if !first.value.contains('\n') {
        return None;
    }
    let opener_line = first_line(&first.value).trim_end();
    parse_directive_line(opener_line, 3)?;
    // Last child must be a Text whose last line is exactly `:::`.
    let MdastNode::Text(last) = p.children.last()? else {
        return None;
    };
    let last_line = last.value.rsplit('\n').next().unwrap_or(&last.value);
    if last_line.trim() != ":::" {
        return None;
    }

    // Build the body inline nodes: clone the paragraph children, strip the
    // opener line from the first Text and the closing `:::` line from the
    // last Text. When first and last are the same node (single Text child),
    // strip both from that one node.
    let mut inline: Vec<MdastNode> = p.children.clone();
    let n = inline.len();
    if n == 1 {
        if let MdastNode::Text(t) = &mut inline[0] {
            let body = strip_opener_line(&t.value);
            let body = strip_closer_line(body).unwrap_or(body);
            t.value = body.to_string();
        }
    } else {
        if let MdastNode::Text(t) = &mut inline[0] {
            t.value = strip_opener_line(&t.value).to_string();
        }
        if let MdastNode::Text(t) = &mut inline[n - 1] {
            if let Some(stripped) = strip_closer_line(&t.value) {
                t.value = stripped.to_string();
            }
        }
    }

    // Drop leading/trailing empty Text nodes left after stripping so the
    // body paragraph has no stray empty text runs.
    if let Some(MdastNode::Text(t)) = inline.first() {
        if t.value.is_empty() {
            inline.remove(0);
        }
    }
    if let Some(MdastNode::Text(t)) = inline.last() {
        if t.value.is_empty() {
            inline.pop();
        }
    }

    if inline.is_empty() {
        // Body was blank (e.g. `:::note\n:::`) — no children.
        return Some(Vec::new());
    }

    // Wrap the remaining inline content in a Paragraph, matching the
    // blank-line-separated form where the body is its own Paragraph node.
    Some(vec![MdastNode::Paragraph(markdown::mdast::Paragraph {
        children: inline,
        position: None,
    })])
}

// -- JSX construction ---------------------------------------------------

/// Build a flow JSX element for a Container directive.
fn build_flow_jsx(
    def: &DirectiveDef,
    parsed: &ParsedDirective,
    children: Vec<MdastNode>,
    validated: Option<&HashMap<String, ValidatedAttrValue>>,
) -> MdastNode {
    let attributes = build_attributes(def, parsed, validated);
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
    validated: Option<&HashMap<String, ValidatedAttrValue>>,
) -> MdastNode {
    let attributes = build_attributes(def, parsed, validated);
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
fn build_text_jsx(
    def: &DirectiveDef,
    parsed: &ParsedDirective,
    validated: Option<&HashMap<String, ValidatedAttrValue>>,
) -> MdastNode {
    let attributes = build_attributes(def, parsed, validated);
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
///
/// When `validated` is `Some`, the validated map is used as the source of
/// truth for schema-declared attrs (with defaults applied). Unknown attrs
/// (not in the schema) still pass through from `parsed.attrs` verbatim,
/// as the unknown-attr policy is warning-only. When `validated` is `None`,
/// `parsed.attrs` is used directly (fallback on validation error or empty
/// schema).
fn build_attributes(
    def: &DirectiveDef,
    parsed: &ParsedDirective,
    validated: Option<&HashMap<String, ValidatedAttrValue>>,
) -> Vec<AttributeContent> {
    let mut out: Vec<AttributeContent> = Vec::new();
    if def.title_from_label {
        if let Some(label) = &parsed.label {
            out.push(AttributeContent::Property(MdxJsxAttribute {
                name: "title".to_string(),
                value: Some(AttributeValue::Literal(label.clone())),
            }));
        }
    }

    match validated {
        None => {
            // No schema or validation failed — emit raw parsed attrs.
            for (k, v) in &parsed.attrs {
                out.push(AttributeContent::Property(MdxJsxAttribute {
                    name: k.clone(),
                    value: Some(AttributeValue::Literal(v.clone())),
                }));
            }
        }
        Some(val_map) => {
            // Schema-declared attrs: emit from validated map (preserves
            // defaults and type-normalised values like "true"/"false").
            let schema_names: std::collections::HashSet<&str> =
                def.attrs.iter().map(|s| s.name.as_str()).collect();
            for schema in &def.attrs {
                if let Some(val) = val_map.get(&schema.name) {
                    out.push(AttributeContent::Property(MdxJsxAttribute {
                        name: schema.name.clone(),
                        value: Some(AttributeValue::Literal(val.as_str().to_string())),
                    }));
                }
            }
            // Unknown attrs pass through verbatim (warning-only policy).
            for (k, v) in &parsed.attrs {
                if !schema_names.contains(k.as_str()) {
                    out.push(AttributeContent::Property(MdxJsxAttribute {
                        name: k.clone(),
                        value: Some(AttributeValue::Literal(v.clone())),
                    }));
                }
            }
        }
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

    /// Test fixture: a registry pre-loaded with the seven admonition
    /// container names (with `title_from_label = true`, matching how a docs
    /// recipe would register them via `features.directives`). Core no longer
    /// ships a built-in preset, so the vocabulary is declared explicitly here
    /// for tests that exercise `:::note` / `:::details` style directives.
    fn registry_with_admonitions() -> DirectiveRegistry {
        let mut r = DirectiveRegistry::new();
        for (name, component) in [
            ("note", "Note"),
            ("tip", "Tip"),
            ("warning", "Warning"),
            ("danger", "Danger"),
            ("info", "Info"),
            ("details", "Details"),
            ("caution", "Caution"),
        ] {
            let mut def = DirectiveDef::container(name, component);
            def.title_from_label = true;
            r.register(def);
        }
        r
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

    // ---- real-parser container tests (Sub #1090) ----

    /// Parse `input` with the real markdown parser, run a registry that
    /// transforms it, and return the resulting top-level nodes. This
    /// exercises the SAME `markdown::to_mdast` → DirectiveRegistry path a
    /// real build uses — NOT hand-built mdast — so it reproduces the
    /// collapsed-paragraph shape that hand-built fixtures never hit.
    fn run_real_parser(reg: &mut DirectiveRegistry, input: &str) -> Vec<MdastNode> {
        let mut root = markdown::to_mdast(input, &markdown::ParseOptions::mdx())
            .expect("markdown-rs should parse the sample");
        reg.visit(&mut root);
        let MdastNode::Root(Root { children, .. }) = root else {
            unreachable!("to_mdast always yields a Root")
        };
        children
    }

    #[test]
    fn real_parser_transforms_container_directives_without_blank_lines() {
        // Issue #1090 / #1085 repro: the reporter's exact markdown with NO
        // blank lines around the fences. `markdown::to_mdast` collapses each
        // `:::name … :::` block into a single multi-line Paragraph; before
        // the fix the registry left them as literal `<p>:::note…</p>` text.
        //
        // This is the guard test: it MUST fail before the fix and pass
        // after. It covers both the bracket title form (`:::tip[Title]`) and
        // the space-after-name title form (`:::tip title="…"`) — the issue
        // calls out both. (The documented title semantics are bracket-label
        // → `title`; the space form carries the title as an explicit
        // `title="…"` attribute, exactly like the legacy `:::details
        // title="Click me"` form. There is no bare-word space-to-title
        // promotion in the engine, so this test does not assert one.)
        let mut r = registry_with_admonitions();
        let input = "\
:::note
plain note body
:::

:::tip[Bracket Title]
tip body
:::

:::tip title=\"Space Title\"
another tip
:::
";
        let out = run_real_parser(&mut r, input);
        // Three directive blocks → three JSX flow elements (no literal text).
        assert_eq!(
            out.len(),
            3,
            "expected 3 transformed directives, got {out:#?}"
        );

        // 1. :::note → <Note> with the plain body.
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        let MdastNode::Paragraph(body) = &note.children[0] else {
            unreachable!(
                "note body should be a Paragraph, got {:?}",
                note.children[0]
            );
        };
        let MdastNode::Text(t) = &body.children[0] else {
            unreachable!("note body text expected");
        };
        assert_eq!(t.value, "plain note body");

        // 2. :::tip[Bracket Title] → <Tip title="Bracket Title">.
        let tip_bracket = flow(&out[1]);
        assert_eq!(tip_bracket.name.as_deref(), Some("Tip"));
        assert_eq!(
            attr(tip_bracket, "title").as_deref(),
            Some("Bracket Title"),
            "bracket label promoted to title attr"
        );

        // 3. :::tip title="Space Title" → <Tip title="Space Title"> (space
        //    form carries the title as an explicit attribute).
        let tip_space = flow(&out[2]);
        assert_eq!(tip_space.name.as_deref(), Some("Tip"));
        assert_eq!(
            attr(tip_space, "title").as_deref(),
            Some("Space Title"),
            "space-form title attribute preserved"
        );

        // No diagnostics — every block is recognised and transformed.
        assert!(
            r.take_diagnostics().is_empty(),
            "no diagnostics expected for well-formed collapsed directives"
        );
    }

    #[test]
    fn real_parser_collapsed_container_bare_space_form_transforms() {
        // A bare space form (`:::tip heads up`) still TRANSFORMS to <Tip>
        // (no longer literal text) — the bug was that it stayed as a
        // `<p>:::tip…</p>` paragraph. The trailing words parse as boolean
        // attributes (engine semantics; not a title), but the key acceptance
        // is that the directive is recognised at all.
        let mut r = registry_with_admonitions();
        let out = run_real_parser(&mut r, ":::tip heads up\nbody\n:::\n");
        assert_eq!(out.len(), 1, "got {out:#?}");
        let tip = flow(&out[0]);
        assert_eq!(tip.name.as_deref(), Some("Tip"));
    }

    #[test]
    fn real_parser_blank_line_separated_form_still_transforms() {
        // The blank-line-separated form parses to SEPARATE paragraphs (the
        // shape the hand-built fixtures simulate). It must keep working
        // through the real parser too.
        let mut r = registry_with_admonitions();
        let input = "\
:::note

separated body

:::
";
        let out = run_real_parser(&mut r, input);
        assert_eq!(out.len(), 1, "got {out:#?}");
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        assert!(r.take_diagnostics().is_empty());
    }

    #[test]
    fn real_parser_unknown_collapsed_container_left_alone_and_warned() {
        // A collapsed `:::unknown` block whose name is NOT registered must
        // stay as literal text and earn an unknown-directive warning — the
        // pre-existing unknown-directive behaviour is preserved.
        let mut r = registry_with_admonitions();
        let input = ":::nope\nbody\n:::\n";
        let out = run_real_parser(&mut r, input);
        assert_eq!(out.len(), 1);
        assert!(
            matches!(out[0], MdastNode::Paragraph(_)),
            "unknown directive preserved as paragraph"
        );
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "unknown-directive warning expected");
        assert!(diags[0].message.contains("nope"));
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
        let out = run_with_registry(&mut r, vec![text_para("::badge[Label]{variant=success}")]);
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
            vec![text_para(":::nope"), text_para("body"), text_para(":::")],
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
        let mut r = registry_with_admonitions();
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
        let mut r = registry_with_admonitions();
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
        let mut r = registry_with_admonitions();
        let out = run_with_registry(&mut r, vec![text_para(":::note"), text_para("body")]);
        assert_eq!(out.len(), 2);
        // First is still a paragraph.
        assert!(matches!(out[0], MdastNode::Paragraph(_)));
    }

    #[test]
    fn nested_admonition_inside_blockquote() {
        let mut r = registry_with_admonitions();
        // Build a blockquote that contains the admonition paragraphs.
        let bq = MdastNode::Blockquote(markdown::mdast::Blockquote {
            children: vec![text_para(":::note"), text_para("inner"), text_para(":::")],
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
        let mut r = registry_with_admonitions();
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
    fn parse_unbraced_attrs_preserves_non_ascii_quoted_values() {
        // Regression: quoted values were copied byte-by-byte via `as char`,
        // corrupting multibyte UTF-8. CJK, accented Latin, and emoji must
        // round-trip intact, and \" / \\ escapes must still work.
        let attrs = parse_unbraced_attrs("t=\"日本語\" u=\"café\" e=\"🎉\" q=\"a\\\"b\"");
        assert_eq!(
            attrs,
            vec![
                ("t".to_string(), "日本語".to_string()),
                ("u".to_string(), "café".to_string()),
                ("e".to_string(), "🎉".to_string()),
                ("q".to_string(), "a\"b".to_string()),
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
            attrs: Vec::new(),
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
        let mut r = registry_with_admonitions();
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

    // ---- merged (collapsed) container transform (Sub #1090) ----

    #[test]
    fn merged_container_transforms_to_jsx() {
        // CHANGED for #1090 (was `merged_container_emits_blank_line_diagnostic`).
        //
        // Previously a merged `:::note\nbody\n:::` (written WITHOUT blank
        // lines around the fences — the shape `markdown::to_mdast` actually
        // produces for real input) was left untransformed and only earned a
        // "missing blank lines" diagnostic. That diverged from `githubAlerts`,
        // which transforms the same no-blank-line shape end-to-end, and meant
        // every container directive in a real build rendered as literal text.
        //
        // After the fix the collapsed single-Paragraph form TRANSFORMS, so
        // this test asserts the new behaviour (transform, no diagnostic)
        // instead of the old diagnostic. The assertion is updated, not
        // deleted: the merged form is now handled, not flagged.
        let mut r = registry_with_admonitions();
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
        // The collapsed paragraph is now rewritten to a <Note> flow element.
        assert_eq!(out.len(), 1);
        let j = flow(&out[0]);
        assert_eq!(j.name.as_deref(), Some("Note"));
        // The body becomes a single paragraph child.
        assert_eq!(j.children.len(), 1, "body wrapped in one paragraph");
        let MdastNode::Paragraph(body) = &j.children[0] else {
            unreachable!("expected body Paragraph, got {:?}", j.children[0]);
        };
        let MdastNode::Text(t) = &body.children[0] else {
            unreachable!("expected body Text, got {:?}", body.children[0]);
        };
        assert_eq!(t.value, "body text");
        // No diagnostic — the form is handled, not flagged.
        assert!(
            r.take_diagnostics().is_empty(),
            "merged container now transforms; no diagnostic expected"
        );
    }

    #[test]
    fn fenced_code_block_does_not_trip_blank_line_diagnostic() {
        // A MdastNode::Code (fenced code block) whose content contains
        // `:::` should not trigger the diagnostic — only Paragraph nodes
        // are inspected.
        let mut r = registry_with_admonitions();
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
        let mut r = registry_with_admonitions();
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

    // ---- directives-only (generic directives feature, zero defaults) tests ----

    // A registry built with only a user-supplied name registers ONLY that
    // name — core seeds no `note`/`tip`/… vocabulary.
    #[test]
    fn directives_only_no_defaults_registered() {
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef::container("spoiler", "Spoiler"));

        // `:::spoiler` is resolved.
        let out = run_with_registry(
            &mut r,
            vec![
                text_para(":::spoiler"),
                text_para("hidden"),
                text_para(":::"),
            ],
        );
        assert_eq!(out.len(), 1);
        assert_eq!(flow(&out[0]).name.as_deref(), Some("Spoiler"));

        // `:::note` is NOT registered — left untransformed (zero defaults).
        let out2 = run_with_registry(
            &mut r,
            vec![text_para(":::note"), text_para("body"), text_para(":::")],
        );
        // Should still be 3 paragraphs (no transformation).
        assert_eq!(out2.len(), 3, ":::note must stay as-is when not registered");
    }

    // Registering the same name twice is last-wins: the later component name
    // overrides the earlier one. This is the semantic the `features.directives`
    // map relies on when a user redefines a name.
    #[test]
    fn register_same_name_is_last_wins() {
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef::container("caution", "Caution"));
        // Redefine `caution` with a different component name.
        r.register(DirectiveDef::container("caution", "MyCaution"));

        let out = run_with_registry(
            &mut r,
            vec![
                text_para(":::caution"),
                text_para("be careful"),
                text_para(":::"),
            ],
        );
        assert_eq!(out.len(), 1);
        assert_eq!(
            flow(&out[0]).name.as_deref(),
            Some("MyCaution"),
            "later registration must override the earlier one for the same name"
        );
    }
}
