use std::collections::BTreeMap;

use lightningcss::{
    properties::{Property, PropertyId},
    stylesheet::{ParserOptions, PrinterOptions},
};

use crate::{
    emit_token_variables, layers, reset_css, RuleMetadata, StylesheetParts, ValidatedWindConfig,
    LAYER_ORDER,
};

pub(crate) fn stylesheet(config: &ValidatedWindConfig, rules: &[RuleMetadata]) -> StylesheetParts {
    let mut parts = StylesheetParts::default();
    let reset = reset_css(config.reset);
    if !reset.is_empty() {
        parts.reset = layers::wrap("@layer zw-reset", reset);
    }
    let variables = emit_token_variables(&config.tokens);
    if !variables.is_empty() {
        let declarations: String = variables
            .iter()
            .map(|variable| {
                format!(
                    "{}: {};\n",
                    variable.name,
                    serialize_token_value(&variable.name, &variable.value)
                )
            })
            .collect();
        parts.tokens = layers::wrap("@layer zw-tokens", &layers::wrap(":root", &declarations));
    }
    let mut registrations = BTreeMap::new();
    for metadata in rules {
        let Some(rule) = &metadata.resolved else {
            continue;
        };
        for registration in &rule.registrations {
            registrations.insert(registration.name, registration);
        }
        // The resolver expands ordered declaration_templates. Preserve all
        // companion/default declarations, including repeated property writes.
        let declarations: String = rule
            .declarations
            .iter()
            .map(|declaration| format!("{}: {};\n", declaration.property, declaration.value))
            .collect();
        let block = layers::wrap(
            metadata.selector.as_deref().expect("resolved selector"),
            &declarations,
        );
        if metadata.conditions.is_empty() {
            parts.utilities.push_str(&block);
        } else {
            parts.utilities.push_str(&layers::wrap(
                &format!("@media {}", metadata.conditions.join(" and ")),
                &block,
            ));
        }
    }
    for registration in registrations.into_values() {
        let contents = format!(
            "syntax: \"{}\";\ninherits: {};\ninitial-value: {};\n",
            registration.syntax, registration.inherits, registration.initial_value
        );
        parts.registrations.push_str(&layers::wrap(
            &format!("@property {}", registration.name),
            &contents,
        ));
    }
    if !parts.reset.is_empty() || !parts.tokens.is_empty() || !parts.utilities.is_empty() {
        parts.prelude = LAYER_ORDER.to_owned();
    }
    parts
}

// Serialize the validated custom-property token stream so configuration values
// containing comments or CRLF whitespace still obey the fixed LF printer.
fn serialize_token_value(name: &str, value: &str) -> String {
    Property::parse_string(PropertyId::from(name), value, ParserOptions::default())
        .expect("validated token value")
        .value_to_css_string(PrinterOptions::default())
        .expect("custom property value serialization")
        // Serialized strings escape line breaks; any remaining ones are
        // comment whitespace and can safely occupy the same output line.
        .replace(['\r', '\n', '\u{c}'], " ")
}
