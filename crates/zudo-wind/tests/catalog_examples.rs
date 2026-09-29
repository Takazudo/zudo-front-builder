use std::collections::BTreeSet;

use zudo_wind::{
    parse_candidate, Catalog, DiagnosticCode, Origin, Resolution, Severity, SourcePositionKind,
    TokenCategory, TokenConfig, ValueKind, ValueStatus, VariantVocabulary,
};

fn tokens() -> zudo_wind::ValidatedTokens {
    TokenConfig {
        spacing_unit: Some("0.25rem".to_owned()),
        ..TokenConfig::default()
    }
    .validate()
    .unwrap()
}

fn origin(kind: SourcePositionKind) -> Origin {
    Origin::Source {
        source_id: "test.tsx".to_owned(),
        byte_offset: 0,
        byte_length: 1,
        line: 1,
        byte_column: 1,
        literal_byte_offset: 0,
        literal_byte_length: 1,
        position_kind: kind,
    }
}

fn resolve(text: &str, origin: &Origin) -> Resolution {
    let candidate = parse_candidate(text, &VariantVocabulary::default()).unwrap();
    Catalog::v1().resolve(&candidate, &tokens(), origin, &BTreeSet::new())
}

fn rule(text: &str) -> zudo_wind::ResolvedRule {
    match resolve(text, &origin(SourcePositionKind::Class)) {
        Resolution::Rule(rule) => rule,
        other => panic!("{text}: {other:?}"),
    }
}

#[test]
fn every_catalog_example_resolves_to_its_declared_output() {
    let catalog = Catalog::v1();
    let tokens = tokens();
    for entry in catalog.entries() {
        assert!(!entry.examples.is_empty(), "{}", entry.id);
        for example in &entry.examples {
            let candidate =
                parse_candidate(&example.candidate, &VariantVocabulary::default()).unwrap();
            match catalog.resolve(
                &candidate,
                &tokens,
                &origin(SourcePositionKind::Class),
                &BTreeSet::new(),
            ) {
                Resolution::Rule(rule) => {
                    assert_eq!(rule.entry_id, entry.id, "{}", example.candidate);
                    assert_eq!(
                        rule.declarations, example.declarations,
                        "{}",
                        example.candidate
                    );
                }
                other => panic!("{}: {other:?}", example.candidate),
            }
        }
    }
}

#[test]
fn every_batch_a_group_has_accepted_and_rejected_examples() {
    let cases = [
        ("block", "block-unknown"),
        ("relative", "relative-unknown"),
        ("inset-x-0", "inset-x-missing"),
        ("flex-col", "flex-col-unknown"),
        ("grow-0", "grow-2"),
        ("grid-cols-2", "grid-cols-13"),
        ("items-start", "items-between"),
        ("max-w-none", "max-w-missing"),
        ("p-4", "p-missing"),
        ("m-auto", "m-missing"),
        ("gap-x-4", "gap-x-missing"),
        ("space-y-2", "space-y-missing"),
        ("overflow-hidden", "overflow-missing"),
        ("z-2", "z-2147483648"),
    ];
    for (accepted, rejected) in cases {
        assert!(
            matches!(
                resolve(accepted, &origin(SourcePositionKind::Class)),
                Resolution::Rule(_)
            ),
            "{accepted}"
        );
        assert!(
            matches!(
                resolve(rejected, &origin(SourcePositionKind::Class)),
                Resolution::Diagnostic(_)
            ),
            "{rejected}"
        );
    }
}

#[test]
fn longest_roots_and_ranks_are_stable() {
    for (text, root) in [
        ("gap-x-4", "gap-x"),
        ("min-w-0", "min-w"),
        ("inset-x-0", "inset-x"),
        ("max-w-none", "max-w"),
    ] {
        assert_eq!(rule(text).root, root);
    }
    for triple in [["p-4", "px-2", "pl-1"], ["m-4", "mx-2", "ml-1"]] {
        let rules = triple.map(rule);
        assert_eq!(rules[0].conflict_group, rules[1].conflict_group);
        assert_eq!(rules[1].conflict_group, rules[2].conflict_group);
        assert!(
            rules[0].order_rank < rules[1].order_rank && rules[1].order_rank < rules[2].order_rank
        );
    }
    let gap = ["gap-4", "gap-x-2", "gap-y-1"].map(rule);
    assert!(gap[0].order_rank < gap[1].order_rank && gap[0].order_rank < gap[2].order_rank);
}

