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

/// #3625: the #3569 (offset panic) and #3570 (quoted island marker) payloads
/// together, with multibyte text before the class candidate.
#[test]
fn quoted_island_marker_with_multibyte_text_before_embedded_class() {
    let source = concat!(
        "const meta = 1;\n",
        r#"export default [{"html":"<span class=\"line\">grep -roh 'data-zfb-island=&quot;[^&quot;]*&quot;' dist/ \u2014 日本語 🎉</span>"}];"#,
    );
    let result = extract_candidates(source.as_bytes(), SourceKind::Tsx);
    assert_positions(source, &result);

    let line = class_occurrence(&result, "line");
    let open = source.find(r#"class=\"line"#).unwrap() + r#"class="#.len();
    assert_eq!(line.byte_offset, open + 2);
    assert_eq!(&source[line.byte_offset..][..line.byte_length], "line");
    assert_eq!((line.line, line.byte_column), line_column(source, open + 2));
    assert_eq!(line.literal_byte_offset, open);
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

#[test]
fn positions_across_raw_newlines_and_multibyte_prefixes() {
    let source = "—\n<div class='line'>日本語</div>\n\n😀<div class='block'>x</div>\n";
    let result = extract_candidates(source.as_bytes(), SourceKind::Html);
    assert_positions(source, &result);
    for (text, line) in [("line", 2), ("block", 4)] {
        let occurrence = class_occurrence(&result, text);
        assert_eq!(occurrence.byte_offset, source.find(text).unwrap());
        assert_eq!(occurrence.line, line);
    }
}

#[test]
fn repeated_generated_html_classes_keep_raw_positions() {
    let mut source = String::from("// — 日本語\nexport default [");
    for index in 0..500 {
        if index != 0 {
            source.push(',');
        }
        let html =
            format!("<span>guide — 日本語 \\n</span><span class=\"line block\">{index}</span>");
        source.push_str("{\"body\":");
        source.push_str(&serde_json::to_string(&html).unwrap());
        source.push('}');
    }
    source.push_str("];\n");
    let result = extract_candidates(source.as_bytes(), SourceKind::Ts);
    assert_positions(&source, &result);
    for text in ["line", "block"] {
        let candidate = result.candidates.iter().find(|c| c.text == text).unwrap();
        let classes: Vec<_> = candidate
            .occurrences
            .iter()
            .filter(|o| o.position_kind == PositionKind::Class)
            .collect();
        assert_eq!(classes.len(), 500, "{text}");
        assert!(classes.iter().all(|o| o.line == 2), "{text}");
        assert!(classes
            .iter()
            .all(|o| &source[o.byte_offset..o.byte_offset + o.byte_length] == text));
    }
}

/// #4647: a plugin data module that inlines a whole HTML app as a JSON string.
/// A class attribute written in a template literal of the app's `<script>`
/// holds `${...}` interpolations; they split the value like a template literal
/// does, their bodies are scanned for literals, and they never reach the
/// compiler as part of a class candidate.
#[test]
fn interpolation_in_embedded_script_class_value_splits_the_candidates() {
    let source = concat!(
        "export default {\"assets/app.html\":",
        r#""<p class=\"intro\">x</p><script>const l = `<text class=\"label label-${kind}\">x</text><li class=\"${s.selected===q.uid?'is-selected':''} item\">`;const i = `<i class=\"text-lg${x}\"><b class=\"${on ? \"is-on\" : \"\"} card\">`;</script>"};"#,
    );
    let result = extract_candidates(source.as_bytes(), SourceKind::Js);
    assert_positions(source, &result);
    let found: Vec<_> = result.candidates.iter().map(|c| c.text.as_str()).collect();
    assert!(!found.iter().any(|c| c.contains("${")), "{found:?}");
    assert!(!found.contains(&"label-"), "{found:?}");

    for (class, at) in [
        ("intro", source.find("intro").unwrap()),
        ("label", source.find("label label-").unwrap()),
        ("item", source.find("''} item").unwrap() + 4),
        ("card", source.find(r#"\"\"} card"#).unwrap() + 6),
    ] {
        let occurrence = class_occurrence(&result, class);
        assert_eq!(occurrence.byte_offset, at, "{class}");
        assert!(!occurrence.adjacent_interpolation, "{class}");
    }
    assert!(class_occurrence(&result, "text-lg").adjacent_interpolation);
    for literal in ["is-selected", "is-on"] {
        let occurrence = result
            .candidates
            .iter()
            .find(|c| c.text == literal)
            .map(|c| (c.occurrences[0].byte_offset, c.occurrences[0].position_kind));
        assert_eq!(
            occurrence,
            Some((source.find(literal).unwrap(), PositionKind::Literal)),
            "{literal}: {found:?}"
        );
    }
}

/// #4658: the standalone HTML script path must retain the class attribute
/// across a template interpolation, including complete classes after it.
#[test]
fn standalone_script_template_preserves_static_classes_after_interpolation() {
    let source = r#"<script>const html = `<div class="flex ${on} block"></div>`;</script>"#;
    let result = extract_candidates(source.as_bytes(), SourceKind::Html);
    assert_positions(source, &result);
    for class in ["flex", "block"] {
        let occurrence = class_occurrence(&result, class);
        assert_eq!(occurrence.byte_offset, source.find(class).unwrap());
        assert_eq!(occurrence.byte_length, class.len());
        assert!(!occurrence.adjacent_interpolation, "{class}");
    }
}

#[test]
fn script_template_markup_maps_escapes_and_keeps_dynamic_boundaries() {
    let script = r#"const html = `\n<div class=\"flex ${on ? ({ value: 'grid' }).value : `hidden ${nested ? 'inline' : 'contents'}`} block label-${kind} ${prefix}suffix text-lg${x} gap-2\"></div>`;"#;
    for (source, kind) in [
        (
            format!("— 日本語\n<script>{script}</script>"),
            SourceKind::Html,
        ),
        (format!("// — 日本語\n{script}"), SourceKind::Js),
    ] {
        let result = extract_candidates(source.as_bytes(), kind);
        assert_positions(&source, &result);
        for class in ["flex", "block", "text-lg", "gap-2"] {
            let occurrence = class_occurrence(&result, class);
            assert_eq!(
                occurrence.byte_offset,
                source.find(class).unwrap(),
                "{class}"
            );
            assert_eq!(occurrence.byte_length, class.len(), "{class}");
            assert_eq!(occurrence.line, 2, "{class}");
            assert_eq!(occurrence.adjacent_interpolation, class == "text-lg");
            let open = source.find(r#"class=\""#).unwrap() + "class=".len();
            let close = source.find(r#"gap-2\""#).unwrap() + r#"gap-2\""#.len();
            assert_eq!(occurrence.literal_byte_offset, open);
            assert_eq!(occurrence.literal_byte_length, close - open);
        }
        for literal in ["grid", "hidden", "inline", "contents"] {
            let candidate = result
                .candidates
                .iter()
                .find(|c| c.text == literal)
                .unwrap();
            assert!(
                candidate.occurrences.iter().any(|o| {
                    o.position_kind == PositionKind::Literal
                        && o.byte_offset == source.find(literal).unwrap()
                }),
                "{literal}: {result:?}"
            );
        }
        for partial in ["label-", "suffix"] {
            assert!(!result.candidates.iter().any(|c| c.text == partial));
            assert!(
                result
                    .notes
                    .iter()
                    .any(|n| { n.kind == NoteKind::DynamicConstruction && n.text == partial }),
                "{partial}: {result:?}"
            );
        }
        assert!(!result.candidates.iter().any(|c| c.text.contains("${")));
    }
}

#[test]
fn script_template_expression_markup_does_not_break_outer_attribute() {
    let source = r#"<script>const html = `<div class="flex ${on ? '<i class="grid">' : ''} block"></div>`;</script>"#;
    let result = extract_candidates(source.as_bytes(), SourceKind::Html);
    assert_positions(source, &result);
    for class in ["flex", "grid", "block"] {
        let occurrence = class_occurrence(&result, class);
        assert_eq!(occurrence.byte_offset, source.find(class).unwrap());
        assert_eq!(occurrence.byte_length, class.len());
    }
}

#[test]
fn script_template_attribute_expression_does_not_consume_static_class() {
    let source = r#"<script>const html = `<div ${on ? 'class="' : ''} class="flex ${x} block"></div>`;</script>"#;
    let result = extract_candidates(source.as_bytes(), SourceKind::Html);
    assert_positions(source, &result);
    for class in ["flex", "block"] {
        let occurrence = class_occurrence(&result, class);
        assert_eq!(occurrence.byte_offset, source.find(class).unwrap());
        assert_eq!(occurrence.byte_length, class.len());
        assert!(!occurrence.adjacent_interpolation);
    }
}

#[test]
fn script_template_interpolation_after_escaped_backslash_keeps_its_boundary() {
    let source =
        r#"<script>const html = `<div class="flex \\${on ? '"' : ''} block"></div>`;</script>"#;
    let result = extract_candidates(source.as_bytes(), SourceKind::Html);
    assert_positions(source, &result);
    for class in ["flex", "block"] {
        let occurrence = class_occurrence(&result, class);
        assert_eq!(occurrence.byte_offset, source.find(class).unwrap());
        assert_eq!(occurrence.byte_length, class.len());
        assert!(!occurrence.adjacent_interpolation);
    }
}

#[test]
fn script_template_escaped_interpolation_stays_literal_after_decoding() {
    let source =
        r#"<script>const html = `<div class="flex \${name} grid ${on} block"></div>`;</script>"#;
    let result = extract_candidates(source.as_bytes(), SourceKind::Html);
    assert_positions(source, &result);
    for class in ["flex", "grid", "block"] {
        let occurrence = class_occurrence(&result, class);
        assert_eq!(occurrence.byte_offset, source.find(class).unwrap());
        assert!(!occurrence.adjacent_interpolation);
    }
    assert!(
        result.notes.iter().any(|note| {
            note.kind == NoteKind::MalformedClassCandidate && note.text == "${name}"
        }),
        "{result:?}"
    );
    assert!(!result.candidates.iter().any(|c| c.text == "name"));
}

/// Outside a `<script>` of JS-carried markup, and in plain HTML, `${` in a
/// class attribute is literal text: it stays a malformed class candidate for
/// the author to see.
#[test]
fn interpolation_syntax_outside_a_carried_script_stays_malformed() {
    for (source, kind) in [
        (r#"<div class="foo ${bar}"></div>"#, SourceKind::Html),
        (
            r#"el.innerHTML = '<div class="foo ${bar}"></div>';"#,
            SourceKind::Js,
        ),
    ] {
        let result = extract_candidates(source.as_bytes(), kind);
        assert!(
            result.notes.iter().any(|note| {
                note.kind == NoteKind::MalformedClassCandidate && note.text == "${bar}"
            }),
            "{kind:?}: {result:?}"
        );
    }
}

/// Inside a carried `<script>`, a `${` that never closes or is escaped is
/// text: it must not swallow later markup or split the value.
#[test]
fn unclosed_or_escaped_interpolation_in_carried_script_stays_text() {
    let source = r#"export default "<script>x = `<input placeholder=\"${\"><div class=\"card p-4\">x</div><i class=\"a \\${b} c\">`;</script>";"#;
    let result = extract_candidates(source.as_bytes(), SourceKind::Js);
    assert_positions(source, &result);
    for class in ["card", "p-4", "a", "c"] {
        class_occurrence(&result, class);
    }
    assert!(
        !result.candidates.iter().any(|c| c.text == "b"),
        "{result:?}"
    );
}
