use std::collections::BTreeSet;

use zudo_wind::{
    parse_candidate, Catalog, DiagnosticCode, Origin, Resolution, ResolvedRule, SourcePositionKind,
    TokenConfig, ValidatedTokens, ValueStatus, VariantVocabulary,
};

fn tokens() -> ValidatedTokens {
    let mut config = TokenConfig {
        spacing_unit: Some("0.25rem".to_owned()),
        ..TokenConfig::default()
    };
    config.shadows.insert(
        "card".to_owned(),
        "0 1px 3px color-mix(in srgb, var(--color-fg) 8%, transparent)".to_owned(),
    );
    config.validate().unwrap()
}

fn resolve(text: &str) -> Resolution {
    let candidate = parse_candidate(text, &VariantVocabulary::default()).unwrap();
    let origin = Origin::Source {
        source_id: "test.tsx".to_owned(),
        byte_offset: 0,
        byte_length: 1,
        line: 1,
        byte_column: 1,
        literal_byte_offset: 0,
        literal_byte_length: 1,
        position_kind: SourcePositionKind::Class,
    };
    Catalog::v1().resolve(&candidate, &tokens(), &origin, &BTreeSet::new())
}

fn rule(text: &str) -> ResolvedRule {
    match resolve(text) {
        Resolution::Rule(rule) => rule,
        other => panic!("{text}: {other:?}"),
    }
}

fn failure(text: &str) -> (DiagnosticCode, String) {
    if let Err(diagnostic) = parse_candidate(text, &VariantVocabulary::default()) {
        return (diagnostic.code, diagnostic.message);
    }
    match resolve(text) {
        Resolution::Failure(diagnostic) | Resolution::Diagnostic(diagnostic) => {
            (diagnostic.code, diagnostic.message)
        }
        other => panic!("{text} should fail: {other:?}"),
    }
}

fn assert_rule(text: &str, entry_id: &str, property: &str, value: &str, status: ValueStatus) {
    let rule = rule(text);
    assert_eq!(rule.entry_id, entry_id, "{text}");
    assert_eq!(rule.value_status, status, "{text}");
    let declaration = &rule.declarations[0];
    assert_eq!(declaration.property, property, "{text}");
    assert_eq!(declaration.value, value, "{text}");
}

#[test]
fn shadow_variables_are_category_unverified_like_their_token_equivalent() {
    for (candidate, value) in [
        ("shadow-[var(--x)]", "var(--x)"),
        ("shadow-[0_1px_3px_var(--x)]", "0 1px 3px var(--x)"),
        (
            "shadow-[0_1px_3px_color-mix(in_srgb,var(--color-fg)_8%,transparent)]",
            "0 1px 3px color-mix(in srgb,var(--color-fg) 8%,transparent)",
        ),
        (
            "shadow-[0_1px_var(--blur,2px)_var(--shadow-color)]",
            "0 1px var(--blur,2px) var(--shadow-color)",
        ),
        (
            "shadow-[0_1px_3px_var(--a,var(--b,red))]",
            "0 1px 3px var(--a,var(--b,red))",
        ),
        (
            "shadow-[0_1px_2px_red,inset_0_0_0_1px_var(--ring)]",
            "0 1px 2px red,inset 0 0 0 1px var(--ring)",
        ),
    ] {
        assert_rule(
            candidate,
            "v1.shadow",
            "box-shadow",
            value,
            ValueStatus::CategoryUnverified,
        );
    }
    assert_rule(
        "shadow-[0_1px_3px_color-mix(in_srgb,red_8%,transparent)]",
        "v1.shadow",
        "box-shadow",
        "0 1px 3px color-mix(in srgb,red 8%,transparent)",
        ValueStatus::Verified,
    );
    assert_eq!(
        rule("shadow-card").value_status,
        ValueStatus::CategoryUnverified,
        "the token and the arbitrary spelling of one value agree"
    );
}

#[test]
fn malformed_injected_and_url_variable_values_stay_rejected() {
    for candidate in [
        "shadow-[var(x)]",
        "shadow-[var()]",
        "bg-[var(--)]",
        "bg-[var(--x)_)]",
        "bg-[var(--x]",
        "bg-[url(var(--x))]",
        "bg-[var(--x);color:red]",
        "bg-[var(--x)!important]",
        "shadow-[0_1px_3px_var(--x)_garbage(]",
    ] {
        let (code, _) = failure(candidate);
        assert!(
            matches!(code, DiagnosticCode::Zw001 | DiagnosticCode::Zw005),
            "{candidate}: {code:?}"
        );
    }
    assert_rule(
        "bg-[var(--zw-color-brand)]",
        "v1.background",
        "background-color",
        "var(--zw-color-brand)",
        ValueStatus::CategoryUnverified,
    );
}

#[test]
fn variable_border_values_are_ambiguous_on_every_side() {
    for candidate in [
        "border-[var(--x)]",
        "border-x-[var(--x)]",
        "border-y-[var(--x)]",
        "border-t-[var(--x)]",
        "border-r-[var(--x)]",
        "border-b-[var(--x)]",
        "border-l-[var(--x)]",
        "text-[var(--x)]",
    ] {
        assert_eq!(
            failure(candidate),
            (
                DiagnosticCode::Zw005,
                "value is ambiguous across catalog entries".to_owned()
            ),
            "{candidate}"
        );
    }
}

