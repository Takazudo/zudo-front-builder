use zudo_wind::{
    compile, explain, render_explanation, CompileInput, DiagnosticCode, Origin, OriginCandidate,
    Severity, SourcePositionKind, TokenConfig, WindConfig,
};

fn config() -> WindConfig {
    let mut tokens = TokenConfig {
        spacing_unit: Some("0.25rem".into()),
        ..Default::default()
    };
    tokens.shadows.insert(
        "card".into(),
        "0 1px 3px color-mix(in srgb, var(--color-fg) 8%, transparent)".into(),
    );
    WindConfig {
        tokens,
        ..Default::default()
    }
}

fn candidate(text: &str, kind: SourcePositionKind) -> OriginCandidate {
    OriginCandidate {
        text: text.into(),
        origin: Origin::Source {
            source_id: "regressions.tsx".into(),
            byte_offset: 0,
            byte_length: text.len(),
            line: 1,
            byte_column: 1,
            literal_byte_offset: 0,
            literal_byte_length: text.len(),
            position_kind: kind,
        },
    }
}

fn compile_classes(texts: &[&str]) -> zudo_wind::CompileResult {
    compile(&CompileInput {
        candidates: texts
            .iter()
            .map(|text| candidate(text, SourcePositionKind::Class))
            .collect(),
        config: config(),
    })
}

#[test]
fn shadows_and_unambiguous_borders_compile_to_their_declarations() {
    let compiled = compile_classes(&[
        "shadow-[0_1px_3px_color-mix(in_srgb,var(--color-fg)_8%,transparent)]",
        "shadow-[var(--elevation)]",
        "shadow-card",
        "border-x-[3px]",
        "border-b-[red]",
        "border-y-[calc(var(--rule)_+_1px)]",
    ]);
    assert!(
        compiled.diagnostics.is_empty(),
        "{:?}",
        compiled.diagnostics
    );
    let css = &compiled.parts.utilities;
    for expected in [
        "box-shadow: 0 1px 3px color-mix(in srgb,var(--color-fg) 8%,transparent)",
        "box-shadow: var(--elevation)",
        "box-shadow: var(--zw-shadow-card)",
        "border-left-width: 3px",
        "border-right-width: 3px",
        "border-bottom-color: red",
        "border-top-width: calc(var(--rule) + 1px)",
        "border-bottom-width: calc(var(--rule) + 1px)",
    ] {
        assert!(css.contains(expected), "missing {expected:?} in:\n{css}");
    }
    assert!(!css.contains("border-left-color"), "{css}");
}

#[test]
fn ambiguous_variables_fail_in_class_position_and_inform_in_literals() {
    let class = compile_classes(&["border-[var(--x)]", "text-[var(--x)]"]);
    assert!(class.has_errors());
    for diagnostic in &class.diagnostics {
        assert_eq!(diagnostic.code, DiagnosticCode::Zw005);
        assert_eq!(diagnostic.severity, Severity::Error);
        assert_eq!(
            diagnostic.message,
            "value is ambiguous across catalog entries"
        );
    }
    assert!(
        class.parts.utilities.is_empty(),
        "{}",
        class.parts.utilities
    );

    let literal = compile(&CompileInput {
        candidates: vec![candidate("border-[var(--x)]", SourcePositionKind::Literal)],
        config: config(),
    });
    assert!(!literal.has_errors());
    assert_eq!(literal.diagnostics[0].severity, Severity::AuditInfo);
    assert_eq!(literal.diagnostics[0].code, DiagnosticCode::Zw005);
}

#[test]
fn explain_reports_status_type_hints_and_calc_suggestions() {
    let config = config();
    for text in [
        "shadow-[0_1px_3px_var(--x)]",
        "shadow-card",
        "bg-[var(--surface)]",
    ] {
        let explanation = explain(text, &config);
        assert_eq!(
            explanation.value_status.as_deref(),
            Some("categoryUnverified"),
            "{text}"
        );
    }
    assert_eq!(
        explain("border-[3px]", &config).value_status.as_deref(),
        Some("verified")
    );

    let hint = explain("text-[color:var(--x)]", &config);
    assert_eq!(hint.diagnostics[0].code, "ZW005");
    assert!(
        hint.diagnostics[0]
            .message
            .contains("data-type hint `color:` is not supported"),
        "{:?}",
        hint.diagnostics
    );

    for (text, property, suggestion) in [
        ("h-[calc(100%-3rem)]", "height", "h-[calc(100%_-_3rem)]"),
        (
            "min-h-[calc(100vh-3.5rem)]",
            "min-height",
            "min-h-[calc(100vh_-_3.5rem)]",
        ),
        ("w-[calc(100vw-2rem)]", "width", "w-[calc(100vw_-_2rem)]"),
        (
            "ml-[calc(var(--x)+1px)]",
            "margin-left",
            "ml-[calc(var(--x)_+_1px)]",
        ),
    ] {
        let explanation = explain(text, &config);
        let diagnostic = &explanation.diagnostics[0];
        assert_eq!(diagnostic.code, "ZW005", "{text}");
        assert!(
            diagnostic
                .message
                .contains(&format!("value is not valid for {property}")),
            "{text}: {}",
            diagnostic.message
        );
        assert_eq!(diagnostic.suggestion.as_deref(), Some(suggestion), "{text}");
        assert!(
            render_explanation(&explanation).contains(suggestion),
            "{text}"
        );
    }
    assert_eq!(
        explain("max-h-[foo]", &config).diagnostics[0].message,
        "value is not valid for max-height"
    );
    assert_eq!(
        explain("max-h-[foo]", &config).diagnostics[0].suggestion,
        None
    );
}
