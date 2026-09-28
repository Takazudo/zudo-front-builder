use super::*;

pub(super) fn add(entries: &mut Vec<CatalogEntry>) {
    for (root, property, rank) in [
        ("overflow", "overflow", 0),
        ("overflow-x", "overflow-x", 1),
        ("overflow-y", "overflow-y", 1),
    ] {
        entries.push(entry(
            root,
            "overflow",
            13,
            rank,
            &[property],
            &[ValueKind::Keyword],
            &[
                ("auto", "auto"),
                ("hidden", "hidden"),
                ("clip", "clip"),
                ("visible", "visible"),
                ("scroll", "scroll"),
            ],
            &[],
            None,
            false,
            false,
            OWN,
            Some("auto"),
            "auto",
        ));
    }
    for (root, property, rank) in [
        ("overscroll", "overscroll-behavior", 0),
        ("overscroll-x", "overscroll-behavior-x", 1),
        ("overscroll-y", "overscroll-behavior-y", 1),
    ] {
        entries.push(entry(
            root,
            "overflow",
            13,
            rank,
            &[property],
            &[ValueKind::Keyword],
            &[("auto", "auto"), ("contain", "contain"), ("none", "none")],
            &[],
            None,
            false,
            false,
            OWN,
            Some("auto"),
            "auto",
        ));
    }
}
