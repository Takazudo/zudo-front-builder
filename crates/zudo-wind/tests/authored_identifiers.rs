//! #3523: underscore authored identifiers and the migration vocabulary across
//! every origin, asserting both emitted CSS and diagnostic severity.

use std::collections::BTreeMap;

use zudo_wind::{
    compile, explain, CompileInput, CompileResult, DiagnosticCode, ExplanationOutcome, Origin,
    OriginCandidate, Severity, SourcePositionKind, WindConfig,
};

fn config() -> WindConfig {
    let mut config = WindConfig::default();
    config.tokens.spacing_unit = Some("0.25rem".to_owned());
    config.tokens.colors = BTreeMap::from([("ink".to_owned(), "#111".to_owned())]);
    config
}

fn source(kind: SourcePositionKind) -> Origin {
    Origin::Source {
        source_id: "card.tsx".to_owned(),
        byte_offset: 10,
        byte_length: 4,
        line: 1,
        byte_column: 11,
        literal_byte_offset: 0,
        literal_byte_length: 40,
        position_kind: kind,
    }
}

fn manifest() -> Origin {
    Origin::Manifest {
        producer: "widgets".to_owned(),
        path: "wind.json".to_owned(),
        index: 0,
    }
}

fn compile_at(text: &str, origin: Origin, config: WindConfig) -> CompileResult {
    compile(&CompileInput {
        candidates: vec![
            OriginCandidate {
                text: text.to_owned(),
                origin,
            },
            OriginCandidate {
                text: "p-2".to_owned(),
                origin: source(SourcePositionKind::Class),
            },
        ],
        config,
    })
}

fn safelisted(text: &str, config: WindConfig) -> CompileResult {
    let mut config = config;
    config
        .safelist
        .insert("app".to_owned(), vec![text.to_owned(), "p-2".to_owned()]);
    compile(&CompileInput {
        candidates: Vec::new(),
        config,
    })
}

/// (code, severity) for the candidate under test; the `p-2` peer must always
/// still generate, so a stronger diagnostic never alters ordinary output.
fn outcome(result: &CompileResult, text: &str) -> Option<(DiagnosticCode, Severity)> {
    assert!(
        result
            .stylesheet
            .contains("padding: calc(var(--zw-spacing-unit) * 2)")
            || result.stylesheet.contains(".p-2"),
        "p-2 must still be emitted beside {text}: {}",
        result.stylesheet
    );
    assert!(
        !result
            .rules
            .iter()
            .any(|rule| rule.candidate == text && rule.resolved.is_some()),
        "{text} must never generate CSS"
    );
    let found: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.candidate.as_deref() == Some(text))
        .map(|diagnostic| (diagnostic.code, diagnostic.severity))
        .collect();
    assert!(found.len() <= 1, "{text}: {found:?}");
    found.first().copied()
}

fn is_ordinary(result: &CompileResult, text: &str) -> bool {
    result
        .ordinary_classes
        .iter()
        .any(|ordinary| ordinary.text == text)
}

#[test]
fn bem_and_snake_case_names_are_ordinary_at_every_source_position() {
    for text in [
        "card__title",
        "card_title",
        "block__el--mod",
        "grid__cell",
        "flex__item--wide",
        "tokenpanel-color-picker__convert",
        "hover:card__title",
    ] {
        for kind in [SourcePositionKind::Class, SourcePositionKind::Literal] {
            let result = compile_at(text, source(kind), config());
            assert_eq!(outcome(&result, text), None, "{text} {kind:?}");
            assert!(is_ordinary(&result, text), "{text} {kind:?}");
        }
        let explanation = explain(text, &config());
        assert_eq!(explanation.outcome, ExplanationOutcome::Ordinary, "{text}");
    }
}

#[test]
fn underscore_after_a_utility_root_stays_a_syntax_error() {
    for text in ["p_thing", "p_4", "grid_cell", "text-link_x", "ring_x"] {
        let class = compile_at(text, source(SourcePositionKind::Class), config());
        assert_eq!(
            outcome(&class, text),
            Some((DiagnosticCode::Zw001, Severity::Error)),
            "{text}"
        );
        let message = &class
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.candidate.as_deref() == Some(text))
            .unwrap()
            .message;
        assert!(message.contains("wind.authoredClasses"), "{message}");
        let literal = compile_at(text, source(SourcePositionKind::Literal), config());
        assert_eq!(
            outcome(&literal, text),
            Some((DiagnosticCode::Zw001, Severity::AuditInfo)),
            "{text}"
        );
    }
}

