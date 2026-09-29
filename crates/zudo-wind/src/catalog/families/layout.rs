use super::*;

pub(super) fn add(entries: &mut Vec<CatalogEntry>) {
    for (root, value) in [
        ("block", "block"),
        ("inline", "inline"),
        ("inline-block", "inline-block"),
        ("flex", "flex"),
        ("inline-flex", "inline-flex"),
        ("grid", "grid"),
        ("inline-grid", "inline-grid"),
        ("hidden", "none"),
    ] {
        entries.push(entry(
            root,
            "display",
            1,
            0,
            &["display"],
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
    for root in ["static", "relative", "absolute", "fixed", "sticky"] {
        entries.push(entry(
            root,
            "position",
            2,
            0,
            &["position"],
            STATIC,
            &[],
            &[],
            None,
            false,
            false,
            OWN,
            None,
            root,
        ));
    }
    for (root, properties, rank) in [
        ("inset", vec!["top", "right", "bottom", "left"], 0),
        ("inset-x", vec!["left", "right"], 1),
        ("inset-y", vec!["top", "bottom"], 1),
        ("top", vec!["top"], 2),
        ("right", vec!["right"], 2),
        ("bottom", vec!["bottom"], 2),
        ("left", vec!["left"], 2),
    ] {
        entries.push(entry(
            root,
            "inset",
            3,
            rank,
            &properties,
            SPACING,
            &[
                ("auto", "auto"),
                ("full", "100%"),
                ("0", "0"),
                ("px", "1px"),
            ],
            &[TokenCategory::Spacing],
            Some("top"),
            false,
            true,
            OWN,
            Some("0"),
            "0",
        ));
    }
}
