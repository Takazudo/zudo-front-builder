use std::collections::BTreeMap;

use zudo_wind::{
    compile, BreakpointConfig, CompileInput, DiagnosticCode, Origin, OriginCandidate, Severity,
    SourcePositionKind, WindConfig,
};

fn origin(text: &str, kind: SourcePositionKind) -> Origin {
    Origin::Source {
        source_id: "native-flex-svg.tsx".into(),
        byte_offset: 0,
        byte_length: text.len(),
        line: 1,
        byte_column: 1,
        literal_byte_offset: 0,
        literal_byte_length: text.len(),
        position_kind: kind,
    }
}

fn run(names: &[&str], config: WindConfig, origin: Origin) -> zudo_wind::CompileResult {
    compile(&CompileInput {
        candidates: names
            .iter()
            .map(|name| OriginCandidate {
                text: (*name).into(),
                origin: origin.clone(),
            })
            .collect(),
        config,
    })
}

fn writes(result: &zudo_wind::CompileResult, name: &str) -> Vec<(String, String)> {
    result
        .rules
        .iter()
        .find(|rule| rule.candidate == name)
        .unwrap()
        .resolved
        .as_ref()
        .unwrap()
        .declarations
        .iter()
        .map(|d| (d.property.clone(), d.value.clone()))
        .collect()
}

