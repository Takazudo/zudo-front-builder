use super::*;

pub(super) fn add(entries: &mut Vec<CatalogEntry>) {
    for (root, value) in [("appearance-auto", "auto"), ("appearance-none", "none")] {
        entries.push(entry(
            root,
            "interaction",
            40,
            0,
            &["appearance"],
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

    let mut cursor = entry(
        "cursor",
        "interaction",
        40,
        0,
        &["cursor"],
        &[ValueKind::Keyword],
        &[
            ("auto", "auto"),
            ("default", "default"),
            ("pointer", "pointer"),
            ("not-allowed", "not-allowed"),
            ("text", "text"),
            ("move", "move"),
            ("grab", "grab"),
            ("grabbing", "grabbing"),
            ("wait", "wait"),
            ("help", "help"),
            ("none", "none"),
            ("progress", "progress"),
            ("crosshair", "crosshair"),
            ("cell", "cell"),
            ("copy", "copy"),
            ("alias", "alias"),
            ("no-drop", "no-drop"),
            ("context-menu", "context-menu"),
            ("vertical-text", "vertical-text"),
            ("all-scroll", "all-scroll"),
            ("zoom-in", "zoom-in"),
            ("zoom-out", "zoom-out"),
            ("n-resize", "n-resize"),
            ("s-resize", "s-resize"),
            ("e-resize", "e-resize"),
            ("w-resize", "w-resize"),
            ("ne-resize", "ne-resize"),
            ("nw-resize", "nw-resize"),
            ("se-resize", "se-resize"),
            ("sw-resize", "sw-resize"),
            ("ew-resize", "ew-resize"),
            ("ns-resize", "ns-resize"),
            ("nesw-resize", "nesw-resize"),
            ("nwse-resize", "nwse-resize"),
            ("col-resize", "col-resize"),
            ("row-resize", "row-resize"),
        ],
        &[],
        None,
        false,
        false,
        OWN,
        Some("pointer"),
        "pointer",
    );
    set_id(&mut cursor, "v1.cursor");
    entries.push(cursor);

    let mut pointer = entry(
        "pointer-events",
        "interaction",
        40,
        0,
        &["pointer-events"],
        &[ValueKind::Keyword],
        &[("auto", "auto"), ("none", "none")],
        &[],
        None,
        false,
        false,
        OWN,
        Some("none"),
        "none",
    );
    set_id(&mut pointer, "v1.pointer-events");
    entries.push(pointer);

    let mut select = entry(
        "select",
        "interaction",
        40,
        0,
        &["user-select"],
        &[ValueKind::Keyword],
        &[
            ("none", "none"),
            ("text", "text"),
            ("all", "all"),
            ("auto", "auto"),
        ],
        &[],
        None,
        false,
        false,
        OWN,
        Some("none"),
        "none",
    );
    set_id(&mut select, "v1.select");
    entries.push(select);

    let mut resize = entry(
        "resize",
        "interaction",
        40,
        0,
        &["resize"],
        &[ValueKind::Keyword],
        &[
            ("", "both"),
            ("x", "horizontal"),
            ("y", "vertical"),
            ("none", "none"),
        ],
        &[],
        None,
        false,
        false,
        OWN,
        Some("x"),
        "horizontal",
    );
    set_id(&mut resize, "v1.resize");
    entries.push(resize);

    let mut accent = entry(
        "accent",
        "interaction",
        40,
        0,
        &["accent-color"],
        &[ValueKind::Keyword, ValueKind::Token, ValueKind::Arbitrary],
        &[("transparent", "transparent"), ("current", "currentColor")],
        &[TokenCategory::Color],
        Some("accent-color"),
        false,
        false,
        OWN,
        Some("[red]/40"),
        "color-mix(in oklab, red 40%, transparent)",
    );
    set_id(&mut accent, "v1.accent");
    entries.push(accent);
}
