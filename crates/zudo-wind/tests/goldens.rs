mod common;

use lightningcss::{
    rules::CssRule,
    stylesheet::{ParserOptions, StyleSheet},
};
use zudo_wind::{
    compile, reset_css, CompileInput, FontSizeToken, ResetMode, WindConfig, LAYER_ORDER,
};

#[test]
fn each_reset_matches_its_exact_stylesheet_golden_and_parses() {
    for (mode, golden) in [
        (ResetMode::None, include_str!("fixtures/golden/none.css")),
        (
            ResetMode::MinimalV1,
            include_str!("fixtures/golden/minimal-v1.css"),
        ),
        (
            ResetMode::OwnedV1,
            include_str!("fixtures/golden/owned-v1.css"),
        ),
    ] {
        let mut input = common::input(&[
            "block",
            "p-4",
            "px-2",
            "pl-1",
            "text-sm",
            "border",
            "divide-y",
            "transition-colors",
            "duration-300",
            "translate-x-2",
            "translate-y-4",
            "sm:dark:group-hover:focus:before:text-panel",
            "max-sm:peer-hover:space-y-2",
            "group",
        ]);
        input.config.reset = mode;
        input.config.tokens.font_sizes.insert(
            "sm".to_owned(),
            FontSizeToken {
                size: "0.875rem".to_owned(),
                line_height: Some("1.25rem".to_owned()),
            },
        );
        let result = compile(&input);
        assert!(!result.has_errors(), "{:?}", result.diagnostics);
        assert_eq!(result.stylesheet, golden, "{mode:?}");
        assert!(result.stylesheet.starts_with(LAYER_ORDER));
        assert!(result.stylesheet.ends_with('\n'));
        assert!(!result.stylesheet.ends_with("\n\n"));
        assert!(!result.stylesheet.contains('\r'));
        assert_eq!(result.parts.stylesheet(), result.stylesheet);
        let sheet = StyleSheet::parse(&result.stylesheet, ParserOptions::default()).unwrap();
        // Own-element utilities are top-level, outside every named layer.
        assert!(sheet
            .rules
            .0
            .iter()
            .any(|rule| matches!(rule, CssRule::Style(_))));
        assert_eq!(
            result
                .parts
                .registrations
                .matches("@property --zw-translate-x")
                .count(),
            1
        );
        assert_eq!(
            result
                .parts
                .registrations
                .matches("@property --zw-translate-y")
                .count(),
            1
        );
        assert!(
            result.stylesheet.find("@layer zw-tokens {").unwrap()
                < result.stylesheet.find("@property").unwrap()
        );
        assert!(
            result.stylesheet.find("@property").unwrap()
                < result.stylesheet.find(".block {").unwrap()
        );
    }
}

#[test]
fn reset_files_are_byte_pinned_to_spec_and_embedded_without_reformatting() {
    let spec = include_str!("../../../research/3242-zudo-wind-v1-spec.md");
    for (mode, heading) in [
        (ResetMode::None, "`none` is the empty byte string:"),
        (ResetMode::MinimalV1, "`minimal-v1`:"),
        (
            ResetMode::OwnedV1,
            "`owned-v1` is this complete original reset",
        ),
    ] {
        let expected = spec
            .split_once(heading)
            .unwrap()
            .1
            .split_once("```css\n")
            .unwrap()
            .1
            .split_once("```")
            .unwrap()
            .0;
        assert_eq!(reset_css(mode), expected);
        let result = compile(&CompileInput {
            config: WindConfig {
                reset: mode,
                ..WindConfig::default()
            },
            ..CompileInput::default()
        });
        if expected.is_empty() {
            assert!(result.stylesheet.is_empty());
        } else {
            let inner = result
                .parts
                .reset
                .strip_prefix("@layer zw-reset {\n")
                .unwrap()
                .strip_suffix("}\n")
                .unwrap();
            let unindented: String = inner
                .lines()
                .map(|line| format!("{}\n", line.strip_prefix("  ").unwrap()))
                .collect();
            assert_eq!(unindented, expected);
        }
    }
}

#[test]
fn tokens_emit_without_usage_and_registrations_require_translate_rules() {
    let input = common::input(&[]);
    let result = compile(&input);
    assert!(result.stylesheet.starts_with(LAYER_ORDER));
    assert!(result.parts.tokens.contains("--zw-color-panel: #123456;"));
    assert!(result.parts.registrations.is_empty());
    assert!(result.parts.utilities.is_empty());
    let result = compile(&common::input(&["rotate-90", "translate-x-missing"]));
    assert!(result.parts.registrations.is_empty());
    let result = compile(&common::input(&[
        "sm:translate-x-2",
        "hover:translate-y-4",
        "translate-x-0",
    ]));
    assert!(!result.has_errors());
    assert_eq!(result.parts.registrations.matches("@property").count(), 2);
    assert!(result
        .parts
        .registrations
        .contains("inherits: false;\n  initial-value: 0px;"));
    assert!(result.parts.utilities.contains(
        "--zw-translate-x: 0.5rem;\n    translate: var(--zw-translate-x) var(--zw-translate-y);"
    ));
}

#[test]
fn configured_values_serialize_comments_and_crlf_without_splitting_declarations() {
    let mut input = common::input(&[]);
    input.config.tokens.font_families.insert(
        "ui".to_owned(),
        "\"Example Family\",\r\n/* fallback\r\nfont */ sans-serif".to_owned(),
    );
    let result = compile(&input);
    assert!(!result.has_errors(), "{:?}", result.diagnostics);
    assert!(!result.parts.tokens.contains('\r'));
    assert_eq!(
        result
            .parts
            .tokens
            .lines()
            .filter(|line| line.contains("--zw-font-family-ui:"))
            .count(),
        1
    );
    assert!(result.parts.tokens.contains("sans-serif;\n"));
    assert!(result.parts.tokens.contains("/* fallback  font */"));
    assert_eq!(result.parts.tokens.lines().count(), 7);
    StyleSheet::parse(&result.stylesheet, ParserOptions::default()).unwrap();
}

#[test]
fn every_catalog_example_emits_parseable_stylesheet_with_its_complete_expansion() {
    let catalog = zudo_wind::Catalog::v1();
    let names: Vec<_> = catalog
        .entries()
        .iter()
        .flat_map(|entry| {
            entry
                .examples
                .iter()
                .map(|example| example.candidate.as_str())
        })
        .collect();
    let result = compile(&common::input(&names));
    assert!(!result.has_errors(), "{:?}", result.diagnostics);
    StyleSheet::parse(&result.stylesheet, ParserOptions::default()).unwrap();
    for entry in catalog.entries() {
        for example in &entry.examples {
            let metadata = result
                .rules
                .iter()
                .find(|rule| rule.candidate == example.candidate)
                .unwrap();
            assert_eq!(
                metadata.resolved.as_ref().unwrap().declarations,
                example.declarations,
                "{}",
                example.candidate
            );
        }
    }
}
