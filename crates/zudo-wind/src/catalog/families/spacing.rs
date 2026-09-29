use super::*;

pub(super) fn add(entries: &mut Vec<CatalogEntry>) {
    for (group, group_rank, prefix, negative) in
        [("padding", 9, "p", false), ("margin", 10, "m", true)]
    {
        for (suffix, properties, rank) in [
            ("", vec!["top", "right", "bottom", "left"], 0),
            ("x", vec!["left", "right"], 1),
            ("y", vec!["top", "bottom"], 1),
            ("t", vec!["top"], 2),
            ("r", vec!["right"], 2),
            ("b", vec!["bottom"], 2),
            ("l", vec!["left"], 2),
        ] {
            let root = format!("{prefix}{suffix}");
            let properties: Vec<&'static str> = properties
                .into_iter()
                .map(|side| match (prefix, side) {
                    ("p", "top") => "padding-top",
                    ("p", "right") => "padding-right",
                    ("p", "bottom") => "padding-bottom",
                    ("p", "left") => "padding-left",
                    ("m", "top") => "margin-top",
                    ("m", "right") => "margin-right",
                    ("m", "bottom") => "margin-bottom",
                    _ => "margin-left",
                })
                .collect();
            let mut keywords = vec![("0", "0"), ("px", "1px")];
            if negative {
                keywords.push(("auto", "auto"));
            }
            entries.push(entry(
                &root,
                group,
                group_rank,
                rank,
                &properties,
                SPACING,
                &keywords,
                &[TokenCategory::Spacing],
                Some(properties[0]),
                false,
                negative,
                OWN,
                Some("0"),
                "0",
            ));
        }
    }
    for (root, property) in [("space-x", "margin-left"), ("space-y", "margin-top")] {
        entries.push(entry(
            root,
            "child-space",
            12,
            0,
            &[property],
            SPACING,
            &[("0", "0"), ("px", "1px")],
            &[TokenCategory::Spacing],
            Some(property),
            false,
            true,
            CHILD,
            Some("0"),
            "0",
        ));
    }
}
