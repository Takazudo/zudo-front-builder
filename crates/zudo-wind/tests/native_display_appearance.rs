use std::collections::BTreeMap;

use zudo_wind::{
    compile, extract_candidates, BreakpointConfig, CompileInput, DiagnosticCode, Origin,
    OriginCandidate, ResetMode, Severity, SourceKind, SourcePositionKind, ValueKind, WindConfig,
    LAYER_ORDER,
};

const DISPLAY: &[(&str, &str)] = &[
    ("contents", "contents"),
    ("flow-root", "flow-root"),
    ("list-item", "list-item"),
    ("table", "table"),
    ("inline-table", "inline-table"),
    ("table-caption", "table-caption"),
    ("table-cell", "table-cell"),
    ("table-column", "table-column"),
    ("table-column-group", "table-column-group"),
    ("table-footer-group", "table-footer-group"),
    ("table-header-group", "table-header-group"),
    ("table-row", "table-row"),
    ("table-row-group", "table-row-group"),
];

const APPEARANCE: &[(&str, &str)] = &[("appearance-none", "none"), ("appearance-auto", "auto")];

fn source(text: &str, position_kind: SourcePositionKind) -> Origin {
    Origin::Source {
        source_id: "native-display-appearance.tsx".into(),
        byte_offset: 0,
        byte_length: text.len(),
        line: 1,
        byte_column: 1,
        literal_byte_offset: 0,
        literal_byte_length: text.len(),
        position_kind,
    }
}

fn origin_candidate(text: &str, origin: Origin) -> OriginCandidate {
    OriginCandidate {
        text: text.into(),
        origin,
    }
}

fn run(names: &[&str], config: WindConfig, origin: Origin) -> zudo_wind::CompileResult {
    compile(&CompileInput {
        candidates: names
            .iter()
            .map(|name| origin_candidate(name, origin.clone()))
            .collect(),
        config,
    })
}

fn declaration(result: &zudo_wind::CompileResult, candidate: &str) -> Vec<(String, String)> {
    let rule = result
        .rules
        .iter()
        .find(|rule| rule.candidate == candidate)
        .unwrap_or_else(|| panic!("missing rule for {candidate}"));
    rule.resolved
        .as_ref()
        .unwrap_or_else(|| panic!("missing utility resolution for {candidate}"))
        .declarations
        .iter()
        .map(|item| (item.property.clone(), item.value.clone()))
        .collect()
}

#[test]
fn approved_display_and_appearance_forms_are_exact_empty_token_static_entries() {
    let names: Vec<_> = DISPLAY
        .iter()
        .chain(APPEARANCE)
        .map(|(candidate, _)| *candidate)
        .collect();
    let result = run(
        &names,
        WindConfig::default(),
        source("contents", SourcePositionKind::Class),
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.rules.len(), names.len());
    assert_eq!(result.parts.reset, "");
    assert_eq!(result.parts.tokens, "");
    assert_eq!(result.parts.prelude, LAYER_ORDER);
    assert_eq!(result.stylesheet.matches(LAYER_ORDER).count(), 1);

    for (candidate, value) in DISPLAY {
        assert_eq!(
            declaration(&result, candidate),
            vec![("display".into(), (*value).into())],
            "{candidate}"
        );
    }
    for (candidate, value) in APPEARANCE {
        assert_eq!(
            declaration(&result, candidate),
            vec![("appearance".into(), (*value).into())],
            "{candidate}"
        );
    }

    let catalog = zudo_wind::Catalog::v1();
    for (candidate, _) in DISPLAY.iter().chain(APPEARANCE) {
        let entry = catalog
            .entries()
            .iter()
            .find(|entry| entry.id == format!("v1.{candidate}"))
            .unwrap_or_else(|| panic!("missing catalog entry {candidate}"));
        assert_eq!(entry.root, *candidate);
        assert_eq!(entry.grammar.accepted_kinds, vec![ValueKind::Exact]);
        assert!(entry.grammar.keywords.is_empty());
        assert!(entry.grammar.token_categories.is_empty());
        assert_eq!(entry.grammar.arbitrary_property, None);
        assert!(!entry.grammar.allows_fraction_slash);
        assert!(!entry.negative);
        assert_eq!(entry.selector_shape, zudo_wind::SelectorShape::OwnElement);
    }
}

