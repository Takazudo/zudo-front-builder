use crate::config::configuration_diagnostic;
use crate::variant::is_state;
use crate::{Diagnostic, VariantVocabulary};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BreakpointConfig {
    pub min_width_px: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RankedBreakpoint {
    pub name: String,
    pub min_width_px: i64,
    pub rank: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ValidatedBreakpoints {
    ranked: Vec<RankedBreakpoint>,
}

impl ValidatedBreakpoints {
    pub fn ranked(&self) -> &[RankedBreakpoint] {
        &self.ranked
    }

    pub fn vocabulary(&self, dark: bool) -> VariantVocabulary {
        VariantVocabulary::new(
            self.ranked.iter().map(|breakpoint| breakpoint.name.clone()),
            dark,
        )
    }

    pub(crate) fn validate(
        configured: &std::collections::BTreeMap<String, BreakpointConfig>,
    ) -> (Self, Vec<Diagnostic>) {
        let mut diagnostics = Vec::new();
        let mut eligible = Vec::new();
        for (name, config) in configured {
            let key_path = format!("breakpoints.{name}");
            if !crate::tokens::is_g13_name(name) {
                diagnostics.push(configuration_diagnostic(
                    &key_path,
                    "breakpoint names must use the configured token-name grammar",
                    "R19",
                ));
                continue;
            }
            if is_reserved_variant_name(name) {
                diagnostics.push(configuration_diagnostic(
                    &key_path,
                    "breakpoint name is reserved for a fixed variant",
                    "R19",
                ));
                continue;
            }
            if config.min_width_px <= 0 {
                diagnostics.push(configuration_diagnostic(
                    &format!("{key_path}.minWidthPx"),
                    "breakpoint width must be a positive integer in pixels",
                    "R19",
                ));
                continue;
            }
            eligible.push((name.clone(), config.min_width_px));
        }
        eligible.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));

        let mut unique = Vec::with_capacity(eligible.len());
        let mut previous_width = None;
        for (name, min_width_px) in eligible {
            if previous_width == Some(min_width_px) {
                diagnostics.push(configuration_diagnostic(
                    &format!("breakpoints.{name}.minWidthPx"),
                    "another breakpoint already uses this width",
                    "R19",
                ));
                continue;
            }
            previous_width = Some(min_width_px);
            unique.push((name, min_width_px));
        }

        let ranked = unique
            .into_iter()
            .enumerate()
            .map(|(rank, (name, min_width_px))| RankedBreakpoint {
                name,
                min_width_px,
                rank,
            })
            .collect();
        (Self { ranked }, diagnostics)
    }
}

fn is_reserved_variant_name(name: &str) -> bool {
    name == "dark"
        || is_state(name)
        || ["before", "after", "marker", "placeholder", "backdrop"].contains(&name)
        || name.starts_with("max-")
        || name.starts_with("group-")
        || name.starts_with("peer-")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{BreakpointConfig, ValidatedBreakpoints};

    fn config(width: i64) -> BreakpointConfig {
        BreakpointConfig {
            min_width_px: width,
        }
    }

    #[test]
    fn breakpoints_validate_and_rank_by_positive_pixel_width() {
        let configured = BTreeMap::from([
            ("lg".to_owned(), config(1280)),
            ("sm".to_owned(), config(640)),
            ("md".to_owned(), config(1024)),
        ]);
        let (validated, diagnostics) = ValidatedBreakpoints::validate(&configured);
        assert!(diagnostics.is_empty());
        assert_eq!(
            validated
                .ranked()
                .iter()
                .map(|entry| (entry.name.as_str(), entry.min_width_px, entry.rank))
                .collect::<Vec<_>>(),
            [("sm", 640, 0), ("md", 1024, 1), ("lg", 1280, 2)]
        );
    }

    #[test]
    fn duplicate_widths_and_nonpositive_widths_are_configuration_errors() {
        let configured = BTreeMap::from([
            ("lg".to_owned(), config(640)),
            ("md".to_owned(), config(640)),
            ("zero".to_owned(), config(0)),
            ("negative".to_owned(), config(-1)),
        ]);
        let (validated, diagnostics) = ValidatedBreakpoints::validate(&configured);
        assert_eq!(validated.ranked().len(), 1);
        assert_eq!(diagnostics.len(), 3);
        assert!(diagnostics
            .iter()
            .all(|diagnostic| diagnostic.rejection_id == Some("R19")));
    }

    #[test]
    fn names_must_be_valid_and_cannot_shadow_variants() {
        let configured = BTreeMap::from([
            ("hover".to_owned(), config(320)),
            ("max-small".to_owned(), config(480)),
            ("Bad".to_owned(), config(640)),
            ("small".to_owned(), config(800)),
        ]);
        let (validated, diagnostics) = ValidatedBreakpoints::validate(&configured);
        assert_eq!(validated.ranked().len(), 1);
        assert_eq!(validated.ranked()[0].name, "small");
        assert_eq!(diagnostics.len(), 3);
    }

    #[test]
    fn vocabulary_contains_ranked_configured_names_and_dark_flag() {
        let configured = BTreeMap::from([("sm".to_owned(), config(640))]);
        let (validated, _) = ValidatedBreakpoints::validate(&configured);
        let vocabulary = validated.vocabulary(true);
        assert!(vocabulary.breakpoints.contains("sm"));
        assert!(vocabulary.dark);
    }
}
