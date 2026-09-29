mod common;

use lightningcss::{
    rules::CssRule,
    stylesheet::{ParserOptions, StyleSheet},
};
use zudo_wind::{compile, escape_class_name, Specificity};

#[test]
fn every_self_and_relation_state_has_exact_selector_guard_rank_and_specificity() {
    for (index, (state, suffix)) in [
        ("first", ":first-child"),
        ("last", ":last-child"),
        ("open", ":is([open], :popover-open)"),
        ("focus-within", ":focus-within"),
        ("hover", ":hover"),
        ("focus", ":focus"),
        ("focus-visible", ":focus-visible"),
        ("active", ":active"),
        ("disabled", ":disabled"),
    ]
    .into_iter()
    .enumerate()
    {
        for (prefix, relation_rank, classes) in [
            ("", 0, 2),
            ("group-", index + 1, 1),
            ("peer-", index + 10, 1),
        ] {
            let candidate = format!("{prefix}{state}:block");
            let result = compile(&common::input(&[&candidate]));
            assert!(!result.has_errors());
            let rule = &result.rules[0];
            let class = format!(".{}", escape_class_name(&candidate));
            let expected = match prefix {
                "group-" => format!(":where(.group{suffix}) {class}"),
                "peer-" => format!(":where(.peer{suffix}) ~ {class}"),
                _ => format!("{class}{suffix}"),
            };
            assert_eq!(rule.selector.as_deref(), Some(expected.as_str()));
            assert_eq!(
                rule.conditions,
                if state == "hover" {
                    vec!["(hover: hover)"]
                } else {
                    vec![]
                }
            );
            assert_eq!(
                rule.sort_key.as_ref().unwrap().relation_rank as usize,
                relation_rank
            );
            assert_eq!(
                rule.sort_key.as_ref().unwrap().state_rank as usize,
                if prefix.is_empty() { index + 1 } else { 0 }
            );
            assert_eq!(
                rule.specificity,
                Specificity {
                    ids: 0,
                    classes,
                    types: 0
                }
            );
            assert_specificity(rule.selector.as_ref().unwrap(), rule.specificity);
            StyleSheet::parse(&result.stylesheet, ParserOptions::default()).unwrap();
        }
    }
}

fn assert_specificity(selector: &str, expected: Specificity) {
    let css = format!("{selector} {{ display: block; }}");
    let sheet = StyleSheet::parse(&css, ParserOptions::default()).unwrap();
    let CssRule::Style(rule) = &sheet.rules.0[0] else {
        panic!()
    };
    let packed = (u32::from(expected.ids) << 20)
        | (u32::from(expected.classes) << 10)
        | u32::from(expected.types);
    assert_eq!(rule.selectors.0[0].specificity(), packed, "{selector}");
}

#[test]
fn all_pseudo_elements_append_last_without_inventing_content() {
    for (index, pseudo) in ["before", "after", "marker", "placeholder", "backdrop"]
        .into_iter()
        .enumerate()
    {
        let candidate = format!("dark:group-focus:focus-visible:{pseudo}:block");
        let result = compile(&common::input(&[&candidate]));
        assert!(!result.has_errors());
        let rule = &result.rules[0];
        assert!(rule
            .selector
            .as_ref()
            .unwrap()
            .ends_with(&format!(":focus-visible::{pseudo}")));
        assert_eq!(
            rule.sort_key.as_ref().unwrap().pseudo_rank as usize,
            index + 1
        );
        assert_eq!(
            rule.specificity,
            Specificity {
                ids: 0,
                classes: 2,
                types: 1
            }
        );
        assert_specificity(rule.selector.as_ref().unwrap(), rule.specificity);
        assert!(!result.stylesheet.contains("content:"));
    }
}

#[test]
fn responsive_hover_is_one_wrapper_and_dark_matches_self_or_any_ancestor() {
    let candidate = "sm:dark:group-hover:hover:before:bg-panel";
    let result = compile(&common::input(&[candidate]));
    assert!(!result.has_errors());
    assert_eq!(
        result.rules[0].conditions,
        ["(min-width: 640px)", "(hover: hover)"]
    );
    assert_eq!(result.parts.utilities.matches("@media").count(), 1);
    assert!(result
        .parts
        .utilities
        .starts_with("@media (min-width: 640px) and (hover: hover) {\n"));
    assert_eq!(result.rules[0].selector.as_deref(), Some(":where(.group:hover) .sm\\:dark\\:group-hover\\:hover\\:before\\:bg-panel:where([data-theme=\"dark\"], [data-theme=\"dark\"] *):hover::before"));
    assert_specificity(
        result.rules[0].selector.as_ref().unwrap(),
        result.rules[0].specificity,
    );
}

