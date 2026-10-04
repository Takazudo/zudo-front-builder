use std::collections::{BTreeMap, BTreeSet};

use crate::breakpoints::{BreakpointConfig, ValidatedBreakpoints};
use crate::tokens::TokenConfig;
use crate::value_check::{validate_value, ValueCategory};
use crate::{
    parse_candidate, Candidate, Diagnostic, DiagnosticCode, Origin, Severity, ValidatedTokens,
    ValueStatus, VariantVocabulary,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ResetMode {
    #[default]
    None,
    MinimalV1,
    OwnedV1,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DarkModeConfig {
    pub attribute: String,
    pub value: String,
}

/// Crate-level, plain-data configuration for zudo-wind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindConfig {
    pub spec: u32,
    pub reset: ResetMode,
    pub tokens: TokenConfig,
    pub breakpoints: BTreeMap<String, BreakpointConfig>,
    pub dark: Option<DarkModeConfig>,
    /// One raw CSS timing function used by transition utilities. Defaults to `ease`.
    pub default_transition_timing_function: Option<String>,
    pub safelist: BTreeMap<String, Vec<String>>,
    /// Authored class keys map to `true`; false values are invalid configuration.
    pub authored_classes: BTreeMap<String, bool>,
    /// Promotes migration-vocabulary warnings at proven class positions to errors.
    pub strict: bool,
    /// Utility rules after (default) or before authored global CSS.
    pub utility_placement: crate::UtilityPlacement,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedWindConfig {
    pub spec: u32,
    pub reset: ResetMode,
    pub tokens: ValidatedTokens,
    pub breakpoints: ValidatedBreakpoints,
    pub dark: Option<DarkModeConfig>,
    pub default_transition_timing_function: String,
    pub default_transition_timing_function_status: ValueStatus,
    pub safelist: BTreeMap<String, Vec<Candidate>>,
    pub authored_classes: BTreeSet<String>,
    pub vocabulary: VariantVocabulary,
    pub strict: bool,
}

impl Default for WindConfig {
    fn default() -> Self {
        Self {
            spec: crate::SPEC_VERSION,
            reset: ResetMode::None,
            tokens: TokenConfig::default(),
            breakpoints: BTreeMap::new(),
            dark: None,
            default_transition_timing_function: None,
            safelist: BTreeMap::new(),
            authored_classes: BTreeMap::new(),
            strict: false,
            utility_placement: crate::UtilityPlacement::AfterAuthored,
        }
    }
}

impl WindConfig {
    pub fn validate(&self) -> Result<ValidatedWindConfig, Vec<Diagnostic>> {
        // Breakpoints are validated first because the safelist vocabulary depends on them.
        let (breakpoints, mut diagnostics) = ValidatedBreakpoints::validate(&self.breakpoints);

        if self.spec != crate::SPEC_VERSION {
            diagnostics.push(configuration_diagnostic(
                "spec",
                &format!(
                    "unsupported wind spec version {}; expected {}",
                    self.spec,
                    crate::SPEC_VERSION
                ),
                "R19",
            ));
        }
        if let Some(dark) = &self.dark {
            validate_dark(dark, &mut diagnostics);
        }

        let transition_timing = self
            .default_transition_timing_function
            .as_deref()
            .unwrap_or("ease");
        let transition_timing_status =
            match validate_value(ValueCategory::Easing, transition_timing) {
                Ok(status) => Some(status),
                Err(message) => {
                    diagnostics.push(configuration_diagnostic(
                        "defaultTransitionTimingFunction",
                        &message,
                        "R19",
                    ));
                    None
                }
            };

        let tokens = match self.tokens.validate() {
            Ok(tokens) => Some(tokens),
            Err(mut token_diagnostics) => {
                diagnostics.append(&mut token_diagnostics);
                None
            }
        };
        let vocabulary = breakpoints.vocabulary(self.dark.is_some());
        let safelist = validate_safelist(&self.safelist, &vocabulary, &mut diagnostics);
        let authored_classes = validate_authored_classes(&self.authored_classes, &mut diagnostics);

        if !diagnostics.is_empty() {
            sort_diagnostics(&mut diagnostics);
            return Err(diagnostics);
        }

        Ok(ValidatedWindConfig {
            spec: self.spec,
            reset: self.reset,
            tokens: tokens.expect("valid configuration has validated tokens"),
            breakpoints,
            dark: self.dark.clone(),
            default_transition_timing_function: transition_timing.to_owned(),
            default_transition_timing_function_status: transition_timing_status
                .expect("valid configuration has validated transition timing"),
            safelist,
            authored_classes,
            vocabulary,
            strict: self.strict,
        })
    }
}

pub(crate) fn configuration_diagnostic(
    key_path: &str,
    message: &str,
    rejection_id: &'static str,
) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        code: DiagnosticCode::Zw007,
        candidate: None,
        origin: Some(Box::new(Origin::Config {
            key_path: key_path.to_owned(),
        })),
        message: format!("{key_path}: {message}"),
        suggested_spelling: None,
        rejection_id: Some(rejection_id),
    }
}

