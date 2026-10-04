//! Offsets found while scanning a decoded JavaScript string (and the HTML
//! embedded in it) must point back at the raw source, on char boundaries.
use zudo_wind::{extract_candidates, ExtractionResult, NoteKind, PositionKind, SourceKind};

const KINDS: [SourceKind; 8] = [
    SourceKind::Tsx,
    SourceKind::Ts,
    SourceKind::Jsx,
    SourceKind::Js,
    SourceKind::Mjs,
    SourceKind::Mdx,
    SourceKind::Md,
    SourceKind::Html,
];

fn line_column(source: &str, at: usize) -> (usize, usize) {
    let prefix = &source[..at];
    let line_start = prefix.rfind('\n').map_or(0, |i| i + 1);
    (prefix.matches('\n').count() + 1, at - line_start + 1)
}

/// Every reported position is a char boundary of `source` whose line and
/// column agree with its byte offset.
fn assert_positions(source: &str, result: &ExtractionResult) {
    let boundary = |at: usize| at <= source.len() && source.is_char_boundary(at);
    for candidate in &result.candidates {
        for o in &candidate.occurrences {
            let end = o.byte_offset + o.byte_length;
            let literal_end = o.literal_byte_offset + o.literal_byte_length;
            assert!(
                boundary(o.byte_offset) && boundary(end),
                "{candidate:?} in {source:?}"
            );
            assert!(
                boundary(o.literal_byte_offset) && boundary(literal_end),
                "{candidate:?} in {source:?}"
            );
            assert_eq!(
                (o.line, o.byte_column),
                line_column(source, o.byte_offset),
                "{candidate:?} in {source:?}"
            );
        }
    }
    for note in &result.notes {
        assert!(boundary(note.byte_offset), "{note:?} in {source:?}");
        assert_eq!(
            (note.line, note.byte_column),
            line_column(source, note.byte_offset),
            "{note:?} in {source:?}"
        );
    }
}

fn class_occurrence(result: &ExtractionResult, text: &str) -> zudo_wind::Occurrence {
    result
        .candidates
        .iter()
        .find(|candidate| candidate.text == text)
        .and_then(|candidate| {
            candidate
                .occurrences
                .iter()
                .find(|o| o.position_kind == PositionKind::Class)
        })
        .cloned()
        .unwrap_or_else(|| panic!("no class occurrence of {text:?} in {result:?}"))
}

