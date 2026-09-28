use zudo_wind::{extract_candidates, NoteKind, PositionKind, SourceKind};

fn names(bytes: &[u8], kind: SourceKind) -> Vec<String> {
    extract_candidates(bytes, kind)
        .candidates
        .into_iter()
        .map(|c| c.text)
        .collect()
}

#[test]
fn eight_source_kinds_and_sorted_determinism() {
    let cases: &[(&[u8], SourceKind, &[&str])] = &[
        (
            include_bytes!("fixtures/extract/basic.ts"),
            SourceKind::Ts,
            &["p-2", "hover:bg-panel"],
        ),
        (
            include_bytes!("fixtures/extract/basic.js"),
            SourceKind::Js,
            &["m-2", "flex"],
        ),
        (
            include_bytes!("fixtures/extract/basic.jsx"),
            SourceKind::Jsx,
            &["grid", "gap-2"],
        ),
        (
            include_bytes!("fixtures/extract/basic.mjs"),
            SourceKind::Mjs,
            &["px-2", "py-3", "py-4"],
        ),
        (
            include_bytes!("fixtures/extract/basic.html"),
            SourceKind::Html,
            &["p-2", "hover:bg-panel", "text-sm"],
        ),
        (
            include_bytes!("fixtures/extract/hello.mdx"),
            SourceKind::Mdx,
            &[],
        ),
        (
            include_bytes!("fixtures/extract/styling.md"),
            SourceKind::Md,
            &[],
        ),
        (
            include_bytes!("fixtures/extract/option-row.tsx"),
            SourceKind::Tsx,
            &["last:border-b-0", "pb-vsp-2xs", "[overflow-wrap:anywhere]"],
        ),
    ];
    for &(input, kind, expected) in cases {
        let one = extract_candidates(input, kind);
        assert_eq!(one, extract_candidates(input, kind));
        let found = names(input, kind);
        assert!(found.windows(2).all(|w| w[0] < w[1]));
        for name in expected {
            assert!(
                found.iter().any(|s| s == name),
                "missing {name:?} in {kind:?}"
            );
        }
    }
}

#[test]
fn literal_map_and_template_class_positions() {
    let result = extract_candidates(
        include_bytes!("fixtures/extract/callout.tsx"),
        SourceKind::Tsx,
    );
    for name in [
        "border-sky-500",
        "bg-emerald-50",
        "dark:bg-violet-950/40",
        "text-rose-700",
    ] {
        let entry = result.candidates.iter().find(|c| c.text == name).unwrap();
        assert!(entry
            .occurrences
            .iter()
            .all(|o| o.position_kind == PositionKind::Literal));
    }
    for name in ["my-6", "rounded-md", "flex", "items-center"] {
        let entry = result.candidates.iter().find(|c| c.text == name).unwrap();
        assert!(entry
            .occurrences
            .iter()
            .any(|o| o.position_kind == PositionKind::Class));
    }
}

#[test]
fn interpolation_boundary_and_dynamic_fragment() {
    let result = extract_candidates(
        include_bytes!("fixtures/extract/option-row.tsx"),
        SourceKind::Tsx,
    );
    let item = result
        .candidates
        .iter()
        .find(|c| c.text == "last:border-b-0")
        .unwrap();
    assert!(item.occurrences.iter().any(|o| o.adjacent_interpolation));
    let dynamic = extract_candidates(b"const x = `bg-${color}`;", SourceKind::Ts);
    assert!(!dynamic.candidates.iter().any(|c| c.text == "bg-"));
    assert_eq!(
        dynamic
            .notes
            .iter()
            .filter(|n| n.kind == NoteKind::DynamicConstruction)
            .count(),
        1
    );
}

#[test]
fn markdown_fences_are_ignored() {
    let names = names(
        include_bytes!("fixtures/extract/styling.md"),
        SourceKind::Md,
    );
    assert!(!names.iter().any(|n| n == "text-blue-600"));
}

