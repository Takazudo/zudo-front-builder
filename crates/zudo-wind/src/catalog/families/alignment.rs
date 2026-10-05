use super::*;

pub(super) fn add(entries: &mut Vec<CatalogEntry>) {
    for (root, property) in [
        ("items", "align-items"),
        ("self", "align-self"),
        ("justify", "justify-content"),
        ("place-items", "place-items"),
        ("justify-items", "justify-items"),
        ("justify-self", "justify-self"),
        ("place-content", "place-content"),
        ("place-self", "place-self"),
    ] {
        let keywords = match root {
            // Keep the original alignment utility spellings and outputs stable.
            "items" | "self" => vec![
                ("start", "flex-start"),
                ("end", "flex-end"),
                ("center", "center"),
                ("baseline", "baseline"),
                ("stretch", "stretch"),
            ],
            "justify" => vec![
                ("start", "flex-start"),
                ("end", "flex-end"),
                ("center", "center"),
                ("between", "space-between"),
                ("around", "space-around"),
                ("evenly", "space-evenly"),
            ],
            "place-items" => vec![
                ("start", "start"),
                ("end", "end"),
                ("center", "center"),
                ("stretch", "stretch"),
            ],
            "justify-items" | "justify-self" | "place-self" => vec![
                ("start", "start"),
                ("end", "end"),
                ("center", "center"),
                ("stretch", "stretch"),
                ("baseline", "baseline"),
            ],
            "place-content" => vec![
                ("start", "start"),
                ("end", "end"),
                ("center", "center"),
                ("stretch", "stretch"),
                ("between", "space-between"),
                ("around", "space-around"),
                ("evenly", "space-evenly"),
            ],
            _ => unreachable!("alignment roots are listed explicitly above"),
        };

        entries.push(entry(
            root,
            "alignment",
            7,
            0,
            &[property],
            &[ValueKind::Keyword],
            &keywords,
            &[],
            None,
            false,
            false,
            OWN,
            Some("center"),
            "center",
        ));
    }
}