#[test]
fn fractions_negatives_and_arbitrary_values_obey_family_rules() {
    assert_eq!(rule("w-1/2").declarations[0].value, "calc(100% * 1 / 2)");
    assert_eq!(rule("-left-[1px]").declarations[0].value, "-1px");
    assert_eq!(
        rule("grid-cols-[auto_1fr]").declarations[0].value,
        "auto 1fr"
    );
    assert_eq!(
        rule("w-[var(--project-width)]").value_status,
        ValueStatus::CategoryUnverified
    );
    for text in [
        "-p-2",
        "p-2/50",
        "w-1/0",
        "w-[red]",
        "p-[-1px]",
        "p-1234567890123456789",
    ] {
        assert!(
            matches!(
                resolve(text, &origin(SourcePositionKind::Class)),
                Resolution::Diagnostic(_)
            ),
            "{text}"
        );
    }
}

#[test]
fn origin_controls_failure_and_audit_severity() {
    let safelist = Origin::Safelist {
        owner: "test".to_owned(),
        index: 0,
    };
    assert!(matches!(
        resolve("p-missing", &safelist),
        Resolution::Failure(_)
    ));
    match resolve("p-missing", &origin(SourcePositionKind::Class)) {
        Resolution::Diagnostic(diagnostic) => {
            assert_eq!(diagnostic.code, DiagnosticCode::Zw006);
            assert_eq!(diagnostic.severity, Severity::Error);
        }
        other => panic!("{other:?}"),
    }
    match resolve("p-missing", &origin(SourcePositionKind::Literal)) {
        Resolution::Diagnostic(diagnostic) => assert_eq!(diagnostic.severity, Severity::AuditInfo),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        resolve("ordinary-class", &safelist),
        Resolution::Failure(_)
    ));
    assert!(matches!(
        resolve("ordinary-class", &origin(SourcePositionKind::Class)),
        Resolution::NotUtility
    ));
    assert!(matches!(
        resolve("group", &safelist),
        Resolution::NotUtility
    ));
    assert!(matches!(resolve("peer", &safelist), Resolution::NotUtility));
    let candidate = parse_candidate("p-missing", &VariantVocabulary::default()).unwrap();
    assert!(matches!(
        Catalog::v1().resolve(
            &candidate,
            &tokens(),
            &safelist,
            &BTreeSet::from(["p-missing".to_owned()])
        ),
        Resolution::NotUtility
    ));
}

#[test]
fn representative_declarations_follow_the_locked_family_table() {
    assert_eq!(
        rule("flex-col-reverse").declarations[0].value,
        "column-reverse"
    );
    assert_eq!(
        rule("grid-cols-2").declarations[0].value,
        "repeat(2,minmax(0,1fr))"
    );
    assert_eq!(rule("col-span-3").declarations[0].value, "span 3 / span 3");
    assert_eq!(rule("items-start").declarations[0].value, "flex-start");
    assert_eq!(
        rule("justify-between").declarations[0].value,
        "space-between"
    );
    assert_eq!(rule("w-screen").declarations[0].value, "100vw");
    assert_eq!(
        rule("size-8").declarations,
        vec![
            zudo_wind::Declaration {
                property: "width".to_owned(),
                value: "2rem".to_owned()
            },
            zudo_wind::Declaration {
                property: "height".to_owned(),
                value: "2rem".to_owned()
            },
        ]
    );
    assert_eq!(
        rule("p-4")
            .declarations
            .iter()
            .map(|d| d.property.as_str())
            .collect::<Vec<_>>(),
        vec![
            "padding-top",
            "padding-right",
            "padding-bottom",
            "padding-left"
        ]
    );
    assert_eq!(rule("-mt-2").declarations[0].value, "-0.5rem");
    assert_eq!(
        rule("gap-4")
            .declarations
            .iter()
            .map(|d| d.property.as_str())
            .collect::<Vec<_>>(),
        vec!["column-gap", "row-gap"]
    );
    assert_eq!(
        rule("space-y-2").selector_shape,
        zudo_wind::SelectorShape::LaterVisibleSiblings
    );
    assert_eq!(
        rule("overscroll-x-contain").declarations[0].property,
        "overscroll-behavior-x"
    );
    assert_eq!(rule("-z-2").declarations[0].value, "-2");
}

