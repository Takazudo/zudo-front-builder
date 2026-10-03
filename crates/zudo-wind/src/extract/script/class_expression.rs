//! Class-context propagation through script expressions. A complete string is
//! a class candidate only when its value provably reaches a class attribute:
//! directly, through a conditional branch or logical fallback, a template
//! interpolation, a supported class helper, or a same-module `const`.

use std::collections::{BTreeMap, BTreeSet};

use super::super::Collector;
use super::emit;

/// Helpers whose string arguments, arrays and object keys are class lists.
const CLASS_HELPERS: &[&str] = &["clsx", "cn", "cx", "classNames", "classnames"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Ident,
    Str,
    Template,
    Number,
    Punct,
}

#[derive(Clone, Copy, Debug)]
struct Token {
    kind: Kind,
    start: usize,
    end: usize,
}

const PUNCTS: &[&str] = &[
    "===", "!==", "...", "**=", "&&=", "||=", "??=", "<<=", ">>=", "=>", "==", "!=", "<=", ">=",
    "&&", "||", "??", "?.", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "**", "<<",
    ">>",
];

fn tokenize(text: &str, from: usize, to: usize) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut tokens: Vec<Token> = Vec::new();
    let mut i = from;
    while i < to {
        let byte = bytes[i];
        if byte.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if bytes[i..to].starts_with(b"//") {
            i = text[i..to].find('\n').map_or(to, |n| i + n);
            continue;
        }
        if bytes[i..to].starts_with(b"/*") {
            i = text[i + 2..to].find("*/").map_or(to, |n| i + n + 4);
            continue;
        }
        let start = i;
        if byte == b'"' || byte == b'\'' {
            i += 1;
            while i < to && bytes[i] != byte && bytes[i] != b'\n' {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(to);
            tokens.push(Token {
                kind: Kind::Str,
                start,
                end: i,
            });
            continue;
        }
        if byte == b'`' {
            i = template_end(text, i, to);
            tokens.push(Token {
                kind: Kind::Template,
                start,
                end: i,
            });
            continue;
        }
        if byte == b'/' && regex_allowed(text, &tokens) {
            i = regex_end(bytes, i, to);
            tokens.push(Token {
                kind: Kind::Punct,
                start,
                end: i,
            });
            continue;
        }
        if byte.is_ascii_digit() {
            while i < to && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.') {
                i += 1;
            }
            tokens.push(Token {
                kind: Kind::Number,
                start,
                end: i,
            });
            continue;
        }
        if byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$' || byte >= 0x80 {
            while i < to
                && (bytes[i].is_ascii_alphanumeric()
                    || bytes[i] == b'_'
                    || bytes[i] == b'$'
                    || bytes[i] >= 0x80)
            {
                i += 1;
            }
            tokens.push(Token {
                kind: Kind::Ident,
                start,
                end: i,
            });
            continue;
        }
        let width = PUNCTS
            .iter()
            .find(|punct| text[i..to].starts_with(**punct))
            .map_or(1, |punct| punct.len());
        i += width;
        tokens.push(Token {
            kind: Kind::Punct,
            start,
            end: i,
        });
    }
    tokens
}

fn template_end(text: &str, open: usize, to: usize) -> usize {
    let bytes = text.as_bytes();
    let mut i = open + 1;
    while i < to {
        match bytes[i] {
            b'\\' => i += 2,
            b'`' => return i + 1,
            b'$' if bytes.get(i + 1) == Some(&b'{') => {
                i = interpolation_end(text, i + 2, to);
            }
            _ => i += 1,
        }
    }
    to
}

