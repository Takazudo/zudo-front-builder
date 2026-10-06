use super::*;

const SPACING_WITH_FRACTION: &[ValueKind] = &[
    ValueKind::Keyword,
    ValueKind::Token,
    ValueKind::Scale,
    ValueKind::Fraction,
    ValueKind::Arbitrary,
];

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
    for (root, value) in [
        ("contents", "contents"),
        ("flow-root", "flow-root"),
        ("list-item", "list-item"),
        ("table", "table"),
        ("inline-table", "inline-table"),
        ("table-caption", "table-caption"),
        ("table-cell", "table-cell"),
        ("table-column", "table-column"),
        ("table-column-group", "table-column-group"),
        ("table-footer-group", "table-footer-group"),
        ("table-header-group", "table-header-group"),
        ("table-row", "table-row"),
        ("table-row-group", "table-row-group"),
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
            SPACING_WITH_FRACTION,
            &[
                ("auto", "auto"),
                ("full", "100%"),
                ("0", "0"),
                ("px", "1px"),
            ],
            &[TokenCategory::Spacing],
            Some("top"),
            true,
            true,
            OWN,
            Some("0"),
            "0",
        ));
    }
}
