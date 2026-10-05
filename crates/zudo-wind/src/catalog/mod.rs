//! Versioned, deterministic utility vocabulary.

mod arbitrary;
pub mod export;
mod families;
pub mod migration;
mod resolve;

use std::collections::BTreeSet;

use crate::TokenCategory;
pub use resolve::{Declaration, Resolution, ResolvedRule};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectorShape {
    OwnElement,
    LaterVisibleSiblings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueKind {
    Exact,
    Keyword,
    Token,
    Scale,
    Fraction,
    Arbitrary,
    Integer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValueGrammar {
    pub accepted_kinds: Vec<ValueKind>,
    pub keywords: Vec<(&'static str, &'static str)>,
    /// Keywords that apply only when no configured token of the same name
    /// exists, so a project token keeps precedence over the constant.
    pub fallback_keywords: Vec<(&'static str, &'static str)>,
    pub token_categories: Vec<TokenCategory>,
    pub arbitrary_property: Option<&'static str>,
    pub allows_fraction_slash: bool,
    pub allows_color_opacity: bool,
}

impl ValueGrammar {
    pub(crate) fn new(
        kinds: &[ValueKind],
        keywords: &[(&'static str, &'static str)],
        categories: &[TokenCategory],
        arbitrary: Option<&'static str>,
        fraction: bool,
    ) -> Self {
        Self {
            accepted_kinds: kinds.to_vec(),
            keywords: keywords.to_vec(),
            fallback_keywords: Vec::new(),
            token_categories: categories.to_vec(),
            arbitrary_property: arbitrary,
            allows_fraction_slash: fraction,
            allows_color_opacity: categories.contains(&TokenCategory::Color),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Example {
    pub candidate: String,
    pub declarations: Vec<Declaration>,
}

/// A value source for one emitted declaration. Most utilities use the
/// candidate's resolved value for every property; a few v1 families need
/// fixed companion declarations or an optional paired font-size leading.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmissionValue {
    Resolved,
    Fixed(&'static str),
    OptionalFontSizeLeading,
    TransitionDuration,
    TransitionTimingFunction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeclarationTemplate {
    pub property: &'static str,
    pub value: EmissionValue,
}

/// Registration metadata needed by the stylesheet emitter when a rule uses
/// CSS individual translate properties.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Registration {
    pub name: &'static str,
    pub syntax: &'static str,
    pub inherits: bool,
    pub initial_value: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogEntry {
    pub id: String,
    pub root: String,
    pub grammar: ValueGrammar,
    /// Properties in declaration order; dynamic values are supplied by the resolver.
    pub emitter: Vec<&'static str>,
    /// Per-property emission details. This extends the batch-A shared-value
    /// shape for v1 entries that have fixed companion values or optional output.
    pub declaration_templates: Vec<DeclarationTemplate>,
    /// CSS registrations required by a resolved rule, deduplicated by the
    /// stylesheet emitter before output.
    pub registrations: Vec<Registration>,
    /// Fixed value for an exact spelling; separate from documentation examples.
    pub fixed_value: Option<String>,
    pub negative: bool,
    pub conflict_group: &'static str,
    pub conflict_group_rank: u16,
    pub order_rank: u16,
    pub selector_shape: SelectorShape,
    pub examples: Vec<Example>,
    pub introduced_in: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CatalogError {
    DuplicateIdentifier(String),
    DuplicateRootKind { root: String, kind: ValueKind },
}

#[derive(Clone, Debug)]
pub struct Catalog {
    entries: Vec<CatalogEntry>,
}

impl Catalog {
    pub fn new(entries: Vec<CatalogEntry>) -> Result<Self, CatalogError> {
        let mut ids = BTreeSet::new();
        let mut claims = BTreeSet::new();
        for entry in &entries {
            if !ids.insert(entry.id.clone()) {
                return Err(CatalogError::DuplicateIdentifier(entry.id.clone()));
            }
            for kind in &entry.grammar.accepted_kinds {
                let qualifications: Vec<String> = match kind {
                    ValueKind::Token => entry
                        .grammar
                        .token_categories
                        .iter()
                        .map(|category| format!("{category:?}"))
                        .collect(),
                    ValueKind::Arbitrary => entry
                        .grammar
                        .arbitrary_property
                        .map(|property| vec![property.to_owned()])
                        .unwrap_or_default(),
                    ValueKind::Keyword => {
                        if entry.grammar.keywords.is_empty() {
                            vec![String::new()]
                        } else {
                            entry
                                .grammar
                                .keywords
                                .iter()
                                .map(|(keyword, _)| format!("keyword:{keyword}"))
                                .collect()
                        }
                    }
                    _ => vec![String::new()],
                };
                for qualification in qualifications {
                    if !claims.insert((entry.root.clone(), *kind as u8, qualification)) {
                        return Err(CatalogError::DuplicateRootKind {
                            root: entry.root.clone(),
                            kind: *kind,
                        });
                    }
                }
            }
        }
        let mut entries = entries;
        entries.sort_by(|a, b| {
            a.conflict_group_rank
                .cmp(&b.conflict_group_rank)
                .then(a.order_rank.cmp(&b.order_rank))
                .then(a.root.cmp(&b.root))
                .then(a.id.cmp(&b.id))
        });
        Ok(Self { entries })
    }

    pub fn v1() -> Self {
        Self::new(families::batch_v1()).expect("v1 catalog must have unique claims")
    }

    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{
        parse_candidate, DiagnosticCode, FontSizeToken, Origin, Resolution, SourcePositionKind,
        TokenConfig, ValidatedTokens, VariantVocabulary,
    };

    fn test_origin() -> Origin {
        Origin::Source {
            source_id: "catalog-test.tsx".to_owned(),
            byte_offset: 0,
            byte_length: 1,
            line: 1,
            byte_column: 1,
            literal_byte_offset: 0,
            literal_byte_length: 1,
            position_kind: SourcePositionKind::Class,
        }
    }

    fn test_tokens() -> ValidatedTokens {
        TokenConfig {
            spacing_unit: Some("0.25rem".to_owned()),
            ..TokenConfig::default()
        }
        .validate()
        .expect("test tokens are valid")
    }

    fn resolve_with(text: &str, tokens: &ValidatedTokens) -> Resolution {
        let candidate = parse_candidate(text, &VariantVocabulary::default()).unwrap();
        Catalog::v1().resolve(&candidate, tokens, &test_origin(), &BTreeSet::new())
    }

    fn rule_with(text: &str, tokens: &ValidatedTokens) -> ResolvedRule {
        match resolve_with(text, tokens) {
            Resolution::Rule(rule) => rule,
            other => panic!("{text}: {other:?}"),
        }
    }

    #[test]
    fn complete_catalog_is_valid_and_ordered() {
        let catalog = Catalog::v1();
        assert!(!catalog.entries().is_empty());
        assert!(catalog
            .entries()
            .windows(2)
            .all(|w| w[0].conflict_group_rank <= w[1].conflict_group_rank));
        let ranks: BTreeSet<_> = catalog
            .entries()
            .iter()
            .map(|entry| entry.conflict_group_rank)
            .collect();
        assert_eq!(ranks, (1..=47).collect::<BTreeSet<u16>>());
    }
    #[test]
    fn duplicate_identifier_is_rejected() {
        let entry = Catalog::v1().entries()[0].clone();
        assert!(matches!(
            Catalog::new(vec![entry.clone(), entry]),
            Err(CatalogError::DuplicateIdentifier(_))
        ));
    }
    #[test]
    fn duplicate_root_and_value_kind_is_rejected() {
        let entry = Catalog::v1().entries()[0].clone();
        let mut second = entry.clone();
        second.id.push_str(".duplicate");
        assert!(matches!(
            Catalog::new(vec![entry, second]),
            Err(CatalogError::DuplicateRootKind { .. })
        ));
    }

    #[test]
    fn every_batch_b_group_has_an_accepted_and_rejected_case() {
        let tokens = test_tokens();
        let cases = [
            ("font-[sans-serif]", "font-missing"),
            ("font-[400]", "font-unknown"),
            ("text-[1rem]", "text-[bogus]"),
            ("leading-[1.5]", "leading-[bogus]"),
            ("tracking-[0.08em]", "tracking-[bogus]"),
            ("text-left", "text-top"),
            ("underline", "italic-bold"),
            ("text-[red]", "text-[url(x)]"),
            ("bg-[#ff6ad5]", "bg-[url(x)]"),
            ("border-2", "border-abc"),
            ("border-[red]", "border-[url(x)]"),
            ("border-dashed", "border-wavy"),
            ("rounded-full", "rounded-missing"),
            ("divide-y", "divide-z"),
            ("divide-[red]", "divide-[url(x)]"),
            ("outline", "outline-wavy"),
            ("outline-2", "outline-wavy"),
            ("outline-[red]", "outline-[url(x)]"),
            ("outline-none", "outline-wavy"),
            ("shadow-none", "shadow-missing"),
            ("opacity-50", "opacity-101"),
            ("transition-colors", "transition-[left;color]"),
            ("duration-200", "duration-60001"),
            ("ease-[ease-in]", "ease-[bogus]"),
            ("translate-x-0", "translate-x-1/0"),
            ("rotate-90", "rotate-361"),
            ("cursor-pointer", "cursor-zoom-in"),
            ("list-inside", "list-start"),
            ("aspect-1/2", "aspect-0/2"),
            ("scroll-mt-2", "scroll-mt-missing"),
            ("sr-only", "sr-only-extra"),
        ];
        for (accepted, rejected_candidate) in cases {
            assert!(
                matches!(resolve_with(accepted, &tokens), Resolution::Rule(_)),
                "expected {accepted} to resolve"
            );
            let rejected = match parse_candidate(rejected_candidate, &VariantVocabulary::default())
            {
                Err(_) => true,
                Ok(candidate) => !matches!(
                    Catalog::v1().resolve(&candidate, &tokens, &test_origin(), &BTreeSet::new()),
                    Resolution::Rule(_)
                ),
            };
            assert!(rejected, "expected {rejected_candidate} to be rejected");
        }
    }

    #[test]
    fn shared_text_root_prefers_statics_then_disjoint_color_and_size_tokens() {
        let tokens = TokenConfig {
            colors: BTreeMap::from([("panel".to_owned(), "#123456".to_owned())]),
            font_sizes: BTreeMap::from([(
                "title".to_owned(),
                FontSizeToken {
                    size: "1.25rem".to_owned(),
                    line_height: Some("1.75rem".to_owned()),
                },
            )]),
            ..TokenConfig::default()
        }
        .validate()
        .expect("disjoint text token categories are valid");
        let align = rule_with("text-left", &tokens);
        let size = rule_with("text-title", &tokens);
        let color = rule_with("text-panel", &tokens);
        assert_eq!(align.declarations[0].property, "text-align");
        assert_eq!(size.declarations[0].property, "font-size");
        assert_eq!(size.declarations[1].property, "line-height");
        assert_eq!(
            size.declarations[1].value,
            "var(--zw-font-size-title-leading)"
        );
        assert_eq!(color.declarations[0].property, "color");
        let ambiguous_value =
            parse_candidate("text-[var(--project-text)]", &VariantVocabulary::default()).unwrap();
        assert!(matches!(
            Catalog::v1().resolve(
                &ambiguous_value,
                &tokens,
                &test_origin(),
                &BTreeSet::new()
            ),
            Resolution::Diagnostic(diagnostic)
                if diagnostic.code == DiagnosticCode::Zw005
                    && diagnostic.rejection_id == Some("R15")
        ));

        let mut ambiguous = TokenConfig::default();
        ambiguous
            .colors
            .insert("shared".to_owned(), "#123456".to_owned());
        ambiguous.font_sizes.insert(
            "shared".to_owned(),
            FontSizeToken {
                size: "1rem".to_owned(),
                line_height: None,
            },
        );
        let diagnostics = ambiguous.validate().unwrap_err();
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.rejection_id == Some("R18") && diagnostic.code == DiagnosticCode::Zw007
        }));
    }

    #[test]
    fn border_width_entries_always_include_their_own_solid_style() {
        let tokens = test_tokens();
        for (root, expected_edges) in [
            ("border", 4),
            ("border-x", 2),
            ("border-y", 2),
            ("border-t", 1),
            ("border-r", 1),
            ("border-b", 1),
            ("border-l", 1),
        ] {
            for candidate in [root.to_owned(), format!("{root}-2")] {
                let declarations = rule_with(&candidate, &tokens).declarations;
                let styles = declarations
                    .iter()
                    .filter(|declaration| declaration.property.ends_with("-style"))
                    .collect::<Vec<_>>();
                assert_eq!(styles.len(), expected_edges, "{candidate}");
                assert!(
                    styles.iter().all(|style| style.value == "solid"),
                    "{candidate}"
                );
                assert_eq!(declarations.len(), expected_edges * 2, "{candidate}");
            }
        }
        assert_eq!(
            rule_with("divide-y", &tokens).selector_shape,
            SelectorShape::LaterVisibleSiblings
        );
        for (candidate, property) in [
            ("divide-x-2", "border-left-width"),
            ("divide-y-2", "border-top-width"),
        ] {
            assert_eq!(
                rule_with(candidate, &tokens).declarations,
                vec![
                    Declaration {
                        property: property.to_owned(),
                        value: "2px".to_owned(),
                    },
                    Declaration {
                        property: property.replace("width", "style"),
                        value: "solid".to_owned(),
                    },
                ]
            );
        }
    }

    #[test]
    fn color_opacity_arbitrary_values_and_noncolor_modifiers_follow_v1() {
        let catalog = Catalog::v1();
        let tokens = test_tokens();
        let background = catalog
            .entries()
            .iter()
            .find(|entry| entry.id == "v1.background")
            .unwrap();
        assert!(background.grammar.allows_color_opacity);
        assert!(!background.grammar.allows_fraction_slash);
        let width = catalog
            .entries()
            .iter()
            .find(|entry| entry.root == "w")
            .unwrap();
        assert!(width.grammar.allows_fraction_slash);
        assert!(!width.grammar.allows_color_opacity);
        assert_eq!(
            rule_with("bg-[#ff6ad5]", &tokens).declarations[0].value,
            "#ff6ad5"
        );
        assert_eq!(
            rule_with("bg-[#ff6ad5]/40", &tokens).declarations[0].value,
            "color-mix(in oklab, #ff6ad5 40%, transparent)"
        );
        assert_eq!(
            rule_with("opacity-[50%]", &tokens).declarations[0].value,
            "50%"
        );
        for text in ["bg-[#ff6ad5]/101", "p-2/50"] {
            let candidate = parse_candidate(text, &VariantVocabulary::default()).unwrap();
            assert!(matches!(
                Catalog::v1().resolve(&candidate, &tokens, &test_origin(), &BTreeSet::new()),
                Resolution::Diagnostic(diagnostic)
                    if diagnostic.code == DiagnosticCode::Zw005
                        && diagnostic.rejection_id == Some("R14")
            ));
        }
        for text in ["opacity-[101%]", "opacity-[1.0001]"] {
            let candidate = parse_candidate(text, &VariantVocabulary::default()).unwrap();
            assert!(matches!(
                Catalog::v1().resolve(&candidate, &tokens, &test_origin(), &BTreeSet::new()),
                Resolution::Diagnostic(diagnostic)
                    if diagnostic.code == DiagnosticCode::Zw005
                        && diagnostic.rejection_id == Some("R15")
            ));
        }
        for text in ["bg-[red;color:red]", "bg-[url(x)]"] {
            match parse_candidate(text, &VariantVocabulary::default()) {
                Err(diagnostic) => assert_eq!(diagnostic.code, DiagnosticCode::Zw005),
                Ok(candidate) => assert!(matches!(
                    Catalog::v1().resolve(&candidate, &tokens, &test_origin(), &BTreeSet::new()),
                    Resolution::Diagnostic(diagnostic)
                        if diagnostic.code == DiagnosticCode::Zw005
                )),
            }
        }
    }

    #[test]
    fn aspect_ratio_and_complete_truncation_statics_are_pinned() {
        let tokens = test_tokens();
        assert_eq!(
            rule_with("aspect-1/2", &tokens).declarations,
            vec![Declaration {
                property: "aspect-ratio".to_owned(),
                value: "1 / 2".to_owned(),
            }]
        );
        assert_eq!(
            rule_with("aspect-[1200/630]", &tokens).declarations,
            vec![Declaration {
                property: "aspect-ratio".to_owned(),
                value: "1200 / 630".to_owned(),
            }]
        );
        for (candidate, value) in [
            ("aspect-1000000/1", "1000000 / 1"),
            ("aspect-16/9", "16 / 9"),
            ("hover:aspect-4/3", "4 / 3"),
            ("aspect-square", "1 / 1"),
            ("aspect-video", "16 / 9"),
            ("aspect-[1.5/2.5]", "1.5 / 2.5"),
        ] {
            assert_eq!(rule_with(candidate, &tokens).declarations[0].value, value);
        }
        for candidate in [
            "aspect-0/2",
            "aspect-2/0",
            "aspect-1000001/1",
            "aspect-1/1000001",
            "-aspect-1/2",
            "aspect-1.5/2",
            "aspect-1/2/3",
            "aspect-[0/2]",
        ] {
            let rejected = match parse_candidate(candidate, &VariantVocabulary::default()) {
                Ok(parsed) => !matches!(
                    Catalog::v1().resolve(&parsed, &tokens, &test_origin(), &BTreeSet::new()),
                    Resolution::Rule(_)
                ),
                Err(_) => true,
            };
            assert!(rejected, "{candidate} should be rejected");
        }
        let decimal = parse_candidate("aspect-1.5", &VariantVocabulary::default()).unwrap();
        assert!(matches!(
            Catalog::v1().resolve(&decimal, &tokens, &test_origin(), &BTreeSet::new()),
            Resolution::Diagnostic(diagnostic)
                if diagnostic.suggested_spelling.as_deref() == Some("aspect-[1.5/1]")
        ));
        let decimal_fraction =
            parse_candidate("aspect-1.5/2", &VariantVocabulary::default()).unwrap();
        assert!(matches!(
            Catalog::v1().resolve(&decimal_fraction, &tokens, &test_origin(), &BTreeSet::new()),
            Resolution::Diagnostic(diagnostic)
                if diagnostic.suggested_spelling.as_deref() == Some("aspect-[1.5/2]")
        ));
        let truncate = rule_with("truncate", &tokens).declarations;
        assert_eq!(
            truncate,
            vec![
                Declaration {
                    property: "overflow".to_owned(),
                    value: "hidden".to_owned(),
                },
                Declaration {
                    property: "text-overflow".to_owned(),
                    value: "ellipsis".to_owned(),
                },
                Declaration {
                    property: "white-space".to_owned(),
                    value: "nowrap".to_owned(),
                },
            ]
        );
        let sr_only = rule_with("sr-only", &tokens).declarations;
        assert_eq!(
            sr_only,
            vec![
                Declaration {
                    property: "position".to_owned(),
                    value: "absolute".to_owned()
                },
                Declaration {
                    property: "width".to_owned(),
                    value: "1px".to_owned()
                },
                Declaration {
                    property: "height".to_owned(),
                    value: "1px".to_owned()
                },
                Declaration {
                    property: "padding".to_owned(),
                    value: "0".to_owned()
                },
                Declaration {
                    property: "margin".to_owned(),
                    value: "-1px".to_owned()
                },
                Declaration {
                    property: "overflow".to_owned(),
                    value: "hidden".to_owned()
                },
                Declaration {
                    property: "clip".to_owned(),
                    value: "rect(0,0,0,0)".to_owned()
                },
                Declaration {
                    property: "white-space".to_owned(),
                    value: "nowrap".to_owned()
                },
                Declaration {
                    property: "border-width".to_owned(),
                    value: "0".to_owned()
                },
            ]
        );
    }

    #[test]
    fn translate_carries_the_locked_noninherited_registrations() {
        let rule = rule_with("translate-x-0", &test_tokens());
        assert_eq!(rule.registrations.len(), 2);
        assert_eq!(rule.registrations[0].name, "--zw-translate-x");
        assert_eq!(rule.registrations[1].name, "--zw-translate-y");
        assert!(rule
            .registrations
            .iter()
            .all(|registration| !registration.inherits));
        assert!(rule.registrations.iter().all(|registration| {
            registration.syntax == "<length-percentage>" && registration.initial_value == "0px"
        }));
    }

    #[test]
    fn transition_defaults_sort_before_explicit_values_and_translate_is_single_axis() {
        let tokens = test_tokens();
        assert_eq!(
            rule_with("transition-[left,color]", &tokens).declarations,
            vec![
                Declaration {
                    property: "transition-property".to_owned(),
                    value: "left,color".to_owned(),
                },
                Declaration {
                    property: "transition-duration".to_owned(),
                    value: "150ms".to_owned(),
                },
                Declaration {
                    property: "transition-timing-function".to_owned(),
                    value: "ease".to_owned(),
                },
            ]
        );
        assert_eq!(
            rule_with("transition-none", &tokens).declarations,
            vec![Declaration {
                property: "transition-property".to_owned(),
                value: "none".to_owned(),
            }]
        );
        let transition = rule_with("transition-colors", &tokens);
        assert_eq!(
            transition.declarations[0].value,
            "color,background-color,border-color,outline-color,text-decoration-color,fill,stroke"
        );
        assert_eq!(
            rule_with("hover:transition-shadow", &tokens).declarations,
            vec![
                Declaration {
                    property: "transition-property".to_owned(),
                    value: "box-shadow".to_owned(),
                },
                Declaration {
                    property: "transition-duration".to_owned(),
                    value: "150ms".to_owned(),
                },
                Declaration {
                    property: "transition-timing-function".to_owned(),
                    value: "ease".to_owned(),
                },
            ]
        );
        let duration = rule_with("duration-200", &tokens);
        let easing = rule_with("ease-[ease-in]", &tokens);
        assert!(transition.conflict_group_rank < duration.conflict_group_rank);
        assert!(duration.conflict_group_rank < easing.conflict_group_rank);
        assert_eq!(
            rule_with("translate-x-1/2", &tokens).declarations[0].value,
            "calc(100% * 1 / 2)"
        );
        let candidate =
            parse_candidate("translate-x-[1px_2px]", &VariantVocabulary::default()).unwrap();
        assert!(matches!(
            Catalog::v1().resolve(&candidate, &tokens, &test_origin(), &BTreeSet::new()),
            Resolution::Diagnostic(diagnostic)
                if diagnostic.code == DiagnosticCode::Zw005
        ));
    }

    #[test]
    fn arbitrary_variables_use_type_correct_validation_placeholders() {
        let tokens = test_tokens();
        for (candidate, expected_property) in [
            ("opacity-[var(--alpha)]", "opacity"),
            ("transition-[var(--properties)]", "transition-property"),
            ("duration-[var(--duration)]", "transition-duration"),
            ("rotate-[var(--angle)]", "rotate"),
            ("aspect-[var(--ratio)]", "aspect-ratio"),
        ] {
            let rule = rule_with(candidate, &tokens);
            assert_eq!(rule.declarations[0].property, expected_property);
            assert_eq!(
                rule.value_status,
                crate::ValueStatus::CategoryUnverified,
                "{candidate}"
            );
            assert_eq!(
                rule.declarations[0].value,
                candidate.split_once("-[").unwrap().1.trim_end_matches(']')
            );
        }
    }

    #[test]
    fn recognized_unsupported_exact_families_use_r20() {
        let tokens = test_tokens();
        for text in ["ring", "animate", "scale", "transform"] {
            let candidate = parse_candidate(text, &VariantVocabulary::default()).unwrap();
            assert!(matches!(
                Catalog::v1().resolve(&candidate, &tokens, &test_origin(), &BTreeSet::new()),
                Resolution::Diagnostic(diagnostic)
                    if diagnostic.code == DiagnosticCode::Zw004
                        && diagnostic.rejection_id == Some("R20")
            ));
        }
    }
}
