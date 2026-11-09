use super::{Collector, PositionKind};

/// `carried` is set for markup carried inside a JS string. Its `<script>`
/// bodies are JS source, so a class attribute written in a template literal
/// there reads `${...}` as interpolation rather than class text (#4647).
pub(super) fn scan(
    source: &str,
    base: usize,
    out: &mut Collector<'_>,
    scripts: bool,
    carried: bool,
) -> bool {
    scan_inner(source, base, out, scripts, carried, None)
}

/// A decoded JavaScript template body with its actual interpolation spans.
/// Unlike carried HTML, its expression bodies are scanned by the JS caller.
pub(super) fn scan_template(
    source: &str,
    out: &mut Collector<'_>,
    spans: &[(usize, usize)],
) -> bool {
    scan_inner(source, 0, out, false, false, Some(spans))
}

#[derive(Clone, Copy)]
enum Interpolations<'a> {
    None,
    Carried,
    Template {
        spans: &'a [(usize, usize)],
        offset: usize,
    },
}

impl Interpolations<'_> {
    fn at(self, text: &str, at: usize) -> Option<usize> {
        match self {
            Self::None => None,
            Self::Carried => interpolation_at(text, at),
            Self::Template { spans, offset } => spans
                .binary_search_by_key(&(offset + at), |&(start, _)| start)
                .ok()
                .and_then(|index| spans[index].1.checked_sub(offset))
                .filter(|&close| close <= text.len()),
        }
    }

    fn shifted(self, by: usize) -> Self {
        match self {
            Self::Template { spans, offset } => Self::Template {
                spans,
                offset: offset + by,
            },
            other => other,
        }
    }
}

fn scan_inner(
    source: &str,
    base: usize,
    out: &mut Collector<'_>,
    scripts: bool,
    carried: bool,
    template_spans: Option<&[(usize, usize)]>,
) -> bool {
    let bytes = source.as_bytes();
    let mut i = 0;
    let mut class_attribute = false;
    let mut in_script = false;
    while i < bytes.len() {
        let interpolations = match template_spans {
            Some(spans) => Interpolations::Template { spans, offset: 0 },
            None if carried && in_script => Interpolations::Carried,
            None => Interpolations::None,
        };
        if template_spans.is_some() {
            if let Some(close) = interpolations.at(source, i) {
                i = close;
                continue;
            }
        }
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
        if carried {
            if let Some(name) = tag_name(&source[tag_start..]) {
                in_script = name == "script";
            }
        }
        let mut quote = None;
        let mut brace = 0usize;
        while i < bytes.len() {
            let b = bytes[i];
            if let Some(close) = interpolations.at(source, i) {
                i = close;
                continue;
            }
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
            interpolations.shifted(tag_start),
        );
        i = (tag_end + 1).min(bytes.len());
    }
    class_attribute
}

fn attributes(
    tag: &str,
    base: usize,
    out: &mut Collector<'_>,
    interpolations: Interpolations<'_>,
) -> bool {
    let bytes = tag.as_bytes();
    let mut i = 1;
    let mut class_attribute = false;
    while i < bytes.len() {
        while i < bytes.len() && !(bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
            // Attribute-position expressions can themselves contain text
            // such as `class="`. It is not a static attribute of this tag.
            if let Some(close) = interpolations.at(tag, i) {
                i = close;
            } else {
                i += 1;
            }
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
            if let Some(close) = interpolations.at(tag, i) {
                i = close;
                continue;
            }
            // A real JS interpolation can follow a decoded backslash
            // (an even raw backslash run). Its known span stays authoritative.
            if bytes[i] == b'\\' && interpolations.at(tag, i + 1).is_none() {
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
            let segments = static_segments(
                value,
                base + value_start,
                out,
                interpolations.shifted(value_start),
            );
            let last = segments.len() - 1;
            for (index, (start, end)) in segments.into_iter().enumerate() {
                let (decoded, mut source_map) = entities(&value[start..end]);
                if start > 0 {
                    source_map.iter_mut().for_each(|at| *at += start);
                }
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

/// The lowercase name of the element a `<script>` / `</script>`-style tag at
/// the start of `tag` opens or closes; `None` for any other tag.
fn tag_name(tag: &str) -> Option<&'static str> {
    let rest = tag.strip_prefix('<')?;
    let closing = rest.starts_with('/');
    let name = rest.trim_start_matches('/');
    let is_script = name
        .get(..6)
        .is_some_and(|head| head.eq_ignore_ascii_case("script"))
        && name[6..]
            .bytes()
            .next()
            .is_none_or(|b| b.is_ascii_whitespace() || b == b'>' || b == b'/');
    match (is_script, closing) {
        (true, false) => Some("script"),
        (true, true) => Some("/script"),
        _ => None,
    }
}

/// The end of a `${...}` at `at` that reads as template interpolation: not
/// escaped by an odd run of backslashes, closed, and with no `<` in its body,
/// so a literal `${` never swallows the markup after it.
fn interpolation_at(text: &str, at: usize) -> Option<usize> {
    if !text[at..].starts_with("${") {
        return None;
    }
    let backslashes = text[..at].bytes().rev().take_while(|&b| b == b'\\').count();
    if backslashes % 2 == 1 {
        return None;
    }
    let close = super::script::interpolation_close(text, at + 2)?;
    (!text[at + 2..close].contains('<')).then_some(close)
}

/// Splits a class value at each `${...}` that reads as interpolation, scanning
/// each body for literals, and returns the byte ranges of the static text
/// between them. Any other `${` stays text.
fn static_segments(
    value: &str,
    base: usize,
    out: &mut Collector<'_>,
    interpolations: Interpolations<'_>,
) -> Vec<(usize, usize)> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while let Some(found) = value[i..].find("${") {
        let open = i + found;
        let Some(close) = interpolations.at(value, open) else {
            i = open + 2;
            continue;
        };
        segments.push((start, open));
        if matches!(interpolations, Interpolations::Carried) && close - 1 > open + 2 {
            super::script::scan_interpolation(&value[open + 2..close - 1], base + open + 2, out);
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