/// The #3569 shape: a generated `export default ${JSON.stringify(records)}`
/// module whose highlighted HTML holds a raw em dash before `class="line"`.
#[test]
fn json_module_em_dash_before_embedded_class() {
    let source = concat!(
        "const meta = 1;\n",
        r#"export default [{"html":"<span>Custom Components&quot; guide — it is\n</span></span><span class=\"line\" style=\"--shiki:#fff\"><span class=\"tail-\">x</span></span>"}];"#,
    );
    let result = extract_candidates(source.as_bytes(), SourceKind::Tsx);
    assert_positions(source, &result);

    let line = class_occurrence(&result, "line");
    let open = source.find(r#"class=\"line"#).unwrap() + r#"class="#.len();
    assert_eq!(line.byte_offset, open + 2);
    assert_eq!(&source[line.byte_offset..][..line.byte_length], "line");
    assert_eq!((line.line, line.byte_column), line_column(source, open + 2));
    assert_eq!(line.literal_byte_offset, open);
    assert_eq!(
        &source[open..open + line.literal_byte_length],
        r#"\"line\""#
    );

    let tail = source.find("tail-").unwrap();
    let malformed = result
        .notes
        .iter()
        .find(|note| note.kind == NoteKind::MalformedClassCandidate && note.text == "tail-")
        .expect("malformed class note");
    assert_eq!(malformed.byte_offset, tail);
    assert_eq!((malformed.line, malformed.byte_column), (2, tail - 16 + 1));
}

/// Each `\n` escape shrinks the decoded text by a byte, so some escape count
/// puts the decoded offset of `line` inside the raw em dash (#3569's panic).
#[test]
fn escape_shrinkage_never_lands_a_decoded_offset_inside_an_em_dash() {
    for escapes in 0..24 {
        let source = format!(
            r#"export default [{{"html":"{}guide — it is<span class=\"line\">x</span>"}}];"#,
            r"\n".repeat(escapes)
        );
        let result = extract_candidates(source.as_bytes(), SourceKind::Tsx);
        assert_positions(&source, &result);
        let line = class_occurrence(&result, "line");
        assert_eq!(line.byte_offset, source.find(r#"line\""#).unwrap());
        assert_eq!(line.byte_length, 4);
    }
}

#[test]
fn escapes_before_embedded_class_map_to_raw_positions() {
    let source = concat!(
        r#"export const html = "a\\b \"q\" \n\t😀 日本 \u2014 \uD83D\uDE00 \x41 \u{1F600} "#,
        r#"<i class=\"p-2 m-1\">—</i>";"#,
        "\n"
    );
    let result = extract_candidates(source.as_bytes(), SourceKind::Js);
    assert_positions(source, &result);
    for text in ["p-2", "m-1"] {
        let found = class_occurrence(&result, text);
        assert_eq!(found.byte_offset, source.find(text).unwrap(), "{text}");
        assert_eq!(&source[found.byte_offset..][..found.byte_length], text);
    }
}

#[test]
fn unicode_escapes_decode_before_markup_is_scanned() {
    let source = r#"const html = "\u003cb class=\u0022flex\u0022\u003e—\u003c/b\u003e";"#;
    let result = extract_candidates(source.as_bytes(), SourceKind::Ts);
    assert_positions(source, &result);
    let flex = class_occurrence(&result, "flex");
    assert_eq!(flex.byte_offset, source.find("flex").unwrap());
    assert_eq!(flex.byte_length, 4);
}

#[test]
fn truncated_interpolations_ending_in_multibyte_characters() {
    for source in [
        "const x = `p-2 ${y —",
        "const x = `p-2 ${y + `${z 日",
        "const x = `p-2 ${'—",
        "<a className={`p-2 ${y 😀",
        "<a className={`p-2 ${cn(\"m-1\", y)} —",
        "<a className={\"p-2 —",
        "<a className={cn('p-2', '日",
    ] {
        for kind in KINDS {
            let result = extract_candidates(source.as_bytes(), kind);
            assert_positions(source, &result);
        }
        let result = extract_candidates(source.as_bytes(), SourceKind::Tsx);
        assert!(
            result.candidates.iter().any(|c| c.text == "p-2"),
            "{source:?}: {result:?}"
        );
    }
}

#[test]
fn malformed_input_around_interpolation_ends() {
    for source in [
        "`${",
        "`${}",
        "`${}`",
        "`${—",
        "`${'—'",
        "`${\"\\",
        "`${`${—",
        "`${}—",
        "`${{—}",
        "`a${b}—${",
        "x = `${\"}\"} p-2 —`",
        "`${/—/}`",
        "`${// —\n}`",
    ] {
        for kind in KINDS {
            let result = extract_candidates(source.as_bytes(), kind);
            assert_positions(source, &result);
        }
    }
}

const FUZZ_BASES: &[&str] = &[
    r#"export default [{"html":"<span class=\"line\">a</span><span class='p-2 m-1'>b</span>"}];"#,
    "const html = \"<b class='flex gap-2'>x</b>\";\nconst y = `p-4 ${z ? 'm-2' : \"m-3\"} px-1`;",
    "<div className={cn(\"p-2\", ok && 'm-1', `gap-${n} flex`)}>\n<p class=\"text-sm\">t</p></div>",
    "# T\n\n<div class=\"p-2 grid\">\n`code`\n</div>\n{`m-1 ${x}`}\n",
    "<p class=\"a-1 b-2\"><script>const s = \"<i class='c-3'>\";</script></p>",
];

const MULTIBYTE: &[char] = &['—', '日', '😀'];

/// Inserts each multibyte character at every char boundary of each base,
/// and truncates the result at every char boundary, scanning all source
/// kinds. Embedded markup in these bases uses unescaped single quotes, so
/// every class occurrence must also slice back to its own text.
#[test]
fn multibyte_characters_at_every_position_never_panic() {
    for base in FUZZ_BASES {
        for &ch in MULTIBYTE {
            let positions: Vec<usize> = (0..=base.len())
                .filter(|&at| base.is_char_boundary(at))
                .collect();
            for &at in &positions {
                let mut source = String::with_capacity(base.len() + 4);
                source.push_str(&base[..at]);
                source.push(ch);
                source.push_str(&base[at..]);
                check(&source);
                let cut = at + ch.len_utf8();
                check(&source[..cut]);
                check(&source[..cut - ch.len_utf8()]);
            }
        }
    }
}

fn check(source: &str) {
    for kind in KINDS {
        let result = extract_candidates(source.as_bytes(), kind);
        assert_positions(source, &result);
        if source.contains('\\') {
            continue;
        }
        for candidate in &result.candidates {
            for o in &candidate.occurrences {
                assert_eq!(
                    &source[o.byte_offset..o.byte_offset + o.byte_length],
                    candidate.text,
                    "{kind:?} {source:?}"
                );
            }
        }
    }
}