#[test]
fn non_utf8_and_unterminated_are_safe() {
    assert_eq!(
        extract_candidates(b"\xff", SourceKind::Ts).notes[0].kind,
        NoteKind::InvalidUtf8
    );
    let _ = extract_candidates(b"const x = 'unfinished", SourceKind::Ts);
    let layout = extract_candidates(
        include_bytes!("fixtures/extract/layout.tsx"),
        SourceKind::Tsx,
    );
    assert!(layout.candidates.iter().any(|c| c
        .occurrences
        .iter()
        .any(|o| o.position_kind == PositionKind::Literal)));
    assert!(layout.candidates.iter().any(|c| c
        .occurrences
        .iter()
        .any(|o| o.position_kind == PositionKind::Class)));
}

#[test]
fn offsets_comments_entities_and_embedded_markup() {
    let source = b"<!-- <div class='hidden'> --><div class=\"p-2&amp;m-1&#32;flex\"></div>";
    let result = extract_candidates(source, SourceKind::Html);
    assert!(!result.candidates.iter().any(|c| c.text == "hidden"));
    let flex = result.candidates.iter().find(|c| c.text == "flex").unwrap();
    let at = source.windows(4).position(|w| w == b"flex").unwrap();
    assert_eq!(flex.occurrences[0].byte_offset, at);
    let source = b"const html = '<div class=\\\"px-2\\\">'; // 'hidden'";
    let result = extract_candidates(source, SourceKind::Ts);
    let entry = result.candidates.iter().find(|c| c.text == "px-2").unwrap();
    assert!(entry
        .occurrences
        .iter()
        .any(|o| o.position_kind == PositionKind::Class));
    let at = source.windows(4).position(|w| w == b"px-2").unwrap();
    assert!(entry
        .occurrences
        .iter()
        .any(|o| o.position_kind == PositionKind::Class && o.byte_offset == at));
    assert!(!result.candidates.iter().any(|c| c.text == "hidden"));
}

#[test]
fn malformed_class_is_preserved_and_unicode_is_safe() {
    let source = "é<div class='hover::block w-[calc(1px]'>";
    let result = extract_candidates(source.as_bytes(), SourceKind::Html);
    assert!(result.candidates.iter().any(|c| c.text == "hover::block"));
    assert!(result.candidates.iter().any(|c| c.text == "w-[calc(1px]"));
    assert!(result
        .notes
        .iter()
        .any(|n| n.kind == NoteKind::MalformedClassCandidate));
}

#[test]
fn markdown_inline_code_is_ignored_but_jsx_template_is_read() {
    let source =
        b"`<div class=\"hidden\">`\n<Component className={`flex ${state ? \"p-2\" : \"m-2\"}`} />";
    let result = extract_candidates(source, SourceKind::Mdx);
    let names: Vec<_> = result.candidates.iter().map(|c| c.text.as_str()).collect();
    assert!(!names.contains(&"hidden"));
    for item in ["flex", "p-2", "m-2"] {
        assert!(names.contains(&item));
    }
}

#[test]
fn multi_backtick_inline_code_is_ignored() {
    let source = b"``<div class='hidden'>`` <div class='flex'>";
    let names = names(source, SourceKind::Md);
    assert!(names.contains(&"flex".to_owned()));
    assert!(!names.contains(&"hidden".to_owned()));
}

#[test]
fn page_and_toggle_fixtures_keep_arbitrary_and_variant_candidates() {
    let page = names(
        include_bytes!("fixtures/extract/index.tsx"),
        SourceKind::Tsx,
    );
    for item in ["tracking-[0.08em]", "first:pt-0", "group-hover:text-accent"] {
        assert!(page.contains(&item.to_owned()));
    }
    let toggle = names(
        include_bytes!("fixtures/extract/theme-toggle.tsx"),
        SourceKind::Tsx,
    );
    for item in ["size-8", "dark:hover:border-neutral-700"] {
        assert!(toggle.contains(&item.to_owned()));
    }
}
