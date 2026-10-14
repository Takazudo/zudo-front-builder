//! Paragraph normalisation inside explicit MDX tables (zfb#4232).
//!
//! markdown-rs accepts a flow-JSX line only when it holds nothing but tags,
//! whitespace, and `{…}` expressions (`construct/mdx_jsx_flow.rs`). A line
//! carrying text between its tags — `<thead><tr><th>A</th></tr></thead>`,
//! `<tr><td>B</td></tr>` — is rejected as flow and falls back to a
//! *paragraph* of `MdxJsxTextElement`s. Inside an explicit table that yields
//! `table > p > thead` and `tbody > p > tr`: neither emitter adds the `p`,
//! both copy it through, and the zudo-react renderer rightly throws
//! `ZR_PARSER_CONTEXT: table > p`. Wrapping the whole table in `{ … }`
//! sidesteps the flow construct, which is why that spelling works.
//!
//! A paragraph is never valid content of a table-structure element, so the
//! normalisation splices it away: its children take its place, in order,
//! keeping their own source positions. Whitespace-only text between the
//! spliced tags (two rows on consecutive lines become one paragraph with a
//! `"\n"` between them) is dropped — it is insignificant in these contexts.
//!
//! Scope, deliberately narrow:
//!
//! - only lowercase intrinsic names in [`TABLE_STRUCTURE`]; a capitalised
//!   `<Table>` is a component whose rendered output zfb cannot know;
//! - `td` / `th` / `caption` are NOT in the set — a paragraph is valid flow
//!   content of a cell, so `<td>\n\nText\n\n</td>` keeps its `p`;
//! - GFM pipe tables (`MdastNode::Table`) and `{…}`-wrapped tables never
//!   produce the shape, so they are untouched by construction.
//!
//! Like [`unwrap_nested_links`](crate::unwrap_nested_links) this runs at the
//! parse sites, directly after `markdown::to_mdast`, so every pipeline is
//! covered and no visitor observes the malformed tree. It is not gated on
//! any construct: the JSX constructs that produce the shape are always on
//! in the MDX dialect, and in any other dialect no JSX node exists to match.

use markdown::mdast::Node as MdastNode;

/// Intrinsic elements whose permitted content excludes `p` and that the
/// renderer's parser-context check (`tableChildren` in zudo-react's
/// `render-html.ts` / `hydrate.ts`) restricts to table structure.
const TABLE_STRUCTURE: &[&str] = &["table", "thead", "tbody", "tfoot", "tr", "colgroup"];

/// Splice every `Paragraph` child of a table-structure MDX JSX element into
/// its parent, recursively over the whole tree.
pub fn unwrap_table_paragraphs(node: &mut MdastNode) {
    let splice = is_table_structure(node);
    if let Some(children) = node.children_mut() {
        if splice
            && children
                .iter()
                .any(|c| matches!(c, MdastNode::Paragraph(_)))
        {
            *children = splice_paragraphs(std::mem::take(children));
        }
        for child in children.iter_mut() {
            unwrap_table_paragraphs(child);
        }
    }
}

fn is_table_structure(node: &MdastNode) -> bool {
    let name = match node {
        MdastNode::MdxJsxFlowElement(e) => e.name.as_deref(),
        MdastNode::MdxJsxTextElement(e) => e.name.as_deref(),
        _ => None,
    };
    name.is_some_and(|n| TABLE_STRUCTURE.contains(&n))
}

