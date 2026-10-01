mod common;

use zudo_wind::{compile, explain, ExplanationOutcome, WindConfig, SPEC_REVISION, SPEC_VERSION};

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
