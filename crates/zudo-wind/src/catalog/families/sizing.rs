use super::*;

pub(super) fn add(entries: &mut Vec<CatalogEntry>) {
    for (root, properties, rank) in [
        ("size", vec!["width", "height"], 0),
        ("w", vec!["width"], 1),
        ("h", vec!["height"], 1),
        ("min-w", vec!["min-width"], 1),
        ("max-w", vec!["max-width"], 1),
        ("min-h", vec!["min-height"], 1),
        ("max-h", vec!["max-height"], 1),
    ] {
        let mut keywords = vec![
            ("0", "0"),
            ("px", "1px"),
            ("full", "100%"),
            ("min", "min-content"),
            ("max", "max-content"),
            ("fit", "fit-content"),
        ];
        if ["size", "w", "h"].contains(&root) {
            keywords.push(("auto", "auto"));
        }
        if root.starts_with("max-") {
            keywords.push(("none", "none"));
        }
        if root != "size" {
            keywords.push((
                "screen",
                if root.ends_with('w') {
                    "100vw"
                } else {
                    "100vh"
                },
            ));
        }
        keywords.push(("dvw", "100dvw"));
        keywords.push(("dvh", "100dvh"));
        entries.push(entry(
            root,
            "sizing",
            8,
            rank,
            &properties,
            &[
                ValueKind::Keyword,
                ValueKind::Token,
                ValueKind::Scale,
                ValueKind::Fraction,
                ValueKind::Arbitrary,
            ],
            &keywords,
            &[TokenCategory::Size, TokenCategory::Spacing],
            Some(properties[0]),
            true,
            false,
            OWN,
            Some("full"),
            "100%",
        ));
    }
}