#[test]
fn exact_static_order_and_conflicts_ignore_input_arrival_and_duplicates() {
    let left = run(
        &[
            "contents",
            "block",
            "table-row-group",
            "table-row",
            "appearance-none",
            "appearance-auto",
            "contents",
        ],
        WindConfig::default(),
        source("block", SourcePositionKind::Class),
    );
    let right = run(
        &[
            "appearance-auto",
            "table-row",
            "contents",
            "appearance-none",
            "block",
            "table-row-group",
        ],
        WindConfig::default(),
        source("block", SourcePositionKind::Class),
    );
    assert!(left.diagnostics.is_empty(), "{:?}", left.diagnostics);
    assert_eq!(left.stylesheet, right.stylesheet);
    assert_eq!(
        left.rules
            .iter()
            .map(|rule| rule.candidate.as_str())
            .collect::<Vec<_>>(),
        vec![
            "block",
            "contents",
            "table-row",
            "table-row-group",
            "appearance-auto",
            "appearance-none"
        ]
    );
    for pair in [
        ["block", "contents"],
        ["table-row", "table-row-group"],
        ["appearance-auto", "appearance-none"],
    ] {
        let first = left
            .rules
            .iter()
            .position(|rule| rule.candidate == pair[0])
            .unwrap();
        let second = left
            .rules
            .iter()
            .position(|rule| rule.candidate == pair[1])
            .unwrap();
        assert!(first < second, "{pair:?}");
    }
}

#[test]
fn new_entries_keep_existing_variants_and_do_not_add_pseudo_content() {
    let mut config = WindConfig::default();
    config
        .breakpoints
        .insert("sm".into(), BreakpointConfig { min_width_px: 640 });
    let result = run(
        &["hover:contents", "sm:appearance-none", "before:contents"],
        config,
        source("hover:contents", SourcePositionKind::Class),
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        declaration(&result, "hover:contents"),
        vec![("display".into(), "contents".into())]
    );
    assert_eq!(
        declaration(&result, "sm:appearance-none"),
        vec![("appearance".into(), "none".into())]
    );
    assert_eq!(
        declaration(&result, "before:contents"),
        vec![("display".into(), "contents".into())]
    );
    assert!(!result.stylesheet.contains("content:"));
    assert!(result.rules.iter().any(|rule| {
        rule.candidate == "hover:contents"
            && rule.selector.as_deref() == Some(".hover\\:contents:hover")
    }));
    assert!(result.rules.iter().any(|rule| {
        rule.candidate == "sm:appearance-none"
            && rule
                .conditions
                .iter()
                .any(|condition| condition.contains("640px"))
    }));
}

#[test]
fn valid_project_tokens_do_not_change_the_new_static_values() {
    let names: Vec<_> = DISPLAY
        .iter()
        .chain(APPEARANCE)
        .map(|(candidate, _)| *candidate)
        .collect();
    let empty = run(
        &names,
        WindConfig::default(),
        source("contents", SourcePositionKind::Class),
    );
    let mut configured = WindConfig::default();
    configured.tokens.colors = ["first", "last", "content", "min", "max", "fit"]
        .into_iter()
        .map(|name| (name.into(), "#123456".into()))
        .collect();
    configured.tokens.spacing = BTreeMap::from([
        ("first".into(), "17px".into()),
        ("last".into(), "19px".into()),
    ]);
    configured.tokens.sizes = BTreeMap::from([("content".into(), "23px".into())]);
    let tokenized = run(
        &names,
        configured,
        source("contents", SourcePositionKind::Class),
    );
    assert!(
        tokenized.diagnostics.is_empty(),
        "{:?}",
        tokenized.diagnostics
    );
    for (candidate, value) in DISPLAY {
        assert_eq!(
            declaration(&tokenized, candidate),
            declaration(&empty, candidate)
        );
        assert_eq!(
            declaration(&tokenized, candidate),
            vec![("display".into(), (*value).into())]
        );
    }
    for (candidate, value) in APPEARANCE {
        assert_eq!(
            declaration(&tokenized, candidate),
            declaration(&empty, candidate)
        );
        assert_eq!(
            declaration(&tokenized, candidate),
            vec![("appearance".into(), (*value).into())]
        );
    }
}