#[test]
fn tokens_and_arbitrary_values_preserve_validation_status() {
    let mut config = TokenConfig {
        spacing_unit: Some("0.25rem".to_owned()),
        ..TokenConfig::default()
    };
    config
        .sizes
        .insert("sidebar".to_owned(), "var(--project-sidebar)".to_owned());
    config
        .spacing
        .insert("gutter".to_owned(), "2rem".to_owned());
    let tokens = config.validate().unwrap();
    let catalog = Catalog::v1();
    for (text, expected, status) in [
        (
            "w-sidebar",
            "var(--zw-size-sidebar)",
            ValueStatus::CategoryUnverified,
        ),
        (
            "w-gutter",
            "var(--zw-spacing-gutter)",
            ValueStatus::Verified,
        ),
    ] {
        let candidate = parse_candidate(text, &VariantVocabulary::default()).unwrap();
        match catalog.resolve(
            &candidate,
            &tokens,
            &origin(SourcePositionKind::Class),
            &BTreeSet::new(),
        ) {
            Resolution::Rule(rule) => {
                assert_eq!(rule.declarations[0].value, expected);
                assert_eq!(rule.value_status, status);
            }
            other => panic!("{text}: {other:?}"),
        }
    }
    for text in [
        "w-[red_var(--x)]",
        "w-[var(--x)_garbage]",
        "w-[url(test.png)]",
    ] {
        if let Ok(candidate) = parse_candidate(text, &VariantVocabulary::default()) {
            assert!(
                matches!(
                    catalog.resolve(
                        &candidate,
                        &tokens,
                        &origin(SourcePositionKind::Class),
                        &BTreeSet::new()
                    ),
                    Resolution::Diagnostic(_)
                ),
                "{text}"
            );
        }
    }
}

#[test]
fn shared_roots_choose_disjoint_categories_and_reject_ambiguous_arbitrary_values() {
    let source = Catalog::v1();
    let mut size = source
        .entries()
        .iter()
        .find(|entry| entry.root == "w")
        .unwrap()
        .clone();
    size.id = "v1.probe.size".to_owned();
    size.root = "probe".to_owned();
    size.grammar.accepted_kinds = vec![ValueKind::Token, ValueKind::Arbitrary];
    size.grammar.token_categories = vec![TokenCategory::Size];
    size.grammar.keywords.clear();
    let mut spacing = size.clone();
    spacing.id = "v1.probe.spacing".to_owned();
    spacing.grammar.token_categories = vec![TokenCategory::Spacing];
    spacing.grammar.arbitrary_property = Some("padding");
    let catalog = Catalog::new(vec![size, spacing]).unwrap();
    let mut config = TokenConfig::default();
    config
        .sizes
        .insert("sidebar".to_owned(), "12rem".to_owned());
    config
        .spacing
        .insert("gutter".to_owned(), "2rem".to_owned());
    let tokens = config.validate().unwrap();
    for (text, id) in [
        ("probe-sidebar", "v1.probe.size"),
        ("probe-gutter", "v1.probe.spacing"),
    ] {
        let candidate = parse_candidate(text, &VariantVocabulary::default()).unwrap();
        match catalog.resolve(
            &candidate,
            &tokens,
            &origin(SourcePositionKind::Class),
            &BTreeSet::new(),
        ) {
            Resolution::Rule(rule) => assert_eq!(rule.entry_id, id),
            other => panic!("{text}: {other:?}"),
        }
    }
    let candidate = parse_candidate("probe-[var(--x)]", &VariantVocabulary::default()).unwrap();
    assert!(matches!(
        catalog.resolve(
            &candidate,
            &tokens,
            &origin(SourcePositionKind::Class),
            &BTreeSet::new()
        ),
        Resolution::Diagnostic(_)
    ));
}
