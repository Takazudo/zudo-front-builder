use super::*;

pub(super) fn add(entries: &mut Vec<CatalogEntry>) {
    for (root, value) in [
        ("flex-row", "row"),
        ("flex-row-reverse", "row-reverse"),
        ("flex-col", "column"),
        ("flex-col-reverse", "column-reverse"),
    ] {
        entries.push(entry(
            root,
            "flex-container",
            4,
            0,
            &["flex-direction"],
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
        ("flex-wrap", "wrap"),
        ("flex-wrap-reverse", "wrap-reverse"),
        ("flex-nowrap", "nowrap"),
    ] {
        entries.push(entry(
            root,
            "flex-container",
            4,
            0,
            &["flex-wrap"],
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
        ("flex-1", "1 1 0%"),
        ("flex-auto", "1 1 auto"),
        ("flex-initial", "0 1 auto"),
        ("flex-none", "none"),
    ] {
        entries.push(entry(
            root,
            "flex-item",
            5,
            0,
            &["flex"],
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
    for root in ["grow", "shrink"] {
        let property = if root == "grow" {
            "flex-grow"
        } else {
            "flex-shrink"
        };
        entries.push(entry(
            root,
            "flex-item",
            5,
            0,
            &[property],
            &[ValueKind::Keyword, ValueKind::Arbitrary],
            &[("", "1"), ("0", "0")],
            &[],
            Some(property),
            false,
            false,
            OWN,
            None,
            "1",
        ));
    }
    for (root, property) in [
        ("grid-cols", "grid-template-columns"),
        ("grid-rows", "grid-template-rows"),
    ] {
        entries.push(entry(
            root,
            "grid",
            6,
            0,
            &[property],
            &[ValueKind::Keyword, ValueKind::Integer, ValueKind::Arbitrary],
            &[("none", "none"), ("subgrid", "subgrid")],
            &[],
            Some(property),
            false,
            false,
            OWN,
            Some("none"),
            "none",
        ));
    }
    for (root, property) in [("col-span", "grid-column"), ("row-span", "grid-row")] {
        entries.push(entry(
            root,
            "grid",
            6,
            0,
            &[property],
            &[ValueKind::Keyword, ValueKind::Integer],
            &[("full", "1 / -1")],
            &[],
            None,
            false,
            false,
            OWN,
            Some("full"),
            "1 / -1",
        ));
    }
    for (root, property) in [
        ("col-start", "grid-column-start"),
        ("col-end", "grid-column-end"),
        ("row-start", "grid-row-start"),
        ("row-end", "grid-row-end"),
    ] {
        entries.push(entry(
            root,
            "grid",
            6,
            0,
            &[property],
            &[ValueKind::Keyword, ValueKind::Integer],
            &[("auto", "auto")],
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
