mod common;

use lightningcss::stylesheet::{ParserOptions, StyleSheet};
use zudo_wind::{
    compile, compile_validated, CompileInput, Origin, OriginCandidate, ProvenanceKind, ResetMode,
    RuleKind, Severity, SourcePositionKind, WindConfig,
};

#[test]
fn twelve_arrival_orders_preserve_all_bytes_metadata_and_diagnostics() {
    let mut input = common::input(&[
        "p-4",
        "px-2",
        "pl-1",
        "dark:hover:bg-panel",
        "sm:group-hover:block",
        "max-sm:peer-focus:space-y-2",
        "group",
        "peer",
        "p-missing",
        "hover:dark:block",
        "translate-y-2",
        "2xl:w-[24px]",
    ]);
    input.candidates.push(OriginCandidate {
        text: "p-4".to_owned(),
        origin: Origin::Manifest {
            producer: "package".to_owned(),
            path: "manifest.json".to_owned(),
            index: 3,
        },
    });
    input.candidates.push(input.candidates[0].clone());
    let expected = compile(&input);
    assert!(expected.has_errors());
    assert_eq!(expected.diagnostics.len(), 2);
    assert_eq!(
        expected
            .rules
            .iter()
            .find(|rule| rule.candidate == "p-4")
            .unwrap()
            .origins
            .len(),
        2
    );
    for order in 0..12 {
        let mut permuted = input.clone();
        permuted.candidates.rotate_left(order);
        if order % 2 == 1 {
            permuted.candidates.reverse();
        }
        assert_eq!(compile(&permuted), expected, "arrival order {order}");
    }
    StyleSheet::parse(&expected.stylesheet, ParserOptions::default()).unwrap();
}

#[test]
fn empty_and_marker_only_results_emit_zero_bytes_without_provenance() {
    let empty = compile(&CompileInput::default());
    assert_eq!(empty.stylesheet, "");
    assert_eq!(empty.provenance, None);
    let mut input = common::input(&["peer", "group"]);
    input.config = WindConfig::default();
    let markers = compile(&input);
    assert_eq!(markers.stylesheet, "");
    assert_eq!(markers.provenance, None);
    assert_eq!(markers.rules.len(), 2);
    assert!(markers
        .rules
        .iter()
        .all(|rule| rule.kind == RuleKind::Marker
            && rule.selector.is_none()
            && rule.resolved.is_none()
            && rule.sort_key.is_none()));
}

#[test]
fn diagnostics_retain_explicit_origins_and_valid_peers() {
    let mut input = common::input(&["p-[", "block"]);
    input.candidates[0].origin = Origin::Manifest {
        producer: "pkg".to_owned(),
        path: "dist/wind.json".to_owned(),
        index: 9,
    };
    input
        .config
        .safelist
        .insert("app".to_owned(), vec!["p-[".to_owned(), "flex".to_owned()]);
    let result = compile(&input);
    assert_eq!(result.diagnostics.len(), 2);
    assert!(result
        .diagnostics
        .iter()
        .all(|diagnostic| diagnostic.candidate.as_deref() == Some("p-[")
            && diagnostic.severity == Severity::Error));
    assert_eq!(
        result.diagnostics[0].origin.as_deref(),
        Some(&Origin::Safelist {
            owner: "app".to_owned(),
            index: 0
        })
    );
    assert_eq!(
        result.diagnostics[1].origin.as_deref(),
        Some(&input.candidates[0].origin)
    );
    assert_eq!(result.rules.len(), 2);
    assert!(result
        .rules
        .iter()
        .all(|rule| ["block", "flex"].contains(&rule.candidate.as_str())));
}

fn source(kind: SourcePositionKind, offset: usize) -> Origin {
    Origin::Source {
        source_id: "page.tsx".to_owned(),
        byte_offset: offset,
        byte_length: 5,
        line: 1,
        byte_column: offset + 1,
        literal_byte_offset: 0,
        literal_byte_length: 100,
        position_kind: kind,
    }
}

#[test]
fn origin_policy_and_allowlist_cover_parser_and_resolver_failures() {
    let mut input = common::input(&[]);
    for text in ["hover:dark:block", "p-missing", "ordinary-component"] {
        for origin in [
            source(SourcePositionKind::Class, 10),
            source(SourcePositionKind::Literal, 20),
            Origin::RoleClass {
                role_key: "keyword".to_owned(),
            },
            Origin::Safelist {
                owner: "explicit".to_owned(),
                index: 10,
            },
        ] {
            input.candidates.push(OriginCandidate {
                text: text.to_owned(),
                origin,
            });
        }
    }
    let result = compile(&input);
    assert_eq!(result.diagnostics.len(), 7);
    assert_eq!(
        result
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::AuditInfo)
            .count(),
        2
    );
    assert_eq!(result.ordinary_classes.len(), 5);
    assert!(result.rules.is_empty());
    for text in ["hover:dark:block", "p-missing", "ordinary-component"] {
        input.config.authored_classes.insert(text.to_owned(), true);
    }
    input
        .config
        .safelist
        .insert("authored".to_owned(), vec!["hover:dark:block".to_owned()]);
    let result = compile(&input);
    assert!(result.diagnostics.is_empty());
    assert!(result.rules.is_empty());
    assert_eq!(result.authored_classes.len(), 13);
}

#[test]
fn diagnostic_positions_sort_numerically_and_duplicates_collapse() {
    let mut input = common::input(&[]);
    for offset in [20, 3, 20, 100] {
        input.candidates.push(OriginCandidate {
            text: "p-missing".to_owned(),
            origin: source(SourcePositionKind::Class, offset),
        });
    }
    let result = compile(&input);
    let offsets: Vec<_> = result
        .diagnostics
        .iter()
        .map(|diagnostic| match diagnostic.origin.as_deref().unwrap() {
            Origin::Source { byte_offset, .. } => *byte_offset,
            _ => panic!(),
        })
        .collect();
    assert_eq!(offsets, [3, 20, 100]);
}

#[test]
fn validated_configuration_entry_point_includes_safelist_and_matches_compile() {
    let mut input = common::input(&["block"]);
    input
        .config
        .safelist
        .insert("app".to_owned(), vec!["p-4".to_owned(), "group".to_owned()]);
    let expected = compile(&input);
    assert_eq!(
        compile_validated(&input.candidates, &input.config.validate().unwrap()),
        expected
    );
}

#[test]
fn generated_provenance_is_separate_and_only_present_for_nonempty_css() {
    let input = CompileInput {
        config: WindConfig {
            reset: ResetMode::MinimalV1,
            ..WindConfig::default()
        },
        ..CompileInput::default()
    };
    let result = compile(&input);
    assert!(result.rules.is_empty());
    let provenance = result.provenance.unwrap();
    assert_eq!(provenance.source_id, "zudo-wind://spec/1");
    assert_eq!(provenance.kind, ProvenanceKind::Generated);
    assert_eq!(
        (
            provenance.spec_version,
            provenance.spec_revision,
            provenance.map
        ),
        (1, 3, None)
    );
    assert!(!result.stylesheet.contains("zudo-wind://"));
}

#[test]
fn invalid_configuration_still_returns_structured_diagnostics() {
    let mut input = common::input(&["block"]);
    input.config.spec = 99;
    let result = compile(&input);
    assert!(result.has_errors());
    assert!(result.stylesheet.is_empty());
    assert!(result.provenance.is_none());
    assert_eq!(result.diagnostics.len(), 1);
}