fn validate_dark(dark: &DarkModeConfig, diagnostics: &mut Vec<Diagnostic>) {
    let attribute_valid = {
        let mut bytes = dark.attribute.bytes();
        matches!(bytes.next(), Some(b'a'..=b'z'))
            && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    };
    if !attribute_valid {
        diagnostics.push(configuration_diagnostic(
            "dark.attribute",
            "dark mode attribute must match [a-z][a-z0-9-]*",
            "R19",
        ));
    }
    if dark.value.is_empty() || dark.value.chars().any(char::is_control) {
        diagnostics.push(configuration_diagnostic(
            "dark.value",
            "dark mode value must be nonempty and contain no control characters",
            "R19",
        ));
    }
}

fn validate_safelist(
    entries: &BTreeMap<String, Vec<String>>,
    vocabulary: &VariantVocabulary,
    diagnostics: &mut Vec<Diagnostic>,
) -> BTreeMap<String, Vec<Candidate>> {
    let mut validated = BTreeMap::new();
    for (owner, candidates) in entries {
        if !valid_owner_id(owner) {
            diagnostics.push(configuration_diagnostic(
                &format!("safelist.{owner}"),
                "safelist owner ids must match [A-Za-z0-9][A-Za-z0-9._/-]*",
                "R19",
            ));
        }
        let mut parsed = Vec::with_capacity(candidates.len());
        for (index, raw) in candidates.iter().enumerate() {
            match parse_candidate(raw, vocabulary) {
                Ok(candidate) => parsed.push(candidate),
                Err(mut diagnostic) => {
                    diagnostic.origin = Some(Box::new(Origin::Safelist {
                        owner: owner.clone(),
                        index,
                    }));
                    diagnostics.push(diagnostic);
                }
            }
        }
        validated.insert(owner.clone(), parsed);
    }
    validated
}

fn validate_authored_classes(
    entries: &BTreeMap<String, bool>,
    diagnostics: &mut Vec<Diagnostic>,
) -> BTreeSet<String> {
    let mut validated = BTreeSet::new();
    for (class_name, enabled) in entries {
        let key_path = format!("authoredClasses.{class_name}");
        if !is_plain_class_name(class_name) {
            diagnostics.push(configuration_diagnostic(
                &key_path,
                "authored class keys must be nonempty complete tokens without whitespace or controls",
                "R19",
            ));
        }
        if !enabled {
            diagnostics.push(configuration_diagnostic(
                &key_path,
                "authored class values must be true",
                "R19",
            ));
        } else if is_plain_class_name(class_name) {
            validated.insert(class_name.clone());
        }
    }
    validated
}

fn is_plain_class_name(value: &str) -> bool {
    !value.is_empty()
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

fn valid_owner_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._/-".contains(byte))
}

fn sort_diagnostics(diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by(|left, right| {
        diagnostic_origin_key(left)
            .cmp(&diagnostic_origin_key(right))
            .then_with(|| diagnostic_code_rank(left.code).cmp(&diagnostic_code_rank(right.code)))
            .then_with(|| {
                left.candidate
                    .as_deref()
                    .unwrap_or("")
                    .as_bytes()
                    .cmp(right.candidate.as_deref().unwrap_or("").as_bytes())
            })
    });
}

fn diagnostic_origin_key(diagnostic: &Diagnostic) -> (u8, String, usize) {
    match diagnostic.origin.as_deref() {
        Some(Origin::Config { key_path }) => (0, key_path.clone(), 0),
        Some(Origin::Safelist { owner, index }) => (1, owner.clone(), *index),
        Some(Origin::Manifest {
            producer,
            path,
            index,
        }) => (2, format!("{producer}/{path}"), *index),
        Some(Origin::Source {
            source_id,
            byte_offset,
            ..
        }) => (3, source_id.clone(), *byte_offset),
        Some(Origin::RoleClass { role_key }) => (4, role_key.clone(), 0),
        Some(Origin::Stylesheet {
            path, byte_offset, ..
        }) => (5, path.clone(), *byte_offset),
        None => (6, String::new(), 0),
    }
}