#[test]
fn explicit_origins_still_fail_for_unknown_and_underscore_names() {
    for (text, code) in [
        ("card__title", DiagnosticCode::Zw008),
        ("p_thing", DiagnosticCode::Zw001),
    ] {
        let manifest = compile_at(text, manifest(), config());
        assert_eq!(outcome(&manifest, text), Some((code, Severity::Error)));
        let safelist = safelisted(text, config());
        assert_eq!(outcome(&safelist, text), Some((code, Severity::Error)));
    }
}

#[test]
fn broken_brackets_and_recognized_roots_keep_their_shipped_diagnostics() {
    let broken = compile_at("p-[", source(SourcePositionKind::Class), config());
    assert!(matches!(
        outcome(&broken, "p-["),
        Some((DiagnosticCode::Zw001, Severity::Error))
    ));
    // #3389/#3460: a recognized root stays strict at a class position.
    let link = compile_at("text-link", source(SourcePositionKind::Class), config());
    assert_eq!(
        outcome(&link, "text-link"),
        Some((DiagnosticCode::Zw006, Severity::Error))
    );
    let message = &link.diagnostics[0].message;
    assert!(message.contains("wind.authoredClasses: { \"text-link\": true }"));
}

#[test]
fn exact_authored_suppression_wins_before_parsing() {
    for text in [
        "p_thing",
        "card__x-[1px]",
        "line-clamp-2",
        "text-link",
        "p-[",
    ] {
        let mut config = config();
        config.authored_classes.insert(text.to_owned(), true);
        for origin in [
            source(SourcePositionKind::Class),
            source(SourcePositionKind::Literal),
            manifest(),
        ] {
            let result = compile_at(text, origin, config.clone());
            assert_eq!(outcome(&result, text), None, "{text}");
            assert!(result
                .authored_classes
                .iter()
                .any(|authored| authored.text == text));
        }
        assert_eq!(
            explain(text, &config).outcome,
            ExplanationOutcome::Ordinary,
            "{text}"
        );
    }
}

const FOREIGN: &[&str] = &[
    "table",
    "contents",
    "flow-root",
    "line-clamp-2",
    "order-1",
    "basis-1/2",
    "fill-current",
    "stroke-current",
    "backdrop-blur-sm",
    "appearance-none",
    "will-change-transform",
    "not-sr-only",
    "content-[\"\"]",
    "before:content-none",
    "container",
];

#[test]
fn foreign_names_warn_by_default_and_error_under_strict_at_class_positions() {
    let mut strict = config();
    strict.strict = true;
    for text in FOREIGN {
        let default = compile_at(text, source(SourcePositionKind::Class), config());
        assert_eq!(
            outcome(&default, text),
            Some((DiagnosticCode::Zw014, Severity::Warning)),
            "{text}"
        );
        assert!(!default.has_errors(), "{text}");
        let message = &default.diagnostics[0].message;
        assert!(
            message.contains("unsupported foreign utility (migration vocabulary v1)")
                && message.contains("no CSS is generated")
                && message.contains("wind.authoredClasses"),
            "{message}"
        );
        assert!(!is_ordinary(&default, text), "{text}");

        let strict_class = compile_at(text, source(SourcePositionKind::Class), strict.clone());
        assert_eq!(
            outcome(&strict_class, text),
            Some((DiagnosticCode::Zw014, Severity::Error)),
            "{text}"
        );

        let explanation = explain(text, &config());
        assert_eq!(
            explanation.outcome,
            ExplanationOutcome::ForeignUtility,
            "{text}"
        );
    }
}

#[test]
fn low_confidence_literals_never_become_strict() {
    let mut strict = config();
    strict.strict = true;
    for text in FOREIGN {
        for config in [config(), strict.clone()] {
            let literal = compile_at(text, source(SourcePositionKind::Literal), config);
            assert_eq!(
                outcome(&literal, text),
                Some((DiagnosticCode::Zw014, Severity::AuditInfo)),
                "{text}"
            );
            assert!(!literal.has_errors());
        }
    }
}

#[test]
fn foreign_names_fail_from_manifests_and_safelists_regardless_of_strict() {
    for text in ["line-clamp-2", "container", "basis-1/2"] {
        let manifest = compile_at(text, manifest(), config());
        assert_eq!(
            outcome(&manifest, text),
            Some((DiagnosticCode::Zw014, Severity::Error))
        );
        let safelist = safelisted(text, config());
        assert_eq!(
            outcome(&safelist, text),
            Some((DiagnosticCode::Zw014, Severity::Error))
        );
    }
}

#[test]
fn plausible_authored_names_near_foreign_roots_stay_ordinary_under_strict() {
    let mut strict = config();
    strict.strict = true;
    for text in [
        "table-wrapper",
        "order-summary",
        "content-area",
        "container-inner",
    ] {
        let result = compile_at(text, source(SourcePositionKind::Class), strict.clone());
        assert_eq!(outcome(&result, text), None, "{text}");
        assert!(is_ordinary(&result, text), "{text}");
    }
}
