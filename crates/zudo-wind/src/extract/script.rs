mod class_expression;

use super::{Collector, NoteKind, PositionKind};

/// Scans every literal, then upgrades the complete strings whose value
/// provably reaches a class attribute to class positions.
pub(super) fn scan(source: &str, base: usize, out: &mut Collector<'_>) {
    scan_literals(source, base, out);
    class_expression::Module::new(source, base).scan(out);
}

fn scan_literals(source: &str, base: usize, out: &mut Collector<'_>) {
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"//") {
            i = source[i..].find('\n').map_or(bytes.len(), |n| i + n);
            continue;
        }
        if bytes[i..].starts_with(b"/*") {
            i = source[i + 2..]
                .find("*/")
                .map_or(bytes.len(), |n| i + n + 4);
            continue;
        }
        if bytes[i] == b'\'' || bytes[i] == b'"' || bytes[i] == b'`' {
            let quote = bytes[i];
            let class = class_context(&source[..i]);
            let known_non_class = !class && known_non_class_context(&source[..i]);
            let open = i;
            i += 1;
            let mut segment = i;
            let mut left = false;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i = (i + 2).min(bytes.len());
                    continue;
                }
                if quote == b'`' && bytes[i..].starts_with(b"${") {
                    if !known_non_class {
                        emit(
                            source,
                            base,
                            out,
                            segment,
                            i,
                            open,
                            i + 2,
                            class,
                            false,
                            true,
                            left,
                        );
                    }
                    let expression_start = i + 2;
                    i = expression_end(source, expression_start);
                    // An unterminated interpolation runs to the end of the
                    // source and has no closing `}` to strip.
                    let body_end = if source[..i].ends_with('}') { i - 1 } else { i };
                    if body_end > expression_start {
                        scan_literals(
                            &source[expression_start..body_end],
                            base + expression_start,
                            out,
                        );
                    }
                    segment = i;
                    left = true;
                    continue;
                }
                if bytes[i] == quote {
                    let right = source[i + 1..].trim_start().starts_with('+');
                    let left = left || source[..open].trim_end().ends_with('+');
                    let whole_url = !class && !left && !right && is_pure_url(&source[segment..i]);
                    if !known_non_class && !whole_url {
                        emit(
                            source,
                            base,
                            out,
                            segment,
                            i,
                            open,
                            i + 1,
                            class,
                            quote != b'`',
                            right,
                            left,
                        );
                    }
                    i += 1;
                    break;
                }
                if quote != b'`' && bytes[i] == b'\n' {
                    break;
                }
                i += 1;
            }
            if i >= bytes.len() && (bytes.last() != Some(&quote)) {
                out.note(
                    NoteKind::UnterminatedLiteral,
                    base + open,
                    "unterminated string literal",
                );
            }
            continue;
        }
        // A regex literal can hold quotes and comment markers. Skip it after expression starters.
        if bytes[i] == b'/' && regex_context(&source[..i]) {
            i += 1;
            let mut bracket = false;
            while i < bytes.len() && bytes[i] != b'\n' {
                if bytes[i] == b'\\' {
                    i = (i + 2).min(bytes.len());
                    continue;
                }
                if bytes[i] == b'[' {
                    bracket = true;
                }
                if bytes[i] == b']' {
                    bracket = false;
                }
                if bytes[i] == b'/' && !bracket {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        i += source[i..].chars().next().map_or(1, char::len_utf8);
    }
}

fn class_context(prefix: &str) -> bool {
    let trimmed = prefix.trim_end();
    let trimmed = trimmed.strip_suffix('{').unwrap_or(trimmed).trim_end();
    let Some(before) = trimmed.strip_suffix('=') else {
        return false;
    };
    let name = before
        .trim_end()
        .rsplit(|c: char| !c.is_ascii_alphanumeric())
        .next()
        .unwrap_or("");
    if name != "class" && name != "className" {
        return false;
    }
    let preceding_word = before
        .trim_end()
        .strip_suffix(name)
        .unwrap_or("")
        .trim_end()
        .rsplit(|c: char| !c.is_ascii_alphanumeric())
        .next()
        .unwrap_or("");
    !matches!(preceding_word, "const" | "let" | "var")
}

/// Attributes whose value is never a class list: links, sources, metadata
/// and input types (`type="hidden"` is not the `hidden` utility).
const NON_CLASS_ATTRIBUTES: &[&str] = &["href", "src", "content", "name", "rel", "type"];

/// A literal that is a module specifier (`import`/`export … from`,
/// `import()`, `require()`) or an intrinsic non-class attribute value.
fn known_non_class_context(prefix: &str) -> bool {
    let trimmed = prefix.trim_end();
    let word = |text: &str| {
        text.rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
            .next()
            .unwrap_or("")
            .to_owned()
    };
    if let Some(call) = trimmed.strip_suffix('(') {
        let callee = word(call.trim_end());
        return callee == "require"
            || (callee == "import" && !call.trim_end().ends_with(".import"));
    }
    if matches!(word(trimmed).as_str(), "from" | "import") {
        return true;
    }
    if style_text_context(trimmed) {
        return true;
    }
    let attribute = trimmed.strip_suffix('{').unwrap_or(trimmed).trim_end();
    let Some(before) = attribute.strip_suffix('=') else {
        return false;
    };
    let before = before.trim_end();
    let name = word(before);
    if !NON_CLASS_ATTRIBUTES.contains(&name.as_str()) {
        return false;
    }
    // `const name = "..."` is a variable, not an attribute.
    let preceding = word(before.strip_suffix(name.as_str()).unwrap_or("").trim_end());
    !matches!(preceding.as_str(), "const" | "let" | "var")
}

/// CSS text of a `<style>` element: `<style>{`…`}</style>` or the
/// `rawHtml` attribute of a `<style>` tag.
fn style_text_context(trimmed: &str) -> bool {
    let tag_start = |text: &str| {
        text.rfind('<')
            .is_some_and(|open| text[open + 1..].trim_start().starts_with("style"))
    };
    if let Some(child) = trimmed.strip_suffix('{') {
        let child = child.trim_end();
        if let Some(tag) = child.strip_suffix('>') {
            return tag_start(tag) && !tag.ends_with('/');
        }
        if let Some(attribute) = child.strip_suffix('=') {
            return attribute.trim_end().ends_with("rawHtml")
                && tag_start(attribute)
                && !attribute[attribute.rfind('<').unwrap_or(0)..].contains('>');
        }
    }
    false
}

/// A whole literal that is only a URL or a relative path.
fn is_pure_url(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(char::is_whitespace)
        && [
            "http://", "https://", "//", "mailto:", "tel:", "data:", "blob:", "file:", "node:",
            "./", "../",
        ]
        .iter()
        .any(|prefix| value.starts_with(prefix))
}

fn regex_context(prefix: &str) -> bool {
    let trimmed = prefix.trim_end();
    matches!(
        trimmed.as_bytes().last(),
        Some(b'=') | Some(b'(') | Some(b',') | Some(b':') | Some(b'[')
    ) || trimmed
        .split(|c: char| !c.is_ascii_alphanumeric())
        .next_back()
        .is_some_and(|word| matches!(word, "return" | "throw" | "case" | "yield"))
}

fn expression_end(source: &str, start: usize) -> usize {
    let bytes = source.as_bytes();
    let mut i = start;
    let mut depth = 1;
    while i < bytes.len() {
        if matches!(bytes[i], b'\'' | b'"' | b'`') {
            let q = bytes[i];
            i += 1;
            while i < bytes.len() && bytes[i] != q {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
        } else if bytes[i] == b'{' {
            depth += 1;
        } else if bytes[i] == b'}' {
            depth -= 1;
            if depth == 0 {
                return i + 1;
            }
        }
        i += 1;
    }
    bytes.len()
}

#[allow(clippy::too_many_arguments)]
fn emit(
    source: &str,
    base: usize,
    out: &mut Collector<'_>,
    start: usize,
    end: usize,
    open: usize,
    close: usize,
    class: bool,
    embedded: bool,
    right: bool,
    left: bool,
) {
    if start >= end {
        return;
    }
    let raw = &source[start..end];
    let (decoded, source_map) = decode(raw);
    let kind = if class {
        PositionKind::Class
    } else {
        PositionKind::Literal
    };
    out.tokens(
        &decoded,
        base + start,
        (base + open, close - open),
        kind,
        right,
        left,
        Some(&source_map),
    );
    if embedded && (raw.contains("class=") || raw.contains("className=")) {
        out.within_decoded(base + start, source_map, |out| {
            super::markup::scan(&decoded, 0, out, false)
        });
    }
}

/// Decodes the JavaScript escapes of a string literal body. The map holds,
/// for every decoded byte, the raw offset of the character or escape that
/// produced it, then `raw.len()`.
fn decode(raw: &str) -> (String, Vec<usize>) {
    let mut result = String::new();
    let mut offsets = Vec::new();
    let mut at = 0;
    while let Some(c) = raw[at..].chars().next() {
        let (decoded, width) = if c == '\\' {
            let (decoded, width) = escape(&raw[at + 1..]);
            (decoded, 1 + width)
        } else {
            (Some(c), c.len_utf8())
        };
        if let Some(decoded) = decoded {
            result.push(decoded);
            offsets.extend(std::iter::repeat_n(at, decoded.len_utf8()));
        }
        at += width;
    }
    offsets.push(raw.len());
    (result, offsets)
}

/// The character an escape after its backslash stands for, and the escape's
/// byte width. A line continuation stands for nothing.
fn escape(rest: &str) -> (Option<char>, usize) {
    let Some(next) = rest.chars().next() else {
        return (None, 0);
    };
    let hex = |digits: &str| {
        (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| u32::from_str_radix(digits, 16).ok())
            .flatten()
    };
    match next {
        'n' => (Some('\n'), 1),
        't' => (Some('\t'), 1),
        'r' => (Some('\r'), 1),
        '\r' if rest[1..].starts_with('\n') => (None, 2),
        '\r' | '\n' | '\u{2028}' | '\u{2029}' => (None, next.len_utf8()),
        'x' => match rest.get(1..3).and_then(hex) {
            Some(code) => (char::from_u32(code), 3),
            None => (Some('x'), 1),
        },
        'u' if rest[1..].starts_with('{') => match rest[2..].find('}') {
            Some(close) if close <= 6 => match hex(&rest[2..2 + close]).and_then(char::from_u32) {
                Some(ch) => (Some(ch), close + 3),
                None => (Some('u'), 1),
            },
            _ => (Some('u'), 1),
        },
        'u' => match rest.get(1..5).and_then(hex) {
            Some(high @ 0xD800..=0xDBFF) => {
                let low = rest
                    .get(5..7)
                    .filter(|marker| *marker == "\\u")
                    .and_then(|_| rest.get(7..11))
                    .and_then(hex)
                    .filter(|low| (0xDC00..=0xDFFF).contains(low));
                match low {
                    Some(low) => (
                        char::from_u32(0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)),
                        11,
                    ),
                    None => (Some(char::REPLACEMENT_CHARACTER), 5),
                }
            }
            Some(code) => (
                Some(char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER)),
                5,
            ),
            None => (Some('u'), 1),
        },
        other => (Some(other), other.len_utf8()),
    }
}