/// The index just past the `}` closing an interpolation that opened before `start`.
fn interpolation_end(text: &str, start: usize, to: usize) -> usize {
    let bytes = text.as_bytes();
    let mut depth = 1;
    let mut i = start;
    while i < to {
        match bytes[i] {
            b'"' | b'\'' => {
                let quote = bytes[i];
                i += 1;
                while i < to && bytes[i] != quote && bytes[i] != b'\n' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'`' => {
                i = template_end(text, i, to);
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    to
}

fn regex_allowed(text: &str, tokens: &[Token]) -> bool {
    match tokens.last() {
        None => true,
        Some(token) => match token.kind {
            Kind::Punct => !matches!(&text[token.start..token.end], ")" | "]" | "}"),
            Kind::Ident => matches!(
                &text[token.start..token.end],
                "return" | "typeof" | "case" | "yield" | "throw" | "in" | "of"
            ),
            _ => false,
        },
    }
}

fn regex_end(bytes: &[u8], open: usize, to: usize) -> usize {
    let mut i = open + 1;
    let mut bracket = false;
    while i < to && bytes[i] != b'\n' {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'[' => bracket = true,
            b']' => bracket = false,
            b'/' if !bracket => return i + 1,
            _ => {}
        }
        i += 1;
    }
    i.min(to)
}

/// A same-module binding that can be traced: exactly one `const NAME = ...`
/// with no other declaration, import, or assignment of NAME.
struct Bindings {
    traceable: BTreeMap<String, (usize, usize)>,
}

const STATEMENT_STARTS: &[&str] = &[
    "const",
    "let",
    "var",
    "export",
    "function",
    "import",
    "return",
    "if",
    "for",
    "while",
    "class",
    "type",
    "interface",
    "async",
    "default",
];

impl Bindings {
    fn collect(text: &str, tokens: &[Token]) -> Self {
        let word = |token: &Token| &text[token.start..token.end];
        let mut consts: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
        let mut untraceable: BTreeSet<String> = BTreeSet::new();
        let mut index = 0;
        while index < tokens.len() {
            let token = tokens[index];
            if token.kind != Kind::Ident {
                index += 1;
                continue;
            }
            match word(&token) {
                "import" => {
                    let mut cursor = index + 1;
                    while cursor < tokens.len()
                        && !(tokens[cursor].kind == Kind::Ident && word(&tokens[cursor]) == "from")
                        && tokens[cursor].kind != Kind::Str
                    {
                        if tokens[cursor].kind == Kind::Ident {
                            untraceable.insert(word(&tokens[cursor]).to_owned());
                        }
                        cursor += 1;
                    }
                    index = cursor;
                    continue;
                }
                "let" | "var" | "function" | "class" => {
                    if let Some(next) = tokens.get(index + 1).filter(|t| t.kind == Kind::Ident) {
                        untraceable.insert(word(next).to_owned());
                    }
                }
                "const" => {
                    if let Some(range) = const_initializer(text, tokens, index) {
                        let name = word(&tokens[index + 1]).to_owned();
                        consts.entry(name).or_default().push(range);
                    } else if let Some(next) =
                        tokens.get(index + 1).filter(|t| t.kind == Kind::Ident)
                    {
                        untraceable.insert(word(next).to_owned());
                    }
                }
                name => {
                    let assigned = tokens.get(index + 1).is_some_and(|next| {
                        next.kind == Kind::Punct
                            && matches!(
                                &text[next.start..next.end],
                                "=" | "+=" | "||=" | "&&=" | "??=" | "++" | "--"
                            )
                    });
                    let declared = index > 0
                        && tokens[index - 1].kind == Kind::Ident
                        && word(&tokens[index - 1]) == "const";
                    if assigned && !declared {
                        untraceable.insert(name.to_owned());
                    }
                }
            }
            index += 1;
        }
        let traceable = consts
            .into_iter()
            .filter(|(name, ranges)| ranges.len() == 1 && !untraceable.contains(name))
            .map(|(name, ranges)| (name, ranges[0]))
            .collect();
        Self { traceable }
    }
}

/// The initializer token range of `const NAME [: Type] = <initializer>`.
fn const_initializer(text: &str, tokens: &[Token], at: usize) -> Option<(usize, usize)> {
    let word = |token: &Token| &text[token.start..token.end];
    tokens.get(at + 1).filter(|t| t.kind == Kind::Ident)?;
    let mut cursor = at + 2;
    if tokens.get(cursor).is_some_and(|t| word(t) == ":") {
        let mut depth = 0i32;
        while let Some(token) = tokens.get(cursor) {
            match word(token) {
                "<" | "(" | "[" | "{" => depth += 1,
                ">" | ")" | "]" | "}" => depth -= 1,
                "=" if depth == 0 => break,
                _ => {}
            }
            cursor += 1;
        }
    }
    if tokens.get(cursor).map(word) != Some("=") {
        return None;
    }
    let start = cursor + 1;
    let mut depth = 0i32;
    let mut end = start;
    while let Some(token) = tokens.get(end) {
        let spelling = word(token);
        if depth == 0 {
            if matches!(spelling, ";" | ",") {
                break;
            }
            let on_new_line = end > start && text[tokens[end - 1].end..token.start].contains('\n');
            if on_new_line
                && (matches!(spelling, "}" | ")" | "]")
                    || (token.kind == Kind::Ident && STATEMENT_STARTS.contains(&spelling)))
            {
                break;
            }
        }
        match spelling {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
        end += 1;
    }
    (end > start).then_some((start, end))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Container {
    Group,
    Helper,
    Array,
    /// An object literal inside a helper: its string keys are classes.
    HelperObject,
    Opaque,
}

pub(super) struct Module<'a> {
    text: &'a str,
    base: usize,
    tokens: Vec<Token>,
    bindings: Bindings,
}

impl<'a> Module<'a> {
    pub(super) fn new(text: &'a str, base: usize) -> Self {
        let tokens = tokenize(text, 0, text.len());
        let bindings = Bindings::collect(text, &tokens);
        Self {
            text,
            base,
            tokens,
            bindings,
        }
    }

    fn word(&self, token: &Token) -> &'a str {
        &self.text[token.start..token.end]
    }

    /// Walks every `class={...}` / `className={...}` expression container.
    pub(super) fn scan(&self, out: &mut Collector<'_>) {
        let mut index = 0;
        while index + 2 < self.tokens.len() {
            let attribute = self.tokens[index];
            let is_class = attribute.kind == Kind::Ident
                && matches!(self.word(&attribute), "class" | "className")
                && self.word(&self.tokens[index + 1]) == "="
                && self.word(&self.tokens[index + 2]) == "{"
                && !(index > 0
                    && matches!(self.word(&self.tokens[index - 1]), "const" | "let" | "var"));
            if !is_class {
                index += 1;
                continue;
            }
            let open = index + 2;
            let close = self.matching(open);
            let mut visiting = BTreeSet::new();
            self.expression(&self.tokens[open + 1..close], out, &mut visiting);
            index = close.max(open + 1);
        }
    }

    fn matching(&self, open: usize) -> usize {
        let mut depth = 0;
        for (offset, token) in self.tokens[open..].iter().enumerate() {
            match self.word(token) {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => {
                    depth -= 1;
                    if depth == 0 {
                        return open + offset;
                    }
                }
                _ => {}
            }
        }
        self.tokens.len()
    }

    /// Tokens of one expression in class context.
    fn expression(
        &self,
        tokens: &[Token],
        out: &mut Collector<'_>,
        visiting: &mut BTreeSet<String>,
    ) {
        let mut stack: Vec<Container> = Vec::new();
        for (index, token) in tokens.iter().enumerate() {
            let spelling = self.word(token);
            let previous = index.checked_sub(1).map(|at| tokens[at]);
            let next = tokens.get(index + 1).copied();
            if token.kind == Kind::Punct {
                match spelling {
                    "(" => stack.push(match previous {
                        Some(p)
                            if p.kind == Kind::Ident && CLASS_HELPERS.contains(&self.word(&p)) =>
                        {
                            Container::Helper
                        }
                        Some(p)
                            if matches!(p.kind, Kind::Ident | Kind::Str | Kind::Template)
                                || matches!(self.word(&p), ")" | "]") =>
                        {
                            Container::Opaque
                        }
                        _ => Container::Group,
                    }),
                    "[" => stack.push(match previous {
                        Some(p)
                            if matches!(p.kind, Kind::Ident | Kind::Str | Kind::Template)
                                || matches!(self.word(&p), ")" | "]") =>
                        {
                            Container::Opaque
                        }
                        _ => Container::Array,
                    }),
                    "{" => stack.push(
                        if stack.contains(&Container::Helper)
                            && previous.is_none_or(|p| self.word(&p) != "=>")
                        {
                            Container::HelperObject
                        } else {
                            Container::Opaque
                        },
                    ),
                    ")" | "]" | "}" => {
                        stack.pop();
                    }
                    _ => {}
                }
                continue;
            }
            if stack.contains(&Container::Opaque) {
                continue;
            }
            let in_object = stack.last() == Some(&Container::HelperObject);
            let value = if in_object {
                previous.is_some_and(|p| matches!(self.word(&p), "{" | ","))
                    && next.is_some_and(|n| self.word(&n) == ":")
            } else {
                self.value_position(previous, next)
            };
            if !value {
                continue;
            }
            match token.kind {
                Kind::Str => self.emit_string(token, out),
                Kind::Template => self.emit_template(token, out, visiting),
                Kind::Ident if !in_object => self.trace(spelling, out, visiting),
                _ => {}
            }
        }
    }

    fn value_position(&self, previous: Option<Token>, next: Option<Token>) -> bool {
        let before = previous.is_none_or(|p| {
            p.kind == Kind::Punct
                && matches!(
                    self.word(&p),
                    "?" | ":" | "&&" | "||" | "??" | "," | "(" | "["
                )
        });
        let after = next.is_none_or(|n| {
            n.kind == Kind::Punct && matches!(self.word(&n), ":" | ")" | "]" | "," | "||" | "??")
        });
        before && after
    }

    fn emit_string(&self, token: &Token, out: &mut Collector<'_>) {
        if token.end - token.start < 2 {
            return;
        }
        emit(
            self.text,
            self.base,
            out,
            token.start + 1,
            token.end - 1,
            token.start,
            token.end,
            true,
            false,
            false,
            false,
        );
    }

    fn emit_template(
        &self,
        token: &Token,
        out: &mut Collector<'_>,
        visiting: &mut BTreeSet<String>,
    ) {
        let bytes = self.text.as_bytes();
        let mut segment = token.start + 1;
        let mut left = false;
        let mut i = segment;
        while i < token.end {
            match bytes[i] {
                b'\\' => i += 2,
                b'`' => {
                    emit(
                        self.text,
                        self.base,
                        out,
                        segment,
                        i,
                        token.start,
                        i + 1,
                        true,
                        false,
                        false,
                        left,
                    );
                    return;
                }
                b'$' if bytes.get(i + 1) == Some(&b'{') => {
                    emit(
                        self.text,
                        self.base,
                        out,
                        segment,
                        i,
                        token.start,
                        i + 2,
                        true,
                        false,
                        true,
                        left,
                    );
                    let end = interpolation_end(self.text, i + 2, token.end);
                    let inner = tokenize(self.text, i + 2, end.saturating_sub(1));
                    self.expression(&inner, out, visiting);
                    segment = end;
                    left = true;
                    i = end;
                }
                _ => i += 1,
            }
        }
    }

    fn trace(&self, name: &str, out: &mut Collector<'_>, visiting: &mut BTreeSet<String>) {
        let Some(&(start, end)) = self.bindings.traceable.get(name) else {
            return;
        };
        if !visiting.insert(name.to_owned()) {
            return;
        }
        self.expression(&self.tokens[start..end], out, visiting);
        visiting.remove(name);
    }
}
