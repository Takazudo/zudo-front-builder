mod common;

use zudo_wind::{
    compile, explain, DiagnosticCode, ExplanationOutcome, Origin, WindConfig, SPEC_REVISION,
    SPEC_VERSION,
};

#[test]
fn new_motion_and_aspect_classes_emit_css_with_variants() {
    let result = compile(&common::input(&[
        "transition-shadow",
        "focus:transition-colors",
        "hover:aspect-4/3",
        "aspect-[1.5/2.5]",
    ]));
    assert!(!result.has_errors(), "{:?}", result.diagnostics);
    let css = &result.stylesheet;
    assert!(css.contains("transition-property: box-shadow;"));
    assert!(css.contains("transition-duration: 150ms;"));
    assert!(css.contains("transition-timing-function: ease;"));
    assert!(css.contains("transition-property: color,background-color,border-color,outline-color,text-decoration-color,fill,stroke;"));
    assert!(css.contains("aspect-ratio: 4 / 3;"));
    assert!(css.contains("aspect-ratio: 1.5 / 2.5;"));
    assert!(!css.contains("calc(100% * 4 / 3)"));
}

#[test]
fn explanation_reports_new_rules_and_decimal_hint() {
    let config = WindConfig::default();
    for (candidate, property, value) in [
        ("transition-shadow", "transition-property", "box-shadow"),
        ("aspect-1/2", "aspect-ratio", "1 / 2"),
    ] {
        let explanation = explain(candidate, &config);
        assert_eq!(explanation.outcome, ExplanationOutcome::ResolvedUtility);
        assert_eq!(
            (explanation.spec_version, explanation.spec_revision),
            (SPEC_VERSION, SPEC_REVISION)
        );
        assert_eq!(explanation.declarations[0].property, property);
        assert_eq!(explanation.declarations[0].value, value);
    }
    let invalid = explain("aspect-1.5", &config);
    assert_eq!(invalid.outcome, ExplanationOutcome::Invalid);
    assert_eq!(
        invalid.diagnostics[0].suggestion.as_deref(),
        Some("aspect-[1.5/1]")
    );
}

#[test]
fn configured_transition_timing_applies_to_every_transition_shape() {
    for timing in [
        "linear",
        "cubic-bezier(0.2, 0, 0, 1)",
        "steps(4, end)",
        "var(--project-ease)",
    ] {
        let mut input = common::input(&[
            "transition",
            "transition-shadow",
            "transition-[left,color]",
            "hover:transition-colors",
            "transition-none",
        ]);
        input.config.default_transition_timing_function = Some(timing.to_owned());
        let compiled = compile(&input);
        assert!(
            !compiled.has_errors(),
            "{timing}: {:?}",
            compiled.diagnostics
        );
        for rule in &compiled.rules {
            if !rule.parsed.utility.named.starts_with("transition") {
                continue;
            }
            let declarations = &rule.resolved.as_ref().unwrap().declarations;
            let timing_declaration = declarations
                .iter()
                .find(|declaration| declaration.property == "transition-timing-function");
            if rule.candidate == "transition-none" {
                assert!(timing_declaration.is_none());
            } else {
                assert_eq!(
                    timing_declaration.unwrap().value,
                    timing,
                    "{}",
                    rule.candidate
                );
            }
        }
        let explained = explain("transition-shadow", &input.config);
        assert_eq!(explained.declarations[2].value, timing);
        assert_eq!(
            explained.value_status.as_deref(),
            Some(if timing.starts_with("var(") {
                "categoryUnverified"
            } else {
                "verified"
            })
        );
    }
}

#[test]
fn explicit_easing_wins_in_each_variant_regardless_of_candidate_arrival_order() {
    for classes in [
        [
            "ease-gentle",
            "transition-shadow",
            "hover:ease-[steps(2,end)]",
            "hover:transition-colors",
        ],
        [
            "hover:transition-colors",
            "hover:ease-[steps(2,end)]",
            "transition-shadow",
            "ease-gentle",
        ],
    ] {
        let mut input = common::input(&classes);
        input.config.default_transition_timing_function = Some("linear".to_owned());
        input
            .config
            .tokens
            .easings
            .insert("gentle".to_owned(), "ease-in-out".to_owned());
        let result = compile(&input);
        assert!(!result.has_errors(), "{:?}", result.diagnostics);
        let candidates: Vec<_> = result
            .rules
            .iter()
            .map(|rule| rule.candidate.as_str())
            .collect();
        assert!(
            candidates
                .iter()
                .position(|candidate| *candidate == "transition-shadow")
                .unwrap()
                < candidates
                    .iter()
                    .position(|candidate| *candidate == "ease-gentle")
                    .unwrap()
        );
        assert!(
            candidates
                .iter()
                .position(|candidate| *candidate == "hover:transition-colors")
                .unwrap()
                < candidates
                    .iter()
                    .position(|candidate| *candidate == "hover:ease-[steps(2,end)]")
                    .unwrap()
        );
        assert!(result
            .stylesheet
            .contains("transition-timing-function: linear;"));
        assert!(result
            .stylesheet
            .contains("transition-timing-function: var(--zw-ease-gentle);"));
        assert!(result
            .stylesheet
            .contains("transition-timing-function: steps(2,end);"));
    }
}

#[test]
fn transition_timing_reuses_easing_value_validation_with_config_path() {
    for value in [
        "",
        "ease, linear",
        "inherit",
        "bogus",
        "ease; color: red",
        "var(--zw-private)",
    ] {
        let config = WindConfig {
            default_transition_timing_function: Some(value.to_owned()),
            ..WindConfig::default()
        };
        let diagnostics = config.validate().unwrap_err();
        assert_eq!(diagnostics.len(), 1, "{value}: {diagnostics:?}");
        assert_eq!(diagnostics[0].code, DiagnosticCode::Zw007);
        assert!(
            matches!(diagnostics[0].origin.as_deref(), Some(Origin::Config { key_path }) if key_path == "defaultTransitionTimingFunction")
        );
    }
}
