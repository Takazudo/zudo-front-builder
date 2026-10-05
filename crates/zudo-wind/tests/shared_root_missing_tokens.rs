use zudo_wind::{
    compile, explain, CompileInput, DiagnosticCode, ExplanationOutcome, Origin, OriginCandidate,
    WindConfig,
};

const COLORS: [(&str, &str, &[&str], &str); 5] = [
    ("text-missing/70", "v1.text.color", &["color"], "70"),
    (
        "border-missing/20",
        "v1.border.color",
        &[
            "border-top-color",
            "border-right-color",
            "border-bottom-color",
            "border-left-color",
        ],
        "20",
    ),
    (
        "border-x-missing/20",
        "v1.border.color.x",
        &["border-left-color", "border-right-color"],
        "20",
    ),
    (
        "outline-missing/20",
        "v1.outline.color",
        &["outline-color"],
        "20",
    ),
    (
        "placeholder:text-missing/50",
        "v1.text.color",
        &["color"],
        "50",
    ),
];

#[test]
fn missing_color_tokens_with_opacity_report_the_token_on_shared_roots() {
    let config = WindConfig::default();
    for (candidate, _, _, _) in COLORS {
        let explanation = explain(candidate, &config);
        assert_eq!(
            explanation.outcome,
            ExplanationOutcome::Invalid,
            "{candidate}"
        );
        assert_eq!(explanation.diagnostics.len(), 1, "{candidate}");
        let diagnostic = &explanation.diagnostics[0];
        assert_eq!(diagnostic.code, "ZW006", "{candidate}");
        assert_eq!(
            diagnostic.rejection_id.as_deref(),
            Some("R17"),
            "{candidate}"
        );
        assert_eq!(
            diagnostic.message, format!("unknown value or token missing. If this is a genuine authored class, reserve its complete name with wind.authoredClasses: {{ \"{candidate}\": true }}; otherwise correct or declare the intended token. No generated utility CSS is emitted for this candidate"),
            "{candidate}"
        );

        let compiled = compile(&CompileInput {
            candidates: vec![OriginCandidate {
                text: candidate.to_owned(),
                origin: Origin::Safelist {
                    owner: "shared-root-test".to_owned(),
                    index: 0,
                },
            }],
            config: config.clone(),
        });
        assert_eq!(compiled.diagnostics.len(), 1, "{candidate}");
        assert_eq!(
            compiled.diagnostics[0].code,
            DiagnosticCode::Zw006,
            "{candidate}"
        );
    }
}

#[test]
fn configured_color_tokens_with_opacity_resolve_on_shared_roots() {
    let mut config = WindConfig::default();
    config
        .tokens
        .colors
        .insert("missing".into(), "#888888".into());
    for (candidate, entry, properties, opacity) in COLORS {
        let explanation = explain(candidate, &config);
        assert_eq!(
            explanation.outcome,
            ExplanationOutcome::ResolvedUtility,
            "{candidate}: {:?}",
            explanation.diagnostics
        );
        assert_eq!(
            explanation.entry_identifier.as_deref(),
            Some(entry),
            "{candidate}"
        );
        let value = format!("color-mix(in oklab, var(--zw-color-missing) {opacity}%, transparent)");
        assert_eq!(
            explanation
                .declarations
                .iter()
                .map(|declaration| (declaration.property.as_str(), declaration.value.as_str()))
                .collect::<Vec<_>>(),
            properties
                .iter()
                .map(|property| (*property, value.as_str()))
                .collect::<Vec<_>>(),
            "{candidate}"
        );
    }
}

#[test]
fn inset_fractions_resolve_for_all_roots_with_negative_and_variant_forms() {
    let config = WindConfig::default();
    let fraction = "calc(100% * 1 / 2)";
    for (candidate, entry, properties) in [
        (
            "inset-1/2",
            "v1.inset",
            &["top", "right", "bottom", "left"][..],
        ),
        ("inset-x-1/2", "v1.inset-x", &["left", "right"][..]),
        ("inset-y-1/2", "v1.inset-y", &["top", "bottom"][..]),
        ("top-1/2", "v1.top", &["top"][..]),
        ("right-1/2", "v1.right", &["right"][..]),
        ("bottom-1/2", "v1.bottom", &["bottom"][..]),
        ("left-1/2", "v1.left", &["left"][..]),
    ] {
        let explanation = explain(candidate, &config);
        assert_eq!(
            explanation.outcome,
            ExplanationOutcome::ResolvedUtility,
            "{candidate}: {:?}",
            explanation.diagnostics
        );
        assert_eq!(
            explanation.entry_identifier.as_deref(),
            Some(entry),
            "{candidate}"
        );
        assert_eq!(
            explanation
                .declarations
                .iter()
                .map(|declaration| (declaration.property.as_str(), declaration.value.as_str()))
                .collect::<Vec<_>>(),
            properties
                .iter()
                .map(|property| (*property, fraction))
                .collect::<Vec<_>>(),
            "{candidate}"
        );
    }

    for (candidate, entry, value) in [
        ("-left-1/2", "v1.left", "calc(calc(100% * 1 / 2) * -1)"),
        ("hover:top-1/2", "v1.top", fraction),
    ] {
        let explanation = explain(candidate, &config);
        assert_eq!(
            explanation.outcome,
            ExplanationOutcome::ResolvedUtility,
            "{candidate}: {:?}",
            explanation.diagnostics
        );
        assert_eq!(explanation.entry_identifier.as_deref(), Some(entry));
        assert_eq!(explanation.declarations.len(), 1, "{candidate}");
        assert_eq!(explanation.declarations[0].value, value, "{candidate}");
    }
}

#[test]
fn unsupported_shapes_still_report_shape_errors() {
    let config = WindConfig::default();
    for (candidate, rejection, message) in [
        ("p-1/2", "R14", "slash modifier is not supported"),
        ("-text-missing/70", "R12", "negative value is not supported"),
        (
            "border-missing/101",
            "R14",
            "colour opacity must be an integer from 0 through 100",
        ),
        ("outline-missing/nope", "R14", "invalid slash modifier"),
    ] {
        let explanation = explain(candidate, &config);
        assert_eq!(
            explanation.outcome,
            ExplanationOutcome::Invalid,
            "{candidate}"
        );
        let diagnostic = &explanation.diagnostics[0];
        assert_eq!(diagnostic.code, "ZW005", "{candidate}");
        assert_eq!(
            diagnostic.rejection_id.as_deref(),
            Some(rejection),
            "{candidate}"
        );
        assert_eq!(diagnostic.message, message, "{candidate}");
    }
}

#[test]
fn inset_fraction_denominators_keep_the_strict_positive_bounds() {
    let config = WindConfig::default();
    for (candidate, rejection, message) in [
        ("top-1/0", "R13", "fraction is out of range"),
        ("left-1/1000001", "R13", "fraction is out of range"),
        (
            "bottom-1/1.5",
            "R13",
            "fraction requires positive integer parts",
        ),
    ] {
        let explanation = explain(candidate, &config);
        assert_eq!(
            explanation.outcome,
            ExplanationOutcome::Invalid,
            "{candidate}"
        );
        let diagnostic = &explanation.diagnostics[0];
        assert_eq!(diagnostic.code, "ZW005", "{candidate}");
        assert_eq!(
            diagnostic.rejection_id.as_deref(),
            Some(rejection),
            "{candidate}"
        );
        assert_eq!(diagnostic.message, message, "{candidate}");
    }
}
