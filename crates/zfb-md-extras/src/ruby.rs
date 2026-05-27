//! Ruby annotation support (`{base|ruby}` syntax).
//!
//! Rust port of [`@lowlighter/remark-ruby`](https://www.npmjs.com/package/@lowlighter/remark-ruby).
//! Scans the mdast tree for `{base|ruby}` patterns and rewrites them into
//! `<ruby><rb>base</rb><rt>ruby</rt></ruby>` HTML elements.
//!
//! # Syntax
//!
//! ```text
//! {漢字|かんじ}
//! ```
//!
//! Produces:
//!
//! ```html
//! <ruby><rb>漢字</rb><rt>かんじ</rt></ruby>
//! ```
//!
//! Multiple annotations in one paragraph are supported:
//!
//! ```text
//! {日本語|にほんご}を{学ぶ|まなぶ}
//! ```
//!
//! Produces:
//!
//! ```html
//! <ruby><rb>日本語</rb><rt>にほんご</rt></ruby>を<ruby><rb>学ぶ</rb><rt>まなぶ</rt></ruby>
//! ```
//!
//! # Implementation notes
//!
//! The pipeline uses MDX-aware parsing (via `markdown-rs`). In that mode, any
//! `{...}` at the inline level is parsed as a `MdxTextExpression` node rather
//! than a `Text` node. The expression's `value` field carries the raw content
//! inside the braces (e.g. `"漢字|かんじ"`). This visitor:
//!
//! 1. Scans `MdxTextExpression` children in inline containers (`Paragraph`,
//!    `Heading`, `Emphasis`, `Strong`, …) for the `base|ruby` pattern.
//! 2. Replaces matching expressions with `MdxJsxTextElement` ruby nodes that
//!    `mdast_to_hast` reconstructs as `<ruby><rb>…</rb><rt>…</rt></ruby>`.
//! 3. Also handles `MdxFlowExpression` at the block level by wrapping the
//!    resulting ruby element in a `Paragraph`.
//!
//! `Text` nodes that happen to contain `{base|ruby}` (possible if MDX parsing
//! is switched off for a specific node) are also scanned via `split_text_node`
//! for completeness.
//!
//! # Edge cases
//!
//! - **Empty base or ruby**: `{|ruby}` or `{base|}` — left as literal text.
//! - **Multiple pipes / JS operators**: `{a|b|c}`, `{a || b}`, `{a?b:c|d}`,
//!   `{obj.prop|filter}`, `{a<b|c}` — left as MDX expressions. The plugin
//!   only matches plain-text annotations to avoid clobbering valid JS
//!   expressions that happen to contain `|`.
//! - **Missing pipe**: a plain MDX expression `{text}` with no `|` — left
//!   as-is (not converted to a ruby element).
//! - **HTML escaping**: base/ruby text is HTML-escaped (`<`, `>`, `&`, `"`,
//!   `'`) before being emitted, so it cannot break the surrounding HTML.
//! - **Escape sequences in Text nodes**: `\{` and `\|` produce literal `{`
//!   and `|` characters (only relevant when scanning raw `Text` nodes).
//!
//! # Out of scope
//!
//! This feature does **NOT** include automatic furigana detection (e.g.
//! kuromoji-style morphological segmentation). It only parses the explicit
//! `{base|ruby}` syntax. Auto-furigana is a separate, much harder feature
//! that is intentionally excluded from this implementation.
//!
//! # Wire
//!
//! Enable via `features.ruby: true` in `zfb.config.ts`. Phase: **mdast**
//! (text-syntax parsing happens in the mdast phase so the visitor can scan
//! raw text before it is converted to hast elements).

use markdown::mdast::{
    AttributeContent, MdxJsxTextElement, Node as MdastNode, Paragraph, Text,
};
use zfb_md_ast::MdastVisitor;

// ── MdxTextExpression ruby parser ─────────────────────────────────────────────

