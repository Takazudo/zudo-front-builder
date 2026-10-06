use std::collections::BTreeSet;

use super::{
    arbitrary, Catalog, CatalogEntry, EmissionValue, Registration, SelectorShape, ValueKind,
};
use crate::{
    Candidate, Decimal, DecimalDimension, Diagnostic, DiagnosticCode, Origin, Severity,
    SourcePositionKind, ValidatedTokens, ValueStatus, VariantKind,
};

const NEW_EXACT_STATIC_ROOTS: &[&str] = &[
    "contents",
    "flow-root",
    "list-item",
    "table",
    "inline-table",
    "table-caption",
    "table-cell",
    "table-column",
    "table-column-group",
    "table-footer-group",
    "table-header-group",
    "table-row",
    "table-row-group",
    "appearance-auto",
    "appearance-none",
];

fn is_new_exact_static_root(root: &str) -> bool {
    NEW_EXACT_STATIC_ROOTS.contains(&root)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Declaration {
    pub property: String,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedRule {
    pub entry_id: String,
    pub root: String,
    pub declarations: Vec<Declaration>,
    pub selector_shape: SelectorShape,
    pub conflict_group: &'static str,
    pub conflict_group_rank: u16,
    pub order_rank: u16,
    pub value_status: ValueStatus,
    pub registrations: Vec<Registration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Resolution {
    Rule(ResolvedRule),
    Diagnostic(Diagnostic),
    NotUtility,
    Failure(Diagnostic),
}

impl Catalog {
    /// Resolution order is exact static, then named token, numeric scale,
    /// fraction, and finally bracket value. Roots use longest matching spelling.
    pub fn resolve(
        &self,
        candidate: &Candidate,
        tokens: &ValidatedTokens,
        origin: &Origin,
        authored_classes: &BTreeSet<String>,
    ) -> Resolution {
        let mut result = self.resolve_once(candidate, tokens, origin, authored_classes);
        if let Resolution::Diagnostic(diagnostic) | Resolution::Failure(diagnostic) = &mut result {
            if diagnostic.code == DiagnosticCode::Zw005 && diagnostic.suggested_spelling.is_none() {
                if let Some(spelling) =
                    self.calc_spacing_suggestion(candidate, tokens, origin, authored_classes)
                {
                    diagnostic
                        .message
                        .push_str("; calc() needs spaces around + and -, written as underscores");
                    diagnostic.suggested_spelling = Some(spelling);
                }
            }
        }
        result
    }

    /// The candidate with calc() operators spaced, when that spelling alone
    /// resolves. Tailwind inserted these spaces itself.
    fn calc_spacing_suggestion(
        &self,
        candidate: &Candidate,
        tokens: &ValidatedTokens,
        origin: &Origin,
        authored_classes: &BTreeSet<String>,
    ) -> Option<String> {
        let value = candidate.utility.arbitrary_value.as_deref()?;
        let repaired = arbitrary::space_calc_operators(value)?;
        let bracket = format!("[{value}]");
        if candidate.raw.matches(&bracket).count() != 1 {
            return None;
        }
        let mut retry = candidate.clone();
        retry.raw = candidate.raw.replace(&bracket, &format!("[{repaired}]"));
        retry.utility.named = candidate
            .utility
            .named
            .replace(&bracket, &format!("[{repaired}]"));
        retry.utility.arbitrary_value = Some(repaired);
        matches!(
            self.resolve_once(&retry, tokens, origin, authored_classes),
            Resolution::Rule(_)
        )
        .then_some(retry.raw)
    }

    fn resolve_once(
        &self,
        candidate: &Candidate,
        tokens: &ValidatedTokens,
        origin: &Origin,
        authored_classes: &BTreeSet<String>,
    ) -> Resolution {
        if authored_classes.contains(&candidate.raw) {
            return Resolution::NotUtility;
        }
        if candidate.raw == "group" || candidate.raw == "peer" {
            return Resolution::NotUtility;
        }
        let name = &candidate.utility.named;
        if let Some((head, rest)) = name
            .split_once('_')
            .filter(|_| candidate.utility.arbitrary_value.is_none())
        {
            // A single `_` after a root reads as a mistyped `-` (`p_4`); a BEM
            // `__` element separator is authored naming even after `block`.
            return if !rest.starts_with('_') && self.recognizes_root(head) {
                invalid(
                    candidate,
                    origin,
                    DiagnosticCode::Zw001,
                    &format!(
                        "invalid named utility characters: `_` follows the utility root in {}. If this is a genuine authored class, reserve its complete name with wind.authoredClasses: {{ {}: true }}",
                        candidate.raw,
                        serde_json::to_string(&candidate.raw).expect("candidate is a string")
                    ),
                    Some("R03"),
                )
            } else {
                unknown_root(candidate, origin)
            };
        }
        if matches!(name.as_str(), "ring" | "animate" | "scale" | "transform") {
            return invalid(
                candidate,
                origin,
                DiagnosticCode::Zw004,
                "recognized utility family is unsupported",
                Some("R20"),
            );
        }
        let matching: Vec<_> = self
            .entries
            .iter()
            .filter_map(|entry| {
                if name == &entry.root {
                    Some((entry, ""))
                } else {
                    name.strip_prefix(&format!("{}-", entry.root))
                        .map(|suffix| (entry, suffix))
                }
            })
            // A static decoration style claims only its exact spelling. Its
            // prefix still belongs to the color token lookup and diagnostics.
            .filter(|(entry, suffix)| {
                (!entry.id.starts_with("v1.decoration.style.") || suffix.is_empty())
                    && (!is_new_exact_static_root(&entry.root) || suffix.is_empty())
            })
            .collect();
        // Exact style roots were added after color tokens. Keep configured
        // colors such as `wavy` and `solid-brand` on their old meaning.
        let configured_decoration_color = name
            .strip_prefix("decoration-")
            .is_some_and(|suffix| tokens.contains(crate::TokenCategory::Color, suffix));
        let matching: Vec<_> = if configured_decoration_color {
            matching
                .into_iter()
                .filter(|(entry, _)| entry.id == "v1.decoration.color")
                .collect()
        } else {
            matching
        };
        let Some(longest) = matching.iter().map(|(entry, _)| entry.root.len()).max() else {
            return match super::migration::foreign_family(candidate, tokens) {
                Some(family) => foreign(candidate, origin, family),
                None => unknown_root(candidate, origin),
            };
        };
        let mut matching: Vec<_> = matching
            .into_iter()
            .filter(|(entry, _)| entry.root.len() == longest)
            .collect();
        matching.sort_by_key(|(entry, suffix)| match_priority(entry, suffix, candidate, tokens));
        let priority = match_priority(matching[0].0, matching[0].1, candidate, tokens);
        let leading: Vec<_> = matching
            .iter()
            .take_while(|(entry, suffix)| {
                match_priority(entry, suffix, candidate, tokens) == priority
            })
            .copied()
            .collect();
        let successful: Vec<_> = leading
            .iter()
            .filter(|(entry, suffix)| entry_value(entry, suffix, candidate, tokens).is_ok())
            .copied()
            .collect();
        if successful.is_empty() {
            // `shadow` is a catalog root, so the foreign-family vocabulary
            // cannot explain this colors-only suffix.
            let valid_shadow_color_opacity = candidate
                .utility
                .slash_modifier
                .as_deref()
                .is_none_or(|modifier| modifier.parse::<u8>().is_ok_and(|opacity| opacity <= 100));
            if !candidate.utility.negative
                && candidate.utility.arbitrary_value.is_none()
                && valid_shadow_color_opacity
            {
                if let Some((_, suffix)) = leading.iter().copied().find(|(entry, suffix)| {
                    entry.root == "shadow"
                        && !suffix.is_empty()
                        && tokens.contains(crate::TokenCategory::Color, suffix)
                        && !tokens.contains(crate::TokenCategory::Shadow, suffix)
                }) {
                    return invalid(
                        candidate,
                        origin,
                        DiagnosticCode::Zw004,
                        &shadow_color_hint(candidate, suffix),
                        Some("R20"),
                    );
                }
            }
            // A vocabulary name can share a catalog root's prefix
            // (`inline-table` under `inline`); after a catalog value fails, it
            // is foreign, not a bad value. Successful configured tokens above
            // keep their catalog meaning.
            if let Some(family) = super::migration::foreign_family(candidate, tokens) {
                return foreign(candidate, origin, family);
            }
        }
        if successful.len() > 1 {
            return invalid(
                candidate,
                origin,
                DiagnosticCode::Zw005,
                "value is ambiguous across catalog entries",
                Some("R15"),
            );
        }
        if successful.is_empty() && leading.len() > 1 {
            if let Some(message) = attempted_categories_message(&leading, candidate, tokens) {
                return invalid(
                    candidate,
                    origin,
                    DiagnosticCode::Zw005,
                    &message,
                    Some("R15"),
                );
            }
        }
        let (entry, suffix) = successful.first().copied().unwrap_or_else(|| {
            leading
                .iter()
                .copied()
                .find(|(entry, _)| {
                    (!candidate.utility.negative || entry.negative)
                        && (candidate.utility.slash_modifier.is_none()
                            || entry.grammar.allows_fraction_slash
                            || entry.grammar.allows_color_opacity)
                })
                .unwrap_or(leading[0])
        });
        if entry.selector_shape == SelectorShape::LaterVisibleSiblings
            && candidate
                .variants
                .0
                .iter()
                .any(|variant| matches!(variant.kind, VariantKind::PseudoElement(_)))
        {
            return invalid(
                candidate,
                origin,
                DiagnosticCode::Zw005,
                "child utilities cannot target pseudo-elements",
                Some("R23"),
            );
        }
        let utility = &candidate.utility;
        if utility.negative && !entry.negative {
            return if is_new_exact_static_root(&entry.root) {
                adopted_static_modifier_error(
                    candidate,
                    origin,
                    &entry.root,
                    "negative values are not supported",
                    "R12",
                )
            } else {
                invalid(
                    candidate,
                    origin,
                    DiagnosticCode::Zw005,
                    "negative value is not supported",
                    Some("R12"),
                )
            };
        }
        if utility.slash_modifier.is_some()
            && !entry.grammar.allows_fraction_slash
            && !entry.grammar.allows_color_opacity
        {
            return if is_new_exact_static_root(&entry.root) {
                adopted_static_modifier_error(
                    candidate,
                    origin,
                    &entry.root,
                    "slash modifiers are not supported",
                    "R14",
                )
            } else {
                invalid(
                    candidate,
                    origin,
                    DiagnosticCode::Zw005,
                    "slash modifier is not supported",
                    Some("R14"),
                )
            };
        }
        let (mut value, mut status) = match entry_value(entry, suffix, candidate, tokens) {
            Ok(value) => value,
            Err((code, message, rejection_id)) => {
                let mut result = invalid(candidate, origin, code, &message, rejection_id);
                if entry.root == "aspect" {
                    let denominator = candidate.utility.slash_modifier.as_deref().unwrap_or("1");
                    let positive = |part: &str| {
                        Decimal::parse(part)
                            .is_ok_and(|decimal| !decimal.is_negative() && !decimal.is_zero())
                    };
                    if (suffix.contains('.') || denominator.contains('.'))
                        && positive(suffix)
                        && positive(denominator)
                    {
                        match &mut result {
                            Resolution::Diagnostic(diagnostic)
                            | Resolution::Failure(diagnostic) => {
                                diagnostic.suggested_spelling =
                                    Some(format!("aspect-[{suffix}/{denominator}]"));
                            }
                            _ => {}
                        }
                    }
                }
                return result;
            }
        };
        if utility.negative {
            if value == "auto" {
                return invalid(
                    candidate,
                    origin,
                    DiagnosticCode::Zw005,
                    "auto cannot be negative",
                    Some("R12"),
                );
            }
            value = negate(&value);
        }
        let mut declarations = Vec::new();
        for template in &entry.declaration_templates {
            let emitted = match template.value {
                EmissionValue::Resolved => Some(value.clone()),
                EmissionValue::Fixed(fixed) => Some(fixed.to_owned()),
                EmissionValue::OptionalFontSizeLeading => {
                    if entry
                        .grammar
                        .token_categories
                        .contains(&crate::TokenCategory::FontSize)
                    {
                        tokens
                            .config
                            .font_sizes
                            .get(suffix)
                            .and_then(|token| token.line_height.as_ref())
                            .map(|_| format!("var(--zw-font-size-{suffix}-leading)"))
                    } else {
                        None
                    }
                }
                EmissionValue::TransitionDuration => (value != "none").then(|| "150ms".to_owned()),
                EmissionValue::TransitionTimingFunction => {
                    (value != "none").then(|| "ease".to_owned())
                }
            };
            if let Some(value) = emitted {
                declarations.push(Declaration {
                    property: template.property.to_owned(),
                    value,
                });
            }
            if template.value == EmissionValue::OptionalFontSizeLeading
                && tokens
                    .config
                    .font_sizes
                    .get(suffix)
                    .is_some_and(|token| token.line_height.is_some())
            {
                let key = format!("tokens.fontSizes.{suffix}.lineHeight");
                if tokens.value_statuses.get(&key) == Some(&ValueStatus::CategoryUnverified) {
                    status = ValueStatus::CategoryUnverified;
                }
            }
        }
        Resolution::Rule(ResolvedRule {
            entry_id: entry.id.clone(),
            root: entry.root.clone(),
            declarations,
            selector_shape: entry.selector_shape,
            conflict_group: entry.conflict_group,
            conflict_group_rank: entry.conflict_group_rank,
            order_rank: entry.order_rank,
            value_status: status,
            registrations: entry.registrations.clone(),
        })
    }
}

type ValueError = (DiagnosticCode, String, Option<&'static str>);

fn entry_value(
    entry: &CatalogEntry,
    suffix: &str,
    candidate: &Candidate,
    tokens: &ValidatedTokens,
) -> Result<(String, ValueStatus), ValueError> {
    if candidate.utility.slash_modifier.is_some() && is_color_entry(entry) {
        let modifier = candidate
            .utility
            .slash_modifier
            .as_deref()
            .expect("colour modifier was checked");
        let alpha = modifier.parse::<u8>().map_err(|_| {
            (
                DiagnosticCode::Zw005,
                "colour opacity must be an integer from 0 through 100".to_owned(),
                Some("R14"),
            )
        })?;
        if alpha > 100 {
            return Err((
                DiagnosticCode::Zw005,
                "colour opacity must be an integer from 0 through 100".to_owned(),
                Some("R14"),
            ));
        }
        let mut base = candidate.clone();
        base.utility.slash_modifier = None;
        return entry_value(entry, suffix, &base, tokens).map(|(value, status)| {
            (
                format!("color-mix(in oklab, {value} {alpha}%, transparent)"),
                status,
            )
        });
    }
    if suffix.is_empty() && entry.grammar.accepted_kinds.contains(&ValueKind::Exact) {
        Ok((
            entry
                .fixed_value
                .clone()
                .expect("exact entries have a fixed value"),
            ValueStatus::Verified,
        ))
    } else if suffix.is_empty() && (entry.root == "rounded" || entry.root == "shadow") {
        let (category, name, variable) = if entry.root == "rounded" {
            (crate::TokenCategory::Radius, "default", "radius")
        } else {
            (crate::TokenCategory::Shadow, "default", "shadow")
        };
        if !tokens.contains(category, name) {
            return Err((
                DiagnosticCode::Zw006,
                format!("{} requires the default token", entry.root),
                Some("R17"),
            ));
        }
        let key = format!("tokens.{}.default", category.config_name());
        let status = tokens
            .value_statuses
            .get(&key)
            .copied()
            .unwrap_or(ValueStatus::Verified);
        Ok((format!("var(--zw-{variable}-default)"), status))
    } else {
        resolve_value(entry, suffix, candidate, tokens)
    }
}

fn match_priority(
    entry: &CatalogEntry,
    suffix: &str,
    candidate: &Candidate,
    tokens: &ValidatedTokens,
) -> u8 {
    let grammar = &entry.grammar;
    if suffix.is_empty() && grammar.accepted_kinds.contains(&ValueKind::Exact) {
        0
    } else if grammar
        .keywords
        .iter()
        .any(|(keyword, _)| *keyword == suffix)
    {
        1
    } else if grammar
        .token_categories
        .iter()
        .any(|category| tokens.contains(*category, suffix))
        || grammar
            .fallback_keywords
            .iter()
            .any(|(keyword, _)| *keyword == suffix)
    {
        2
    } else if candidate.utility.slash_modifier.is_some()
        && grammar.accepted_kinds.contains(&ValueKind::Fraction)
    {
        4
    } else if suffix
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'.')
        && grammar
            .accepted_kinds
            .iter()
            .any(|kind| matches!(kind, ValueKind::Scale | ValueKind::Integer))
    {
        3
    } else if full_arbitrary_suffix(suffix, candidate)
        && grammar.accepted_kinds.contains(&ValueKind::Arbitrary)
    {
        5
    } else {
        6
    }
}

/// A bracket value belongs only to the root immediately before `-[`.
/// A shorter root must not consume `border-s-[3px]` as `border-[3px]`.
fn full_arbitrary_suffix(suffix: &str, candidate: &Candidate) -> bool {
    let Some(value) = candidate.utility.arbitrary_value.as_deref() else {
        return false;
    };
    suffix
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        == Some(value)
}

fn resolve_value(
    entry: &CatalogEntry,
    suffix: &str,
    candidate: &Candidate,
    tokens: &ValidatedTokens,
) -> Result<(String, ValueStatus), ValueError> {
    let grammar = &entry.grammar;
    let modifier = candidate.utility.slash_modifier.as_deref();
    if candidate.utility.arbitrary_value.is_some() && !full_arbitrary_suffix(suffix, candidate) {
        return Err((
            DiagnosticCode::Zw005,
            "arbitrary value must immediately follow the utility root".to_owned(),
            Some("R15"),
        ));
    }
    if suffix.is_empty() {
        if let Some((_, value)) = grammar.keywords.iter().find(|(key, _)| key.is_empty()) {
            return Ok(((*value).to_owned(), ValueStatus::Verified));
        }
        return Err((
            DiagnosticCode::Zw005,
            "utility requires a value".to_owned(),
            Some("R15"),
        ));
    }
    if let Some((_, value)) = grammar.keywords.iter().find(|(key, _)| *key == suffix) {
        if modifier.is_some() {
            return Err((
                DiagnosticCode::Zw005,
                "keyword does not accept a slash modifier".to_owned(),
                Some("R14"),
            ));
        }
        return Ok(((*value).to_owned(), ValueStatus::Verified));
    }
    if grammar.accepted_kinds.contains(&ValueKind::Token) {
        for category in &grammar.token_categories {
            if tokens.contains(*category, suffix) {
                if modifier.is_some() {
                    return Err((
                        DiagnosticCode::Zw005,
                        "token does not accept a slash modifier".to_owned(),
                        Some("R14"),
                    ));
                }
                if grammar.keywords.iter().any(|(key, _)| *key == suffix) {
                    return Err((
                        DiagnosticCode::Zw007,
                        "token collides with a static keyword".to_owned(),
                        Some("R18"),
                    ));
                }
                let key = format!("tokens.{}.{suffix}", category.config_name());
                let status = tokens
                    .value_statuses
                    .get(&key)
                    .copied()
                    .unwrap_or(ValueStatus::Verified);
                return Ok((
                    format!("var(--zw-{}-{suffix})", category.variable_prefix()),
                    status,
                ));
            }
        }
    }
    if let Some((_, value)) = grammar
        .fallback_keywords
        .iter()
        .find(|(key, _)| *key == suffix)
    {
        if modifier.is_some() {
            return Err((
                DiagnosticCode::Zw005,
                "keyword does not accept a slash modifier".to_owned(),
                Some("R14"),
            ));
        }
        return Ok(((*value).to_owned(), ValueStatus::Verified));
    }
    if let Some(resolved) = resolve_special_integer(entry, suffix) {
        return resolved;
    }
    if let Some(denominator) = modifier {
        if !grammar.accepted_kinds.contains(&ValueKind::Fraction) {
            return Err((
                DiagnosticCode::Zw005,
                "fraction is not supported".to_owned(),
                Some("R14"),
            ));
        }
        let numerator = positive_ratio_part(suffix)?;
        let denominator = positive_ratio_part(denominator)?;
        if entry.root == "aspect" {
            return Ok((
                format!("{numerator} / {denominator}"),
                ValueStatus::Verified,
            ));
        }
        return Ok((
            format!("calc(100% * {numerator} / {denominator})"),
            ValueStatus::Verified,
        ));
    }
    if grammar.accepted_kinds.contains(&ValueKind::Scale) {
        if let Ok(number) = Decimal::parse(suffix) {
            if number.is_negative() {
                return Err((
                    DiagnosticCode::Zw005,
                    "scale cannot have a signed suffix".to_owned(),
                    Some("R15"),
                ));
            }
            if number.is_zero() {
                return Ok(("0".to_owned(), ValueStatus::Verified));
            }
            let Some(unit) = tokens.config.spacing_unit.as_deref() else {
                return Err((
                    DiagnosticCode::Zw006,
                    "nonzero numeric spacing requires spacingUnit".to_owned(),
                    Some("R17"),
                ));
            };
            let dimension = DecimalDimension::parse(unit).expect("validated spacingUnit");
            let product = dimension
                .multiply(&number)
                .map_err(|error| (DiagnosticCode::Zw005, error.to_string(), Some("R15")))?;
            return Ok((product.to_css(), ValueStatus::Verified));
        }
    }
    if grammar.accepted_kinds.contains(&ValueKind::Integer) {
        if let Ok(number) = suffix.parse::<u32>() {
            let value = match entry.root.as_str() {
                "z" if number <= i32::MAX as u32 => number.to_string(),
                "grid-cols" | "grid-rows" if (1..=12).contains(&number) => {
                    format!("repeat({number},minmax(0,1fr))")
                }
                "col-span" | "row-span" if (1..=12).contains(&number) => {
                    format!("span {number} / span {number}")
                }
                "col-start" | "col-end" | "row-start" | "row-end" if (1..=13).contains(&number) => {
                    number.to_string()
                }
                _ => {
                    return Err((
                        DiagnosticCode::Zw005,
                        "integer is out of range".to_owned(),
                        Some("R15"),
                    ))
                }
            };
            return Ok((value, ValueStatus::Verified));
        }
    }
    if !suffix.is_empty()
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
        && (grammar.accepted_kinds.contains(&ValueKind::Scale)
            || grammar.accepted_kinds.contains(&ValueKind::Integer))
    {
        return Err((
            DiagnosticCode::Zw005,
            "invalid or out-of-range numeric value".to_owned(),
            Some("R15"),
        ));
    }
    if let Some(bracket) = &candidate.utility.arbitrary_value {
        if grammar.accepted_kinds.contains(&ValueKind::Arbitrary) {
            let property = grammar
                .arbitrary_property
                .expect("arbitrary entries have a property");
            let (mut value, status) = if entry.root == "transition" {
                validate_transition_property_list(bracket)
            } else {
                arbitrary::validate(property, bracket)
            }
            .map_err(|message| (DiagnosticCode::Zw005, message, Some("R15")))?;
            if entry.root == "size" {
                let _height_validation = arbitrary::validate("height", bracket)
                    .map_err(|message| (DiagnosticCode::Zw005, message, Some("R15")))?;
            }
            if entry.root == "opacity" {
                validate_opacity_arbitrary(&value, status)
                    .map_err(|message| (DiagnosticCode::Zw005, message, Some("R15")))?;
            }
            if entry.root == "aspect" {
                validate_aspect_ratio(&value, status)
                    .map_err(|message| (DiagnosticCode::Zw005, message, Some("R15")))?;
                value = normalize_ratio(&value);
            }
            if (entry.root == "underline-offset" || entry.id == "v1.decoration.thickness")
                && status == ValueStatus::Verified
            {
                validate_length(&value)
                    .map_err(|message| (DiagnosticCode::Zw005, message, Some("R15")))?;
            }
            if entry.root == "rotate" && value == "none" {
                return Err((
                    DiagnosticCode::Zw005,
                    "rotate arbitrary values must be angles".to_owned(),
                    Some("R15"),
                ));
            }
            if entry.root.starts_with("translate-") {
                validate_single_component(&value)
                    .map_err(|message| (DiagnosticCode::Zw005, message, Some("R15")))?;
            }
            if entry.id.starts_with("v1.rounded") {
                validate_single_radius(&value, status)
                    .map_err(|message| (DiagnosticCode::Zw005, message, Some("R15")))?;
            }
            return Ok((value, status));
        }
        return Err((
            DiagnosticCode::Zw005,
            "arbitrary value is unsupported for this utility".to_owned(),
            Some("R15"),
        ));
    }
    Err((
        DiagnosticCode::Zw006,
        format!("unknown value or token {suffix}"),
        Some("R17"),
    ))
}

/// When every same-priority entry of an overloaded root rejected an
/// arbitrary value only as invalid for its property, name all of them
/// instead of whichever entry happened to be tried first.
fn attempted_categories_message(
    leading: &[(&CatalogEntry, &str)],
    candidate: &Candidate,
    tokens: &ValidatedTokens,
) -> Option<String> {
    candidate.utility.arbitrary_value.as_ref()?;
    let mut properties = Vec::new();
    for (entry, suffix) in leading {
        let Err((DiagnosticCode::Zw005, message, _)) =
            entry_value(entry, suffix, candidate, tokens)
        else {
            return None;
        };
        let property = message.strip_prefix("value is not valid for ")?.to_owned();
        if !properties.contains(&property) {
            properties.push(property);
        }
    }
    if properties.len() < 2 {
        return None;
    }
    let last = properties.pop()?;
    Some(format!(
        "value is not valid for any of {} or {last}",
        properties.join(", ")
    ))
}

fn shadow_color_hint(candidate: &Candidate, color: &str) -> String {
    let class = serde_json::to_string(&candidate.raw).expect("candidate is a string");
    format!(
        "{color} is a colors token, but zudo-wind v1 does not compose colors into shadow values. The shadow root accepts complete values: none, arbitrary box-shadow values, or shadows tokens. Move the tint into a shadows token, or author the complete box-shadow in CSS and reserve {class} with wind.authoredClasses"
    )
}

fn is_color_entry(entry: &CatalogEntry) -> bool {
    entry
        .grammar
        .token_categories
        .contains(&crate::TokenCategory::Color)
}

fn resolve_special_integer(
    entry: &CatalogEntry,
    suffix: &str,
) -> Option<Result<(String, ValueStatus), ValueError>> {
    if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if entry.id.starts_with("v1.border.width")
        || entry.id.starts_with("v1.divide.width")
        || entry.id == "v1.outline.width"
        || entry.id == "v1.outline.offset"
        || entry.id == "v1.underline-offset"
        || entry.id == "v1.decoration.thickness"
    {
        let canonical = match Decimal::parse(suffix) {
            Ok(value) => value.to_string(),
            Err(error) => {
                return Some(Err((DiagnosticCode::Zw005, error.to_string(), Some("R15"))))
            }
        };
        return Some(Ok((format!("{canonical}px"), ValueStatus::Verified)));
    }
    let canonical = match Decimal::parse(suffix) {
        Ok(value) => value.to_string(),
        Err(error) => return Some(Err((DiagnosticCode::Zw005, error.to_string(), Some("R15")))),
    };
    let integer = match canonical.parse::<u32>() {
        Ok(integer) => integer,
        Err(_) => {
            return Some(Err((
                DiagnosticCode::Zw005,
                "integer is out of range".to_owned(),
                Some(if entry.id == "v1.opacity" {
                    "R14"
                } else {
                    "R15"
                }),
            )))
        }
    };
    match entry.id.as_str() {
        "v1.opacity" => {
            if integer > 100 {
                Some(Err((
                    DiagnosticCode::Zw005,
                    "opacity must be an integer from 0 through 100".to_owned(),
                    Some("R14"),
                )))
            } else {
                Some(Ok((integer_percent(integer), ValueStatus::Verified)))
            }
        }
        "v1.duration" => {
            if integer > 60_000 {
                Some(Err((
                    DiagnosticCode::Zw005,
                    "duration must be an integer from 0 through 60000 milliseconds".to_owned(),
                    Some("R15"),
                )))
            } else {
                Some(Ok((format!("{integer}ms"), ValueStatus::Verified)))
            }
        }
        "v1.rotate" => {
            if integer > 360 {
                Some(Err((
                    DiagnosticCode::Zw005,
                    "rotation must be an integer from 0 through 360 degrees".to_owned(),
                    Some("R15"),
                )))
            } else {
                Some(Ok((format!("{integer}deg"), ValueStatus::Verified)))
            }
        }
        _ => None,
    }
}

/// Rejects the keyword and percentage forms the property also accepts, so an
/// arbitrary underline offset stays a nonnegative length.
// Lightning CSS has no typed `text-underline-offset`, so the property parse accepts any
// tokens; check the value as a standalone `<length>` instead.
fn validate_length(value: &str) -> Result<(), String> {
    use lightningcss::traits::{Parse, TrySign};
    use lightningcss::values::length::Length;
    let value = value.trim();
    // Lightning CSS also reads a bare number as px; CSS only allows a unitless zero.
    let bare_nonzero_number = value.parse::<f64>().is_ok_and(|number| number != 0.0);
    let nonnegative = !bare_nonzero_number
        && Length::parse_string(value)
            .ok()
            .and_then(|length| length.try_sign())
            .is_some_and(|sign| sign.is_sign_positive());
    if nonnegative {
        Ok(())
    } else {
        Err("arbitrary value must be a nonnegative length".to_owned())
    }
}

fn integer_percent(value: u32) -> String {
    match value {
        0 => "0".to_owned(),
        100 => "1".to_owned(),
        1..=9 => format!("0.0{value}"),
        value if value % 10 == 0 => format!("0.{}", value / 10),
        _ => format!("0.{value}"),
    }
}

fn validate_opacity_arbitrary(value: &str, status: ValueStatus) -> Result<(), String> {
    if status == ValueStatus::CategoryUnverified {
        if value.trim_start().starts_with("var(") && value.trim_end().ends_with(')') {
            return Ok(());
        }
        return Err(
            "opacity arbitrary values may use var() only as an unverified value".to_owned(),
        );
    }
    let (number, maximum) = value
        .strip_suffix('%')
        .map_or((value, "1"), |number| (number, "100"));
    if decimal_exceeds(number, maximum) || !valid_nonnegative_decimal(number) {
        return Err(
            "opacity arbitrary values must be a number from 0 to 1 or a percentage from 0% to 100%"
                .to_owned(),
        );
    }
    Ok(())
}

fn valid_nonnegative_decimal(value: &str) -> bool {
    let mantissa = if let Some((mantissa, exponent)) = value.split_once(['e', 'E']) {
        if exponent.parse::<i32>().is_err() {
            return false;
        }
        mantissa
    } else {
        value
    };
    if mantissa.is_empty() {
        return false;
    }
    let mut parts = mantissa.split('.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next();
    if parts.next().is_some() || (whole.is_empty() && fraction.is_none()) {
        return false;
    }
    whole.bytes().all(|byte| byte.is_ascii_digit())
        && fraction.is_none_or(|fraction| fraction.bytes().all(|byte| byte.is_ascii_digit()))
}

fn decimal_exceeds(value: &str, maximum: &str) -> bool {
    let (mantissa, exponent) = match value.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => match exponent.parse::<i32>() {
            Ok(exponent) => (mantissa, exponent),
            Err(_) => return true,
        },
        None => (value, 0),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{whole}{fraction}");
    let Some(first_nonzero) = digits.bytes().position(|byte| byte != b'0') else {
        return false;
    };
    let significant = &digits[first_nonzero..];
    let decimal_point = (whole.len() as i32)
        .saturating_add(exponent)
        .saturating_sub(first_nonzero as i32);
    if decimal_point > maximum.len() as i32 {
        return true;
    }
    if decimal_point <= 0 {
        return false;
    }
    let point = decimal_point as usize;
    let mut integer_part = significant.chars().take(point).collect::<String>();
    if integer_part.len() < point {
        integer_part.push_str(&"0".repeat(point - integer_part.len()));
    }
    let maximum_value = maximum.parse::<u32>().unwrap_or(u32::MAX).to_string();
    if integer_part.len() > maximum_value.len() {
        return true;
    }
    if integer_part.len() < maximum_value.len() {
        return false;
    }
    if integer_part < maximum_value {
        return false;
    }
    if integer_part > maximum_value {
        return true;
    }
    significant.chars().skip(point).any(|digit| digit != '0')
}

fn validate_transition_property_list(input: &str) -> Result<(String, ValueStatus), String> {
    let (value, status) = arbitrary::validate("transition-property", input)?;
    for item in split_top_level_commas(&value) {
        if item.trim().is_empty() {
            return Err("transition property list contains an empty item".to_owned());
        }
        arbitrary::validate("transition-property", item.trim())?;
    }
    Ok((value, status))
}

fn split_top_level_commas(value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0_u32;
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(current_quote) = quote {
            if character == current_quote {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&value[start..index]);
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&value[start..]);
    parts
}

fn normalize_ratio(value: &str) -> String {
    value
        .split_once('/')
        .map(|(left, right)| format!("{} / {}", left.trim(), right.trim()))
        .unwrap_or_else(|| value.to_owned())
}

fn validate_aspect_ratio(value: &str, status: ValueStatus) -> Result<(), String> {
    if status == ValueStatus::CategoryUnverified {
        return Ok(());
    }
    let Some((numerator, denominator)) = value.split_once('/') else {
        return Err("aspect arbitrary values must be ratios".to_owned());
    };
    if denominator.contains('/') {
        return Err("aspect ratio must contain one slash".to_owned());
    }
    for part in [numerator.trim(), denominator.trim()] {
        let value = Decimal::parse(part)
            .map_err(|_| "aspect ratio parts must be positive numbers".to_owned())?;
        if value.is_negative() || value.is_zero() {
            return Err("aspect ratio parts must be positive numbers".to_owned());
        }
    }
    Ok(())
}

fn validate_single_radius(value: &str, status: ValueStatus) -> Result<(), String> {
    validate_single_component(value)?;
    if status == ValueStatus::CategoryUnverified {
        return Ok(());
    }
    let dimension = DecimalDimension::parse(value)
        .map_err(|_| "radius values must be one nonnegative length-percentage".to_owned())?;
    if dimension.magnitude.is_negative()
        || (dimension.unit.is_empty() && !dimension.magnitude.is_zero())
    {
        return Err("radius values must be one nonnegative length-percentage".to_owned());
    }
    Ok(())
}

fn validate_single_component(value: &str) -> Result<(), String> {
    let mut depth = 0_u32;
    let mut quote = None;
    for character in value.chars() {
        match character {
            '\'' | '"' if quote == Some(character) => quote = None,
            '\'' | '"' if quote.is_none() => quote = Some(character),
            '(' if quote.is_none() => depth += 1,
            ')' if quote.is_none() => depth = depth.saturating_sub(1),
            character if quote.is_none() && depth == 0 && character.is_whitespace() => {
                return Err("value must be a single length-percentage".to_owned())
            }
            _ => {}
        }
    }
    Ok(())
}

fn positive_ratio_part(value: &str) -> Result<u32, ValueError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err((
            DiagnosticCode::Zw005,
            "fraction requires positive integer parts".to_owned(),
            Some("R13"),
        ));
    }
    let number = value.parse::<u32>().map_err(|_| {
        (
            DiagnosticCode::Zw005,
            "fraction is out of range".to_owned(),
            Some("R13"),
        )
    })?;
    if !(1..=1_000_000).contains(&number) {
        return Err((
            DiagnosticCode::Zw005,
            "fraction is out of range".to_owned(),
            Some("R13"),
        ));
    }
    Ok(number)
}

fn negate(value: &str) -> String {
    if let Ok(dimension) = DecimalDimension::parse(value) {
        if dimension.magnitude.is_zero() {
            "0".to_owned()
        } else {
            format!("-{}", dimension.to_css())
        }
    } else if value == "100%" {
        "-100%".to_owned()
    } else {
        format!("calc({value} * -1)")
    }
}

impl Catalog {
    /// Whether an underscore-separated head starts at a utility root: `p`,
    /// `grid` and `text-link` do; `card` does not.
    fn recognizes_root(&self, head: &str) -> bool {
        let starts_at = |root: &str| {
            head == root
                || head
                    .strip_prefix(root)
                    .is_some_and(|rest| rest.starts_with('-'))
        };
        ["ring", "animate", "scale", "transform", "group", "peer"]
            .into_iter()
            .any(starts_at)
            || self
                .entries
                .iter()
                .any(|entry| !is_new_exact_static_root(&entry.root) && starts_at(&entry.root))
    }
}

fn adopted_static_modifier_error(
    candidate: &Candidate,
    origin: &Origin,
    supported: &str,
    reason: &str,
    rejection_id: &'static str,
) -> Resolution {
    let reserved = serde_json::to_string(&candidate.raw).expect("candidate is a string");
    let message = format!(
        "{}: {reason}; use `{supported}` or author the declaration in CSS and reserve the complete candidate with wind.authoredClasses: {{ {reserved}: true }}",
        candidate.raw
    );
    let mut result = invalid(
        candidate,
        origin,
        DiagnosticCode::Zw005,
        &message,
        Some(rejection_id),
    );
    if let Resolution::Diagnostic(diagnostic) | Resolution::Failure(diagnostic) = &mut result {
        diagnostic.suggested_spelling = Some(supported.to_owned());
    }
    result
}

fn unknown_root(candidate: &Candidate, origin: &Origin) -> Resolution {
    if strict(origin) {
        Resolution::Failure(diagnostic(
            candidate,
            origin,
            DiagnosticCode::Zw008,
            "unknown explicit utility",
            None,
        ))
    } else {
        Resolution::NotUtility
    }
}

/// A migration-vocabulary name. Explicit origins still fail; a class position
/// warns (compile promotes it under `wind.strict`), and a low-confidence
/// literal stays audit information.
fn foreign(
    candidate: &Candidate,
    origin: &Origin,
    family: &super::migration::ForeignFamily,
) -> Resolution {
    let message = format!(
        "unsupported foreign utility (migration vocabulary v{}): {} uses the Tailwind `{}` utility, which zudo-wind v1 does not implement, so no CSS is generated. Author {} in CSS and reserve the class with wind.authoredClasses: {{ {}: true }}",
        super::migration::MIGRATION_VOCABULARY_VERSION,
        candidate.raw,
        family.root,
        family.alternative,
        serde_json::to_string(&candidate.raw).expect("candidate is a string")
    );
    match origin {
        Origin::RoleClass { .. } => Resolution::NotUtility,
        Origin::Safelist { .. } | Origin::Manifest { .. } => Resolution::Failure(diagnostic(
            candidate,
            origin,
            DiagnosticCode::Zw014,
            &message,
            None,
        )),
        Origin::Source { .. } => {
            let mut diagnostic =
                diagnostic(candidate, origin, DiagnosticCode::Zw014, &message, None);
            if diagnostic.severity == Severity::Error {
                diagnostic.severity = Severity::Warning;
            }
            Resolution::Diagnostic(diagnostic)
        }
        Origin::Config { .. } | Origin::Stylesheet { .. } => Resolution::NotUtility,
    }
}

fn strict(origin: &Origin) -> bool {
    matches!(origin, Origin::Safelist { .. } | Origin::Manifest { .. })
}

fn invalid(
    candidate: &Candidate,
    origin: &Origin,
    code: DiagnosticCode,
    message: &str,
    rejection_id: Option<&'static str>,
) -> Resolution {
    if matches!(origin, Origin::RoleClass { .. }) {
        return Resolution::NotUtility;
    }
    let diagnostic = diagnostic(candidate, origin, code, message, rejection_id);
    if strict(origin) {
        Resolution::Failure(diagnostic)
    } else {
        Resolution::Diagnostic(diagnostic)
    }
}

fn diagnostic(
    candidate: &Candidate,
    origin: &Origin,
    code: DiagnosticCode,
    message: &str,
    rejection_id: Option<&'static str>,
) -> Diagnostic {
    let severity = match origin {
        Origin::Source {
            position_kind: SourcePositionKind::Literal,
            ..
        } => Severity::AuditInfo,
        _ => Severity::Error,
    };
    let message = if code == DiagnosticCode::Zw006
        && matches!(
            origin,
            Origin::Source {
                position_kind: SourcePositionKind::Class,
                ..
            }
        ) {
        format!(
            "{message}. If this is a genuine authored class, reserve its complete name with wind.authoredClasses: {{ {}: true }}; otherwise correct or declare the intended token. No generated utility CSS is emitted for this candidate",
            serde_json::to_string(&candidate.raw).expect("candidate is a string")
        )
    } else {
        message.to_owned()
    };
    Diagnostic {
        severity,
        code,
        candidate: Some(candidate.raw.clone()),
        origin: Some(Box::new(origin.clone())),
        message,
        suggested_spelling: None,
        rejection_id,
    }
}
