use super::{Collector, PositionKind};

/// `interpolations` is set for markup carried inside a JS string, where a
/// `${...}` is template interpolation rather than attribute text (#4647).
pub(super) fn scan(
    source: &str,
    base: usize,
    out: &mut Collector<'_>,
    scripts: bool,
    interpolations: bool,
) -> bool {
    let bytes = source.as_bytes();
    let mut i = 0;
    let mut class_attribute = false;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"<!--") {
            i = source[i + 4..]
                .find("-->")
                .map_or(bytes.len(), |n| i + n + 7);
            continue;
        }
        if bytes[i] != b'<' {
            i += source[i..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        if scripts && source[i..].to_ascii_lowercase().starts_with("<script") {
            if let Some(open) = source[i..].find('>') {
                let start = i + open + 1;
                if let Some(close) = source[start..].to_ascii_lowercase().find("</script") {
                    super::script::scan(&source[start..start + close], base + start, out);
                    i = start + close;
                    continue;
                }
            }
        }
        let tag_start = i;
        i += 1;
        if i >= bytes.len() || !(bytes[i].is_ascii_alphabetic() || bytes[i] == b'/') {
            continue;
        }
        let mut quote = None;
        let mut brace = 0usize;
        while i < bytes.len() {
            let b = bytes[i];
            if let Some(q) = quote {
                if b == q && (i == 0 || bytes[i - 1] != b'\\') {
                    quote = None;
                }
            } else if b == b'\'' || b == b'"' || b == b'`' {
                quote = Some(b);
            } else if b == b'{' {
                brace += 1;
            } else if b == b'}' {
                brace = brace.saturating_sub(1);
            } else if b == b'>' && brace == 0 {
                break;
            }
            i += 1;
        }
        let tag_end = i.min(bytes.len());
        class_attribute |= attributes(
            &source[tag_start..tag_end],
            base + tag_start,
            out,
            interpolations,
        );
        i = (tag_end + 1).min(bytes.len());
    }
    class_attribute
}

fn attributes(tag: &str, base: usize, out: &mut Collector<'_>, interpolations: bool) -> bool {
    let bytes = tag.as_bytes();
    let mut i = 1;
    let mut class_attribute = false;
    while i < bytes.len() {
        while i < bytes.len() && !(bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
            i += 1;
        }
        let start = i;
        while i < bytes.len()
            && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'-' | b'_' | b':'))
        {
            i += 1;
        }
        if start == i {
            break;
        }
        let name = &tag[start..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let brace = bytes[i] == b'{';
        if brace {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let q = bytes[i];
        if !matches!(q, b'\'' | b'"' | b'`') {
            continue;
        }
        let open = i;
        i += 1;
        let value_start = i;
        while i < bytes.len() && bytes[i] != q {
            if bytes[i] == b'\\' {
                i += 1;
            }
            i += 1;
        }
        let value_end = i.min(bytes.len());
        if name == "class" || name == "className" {
            class_attribute = true;
            // An unterminated value has no closing quote inside the tag.
            let close = if value_end < bytes.len() {
                value_end + 1
            } else {
                value_end
            };
            let value = &tag[value_start..value_end];
            let segments = if interpolations && value.contains("${") {
                static_segments(value, base + value_start, out)
            } else {
                vec![(0, value.len())]
            };
            let last = segments.len() - 1;
            for (index, (start, end)) in segments.into_iter().enumerate() {
                let (decoded, offsets) = entities(&value[start..end]);
                let source_map: Vec<usize> = offsets.into_iter().map(|at| start + at).collect();
                out.tokens(
                    &decoded,
                    base + value_start,
                    (base + open, close - open),
                    PositionKind::Class,
                    index < last,
                    index > 0,
                    Some(&source_map),
                );
            }
        }
        i = (i + 1).min(bytes.len());
        if brace && i < bytes.len() && bytes[i] == b'}' {
            i += 1;
        }
    }
    class_attribute
}

/// Splits a class value at each unescaped `${...}` that closes inside it,
/// scanning each body for literals, and returns the byte ranges of the static
/// text between them. A `${` that is escaped or never closes stays text.
fn static_segments(value: &str, base: usize, out: &mut Collector<'_>) -> Vec<(usize, usize)> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while let Some(found) = value[i..].find("${") {
        let open = i + found;
        let body = open + 2;
        let close = (!value[..open].ends_with('\\'))
            .then(|| super::script::interpolation_close(value, body))
            .flatten();
        let Some(close) = close else {
            i = body;
            continue;
        };
        segments.push((start, open));
        if close - 1 > body {
            super::script::scan_interpolation(&value[body..close - 1], base + body, out);
        }
        start = close;
        i = close;
    }
    segments.push((start, value.len()));
    segments
}

fn entities(value: &str) -> (String, Vec<usize>) {
    let mut decoded = String::new();
    let mut offsets = Vec::new();
    let mut i = 0;
    while i < value.len() {
        if value.as_bytes()[i] == b'&' {
            if let Some(end) = value[i..].find(';').filter(|&n| n <= 12) {
                let entity = &value[i + 1..i + end];
                let replacement = match entity {
                    "amp" => Some('&'),
                    "quot" => Some('"'),
                    "apos" | "#39" => Some('\''),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "nbsp" | "#32" => Some(' '),
                    "#9" => Some('\t'),
                    "#10" => Some('\n'),
                    "#34" => Some('"'),
                    _ => entity
                        .strip_prefix("#x")
                        .and_then(|n| u32::from_str_radix(n, 16).ok())
                        .and_then(char::from_u32)
                        .or_else(|| {
                            entity
                                .strip_prefix('#')
                                .and_then(|n| n.parse::<u32>().ok())
                                .and_then(char::from_u32)
                        }),
                };
                if let Some(ch) = replacement {
                    decoded.push(ch);
                    offsets.extend(std::iter::repeat_n(i, ch.len_utf8()));
                    i += end + 1;
                    continue;
                }
            }
        }
        let ch = value[i..].chars().next().expect("valid UTF-8");
        decoded.push(ch);
        offsets.extend(std::iter::repeat_n(i, ch.len_utf8()));
        i += ch.len_utf8();
    }
    offsets.push(value.len());
    (decoded, offsets)
}