/// Try to parse the content of an `MdxTextExpression` as a `base|ruby`
/// annotation.
///
/// The MDX parser stores the raw expression text (without surrounding `{}`),
/// so this function expects a string like `"漢字|かんじ"`. Returns
/// `Some((base, ruby))` when the expression contains exactly one pipe and
/// both halves contain only plain "text" characters (no JS operators).
/// Returns `None` otherwise (the expression is left unchanged).
///
/// # Why a strict check?
///
/// `features.ruby` opts the user into rewriting `{base|ruby}` expressions,
/// but we MUST NOT accidentally swallow a valid MDX/JS expression that
/// happens to contain a `|`. Cases to guard against:
///
/// - `{a || b}` — JS logical-OR. Contains `||` (double pipe).
/// - `{flag ? x | y : z}` — ternary with bitwise OR. Contains `?` and `:`.
/// - `{obj.prop | filter}` — pipeline-style. Contains `.`.
///
/// The heuristic: reject any value that contains a character that has
/// special meaning in JS expressions (`&|?:=<>().,;[]{}\`) or that
/// contains a double-pipe `||`. Plain text — letters (incl. CJK),
/// digits, spaces, common punctuation that doesn't conflict with JS —
/// passes through.
fn parse_expression_as_ruby(value: &str) -> Option<(String, String)> {
    // Reject `||` (logical-OR) — `{a || b}` must not be treated as ruby.
    if value.contains("||") {
        return None;
    }

    let pipe_pos = value.find('|')?;
    let base = &value[..pipe_pos];
    let ruby = &value[pipe_pos + 1..];
    if base.is_empty() || ruby.is_empty() {
        return None;
    }

    // Reject values containing JS-expression-like characters in either half.
    // Conservative: any char that suggests this is a real JS expression.
    if has_js_operator_chars(base) || has_js_operator_chars(ruby) {
        return None;
    }

    // Reject multiple pipes — `{a|b|c}` looks like bitwise-OR chain, not ruby.
    if ruby.contains('|') {
        return None;
    }

    Some((base.to_string(), ruby.to_string()))
}

/// Return true if `s` contains a character that indicates JS-expression
/// syntax rather than plain ruby text. Used to filter MDX expressions that
/// happen to have a pipe but are not ruby annotations.
fn has_js_operator_chars(s: &str) -> bool {
    s.chars()
        .any(|c| matches!(c, '&' | '?' | ':' | '=' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | ';' | ',' | '\\' | '.' | '!' | '+' | '*' | '/' | '%' | '~' | '^'))
}

// ── HTML escaping ────────────────────────────────────────────────────────────

/// Escape HTML special characters in a text value so it is safe to inline
/// inside an `MdxJsxTextElement` `Text` child that the HTML-path
/// reconstruct-JSX serializer will concatenate verbatim.
///
/// Without this, base/ruby text containing `<`, `>`, `&`, `"` would break
/// the surrounding HTML (or worse, allow markup injection through `<` and
/// `>`). The mdast→hast `HtmlPath` strategy reconstructs `MdxJsxTextElement`
/// via `reconstruct_jsx`, which clones the `Text.value` directly into the
/// output string — there is no later escape pass.
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

// ── mdast node builders ──────────────────────────────────────────────────────

/// Build a `<ruby><rb>base</rb><rt>ruby</rt></ruby>` mdast representation
/// using `MdxJsxTextElement` nodes.
///
/// `MdxJsxTextElement` is the inline JSX variant (as opposed to the block
/// `MdxJsxFlowElement`) — appropriate here because ruby annotations appear
/// inline inside paragraphs. The `HtmlPath` mdast→hast strategy emits these
/// as `JsxRaw` (verbatim passthrough), which the HTML serializer outputs
/// literally. This gives us the desired raw HTML output.
///
/// Base and ruby text are **HTML-escaped** here because the JsxRaw path
/// concatenates `Text.value` verbatim without a later escape pass — without
/// this, a user-supplied `<`, `>`, `&`, or `"` in either half would break
/// the surrounding HTML.
fn make_ruby_node(base: &str, ruby_text: &str) -> MdastNode {
    // <rb>base</rb>
    let rb = MdxJsxTextElement {
        name: Some("rb".to_string()),
        attributes: Vec::<AttributeContent>::new(),
        children: vec![MdastNode::Text(Text {
            value: escape_html(base),
            position: None,
        })],
        position: None,
    };
    // <rt>ruby</rt>
    let rt = MdxJsxTextElement {
        name: Some("rt".to_string()),
        attributes: Vec::<AttributeContent>::new(),
        children: vec![MdastNode::Text(Text {
            value: escape_html(ruby_text),
            position: None,
        })],
        position: None,
    };
    // <ruby>…</ruby>
    let ruby_el = MdxJsxTextElement {
        name: Some("ruby".to_string()),
        attributes: Vec::<AttributeContent>::new(),
        children: vec![
            MdastNode::MdxJsxTextElement(rb),
            MdastNode::MdxJsxTextElement(rt),
        ],
        position: None,
    };
    MdastNode::MdxJsxTextElement(ruby_el)
}

// ── Regex-free Text-node parser ───────────────────────────────────────────────

