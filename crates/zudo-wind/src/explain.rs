//! Candidate explanations for the CLI and documentation consumers.

use serde::Serialize;

use crate::{
    compile_validated, parse_candidate, Catalog, Diagnostic, DiagnosticCode, Origin,
    OriginCandidate, RuleKind, Severity, SortKey, SourcePositionKind, TokenCategory, TokenOverride,
    ValidatedTokens, ValueStatus, WindConfig, SPEC_REVISION, SPEC_VERSION,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplanationOutcome {
    ResolvedUtility,
    Marker,
    Ordinary,
    /// A migration-vocabulary name: reported, never generated.
    ForeignUtility,
    Invalid,
    GenerationDisabled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Explanation {
    pub candidate: String,
    pub outcome: ExplanationOutcome,
    pub spec_version: u32,
    pub spec_revision: u32,
    pub parsed: Option<ParsedCandidate>,
    pub entry_identifier: Option<String>,
    pub value_status: Option<String>,
    pub token_resolutions: Vec<TokenResolution>,
    pub selector: Option<String>,
    pub conditions: Vec<String>,
    pub declarations: Vec<DeclarationView>,
    pub sort_tuple: Option<SortTuple>,
    pub diagnostics: Vec<DiagnosticView>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedCandidate {
    pub variant_chain: Vec<String>,
    pub utility: String,
    pub negative: bool,
    pub arbitrary_value: Option<String>,
    pub slash_modifier: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenResolution {
    pub token_name: String,
    pub category: String,
    pub variable: String,
    pub configured_value: Option<String>,
    pub category_unverified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_override: Option<HostTokenOverride>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostTokenOverride {
    pub preset_index: usize,
    pub previous_value: serde_json::Value,
    pub final_value: serde_json::Value,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclarationView {
    pub property: String,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SortTuple {
    pub responsive_rank: usize,
    pub dark_rank: u8,
    pub relation_rank: u8,
    pub state_rank: u8,
    pub pseudo_rank: u8,
    pub conflict_group_rank: u16,
    pub scope_rank: u16,
    pub catalog_rank: usize,
    pub raw_candidate: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticView {
    pub severity: String,
    pub code: String,
    pub candidate: Option<String>,
    pub origin: Option<OriginView>,
    pub message: String,
    pub suggestion: Option<String>,
    pub rejection_id: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginView {
    pub kind: String,
    pub source_id: Option<String>,
    pub byte_offset: Option<usize>,
    pub byte_length: Option<usize>,
    pub line: Option<usize>,
    pub byte_column: Option<usize>,
    pub literal_byte_offset: Option<usize>,
    pub literal_byte_length: Option<usize>,
    pub position_kind: Option<String>,
    pub owner: Option<String>,
    pub producer: Option<String>,
    pub path: Option<String>,
    pub index: Option<usize>,
    pub key_path: Option<String>,
    pub role_key: Option<String>,
}

/// Explains one candidate using the supplied configuration. It never fails:
/// invalid candidates and configuration are represented as structured output.
pub fn explain(candidate: &str, config: &WindConfig) -> Explanation {
    explain_with_generation_and_token_overrides(candidate, config, true, &[])
}

/// Explicitly represents the `wind: false` command outcome without validating
/// tokens or parsing candidates, matching the disabled-generation contract.
pub fn explain_disabled(candidate: &str) -> Explanation {
    Explanation {
        candidate: candidate.to_owned(),
        outcome: ExplanationOutcome::GenerationDisabled,
        spec_version: SPEC_VERSION,
        spec_revision: SPEC_REVISION,
        parsed: None,
        entry_identifier: None,
        value_status: None,
        token_resolutions: Vec::new(),
        selector: None,
        conditions: Vec::new(),
        declarations: Vec::new(),
        sort_tuple: None,
        diagnostics: Vec::new(),
    }
}

pub fn explain_with_generation(
    candidate: &str,
    config: &WindConfig,
    generation_enabled: bool,
) -> Explanation {
    explain_with_generation_and_token_overrides(candidate, config, generation_enabled, &[])
}

/// Explains one candidate with merge provenance retained by the host config loader.
pub fn explain_with_generation_and_token_overrides(
    candidate: &str,
    config: &WindConfig,
    generation_enabled: bool,
    token_overrides: &[TokenOverride],
) -> Explanation {
    if !generation_enabled {
        return explain_disabled(candidate);
    }

    let mut output = empty_explanation(candidate, config.spec);
    let validated = match config.validate() {
        Ok(validated) => validated,
        Err(diagnostics) => {
            output.outcome = ExplanationOutcome::Invalid;
            output.diagnostics = diagnostics.iter().map(diagnostic_view).collect();
            return output;
        }
    };
    if validated.authored_classes.contains(candidate) {
        output.outcome = ExplanationOutcome::Ordinary;
        return output;
    }
    let parsed = match parse_candidate(candidate, &validated.vocabulary) {
        Ok(parsed) => parsed,
        Err(mut diagnostic) => {
            diagnostic.origin = Some(Box::new(explain_origin(candidate)));
            output.outcome = ExplanationOutcome::Invalid;
            output.diagnostics.push(diagnostic_view(&diagnostic));
            return output;
        }
    };
    output.parsed = Some(ParsedCandidate {
        variant_chain: parsed
            .variants
            .0
            .iter()
            .map(|variant| variant.raw.clone())
            .collect(),
        utility: parsed.utility.named.clone(),
        negative: parsed.utility.negative,
        arbitrary_value: parsed.utility.arbitrary_value.clone(),
        slash_modifier: parsed.utility.slash_modifier.clone(),
    });

    let origin = explain_origin(candidate);
    let result = compile_validated(
        &[OriginCandidate {
            text: candidate.to_owned(),
            origin: origin.clone(),
        }],
        &validated,
    );
    if let Some(rule) = result.rules.iter().find(|rule| rule.candidate == candidate) {
        if rule.kind == RuleKind::Marker {
            output.outcome = ExplanationOutcome::Marker;
            return output;
        }
        if let Some(resolved) = &rule.resolved {
            output.outcome = ExplanationOutcome::ResolvedUtility;
            output.entry_identifier = Some(resolved.entry_id.clone());
            output.value_status = Some(value_status_name(resolved.value_status).to_owned());
            output.selector = rule.selector.clone();
            output.conditions = rule.conditions.clone();
            output.declarations = resolved
                .declarations
                .iter()
                .map(|declaration| DeclarationView {
                    property: declaration.property.clone(),
                    value: declaration.value.clone(),
                })
                .collect();
            output.sort_tuple = rule.sort_key.as_ref().map(sort_tuple);
            if let Some(entry) = Catalog::v1()
                .entries()
                .iter()
                .find(|entry| entry.id == resolved.entry_id)
            {
                output.token_resolutions = token_resolutions(
                    &parsed,
                    entry,
                    &validated.tokens,
                    &resolved.declarations,
                    token_overrides,
                );
            }
            return output;
        }
    }

    output.diagnostics = result
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.origin.as_deref() == Some(&origin))
        .map(diagnostic_view)
        .collect();
    output.outcome = if output.diagnostics.is_empty() {
        ExplanationOutcome::Ordinary
    } else if result
        .diagnostics
        .iter()
        .all(|diagnostic| diagnostic.code == DiagnosticCode::Zw014)
    {
        ExplanationOutcome::ForeignUtility
    } else {
        ExplanationOutcome::Invalid
    };
    output
}

pub fn render_explanation(explanation: &Explanation) -> String {
    let mut output = String::new();
    output.push_str(&format!("candidate: {}\n", explanation.candidate));
    output.push_str(&format!("outcome: {}\n", outcome_name(explanation.outcome)));
    output.push_str(&format!(
        "spec: {} revision {}\n",
        explanation.spec_version, explanation.spec_revision
    ));
    if let Some(parsed) = &explanation.parsed {
        output.push_str(&format!(
            "variants: {}\nutility: {}\nnegative: {}\n",
            if parsed.variant_chain.is_empty() {
                "(none)".to_owned()
            } else {
                parsed.variant_chain.join(" → ")
            },
            parsed.utility,
            parsed.negative
        ));
        if let Some(value) = &parsed.arbitrary_value {
            output.push_str(&format!("arbitrary value: {value}\n"));
        }
        if let Some(modifier) = &parsed.slash_modifier {
            output.push_str(&format!("slash modifier: {modifier}\n"));
        }
    }
    if let Some(entry) = &explanation.entry_identifier {
        output.push_str(&format!("entry: {entry}\n"));
    }
    if let Some(value_status) = &explanation.value_status {
        output.push_str(&format!("value status: {value_status}\n"));
    }
    for token in &explanation.token_resolutions {
        output.push_str(&format!(
            "token: {} ({}) -> {} = {}{}\n",
            token.token_name,
            token.category,
            token.variable,
            token.configured_value.as_deref().unwrap_or("(unavailable)"),
            if token.category_unverified {
                " [category-unverified]"
            } else {
                ""
            }
        ));
        if let Some(host_override) = &token.host_override {
            output.push_str(&format!(
                "  origin: host override of preset[{}]: {} -> {}\n",
                host_override.preset_index, host_override.previous_value, host_override.final_value
            ));
        }
    }
    if let Some(selector) = &explanation.selector {
        output.push_str(&format!("selector: {selector}\n"));
    }
    if !explanation.conditions.is_empty() {
        output.push_str(&format!(
            "conditions: {}\n",
            explanation.conditions.join(" and ")
        ));
    }
    for declaration in &explanation.declarations {
        output.push_str(&format!(
            "declaration: {}: {}\n",
            declaration.property, declaration.value
        ));
    }
    if let Some(tuple) = &explanation.sort_tuple {
        output.push_str(&format!(
            "sort: responsive={}, dark={}, relation={}, state={}, pseudo={}, conflict-group={}, scope={}, catalog={}, candidate={}\n",
            tuple.responsive_rank,
            tuple.dark_rank,
            tuple.relation_rank,
            tuple.state_rank,
            tuple.pseudo_rank,
            tuple.conflict_group_rank,
            tuple.scope_rank,
            tuple.catalog_rank,
            tuple.raw_candidate
        ));
    }
    for diagnostic in &explanation.diagnostics {
        let location = diagnostic
            .origin
            .as_ref()
            .and_then(|origin| origin.source_id.as_deref().zip(origin.byte_offset))
            .map(|(source, offset)| format!(" at {source}:{offset}"))
            .unwrap_or_default();
        let suggestion = diagnostic
            .suggestion
            .as_deref()
            .map(|spelling| format!("; suggested spelling: {spelling}"))
            .unwrap_or_default();
        output.push_str(&format!(
            "diagnostic: {} {}{}: {}{}\n",
            diagnostic.code, diagnostic.severity, location, diagnostic.message, suggestion
        ));
    }
    output
}

pub fn explanation_json(explanation: &Explanation) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(explanation)
}

pub(crate) fn diagnostic_view(diagnostic: &Diagnostic) -> DiagnosticView {
    DiagnosticView {
        severity: severity_name(diagnostic.severity).to_owned(),
        code: diagnostic_code_name(diagnostic.code).to_owned(),
        candidate: diagnostic.candidate.clone(),
        origin: diagnostic.origin.as_deref().map(origin_view),
        message: diagnostic.message.clone(),
        suggestion: diagnostic.suggested_spelling.clone(),
        rejection_id: diagnostic.rejection_id.map(str::to_owned),
    }
}

pub(crate) fn origin_view(origin: &Origin) -> OriginView {
    let mut view = OriginView::default();
    match origin {
        Origin::Source {
            source_id,
            byte_offset,
            byte_length,
            line,
            byte_column,
            literal_byte_offset,
            literal_byte_length,
            position_kind,
        } => {
            view.kind = "source".to_owned();
            view.source_id = Some(source_id.clone());
            view.byte_offset = Some(*byte_offset);
            view.byte_length = Some(*byte_length);
            view.line = Some(*line);
            view.byte_column = Some(*byte_column);
            view.literal_byte_offset = Some(*literal_byte_offset);
            view.literal_byte_length = Some(*literal_byte_length);
            view.position_kind = Some(
                match position_kind {
                    SourcePositionKind::Class => "class",
                    SourcePositionKind::Literal => "literal",
                }
                .to_owned(),
            );
        }
        Origin::Safelist { owner, index } => {
            view.kind = "safelist".to_owned();
            view.owner = Some(owner.clone());
            view.index = Some(*index);
        }
        Origin::Manifest {
            producer,
            path,
            index,
        } => {
            view.kind = "manifest".to_owned();
            view.producer = Some(producer.clone());
            view.path = Some(path.clone());
            view.index = Some(*index);
        }
        Origin::Config { key_path } => {
            view.kind = "config".to_owned();
            view.key_path = Some(key_path.clone());
        }
        Origin::RoleClass { role_key } => {
            view.kind = "roleClass".to_owned();
            view.role_key = Some(role_key.clone());
        }
        Origin::Stylesheet {
            path,
            byte_offset,
            byte_length,
        } => {
            view.kind = "stylesheet".to_owned();
            view.path = Some(path.clone());
            view.byte_offset = Some(*byte_offset);
            view.byte_length = Some(*byte_length);
        }
    }
    view
}

fn empty_explanation(candidate: &str, spec_version: u32) -> Explanation {
    Explanation {
        candidate: candidate.to_owned(),
        outcome: ExplanationOutcome::Invalid,
        spec_version,
        spec_revision: SPEC_REVISION,
        parsed: None,
        entry_identifier: None,
        value_status: None,
        token_resolutions: Vec::new(),
        selector: None,
        conditions: Vec::new(),
        declarations: Vec::new(),
        sort_tuple: None,
        diagnostics: Vec::new(),
    }
}

fn explain_origin(candidate: &str) -> Origin {
    Origin::Source {
        source_id: "zudo-wind://explain/candidate".to_owned(),
        byte_offset: 0,
        byte_length: candidate.len(),
        line: 1,
        byte_column: 1,
        literal_byte_offset: 0,
        literal_byte_length: candidate.len(),
        position_kind: SourcePositionKind::Class,
    }
}

fn sort_tuple(key: &SortKey) -> SortTuple {
    SortTuple {
        responsive_rank: key.responsive_rank,
        dark_rank: key.dark_rank,
        relation_rank: key.relation_rank,
        state_rank: key.state_rank,
        pseudo_rank: key.pseudo_rank,
        conflict_group_rank: key.conflict_group_rank,
        scope_rank: key.scope_rank,
        catalog_rank: key.catalog_rank,
        raw_candidate: key.raw_candidate.clone(),
    }
}

fn token_resolutions(
    candidate: &crate::Candidate,
    entry: &crate::CatalogEntry,
    tokens: &ValidatedTokens,
    declarations: &[crate::Declaration],
    token_overrides: &[TokenOverride],
) -> Vec<TokenResolution> {
    let mut resolutions = Vec::new();
    let referenced_variables = declarations
        .iter()
        .flat_map(|declaration| variable_references(&declaration.value))
        .collect::<std::collections::BTreeSet<_>>();

    if entry
        .grammar
        .token_categories
        .contains(&TokenCategory::Spacing)
        && tokens.config.spacing_unit.is_some()
        && uses_numeric_spacing(candidate, entry.root.as_str())
        && !is_zero_scale(candidate, entry.root.as_str())
    {
        resolutions.push(TokenResolution {
            token_name: "spacingUnit".to_owned(),
            category: "spacing".to_owned(),
            variable: "--zw-spacing-unit".to_owned(),
            configured_value: tokens.config.spacing_unit.clone(),
            category_unverified: tokens
                .value_statuses
                .get("tokens.spacingUnit")
                .copied()
                .unwrap_or(ValueStatus::Verified)
                == ValueStatus::CategoryUnverified,
            host_override: None,
        });
    }

    for category in &entry.grammar.token_categories {
        for name in token_names(tokens, *category) {
            let variable = format!("--zw-{}-{name}", category.variable_prefix());
            if !referenced_variables.contains(&variable) {
                continue;
            }
            let key = if *category == TokenCategory::FontSize {
                format!("tokens.fontSizes.{name}.size")
            } else {
                format!("tokens.{}.{name}", category.config_name())
            };
            let configured_value = token_value(tokens, *category, &name);
            let unverified = tokens
                .value_statuses
                .get(&key)
                .copied()
                .unwrap_or(ValueStatus::Verified)
                == ValueStatus::CategoryUnverified;
            let host_override = host_token_override(*category, &name, token_overrides);
            resolutions.push(TokenResolution {
                token_name: name,
                category: category.config_name().to_owned(),
                variable,
                configured_value,
                category_unverified: unverified,
                host_override,
            });
        }
    }

    if entry
        .grammar
        .token_categories
        .contains(&TokenCategory::FontSize)
    {
        for (name, token) in &tokens.config.font_sizes {
            let variable = format!("--zw-font-size-{name}-leading");
            if referenced_variables.contains(&variable) {
                let key = format!("tokens.fontSizes.{name}.lineHeight");
                resolutions.push(TokenResolution {
                    token_name: format!("{name}-leading"),
                    category: "fontSizes.leading".to_owned(),
                    variable,
                    configured_value: token.line_height.clone(),
                    category_unverified: tokens
                        .value_statuses
                        .get(&key)
                        .copied()
                        .unwrap_or(ValueStatus::Verified)
                        == ValueStatus::CategoryUnverified,
                    host_override: host_token_override(
                        TokenCategory::FontSize,
                        name,
                        token_overrides,
                    ),
                });
            }
        }
    }
    resolutions.sort_by(|a, b| {
        a.variable
            .cmp(&b.variable)
            .then(a.token_name.cmp(&b.token_name))
    });
    resolutions
}

fn host_token_override(
    category: TokenCategory,
    name: &str,
    token_overrides: &[TokenOverride],
) -> Option<HostTokenOverride> {
    token_overrides
        .iter()
        .find(|over| over.category == category && over.name == name)
        .map(|over| HostTokenOverride {
            preset_index: over.preset_index,
            previous_value: over.previous_value.clone(),
            final_value: over.final_value.clone(),
        })
}

fn token_names(tokens: &ValidatedTokens, category: TokenCategory) -> Vec<String> {
    let mut names = tokens.names(category).iter().cloned().collect::<Vec<_>>();
    if matches!(category, TokenCategory::Radius | TokenCategory::Shadow)
        && (category == TokenCategory::Radius && tokens.config.radii.contains_key("default")
            || category == TokenCategory::Shadow && tokens.config.shadows.contains_key("default"))
    {
        names.push("default".to_owned());
    }
    names.sort();
    names.dedup();
    names
}

fn token_value(tokens: &ValidatedTokens, category: TokenCategory, name: &str) -> Option<String> {
    let map = match category {
        TokenCategory::Color => &tokens.config.colors,
        TokenCategory::Spacing => &tokens.config.spacing,
        TokenCategory::Size => &tokens.config.sizes,
        TokenCategory::FontFamily => &tokens.config.font_families,
        TokenCategory::FontWeight => &tokens.config.font_weights,
        TokenCategory::LineHeight => &tokens.config.line_heights,
        TokenCategory::LetterSpacing => &tokens.config.letter_spacings,
        TokenCategory::Radius => &tokens.config.radii,
        TokenCategory::Shadow => &tokens.config.shadows,
        TokenCategory::ZIndex => &tokens.config.z_indices,
        TokenCategory::Easing => &tokens.config.easings,
        TokenCategory::FontSize => {
            return tokens
                .config
                .font_sizes
                .get(name)
                .map(|value| value.size.clone())
        }
    };
    map.get(name).cloned()
}

fn variable_references(value: &str) -> Vec<String> {
    let bytes = value.as_bytes();
    let mut found = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = value[cursor..].find("var(") {
        let start = cursor + relative + 4;
        let Some(end_relative) = value[start..].find(')') else {
            break;
        };
        let end = start + end_relative;
        let name = value[start..end].trim();
        if name.starts_with("--zw-") {
            found.push(name.to_owned());
        }
        cursor = end + 1;
        if cursor >= bytes.len() {
            break;
        }
    }
    found
}

fn uses_numeric_spacing(candidate: &crate::Candidate, root: &str) -> bool {
    candidate
        .utility
        .named
        .strip_prefix(&format!("{root}-"))
        .is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'.')
        })
}

fn is_zero_scale(candidate: &crate::Candidate, root: &str) -> bool {
    candidate
        .utility
        .named
        .strip_prefix(&format!("{root}-"))
        .is_some_and(|suffix| {
            suffix
                .bytes()
                .filter(|byte| *byte != b'.')
                .all(|byte| byte == b'0')
        })
}

fn value_status_name(status: ValueStatus) -> &'static str {
    match status {
        ValueStatus::Verified => "verified",
        ValueStatus::CategoryUnverified => "categoryUnverified",
    }
}

fn outcome_name(outcome: ExplanationOutcome) -> &'static str {
    match outcome {
        ExplanationOutcome::ResolvedUtility => "resolved utility",
        ExplanationOutcome::Marker => "marker",
        ExplanationOutcome::Ordinary => "ordinary class",
        ExplanationOutcome::ForeignUtility => "unsupported foreign utility",
        ExplanationOutcome::Invalid => "recognized invalid or malformed",
        ExplanationOutcome::GenerationDisabled => "generation disabled",
    }
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::AuditInfo => "auditInfo",
    }
}

fn diagnostic_code_name(code: DiagnosticCode) -> &'static str {
    match code {
        DiagnosticCode::Zw001 => "ZW001",
        DiagnosticCode::Zw002 => "ZW002",
        DiagnosticCode::Zw003 => "ZW003",
        DiagnosticCode::Zw004 => "ZW004",
        DiagnosticCode::Zw005 => "ZW005",
        DiagnosticCode::Zw006 => "ZW006",
        DiagnosticCode::Zw007 => "ZW007",
        DiagnosticCode::Zw008 => "ZW008",
        DiagnosticCode::Zw009 => "ZW009",
        DiagnosticCode::Zw010 => "ZW010",
        DiagnosticCode::Zw011 => "ZW011",
        DiagnosticCode::Zw012 => "ZW012",
        DiagnosticCode::Zw013 => "ZW013",
        DiagnosticCode::Zw014 => "ZW014",
        DiagnosticCode::Zw015 => "ZW015",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::{BreakpointConfig, DarkModeConfig, FontSizeToken, TokenConfig};

    use super::*;

    #[test]
    fn diagnostic_code_name_registry_is_unique() {
        let codes = [
            DiagnosticCode::Zw001,
            DiagnosticCode::Zw002,
            DiagnosticCode::Zw003,
            DiagnosticCode::Zw004,
            DiagnosticCode::Zw005,
            DiagnosticCode::Zw006,
            DiagnosticCode::Zw007,
            DiagnosticCode::Zw008,
            DiagnosticCode::Zw009,
            DiagnosticCode::Zw010,
            DiagnosticCode::Zw011,
            DiagnosticCode::Zw012,
            DiagnosticCode::Zw013,
            DiagnosticCode::Zw014,
            DiagnosticCode::Zw015,
        ];
        let names = codes.map(diagnostic_code_name);
        let unique = names
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), names.len());
        assert_eq!(names[14], "ZW015");
    }

    fn configured() -> WindConfig {
        WindConfig {
            tokens: TokenConfig {
                colors: BTreeMap::from([("panel".to_owned(), "var(--project-panel)".to_owned())]),
                font_sizes: BTreeMap::from([(
                    "small".to_owned(),
                    FontSizeToken {
                        size: "0.875rem".to_owned(),
                        line_height: Some("1.25rem".to_owned()),
                    },
                )]),
                spacing_unit: Some("0.25rem".to_owned()),
                ..TokenConfig::default()
            },
            breakpoints: BTreeMap::from([(
                "sm".to_owned(),
                BreakpointConfig { min_width_px: 640 },
            )]),
            dark: Some(DarkModeConfig {
                attribute: "data-theme".to_owned(),
                value: "dark".to_owned(),
            }),
            ..WindConfig::default()
        }
    }

    #[test]
    fn stacked_variant_explanation_exposes_tokens_selector_conditions_and_sort_tuple() {
        let explanation = explain("sm:dark:hover:bg-panel", &configured());
        assert_eq!(explanation.outcome, ExplanationOutcome::ResolvedUtility);
        assert_eq!(
            explanation.entry_identifier.as_deref(),
            Some("v1.background")
        );
        assert_eq!(
            explanation.parsed.as_ref().unwrap().variant_chain,
            ["sm", "dark", "hover"]
        );
        assert_eq!(
            explanation.token_resolutions,
            [TokenResolution {
                token_name: "panel".to_owned(),
                category: "colors".to_owned(),
                variable: "--zw-color-panel".to_owned(),
                configured_value: Some("var(--project-panel)".to_owned()),
                category_unverified: true,
                host_override: None,
            }]
        );
        assert_eq!(
            explanation.selector.as_deref(),
            Some(
                r#".sm\:dark\:hover\:bg-panel:where([data-theme="dark"], [data-theme="dark"] *):hover"#
            )
        );
        assert_eq!(
            explanation.conditions,
            ["(min-width: 640px)", "(hover: hover)"]
        );
        assert_eq!(explanation.declarations[0].property, "background-color");
        assert!(explanation.sort_tuple.is_some());
        assert_eq!(
            explain("sm:dark:hover:bg-panel", &configured()),
            explanation
        );
    }

    #[test]
    fn explanation_reports_invalid_ordinary_marker_and_disabled_outcomes() {
        assert_eq!(
            explain("site-header", &configured()).outcome,
            ExplanationOutcome::Ordinary
        );
        assert_eq!(
            explain("p-missing", &configured()).outcome,
            ExplanationOutcome::Invalid
        );
        assert_eq!(
            explain("group", &configured()).outcome,
            ExplanationOutcome::Marker
        );
        assert_eq!(
            explain_with_generation("broken candidate", &configured(), false).outcome,
            ExplanationOutcome::GenerationDisabled
        );
        assert!(explain_disabled("anything").diagnostics.is_empty());
    }

    #[test]
    fn explanation_snapshots_host_override_origin_in_text_and_json() {
        let override_entry = TokenOverride {
            category: TokenCategory::Color,
            name: "panel".to_owned(),
            preset_index: 1,
            previous_value: serde_json::json!("var(--preset-panel)"),
            final_value: serde_json::json!("var(--project-panel)"),
        };
        let explanation = explain_with_generation_and_token_overrides(
            "bg-panel",
            &configured(),
            true,
            &[override_entry],
        );

        assert_eq!(
            explanation.token_resolutions[0].host_override,
            Some(HostTokenOverride {
                preset_index: 1,
                previous_value: serde_json::json!("var(--preset-panel)"),
                final_value: serde_json::json!("var(--project-panel)"),
            })
        );
        let rendered = render_explanation(&explanation);
        assert!(rendered.contains(
            "  origin: host override of preset[1]: \"var(--preset-panel)\" -> \"var(--project-panel)\"\n"
        ), "{rendered}");

        let json = serde_json::to_value(&explanation).unwrap();
        assert_eq!(
            json["tokenResolutions"][0]["hostOverride"],
            serde_json::json!({
                "presetIndex": 1,
                "previousValue": "var(--preset-panel)",
                "finalValue": "var(--project-panel)"
            })
        );
        assert!(json.get("sourcePath").is_none());

        let without_metadata = explain("bg-panel", &configured());
        let without_metadata_json = serde_json::to_value(without_metadata).unwrap();
        assert!(without_metadata_json["tokenResolutions"][0]
            .get("hostOverride")
            .is_none());
    }

    #[test]
    fn explanation_resolves_spacing_unit_and_paired_font_size_tokens() {
        let spacing = explain("p-4", &configured());
        assert_eq!(spacing.value_status.as_deref(), Some("verified"));
        assert_eq!(
            spacing.token_resolutions,
            [TokenResolution {
                token_name: "spacingUnit".to_owned(),
                category: "spacing".to_owned(),
                variable: "--zw-spacing-unit".to_owned(),
                configured_value: Some("0.25rem".to_owned()),
                category_unverified: false,
                host_override: None,
            }]
        );

        let font_size = explain("text-small", &configured());
        assert_eq!(font_size.token_resolutions.len(), 2);
        assert!(font_size.token_resolutions.iter().any(|token| {
            token.token_name == "small"
                && token.variable == "--zw-font-size-small"
                && token.configured_value.as_deref() == Some("0.875rem")
        }));
        assert!(font_size.token_resolutions.iter().any(|token| {
            token.token_name == "small-leading"
                && token.variable == "--zw-font-size-small-leading"
                && token.configured_value.as_deref() == Some("1.25rem")
        }));
    }

    #[test]
    fn target_explanation_matches_compiled_metadata_across_candidate_arrival_orders() {
        let config = configured();
        let expected = explain("sm:dark:hover:bg-panel", &config);
        let orders = [
            ["sm:dark:hover:bg-panel", "p-4", "block"],
            ["block", "p-4", "sm:dark:hover:bg-panel"],
            ["p-4", "sm:dark:hover:bg-panel", "block"],
        ];
        for order in orders {
            let candidates = order
                .iter()
                .map(|candidate| {
                    let offset = match *candidate {
                        "sm:dark:hover:bg-panel" => 0,
                        "p-4" => 24,
                        _ => 40,
                    };
                    crate::OriginCandidate {
                        text: (*candidate).to_owned(),
                        origin: Origin::Source {
                            source_id: "src/example.tsx".to_owned(),
                            byte_offset: offset,
                            byte_length: candidate.len(),
                            line: 1,
                            byte_column: offset + 1,
                            literal_byte_offset: 0,
                            literal_byte_length: 48,
                            position_kind: SourcePositionKind::Class,
                        },
                    }
                })
                .collect();
            let compiled = crate::compile(&crate::CompileInput {
                candidates,
                config: config.clone(),
            });
            let target = compiled
                .rules
                .iter()
                .find(|rule| rule.candidate == "sm:dark:hover:bg-panel")
                .expect("target was compiled");
            assert_eq!(expected.selector, target.selector);
            assert_eq!(expected.conditions, target.conditions);
            assert_eq!(
                expected.entry_identifier,
                target.resolved.as_ref().map(|r| r.entry_id.clone())
            );
            assert_eq!(
                expected.sort_tuple,
                target.sort_key.as_ref().map(sort_tuple)
            );
            assert_eq!(expected, explain("sm:dark:hover:bg-panel", &config));
        }
    }

    #[test]
    fn plain_and_json_explanation_renderings_are_repeatable() {
        let explanation = explain("sm:dark:hover:bg-panel", &configured());
        assert_eq!(
            render_explanation(&explanation),
            render_explanation(&explanation)
        );
        assert_eq!(
            explanation_json(&explanation).unwrap(),
            explanation_json(&explanation).unwrap()
        );
    }
}
