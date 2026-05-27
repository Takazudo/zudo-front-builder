//! Admonitions preset — the six built-in directive definitions.
//!
//! This module owns the *preset list* only: it returns a `Vec<DirectiveDef>`
//! that registers `note`, `tip`, `warning`, `danger`, `info`, and `details`
//! as container directives. The actual MDX expansion engine
//! (`DirectiveRegistry` / `AdmonitionsPlugin`) stays in `zfb-content`; this
//! crate simply produces the preset configuration.
//!
//! # Usage (in `register_features`)
//!
//! ```rust,ignore
//! use zfb_md_extras::admonitions_preset::default_admonition_directives;
//! use zfb_content::plugins::directives::DirectiveRegistry;
//!
//! let mut registry = DirectiveRegistry::new();
//! for def in default_admonition_directives() {
//!     registry.register(def);
//! }
//! pipeline.add_mdast_visitor(registry.into_visitor());
//! ```
//!
//! Moved from `zfb-content::plugins::admonitions::default_admonition_directives`
//! in Wave 3 (#570). Wire via `features.admonitionsPreset = true` in
//! `zfb.config.ts`.

use zfb_md_ast::{AttrSchema, AttrType, DirectiveDef, DirectiveKind};

/// The six built-in admonition directives.
///
/// Returned in the historical match order (`note`, `tip`, `warning`,
/// `danger`, `info`, `details`) so callers building a registry from
/// these get a deterministic order. All six have `title_from_label = true`
/// so `:::note[Custom Title]` promotes the bracketed label to a
/// `title="…"` attribute — matching Docusaurus/Starlight behaviour.
/// Each one also declares a `title` attr of type `AttrType::String` via
/// `.with_attrs()` so the Wave 4 typed-attr schema is forward-compatible.
#[must_use]
pub fn default_admonition_directives() -> Vec<DirectiveDef> {
    // The `title` AttrSchema is declared on every admonition so the
    // Wave 4 typed-attribute schema is forward-compatible. The value is
    // an optional string (required: false, no default) matching the
    // existing `:::details title="Click me"` legacy form.
    let title_attr = || AttrSchema {
        name: "title".to_string(),
        ty: AttrType::String,
        default: None,
        required: false,
    };

    vec![
        DirectiveDef {
            name: "note".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Note".to_string(),
            title_from_label: true,
            attrs: Vec::new(),
        }
        .with_attrs(vec![title_attr()]),
        DirectiveDef {
            name: "tip".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Tip".to_string(),
            title_from_label: true,
            attrs: Vec::new(),
        }
        .with_attrs(vec![title_attr()]),
        DirectiveDef {
            name: "warning".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Warning".to_string(),
            title_from_label: true,
            attrs: Vec::new(),
        }
        .with_attrs(vec![title_attr()]),
        DirectiveDef {
            name: "danger".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Danger".to_string(),
            title_from_label: true,
            attrs: Vec::new(),
        }
        .with_attrs(vec![title_attr()]),
        DirectiveDef {
            name: "info".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Info".to_string(),
            title_from_label: true,
            attrs: Vec::new(),
        }
        .with_attrs(vec![title_attr()]),
        DirectiveDef {
            name: "details".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Details".to_string(),
            title_from_label: true,
            attrs: Vec::new(),
        }
        .with_attrs(vec![title_attr()]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_six_directives() {
        let defs = default_admonition_directives();
        assert_eq!(defs.len(), 6);
    }

    #[test]
    fn all_are_containers_with_title_from_label() {
        for def in default_admonition_directives() {
            assert_eq!(
                def.kind,
                DirectiveKind::Container,
                "{} must be a Container",
                def.name
            );
            assert!(
                def.title_from_label,
                "{} must have title_from_label=true",
                def.name
            );
        }
    }

    #[test]
    fn all_have_title_attr_schema() {
        for def in default_admonition_directives() {
            assert!(
                def.attrs.iter().any(|a| a.name == "title"),
                "{} must declare a `title` attr in its schema",
                def.name
            );
        }
    }

    #[test]
    fn names_match_expected_set() {
        let defs = default_admonition_directives();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["note", "tip", "warning", "danger", "info", "details"]
        );
    }
}
