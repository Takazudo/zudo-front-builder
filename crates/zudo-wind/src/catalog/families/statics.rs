use super::*;

pub(super) fn add(entries: &mut Vec<CatalogEntry>) {
    for (root, property, value) in [
        ("list-none", "list-style-type", "none"),
        ("list-disc", "list-style-type", "disc"),
        ("list-decimal", "list-style-type", "decimal"),
        ("list-inside", "list-style-position", "inside"),
        ("list-outside", "list-style-position", "outside"),
    ] {
        entries.push(entry(
            root,
            "list",
            41,
            0,
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

    let mut aspect = entry(
        "aspect",
        "aspect",
        42,
        0,
        &["aspect-ratio"],
        &[
            ValueKind::Keyword,
            ValueKind::Fraction,
            ValueKind::Arbitrary,
        ],
        &[("auto", "auto"), ("square", "1 / 1"), ("video", "16 / 9")],
        &[],
        Some("aspect-ratio"),
        true,
        false,
        OWN,
        Some("16/9"),
        "16 / 9",
    );
    set_id(&mut aspect, "v1.aspect");
    set_templates(
        &mut aspect,
        &[resolved("aspect-ratio")],
        &[("aspect-ratio", "16 / 9")],
    );
    entries.push(aspect);

    let scroll_roots = [
        ("scroll-m", vec!["top", "right", "bottom", "left"], 0),
        ("scroll-mx", vec!["left", "right"], 1),
        ("scroll-my", vec!["top", "bottom"], 1),
        ("scroll-mt", vec!["top"], 2),
        ("scroll-mr", vec!["right"], 2),
        ("scroll-mb", vec!["bottom"], 2),
        ("scroll-ml", vec!["left"], 2),
    ];
    for (root, sides, rank) in scroll_roots {
        let properties = sides
            .iter()
            .map(|side| match *side {
                "top" => "scroll-margin-top",
                "right" => "scroll-margin-right",
                "bottom" => "scroll-margin-bottom",
                _ => "scroll-margin-left",
            })
            .collect::<Vec<_>>();
        let mut scroll_margin = entry(
            root,
            "scroll-margin",
            43,
            rank,
            &properties,
            SPACING,
            &[("0", "0"), ("px", "1px")],
            &[TokenCategory::Spacing],
            properties.first().copied(),
            false,
            true,
            OWN,
            Some("0"),
            "0",
        );
        set_id(&mut scroll_margin, &format!("v1.{root}"));
        entries.push(scroll_margin);
    }

    for (root, value) in [
        ("align-top", "top"),
        ("align-middle", "middle"),
        ("align-bottom", "bottom"),
        ("align-baseline", "baseline"),
    ] {
        entries.push(entry(
            root,
            "miscellaneous",
            44,
            0,
            &["vertical-align"],
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
    for (root, value) in [("box-border", "border-box"), ("box-content", "content-box")] {
        entries.push(entry(
            root,
            "miscellaneous",
            44,
            1,
            &["box-sizing"],
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
    for (root, value) in [
        ("object-contain", "contain"),
        ("object-cover", "cover"),
        ("object-fill", "fill"),
        ("object-none", "none"),
        ("object-scale-down", "scale-down"),
    ] {
        entries.push(entry(
            root,
            "miscellaneous",
            44,
            2,
            &["object-fit"],
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

    let mut sr_only = entry(
        "sr-only",
        "miscellaneous",
        44,
        3,
        &[
            "position",
            "width",
            "height",
            "padding",
            "margin",
            "overflow",
            "clip",
            "white-space",
            "border-width",
        ],
        STATIC,
        &[],
        &[],
        None,
        false,
        false,
        OWN,
        None,
        "absolute",
    );
    set_templates(
        &mut sr_only,
        &[
            fixed("position", "absolute"),
            fixed("width", "1px"),
            fixed("height", "1px"),
            fixed("padding", "0"),
            fixed("margin", "-1px"),
            fixed("overflow", "hidden"),
            fixed("clip", "rect(0,0,0,0)"),
            fixed("white-space", "nowrap"),
            fixed("border-width", "0"),
        ],
        &[
            ("position", "absolute"),
            ("width", "1px"),
            ("height", "1px"),
            ("padding", "0"),
            ("margin", "-1px"),
            ("overflow", "hidden"),
            ("clip", "rect(0,0,0,0)"),
            ("white-space", "nowrap"),
            ("border-width", "0"),
        ],
    );
    entries.push(sr_only);
}