#[test]
fn bounded_order_basis_and_svg_emit_exact_values_and_metadata() {
    let values = [
        ("order-first", "order", "-9999"),
        ("order-last", "order", "9999"),
        ("order-none", "order", "0"),
        ("order-0", "order", "0"),
        ("order-01", "order", "1"),
        ("-order-01", "order", "-1"),
        ("-order-0", "order", "0"),
        ("order-2147483647", "order", "2147483647"),
        ("basis-auto", "flex-basis", "auto"),
        ("basis-full", "flex-basis", "100%"),
        ("basis-px", "flex-basis", "1px"),
        ("basis-0", "flex-basis", "0"),
        ("basis-min", "flex-basis", "min-content"),
        ("basis-max", "flex-basis", "max-content"),
        ("basis-fit", "flex-basis", "fit-content"),
        ("basis-content", "flex-basis", "content"),
        ("basis-1/2", "flex-basis", "calc(100% * 1 / 2)"),
        ("basis-01/02", "flex-basis", "calc(100% * 1 / 2)"),
        ("basis-3/2", "flex-basis", "calc(100% * 3 / 2)"),
        ("basis-1/1000000", "flex-basis", "calc(100% * 1 / 1000000)"),
        ("basis-1000000/1", "flex-basis", "calc(100% * 1000000 / 1)"),
        ("fill-current", "fill", "currentColor"),
        ("fill-none", "fill", "none"),
        ("stroke-current", "stroke", "currentColor"),
        ("stroke-none", "stroke", "none"),
    ];
    let names = values.iter().map(|(name, _, _)| *name).collect::<Vec<_>>();
    let result = run(
        &names,
        WindConfig::default(),
        origin("order-0", SourcePositionKind::Class),
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    for (name, property, value) in values {
        assert_eq!(
            writes(&result, name),
            vec![(property.into(), value.into())],
            "{name}"
        );
        let rule = result
            .rules
            .iter()
            .find(|rule| rule.candidate == name)
            .unwrap();
        let resolved = rule.resolved.as_ref().unwrap();
        let (id, group, rank, order) = if name.starts_with("order-") || name.starts_with("-order-")
        {
            ("v1.order", "flex-item", 5, 2)
        } else if name.starts_with("basis-") {
            ("v1.basis", "flex-item", 5, 1)
        } else {
            (
                if name.starts_with("fill-") {
                    if name == "fill-current" {
                        "v1.fill-current"
                    } else {
                        "v1.fill-none"
                    }
                } else if name == "stroke-current" {
                    "v1.stroke-current"
                } else {
                    "v1.stroke-none"
                },
                "svg",
                48,
                if name.starts_with("fill-") { 0 } else { 1 },
            )
        };
        assert_eq!(resolved.entry_id, id, "{name}");
        assert_eq!(
            (
                resolved.conflict_group,
                resolved.conflict_group_rank,
                resolved.order_rank
            ),
            (group, rank, order),
            "{name}"
        );
    }
}

#[test]
fn reversal_is_exact_and_sorted_after_sr_only() {
    let names = ["not-sr-only", "sr-only", "focus:not-sr-only"];
    let left = run(
        &names,
        WindConfig::default(),
        origin("sr-only", SourcePositionKind::Class),
    );
    let right = run(
        &["focus:not-sr-only", "sr-only", "not-sr-only", "sr-only"],
        WindConfig::default(),
        origin("sr-only", SourcePositionKind::Class),
    );
    assert_eq!(left.stylesheet, right.stylesheet);
    assert_eq!(
        writes(&left, "not-sr-only"),
        [
            ("position", "static"),
            ("width", "auto"),
            ("height", "auto"),
            ("padding", "0"),
            ("margin", "0"),
            ("overflow", "visible"),
            ("clip", "auto"),
            ("clip-path", "none"),
            ("white-space", "normal"),
        ]
        .into_iter()
        .map(|(p, v)| (p.into(), v.into()))
        .collect::<Vec<_>>()
    );
    assert!(
        left.rules
            .iter()
            .position(|r| r.candidate == "sr-only")
            .unwrap()
            < left
                .rules
                .iter()
                .position(|r| r.candidate == "not-sr-only")
                .unwrap()
    );
    assert_eq!(
        writes(&left, "focus:not-sr-only"),
        writes(&left, "not-sr-only")
    );
}

#[test]
fn invalid_adopted_shapes_and_retained_gaps_keep_distinct_diagnostics() {
    for (name, rejection) in [
        ("-order-first", "R12"),
        ("-order-last", "R12"),
        ("-order-none", "R12"),
        ("order-1/2", "R14"),
        ("order-2147483648", "R15"),
        ("basis-0/2", "R13"),
        ("basis-1/0", "R13"),
        ("basis-0.5/2", "R13"),
        ("basis-1000001/2", "R13"),
        ("basis-auto/2", "R14"),
        ("-basis-auto", "R12"),
        ("-basis-1/2", "R12"),
        ("-fill-current", "R12"),
        ("fill-current/50", "R14"),
        ("-stroke-none", "R12"),
        ("stroke-none/2", "R14"),
        ("-not-sr-only", "R12"),
        ("not-sr-only/2", "R14"),
    ] {
        let result = run(
            &[name],
            WindConfig::default(),
            origin(name, SourcePositionKind::Class),
        );
        assert_eq!(
            result.diagnostics.len(),
            1,
            "{name}: {:?}",
            result.diagnostics
        );
        let d = &result.diagnostics[0];
        assert_eq!(
            (d.code, d.rejection_id),
            (DiagnosticCode::Zw005, Some(rejection)),
            "{name}"
        );
        assert!(result.rules.is_empty(), "{name}");
    }
    for name in [
        "order-[2]",
        "order-[var(--priority)]",
        "basis-1",
        "basis-1.5",
        "basis-hsp-sm",
        "basis-[20px]",
        "fill-[red]",
        "stroke-[2px]",
        "stroke-0",
        "stroke-1",
    ] {
        let result = run(
            &[name],
            WindConfig::default(),
            origin(name, SourcePositionKind::Class),
        );
        assert_eq!(result.diagnostics[0].code, DiagnosticCode::Zw014, "{name}");
        assert_eq!(result.diagnostics[0].severity, Severity::Warning, "{name}");
        assert!(result.rules.is_empty(), "{name}");
    }
    for name in [
        "fill-brand",
        "stroke-brand",
        "fill-transparent",
        "stroke-inherit",
        "order-summary",
        "order-1.5",
        "order-1e2",
        "not-sr-only-label",
        "order_summary",
        "fill_logo",
    ] {
        let result = run(
            &[name],
            WindConfig::default(),
            origin(name, SourcePositionKind::Class),
        );
        assert!(
            result.diagnostics.is_empty(),
            "{name}: {:?}",
            result.diagnostics
        );
        assert!(result.rules.is_empty(), "{name}");
    }
}

#[test]
fn native_forms_ignore_configured_names_and_authored_complete_candidates() {
    let mut config = WindConfig::default();
    config.tokens.colors = BTreeMap::from([("brand".into(), "green".into())]);
    config.tokens.spacing = BTreeMap::from([
        ("first".into(), "17px".into()),
        ("content".into(), "19px".into()),
    ]);
    let result = run(
        &[
            "order-first",
            "basis-content",
            "basis-min",
            "fill-current",
            "stroke-none",
        ],
        config,
        origin("order-first", SourcePositionKind::Class),
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        writes(&result, "basis-content"),
        vec![("flex-basis".into(), "content".into())]
    );
    assert_eq!(
        writes(&result, "fill-current"),
        vec![("fill".into(), "currentColor".into())]
    );
    let mut reserved = WindConfig::default();
    reserved.authored_classes = BTreeMap::from([
        ("order-first".into(), true),
        ("hover:fill-current".into(), true),
    ]);
    let result = run(
        &["order-first", "hover:fill-current", "fill-current"],
        reserved,
        origin("order-first", SourcePositionKind::Class),
    );
    assert_eq!(result.rules.len(), 1);
    assert_eq!(result.rules[0].candidate, "fill-current");
}

#[test]
fn adopted_forms_use_existing_variants_and_stable_raw_tie_break() {
    let mut config = WindConfig::default();
    config
        .breakpoints
        .insert("sm".into(), BreakpointConfig { min_width_px: 640 });
    let left = run(
        &[
            "order-none",
            "order-last",
            "order-2",
            "order-10",
            "order-1",
            "-order-1",
            "basis-auto",
            "flex-1",
            "fill-none",
            "fill-current",
            "stroke-none",
            "stroke-current",
            "focus:order-1",
            "sm:basis-1/2",
            "hover:fill-current",
            "focus:not-sr-only",
        ],
        config.clone(),
        origin("order-1", SourcePositionKind::Class),
    );
    let right = run(
        &[
            "focus:not-sr-only",
            "hover:fill-current",
            "sm:basis-1/2",
            "focus:order-1",
            "-order-1",
            "order-1",
            "order-10",
            "order-2",
            "order-last",
            "order-none",
            "stroke-current",
            "stroke-none",
            "fill-current",
            "fill-none",
            "flex-1",
            "basis-auto",
            "order-1",
        ],
        config,
        origin("order-1", SourcePositionKind::Class),
    );
    assert!(left.diagnostics.is_empty(), "{:?}", left.diagnostics);
    assert_eq!(left.stylesheet, right.stylesheet);
    let ordered = left
        .rules
        .iter()
        .map(|rule| rule.candidate.as_str())
        .collect::<Vec<_>>();
    for (before, after) in [
        ("flex-1", "basis-auto"),
        ("basis-auto", "-order-1"),
        ("-order-1", "order-1"),
        ("order-10", "order-2"),
        ("order-last", "order-none"),
        ("fill-current", "fill-none"),
        ("stroke-current", "stroke-none"),
    ] {
        assert!(
            ordered.iter().position(|x| x == &before).unwrap()
                < ordered.iter().position(|x| x == &after).unwrap(),
            "{before} before {after}"
        );
    }
    assert_eq!(
        writes(&left, "focus:order-1"),
        vec![("order".into(), "1".into())]
    );
    assert_eq!(
        writes(&left, "sm:basis-1/2"),
        vec![("flex-basis".into(), "calc(100% * 1 / 2)".into())]
    );
    assert!(left.stylesheet.contains("640px"));
    assert!(!left.stylesheet.contains("content:"));
}

#[test]
fn invalid_and_migration_forms_preserve_origin_and_strictness_policy() {
    for (name, code) in [
        ("basis-0/2", DiagnosticCode::Zw005),
        ("basis-1", DiagnosticCode::Zw014),
    ] {
        for strict in [false, true] {
            let config = WindConfig {
                strict,
                ..WindConfig::default()
            };
            for (source_kind, severity) in [
                (
                    SourcePositionKind::Class,
                    if code == DiagnosticCode::Zw014 && !strict {
                        Severity::Warning
                    } else {
                        Severity::Error
                    },
                ),
                (SourcePositionKind::Literal, Severity::AuditInfo),
            ] {
                let result = run(&[name], config.clone(), origin(name, source_kind));
                assert_eq!(result.diagnostics.len(), 1, "{name}");
                assert_eq!(
                    (result.diagnostics[0].code, result.diagnostics[0].severity),
                    (code, severity),
                    "{name}"
                );
                assert!(result.diagnostics[0].message.contains(name), "{name}");
                assert!(result.rules.is_empty());
            }
            for explicit in [
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
                let result = run(&[name], config.clone(), explicit);
                assert_eq!(
                    (result.diagnostics[0].code, result.diagnostics[0].severity),
                    (code, Severity::Error)
                );
            }
            let role = run(
                &[name],
                config,
                Origin::RoleClass {
                    role_key: "className".into(),
                },
            );
            assert!(role.diagnostics.is_empty(), "{name}");
            assert!(role.rules.is_empty(), "{name}");
        }
    }
}
