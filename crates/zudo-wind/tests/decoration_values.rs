mod common;

use zudo_wind::{compile, explain, ExplanationOutcome, WindConfig, SPEC_REVISION};

fn resolved(candidate: &str, config: &WindConfig, id: &str, property: &str, value: &str) {
    let explanation = explain(candidate, config);
    assert_eq!(
        explanation.outcome,
        ExplanationOutcome::ResolvedUtility,
        "{candidate}"
    );
    assert_eq!(
        explanation.entry_identifier.as_deref(),
        Some(id),
        "{candidate}"
    );
    assert_eq!(explanation.spec_revision, SPEC_REVISION, "{candidate}");
    assert_eq!(explanation.declarations.len(), 1, "{candidate}");
    assert_eq!(
        explanation.declarations[0].property, property,
        "{candidate}"
    );
    assert_eq!(explanation.declarations[0].value, value, "{candidate}");
}

#[test]
fn thickness_and_style_values_resolve() {
    let config = common::config();
    for (candidate, value) in [
        ("decoration-0", "0px"),
        ("decoration-2", "2px"),
        ("decoration-[3px]", "3px"),
        ("decoration-[0.3em]", "0.3em"),
        ("decoration-[calc(1px_+_2px)]", "calc(1px + 2px)"),
    ] {
        resolved(
            candidate,
            &config,
            "v1.decoration.thickness",
            "text-decoration-thickness",
            value,
        );
    }
    for value in ["solid", "double", "dotted", "dashed", "wavy"] {
        resolved(
            &format!("decoration-{value}"),
            &config,
            &format!("v1.decoration.style.{value}"),
            "text-decoration-style",
            value,
        );
    }
    resolved(
        "decoration-[#ff0000]",
        &config,
        "v1.decoration.color",
        "text-decoration-color",
        "#ff0000",
    );
    resolved(
        "decoration-[red]/40",
        &config,
        "v1.decoration.color",
        "text-decoration-color",
        "color-mix(in oklab, red 40%, transparent)",
    );
}

#[test]
fn configured_color_tokens_retain_precedence_over_style_roots() {
    let mut config = common::config();
    for value in [
        "wavy",
        "solid-brand",
        "double-brand",
        "dotted-muted",
        "dashed-brand",
    ] {
        config
            .tokens
            .colors
            .insert(value.to_owned(), "#123456".to_owned());
    }
    assert!(config.validate().is_ok());
    for value in [
        "wavy",
        "solid-brand",
        "double-brand",
        "dotted-muted",
        "dashed-brand",
    ] {
        resolved(
            &format!("decoration-{value}"),
            &config,
            "v1.decoration.color",
            "text-decoration-color",
            &format!("var(--zw-color-{value})"),
        );
    }
    resolved(
        "decoration-dotted-muted/40",
        &config,
        "v1.decoration.color",
        "text-decoration-color",
        "color-mix(in oklab, var(--zw-color-dotted-muted) 40%, transparent)",
    );
}

#[test]
fn already_reserved_exact_style_color_token_names_stay_invalid() {
    for value in ["solid", "double", "dotted", "dashed"] {
        let mut config = common::config();
        config
            .tokens
            .colors
            .insert(value.to_owned(), "#123456".to_owned());
        assert!(config.validate().is_err(), "{value}");
    }
}

#[test]
fn missing_and_invalid_decoration_values_keep_diagnostics() {
    let config = common::config();
    for (candidate, code, rejection) in [
        ("decoration-missing", "ZW006", "R17"),
        ("decoration-dotted-muted", "ZW006", "R17"),
        ("decoration-dotted-muted/40", "ZW006", "R17"),
        ("decoration-auto", "ZW006", "R17"),
        ("decoration-from-font", "ZW006", "R17"),
        ("decoration-1.5", "ZW005", "R15"),
        ("decoration-[auto]", "ZW005", "R15"),
        ("decoration-[10%]", "ZW005", "R15"),
        ("decoration-[-2px]", "ZW005", "R15"),
        ("decoration-[1px_2px]", "ZW005", "R15"),
        ("decoration-[min(-2px,0px)]", "ZW005", "R15"),
        ("-decoration-2", "ZW005", "R12"),
        ("-decoration-[3px]", "ZW005", "R12"),
        ("-decoration-dotted", "ZW005", "R12"),
        ("decoration-2/4", "ZW005", "R14"),
        ("decoration-[3px]/40", "ZW005", "R14"),
        ("decoration-dotted/40", "ZW005", "R14"),
    ] {
        let explanation = explain(candidate, &config);
        assert_eq!(
            explanation.outcome,
            ExplanationOutcome::Invalid,
            "{candidate}"
        );
        assert_eq!(explanation.diagnostics[0].code, code, "{candidate}");
        assert_eq!(
            explanation.diagnostics[0].rejection_id.as_deref(),
            Some(rejection),
            "{candidate}"
        );
    }
    let missing = explain("decoration-missing", &config);
    assert!(missing.diagnostics[0]
        .message
        .contains("declare the intended token"));
    let prefixed_missing = explain("decoration-dotted-muted", &config);
    assert!(prefixed_missing.diagnostics[0]
        .message
        .contains("unknown value or token dotted-muted"));
    let prefixed_missing_with_opacity = explain("decoration-dotted-muted/40", &config);
    assert!(prefixed_missing_with_opacity.diagnostics[0]
        .message
        .contains("unknown value or token dotted-muted"));
    let ambiguous = explain("decoration-[var(--x)]", &config);
    assert_eq!(ambiguous.outcome, ExplanationOutcome::Invalid);
    assert_eq!(ambiguous.diagnostics[0].code, "ZW005");
    assert!(ambiguous.diagnostics[0]
        .message
        .contains("ambiguous across catalog entries"));
    for hint in ["color", "length"] {
        let candidate = format!("decoration-[{hint}:var(--x)]");
        let typed = explain(&candidate, &config);
        assert_eq!(typed.outcome, ExplanationOutcome::Invalid);
        assert!(typed.diagnostics[0]
            .message
            .contains(&format!("data-type hint `{hint}:` is not supported")));
    }
}

#[test]
fn decoration_output_is_deterministic_and_keeps_separate_properties() {
    let mut input = common::input(&[
        "decoration-panel",
        "decoration-2",
        "decoration-dotted",
        "underline",
        "hover:decoration-[3px]",
        "hover:decoration-wavy",
    ]);
    let baseline = compile(&input);
    assert!(!baseline.has_errors(), "{:?}", baseline.diagnostics);
    input.candidates.reverse();
    assert_eq!(compile(&input), baseline);
    let css = &baseline.stylesheet;
    for declaration in [
        "text-decoration-color: var(--zw-color-panel);",
        "text-decoration-thickness: 2px;",
        "text-decoration-style: dotted;",
        "text-decoration-thickness: 3px;",
        "text-decoration-style: wavy;",
    ] {
        assert!(css.contains(declaration), "{declaration}\n{css}");
    }
}
