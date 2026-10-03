mod common;

use zudo_wind::{compile, explain, ExplanationOutcome, WindConfig};

fn resolved(candidate: &str, config: &WindConfig) -> Vec<(String, String)> {
    let explanation = explain(candidate, config);
    assert_eq!(
        explanation.outcome,
        ExplanationOutcome::ResolvedUtility,
        "{candidate}: {:?}",
        explanation.diagnostics
    );
    explanation
        .declarations
        .into_iter()
        .map(|declaration| (declaration.property, declaration.value))
        .collect()
}

fn rejected(candidate: &str, config: &WindConfig) -> (String, Option<String>) {
    let explanation = explain(candidate, config);
    assert_eq!(
        explanation.outcome,
        ExplanationOutcome::Invalid,
        "{candidate}: {:?}",
        explanation.declarations
    );
    let diagnostic = &explanation.diagnostics[0];
    (diagnostic.code.clone(), diagnostic.rejection_id.clone())
}

fn single(property: &str, value: &str) -> Vec<(String, String)> {
    vec![(property.to_owned(), value.to_owned())]
}

#[test]
fn each_new_utility_resolves_to_its_owned_declaration() {
    let config = common::config();
    for (candidate, property, value) in [
        ("wrap-anywhere", "overflow-wrap", "anywhere"),
        (
            "decoration-panel",
            "text-decoration-color",
            "var(--zw-color-panel)",
        ),
        (
            "decoration-current",
            "text-decoration-color",
            "currentColor",
        ),
        (
            "decoration-transparent",
            "text-decoration-color",
            "transparent",
        ),
        (
            "decoration-panel/40",
            "text-decoration-color",
            "color-mix(in oklab, var(--zw-color-panel) 40%, transparent)",
        ),
        ("decoration-[#ff0000]", "text-decoration-color", "#ff0000"),
        ("leading-none", "line-height", "1"),
        ("visible", "visibility", "visible"),
        ("invisible", "visibility", "hidden"),
        ("underline-offset-4", "text-underline-offset", "4px"),
        ("underline-offset-0", "text-underline-offset", "0px"),
        ("underline-offset-[0.3em]", "text-underline-offset", "0.3em"),
        (
            "underline-offset-[calc(1px_+_2px)]",
            "text-underline-offset",
            "calc(1px + 2px)",
        ),
    ] {
        assert_eq!(
            resolved(candidate, &config),
            single(property, value),
            "{candidate}"
        );
    }
    for keyword in [
        "n-resize",
        "s-resize",
        "e-resize",
        "w-resize",
        "ne-resize",
        "nw-resize",
        "se-resize",
        "sw-resize",
        "ew-resize",
        "ns-resize",
        "nesw-resize",
        "nwse-resize",
        "col-resize",
        "row-resize",
    ] {
        assert_eq!(
            resolved(&format!("cursor-{keyword}"), &config),
            single("cursor", keyword)
        );
    }
}

#[test]
fn configured_none_line_height_token_keeps_precedence_over_the_constant() {
    let mut config = common::config();
    config
        .tokens
        .line_heights
        .insert("none".to_owned(), "1.1".to_owned());
    assert!(config.validate().is_ok());
    assert_eq!(
        resolved("leading-none", &config),
        single("line-height", "var(--zw-leading-none)")
    );
    let explanation = explain("leading-none", &config);
    assert_eq!(explanation.token_resolutions[0].token_name, "none");
}

#[test]
fn missing_tokens_and_unsupported_forms_are_rejected() {
    let config = common::config();
    for (candidate, code, rejection) in [
        ("decoration-missing", "ZW006", "R17"),
        ("decoration-panel/101", "ZW005", "R14"),
        ("-decoration-panel", "ZW005", "R12"),
        ("leading-none/2", "ZW005", "R14"),
        ("-leading-none", "ZW005", "R12"),
        ("wrap-anywhere/2", "ZW005", "R14"),
        ("-wrap-anywhere", "ZW005", "R12"),
        ("visible/2", "ZW005", "R14"),
        ("-invisible", "ZW005", "R12"),
        ("cursor-col-resize/2", "ZW005", "R14"),
        ("cursor-diagonal-resize", "ZW006", "R17"),
        ("-underline-offset-2", "ZW005", "R12"),
        ("underline-offset-2/4", "ZW005", "R14"),
        ("underline-offset-1.5", "ZW005", "R15"),
        ("underline-offset-[auto]", "ZW005", "R15"),
        ("underline-offset-[10%]", "ZW005", "R15"),
        ("underline-offset-[-2px]", "ZW005", "R15"),
        ("underline-offset-[red]", "ZW005", "R15"),
        ("underline-offset-[1]", "ZW005", "R15"),
        ("underline-offset-[1px_2px]", "ZW005", "R15"),
        ("underline-offset-[calc(10%_+_1px)]", "ZW005", "R15"),
        ("underline-offset-[min(-2px,0px)]", "ZW005", "R15"),
        ("underline-offset-thick", "ZW006", "R17"),
    ] {
        assert_eq!(
            rejected(candidate, &config),
            (code.to_owned(), Some(rejection.to_owned())),
            "{candidate}"
        );
    }
}

#[test]
fn underline_offset_does_not_shadow_the_underline_static() {
    let config = common::config();
    assert_eq!(
        resolved("underline", &config),
        single("text-decoration-line", "underline")
    );
    let explanation = explain("underline-offset-2", &config);
    assert_eq!(
        explanation.entry_identifier.as_deref(),
        Some("v1.underline-offset")
    );
}

#[test]
fn new_utilities_emit_in_a_deterministic_order_with_variants() {
    let classes = [
        "hover:decoration-panel",
        "invisible",
        "md:visible",
        "underline-offset-2",
        "leading-none",
        "focus:cursor-ew-resize",
        "wrap-anywhere",
    ];
    let mut input = common::input(&classes);
    input.config.breakpoints.insert(
        "md".to_owned(),
        zudo_wind::BreakpointConfig { min_width_px: 768 },
    );
    let baseline = compile(&input);
    assert!(!baseline.has_errors(), "{:?}", baseline.diagnostics);
    let mut reversed = input.clone();
    reversed.candidates.reverse();
    assert_eq!(compile(&reversed), baseline);
    let css = &baseline.stylesheet;
    for declaration in [
        "overflow-wrap: anywhere;",
        "text-decoration-color: var(--zw-color-panel);",
        "line-height: 1;",
        "visibility: hidden;",
        "visibility: visible;",
        "cursor: ew-resize;",
        "text-underline-offset: 2px;",
    ] {
        assert!(css.contains(declaration), "{declaration}\n{css}");
    }
}
