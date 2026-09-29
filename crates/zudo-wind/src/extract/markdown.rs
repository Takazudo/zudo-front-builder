use super::Collector;

pub(super) fn scan(source: &str, out: &mut Collector<'_>, mdx: bool) {
    let mut masked = source.as_bytes().to_vec();
    let mut offset = 0;
    let mut fence: Option<(u8, usize)> = None;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let marker = trimmed.as_bytes().first().copied();
        let count = marker.map_or(0, |m| trimmed.bytes().take_while(|&b| b == m).count());
        let is_fence = matches!(marker, Some(b'`' | b'~')) && count >= 3;
        if is_fence {
            if fence.is_none() {
                fence = Some((marker.unwrap_or_default(), count));
            } else if fence.is_some_and(|(m, n)| Some(m) == marker && count >= n) {
                fence = None;
            }
        }
        if fence.is_some() || is_fence {
            for b in &mut masked[offset..offset + line.len()] {
                if *b != b'\n' {
                    *b = b' ';
                }
            }
        }
        offset += line.len();
    }
    let mut inline: Option<usize> = None;
    let mut i = 0;
    while i < masked.len() {
        if inline.is_none()
            && masked[i] == b'<'
            && i + 1 < masked.len()
            && masked[i + 1].is_ascii_alphabetic()
        {
            let mut quote = None;
            i += 1;
            while i < masked.len() {
                let b = masked[i];
                if let Some(q) = quote {
                    if b == q && masked[i.saturating_sub(1)] != b'\\' {
                        quote = None;
                    }
                } else if matches!(b, b'\'' | b'"' | b'`') {
                    quote = Some(b);
                } else if b == b'>' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if masked[i] == b'`' {
            let count = masked[i..].iter().take_while(|&&b| b == b'`').count();
            if inline.is_none() {
                inline = Some(count);
            } else if inline == Some(count) {
                inline = None;
            }
            for byte in &mut masked[i..i + count] {
                *byte = b' ';
            }
            i += count;
            continue;
        }
        if inline.is_some() && masked[i] != b'\n' {
            masked[i] = b' ';
        }
        i += 1;
    }
    let clean = String::from_utf8(masked).expect("masked UTF-8 source");
    super::markup::scan(&clean, 0, out, false);
    if mdx {
        let bytes = clean.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'{' {
                let start = i + 1;
                let mut depth = 1;
                i += 1;
                while i < bytes.len() && depth > 0 {
                    if bytes[i] == b'{' {
                        depth += 1;
                    }
                    if bytes[i] == b'}' {
                        depth -= 1;
                    }
                    i += 1;
                }
                if depth == 0 {
                    super::script::scan(&clean[start..i - 1], start, out);
                }
            } else {
                i += 1;
            }
        }
    }
}
