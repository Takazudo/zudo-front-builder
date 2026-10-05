mod common;

use zudo_wind::compile;

#[test]
fn every_padding_margin_and_gap_permutation_has_the_locked_winners() {
    for (classes, properties, expected) in [
        (
            ["p-4", "px-2", "pl-1"],
            vec![
                "padding-top",
                "padding-right",
                "padding-bottom",
                "padding-left",
            ],
            vec!["1rem", "0.5rem", "1rem", "0.25rem"],
        ),
        (
            ["m-4", "mx-2", "ml-1"],
            vec!["margin-top", "margin-right", "margin-bottom", "margin-left"],
            vec!["1rem", "0.5rem", "1rem", "0.25rem"],
        ),
        (
            ["gap-4", "gap-x-2", "gap-y-1"],
            vec!["column-gap", "row-gap"],
            vec!["0.5rem", "0.25rem"],
        ),
    ] {
        let input = common::input(&classes);
        let baseline = compile(&input);
        assert!(!baseline.has_errors());
        let sorted: Vec<_> = baseline
            .rules
            .iter()
            .map(|rule| rule.candidate.as_str())
            .collect();
        assert_eq!(sorted, classes);
        // This checks emitted declaration ordering, not browser computed styles.
        let writes: std::collections::BTreeMap<_, _> = baseline
            .rules
            .iter()
            .flat_map(|rule| rule.resolved.as_ref().unwrap().declarations.iter())
            .map(|declaration| (declaration.property.as_str(), declaration.value.as_str()))
            .collect();
        for (property, expected) in properties.iter().zip(expected) {
            assert_eq!(writes[property], expected);
        }
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let mut permuted = input.clone();
            permuted.candidates = order.map(|index| input.candidates[index].clone()).to_vec();
            assert_eq!(compile(&permuted), baseline);
        }
    }
}

#[test]
fn scope_order_covers_inset_sizing_border_radius_scroll_and_overflow() {
    for (names, scopes) in [
        (vec!["inset-4", "inset-x-2", "left-1"], vec![0, 1, 2]),
        (vec!["size-4", "w-2"], vec![0, 1]),
        (vec!["border-4", "border-x-2", "border-l-1"], vec![0, 1, 2]),
        (
            vec!["border-panel", "border-x-panel", "border-l-panel"],
            vec![0, 1, 2],
        ),
        (
            vec!["rounded-full", "rounded-t-full", "rounded-tl-full"],
            vec![0, 1, 2],
        ),
        (
            vec!["scroll-m-4", "scroll-mx-2", "scroll-ml-1"],
            vec![0, 1, 2],
        ),
        (vec!["overflow-hidden", "overflow-x-auto"], vec![0, 1]),
    ] {
        let result = compile(&common::input(&names));
        assert!(!result.has_errors(), "{:?}", result.diagnostics);
        assert_eq!(
            result
                .rules
                .iter()
                .map(|rule| rule.candidate.as_str())
                .collect::<Vec<_>>(),
            names
        );
        assert_eq!(
            result
                .rules
                .iter()
                .map(|rule| rule.sort_key.as_ref().unwrap().scope_rank)
                .collect::<Vec<_>>(),
            scopes
        );
    }
}

#[test]
fn same_scope_uses_raw_bytes_and_transition_defaults_precede_overrides() {
    let result = compile(&common::input(&[
        "p-2",
        "p-10",
        "duration-300",
        "transition-colors",
    ]));
    assert!(!result.has_errors());
    assert_eq!(
        result
            .rules
            .iter()
            .map(|rule| rule.candidate.as_str())
            .collect::<Vec<_>>(),
        ["p-10", "p-2", "transition-colors", "duration-300"]
    );
    let transition = result.rules[2].resolved.as_ref().unwrap();
    assert_eq!(transition.declarations[1].value, "150ms");
    assert_eq!(transition.declarations[2].value, "ease");
    assert!(
        result
            .stylesheet
            .find("transition-duration: 150ms")
            .unwrap()
            < result
                .stylesheet
                .find("transition-duration: 300ms")
                .unwrap()
    );
}

#[test]
fn bare_and_sized_outline_variants_resolve_and_order_deterministically() {
    let input = common::input(&["focus-visible:outline-2", "focus-visible:outline"]);
    let baseline = compile(&input);
    assert!(!baseline.has_errors(), "{:?}", baseline.diagnostics);

    assert_eq!(
        baseline
            .rules
            .iter()
            .map(|rule| rule.candidate.as_str())
            .collect::<Vec<_>>(),
        ["focus-visible:outline", "focus-visible:outline-2"]
    );
    assert_eq!(
        baseline.rules[0].selector.as_deref(),
        Some(".focus-visible\\:outline:focus-visible")
    );

    let bare = baseline.rules[0].resolved.as_ref().unwrap();
    assert_eq!(
        bare.declarations
            .iter()
            .map(|declaration| (declaration.property.as_str(), declaration.value.as_str()))
            .collect::<Vec<_>>(),
        [("outline-width", "1px"), ("outline-style", "solid")]
    );
    let sized = baseline.rules[1].resolved.as_ref().unwrap();
    assert_eq!(
        sized
            .declarations
            .iter()
            .map(|declaration| (declaration.property.as_str(), declaration.value.as_str()))
            .collect::<Vec<_>>(),
        [("outline-width", "2px"), ("outline-style", "solid")]
    );
    assert!(baseline.stylesheet.contains(":focus-visible"));
    assert!(
        baseline
            .stylesheet
            .find(".focus-visible\\:outline:focus-visible {")
            .unwrap()
            < baseline
                .stylesheet
                .find(".focus-visible\\:outline-2:focus-visible {")
                .unwrap()
    );

    for order in [[0, 1], [1, 0]] {
        let mut permuted = input.clone();
        permuted.candidates = order.map(|index| input.candidates[index].clone()).to_vec();
        assert_eq!(compile(&permuted), baseline);
    }
}
