use crate::{TokenCategory, ValidatedTokens, ValueStatus};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenVariable {
    pub name: String,
    pub value: String,
    pub status: ValueStatus,
}

/// Returns configured custom properties in category-name and token-name byte order.
pub fn emit_token_variables(tokens: &ValidatedTokens) -> Vec<TokenVariable> {
    let mut variables = Vec::new();
    let config = &tokens.config;

    if let Some(value) = &config.spacing_unit {
        variables.push((
            "spacingUnit".to_owned(),
            "unit".to_owned(),
            0_u8,
            make_variable("--zw-spacing-unit", value, "tokens.spacingUnit", tokens),
        ));
    }

    push_map(&mut variables, TokenCategory::Color, &config.colors, tokens);
    push_map(
        &mut variables,
        TokenCategory::Spacing,
        &config.spacing,
        tokens,
    );
    push_map(&mut variables, TokenCategory::Size, &config.sizes, tokens);
    for (name, token) in &config.font_sizes {
        variables.push((
            TokenCategory::FontSize.config_name().to_owned(),
            name.clone(),
            0,
            make_variable(
                &format!("--zw-font-size-{name}"),
                &token.size,
                &format!("tokens.fontSizes.{name}.size"),
                tokens,
            ),
        ));
        if let Some(line_height) = &token.line_height {
            variables.push((
                TokenCategory::FontSize.config_name().to_owned(),
                name.clone(),
                1,
                make_variable(
                    &format!("--zw-font-size-{name}-leading"),
                    line_height,
                    &format!("tokens.fontSizes.{name}.lineHeight"),
                    tokens,
                ),
            ));
        }
    }
    push_map(
        &mut variables,
        TokenCategory::FontFamily,
        &config.font_families,
        tokens,
    );
    push_map(
        &mut variables,
        TokenCategory::FontWeight,
        &config.font_weights,
        tokens,
    );
    push_map(
        &mut variables,
        TokenCategory::LineHeight,
        &config.line_heights,
        tokens,
    );
    push_map(
        &mut variables,
        TokenCategory::LetterSpacing,
        &config.letter_spacings,
        tokens,
    );
    push_map(&mut variables, TokenCategory::Radius, &config.radii, tokens);
    push_map(
        &mut variables,
        TokenCategory::Shadow,
        &config.shadows,
        tokens,
    );
    push_map(
        &mut variables,
        TokenCategory::ZIndex,
        &config.z_indices,
        tokens,
    );
    push_map(
        &mut variables,
        TokenCategory::Easing,
        &config.easings,
        tokens,
    );

    variables.sort_by(|left, right| {
        left.0
            .as_bytes()
            .cmp(right.0.as_bytes())
            .then_with(|| left.1.as_bytes().cmp(right.1.as_bytes()))
            .then_with(|| left.2.cmp(&right.2))
    });
    variables
        .into_iter()
        .map(|(_, _, _, variable)| variable)
        .collect()
}

fn push_map(
    output: &mut Vec<(String, String, u8, TokenVariable)>,
    category: TokenCategory,
    values: &std::collections::BTreeMap<String, String>,
    tokens: &ValidatedTokens,
) {
    for (name, value) in values {
        let key_path = format!("tokens.{}.{name}", category.config_name());
        output.push((
            category.config_name().to_owned(),
            name.clone(),
            0,
            make_variable(
                &format!("--zw-{}-{name}", category.variable_prefix()),
                value,
                &key_path,
                tokens,
            ),
        ));
    }
}

fn make_variable(
    name: &str,
    value: &str,
    key_path: &str,
    tokens: &ValidatedTokens,
) -> TokenVariable {
    TokenVariable {
        name: name.to_owned(),
        value: value.to_owned(),
        status: tokens
            .value_statuses
            .get(key_path)
            .copied()
            .unwrap_or(ValueStatus::Verified),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::WindConfig;
    use crate::{emit_token_variables, FontSizeToken, TokenConfig, ValueStatus};

    #[test]
    fn emits_all_categories_with_spec_variable_names_and_stable_sorting() {
        let tokens = TokenConfig {
            spacing_unit: Some("0.25rem".to_owned()),
            colors: BTreeMap::from([
                ("panel".to_owned(), "var(--project-panel)".to_owned()),
                ("accent".to_owned(), "#123456".to_owned()),
            ]),
            font_sizes: BTreeMap::from([(
                "small".to_owned(),
                FontSizeToken {
                    size: "0.875rem".to_owned(),
                    line_height: Some("1.25rem".to_owned()),
                },
            )]),
            radii: BTreeMap::from([("default".to_owned(), "0.25rem".to_owned())]),
            ..TokenConfig::default()
        };
        let validated = tokens.validate().expect("valid token config");
        let variables = emit_token_variables(&validated);
        assert_eq!(
            variables
                .iter()
                .map(|variable| (variable.name.as_str(), variable.value.as_str()))
                .collect::<Vec<_>>(),
            [
                ("--zw-color-accent", "#123456"),
                ("--zw-color-panel", "var(--project-panel)"),
                ("--zw-font-size-small", "0.875rem"),
                ("--zw-font-size-small-leading", "1.25rem"),
                ("--zw-radius-default", "0.25rem"),
                ("--zw-spacing-unit", "0.25rem"),
            ]
        );
        assert_eq!(variables[1].status, ValueStatus::CategoryUnverified);
    }

    #[test]
    fn insertion_order_does_not_change_emission() {
        let first = TokenConfig {
            colors: BTreeMap::from([
                ("accent".to_owned(), "#123456".to_owned()),
                ("panel".to_owned(), "#ffffff".to_owned()),
            ]),
            ..TokenConfig::default()
        };
        let second = TokenConfig {
            colors: BTreeMap::from([
                ("panel".to_owned(), "#ffffff".to_owned()),
                ("accent".to_owned(), "#123456".to_owned()),
            ]),
            ..TokenConfig::default()
        };
        let first = emit_token_variables(&first.validate().unwrap());
        let second = emit_token_variables(&second.validate().unwrap());
        assert_eq!(first, second);
    }

    #[test]
    fn one_configuration_permutation_has_equal_token_lists_and_diagnostics() {
        let first = WindConfig {
            tokens: TokenConfig {
                colors: BTreeMap::from([
                    ("accent".to_owned(), "#123456".to_owned()),
                    ("panel".to_owned(), "var(--project-panel)".to_owned()),
                ]),
                ..TokenConfig::default()
            },
            authored_classes: BTreeMap::from([("bad class".to_owned(), true)]),
            ..WindConfig::default()
        };
        let second = WindConfig {
            tokens: TokenConfig {
                colors: BTreeMap::from([
                    ("panel".to_owned(), "var(--project-panel)".to_owned()),
                    ("accent".to_owned(), "#123456".to_owned()),
                ]),
                ..TokenConfig::default()
            },
            authored_classes: BTreeMap::from([("bad class".to_owned(), true)]),
            ..WindConfig::default()
        };
        let first_variables = emit_token_variables(&first.tokens.validate().unwrap());
        let second_variables = emit_token_variables(&second.tokens.validate().unwrap());
        let first_diagnostics = first.validate().unwrap_err();
        let second_diagnostics = second.validate().unwrap_err();
        assert_eq!(first_variables, second_variables);
        assert_eq!(first_diagnostics, second_diagnostics);
        assert_eq!(first_diagnostics.len(), 1);
    }
}
