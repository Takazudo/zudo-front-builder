use std::collections::{BTreeMap, BTreeSet};

use crate::config::configuration_diagnostic;
use crate::value_check::{validate_value, ValueCategory};
use crate::{Diagnostic, ValueStatus};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TokenCategory {
    Color,
    Spacing,
    Size,
    FontSize,
    FontFamily,
    FontWeight,
    LineHeight,
    LetterSpacing,
    Radius,
    Shadow,
    ZIndex,
    Easing,
}

impl TokenCategory {
    pub const ALL: [Self; 12] = [
        Self::Color,
        Self::Spacing,
        Self::Size,
        Self::FontSize,
        Self::FontFamily,
        Self::FontWeight,
        Self::LineHeight,
        Self::LetterSpacing,
        Self::Radius,
        Self::Shadow,
        Self::ZIndex,
        Self::Easing,
    ];

    pub fn config_name(self) -> &'static str {
        match self {
            Self::Color => "colors",
            Self::Spacing => "spacing",
            Self::Size => "sizes",
            Self::FontSize => "fontSizes",
            Self::FontFamily => "fontFamilies",
            Self::FontWeight => "fontWeights",
            Self::LineHeight => "lineHeights",
            Self::LetterSpacing => "letterSpacings",
            Self::Radius => "radii",
            Self::Shadow => "shadows",
            Self::ZIndex => "zIndices",
            Self::Easing => "easings",
        }
    }

    pub(crate) fn variable_prefix(self) -> &'static str {
        match self {
            Self::Color => "color",
            Self::Spacing => "spacing",
            Self::Size => "size",
            Self::FontSize => "font-size",
            Self::FontFamily => "font-family",
            Self::FontWeight => "font-weight",
            Self::LineHeight => "leading",
            Self::LetterSpacing => "tracking",
            Self::Radius => "radius",
            Self::Shadow => "shadow",
            Self::ZIndex => "z",
            Self::Easing => "ease",
        }
    }

    fn value_category(self) -> ValueCategory {
        match self {
            Self::Color => ValueCategory::Color,
            Self::Spacing => ValueCategory::Spacing,
            Self::Size => ValueCategory::Size,
            Self::FontSize => ValueCategory::FontSize,
            Self::FontFamily => ValueCategory::FontFamily,
            Self::FontWeight => ValueCategory::FontWeight,
            Self::LineHeight => ValueCategory::LineHeight,
            Self::LetterSpacing => ValueCategory::LetterSpacing,
            Self::Radius => ValueCategory::Radius,
            Self::Shadow => ValueCategory::Shadow,
            Self::ZIndex => ValueCategory::ZIndex,
            Self::Easing => ValueCategory::Easing,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FontSizeToken {
    pub size: String,
    pub line_height: Option<String>,
}

/// Plain data supplied by the caller; no token defaults are implied.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TokenConfig {
    pub spacing_unit: Option<String>,
    pub colors: BTreeMap<String, String>,
    pub spacing: BTreeMap<String, String>,
    pub sizes: BTreeMap<String, String>,
    pub font_sizes: BTreeMap<String, FontSizeToken>,
    pub font_families: BTreeMap<String, String>,
    pub font_weights: BTreeMap<String, String>,
    pub line_heights: BTreeMap<String, String>,
    pub letter_spacings: BTreeMap<String, String>,
    pub radii: BTreeMap<String, String>,
    pub shadows: BTreeMap<String, String>,
    pub z_indices: BTreeMap<String, String>,
    pub easings: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedTokens {
    pub(crate) config: TokenConfig,
    names: BTreeMap<TokenCategory, BTreeSet<String>>,
    pub(crate) value_statuses: BTreeMap<String, ValueStatus>,
}

impl ValidatedTokens {
    pub fn names(&self, category: TokenCategory) -> &BTreeSet<String> {
        &self.names[&category]
    }

    pub fn contains(&self, category: TokenCategory, name: &str) -> bool {
        self.names(category).contains(name)
    }
}

impl TokenConfig {
    pub fn validate(&self) -> Result<ValidatedTokens, Vec<Diagnostic>> {
        let mut diagnostics = Vec::new();
        let names = self.all_names();
        validate_collisions(&names, &mut diagnostics);
        validate_name_map(TokenCategory::Color, &self.colors, &mut diagnostics);
        validate_name_map(TokenCategory::Spacing, &self.spacing, &mut diagnostics);
        validate_name_map(TokenCategory::Size, &self.sizes, &mut diagnostics);
        validate_font_size_names(&self.font_sizes, &mut diagnostics);
        validate_name_map(
            TokenCategory::FontFamily,
            &self.font_families,
            &mut diagnostics,
        );
        validate_name_map(
            TokenCategory::FontWeight,
            &self.font_weights,
            &mut diagnostics,
        );
        validate_name_map(
            TokenCategory::LineHeight,
            &self.line_heights,
            &mut diagnostics,
        );
        validate_name_map(
            TokenCategory::LetterSpacing,
            &self.letter_spacings,
            &mut diagnostics,
        );
        validate_name_map(TokenCategory::Radius, &self.radii, &mut diagnostics);
        validate_name_map(TokenCategory::Shadow, &self.shadows, &mut diagnostics);
        validate_name_map(TokenCategory::ZIndex, &self.z_indices, &mut diagnostics);
        validate_name_map(TokenCategory::Easing, &self.easings, &mut diagnostics);

        let mut value_statuses = BTreeMap::new();
        if let Some(spacing_unit) = &self.spacing_unit {
            record_value_status(
                "tokens.spacingUnit",
                ValueCategory::SpacingUnit,
                spacing_unit,
                &mut value_statuses,
                &mut diagnostics,
            );
        }
        validate_value_map(
            TokenCategory::Color,
            &self.colors,
            &mut value_statuses,
            &mut diagnostics,
        );
        validate_value_map(
            TokenCategory::Spacing,
            &self.spacing,
            &mut value_statuses,
            &mut diagnostics,
        );
        validate_value_map(
            TokenCategory::Size,
            &self.sizes,
            &mut value_statuses,
            &mut diagnostics,
        );
        for (name, token) in &self.font_sizes {
            let base_path = format!("tokens.fontSizes.{name}");
            record_value_status(
                &format!("{base_path}.size"),
                ValueCategory::FontSize,
                &token.size,
                &mut value_statuses,
                &mut diagnostics,
            );
            if let Some(line_height) = &token.line_height {
                record_value_status(
                    &format!("{base_path}.lineHeight"),
                    ValueCategory::LineHeight,
                    line_height,
                    &mut value_statuses,
                    &mut diagnostics,
                );
            }
        }
        validate_value_map(
            TokenCategory::FontFamily,
            &self.font_families,
            &mut value_statuses,
            &mut diagnostics,
        );
        validate_value_map(
            TokenCategory::FontWeight,
            &self.font_weights,
            &mut value_statuses,
            &mut diagnostics,
        );
        validate_value_map(
            TokenCategory::LineHeight,
            &self.line_heights,
            &mut value_statuses,
            &mut diagnostics,
        );
        validate_value_map(
            TokenCategory::LetterSpacing,
            &self.letter_spacings,
            &mut value_statuses,
            &mut diagnostics,
        );
        validate_value_map(
            TokenCategory::Radius,
            &self.radii,
            &mut value_statuses,
            &mut diagnostics,
        );
        validate_value_map(
            TokenCategory::Shadow,
            &self.shadows,
            &mut value_statuses,
            &mut diagnostics,
        );
        validate_value_map(
            TokenCategory::ZIndex,
            &self.z_indices,
            &mut value_statuses,
            &mut diagnostics,
        );
        validate_value_map(
            TokenCategory::Easing,
            &self.easings,
            &mut value_statuses,
            &mut diagnostics,
        );

        if diagnostics.is_empty() {
            Ok(ValidatedTokens {
                config: self.clone(),
                names,
                value_statuses,
            })
        } else {
            Err(diagnostics)
        }
    }

    fn all_names(&self) -> BTreeMap<TokenCategory, BTreeSet<String>> {
        BTreeMap::from([
            (TokenCategory::Color, self.colors.keys().cloned().collect()),
            (
                TokenCategory::Spacing,
                self.spacing.keys().cloned().collect(),
            ),
            (TokenCategory::Size, self.sizes.keys().cloned().collect()),
            (
                TokenCategory::FontSize,
                self.font_sizes.keys().cloned().collect(),
            ),
            (
                TokenCategory::FontFamily,
                self.font_families.keys().cloned().collect(),
            ),
            (
                TokenCategory::FontWeight,
                self.font_weights.keys().cloned().collect(),
            ),
            (
                TokenCategory::LineHeight,
                self.line_heights.keys().cloned().collect(),
            ),
            (
                TokenCategory::LetterSpacing,
                self.letter_spacings.keys().cloned().collect(),
            ),
            (TokenCategory::Radius, self.radii.keys().cloned().collect()),
            (
                TokenCategory::Shadow,
                self.shadows.keys().cloned().collect(),
            ),
            (
                TokenCategory::ZIndex,
                self.z_indices.keys().cloned().collect(),
            ),
            (
                TokenCategory::Easing,
                self.easings.keys().cloned().collect(),
            ),
        ])
    }
}

pub(crate) fn is_g13_name(name: &str) -> bool {
    if name.is_empty() || name.starts_with('-') || name.ends_with('-') {
        return false;
    }
    let mut previous_hyphen = false;
    for byte in name.bytes() {
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            previous_hyphen = false;
        } else if byte == b'-' && !previous_hyphen {
            previous_hyphen = true;
        } else {
            return false;
        }
    }
    !previous_hyphen
}

fn validate_name_map(
    category: TokenCategory,
    values: &BTreeMap<String, String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for name in values.keys() {
        validate_name(category, name, diagnostics);
    }
}

fn validate_font_size_names(
    values: &BTreeMap<String, FontSizeToken>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for name in values.keys() {
        validate_name(TokenCategory::FontSize, name, diagnostics);
        if name.ends_with("-leading") {
            diagnostics.push(configuration_diagnostic(
                &format!("tokens.fontSizes.{name}"),
                "font-size names cannot end in -leading because paired variables use that suffix",
                "R18",
            ));
        }
    }
}

fn validate_name(category: TokenCategory, name: &str, diagnostics: &mut Vec<Diagnostic>) {
    let key_path = format!("tokens.{}.{name}", category.config_name());
    if !is_g13_name(name) {
        let mut diagnostic = configuration_diagnostic(
            &key_path,
            "token names must be lowercase ASCII letters and digits separated by single hyphens",
            "R18",
        );
        if name == "DEFAULT" && matches!(category, TokenCategory::Radius | TokenCategory::Shadow) {
            diagnostic.suggested_spelling = Some("default".to_owned());
        }
        diagnostics.push(diagnostic);
        return;
    }
    if name.bytes().all(|byte| byte.is_ascii_digit()) {
        diagnostics.push(configuration_diagnostic(
            &key_path,
            "numeric-only token names are reserved for numeric utility values",
            "R18",
        ));
    }
    if name == "default" && !matches!(category, TokenCategory::Radius | TokenCategory::Shadow) {
        diagnostics.push(configuration_diagnostic(
            &key_path,
            "default is allowed only for radii and shadows",
            "R18",
        ));
    }
    if reserved_static_suffixes(category).contains(&name) {
        diagnostics.push(configuration_diagnostic(
            &key_path,
            "token name collides with a reserved static utility suffix",
            "R18",
        ));
    }
    if category == TokenCategory::Spacing && name == "unit" {
        diagnostics.push(configuration_diagnostic(
            &key_path,
            "spacing.unit would collide with the --zw-spacing-unit variable",
            "R18",
        ));
    }
}

fn reserved_static_suffixes(category: TokenCategory) -> &'static [&'static str] {
    match category {
        TokenCategory::Color => &[
            "center",
            "left",
            "right",
            "justify",
            "start",
            "end",
            "transparent",
            "current",
            "x",
            "y",
            "t",
            "r",
            "b",
            "l",
            "collapse",
            "separate",
            "none",
            "solid",
            "dashed",
            "dotted",
            "double",
        ],
        TokenCategory::Spacing | TokenCategory::Size => &[
            "auto", "full", "min", "max", "fit", "screen", "dvw", "dvh", "none", "px",
        ],
        TokenCategory::FontSize => &["center", "left", "right", "justify", "start", "end"],
        TokenCategory::FontFamily | TokenCategory::FontWeight => &[],
        TokenCategory::LineHeight | TokenCategory::LetterSpacing => &[],
        TokenCategory::Radius => &["none", "full", "t", "r", "b", "l", "tl", "tr", "br", "bl"],
        TokenCategory::Shadow => &["none"],
        TokenCategory::ZIndex => &["auto"],
        TokenCategory::Easing => &[],
    }
}

