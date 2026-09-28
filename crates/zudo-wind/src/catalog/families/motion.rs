use super::*;

pub(super) fn add(entries: &mut Vec<CatalogEntry>) {
    let mut transition = entry(
        "transition",
        "transition",
        35,
        0,
        &[
            "transition-property",
            "transition-duration",
            "transition-timing-function",
        ],
        &[ValueKind::Keyword, ValueKind::Arbitrary],
        &[
            ("", "all"),
            ("all", "all"),
            ("none", "none"),
            (
                "colors",
                "color,background-color,border-color,text-decoration-color,fill,stroke",
            ),
            ("opacity", "opacity"),
            ("transform", "transform,translate,rotate,scale"),
        ],
        &[],
        Some("transition-property"),
        false,
        false,
        OWN,
        Some("colors"),
        "color,background-color,border-color,text-decoration-color,fill,stroke",
    );
    set_id(&mut transition, "v1.transition");
    set_templates(
        &mut transition,
        &[
            resolved("transition-property"),
            DeclarationTemplate {
                property: "transition-duration",
                value: EmissionValue::TransitionDuration,
            },
            DeclarationTemplate {
                property: "transition-timing-function",
                value: EmissionValue::TransitionTimingFunction,
            },
        ],
        &[
            (
                "transition-property",
                "color,background-color,border-color,text-decoration-color,fill,stroke",
            ),
            ("transition-duration", "150ms"),
            ("transition-timing-function", "ease"),
        ],
    );
    entries.push(transition);

    let mut duration = entry(
        "duration",
        "duration",
        36,
        0,
        &["transition-duration"],
        &[ValueKind::Integer, ValueKind::Arbitrary],
        &[],
        &[],
        Some("transition-duration"),
        false,
        false,
        OWN,
        Some("200"),
        "200ms",
    );
    set_id(&mut duration, "v1.duration");
    entries.push(duration);

    let mut easing = entry(
        "ease",
        "easing",
        37,
        0,
        &["transition-timing-function"],
        &[ValueKind::Token, ValueKind::Arbitrary],
        &[],
        &[TokenCategory::Easing],
        Some("transition-timing-function"),
        false,
        false,
        OWN,
        Some("[ease-in]"),
        "ease-in",
    );
    set_id(&mut easing, "v1.ease");
    entries.push(easing);

    for (root, property, rank) in [
        ("translate-x", "--zw-translate-x", 0),
        ("translate-y", "--zw-translate-y", 1),
    ] {
        let translate = "var(--zw-translate-x) var(--zw-translate-y)";
        let mut axis = entry(
            root,
            "translate",
            38,
            rank,
            &[property],
            &[
                ValueKind::Keyword,
                ValueKind::Token,
                ValueKind::Scale,
                ValueKind::Fraction,
                ValueKind::Arbitrary,
            ],
            &[("0", "0"), ("full", "100%")],
            &[TokenCategory::Spacing],
            Some("translate"),
            true,
            true,
            OWN,
            Some("0"),
            "0",
        );
        let axis_name = root.strip_prefix("translate-").unwrap_or_default();
        set_id(&mut axis, &format!("v1.translate.{axis_name}"));
        set_templates(
            &mut axis,
            &[resolved(property), fixed("translate", translate)],
            &[(property, "0"), ("translate", translate)],
        );
        axis.registrations = vec![
            Registration {
                name: "--zw-translate-x",
                syntax: "<length-percentage>",
                inherits: false,
                initial_value: "0px",
            },
            Registration {
                name: "--zw-translate-y",
                syntax: "<length-percentage>",
                inherits: false,
                initial_value: "0px",
            },
        ];
        entries.push(axis);
    }

    let mut rotate = entry(
        "rotate",
        "rotate",
        39,
        0,
        &["rotate"],
        &[ValueKind::Integer, ValueKind::Arbitrary],
        &[],
        &[],
        Some("rotate"),
        false,
        true,
        OWN,
        Some("90"),
        "90deg",
    );
    set_id(&mut rotate, "v1.rotate");
    entries.push(rotate);
}
