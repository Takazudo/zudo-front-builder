use super::*;

pub(super) fn add(entries: &mut Vec<CatalogEntry>) {
    for (root, properties, rank) in [
        ("gap", vec!["column-gap", "row-gap"], 0),
        ("gap-x", vec!["column-gap"], 1),
        ("gap-y", vec!["row-gap"], 1),
    ] {
        entries.push(entry(
            root,
            "gap",
            11,
            rank,
            &properties,
            SPACING,
            &[("0", "0"), ("px", "1px")],
            &[TokenCategory::Spacing],
            Some("column-gap"),
            false,
            false,
            OWN,
            Some("0"),
            "0",
        ));
    }
}
