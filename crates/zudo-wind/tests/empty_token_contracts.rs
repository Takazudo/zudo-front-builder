//! #3830: the finite preset-free contract. Expected writes below are authored
//! here, independently of Catalog examples and the compatibility runner.
use std::collections::BTreeMap;

use zudo_wind::{
    compile, CompileInput, CompileResult, DiagnosticCode, Origin, OriginCandidate, ResetMode,
    Severity, SourcePositionKind, WindConfig, LAYER_ORDER,
};

fn source(kind: SourcePositionKind) -> Origin {
    Origin::Source {
        source_id: "empty-token-contract.tsx".into(),
        byte_offset: 7,
        byte_length: 12,
        line: 1,
        byte_column: 8,
        literal_byte_offset: 0,
        literal_byte_length: 40,
        position_kind: kind,
    }
}

fn run(names: &[&str], config: WindConfig, origin: Origin) -> CompileResult {
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

fn writes(result: &CompileResult, name: &str) -> Vec<(String, String)> {
    let rule = result
        .rules
        .iter()
        .find(|rule| rule.candidate == name)
        .unwrap_or_else(|| panic!("missing generated rule for {name}"));
    rule.resolved
        .as_ref()
        .unwrap_or_else(|| panic!("marker instead of utility for {name}"))
        .declarations
        .iter()
        .map(|declaration| (declaration.property.clone(), declaration.value.clone()))
        .collect()
}

fn expect_writes(result: &CompileResult, name: &str, expected: &[(&str, &str)]) {
    assert_eq!(
        writes(result, name),
        expected
            .iter()
            .map(|(property, value)| ((*property).into(), (*value).into()))
            .collect::<Vec<(String, String)>>(),
        "{name}"
    );
}

fn assert_one_prelude(result: &CompileResult) {
    assert_eq!(result.parts.prelude, LAYER_ORDER);
    assert!(result.stylesheet.starts_with(LAYER_ORDER));
    assert_eq!(result.stylesheet.matches(LAYER_ORDER).count(), 1);
}

#[test]
fn native_guarantees_generate_with_exactly_empty_token_maps() {
    let config = WindConfig::default();
    assert_eq!(config.reset, ResetMode::None);
    assert_eq!(config.tokens, Default::default());
    assert!(config.breakpoints.is_empty());
    let names = [
        "block",
        "hidden",
        "inline-flex",
        "mx-auto",
        "grid-cols-2",
        "p-0",
        "relative",
        "absolute",
        "flex-row",
        "flex-wrap",
        "flex-1",
        "w-min",
        "h-max",
        "size-px",
        "inset-0",
        "top-px",
        "m-0",
        "p-px",
    ];
    let result = run(&names, config, source(SourcePositionKind::Class));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.rules.len(), names.len());
    assert!(result.parts.reset.is_empty());
    assert!(result.parts.tokens.is_empty());
    assert_one_prelude(&result);
    for (name, expected) in [
        ("block", vec![("display", "block")]),
        ("hidden", vec![("display", "none")]),
        ("inline-flex", vec![("display", "inline-flex")]),
        (
            "mx-auto",
            vec![("margin-left", "auto"), ("margin-right", "auto")],
        ),
        (
            "grid-cols-2",
            vec![("grid-template-columns", "repeat(2,minmax(0,1fr))")],
        ),
        (
            "p-0",
            vec![
                ("padding-top", "0"),
                ("padding-right", "0"),
                ("padding-bottom", "0"),
                ("padding-left", "0"),
            ],
        ),
        ("relative", vec![("position", "relative")]),
        ("absolute", vec![("position", "absolute")]),
        ("flex-row", vec![("flex-direction", "row")]),
        ("flex-wrap", vec![("flex-wrap", "wrap")]),
        ("flex-1", vec![("flex", "1 1 0%")]),
        ("w-min", vec![("width", "min-content")]),
        ("h-max", vec![("height", "max-content")]),
        ("size-px", vec![("width", "1px"), ("height", "1px")]),
        (
            "inset-0",
            vec![("top", "0"), ("right", "0"), ("bottom", "0"), ("left", "0")],
        ),
        ("top-px", vec![("top", "1px")]),
        (
            "m-0",
            vec![
                ("margin-top", "0"),
                ("margin-right", "0"),
                ("margin-bottom", "0"),
                ("margin-left", "0"),
            ],
        ),
        (
            "p-px",
            vec![
                ("padding-top", "1px"),
                ("padding-right", "1px"),
                ("padding-bottom", "1px"),
                ("padding-left", "1px"),
            ],
        ),
    ] {
        expect_writes(&result, name, &expected);
    }
    // p-0 is Wind-owned. The pinned empty-theme reference omits it; the
    // separate p-0-mapped reference case declares --spacing-0 explicitly.
}

