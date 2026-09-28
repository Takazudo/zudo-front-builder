use super::*;

pub(super) fn add(entries: &mut Vec<CatalogEntry>) {
    entries.push(entry(
        "z",
        "z-index",
        14,
        0,
        &["z-index"],
        &[
            ValueKind::Keyword,
            ValueKind::Token,
            ValueKind::Integer,
            ValueKind::Arbitrary,
        ],
        &[("auto", "auto")],
        &[TokenCategory::ZIndex],
        Some("z-index"),
        false,
        true,
        OWN,
        Some("0"),
        "0",
    ));
}