fn splice_paragraphs(children: Vec<MdastNode>) -> Vec<MdastNode> {
    let mut out = Vec::with_capacity(children.len());
    for child in children {
        match child {
            MdastNode::Paragraph(p) => out.extend(
                p.children
                    .into_iter()
                    .filter(|c| !matches!(c, MdastNode::Text(t) if t.value.trim().is_empty())),
            ),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use markdown::mdast::{MdxJsxFlowElement, MdxJsxTextElement, Paragraph, Root, Table, Text};
    use markdown::unist::Position;

    fn parse(src: &str) -> MdastNode {
        markdown::to_mdast(src, &markdown::ParseOptions::mdx()).expect("parse ok")
    }

    fn text(value: &str) -> MdastNode {
        MdastNode::Text(Text {
            value: value.to_string(),
            position: None,
        })
    }

    fn para(children: Vec<MdastNode>) -> MdastNode {
        MdastNode::Paragraph(Paragraph {
            children,
            position: None,
        })
    }

    fn flow(name: &str, children: Vec<MdastNode>) -> MdastNode {
        MdastNode::MdxJsxFlowElement(MdxJsxFlowElement {
            name: Some(name.to_string()),
            attributes: vec![],
            children,
            position: None,
        })
    }

    fn inline(name: &str, children: Vec<MdastNode>) -> MdastNode {
        MdastNode::MdxJsxTextElement(MdxJsxTextElement {
            name: Some(name.to_string()),
            attributes: vec![],
            children,
            position: None,
        })
    }

    fn root(children: Vec<MdastNode>) -> MdastNode {
        MdastNode::Root(Root {
            children,
            position: None,
        })
    }

    /// True when some table-structure element anywhere in `node` still holds
    /// a `Paragraph` child.
    fn has_table_paragraph(node: &MdastNode) -> bool {
        let here = is_table_structure(node)
            && node
                .children()
                .is_some_and(|c| c.iter().any(|n| matches!(n, MdastNode::Paragraph(_))));
        here || node
            .children()
            .is_some_and(|c| c.iter().any(has_table_paragraph))
    }

    fn find_named<'a>(node: &'a MdastNode, name: &str) -> Option<&'a MdastNode> {
        let matches = match node {
            MdastNode::MdxJsxFlowElement(e) => e.name.as_deref() == Some(name),
            MdastNode::MdxJsxTextElement(e) => e.name.as_deref() == Some(name),
            _ => false,
        };
        if matches {
            return Some(node);
        }
        node.children()?.iter().find_map(|c| find_named(c, name))
    }

    /// The exact #4232 source: the parse really produces the paragraphs, and
    /// the pass removes all of them.
    #[test]
    fn unwraps_the_issue_repro() {
        let mut tree = parse(
            "<table>\n<thead><tr><th>A</th></tr></thead>\n<tbody>\n<tr><td>B</td></tr>\n</tbody>\n</table>\n",
        );
        assert!(
            has_table_paragraph(&tree),
            "precondition: markdown-rs wraps the text-bearing lines: {tree:#?}"
        );

        unwrap_table_paragraphs(&mut tree);

        assert!(!has_table_paragraph(&tree), "{tree:#?}");
        let table = find_named(&tree, "table").expect("table survives");
        let names: Vec<_> = table
            .children()
            .unwrap()
            .iter()
            .filter_map(|c| match c {
                MdastNode::MdxJsxFlowElement(e) => e.name.clone(),
                MdastNode::MdxJsxTextElement(e) => e.name.clone(),
                _ => None,
            })
            .collect();
        assert_eq!(names, ["thead", "tbody"]);
        assert!(find_named(&tree, "th").is_some() && find_named(&tree, "td").is_some());
    }

    /// Two row lines in one section become ONE paragraph holding both `tr`
    /// with a newline `Text` between them; the newline is dropped.
    #[test]
    fn splices_multiple_rows_and_drops_whitespace_text() {
        let row = |cell: &str| inline("tr", vec![inline("td", vec![text(cell)])]);
        let mut tree = root(vec![flow(
            "tbody",
            vec![para(vec![row("1"), text("\n"), row("2")])],
        )]);

        unwrap_table_paragraphs(&mut tree);

        assert_eq!(tree, root(vec![flow("tbody", vec![row("1"), row("2")])]));
    }

    /// Exploded `<tr>` / `<td>B</td>` / `</tr>` gives `tr > p > td`.
    #[test]
    fn unwraps_an_exploded_row() {
        let mut tree = parse("<table>\n<tbody>\n<tr>\n<td>B</td>\n</tr>\n</tbody>\n</table>\n");

        unwrap_table_paragraphs(&mut tree);

        assert!(!has_table_paragraph(&tree), "{tree:#?}");
        let tr = find_named(&tree, "tr").expect("tr");
        assert!(
            matches!(tr.children().unwrap().as_slice(), [MdastNode::MdxJsxTextElement(e)] if e.name.as_deref() == Some("td")),
            "{tr:#?}"
        );
    }

    /// Paragraphs are valid flow content of a cell: `td`/`th`/`caption` keep
    /// theirs, even while the surrounding row is normalised.
    #[test]
    fn keeps_paragraphs_inside_cells() {
        let cell = flow("td", vec![para(vec![text("cell prose")])]);
        let caption = flow("caption", vec![para(vec![text("cap")])]);
        let mut tree = root(vec![flow(
            "table",
            vec![caption, flow("tbody", vec![flow("tr", vec![cell])])],
        )]);
        let expected = tree.clone();

        unwrap_table_paragraphs(&mut tree);

        assert_eq!(tree, expected);
    }

    /// A capitalised component is not table structure, whatever its name.
    #[test]
    fn leaves_components_alone() {
        let mut tree = root(vec![flow(
            "Table",
            vec![para(vec![inline("Row", vec![text("x")])])],
        )]);
        let expected = tree.clone();

        unwrap_table_paragraphs(&mut tree);

        assert_eq!(tree, expected);
    }

    /// Top-level prose and a GFM pipe table are untouched.
    #[test]
    fn leaves_prose_and_gfm_tables_alone() {
        let mut tree = root(vec![
            para(vec![text("before")]),
            MdastNode::Table(Table {
                children: vec![],
                align: vec![],
                position: None,
            }),
            para(vec![text("after")]),
        ]);
        let expected = tree.clone();

        unwrap_table_paragraphs(&mut tree);

        assert_eq!(tree, expected);
    }

    /// The spliced children keep their own source positions, so a
    /// diagnostic about a cell still points at the author's line.
    #[test]
    fn spliced_children_keep_their_positions() {
        let mut tree = parse("<table>\n<tbody>\n<tr><td>B</td></tr>\n</tbody>\n</table>\n");

        unwrap_table_paragraphs(&mut tree);

        let tr = find_named(&tree, "tr").expect("tr");
        let Some(Position { start, .. }) = tr.position() else {
            panic!("tr lost its position: {tr:#?}");
        };
        assert_eq!((start.line, start.column), (3, 1));
    }

    /// Tables nested in other containers (a component body) are reached.
    #[test]
    fn reaches_tables_nested_in_components() {
        let mut tree = root(vec![flow(
            "Note",
            vec![flow(
                "table",
                vec![para(vec![inline("tbody", vec![])]), text("\n")],
            )],
        )]);

        unwrap_table_paragraphs(&mut tree);

        assert_eq!(
            tree,
            root(vec![flow(
                "Note",
                vec![flow("table", vec![inline("tbody", vec![]), text("\n")])],
            )])
        );
    }
}
