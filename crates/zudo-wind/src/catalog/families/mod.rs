mod alignment;
mod flex_grid;
mod gap;
mod layout;
mod overflow;
mod sizing;
mod spacing;
mod z_index;

use super::{CatalogEntry, Declaration, Example, SelectorShape, ValueGrammar, ValueKind};
use crate::TokenCategory;

pub(super) fn batch_a() -> Vec<CatalogEntry> {
    let mut entries = Vec::new();
    layout::add(&mut entries);
    flex_grid::add(&mut entries);
    alignment::add(&mut entries);
    sizing::add(&mut entries);
    spacing::add(&mut entries);
    gap::add(&mut entries);
    overflow::add(&mut entries);
    z_index::add(&mut entries);
    entries
}

#[allow(clippy::too_many_arguments)]
pub(super) fn entry(
    root: &str,
    group: &'static str,
    group_rank: u16,
    rank: u16,
    properties: &[&'static str],
    kinds: &[ValueKind],
    keywords: &[(&'static str, &'static str)],
    categories: &[TokenCategory],
    arbitrary_property: Option<&'static str>,
    fraction: bool,
    negative: bool,
    selector_shape: SelectorShape,
    example_suffix: Option<&str>,
    example_value: &str,
) -> CatalogEntry {
    let candidate = match example_suffix {
        Some(suffix) => format!("{root}-{suffix}"),
        None => root.to_owned(),
    };
    CatalogEntry {
        id: format!("v1.{root}"),
        root: root.to_owned(),
        grammar: ValueGrammar::new(kinds, keywords, categories, arbitrary_property, fraction),
        emitter: properties.to_vec(),
        fixed_value: kinds
            .contains(&ValueKind::Exact)
            .then(|| example_value.to_owned()),
        negative,
        conflict_group: group,
        conflict_group_rank: group_rank,
        order_rank: rank,
        selector_shape,
        examples: vec![Example {
            candidate,
            declarations: properties
                .iter()
                .map(|property| Declaration {
                    property: (*property).to_owned(),
                    value: example_value.to_owned(),
                })
                .collect(),
        }],
        introduced_in: crate::SPEC_VERSION,
    }
}

pub(super) const OWN: SelectorShape = SelectorShape::OwnElement;
pub(super) const CHILD: SelectorShape = SelectorShape::LaterVisibleSiblings;
pub(super) const SPACING: &[ValueKind] = &[
    ValueKind::Keyword,
    ValueKind::Token,
    ValueKind::Scale,
    ValueKind::Arbitrary,
];
pub(super) const STATIC: &[ValueKind] = &[ValueKind::Exact];