fn validate_collisions(
    names: &BTreeMap<TokenCategory, BTreeSet<String>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (left, right) in [
        (TokenCategory::Spacing, TokenCategory::Size),
        (TokenCategory::Color, TokenCategory::FontSize),
        (TokenCategory::FontFamily, TokenCategory::FontWeight),
    ] {
        for name in names[&left].intersection(&names[&right]) {
            diagnostics.push(configuration_diagnostic(
                &format!("tokens.{}.{name}", right.config_name()),
                &format!(
                    "token name {name} is ambiguous between {} and {}",
                    left.config_name(),
                    right.config_name()
                ),
                "R18",
            ));
        }
    }
}

fn validate_value_map(
    category: TokenCategory,
    values: &BTreeMap<String, String>,
    statuses: &mut BTreeMap<String, ValueStatus>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (name, value) in values {
        let key_path = format!("tokens.{}.{name}", category.config_name());
        record_value_status(
            &key_path,
            category.value_category(),
            value,
            statuses,
            diagnostics,
        );
    }
}

fn record_value_status(
    key_path: &str,
    category: ValueCategory,
    value: &str,
    statuses: &mut BTreeMap<String, ValueStatus>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match validate_value(category, value) {
        Ok(status) => {
            statuses.insert(key_path.to_owned(), status);
        }
        Err(message) => diagnostics.push(configuration_diagnostic(key_path, &message, "R19")),
    }
}