/// Result of parsing a single `{base|ruby}` token from the start of a string.
/// Used only for `Text` node scanning (MDX expression nodes are handled
/// separately by `parse_expression_as_ruby`).
#[derive(Debug, PartialEq, Eq)]
enum ParseResult<'a> {
    /// A valid ruby annotation was found at the start of `input`.
    Ruby {
        base: String,
        ruby: String,
        rest: &'a str,
    },
    /// The `{` at the start did not produce a valid annotation.
    Invalid { consumed: String, rest: &'a str },
    /// No `{` at the start; `consumed` is the literal prefix up to the next
    /// `{` (or end of string), and `rest` is the remaining input.
    Literal { consumed: String, rest: &'a str },
}

/// Consume a backslash-escaped sequence from `input` (starting **after** the
/// leading `\`). Returns `(char, rest)`. Handles `\{` → `{`, `\|` → `|`,
/// `\}` → `}`, `\\` → `\`; any other char is returned verbatim.
fn consume_escape(input: &str) -> (char, &str) {
    let mut chars = input.chars();
    match chars.next() {
        Some(c @ ('{' | '|' | '}' | '\\')) => (c, &input[c.len_utf8()..]),
        Some(c) => (c, &input[c.len_utf8()..]),
        None => ('\\', input),
    }
}

/// Scan one "token" from the beginning of `input`:
///
/// - If the first char is NOT `{` → collect literal text up to the next `{`
///   (or end of string) and return `Literal`.
/// - If the first char IS `{` → try to parse `{base|ruby}`. If the brace
///   does not contain a pipe or is unclosed, return `Invalid` (the `{`
///   itself becomes a literal).
fn next_token(input: &str) -> ParseResult<'_> {
    if input.is_empty() {
        return ParseResult::Literal {
            consumed: String::new(),
            rest: "",
        };
    }

    let mut chars = input.chars();
    let first = chars.next().unwrap();

    // ── Literal prefix ────────────────────────────────────────────────────
    if first != '{' {
        // Collect characters up to the next unescaped `{`.
        let mut literal = String::new();
        let mut rest = input;
        loop {
            let mut rc = rest.chars();
            match rc.next() {
                None => break,
                Some('\\') => {
                    let after_slash = &rest[1..];
                    let (c, tail) = consume_escape(after_slash);
                    literal.push(c);
                    rest = tail;
                }
                Some('{') => break,
                Some(c) => {
                    literal.push(c);
                    rest = &rest[c.len_utf8()..];
                }
            }
        }
        return ParseResult::Literal {
            consumed: literal,
            rest,
        };
    }

    // ── `{...}` annotation attempt ────────────────────────────────────────
    // We are at the `{`. Try to parse `{base|ruby}`.
    let inner_start = &input[1..]; // skip `{`
    let mut base = String::new();
    let mut ruby = String::new();
    let mut found_pipe = false;
    let mut rest_after_close: Option<&str> = None;
    let mut pos = inner_start;

    loop {
        let mut rc = pos.chars();
        match rc.next() {
            None => {
                // Unclosed brace — emit literal `{` + everything consumed.
                break;
            }
            Some('\\') => {
                let after_slash = &pos[1..];
                let (c, tail) = consume_escape(after_slash);
                if found_pipe {
                    ruby.push(c);
                } else {
                    base.push(c);
                }
                pos = tail;
            }
            Some('{') => {
                // Nested `{` — treat the whole thing as invalid. Emit literal
                // `{` for the outer brace, let the scanner re-process from here.
                break;
            }
            Some('}') => {
                rest_after_close = Some(&pos[1..]);
                break;
            }
            Some('|') if !found_pipe => {
                found_pipe = true;
                pos = &pos[1..];
            }
            Some(c) => {
                if found_pipe {
                    ruby.push(c);
                } else {
                    base.push(c);
                }
                pos = &pos[c.len_utf8()..];
            }
        }
    }

    if let Some(rest) = rest_after_close {
        // Mirror the strict check in `parse_expression_as_ruby` — reject
        // JS-operator-like characters so `{a||b}` etc. stay literal.
        if found_pipe
            && !base.is_empty()
            && !ruby.is_empty()
            && !has_js_operator_chars(&base)
            && !has_js_operator_chars(&ruby)
            && !ruby.contains('|')
        {
            return ParseResult::Ruby { base, ruby, rest };
        }
        // Well-formed `{...}` but no pipe / empty half / suspicious chars
        // → literal `{`.
        return ParseResult::Invalid {
            consumed: "{".to_string(),
            rest: inner_start,
        };
    }

    // Unclosed brace — emit literal `{`.
    ParseResult::Invalid {
        consumed: "{".to_string(),
        rest: inner_start,
    }
}

// ── Text-node rewriter ───────────────────────────────────────────────────────

