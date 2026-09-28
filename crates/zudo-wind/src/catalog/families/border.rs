use super::*;

pub(super) fn add(entries: &mut Vec<CatalogEntry>) {
    let border_roots = [
        ("border", 0, vec!["top", "right", "bottom", "left"]),
        ("border-x", 1, vec!["left", "right"]),
        ("border-y", 1, vec!["top", "bottom"]),
        ("border-t", 2, vec!["top"]),
        ("border-r", 2, vec!["right"]),
        ("border-b", 2, vec!["bottom"]),
        ("border-l", 2, vec!["left"]),
    ];
    for (root, rank, sides) in &border_roots {
        let mut templates = Vec::new();
        for side in sides {
            let width = match *side {
                "top" => "border-top-width",
                "right" => "border-right-width",
                "bottom" => "border-bottom-width",
                _ => "border-left-width",
            };
            let style = match *side {
                "top" => "border-top-style",
                "right" => "border-right-style",
                "bottom" => "border-bottom-style",
                _ => "border-left-style",
            };
            templates.push(resolved(width));
            templates.push(fixed(style, "solid"));
        }
        let width_properties = templates
            .iter()
            .step_by(2)
            .map(|template| template.property)
            .collect::<Vec<_>>();
        let mut width = entry(
            root,
            "border-width",
            24,
            *rank,
            &width_properties,
            &[ValueKind::Exact, ValueKind::Integer, ValueKind::Arbitrary],
            &[],
            &[],
            width_properties.first().copied(),
            false,
            false,
            OWN,
            None,
            "1px",
        );
        let suffix = root.strip_prefix("border").unwrap_or("");
        let id = if suffix.is_empty() {
            "v1.border.width".to_owned()
        } else {
            format!("v1.border.width.{}", suffix.trim_start_matches('-'))
        };
        set_id(&mut width, &id);
        let expected = templates
            .iter()
            .map(|template| {
                (
                    template.property,
                    match template.value {
                        EmissionValue::Resolved => "1px",
                        EmissionValue::Fixed(value) => value,
                        _ => unreachable!(),
                    },
                )
            })
            .collect::<Vec<_>>();
        set_templates(&mut width, &templates, &expected);
        entries.push(width);

        let properties = sides
            .iter()
            .map(|side| match *side {
                "top" => "border-top-color",
                "right" => "border-right-color",
                "bottom" => "border-bottom-color",
                _ => "border-left-color",
            })
            .collect::<Vec<_>>();
        let mut color = entry(
            root,
            "border-color",
            25,
            *rank,
            &properties,
            &[ValueKind::Keyword, ValueKind::Token, ValueKind::Arbitrary],
            &[("transparent", "transparent"), ("current", "currentColor")],
            &[TokenCategory::Color],
            properties.first().copied(),
            false,
            false,
            OWN,
            Some("[red]/40"),
            "color-mix(in oklab, red 40%, transparent)",
        );
        let id = if suffix.is_empty() {
            "v1.border.color".to_owned()
        } else {
            format!("v1.border.color.{}", suffix.trim_start_matches('-'))
        };
        set_id(&mut color, &id);
        let expected = properties
            .iter()
            .map(|property| (*property, "color-mix(in oklab, red 40%, transparent)"))
            .collect::<Vec<_>>();
        set_templates(
            &mut color,
            &properties
                .iter()
                .map(|property| resolved(property))
                .collect::<Vec<_>>(),
            &expected,
        );
        entries.push(color);
    }

    for (root, value) in [
        ("border-solid", "solid"),
        ("border-dashed", "dashed"),
        ("border-dotted", "dotted"),
        ("border-double", "double"),
        ("border-none", "none"),
    ] {
        let mut style = entry(
            root,
            "border-style",
            26,
            0,
            &["border-style"],
            STATIC,
            &[],
            &[],
            None,
            false,
            false,
            OWN,
            None,
            value,
        );
        set_id(&mut style, &format!("v1.border.style.{value}"));
        entries.push(style);
    }
    for (root, value) in [
        ("border-collapse", "collapse"),
        ("border-collapse-separate", "separate"),
    ] {
        let mut style = entry(
            root,
            "border-style",
            26,
            1,
            &["border-collapse"],
            STATIC,
            &[],
            &[],
            None,
            false,
            false,
            OWN,
            None,
            value,
        );
        set_id(&mut style, &format!("v1.border.style.{value}"));
        entries.push(style);
    }

    let radius_roots = [
        (
            "rounded",
            0,
            vec!["top-left", "top-right", "bottom-right", "bottom-left"],
        ),
        ("rounded-t", 1, vec!["top-left", "top-right"]),
        ("rounded-r", 1, vec!["top-right", "bottom-right"]),
        ("rounded-b", 1, vec!["bottom-right", "bottom-left"]),
        ("rounded-l", 1, vec!["top-left", "bottom-left"]),
        ("rounded-tl", 2, vec!["top-left"]),
        ("rounded-tr", 2, vec!["top-right"]),
        ("rounded-br", 2, vec!["bottom-right"]),
        ("rounded-bl", 2, vec!["bottom-left"]),
    ];
    for (root, rank, corners) in radius_roots {
        let properties = corners
            .iter()
            .map(|corner| match *corner {
                "top-left" => "border-top-left-radius",
                "top-right" => "border-top-right-radius",
                "bottom-right" => "border-bottom-right-radius",
                _ => "border-bottom-left-radius",
            })
            .collect::<Vec<_>>();
        let mut radius = entry(
            root,
            "radius",
            27,
            rank,
            &properties,
            &[ValueKind::Keyword, ValueKind::Token, ValueKind::Arbitrary],
            &[("none", "0"), ("full", "9999px")],
            &[TokenCategory::Radius],
            properties.first().copied(),
            false,
            false,
            OWN,
            Some("full"),
            "9999px",
        );
        set_id(&mut radius, &format!("v1.{root}"));
        entries.push(radius);
    }

    for (root, property, rank) in [
        ("divide-x", "border-left-width", 0),
        ("divide-y", "border-top-width", 0),
    ] {
        let style = if root == "divide-x" {
            "border-left-style"
        } else {
            "border-top-style"
        };
        let mut width = entry(
            root,
            "divide-width",
            28,
            rank,
            &[property],
            &[ValueKind::Exact, ValueKind::Integer, ValueKind::Arbitrary],
            &[],
            &[],
            Some(property),
            false,
            false,
            CHILD,
            None,
            "1px",
        );
        set_id(
            &mut width,
            if root == "divide-x" {
                "v1.divide.width.x"
            } else {
                "v1.divide.width.y"
            },
        );
        set_templates(
            &mut width,
            &[resolved(property), fixed(style, "solid")],
            &[(property, "1px"), (style, "solid")],
        );
        entries.push(width);
    }
    let mut divide_color = entry(
        "divide",
        "divide-color",
        29,
        0,
        &[
            "border-top-color",
            "border-right-color",
            "border-bottom-color",
            "border-left-color",
        ],
        &[ValueKind::Keyword, ValueKind::Token, ValueKind::Arbitrary],
        &[("transparent", "transparent"), ("current", "currentColor")],
        &[TokenCategory::Color],
        Some("border-top-color"),
        false,
        false,
        CHILD,
        Some("[red]/40"),
        "color-mix(in oklab, red 40%, transparent)",
    );
    set_id(&mut divide_color, "v1.divide.color");
    set_templates(
        &mut divide_color,
        &[
            resolved("border-top-color"),
            resolved("border-right-color"),
            resolved("border-bottom-color"),
            resolved("border-left-color"),
        ],
        &[
            (
                "border-top-color",
                "color-mix(in oklab, red 40%, transparent)",
            ),
            (
                "border-right-color",
                "color-mix(in oklab, red 40%, transparent)",
            ),
            (
                "border-bottom-color",
                "color-mix(in oklab, red 40%, transparent)",
            ),
            (
                "border-left-color",
                "color-mix(in oklab, red 40%, transparent)",
            ),
        ],
    );
    entries.push(divide_color);

    let mut outline_width = entry(
        "outline",
        "outline-width",
        30,
        0,
        &["outline-width"],
        &[ValueKind::Integer, ValueKind::Arbitrary],
        &[],
        &[],
        Some("outline-width"),
        false,
        false,
        OWN,
        Some("2"),
        "2px",
    );
    set_id(&mut outline_width, "v1.outline.width");
    set_templates(
        &mut outline_width,
        &[resolved("outline-width"), fixed("outline-style", "solid")],
        &[("outline-width", "2px"), ("outline-style", "solid")],
    );
    entries.push(outline_width);

    let mut outline_offset = entry(
        "outline-offset",
        "outline-width",
        30,
        1,
        &["outline-offset"],
        &[ValueKind::Integer, ValueKind::Arbitrary],
        &[],
        &[],
        Some("outline-offset"),
        false,
        true,
        OWN,
        Some("2"),
        "2px",
    );
    set_id(&mut outline_offset, "v1.outline.offset");
    entries.push(outline_offset);

    let mut outline_color = entry(
        "outline",
        "outline-color",
        31,
        0,
        &["outline-color"],
        &[ValueKind::Keyword, ValueKind::Token, ValueKind::Arbitrary],
        &[("transparent", "transparent"), ("current", "currentColor")],
        &[TokenCategory::Color],
        Some("outline-color"),
        false,
        false,
        OWN,
        Some("[red]/40"),
        "color-mix(in oklab, red 40%, transparent)",
    );
    set_id(&mut outline_color, "v1.outline.color");
    entries.push(outline_color);

    for (root, value) in [
        ("outline-solid", "solid"),
        ("outline-dashed", "dashed"),
        ("outline-dotted", "dotted"),
        ("outline-double", "double"),
        ("outline-none", "none"),
    ] {
        let mut style = entry(
            root,
            "outline-style",
            32,
            0,
            &["outline-style"],
            STATIC,
            &[],
            &[],
            None,
            false,
            false,
            OWN,
            None,
            value,
        );
        set_id(&mut style, &format!("v1.outline.style.{value}"));
        entries.push(style);
    }
}
