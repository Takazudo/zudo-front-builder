//! Class-context propagation through script expressions. A complete string is
//! a class candidate only when its value provably reaches a class attribute:
//! directly, through a conditional branch or logical fallback, a template
//! interpolation, a supported class helper, or a same-module `const`.

use std::collections::{BTreeMap, BTreeSet};

use super::super::Collector;
use super::{emit, JsxSpans};

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

fn tokenize(text: &str, from: usize, to: usize, jsx: &JsxSpans) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut tokens: Vec<Token> = Vec::new();
    let mut i = from;
    while i < to {
        if let Some(end) = jsx.text_end(i) {
            i = end.min(to);
            continue;
        }
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
            let direct_jsx = jsx.quotes.contains(&i);
            i += 1;
            while i < to && bytes[i] != byte && (direct_jsx || bytes[i] != b'\n') {
                i += if !direct_jsx && bytes[i] == b'\\' {
                    2
                } else {
                    1
                };
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
        untraceable.extend(pattern_bindings(text, tokens));
        let traceable = consts
            .into_iter()
            .filter(|(name, ranges)| ranges.len() == 1 && !untraceable.contains(name))
            .map(|(name, ranges)| (name, ranges[0]))
            .collect();
        Self { traceable }
    }
}

/// Every identifier inside a parameter list, a single arrow parameter, a
/// `catch` binding, or a destructuring declaration. Defaults and type names
/// are included too; over-collecting only leaves a constant untraced.
fn pattern_bindings(text: &str, tokens: &[Token]) -> BTreeSet<String> {
    let word = |token: &Token| &text[token.start..token.end];
    let mut bound = BTreeSet::new();
    let mark = |from: usize, to: usize, bound: &mut BTreeSet<String>| {
        for token in &tokens[from..to.min(tokens.len())] {
            if token.kind == Kind::Ident {
                bound.insert(word(token).to_owned());
            }
        }
    };
    for (index, token) in tokens.iter().enumerate() {
        let spelling = word(token);
        let next = tokens.get(index + 1).map(word);
        if token.kind == Kind::Ident && next == Some("=>") {
            bound.insert(spelling.to_owned());
            continue;
        }
        if matches!(spelling, "const" | "let" | "var") && matches!(next, Some("{" | "[")) {
            mark(index + 1, matching(text, tokens, index + 1), &mut bound);
            continue;
        }
        if spelling != "(" {
            continue;
        }
        let close = matching(text, tokens, index);
        let after = tokens.get(close + 1).map(word);
        let before = index.checked_sub(1).map(|at| &tokens[at]);
        let before_word = before.map(word);
        let declares = matches!(after, Some("=>"))
            || matches!(before_word, Some("function" | "catch"))
            || index >= 2 && word(&tokens[index - 2]) == "function"
            // A method shorthand `name(params) {`, but not a control statement.
            || (matches!(after, Some("{" | ":"))
                && before.is_some_and(|token| {
                    token.kind == Kind::Ident
                        && !matches!(
                            word(token),
                            "if" | "for" | "while" | "switch" | "with" | "return"
                        )
                }));
        if declares {
            mark(index + 1, close, &mut bound);
        }
    }
    bound
}

