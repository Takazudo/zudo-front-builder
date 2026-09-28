use super::{Collector, NoteKind, PositionKind};

pub(super) fn scan(source: &str, base: usize, out: &mut Collector<'_>) {
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
                    let expression_start = i + 2;
                    i = expression_end(source, expression_start);
                    if i > expression_start {
                        scan(
                            &source[expression_start..i.saturating_sub(1)],
                            base + expression_start,
                            out,
                        );
                    }
                    segment = i;
                    left = true;
                    continue;
                }
                if bytes[i] == quote {
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
                        false,
                        left,
                    );
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
    name == "class" || name == "className"
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
        let counts = out.occurrence_counts();
        super::markup::scan(&decoded, base + start, out, false);
        out.remap_new_occurrences(&counts, base + start, &source_map);
    }
}

fn decode(raw: &str) -> (String, Vec<usize>) {
    let mut result = String::new();
    let mut offsets = Vec::new();
    let mut chars = raw.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let decoded = if c == '\\' {
            chars.next().map(|(_, next)| match next {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                other => other,
            })
        } else {
            Some(c)
        };
        if let Some(decoded) = decoded {
            result.push(decoded);
            offsets.extend(std::iter::repeat_n(at, decoded.len_utf8()));
        }
    }
    offsets.push(raw.len());
    (result, offsets)
}
