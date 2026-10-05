mod alignment;
mod border;
mod color;
mod effects;
mod flex_grid;
mod gap;
mod interaction;
mod layout;
mod motion;
mod overflow;
mod sizing;
mod spacing;
mod statics;
mod typography;
mod z_index;

use super::{
    CatalogEntry, Declaration, DeclarationTemplate, DefaultOverride, EmissionValue, Example,
    Registration, SelectorShape, ValueGrammar, ValueKind,
};
use crate::TokenCategory;

pub(super) fn batch_v1() -> Vec<CatalogEntry> {
    let mut entries = Vec::new();
    layout::add(&mut entries);
    flex_grid::add(&mut entries);
    alignment::add(&mut entries);
    sizing::add(&mut entries);
    spacing::add(&mut entries);
    gap::add(&mut entries);
    overflow::add(&mut entries);
    z_index::add(&mut entries);
    typography::add(&mut entries);
    color::add(&mut entries);
    border::add(&mut entries);
    effects::add(&mut entries);
    motion::add(&mut entries);
    interaction::add(&mut entries);
    statics::add(&mut entries);
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
    let declaration_templates = properties
        .iter()
        .map(|property| DeclarationTemplate {
            property,
            value: EmissionValue::Resolved,
        })
        .collect();
    CatalogEntry {
        id: format!("v1.{root}"),
        root: root.to_owned(),
        grammar: ValueGrammar::new(kinds, keywords, categories, arbitrary_property, fraction),
        emitter: properties.to_vec(),
        declaration_templates,
        default_overrides: Vec::new(),
        registrations: Vec::new(),
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

pub(super) fn set_id(entry: &mut CatalogEntry, id: &str) {
    entry.id = id.to_owned();
}

pub(super) fn set_templates(
    entry: &mut CatalogEntry,
    templates: &[DeclarationTemplate],
    example: &[(&str, &str)],
) {
    entry.emitter = templates.iter().map(|template| template.property).collect();
    entry.declaration_templates = templates.to_vec();
    entry.examples[0].declarations = example
        .iter()
        .map(|(property, value)| Declaration {
            property: (*property).to_owned(),
            value: (*value).to_owned(),
        })
        .collect();
}

pub(super) fn resolved(property: &'static str) -> DeclarationTemplate {
    DeclarationTemplate {
        property,
        value: EmissionValue::Resolved,
    }
}

pub(super) fn fixed(property: &'static str, value: &'static str) -> DeclarationTemplate {
    DeclarationTemplate {
        property,
        value: EmissionValue::Fixed(value),
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