#[test]
fn adopted_modifier_rejections_keep_exact_codes_origins_and_alternatives() {
    for (candidate, rejection, supported) in [
        ("-contents", "R12", "contents"),
        ("-table-cell", "R12", "table-cell"),
        ("-appearance-none", "R12", "appearance-none"),
        ("contents/50", "R14", "contents"),
        ("table-row/2", "R14", "table-row"),
        ("appearance-auto/50", "R14", "appearance-auto"),
    ] {
        for (origin, severity) in [
            (
                source(candidate, SourcePositionKind::Class),
                Severity::Error,
            ),
            (
                source(candidate, SourcePositionKind::Literal),
                Severity::AuditInfo,
            ),
            (
                Origin::Safelist {
                    owner: "test".into(),
                    index: 0,
                },
                Severity::Error,
            ),
            (
                Origin::Manifest {
                    producer: "test".into(),
                    path: "classes.json".into(),
                    index: 0,
                },
                Severity::Error,
            ),
        ] {
            for strict in [false, true] {
                let config = WindConfig {
                    strict,
                    ..WindConfig::default()
                };
                let result = run(&[candidate], config, origin.clone());
                assert_eq!(result.diagnostics.len(), 1, "{candidate} {origin:?}");
                let diagnostic = &result.diagnostics[0];
                assert_eq!(diagnostic.code, DiagnosticCode::Zw005, "{candidate}");
                assert_eq!(diagnostic.rejection_id, Some(rejection), "{candidate}");
                assert_eq!(diagnostic.candidate.as_deref(), Some(candidate));
                assert_eq!(diagnostic.severity, severity, "{candidate} {origin:?}");
                assert_eq!(diagnostic.suggested_spelling.as_deref(), Some(supported));
                assert!(
                    diagnostic.message.contains(candidate),
                    "{}",
                    diagnostic.message
                );
                assert!(
                    diagnostic.message.contains(supported),
                    "{}",
                    diagnostic.message
                );
                assert!(diagnostic.message.contains("author the declaration in CSS"));
                assert!(diagnostic.message.contains("wind.authoredClasses"));
                assert!(result.rules.is_empty());
            }
        }

        let role = run(
            &[candidate],
            WindConfig::default(),
            Origin::RoleClass {
                role_key: "className".into(),
            },
        );
        assert!(role.diagnostics.is_empty(), "{candidate}");
        assert!(role.rules.is_empty(), "{candidate}");
        assert!(role
            .ordinary_classes
            .iter()
            .any(|item| item.text == candidate));
    }
}

#[test]
fn exact_static_roots_do_not_claim_suffixes_or_underscore_names() {
    let ordinary = [
        "table-widget",
        "contents-panel",
        "appearance-button",
        "fill-logo",
        "stroke-icon",
        "not-sr-only-label",
        "order-summary",
        "order-1.5",
        "contents_panel",
        "table_widget",
        "appearance_none",
        "order_summary",
        "fill_logo",
    ];
    for candidate in ordinary {
        for strict in [false, true] {
            let config = WindConfig {
                strict,
                ..WindConfig::default()
            };
            let result = run(
                &[candidate],
                config,
                source(candidate, SourcePositionKind::Class),
            );
            assert!(
                result.diagnostics.is_empty(),
                "{candidate}: {:?}",
                result.diagnostics
            );
            assert!(result.rules.is_empty(), "{candidate}");
            assert!(
                result
                    .ordinary_classes
                    .iter()
                    .any(|item| item.text == candidate),
                "{candidate}"
            );
        }
        for origin in [
            Origin::Safelist {
                owner: "test".into(),
                index: 0,
            },
            Origin::Manifest {
                producer: "test".into(),
                path: "classes.json".into(),
                index: 0,
            },
        ] {
            let result = run(&[candidate], WindConfig::default(), origin);
            assert_eq!(result.diagnostics.len(), 1, "{candidate}");
            assert_eq!(
                result.diagnostics[0].code,
                DiagnosticCode::Zw008,
                "{candidate}"
            );
        }
    }
}

