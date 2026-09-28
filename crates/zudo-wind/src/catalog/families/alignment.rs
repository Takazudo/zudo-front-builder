use super::*;

pub(super) fn add(entries: &mut Vec<CatalogEntry>) {
    for (root, property) in [
        ("items", "align-items"),
        ("self", "align-self"),
        ("justify", "justify-content"),
        ("place-items", "place-items"),
    ] {
        let mut keywords = vec![
            ("start", "flex-start"),
            ("end", "flex-end"),
            ("center", "center"),
        ];
        if root == "justify" {
            keywords.extend([
                ("between", "space-between"),
                ("around", "space-around"),
                ("evenly", "space-evenly"),
            ]);
        } else {
            if root != "place-items" {
                keywords.push(("baseline", "baseline"));
            }
            keywords.push(("stretch", "stretch"));
            if root == "place-items" {
                keywords[0] = ("start", "start");
                keywords[1] = ("end", "end");
            }
        }
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
