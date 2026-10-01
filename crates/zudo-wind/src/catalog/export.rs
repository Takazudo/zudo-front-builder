//! Stable JSON representation of the complete v1 catalog.

use serde::Serialize;

use super::{Catalog, EmissionValue, ValueKind};
use crate::{TokenCategory, SPEC_REVISION, SPEC_VERSION};

pub const CATALOG_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogExport {
    pub schema_version: u32,
    pub spec_version: u32,
    pub spec_revision: u32,
    pub entries: Vec<CatalogEntryExport>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntryExport {
    pub id: String,
    pub root: String,
    pub grammar: GrammarExport,
    pub accepted_values: Vec<AcceptedValueExport>,
    pub token_categories: Vec<String>,
    pub negative_policy: String,
    pub conflict_group: String,
    pub conflict_group_rank: u16,
    pub order_rank: u16,
    pub selector_shape: String,
    pub emitter: Vec<String>,
    pub declaration_templates: Vec<DeclarationTemplateExport>,
    pub registrations: Vec<RegistrationExport>,
    pub spec_version: u32,
    pub examples: Vec<ExampleExport>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrammarExport {
    pub accepted_kinds: Vec<String>,
    pub arbitrary_property: Option<String>,
    pub allows_fraction_slash: bool,
    pub allows_color_opacity: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedValueExport {
    pub kind: String,
    pub suffix: Option<String>,
    pub emitted_value: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExampleExport {
    pub candidate: String,
    pub declarations: Vec<DeclarationExport>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclarationExport {
    pub property: String,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclarationTemplateExport {
    pub property: String,
    pub value_kind: String,
    pub fixed_value: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationExport {
    pub name: String,
    pub syntax: String,
    pub inherits: bool,
    pub initial_value: String,
}

impl CatalogExport {
    /// Projects catalog metadata in the catalog's canonical order. All maps
    /// represented here are emitted as sorted vectors, never hash iteration.
    pub fn v1() -> Self {
        let catalog = Catalog::v1();
        Self {
            schema_version: CATALOG_SCHEMA_VERSION,
            spec_version: SPEC_VERSION,
            spec_revision: SPEC_REVISION,
            entries: catalog.entries().iter().map(export_entry).collect(),
        }
    }
}

pub fn export_json_pretty() -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&CatalogExport::v1())
}

fn export_entry(entry: &super::CatalogEntry) -> CatalogEntryExport {
    let mut accepted_values = Vec::new();
    if entry.grammar.accepted_kinds.contains(&ValueKind::Exact) {
        accepted_values.push(AcceptedValueExport {
            kind: "exact".to_owned(),
            suffix: None,
            emitted_value: entry.fixed_value.clone(),
        });
    }
    accepted_values.extend(entry.grammar.keywords.iter().map(|(suffix, value)| {
        AcceptedValueExport {
            kind: "keyword".to_owned(),
            suffix: (!suffix.is_empty()).then(|| (*suffix).to_owned()),
            emitted_value: Some((*value).to_owned()),
        }
    }));

    CatalogEntryExport {
        id: entry.id.clone(),
        root: entry.root.clone(),
        grammar: GrammarExport {
            accepted_kinds: entry
                .grammar
                .accepted_kinds
                .iter()
                .map(value_kind_name)
                .map(str::to_owned)
                .collect(),
            arbitrary_property: entry.grammar.arbitrary_property.map(str::to_owned),
            allows_fraction_slash: entry.grammar.allows_fraction_slash,
            allows_color_opacity: entry.grammar.allows_color_opacity,
        },
        accepted_values,
        token_categories: entry
            .grammar
            .token_categories
            .iter()
            .map(token_category_name)
            .map(str::to_owned)
            .collect(),
        negative_policy: if entry.negative {
            "allowed"
        } else {
            "forbidden"
        }
        .to_owned(),
        conflict_group: entry.conflict_group.to_owned(),
        conflict_group_rank: entry.conflict_group_rank,
        order_rank: entry.order_rank,
        selector_shape: match entry.selector_shape {
            super::SelectorShape::OwnElement => "ownElement",
            super::SelectorShape::LaterVisibleSiblings => "laterVisibleSiblings",
        }
        .to_owned(),
        emitter: entry
            .emitter
            .iter()
            .map(|property| (*property).to_owned())
            .collect(),
        declaration_templates: entry
            .declaration_templates
            .iter()
            .map(|template| {
                let (value_kind, fixed_value) = match template.value {
                    EmissionValue::Resolved => ("resolved", None),
                    EmissionValue::Fixed(value) => ("fixed", Some(value.to_owned())),
                    EmissionValue::OptionalFontSizeLeading => ("optionalFontSizeLeading", None),
                    EmissionValue::TransitionDuration => ("transitionDuration", None),
                    EmissionValue::TransitionTimingFunction => ("transitionTimingFunction", None),
                };
                DeclarationTemplateExport {
                    property: template.property.to_owned(),
                    value_kind: value_kind.to_owned(),
                    fixed_value,
                }
            })
            .collect(),
        registrations: entry
            .registrations
            .iter()
            .map(|registration| RegistrationExport {
                name: registration.name.to_owned(),
                syntax: registration.syntax.to_owned(),
                inherits: registration.inherits,
                initial_value: registration.initial_value.to_owned(),
            })
            .collect(),
        spec_version: entry.introduced_in,
        examples: entry
            .examples
            .iter()
            .map(|example| ExampleExport {
                candidate: example.candidate.clone(),
                declarations: example
                    .declarations
                    .iter()
                    .map(|declaration| DeclarationExport {
                        property: declaration.property.clone(),
                        value: declaration.value.clone(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn value_kind_name(kind: &ValueKind) -> &'static str {
    match kind {
        ValueKind::Exact => "exact",
        ValueKind::Keyword => "keyword",
        ValueKind::Token => "token",
        ValueKind::Scale => "scale",
        ValueKind::Fraction => "fraction",
        ValueKind::Arbitrary => "arbitrary",
        ValueKind::Integer => "integer",
    }
}

fn token_category_name(category: &TokenCategory) -> &'static str {
    match category {
        TokenCategory::Color => "colors",
        TokenCategory::Spacing => "spacing",
        TokenCategory::Size => "sizes",
        TokenCategory::FontSize => "fontSizes",
        TokenCategory::FontFamily => "fontFamilies",
        TokenCategory::FontWeight => "fontWeights",
        TokenCategory::LineHeight => "lineHeights",
        TokenCategory::LetterSpacing => "letterSpacings",
        TokenCategory::Radius => "radii",
        TokenCategory::Shadow => "shadows",
        TokenCategory::ZIndex => "zIndices",
        TokenCategory::Easing => "easings",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_includes_every_entry_and_example_in_catalog_order() {
        let catalog = Catalog::v1();
        let export = CatalogExport::v1();
        assert_eq!(export.entries.len(), catalog.entries().len());
        assert_eq!(
            export
                .entries
                .iter()
                .map(|entry| entry.examples.len())
                .sum::<usize>(),
            catalog
                .entries()
                .iter()
                .map(|entry| entry.examples.len())
                .sum::<usize>()
        );
        for (exported, source) in export.entries.iter().zip(catalog.entries()) {
            assert_eq!(exported.id, source.id);
            assert_eq!(
                exported.token_categories,
                source
                    .grammar
                    .token_categories
                    .iter()
                    .map(token_category_name)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                exported.grammar.allows_fraction_slash, source.grammar.allows_fraction_slash,
                "{}",
                source.id
            );
            assert_eq!(
                exported.grammar.allows_color_opacity, source.grammar.allows_color_opacity,
                "{}",
                source.id
            );
            assert_eq!(
                exported.examples.len(),
                source.examples.len(),
                "{}",
                source.id
            );
            for (exported_example, source_example) in exported.examples.iter().zip(&source.examples)
            {
                assert_eq!(exported_example.candidate, source_example.candidate);
                assert_eq!(
                    exported_example
                        .declarations
                        .iter()
                        .map(|declaration| (&declaration.property, &declaration.value))
                        .collect::<Vec<_>>(),
                    source_example
                        .declarations
                        .iter()
                        .map(|declaration| (&declaration.property, &declaration.value))
                        .collect::<Vec<_>>()
                );
            }
            assert_eq!(
                exported.declaration_templates.len(),
                source.declaration_templates.len(),
                "{}",
                source.id
            );
            assert_eq!(
                exported.registrations.len(),
                source.registrations.len(),
                "{}",
                source.id
            );
            assert_eq!(
                exported.emitter,
                source
                    .emitter
                    .iter()
                    .map(|property| (*property).to_owned())
                    .collect::<Vec<_>>()
            );
            for (exported_template, source_template) in exported
                .declaration_templates
                .iter()
                .zip(&source.declaration_templates)
            {
                assert_eq!(exported_template.property, source_template.property);
                match source_template.value {
                    EmissionValue::Resolved => {
                        assert_eq!(exported_template.value_kind, "resolved");
                        assert_eq!(exported_template.fixed_value, None);
                    }
                    EmissionValue::Fixed(value) => {
                        assert_eq!(exported_template.value_kind, "fixed");
                        assert_eq!(exported_template.fixed_value.as_deref(), Some(value));
                    }
                    EmissionValue::OptionalFontSizeLeading => {
                        assert_eq!(exported_template.value_kind, "optionalFontSizeLeading");
                        assert_eq!(exported_template.fixed_value, None);
                    }
                    EmissionValue::TransitionDuration => {
                        assert_eq!(exported_template.value_kind, "transitionDuration");
                        assert_eq!(exported_template.fixed_value, None);
                    }
                    EmissionValue::TransitionTimingFunction => {
                        assert_eq!(exported_template.value_kind, "transitionTimingFunction");
                        assert_eq!(exported_template.fixed_value, None);
                    }
                }
            }
            for (exported_registration, source_registration) in
                exported.registrations.iter().zip(&source.registrations)
            {
                assert_eq!(exported_registration.name, source_registration.name);
                assert_eq!(exported_registration.syntax, source_registration.syntax);
                assert_eq!(exported_registration.inherits, source_registration.inherits);
                assert_eq!(
                    exported_registration.initial_value,
                    source_registration.initial_value
                );
            }
        }
    }

    #[test]
    fn export_json_is_deterministic() {
        assert_eq!(export_json_pretty().unwrap(), export_json_pretty().unwrap());
    }
}
