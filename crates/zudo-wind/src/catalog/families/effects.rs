use super::*;

pub(super) fn add(entries: &mut Vec<CatalogEntry>) {
    let mut shadow = entry(
        "shadow",
        "shadow",
        33,
        0,
        &["box-shadow"],
        &[ValueKind::Keyword, ValueKind::Token, ValueKind::Arbitrary],
        &[("none", "none")],
        &[TokenCategory::Shadow],
        Some("box-shadow"),
        false,
        false,
        OWN,
        Some("none"),
        "none",
    );
    set_id(&mut shadow, "v1.shadow");
    entries.push(shadow);

    let mut opacity = entry(
        "opacity",
        "opacity",
        34,
        0,
        &["opacity"],
        &[ValueKind::Integer, ValueKind::Arbitrary],
        &[],
        &[],
        Some("opacity"),
        false,
        false,
        OWN,
        Some("50"),
        "0.5",
    );
    set_id(&mut opacity, "v1.opacity");
    set_templates(&mut opacity, &[resolved("opacity")], &[("opacity", "0.5")]);
    entries.push(opacity);
}