#[test]
fn semantic_tokens_use_variables_and_each_new_config_replaces_all_uses() {
    for (hsp, vsp, surface) in [("17px", "29px", "#123456"), ("23px", "31px", "#654321")] {
        let mut config = WindConfig::default();
        config.tokens.spacing =
            BTreeMap::from([("hsp-sm".into(), hsp.into()), ("vsp-md".into(), vsp.into())]);
        config.tokens.colors = BTreeMap::from([("surface".into(), surface.into())]);
        let result = run(
            &[
                "p-hsp-sm",
                "mx-hsp-sm",
                "pt-vsp-md",
                "my-vsp-md",
                "bg-surface",
                "text-surface",
            ],
            config,
            source(SourcePositionKind::Class),
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_one_prelude(&result);
        assert_eq!(result.parts.tokens.matches("@layer zw-tokens").count(), 1);
        for (name, value) in [
            ("--zw-spacing-hsp-sm", hsp),
            ("--zw-spacing-vsp-md", vsp),
            ("--zw-color-surface", surface),
        ] {
            assert!(result.parts.tokens.contains(&format!("{name}: {value};")));
            assert_eq!(result.parts.tokens.matches(&format!("{name}:")).count(), 1);
        }
        expect_writes(
            &result,
            "p-hsp-sm",
            &[
                ("padding-top", "var(--zw-spacing-hsp-sm)"),
                ("padding-right", "var(--zw-spacing-hsp-sm)"),
                ("padding-bottom", "var(--zw-spacing-hsp-sm)"),
                ("padding-left", "var(--zw-spacing-hsp-sm)"),
            ],
        );
        expect_writes(
            &result,
            "mx-hsp-sm",
            &[
                ("margin-left", "var(--zw-spacing-hsp-sm)"),
                ("margin-right", "var(--zw-spacing-hsp-sm)"),
            ],
        );
        expect_writes(
            &result,
            "pt-vsp-md",
            &[("padding-top", "var(--zw-spacing-vsp-md)")],
        );
        expect_writes(
            &result,
            "my-vsp-md",
            &[
                ("margin-top", "var(--zw-spacing-vsp-md)"),
                ("margin-bottom", "var(--zw-spacing-vsp-md)"),
            ],
        );
        expect_writes(
            &result,
            "bg-surface",
            &[("background-color", "var(--zw-color-surface)")],
        );
        expect_writes(
            &result,
            "text-surface",
            &[("color", "var(--zw-color-surface)")],
        );
        assert!(result
            .parts
            .tokens
            .contains("@layer zw-tokens {\n  :root {"));
        assert!(!result.parts.tokens.contains("--zw-spacing-unit"));
    }
}

#[test]
fn missing_design_defaults_do_not_emit_css_and_keep_origin_policy() {
    let missing = [
        "p-4",
        "bg-gray-500",
        "text-gray-500",
        "font-sans",
        "rounded-lg",
        "shadow-md",
    ];
    for name in missing {
        let class_origin = source(SourcePositionKind::Class);
        let result = run(&[name], WindConfig::default(), class_origin.clone());
        assert!(result.rules.is_empty(), "{name}");
        assert!(
            result.stylesheet.is_empty(),
            "{name}: {}",
            result.stylesheet
        );
        assert_eq!(result.diagnostics.len(), 1, "{name}");
        let diagnostic = &result.diagnostics[0];
        assert_eq!(diagnostic.code, DiagnosticCode::Zw006, "{name}");
        assert_eq!(diagnostic.severity, Severity::Error, "{name}");
        assert_eq!(diagnostic.rejection_id, Some("R17"), "{name}");
        assert_eq!(diagnostic.origin.as_deref(), Some(&class_origin), "{name}");
        assert_eq!(diagnostic.candidate.as_deref(), Some(name));
        let literal_origin = source(SourcePositionKind::Literal);
        let literal = run(&[name], WindConfig::default(), literal_origin.clone());
        assert!(literal.stylesheet.is_empty(), "{name}");
        assert_eq!(
            literal.diagnostics[0].severity,
            Severity::AuditInfo,
            "{name}"
        );
        assert_eq!(
            literal.diagnostics[0].origin.as_deref(),
            Some(&literal_origin)
        );
        let manifest_origin = Origin::Manifest {
            producer: "fixture".into(),
            path: "wind.json".into(),
            index: 0,
        };
        let manifest = run(&[name], WindConfig::default(), manifest_origin.clone());
        assert_eq!(manifest.diagnostics[0].severity, Severity::Error, "{name}");
        assert_eq!(
            manifest.diagnostics[0].origin.as_deref(),
            Some(&manifest_origin)
        );
    }
    // A low-confidence ordinary class is not an implicit failed utility.
    let ordinary = run(
        &["card__body"],
        WindConfig::default(),
        source(SourcePositionKind::Literal),
    );
    assert!(ordinary.diagnostics.is_empty());
    assert_eq!(ordinary.ordinary_classes.len(), 1);
    assert!(ordinary.stylesheet.is_empty());
}

#[test]
fn numeric_spacing_and_palette_names_are_explicit_opt_ins() {
    let mut config = WindConfig::default();
    config.tokens.spacing_unit = Some("0.25rem".into());
    config
        .tokens
        .colors
        .insert("gray-500".into(), "#37628a".into());
    let result = run(
        &["p-4", "bg-gray-500"],
        config,
        source(SourcePositionKind::Class),
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_one_prelude(&result);
    assert!(result.parts.tokens.contains("--zw-spacing-unit: .25rem;"));
    assert!(result
        .parts
        .tokens
        .contains("--zw-color-gray-500: #37628a;"));
    assert_eq!(result.parts.tokens.matches("@layer zw-tokens").count(), 1);
    expect_writes(
        &result,
        "p-4",
        &[
            ("padding-top", "1rem"),
            ("padding-right", "1rem"),
            ("padding-bottom", "1rem"),
            ("padding-left", "1rem"),
        ],
    );
    expect_writes(
        &result,
        "bg-gray-500",
        &[("background-color", "var(--zw-color-gray-500)")],
    );
}

#[test]
fn removed_tokens_cannot_reuse_a_previous_stylesheet() {
    let mut config = WindConfig::default();
    config.tokens.spacing.insert("hsp-sm".into(), "17px".into());
    config
        .tokens
        .colors
        .insert("surface".into(), "#123456".into());
    let names = ["block", "p-hsp-sm", "bg-surface"];
    let first = run(&names, config.clone(), source(SourcePositionKind::Class));
    assert_eq!(first.rules.len(), 3);
    assert!(first.parts.tokens.contains("--zw-spacing-hsp-sm: 17px;"));
    config.tokens.spacing.insert("hsp-sm".into(), "23px".into());
    config
        .tokens
        .colors
        .insert("surface".into(), "#654321".into());
    let changed = run(&names, config.clone(), source(SourcePositionKind::Class));
    assert_eq!(changed.rules.len(), 3);
    assert!(changed.parts.tokens.contains("--zw-spacing-hsp-sm: 23px;"));
    assert!(changed
        .parts
        .tokens
        .contains("--zw-color-surface: #654321;"));
    assert!(!changed.stylesheet.contains("17px"));
    assert!(!changed.stylesheet.contains("#123456"));
    config.tokens.spacing.clear();
    config.tokens.colors.clear();
    let removed = run(&names, config, source(SourcePositionKind::Class));
    assert_eq!(removed.rules.len(), 1);
    expect_writes(&removed, "block", &[("display", "block")]);
    assert_eq!(removed.diagnostics.len(), 2);
    assert!(removed
        .diagnostics
        .iter()
        .all(|d| d.code == DiagnosticCode::Zw006 && d.severity == Severity::Error));
    assert!(removed.parts.tokens.is_empty());
    assert!(!removed.stylesheet.contains("--zw-spacing-hsp-sm"));
    assert!(!removed.stylesheet.contains("--zw-color-surface"));
    assert_one_prelude(&removed);
    let empty = run(
        &["p-hsp-sm", "bg-surface"],
        WindConfig::default(),
        source(SourcePositionKind::Class),
    );
    assert!(empty.stylesheet.is_empty());
    assert!(empty.parts.prelude.is_empty());
}

#[test]
fn authored_reservations_and_arbitrary_values_remain_independent_of_tokens() {
    let mut config = WindConfig::default();
    config.authored_classes.insert("bg-gray-500".into(), true);
    let result = run(
        &["bg-gray-500", "p-[7px]"],
        config,
        source(SourcePositionKind::Class),
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.authored_classes.len(), 1);
    assert_eq!(result.authored_classes[0].text, "bg-gray-500");
    expect_writes(
        &result,
        "p-[7px]",
        &[
            ("padding-top", "7px"),
            ("padding-right", "7px"),
            ("padding-bottom", "7px"),
            ("padding-left", "7px"),
        ],
    );
}

#[test]
fn reset_mode_does_not_supply_design_tokens() {
    let empty = run(
        &[],
        WindConfig::default(),
        source(SourcePositionKind::Class),
    );
    assert!(empty.stylesheet.is_empty());
    assert!(empty.parts.prelude.is_empty());

    let reset = WindConfig {
        reset: ResetMode::MinimalV1,
        ..Default::default()
    };
    let result = run(&["p-4"], reset, source(SourcePositionKind::Class));
    assert_one_prelude(&result);
    assert!(!result.parts.reset.is_empty());
    assert!(result.parts.tokens.is_empty());
    assert!(result.parts.utilities.is_empty());
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].code, DiagnosticCode::Zw006);
}

#[test]
fn token_free_variants_keep_their_conditions() {
    let hover = run(
        &["hover:block"],
        WindConfig::default(),
        source(SourcePositionKind::Class),
    );
    assert!(hover.diagnostics.is_empty());
    expect_writes(&hover, "hover:block", &[("display", "block")]);
    assert_eq!(hover.rules[0].conditions, ["(hover: hover)"]);
    assert_one_prelude(&hover);

    let mut breakpoint = WindConfig::default();
    breakpoint.breakpoints.insert(
        "sm".into(),
        zudo_wind::BreakpointConfig { min_width_px: 640 },
    );
    let responsive = run(&["sm:block"], breakpoint, source(SourcePositionKind::Class));
    assert!(responsive.diagnostics.is_empty());
    expect_writes(&responsive, "sm:block", &[("display", "block")]);
    assert_eq!(responsive.rules[0].conditions, ["(min-width: 640px)"]);
    assert!(responsive.parts.tokens.is_empty());
    assert_one_prelude(&responsive);
}
