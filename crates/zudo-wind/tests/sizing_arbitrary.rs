use std::collections::BTreeSet;

use zudo_wind::{
    compile, explain, parse_candidate, Catalog, CompileInput, DiagnosticCode, ExplanationOutcome,
    Origin, OriginCandidate, Resolution, SourcePositionKind, TokenConfig, VariantVocabulary,
    WindConfig,
};

fn tokens() -> zudo_wind::ValidatedTokens {
    TokenConfig::default().validate().unwrap()
}

fn class_origin() -> Origin {
    Origin::Source {
        source_id: "sizing.tsx".to_owned(),
        byte_offset: 0,
        byte_length: 1,
        line: 1,
        byte_column: 1,
        literal_byte_offset: 0,
        literal_byte_length: 1,
        position_kind: SourcePositionKind::Class,
    }
}

fn resolve(candidate: &str) -> Resolution {
    let parsed = parse_candidate(candidate, &VariantVocabulary::default()).unwrap();
    Catalog::v1().resolve(&parsed, &tokens(), &class_origin(), &BTreeSet::new())
}

fn declarations(candidate: &str) -> Vec<(String, String)> {
    match resolve(candidate) {
        Resolution::Rule(rule) => rule
            .declarations
            .into_iter()
            .map(|declaration| (declaration.property, declaration.value))
            .collect(),
        other => panic!("{candidate}: {other:?}"),
    }
}

#[test]
fn sizing_arbitrary_values_validate_against_their_css_properties() {
    assert_eq!(
        declarations("max-h-[none]"),
        [("max-height".to_owned(), "none".to_owned())]
    );
    assert_eq!(declarations("max-h-[none]"), declarations("max-h-none"));
    assert_eq!(
        declarations("max-w-[none]"),
        [("max-width".to_owned(), "none".to_owned())]
    );
    assert_eq!(
        declarations("h-[calc(100%_-_3rem)]"),
        [("height".to_owned(), "calc(100% - 3rem)".to_owned())]
    );
    assert_eq!(
        declarations("min-h-[calc(100vh_-_3.5rem)]"),
        [("min-height".to_owned(), "calc(100vh - 3.5rem)".to_owned())]
    );
    assert_eq!(
        declarations("size-[10px]"),
        [
            ("width".to_owned(), "10px".to_owned()),
            ("height".to_owned(), "10px".to_owned()),
        ]
    );
}

#[test]
fn sizing_arbitrary_rejections_name_the_roots_property() {
    for (candidate, property) in [
        ("h-[foo]", "height"),
        ("min-h-[foo]", "min-height"),
        ("max-h-[foo]", "max-height"),
    ] {
        match resolve(candidate) {
            Resolution::Diagnostic(diagnostic) => {
                assert_eq!(diagnostic.code, DiagnosticCode::Zw005, "{candidate}");
                assert!(diagnostic.message.contains(property), "{diagnostic:?}");
            }
            other => panic!("{candidate}: {other:?}"),
        }
    }
    match resolve("min-w-[none]") {
        Resolution::Diagnostic(diagnostic) => {
            assert_eq!(diagnostic.code, DiagnosticCode::Zw005);
            assert!(diagnostic.message.contains("min-width"), "{diagnostic:?}");
        }
        other => panic!("min-w-[none]: {other:?}"),
    }
}

#[test]
fn public_compile_and_explain_resolve_strict_max_height_arbitrary_values() {
    let result = compile(&CompileInput {
        config: WindConfig::default(),
        candidates: vec![OriginCandidate {
            text: "max-h-[none]".to_owned(),
            origin: Origin::Safelist {
                owner: "sizing test".to_owned(),
                index: 0,
            },
        }],
    });
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert!(!result.has_errors());
    assert_eq!(result.rules.len(), 1);
    assert_eq!(
        result.rules[0]
            .resolved
            .as_ref()
            .unwrap()
            .declarations
            .iter()
            .map(|declaration| (declaration.property.as_str(), declaration.value.as_str()))
            .collect::<Vec<_>>(),
        [("max-height", "none")]
    );

    let explanation = explain("max-h-[none]", &WindConfig::default());
    assert_eq!(explanation.outcome, ExplanationOutcome::ResolvedUtility);
    assert_eq!(explanation.declarations.len(), 1);
    assert_eq!(explanation.declarations[0].property, "max-height");
    assert_eq!(explanation.declarations[0].value, "none");
    assert!(explanation.diagnostics.is_empty());
}
