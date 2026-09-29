use super::*;

pub(super) fn add(entries: &mut Vec<CatalogEntry>) {
    let mut text = color_entry("text", "color", 22, "color", "v1.text.color");
    set_templates(
        &mut text,
        &[resolved("color")],
        &[("color", "color-mix(in oklab, red 40%, transparent)")],
    );
    entries.push(text);

    let mut background = color_entry("bg", "background", 23, "background-color", "v1.background");
    set_templates(
        &mut background,
        &[resolved("background-color")],
        &[(
            "background-color",
            "color-mix(in oklab, red 40%, transparent)",
        )],
    );
    entries.push(background);
}

fn color_entry(
    root: &str,
    group: &'static str,
    group_rank: u16,
    property: &'static str,
    id: &str,
) -> CatalogEntry {
    let mut entry = entry(
        root,
        group,
        group_rank,
        0,
        &[property],
        &[ValueKind::Keyword, ValueKind::Token, ValueKind::Arbitrary],
        &[("transparent", "transparent"), ("current", "currentColor")],
        &[TokenCategory::Color],
        Some(property),
        false,
        false,
        OWN,
        Some("[red]/40"),
        "color-mix(in oklab, red 40%, transparent)",
    );
    set_id(&mut entry, id);
    entry
}