/// The index of the bracket closing the one at `open`.
fn matching(text: &str, tokens: &[Token], open: usize) -> usize {
    let mut depth = 0;
    for (offset, token) in tokens[open..].iter().enumerate() {
        match &text[token.start..token.end] {
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
    tokens.len()
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
    class_helpers: BTreeSet<String>,
    jsx: &'a JsxSpans,
}

impl<'a> Module<'a> {
    pub(super) fn new(
        text: &'a str,
        base: usize,
        class_helpers: BTreeSet<String>,
        jsx: &'a JsxSpans,
    ) -> Self {
        let tokens = tokenize(text, 0, text.len(), jsx);
        let bindings = Bindings::collect(text, &tokens);
        Self {
            text,
            base,
            tokens,
            bindings,
            class_helpers,
            jsx,
        }
    }

    fn word(&self, token: &Token) -> &'a str {
        &self.text[token.start..token.end]
    }

    /// Walks every `class={...}` / `className={...}` expression container.
    pub(super) fn scan(&self, out: &mut Collector<'_>) {
        self.scan_owned_factories(out);
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

    /// Only a named, value import from the owned renderer proves that a call
    /// constructs a zudo-react description. A name alone is not sufficient.
    fn owned_factory_names(&self) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        let tokens = &self.tokens;
        let word = |at: usize| tokens.get(at).map(|token| self.word(token));
        for index in 0..tokens.len() {
            if word(index) != Some("import") || word(index + 1) == Some("type") {
                continue;
            }
            let Some(open) = (index + 1..tokens.len())
                .take_while(|&at| !matches!(word(at), Some(";" | "from")))
                .find(|&at| word(at) == Some("{"))
            else {
                continue;
            };
            let close = self.matching(open);
            if word(close + 1) != Some("from")
                || !matches!(
                    word(close + 2),
                    Some("\"@takazudo/zfb/zudo-react\"" | "'@takazudo/zfb/zudo-react'")
                )
            {
                continue;
            }
            let mut at = open + 1;
            while at < close {
                let end = (at..close).find(|&n| word(n) == Some(",")).unwrap_or(close);
                let typed = word(at) == Some("type");
                let first = at + usize::from(typed);
                if !typed && word(first) == Some("h") {
                    let alias = if word(first + 1) == Some("as") {
                        first + 2
                    } else {
                        first
                    };
                    if alias < end && tokens[alias].kind == Kind::Ident {
                        names.insert(self.word(&tokens[alias]).to_owned());
                    }
                }
                at = end + 1;
            }
        }
        let shadowed = pattern_bindings(self.text, tokens);
        names.retain(|name| {
            !shadowed.contains(name)
                && !tokens.iter().enumerate().any(|(at, token)| {
                    self.word(token) == name
                        && (at > 0
                            && matches!(
                                word(at - 1),
                                Some("const" | "let" | "var" | "function" | "class")
                            )
                            || matches!(
                                word(at + 1),
                                Some("=" | "+=" | "||=" | "&&=" | "??=" | "++" | "--")
                            ))
                })
        });
        names
    }

    fn scan_owned_factories(&self, out: &mut Collector<'_>) {
        let names = self.owned_factory_names();
        if names.is_empty() {
            return;
        }
        let tokens = &self.tokens;
        for at in 0..tokens.len().saturating_sub(1) {
            if tokens[at].kind != Kind::Ident
                || !names.contains(self.word(&tokens[at]))
                || self.word(&tokens[at + 1]) != "("
                || (at > 0 && matches!(self.word(&tokens[at - 1]), "." | "?." | "new" | "function"))
            {
                continue;
            }
            let close = self.matching(at + 1);
            if close >= tokens.len() {
                continue;
            }
            let Some(comma) = self.top_level_comma(at + 2, close) else {
                continue;
            };
            let object = comma + 1;
            if object >= close || self.word(&tokens[object]) != "{" {
                continue;
            }
            let object_close = self.matching(object);
            if object_close >= close || !matches!(self.word(&tokens[object_close + 1]), "," | ")") {
                continue;
            }
            let mut field = object + 1;
            while field < object_close {
                let end = self
                    .top_level_comma(field, object_close)
                    .unwrap_or(object_close);
                if field + 1 < end
                    && self.class_key(&tokens[field])
                    && self.word(&tokens[field + 1]) == ":"
                {
                    let mut visiting = BTreeSet::new();
                    self.expression(&tokens[field + 2..end], out, &mut visiting);
                }
                field = end + 1;
            }
        }
    }

    fn class_key(&self, token: &Token) -> bool {
        let spelling = self.word(token);
        matches!(
            spelling,
            "class" | "className" | "\"class\"" | "'class'" | "\"className\"" | "'className'"
        )
    }

    fn top_level_comma(&self, from: usize, to: usize) -> Option<usize> {
        let mut at = from;
        while at < to {
            match self.word(&self.tokens[at]) {
                "(" | "[" | "{" => at = self.matching(at).saturating_add(1),
                "," => return Some(at),
                _ => at += 1,
            }
        }
        None
    }

    fn matching(&self, open: usize) -> usize {
        matching(self.text, &self.tokens, open)
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
                            if p.kind == Kind::Ident
                                && self.class_helpers.contains(self.word(&p)) =>
                        {
                            Container::Helper
                        }
                        Some(p)
                            if matches!(p.kind, Kind::Ident | Kind::Str | Kind::Template)
                                || matches!(self.word(&p), ")" | "]") =>
                        {
                            Container::Opaque
                        }
                        // A group whose result is compared is a condition,
                        // not a class value: `(v || "a") === b ? ...`.
                        _ if previous.is_some_and(|p| self.compares(&p))
                            || tokens
                                .get(matching(self.text, tokens, index) + 1)
                                .is_some_and(|n| self.compares(n)) =>
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

    fn compares(&self, token: &Token) -> bool {
        matches!(
            self.word(token),
            "===" | "!==" | "==" | "!=" | "<" | ">" | "<=" | ">=" | "in" | "instanceof"
        )
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
        // An unterminated string runs to the end of the text with no closing
        // quote (or line break) to strip.
        let bytes = self.text.as_bytes();
        let last = bytes[token.end - 1];
        let value_end = if last == bytes[token.start] || last == b'\n' {
            token.end - 1
        } else {
            token.end
        };
        emit(
            self.text,
            self.base,
            out,
            token.start + 1,
            value_end,
            token.start,
            token.end,
            true,
            false,
            false,
            false,
            true,
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
                        true,
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
                        true,
                    );
                    let end = interpolation_end(self.text, i + 2, token.end);
                    let body_end = if self.text[..end].ends_with('}') {
                        end - 1
                    } else {
                        end
                    };
                    let inner = tokenize(self.text, i + 2, body_end.max(i + 2), self.jsx);
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