fn diagnostic_code_rank(code: DiagnosticCode) -> u8 {
    match code {
        DiagnosticCode::Zw001 => 1,
        DiagnosticCode::Zw002 => 2,
        DiagnosticCode::Zw003 => 3,
        DiagnosticCode::Zw004 => 4,
        DiagnosticCode::Zw005 => 5,
        DiagnosticCode::Zw006 => 6,
        DiagnosticCode::Zw007 => 7,
        DiagnosticCode::Zw008 => 8,
        DiagnosticCode::Zw009 => 9,
        DiagnosticCode::Zw010 => 10,
        DiagnosticCode::Zw011 => 11,
        DiagnosticCode::Zw012 => 12,
        DiagnosticCode::Zw013 => 13,
        DiagnosticCode::Zw014 => 14,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::{BreakpointConfig, DarkModeConfig, Origin, TokenConfig, WindConfig};

    #[test]
    fn configured_breakpoint_safelist_entry_validates_and_removal_fails() {
        let config = WindConfig {
            breakpoints: BTreeMap::from([(
                "sm".to_owned(),
                BreakpointConfig { min_width_px: 640 },
            )]),
            safelist: BTreeMap::from([("app".to_owned(), vec!["sm:hover:bg-panel".to_owned()])]),
            ..WindConfig::default()
        };
        assert!(config.validate().is_ok());

        let removed = WindConfig {
            safelist: config.safelist.clone(),
            ..WindConfig::default()
        };
        let diagnostics = removed.validate().unwrap_err();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, crate::DiagnosticCode::Zw002);
        assert_eq!(diagnostics[0].rejection_id, Some("R04"));
        assert!(matches!(
            diagnostics[0].origin.as_deref(),
            Some(Origin::Safelist { owner, index: 0 }) if owner == "app"
        ));
    }

    #[test]
    fn invalid_version_and_dark_settings_report_zw007_with_key_paths() {
        let config = WindConfig {
            spec: 2,
            dark: Some(DarkModeConfig {
                attribute: "Data Theme".to_owned(),
                value: "".to_owned(),
            }),
            ..WindConfig::default()
        };
        let diagnostics = config.validate().unwrap_err();
        assert_eq!(diagnostics.len(), 3);
        assert!(diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == crate::DiagnosticCode::Zw007));
        assert!(diagnostics
            .iter()
            .all(|diagnostic| diagnostic.rejection_id == Some("R19")));
        assert!(diagnostics
            .iter()
            .all(|diagnostic| matches!(diagnostic.origin.as_deref(), Some(Origin::Config { .. }))));
    }

    #[test]
    fn duplicate_breakpoint_width_and_bad_token_value_report_zw007() {
        let config = WindConfig {
            breakpoints: BTreeMap::from([
                ("md".to_owned(), BreakpointConfig { min_width_px: 640 }),
                ("sm".to_owned(), BreakpointConfig { min_width_px: 640 }),
            ]),
            tokens: TokenConfig {
                colors: BTreeMap::from([("panel".to_owned(), "12px".to_owned())]),
                ..TokenConfig::default()
            },
            ..WindConfig::default()
        };
        let diagnostics = config.validate().unwrap_err();
        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == crate::DiagnosticCode::Zw007));
        assert!(diagnostics
            .iter()
            .all(|diagnostic| diagnostic.rejection_id == Some("R19")));
        assert!(diagnostics.iter().all(|diagnostic| matches!(
            diagnostic.origin.as_deref(),
            Some(Origin::Config { key_path }) if key_path.starts_with("breakpoints.") || key_path == "tokens.colors.panel"
        )));
    }

    #[test]
    fn diagnostic_order_is_independent_of_configuration_construction_order() {
        let first = WindConfig {
            breakpoints: BTreeMap::from([
                ("md".to_owned(), BreakpointConfig { min_width_px: 640 }),
                ("sm".to_owned(), BreakpointConfig { min_width_px: 640 }),
            ]),
            tokens: TokenConfig {
                colors: BTreeMap::from([
                    ("panel".to_owned(), "12px".to_owned()),
                    ("bad".to_owned(), "red; blue".to_owned()),
                ]),
                ..TokenConfig::default()
            },
            ..WindConfig::default()
        };
        let second = WindConfig {
            breakpoints: BTreeMap::from([
                ("sm".to_owned(), BreakpointConfig { min_width_px: 640 }),
                ("md".to_owned(), BreakpointConfig { min_width_px: 640 }),
            ]),
            tokens: TokenConfig {
                colors: BTreeMap::from([
                    ("bad".to_owned(), "red; blue".to_owned()),
                    ("panel".to_owned(), "12px".to_owned()),
                ]),
                ..TokenConfig::default()
            },
            ..WindConfig::default()
        };
        assert_eq!(
            first.validate().unwrap_err(),
            second.validate().unwrap_err()
        );
    }

    #[test]
    fn authored_class_allowlist_requires_plain_complete_keys_with_true_values() {
        let config = WindConfig {
            authored_classes: BTreeMap::from([
                ("prose".to_owned(), true),
                ("button:primary".to_owned(), true),
                ("bad class".to_owned(), true),
                ("disabled".to_owned(), false),
            ]),
            ..WindConfig::default()
        };
        let diagnostics = config.validate().unwrap_err();
        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == crate::DiagnosticCode::Zw007));
    }
}