/// Scan a `Text` node's value for `{base|ruby}` patterns. Returns `None` when
/// there are no annotations (the caller keeps the node unchanged). Returns
/// `Some(nodes)` when at least one annotation was found.
///
/// This path is used when the MDX parser left text as a literal `Text` node
/// rather than parsing `{...}` as an expression. In practice the default MDX
/// pipeline will almost always use `MdxTextExpression` for `{...}` inline
/// content; this branch covers edge-cases or non-MDX parse modes.
fn split_text_node(value: &str) -> Option<Vec<MdastNode>> {
    if !value.contains('{') {
        return None;
    }

    let mut nodes: Vec<MdastNode> = Vec::new();
    let mut rest = value;
    let mut found_any = false;

    while !rest.is_empty() {
        match next_token(rest) {
            ParseResult::Ruby { base, ruby, rest: tail } => {
                found_any = true;
                nodes.push(make_ruby_node(&base, &ruby));
                rest = tail;
            }
            ParseResult::Literal { consumed, rest: tail } => {
                if !consumed.is_empty() {
                    nodes.push(MdastNode::Text(Text {
                        value: consumed,
                        position: None,
                    }));
                }
                rest = tail;
            }
            ParseResult::Invalid { consumed, rest: tail } => {
                nodes.push(MdastNode::Text(Text {
                    value: consumed,
                    position: None,
                }));
                rest = tail;
            }
        }
    }

    if found_any {
        Some(merge_adjacent_text(nodes))
    } else {
        None
    }
}

/// Merge consecutive `Text` nodes into a single `Text` node.
fn merge_adjacent_text(nodes: Vec<MdastNode>) -> Vec<MdastNode> {
    let mut out: Vec<MdastNode> = Vec::with_capacity(nodes.len());
    for node in nodes {
        match node {
            MdastNode::Text(t) => {
                if let Some(MdastNode::Text(prev)) = out.last_mut() {
                    prev.value.push_str(&t.value);
                } else {
                    out.push(MdastNode::Text(t));
                }
            }
            other => out.push(other),
        }
    }
    out
}

// ── Inline-child rewriter ────────────────────────────────────────────────────

/// Rewrite inline children by expanding any `MdxTextExpression` with a
/// `base|ruby` value (or any `Text` node with `{base|ruby}` literal syntax)
/// into `MdxJsxTextElement` ruby nodes.
///
/// Returns `true` when at least one rewrite happened.
fn rewrite_inline_children(children: &mut Vec<MdastNode>) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i < children.len() {
        // ── MdxTextExpression: `{base|ruby}` parsed by MDX ────────────────
        // In MDX mode `{漢字|かんじ}` becomes MdxTextExpression { value: "漢字|かんじ" }.
        if let MdastNode::MdxTextExpression(expr) = &children[i] {
            if let Some((base, ruby)) = parse_expression_as_ruby(&expr.value.clone()) {
                children[i] = make_ruby_node(&base, &ruby);
                changed = true;
                i += 1;
                continue;
            }
        }

        // ── Text node: `{base|ruby}` literal (non-MDX or edge-cases) ──────
        if let MdastNode::Text(t) = &children[i] {
            if let Some(replacement) = split_text_node(&t.value.clone()) {
                let tail = children.split_off(i + 1);
                children.pop(); // remove the original Text node
                let replacement_len = replacement.len();
                children.extend(replacement);
                children.extend(tail);
                i += replacement_len;
                changed = true;
                continue;
            }
        }

        i += 1;
    }
    changed
}

// ── Visitor ──────────────────────────────────────────────────────────────────

/// Mdast visitor that rewrites `{base|ruby}` patterns into inline
/// `<ruby><rb>…</rb><rt>…</rt></ruby>` JSX elements.
///
/// In the MDX pipeline, `{base|ruby}` is parsed as:
/// - `MdxTextExpression` when it appears inline inside a paragraph/heading.
/// - `MdxFlowExpression` when it appears on its own line (block-level).
///
/// This visitor handles both:
/// - Inline: replaces `MdxTextExpression` with the ruby element in-place.
/// - Flow: replaces `MdxFlowExpression` with a `Paragraph` wrapping the ruby
///   element, so the HTML output gets a `<p>` wrapper.
///
/// `Text` nodes that happen to contain `{base|ruby}` literals are also scanned
/// (covers non-MDX or pre-MDX-parse contexts).
///
/// Wire via `features.ruby: true` in `zfb.config.ts`. The plugin is registered
/// in the **mdast phase** (before mdast→hast conversion).
#[derive(Debug, Default, Clone, Copy)]
pub struct RubyPlugin;