#[test]
fn responsive_ranks_use_width_order_and_exclusive_max() {
    let result = compile(&common::input(&[
        "max-2xl:block",
        "2xl:block",
        "max-sm:block",
        "sm:block",
        "block",
    ]));
    assert!(!result.has_errors());
    assert_eq!(
        result
            .rules
            .iter()
            .map(|rule| rule.candidate.as_str())
            .collect::<Vec<_>>(),
        [
            "block",
            "sm:block",
            "max-sm:block",
            "2xl:block",
            "max-2xl:block"
        ]
    );
    assert_eq!(
        result
            .rules
            .iter()
            .map(|rule| rule.sort_key.as_ref().unwrap().responsive_rank)
            .collect::<Vec<_>>(),
        [0, 1, 2, 3, 4]
    );
    assert_eq!(result.rules[2].conditions, ["(width < 640px)"]);
    assert_eq!(result.rules[4].conditions, ["(width < 1280px)"]);
    assert!(result.rules[3]
        .selector
        .as_ref()
        .unwrap()
        .starts_with(".\\32 xl"));
}

#[test]
fn dark_configuration_value_escapes_quoted_css_strings() {
    let mut input = common::input(&["dark:block"]);
    input.config.dark.as_mut().unwrap().value = "quoted\"\\value'[]".to_owned();
    let result = compile(&input);
    assert!(!result.has_errors());
    let selector = result.rules[0].selector.as_ref().unwrap();
    assert!(selector.contains(r#"[data-theme="quoted\"\\value'[]"]"#));
    assert_specificity(
        selector,
        Specificity {
            ids: 0,
            classes: 1,
            types: 0,
        },
    );
}

#[test]
fn child_filters_follow_complete_subject_and_document_added_specificity() {
    for utility in [
        "space-x-2",
        "space-y-2",
        "divide-x",
        "divide-y",
        "divide-panel",
    ] {
        let candidate = format!("dark:peer-hover:focus:{utility}");
        let result = compile(&common::input(&[&candidate]));
        assert!(!result.has_errors());
        let rule = &result.rules[0];
        assert!(rule
            .selector
            .as_ref()
            .unwrap()
            .ends_with(":focus > :not([hidden]) ~ :not([hidden])"));
        assert_eq!(rule.conditions, ["(hover: hover)"]);
        assert_eq!(
            rule.specificity,
            Specificity {
                ids: 0,
                classes: 4,
                types: 0
            }
        );
        assert_specificity(rule.selector.as_ref().unwrap(), rule.specificity);
    }
}

#[test]
fn rejected_chains_and_named_markers_never_emit() {
    let input = common::input(&[
        "hover:dark:block",
        "focus:hover:block",
        "sm:max-sm:block",
        "group-hover:peer-hover:block",
        "before:after:block",
        "before:divide-y",
        "group/name",
        "peer/name",
        "group-hover/name:block",
        "hover:group",
        "!block",
    ]);
    let result = compile(&input);
    assert_eq!(result.diagnostics.len(), input.candidates.len());
    assert!(result.rules.is_empty());
    assert!(result.parts.utilities.is_empty());
    assert_eq!(
        result.diagnostics[0].suggested_spelling.as_deref(),
        Some("dark:hover:block")
    );
}

#[test]
fn variant_axes_sort_before_conflict_groups() {
    let result = compile(&common::input(&[
        "sm:block",
        "dark:block",
        "peer-first:block",
        "group-disabled:block",
        "disabled:block",
        "before:block",
        "p-0",
    ]));
    assert!(!result.has_errors());
    assert_eq!(
        result
            .rules
            .iter()
            .map(|rule| rule.candidate.as_str())
            .collect::<Vec<_>>(),
        [
            "p-0",
            "before:block",
            "disabled:block",
            "group-disabled:block",
            "peer-first:block",
            "dark:block",
            "sm:block"
        ]
    );
}
