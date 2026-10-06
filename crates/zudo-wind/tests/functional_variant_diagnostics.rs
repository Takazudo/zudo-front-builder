use std::collections::BTreeMap;

use zudo_wind::{
    audit, audit_json, compile, explain, explanation_json, extract_candidates, render_audit,
    render_explanation, AuditInput, CompileInput, DiagnosticCode, Origin, OriginCandidate,
    Severity, SourceKind, SourcePositionKind, WindConfig,
};

fn config() -> WindConfig {
    let mut config = WindConfig::default();
    config.tokens.colors = BTreeMap::from([("accent".to_owned(), "#123456".to_owned())]);
    config
}

fn source(text: &str, kind: SourcePositionKind) -> OriginCandidate {
    OriginCandidate {
        text: text.to_owned(),
        origin: Origin::Source {
            source_id: "src/page.tsx".to_owned(),
            byte_offset: 17,
            byte_length: text.len(),
            line: 2,
            byte_column: 5,
            literal_byte_offset: 10,
            literal_byte_length: text.len() + 2,
            position_kind: kind,
        },
    }
}

#[test]
fn rejected_variants_keep_wrapped_migration_and_original_origin() {
    for text in [
        "min-[56rem]:table-auto",
        "max-[42rem]:table-auto",
        "has-[:focus-visible]:filter-[blur(2px)]",
        "supports-[display:grid]:bg-gradient-to-b",
        "group-data-[current=true]:table-auto",
    ] {
        let input = source(text, SourcePositionKind::Class);
        let result = compile(&CompileInput {
            candidates: vec![input.clone()],
            config: config(),
        });
        assert_eq!(
            result.diagnostics.len(),
            2,
            "{text}: {:?}",
            result.diagnostics
        );
        assert_eq!(result.diagnostics[0].code, DiagnosticCode::Zw004, "{text}");
        assert_eq!(result.diagnostics[1].code, DiagnosticCode::Zw014, "{text}");
        assert_eq!(result.diagnostics[1].severity, Severity::Warning, "{text}");
        assert!(result.rules.is_empty(), "{text}");
        assert!(!result.stylesheet.contains(text), "{text}");
        for diagnostic in &result.diagnostics {
            assert_eq!(diagnostic.candidate.as_deref(), Some(text));
            assert_eq!(diagnostic.origin.as_deref(), Some(&input.origin));
        }

        let explanation = explain(text, &config());
        assert_eq!(explanation.diagnostics.len(), 2, "{text}");
        assert!(
            render_explanation(&explanation).contains("ZW014 warning"),
            "{text}"
        );
        let json: serde_json::Value =
            serde_json::from_str(&explanation_json(&explanation).unwrap()).unwrap();
        assert_eq!(json["candidate"], text);
        assert_eq!(json["diagnostics"][1]["candidate"], text);
    }

    let mut with_safelist = config();
    with_safelist
        .safelist
        .insert("app".to_owned(), vec!["table-cell".to_owned()]);
    let explanation = explain("min-[56rem]:table-cell", &with_safelist);
    assert_eq!(explanation.diagnostics.len(), 2);
    assert!(explanation
        .diagnostics
        .iter()
        .all(|diagnostic| diagnostic.candidate.as_deref() == Some("min-[56rem]:table-cell")));
}

#[test]
fn valid_wrapped_utility_is_only_a_variant_error_and_bad_forms_stay_syntax_errors() {
    for text in [
        "min-[56rem]:flex",
        "not-[:first-child]:block",
        "nth-[2]:block",
        "nth-last-[2]:block",
        "peer-aria-[pressed=true]:block",
    ] {
        let result = compile(&CompileInput {
            candidates: vec![source(text, SourcePositionKind::Class)],
            config: config(),
        });
        assert_eq!(result.diagnostics.len(), 1, "{text}");
        assert_eq!(result.diagnostics[0].code, DiagnosticCode::Zw004, "{text}");
        assert!(result.rules.is_empty(), "{text}");
    }
    for text in [
        "min-[56rem:table-cell",
        "has-[]:table-cell",
        "custom-[x]:table-cell",
    ] {
        let result = compile(&CompileInput {
            candidates: vec![source(text, SourcePositionKind::Class)],
            config: config(),
        });
        assert_eq!(result.diagnostics.len(), 1, "{text}");
        assert_eq!(result.diagnostics[0].code, DiagnosticCode::Zw001, "{text}");
    }
    let forbidden_value = "has-[:focus-visible]:filter-[url(#fx)]";
    let result = compile(&CompileInput {
        candidates: vec![source(forbidden_value, SourcePositionKind::Class)],
        config: config(),
    });
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].code, DiagnosticCode::Zw004);
}

#[test]
fn audit_text_json_severity_and_authored_class_behavior() {
    let classes = [
        "min-[56rem]:table-auto",
        "has-[:focus-visible]:filter-[blur(2px)]",
        "supports-[display:grid]:bg-gradient-to-b",
    ];
    let html = format!("<div class=\"{}\"></div>", classes.join(" "));
    let report = audit(
        &AuditInput::single(
            "src/page.html",
            extract_candidates(html.as_bytes(), SourceKind::Html),
        ),
        &config(),
    );
    assert_eq!(report.diagnostics.len(), classes.len() * 2);
    for text in classes {
        let diagnostics: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.candidate.as_deref() == Some(text))
            .collect();
        assert_eq!(diagnostics.len(), 2, "{text}");
        assert_eq!(diagnostics[0].code, "ZW004", "{text}");
        assert_eq!(diagnostics[1].code, "ZW014", "{text}");
        for diagnostic in diagnostics {
            assert_eq!(
                diagnostic.origin.as_ref().unwrap().byte_offset,
                Some(html.find(text).unwrap())
            );
            assert_eq!(
                diagnostic.origin.as_ref().unwrap().byte_length,
                Some(text.len())
            );
        }
    }
    assert!(render_audit(&report).contains("ZW014 warning"));
    let json: serde_json::Value = serde_json::from_str(&audit_json(&report).unwrap()).unwrap();
    assert_eq!(
        json["diagnostics"].as_array().unwrap().len(),
        classes.len() * 2
    );

    let text = classes[0];

    let mut strict = config();
    strict.strict = true;
    let result = compile(&CompileInput {
        candidates: vec![source(text, SourcePositionKind::Class)],
        config: strict,
    });
    assert_eq!(result.diagnostics[1].severity, Severity::Error);
    let literal = compile(&CompileInput {
        candidates: vec![source(text, SourcePositionKind::Literal)],
        config: config(),
    });
    assert_eq!(literal.diagnostics[0].severity, Severity::AuditInfo);
    assert_eq!(literal.diagnostics[1].severity, Severity::AuditInfo);

    let mut authored = config();
    authored.authored_classes.insert(text.to_owned(), true);
    let result = compile(&CompileInput {
        candidates: vec![source(text, SourcePositionKind::Class)],
        config: authored,
    });
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.authored_classes.len(), 1);
}