impl RubyPlugin {
    /// Create a new (stateless) plugin instance.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// Try to rewrite a block-level `MdxFlowExpression` into a `Paragraph`
/// wrapping a ruby element. Returns `true` if rewritten.
fn try_rewrite_flow_expression(node: &mut MdastNode) -> bool {
    let MdastNode::MdxFlowExpression(expr) = node else {
        return false;
    };
    let Some((base, ruby)) = parse_expression_as_ruby(&expr.value.clone()) else {
        return false;
    };
    let ruby_node = make_ruby_node(&base, &ruby);
    *node = MdastNode::Paragraph(Paragraph {
        children: vec![ruby_node],
        position: None,
    });
    true
}

impl MdastVisitor for RubyPlugin {
    fn visit(&mut self, node: &mut MdastNode) {
        match node {
            MdastNode::Root(root) => {
                for child in &mut root.children {
                    self.visit(child);
                }
            }
            MdastNode::MdxFlowExpression(_) => {
                // Block-level `{base|ruby}` — try to convert to Paragraph.
                try_rewrite_flow_expression(node);
            }
            MdastNode::Paragraph(p) => {
                rewrite_inline_children(&mut p.children);
                for child in &mut p.children {
                    self.visit(child);
                }
            }
            MdastNode::Heading(h) => {
                rewrite_inline_children(&mut h.children);
                for child in &mut h.children {
                    self.visit(child);
                }
            }
            MdastNode::Emphasis(e) => {
                rewrite_inline_children(&mut e.children);
                for child in &mut e.children {
                    self.visit(child);
                }
            }
            MdastNode::Strong(s) => {
                rewrite_inline_children(&mut s.children);
                for child in &mut s.children {
                    self.visit(child);
                }
            }
            MdastNode::Delete(d) => {
                rewrite_inline_children(&mut d.children);
                for child in &mut d.children {
                    self.visit(child);
                }
            }
            MdastNode::Link(l) => {
                rewrite_inline_children(&mut l.children);
                for child in &mut l.children {
                    self.visit(child);
                }
            }
            MdastNode::LinkReference(lr) => {
                rewrite_inline_children(&mut lr.children);
                for child in &mut lr.children {
                    self.visit(child);
                }
            }
            MdastNode::Blockquote(bq) => {
                for child in &mut bq.children {
                    self.visit(child);
                }
            }
            MdastNode::List(list) => {
                for item in &mut list.children {
                    self.visit(item);
                }
            }
            MdastNode::ListItem(item) => {
                for child in &mut item.children {
                    self.visit(child);
                }
            }
            MdastNode::TableCell(tc) => {
                rewrite_inline_children(&mut tc.children);
                for child in &mut tc.children {
                    self.visit(child);
                }
            }
            MdastNode::TableRow(tr) => {
                for child in &mut tr.children {
                    self.visit(child);
                }
            }
            MdastNode::Table(t) => {
                for row in &mut t.children {
                    self.visit(row);
                }
            }
            MdastNode::MdxJsxFlowElement(j) => {
                for child in &mut j.children {
                    self.visit(child);
                }
            }
            MdastNode::MdxJsxTextElement(j) => {
                rewrite_inline_children(&mut j.children);
                for child in &mut j.children {
                    self.visit(child);
                }
            }
            // Leaf nodes (Text, Code, InlineCode, …) are not containers.
            _ => {}
        }
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_expression_as_ruby ──────────────────────────────────────────

    #[test]
    fn expression_valid_ruby() {
        assert_eq!(
            parse_expression_as_ruby("漢字|かんじ"),
            Some(("漢字".to_string(), "かんじ".to_string()))
        );
    }

    #[test]
    fn expression_empty_base_returns_none() {
        assert!(parse_expression_as_ruby("|かんじ").is_none());
    }

    #[test]
    fn expression_empty_ruby_returns_none() {
        assert!(parse_expression_as_ruby("漢字|").is_none());
    }

    #[test]
    fn expression_no_pipe_returns_none() {
        assert!(parse_expression_as_ruby("漢字").is_none());
    }

    #[test]
    fn expression_multiple_pipes_rejected() {
        // Multiple pipes look like a JS bitwise-OR chain rather than ruby.
        assert!(parse_expression_as_ruby("a|b|c").is_none());
    }

    #[test]
    fn expression_logical_or_rejected() {
        // `{a || b}` is JS logical-OR, NOT ruby.
        assert!(parse_expression_as_ruby("a || b").is_none());
    }

    #[test]
    fn expression_ternary_rejected() {
        // `{flag ? x | y : z}` is a JS ternary, NOT ruby.
        assert!(parse_expression_as_ruby("flag ? x | y : z").is_none());
    }

    #[test]
    fn expression_with_dot_rejected() {
        // `{obj.prop|filter}` looks like a pipeline expression, NOT ruby.
        assert!(parse_expression_as_ruby("obj.prop|filter").is_none());
    }

    #[test]
    fn expression_with_angle_bracket_rejected() {
        // Reject expressions that contain `<` or `>`.
        assert!(parse_expression_as_ruby("a<b|c").is_none());
        assert!(parse_expression_as_ruby("a|b>c").is_none());
    }

    // ── next_token (Text-node scanner) ────────────────────────────────────

    #[test]
    fn token_plain_literal() {
        let r = next_token("hello world");
        assert_eq!(
            r,
            ParseResult::Literal {
                consumed: "hello world".to_string(),
                rest: ""
            }
        );
    }

    #[test]
    fn token_literal_stops_at_open_brace() {
        let r = next_token("hello{漢字|かんじ}");
        assert_eq!(
            r,
            ParseResult::Literal {
                consumed: "hello".to_string(),
                rest: "{漢字|かんじ}"
            }
        );
    }

    #[test]
    fn token_valid_ruby() {
        let r = next_token("{漢字|かんじ}");
        assert_eq!(
            r,
            ParseResult::Ruby {
                base: "漢字".to_string(),
                ruby: "かんじ".to_string(),
                rest: ""
            }
        );
    }

    #[test]
    fn token_valid_ruby_with_tail() {
        let r = next_token("{日|に}本");
        assert_eq!(
            r,
            ParseResult::Ruby {
                base: "日".to_string(),
                ruby: "に".to_string(),
                rest: "本"
            }
        );
    }

    #[test]
    fn token_unclosed_brace_is_invalid() {
        let r = next_token("{unclosed");
        assert!(matches!(r, ParseResult::Invalid { .. }));
    }

    #[test]
    fn token_no_pipe_is_invalid() {
        let r = next_token("{nopipe}");
        assert!(matches!(r, ParseResult::Invalid { .. }));
    }

    #[test]
    fn token_empty_base_is_invalid() {
        let r = next_token("{|ruby}");
        assert!(matches!(r, ParseResult::Invalid { .. }));
    }

    #[test]
    fn token_empty_ruby_is_invalid() {
        let r = next_token("{base|}");
        assert!(matches!(r, ParseResult::Invalid { .. }));
    }

    #[test]
    fn token_escape_open_brace() {
        // `\{base|ruby}` — the `\{` produces a literal `{`, then the rest
        // of the text is consumed as plain literal characters (no annotation).
        let r = next_token("\\{base|ruby}");
        assert!(matches!(r, ParseResult::Literal { .. }));
        if let ParseResult::Literal { consumed, rest } = r {
            // The whole string is literal: `{base|ruby}`.
            assert_eq!(consumed, "{base|ruby}");
            assert_eq!(rest, "");
        }
    }

    #[test]
    fn token_escape_pipe_inside_annotation() {
        // `{base\|still_base|ruby}` — `\|` inside an annotation produces a
        // literal `|` in the base, so the first real `|` is the separator.
        let r = next_token("{a\\|b|c}");
        assert_eq!(
            r,
            ParseResult::Ruby {
                base: "a|b".to_string(),
                ruby: "c".to_string(),
                rest: ""
            }
        );
    }

    // ── split_text_node ───────────────────────────────────────────────────

    #[test]
    fn split_no_annotations_returns_none() {
        assert!(split_text_node("plain text").is_none());
    }

    #[test]
    fn split_single_annotation() {
        let nodes = split_text_node("{漢字|かんじ}").unwrap();
        assert_eq!(nodes.len(), 1);
        assert!(matches!(nodes[0], MdastNode::MdxJsxTextElement(_)));
    }

    #[test]
    fn split_annotation_in_sentence() {
        let nodes = split_text_node("私は{日本語|にほんご}を話す").unwrap();
        // Expect: Text("私は"), MdxJsxTextElement, Text("を話す")
        assert_eq!(nodes.len(), 3);
        assert!(matches!(nodes[0], MdastNode::Text(_)));
        assert!(matches!(nodes[1], MdastNode::MdxJsxTextElement(_)));
        assert!(matches!(nodes[2], MdastNode::Text(_)));
    }

    #[test]
    fn split_multiple_annotations() {
        let nodes = split_text_node("{日本語|にほんご}を{学ぶ|まなぶ}").unwrap();
        // Expect: MdxJsxTextElement, Text("を"), MdxJsxTextElement
        assert_eq!(nodes.len(), 3);
        assert!(matches!(nodes[0], MdastNode::MdxJsxTextElement(_)));
        assert!(matches!(nodes[1], MdastNode::Text(_)));
        assert!(matches!(nodes[2], MdastNode::MdxJsxTextElement(_)));
    }

    #[test]
    fn split_adjacent_annotations() {
        let nodes = split_text_node("{a|b}{c|d}").unwrap();
        assert_eq!(nodes.len(), 2);
        assert!(matches!(nodes[0], MdastNode::MdxJsxTextElement(_)));
        assert!(matches!(nodes[1], MdastNode::MdxJsxTextElement(_)));
    }

    #[test]
    fn split_unbalanced_brace_is_literal() {
        // An unclosed `{` is left as literal text.
        // No valid annotation → should return None.
        let result = split_text_node("{unclosed");
        assert!(result.is_none());
    }

    // ── RubyPlugin visitor (Text nodes) ───────────────────────────────────

    fn make_para_text(text: &str) -> MdastNode {
        MdastNode::Paragraph(Paragraph {
            children: vec![MdastNode::Text(Text {
                value: text.to_string(),
                position: None,
            })],
            position: None,
        })
    }

    fn make_root(children: Vec<MdastNode>) -> MdastNode {
        MdastNode::Root(markdown::mdast::Root {
            children,
            position: None,
        })
    }

    #[test]
    fn visitor_rewrites_annotation_in_text_node() {
        // Text-node path (non-MDX mode simulation).
        let mut tree = make_root(vec![make_para_text("{漢字|かんじ}")]);
        RubyPlugin::new().visit(&mut tree);
        let MdastNode::Root(root) = &tree else {
            panic!("expected Root");
        };
        let MdastNode::Paragraph(p) = &root.children[0] else {
            panic!("expected Paragraph");
        };
        assert_eq!(p.children.len(), 1, "one child: the <ruby> element");
        assert!(
            matches!(p.children[0], MdastNode::MdxJsxTextElement(_)),
            "child must be MdxJsxTextElement"
        );
    }

    #[test]
    fn visitor_leaves_plain_text_unchanged() {
        let mut tree = make_root(vec![make_para_text("hello world")]);
        let before = format!("{tree:?}");
        RubyPlugin::new().visit(&mut tree);
        assert_eq!(format!("{tree:?}"), before, "tree must not change");
    }

    #[test]
    fn visitor_rewrites_multiple_text_annotations() {
        let mut tree = make_root(vec![make_para_text(
            "{東京|とうきょう}は{首都|しゅと}です",
        )]);
        RubyPlugin::new().visit(&mut tree);
        let MdastNode::Root(root) = &tree else {
            panic!("expected Root");
        };
        let MdastNode::Paragraph(p) = &root.children[0] else {
            panic!("expected Paragraph");
        };
        // Expected: <ruby>(東京), Text("は"), <ruby>(首都), Text("です")
        assert_eq!(p.children.len(), 4);
        assert!(matches!(p.children[0], MdastNode::MdxJsxTextElement(_)));
        assert!(matches!(p.children[1], MdastNode::Text(_)));
        assert!(matches!(p.children[2], MdastNode::MdxJsxTextElement(_)));
        assert!(matches!(p.children[3], MdastNode::Text(_)));
    }

    #[test]
    fn visitor_handles_invalid_text_syntax_gracefully() {
        let mut tree = make_root(vec![make_para_text("{unclosed and {nopipe} text")]);
        RubyPlugin::new().visit(&mut tree);
        let MdastNode::Root(root) = &tree else {
            panic!("expected Root");
        };
        let MdastNode::Paragraph(p) = &root.children[0] else {
            panic!("expected Paragraph");
        };
        for child in &p.children {
            assert!(
                matches!(child, MdastNode::Text(_)),
                "expected all Text nodes, got {child:?}"
            );
        }
    }

    // ── RubyPlugin visitor (MdxTextExpression nodes) ──────────────────────

    fn make_para_expr(value: &str) -> MdastNode {
        MdastNode::Paragraph(Paragraph {
            children: vec![MdastNode::MdxTextExpression(
                markdown::mdast::MdxTextExpression {
                    value: value.to_string(),
                    position: None,
                    stops: Vec::new(),
                },
            )],
            position: None,
        })
    }

    #[test]
    fn visitor_rewrites_mdx_text_expression() {
        let mut tree = make_root(vec![make_para_expr("漢字|かんじ")]);
        RubyPlugin::new().visit(&mut tree);
        let MdastNode::Root(root) = &tree else {
            panic!("expected Root");
        };
        let MdastNode::Paragraph(p) = &root.children[0] else {
            panic!("expected Paragraph");
        };
        assert_eq!(p.children.len(), 1);
        assert!(
            matches!(p.children[0], MdastNode::MdxJsxTextElement(_)),
            "expected MdxJsxTextElement from MDX expression, got {:?}",
            p.children[0]
        );
    }

    #[test]
    fn visitor_leaves_non_ruby_expression_unchanged() {
        let mut tree = make_root(vec![make_para_expr("someJsExpr")]);
        let before = format!("{tree:?}");
        RubyPlugin::new().visit(&mut tree);
        assert_eq!(format!("{tree:?}"), before, "non-ruby expression must not change");
    }

    // ── MdxFlowExpression (block-level) ───────────────────────────────────

    fn make_flow_expr(value: &str) -> MdastNode {
        MdastNode::MdxFlowExpression(markdown::mdast::MdxFlowExpression {
            value: value.to_string(),
            position: None,
            stops: Vec::new(),
        })
    }

    #[test]
    fn visitor_rewrites_flow_expression_to_paragraph() {
        let mut tree = make_root(vec![make_flow_expr("漢字|かんじ")]);
        RubyPlugin::new().visit(&mut tree);
        let MdastNode::Root(root) = &tree else {
            panic!("expected Root");
        };
        assert!(
            matches!(root.children[0], MdastNode::Paragraph(_)),
            "flow expression must become Paragraph, got {:?}",
            root.children[0]
        );
        let MdastNode::Paragraph(p) = &root.children[0] else {
            panic!("expected Paragraph");
        };
        assert_eq!(p.children.len(), 1);
        assert!(
            matches!(p.children[0], MdastNode::MdxJsxTextElement(_)),
            "paragraph child must be ruby element"
        );
    }

    #[test]
    fn visitor_leaves_non_ruby_flow_expression_unchanged() {
        let mut tree = make_root(vec![make_flow_expr("someJsExpr")]);
        let before = format!("{tree:?}");
        RubyPlugin::new().visit(&mut tree);
        assert_eq!(format!("{tree:?}"), before, "non-ruby flow expression must not change");
    }

    // ── escape_html ───────────────────────────────────────────────────────

    #[test]
    fn escape_html_basic() {
        assert_eq!(escape_html("a & b"), "a &amp; b");
        assert_eq!(escape_html("<em>"), "&lt;em&gt;");
        assert_eq!(escape_html("\"q\""), "&quot;q&quot;");
        assert_eq!(escape_html("plain"), "plain");
    }

    // ── Link container ────────────────────────────────────────────────────

    #[test]
    fn visitor_rewrites_inside_link() {
        // [{漢字|かんじ}](/url) — annotation should be rewritten inside link text.
        let link = MdastNode::Link(markdown::mdast::Link {
            url: "/url".to_string(),
            title: None,
            children: vec![MdastNode::MdxTextExpression(
                markdown::mdast::MdxTextExpression {
                    value: "漢字|かんじ".to_string(),
                    position: None,
                    stops: Vec::new(),
                },
            )],
            position: None,
        });
        let mut tree = make_root(vec![MdastNode::Paragraph(Paragraph {
            children: vec![link],
            position: None,
        })]);
        RubyPlugin::new().visit(&mut tree);
        let MdastNode::Root(root) = &tree else {
            panic!("expected Root");
        };
        let MdastNode::Paragraph(p) = &root.children[0] else {
            panic!("expected Paragraph");
        };
        let MdastNode::Link(l) = &p.children[0] else {
            panic!("expected Link");
        };
        assert!(
            matches!(l.children[0], MdastNode::MdxJsxTextElement(_)),
            "annotation inside link must become MdxJsxTextElement, got {:?}",
            l.children[0]
        );
    }

    // ── make_ruby_node structure ──────────────────────────────────────────

    #[test]
    fn make_ruby_node_structure() {
        let node = make_ruby_node("漢字", "かんじ");
        let MdastNode::MdxJsxTextElement(ruby) = node else {
            panic!("expected MdxJsxTextElement");
        };
        assert_eq!(ruby.name.as_deref(), Some("ruby"));
        assert_eq!(ruby.children.len(), 2);
        let MdastNode::MdxJsxTextElement(rb) = &ruby.children[0] else {
            panic!("expected rb element");
        };
        assert_eq!(rb.name.as_deref(), Some("rb"));
        let MdastNode::MdxJsxTextElement(rt) = &ruby.children[1] else {
            panic!("expected rt element");
        };
        assert_eq!(rt.name.as_deref(), Some("rt"));
    }

    #[test]
    fn make_ruby_node_escapes_html_chars() {
        // Defensive: the parser already rejects `<>` in base/ruby (they trip
        // `has_js_operator_chars`), but if a caller somehow gets unsafe text
        // through (e.g. via the Text-node path), make_ruby_node must still
        // escape it so the JSX raw output cannot break the surrounding HTML.
        let node = make_ruby_node("a&b", "c\"d");
        let MdastNode::MdxJsxTextElement(ruby) = node else {
            panic!("expected MdxJsxTextElement");
        };
        let MdastNode::MdxJsxTextElement(rb) = &ruby.children[0] else {
            panic!("expected rb");
        };
        let MdastNode::Text(t) = &rb.children[0] else {
            panic!("expected Text");
        };
        assert_eq!(t.value, "a&amp;b");
        let MdastNode::MdxJsxTextElement(rt) = &ruby.children[1] else {
            panic!("expected rt");
        };
        let MdastNode::Text(t) = &rt.children[0] else {
            panic!("expected Text");
        };
        assert_eq!(t.value, "c&quot;d");
    }
}
