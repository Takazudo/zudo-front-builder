use super::*;

pub(super) fn add(entries: &mut Vec<CatalogEntry>) {
    let mut family = entry(
        "font",
        "font-family",
        15,
        0,
        &["font-family"],
        &[ValueKind::Token, ValueKind::Arbitrary],
        &[],
        &[TokenCategory::FontFamily],
        Some("font-family"),
        false,
        false,
        OWN,
        Some("[sans-serif]"),
        "sans-serif",
    );
    set_id(&mut family, "v1.font.family");
    entries.push(family);

    let mut weight = entry(
        "font",
        "font-weight",
        16,
        0,
        &["font-weight"],
        &[ValueKind::Token, ValueKind::Arbitrary],
        &[],
        &[TokenCategory::FontWeight],
        Some("font-weight"),
        false,
        false,
        OWN,
        Some("[400]"),
        "400",
    );
    set_id(&mut weight, "v1.font.weight");
    entries.push(weight);

    let mut size = entry(
        "text",
        "font-size",
        17,
        0,
        &["font-size", "line-height"],
        &[ValueKind::Token, ValueKind::Arbitrary],
        &[],
        &[TokenCategory::FontSize],
        Some("font-size"),
        false,
        false,
        OWN,
        Some("[1rem]"),
        "1rem",
    );
    set_id(&mut size, "v1.text.size");
    set_templates(
        &mut size,
        &[
            resolved("font-size"),
            DeclarationTemplate {
                property: "line-height",
                value: EmissionValue::OptionalFontSizeLeading,
            },
        ],
        &[("font-size", "1rem")],
    );
    entries.push(size);

    let mut leading = entry(
        "leading",
        "line-height",
        18,
        0,
        &["line-height"],
        &[ValueKind::Token, ValueKind::Arbitrary],
        &[],
        &[TokenCategory::LineHeight],
        Some("line-height"),
        false,
        false,
        OWN,
        Some("[1.5]"),
        "1.5",
    );
    set_id(&mut leading, "v1.leading");
    // `leading-none` is the constant 1 unless the project configures lineHeights.none.
    leading.grammar.fallback_keywords = vec![("none", "1")];
    entries.push(leading);

    let mut tracking = entry(
        "tracking",
        "tracking",
        19,
        0,
        &["letter-spacing"],
        &[ValueKind::Token, ValueKind::Arbitrary],
        &[],
        &[TokenCategory::LetterSpacing],
        Some("letter-spacing"),
        false,
        true,
        OWN,
        Some("[0.08em]"),
        "0.08em",
    );
    set_id(&mut tracking, "v1.tracking");
    entries.push(tracking);

    let mut align = entry(
        "text",
        "text-layout",
        20,
        0,
        &["text-align"],
        &[ValueKind::Keyword],
        &[
            ("left", "left"),
            ("center", "center"),
            ("right", "right"),
            ("justify", "justify"),
            ("start", "start"),
            ("end", "end"),
        ],
        &[],
        None,
        false,
        false,
        OWN,
        Some("left"),
        "left",
    );
    set_id(&mut align, "v1.text.align");
    entries.push(align);

    let whitespace_values = [
        ("normal", "normal"),
        ("nowrap", "nowrap"),
        ("pre", "pre"),
        ("pre-line", "pre-line"),
        ("pre-wrap", "pre-wrap"),
        ("break-spaces", "break-spaces"),
    ];
    let mut whitespace = entry(
        "whitespace",
        "text-layout",
        20,
        1,
        &["white-space"],
        &[ValueKind::Keyword],
        &whitespace_values,
        &[],
        None,
        false,
        false,
        OWN,
        Some("normal"),
        "normal",
    );
    set_id(&mut whitespace, "v1.whitespace");
    entries.push(whitespace);

    for (root, templates, example) in [
        (
            "break-normal",
            vec![
                fixed("overflow-wrap", "normal"),
                fixed("word-break", "normal"),
            ],
            vec![("overflow-wrap", "normal"), ("word-break", "normal")],
        ),
        (
            "break-words",
            vec![fixed("overflow-wrap", "break-word")],
            vec![("overflow-wrap", "break-word")],
        ),
        (
            "break-all",
            vec![fixed("word-break", "break-all")],
            vec![("word-break", "break-all")],
        ),
        (
            "wrap-anywhere",
            vec![fixed("overflow-wrap", "anywhere")],
            vec![("overflow-wrap", "anywhere")],
        ),
    ] {
        let mut static_entry = entry(
            root,
            "text-layout",
            20,
            2,
            &[templates[0].property],
            STATIC,
            &[],
            &[],
            None,
            false,
            false,
            OWN,
            None,
            example[0].1,
        );
        set_templates(&mut static_entry, &templates, &example);
        entries.push(static_entry);
    }

    let mut truncate = entry(
        "truncate",
        "text-layout",
        20,
        3,
        &["overflow", "text-overflow", "white-space"],
        STATIC,
        &[],
        &[],
        None,
        false,
        false,
        OWN,
        None,
        "hidden",
    );
    set_templates(
        &mut truncate,
        &[
            fixed("overflow", "hidden"),
            fixed("text-overflow", "ellipsis"),
            fixed("white-space", "nowrap"),
        ],
        &[
            ("overflow", "hidden"),
            ("text-overflow", "ellipsis"),
            ("white-space", "nowrap"),
        ],
    );
    entries.push(truncate);

    for (root, property, value, rank) in [
        ("underline", "text-decoration-line", "underline", 0),
        ("overline", "text-decoration-line", "overline", 0),
        ("line-through", "text-decoration-line", "line-through", 0),
        ("no-underline", "text-decoration-line", "none", 0),
        ("uppercase", "text-transform", "uppercase", 1),
        ("lowercase", "text-transform", "lowercase", 1),
        ("capitalize", "text-transform", "capitalize", 1),
        ("normal-case", "text-transform", "none", 1),
        ("italic", "font-style", "italic", 2),
        ("not-italic", "font-style", "normal", 2),
        ("tabular-nums", "font-variant-numeric", "tabular-nums", 3),
    ] {
        entries.push(entry(
            root,
            "text-style",
            21,
            rank,
            &[property],
            STATIC,
            &[],
            &[],
            None,
            false,
            false,
            OWN,
            None,
            value,
        ));
    }
    let mut antialiased = entry(
        "antialiased",
        "text-style",
        21,
        4,
        &["-webkit-font-smoothing", "-moz-osx-font-smoothing"],
        STATIC,
        &[],
        &[],
        None,
        false,
        false,
        OWN,
        None,
        "antialiased",
    );
    set_templates(
        &mut antialiased,
        &[
            fixed("-webkit-font-smoothing", "antialiased"),
            fixed("-moz-osx-font-smoothing", "grayscale"),
        ],
        &[
            ("-webkit-font-smoothing", "antialiased"),
            ("-moz-osx-font-smoothing", "grayscale"),
        ],
    );
    entries.push(antialiased);

    let mut decoration = entry(
        "decoration",
        "text-decoration-color",
        45,
        0,
        &["text-decoration-color"],
        &[ValueKind::Keyword, ValueKind::Token, ValueKind::Arbitrary],
        &[("transparent", "transparent"), ("current", "currentColor")],
        &[TokenCategory::Color],
        Some("text-decoration-color"),
        false,
        false,
        OWN,
        Some("[red]/40"),
        "color-mix(in oklab, red 40%, transparent)",
    );
    set_id(&mut decoration, "v1.decoration.color");
    entries.push(decoration);

    let mut underline_offset = entry(
        "underline-offset",
        "text-underline-offset",
        46,
        0,
        &["text-underline-offset"],
        &[ValueKind::Integer, ValueKind::Arbitrary],
        &[],
        &[],
        Some("text-underline-offset"),
        false,
        false,
        OWN,
        Some("4"),
        "4px",
    );
    set_id(&mut underline_offset, "v1.underline-offset");
    entries.push(underline_offset);
}