#[test]
fn literal_border_and_text_values_select_one_category() {
    assert_rule(
        "border-[3px]",
        "v1.border.width",
        "border-top-width",
        "3px",
        ValueStatus::Verified,
    );
    assert_rule(
        "border-x-[3px]",
        "v1.border.width.x",
        "border-left-width",
        "3px",
        ValueStatus::Verified,
    );
    assert_rule(
        "border-[red]",
        "v1.border.color",
        "border-top-color",
        "red",
        ValueStatus::Verified,
    );
    assert_rule(
        "border-b-[#123456]",
        "v1.border.color.b",
        "border-bottom-color",
        "#123456",
        ValueStatus::Verified,
    );
    assert_rule(
        "border-y-[calc(var(--x)_+_1px)]",
        "v1.border.width.y",
        "border-top-width",
        "calc(var(--x) + 1px)",
        ValueStatus::CategoryUnverified,
    );
    let size = rule("text-[1.25rem]");
    assert_eq!(size.declarations[0].property, "font-size");
    assert_eq!(size.value_status, ValueStatus::Verified);
    let color = rule("text-[color-mix(in_srgb,red_50%,blue)]");
    assert_eq!(color.declarations[0].property, "color");
    assert_eq!(color.value_status, ValueStatus::Verified);
}

fn suggestion(text: &str) -> Option<String> {
    match resolve(text) {
        Resolution::Failure(diagnostic) | Resolution::Diagnostic(diagnostic) => {
            diagnostic.suggested_spelling
        }
        other => panic!("{text} should fail: {other:?}"),
    }
}

#[test]
fn type_hints_are_reported_as_unsupported_syntax() {
    for (candidate, hint) in [
        ("text-[color:var(--x)]", "color"),
        ("text-[length:var(--x)]", "length"),
        ("bg-[color:red]", "color"),
        ("w-[length:2rem]", "length"),
    ] {
        let (code, message) = failure(candidate);
        assert_eq!(code, DiagnosticCode::Zw005, "{candidate}");
        assert!(
            message.contains(&format!("data-type hint `{hint}:` is not supported")),
            "{candidate}: {message}"
        );
        assert!(!message.contains("font-size"), "{candidate}: {message}");
    }
}

#[test]
fn overloaded_roots_name_every_attempted_property() {
    let (_, message) = failure("text-[bogus]");
    assert!(
        message.starts_with("value is not valid for any of ")
            && message.contains("font-size")
            && message.contains("color"),
        "{message}"
    );
    let (_, message) = failure("border-[bogus]");
    assert!(
        message.contains("border-top-width") && message.contains("border-top-color"),
        "{message}"
    );
    let (_, message) = failure("h-[bogus]");
    assert_eq!(message, "value is not valid for height");
}

#[test]
fn unspaced_calc_operators_get_a_validated_suggestion() {
    for (candidate, expected, property) in [
        ("w-[calc(100vw-2rem)]", "w-[calc(100vw_-_2rem)]", "width"),
        ("h-[calc(100%-3rem)]", "h-[calc(100%_-_3rem)]", "height"),
        (
            "min-h-[calc(100vh-3.5rem)]",
            "min-h-[calc(100vh_-_3.5rem)]",
            "min-height",
        ),
        (
            "ml-[calc(var(--x)+1px)]",
            "ml-[calc(var(--x)_+_1px)]",
            "margin-left",
        ),
        (
            "w-[calc(var(--side-gap)-1px)]",
            "w-[calc(var(--side-gap)_-_1px)]",
            "width",
        ),
        (
            "w-[calc(-1*var(--x)-2px)]",
            "w-[calc(-1*var(--x)_-_2px)]",
            "width",
        ),
        (
            "w-[calc(var(--a,var(--b-c))-1px)]",
            "w-[calc(var(--a,var(--b-c))_-_1px)]",
            "width",
        ),
        (
            "hover:w-[calc(100%-min(2rem,10vw))]",
            "hover:w-[calc(100%_-_min(2rem,10vw))]",
            "width",
        ),
    ] {
        let (code, message) = failure(candidate);
        assert_eq!(code, DiagnosticCode::Zw005, "{candidate}");
        assert!(
            message.contains(&format!("value is not valid for {property}"))
                && message.contains("calc() needs spaces around + and -"),
            "{candidate}: {message}"
        );
        assert_eq!(
            suggestion(candidate).as_deref(),
            Some(expected),
            "{candidate}"
        );
        assert!(
            matches!(resolve(expected), Resolution::Rule(_)),
            "{expected} must resolve"
        );
    }
    for candidate in [
        "w-[calc(1px-red)]",
        "w-[calc(100%_*_foo)]",
        "w-[bogus-value]",
    ] {
        assert_eq!(suggestion(candidate), None, "{candidate}");
    }
    assert!(matches!(
        resolve("w-[calc(100vw_-_2rem)]"),
        Resolution::Rule(_)
    ));
}

#[test]
fn sizing_and_underline_offset_validate_against_their_own_properties() {
    assert_rule(
        "max-w-[none]",
        "v1.max-w",
        "max-width",
        "none",
        ValueStatus::Verified,
    );
    assert_rule(
        "max-h-[none]",
        "v1.max-h",
        "max-height",
        "none",
        ValueStatus::Verified,
    );
    let size = rule("size-[3rem]");
    assert_eq!(
        size.declarations
            .iter()
            .map(|declaration| (declaration.property.as_str(), declaration.value.as_str()))
            .collect::<Vec<_>>(),
        [("width", "3rem"), ("height", "3rem")]
    );
    assert_rule(
        "underline-offset-4",
        "v1.underline-offset",
        "text-underline-offset",
        "4px",
        ValueStatus::Verified,
    );
    assert_rule(
        "underline-offset-[3px]",
        "v1.underline-offset",
        "text-underline-offset",
        "3px",
        ValueStatus::Verified,
    );
    assert_eq!(
        failure("underline-offset-[red]").1,
        "arbitrary value must be a nonnegative length"
    );
}