#[test]
fn retained_table_layout_and_deferred_arbitrary_forms_keep_their_old_dispositions() {
    for candidate in ["table-auto", "table-fixed"] {
        let default = run(
            &[candidate],
            WindConfig::default(),
            source(candidate, SourcePositionKind::Class),
        );
        assert_eq!(default.diagnostics.len(), 1, "{candidate}");
        assert_eq!(
            default.diagnostics[0].code,
            DiagnosticCode::Zw014,
            "{candidate}"
        );
        assert_eq!(
            default.diagnostics[0].severity,
            Severity::Warning,
            "{candidate}"
        );

        let strict = WindConfig {
            strict: true,
            ..WindConfig::default()
        };
        let strict_result = run(
            &[candidate],
            strict,
            source(candidate, SourcePositionKind::Class),
        );
        assert_eq!(
            strict_result.diagnostics[0].severity,
            Severity::Error,
            "{candidate}"
        );
        let literal = run(
            &[candidate],
            WindConfig::default(),
            source(candidate, SourcePositionKind::Literal),
        );
        assert_eq!(
            literal.diagnostics[0].severity,
            Severity::AuditInfo,
            "{candidate}"
        );
        for origin in [
            Origin::Safelist {
                owner: "test".into(),
                index: 0,
            },
            Origin::Manifest {
                producer: "test".into(),
                path: "classes.json".into(),
                index: 0,
            },
        ] {
            let explicit = run(&[candidate], WindConfig::default(), origin);
            assert_eq!(
                explicit.diagnostics[0].code,
                DiagnosticCode::Zw014,
                "{candidate}"
            );
            assert_eq!(
                explicit.diagnostics[0].severity,
                Severity::Error,
                "{candidate}"
            );
        }
        let role = run(
            &[candidate],
            WindConfig::default(),
            Origin::RoleClass {
                role_key: "className".into(),
            },
        );
        assert!(role.diagnostics.is_empty(), "{candidate}");
        assert!(role.rules.is_empty(), "{candidate}");
    }

    for candidate in ["appearance-[none]", "display-[contents]"] {
        let source_result = run(
            &[candidate],
            WindConfig::default(),
            source(candidate, SourcePositionKind::Class),
        );
        assert!(source_result.diagnostics.is_empty(), "{candidate}");
        assert!(source_result
            .ordinary_classes
            .iter()
            .any(|item| item.text == candidate));
        let strict = run(
            &[candidate],
            WindConfig::default(),
            Origin::Manifest {
                producer: "test".into(),
                path: "classes.json".into(),
                index: 0,
            },
        );
        assert_eq!(
            strict.diagnostics[0].code,
            DiagnosticCode::Zw008,
            "{candidate}"
        );
    }
}

#[test]
fn authored_reservations_suppress_only_the_complete_candidate_at_every_origin() {
    for (reserved, other) in [("table", "hover:table"), ("hover:table", "table")] {
        for origin in [
            source(reserved, SourcePositionKind::Class),
            source(reserved, SourcePositionKind::Literal),
            Origin::Safelist {
                owner: "test".into(),
                index: 0,
            },
            Origin::Manifest {
                producer: "test".into(),
                path: "classes.json".into(),
                index: 0,
            },
            Origin::RoleClass {
                role_key: "className".into(),
            },
        ] {
            let mut config = WindConfig::default();
            config.authored_classes.insert(reserved.into(), true);
            let result = compile(&CompileInput {
                candidates: vec![
                    origin_candidate(reserved, origin.clone()),
                    origin_candidate(other, source(other, SourcePositionKind::Class)),
                ],
                config,
            });
            assert!(result.diagnostics.is_empty(), "{reserved} {origin:?}");
            assert!(result
                .authored_classes
                .iter()
                .any(|item| item.text == reserved));
            assert!(result.rules.iter().any(|rule| rule.candidate == other));
            assert!(!result.rules.iter().any(|rule| rule.candidate == reserved));
        }
    }
}

#[test]
fn html_tsx_and_mdx_keep_the_existing_static_class_extraction_paths() {
    let fixtures: [(&str, SourceKind, &[&str]); 3] = [
        (
            r#"<div class="contents table-cell"></div>"#,
            SourceKind::Html,
            &["contents", "table-cell"],
        ),
        (
            r#"const view = <div className="appearance-none flow-root" />;"#,
            SourceKind::Tsx,
            &["appearance-none", "flow-root"],
        ),
        (
            "<Card className=\"list-item table-row\" />",
            SourceKind::Mdx,
            &["list-item", "table-row"],
        ),
    ];
    for (source_text, kind, expected) in fixtures {
        let extracted = extract_candidates(source_text.as_bytes(), kind);
        let names: Vec<_> = extracted
            .candidates
            .iter()
            .map(|item| item.text.as_str())
            .collect();
        assert_eq!(names.len(), expected.len(), "{kind:?}: {names:?}");
        for candidate in expected {
            assert!(names.contains(candidate), "{kind:?}: {names:?}");
        }
    }
}

#[test]
fn valid_entries_resolve_for_source_explicit_and_role_origins() {
    for origin in [
        source("contents", SourcePositionKind::Class),
        Origin::Safelist {
            owner: "test".into(),
            index: 0,
        },
        Origin::Manifest {
            producer: "test".into(),
            path: "classes.json".into(),
            index: 0,
        },
        Origin::RoleClass {
            role_key: "className".into(),
        },
    ] {
        let result = run(&["contents"], WindConfig::default(), origin.clone());
        assert!(result.diagnostics.is_empty(), "{origin:?}");
        assert_eq!(
            declaration(&result, "contents"),
            vec![("display".into(), "contents".into())]
        );
    }
    assert_eq!(WindConfig::default().reset, ResetMode::None);
}
