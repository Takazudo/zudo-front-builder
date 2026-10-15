//! Shared assertions for explicit MDX table output (zfb#4232).

// Each test binary that includes this module uses a different subset.
#![allow(dead_code)]

/// Intrinsic table-structure elements a `p` may never be a child of.
pub const TABLE_STRUCTURE: [&str; 6] = ["table", "thead", "tbody", "tfoot", "tr", "colgroup"];

/// Names of the `<_components.X` opening tags inside the first
/// `<_components.table` … last `</_components.table>` region of an emitted
/// module, in source order. Whitespace- and attribute-insensitive, so both
/// emit routes can be compared against one expected sequence.
pub fn table_tag_sequence(out: &str) -> Vec<String> {
    let start = out
        .find("<_components.table")
        .unwrap_or_else(|| panic!("no `<_components.table` in:\n{out}"));
    let end = out
        .rfind("</_components.table>")
        .unwrap_or_else(|| panic!("no `</_components.table>` in:\n{out}"));
    out[start..end]
        .split("<_components.")
        .skip(1)
        .map(|rest| {
            rest.chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect()
        })
        .collect()
}

/// Assert that no `p` opens directly inside a table-structure element.
pub fn assert_no_structure_paragraph(seq: &[String], out: &str) {
    for pair in seq.windows(2) {
        assert!(
            !(pair[1] == "p" && TABLE_STRUCTURE.contains(&pair[0].as_str())),
            "`{} > p` emitted inside an explicit table ({seq:?}):\n{out}",
            pair[0],
        );
    }
}
