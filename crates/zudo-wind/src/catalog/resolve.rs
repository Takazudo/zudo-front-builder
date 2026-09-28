use std::collections::BTreeSet;

use super::{arbitrary, Catalog, CatalogEntry, SelectorShape, ValueKind};
use crate::{
    Candidate, Decimal, DecimalDimension, Diagnostic, DiagnosticCode, Origin, Severity,
    SourcePositionKind, ValidatedTokens, ValueStatus, VariantKind,
};

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
        if authored_classes.contains(&candidate.raw) {
            return Resolution::NotUtility;
        }
        if candidate.raw == "group" || candidate.raw == "peer" {
            return Resolution::NotUtility;
        }
        let name = &candidate.utility.named;
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
            .collect();
        let Some(longest) = matching.iter().map(|(entry, _)| entry.root.len()).max() else {
            return if strict(origin) {
                Resolution::Failure(diagnostic(
                    candidate,
                    origin,
                    DiagnosticCode::Zw008,
                    "unknown explicit utility",
                    None,
                ))
            } else {
                Resolution::NotUtility
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
        if successful.len() > 1 {
            return invalid(
                candidate,
                origin,
                DiagnosticCode::Zw005,
                "value is ambiguous across catalog entries",
                Some("R15"),
            );
        }
        let (entry, suffix) = successful.first().copied().unwrap_or(leading[0]);
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
            return invalid(
                candidate,
                origin,
                DiagnosticCode::Zw005,
                "negative value is not supported",
                Some("R12"),
            );
        }
        if utility.slash_modifier.is_some() && !entry.grammar.allows_fraction_slash {
            return invalid(
                candidate,
                origin,
                DiagnosticCode::Zw005,
                "slash modifier is not supported",
                Some("R14"),
            );
        }
        let value = entry_value(entry, suffix, candidate, tokens);
        let (mut value, status) = match value {
            Ok(value) => value,
            Err((code, message, rejection_id)) => {
                return invalid(candidate, origin, code, &message, rejection_id)
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
        let declarations = entry
            .emitter
            .iter()
            .map(|property| Declaration {
                property: (*property).to_owned(),
                value: value.clone(),
            })
            .collect();
        Resolution::Rule(ResolvedRule {
            entry_id: entry.id.clone(),
            root: entry.root.clone(),
            declarations,
            selector_shape: entry.selector_shape,
            conflict_group: entry.conflict_group,
            conflict_group_rank: entry.conflict_group_rank,
            order_rank: entry.order_rank,
            value_status: status,
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
    if suffix.is_empty() && entry.grammar.accepted_kinds.contains(&ValueKind::Exact) {
        Ok((
            entry
                .fixed_value
                .clone()
                .expect("exact entries have a fixed value"),
            ValueStatus::Verified,
        ))
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
    } else if candidate.utility.arbitrary_value.is_some()
        && grammar.accepted_kinds.contains(&ValueKind::Arbitrary)
    {
        5
    } else {
        6
    }
}

fn resolve_value(
    entry: &CatalogEntry,
    suffix: &str,
    candidate: &Candidate,
    tokens: &ValidatedTokens,
) -> Result<(String, ValueStatus), ValueError> {
    let grammar = &entry.grammar;
    let modifier = candidate.utility.slash_modifier.as_deref();
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
            return arbitrary::validate(property, bracket)
                .map_err(|message| (DiagnosticCode::Zw005, message, Some("R15")));
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
    Diagnostic {
        severity,
        code,
        candidate: Some(candidate.raw.clone()),
        origin: Some(Box::new(origin.clone())),
        message: message.to_owned(),
        suggested_spelling: None,
        rejection_id,
    }
}