#[cfg(test)]
mod tests {
    use super::{is_g13_name, FontSizeToken, TokenCategory, TokenConfig};

    #[test]
    fn g13_name_rules_and_reserved_names_report_r18_with_key_paths() {
        let tokens = TokenConfig {
            colors: BTreeMap::from([
                ("Panel".to_owned(), "#fff".to_owned()),
                ("-leading".to_owned(), "#fff".to_owned()),
                ("trailing-".to_owned(), "#fff".to_owned()),
                ("double--hyphen".to_owned(), "#fff".to_owned()),
                ("2".to_owned(), "#fff".to_owned()),
                ("center".to_owned(), "#fff".to_owned()),
                ("default".to_owned(), "#fff".to_owned()),
            ]),
            spacing: BTreeMap::from([
                ("auto".to_owned(), "1rem".to_owned()),
                ("unit".to_owned(), "1rem".to_owned()),
            ]),
            radii: BTreeMap::from([
                ("full".to_owned(), "1rem".to_owned()),
                ("tl".to_owned(), "1rem".to_owned()),
                ("DEFAULT".to_owned(), "1rem".to_owned()),
            ]),
            font_sizes: BTreeMap::from([(
                "small-leading".to_owned(),
                FontSizeToken {
                    size: "1rem".to_owned(),
                    line_height: Some("1.5".to_owned()),
                },
            )]),
            ..TokenConfig::default()
        };
        let diagnostics = tokens.validate().unwrap_err();
        assert!(diagnostics.len() >= 9);
        assert!(diagnostics
            .iter()
            .all(|diagnostic| diagnostic.rejection_id == Some("R18")));
        assert!(diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == crate::DiagnosticCode::Zw007));
        assert!(diagnostics.iter().all(|diagnostic| matches!(
            diagnostic.origin.as_deref(),
            Some(crate::Origin::Config { key_path }) if key_path.starts_with("tokens.")
        )));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.suggested_spelling.as_deref() == Some("default")
                && matches!(
                    diagnostic.origin.as_deref(),
                    Some(crate::Origin::Config { key_path }) if key_path == "tokens.radii.DEFAULT"
                )
        }));
    }

    #[test]
    fn g13_accepts_digit_leading_and_internal_hyphen_names() {
        assert!(is_g13_name("2xl"));
        assert!(is_g13_name("hsp-sm"));
        assert!(!is_g13_name("-sm"));
        assert!(!is_g13_name("sm-"));
        assert!(!is_g13_name("sm--wide"));
        assert!(!is_g13_name("Panel"));
        assert!(!is_g13_name("panel_name"));
    }

    #[test]
    fn all_ambiguous_binding_groups_are_rejected() {
        let tokens = TokenConfig {
            spacing: BTreeMap::from([("narrow".to_owned(), "1rem".to_owned())]),
            sizes: BTreeMap::from([("narrow".to_owned(), "2rem".to_owned())]),
            colors: BTreeMap::from([("label".to_owned(), "#123".to_owned())]),
            font_sizes: BTreeMap::from([(
                "label".to_owned(),
                FontSizeToken {
                    size: "1rem".to_owned(),
                    line_height: None,
                },
            )]),
            font_families: BTreeMap::from([("code".to_owned(), "monospace".to_owned())]),
            font_weights: BTreeMap::from([("code".to_owned(), "600".to_owned())]),
            ..TokenConfig::default()
        };
        let diagnostics = tokens.validate().unwrap_err();
        let collisions = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.rejection_id == Some("R18"))
            .filter(|diagnostic| diagnostic.message.contains("ambiguous"))
            .count();
        assert_eq!(collisions, 3);
    }

    #[test]
    fn valid_tokens_have_deterministic_lookup_and_statuses() {
        let tokens = TokenConfig {
            spacing_unit: Some("0.25rem".to_owned()),
            colors: BTreeMap::from([
                ("panel".to_owned(), "var(--project-panel)".to_owned()),
                ("accent".to_owned(), "#123456".to_owned()),
            ]),
            radii: BTreeMap::from([("default".to_owned(), "0.25rem".to_owned())]),
            shadows: BTreeMap::from([("default".to_owned(), "0 1px 2px #000".to_owned())]),
            ..TokenConfig::default()
        };
        let validated = tokens.validate().expect("configuration is valid");
        assert!(validated.contains(TokenCategory::Color, "panel"));
        assert_eq!(validated.names(TokenCategory::Color).len(), 2);
        assert_eq!(
            validated.value_statuses["tokens.colors.panel"],
            crate::ValueStatus::CategoryUnverified
        );
    }

    use std::collections::BTreeMap;
}
