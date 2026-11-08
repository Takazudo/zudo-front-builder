use super::{Collector, PositionKind};

pub(super) fn scan(source: &str, base: usize, out: &mut Collector<'_>, scripts: bool) -> bool {
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
        class_attribute |= attributes(&source[tag_start..tag_end], base + tag_start, out);
        i = (tag_end + 1).min(bytes.len());
    }
    class_attribute
}

fn attributes(tag: &str, base: usize, out: &mut Collector<'_>) -> bool {
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
            let (decoded, source_map) = entities(&tag[value_start..value_end]);
            // An unterminated value has no closing quote inside the tag.
            let close = if value_end < bytes.len() {
                value_end + 1
            } else {
                value_end
            };
            // A `${...}` in a class value is a template interpolation (markup
            // that a JS string carries as data, #4647): split at it like a
            // script template literal instead of reading it as class text.
            let segments = static_segments(&decoded);
            let last = segments.len() - 1;
            for (index, (start, end)) in segments.into_iter().enumerate() {
                out.tokens(
                    &decoded[start..end],
                    base + value_start,
                    (base + open, close - open),
                    PositionKind::Class,
                    index < last,
                    index > 0,
                    Some(&source_map[start..]),
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

/// The byte ranges of `value` outside `${...}` interpolations, in order. An
/// unterminated interpolation runs to the end of the value.
fn static_segments(value: &str) -> Vec<(usize, usize)> {
    let bytes = value.as_bytes();
    let mut segments = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] != b'$' || bytes[i + 1] != b'{' {
            i += 1;
            continue;
        }
        segments.push((start, i));
        let mut depth = 0usize;
        let mut quote = None;
        i += 1;
        while i < bytes.len() {
            let b = bytes[i];
            if let Some(q) = quote {
                if b == b'\\' {
                    i += 1;
                } else if b == q {
                    quote = None;
                }
            } else if matches!(b, b'\'' | b'"' | b'`') {
                quote = Some(b);
            } else if b == b'{' {
                depth += 1;
            } else if b == b'}' {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            i += 1;
        }
        i = (i + 1).min(bytes.len());
        start = i;
    }
    segments.push((start, bytes.len()));
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
