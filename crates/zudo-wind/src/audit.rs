//! Deterministic diagnostics over extracted candidate occurrences.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::explain::{diagnostic_view, origin_view, DiagnosticView, OriginView};
use crate::{
    compile_validated, structural_split, Catalog, Diagnostic, DiagnosticCode, ExtractionResult,
    NoteKind, Origin, OriginCandidate, PositionKind, RuleMetadata, Severity, SortKey,
    SourcePositionKind, TokenOverride, WindConfig, SPEC_REVISION, SPEC_VERSION,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Complete,
    InvalidConfiguration,
    GenerationDisabled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditReport {
    pub outcome: AuditOutcome,
    pub spec_version: u32,
    pub spec_revision: u32,
    pub diagnostics: Vec<DiagnosticView>,
    pub unrecognized_classes: Vec<UnrecognizedClass>,
    pub conflicts: Vec<AuditConflict>,
    pub dead_classes: Vec<DeadClass>,
    pub dynamic_constructions: Vec<DynamicConstruction>,
    pub adjacent_interpolations: Vec<InterpolatedCandidate>,
    pub extraction_notes: Vec<AuditNote>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditSource {
    /// Stable SourcePlan identity, usually a relative root label and path.
    pub source_id: String,
    pub extraction: ExtractionResult,
}

impl AuditSource {
    pub fn new(source_id: impl Into<String>, extraction: ExtractionResult) -> Self {
        Self {
            source_id: source_id.into(),
            extraction,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditInput {
    pub sources: Vec<AuditSource>,
    /// Set this to false for the explicit `wind: false` audit outcome.
    pub generation_enabled: bool,
}

impl Default for AuditInput {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            generation_enabled: true,
        }
    }
}

impl AuditInput {
    pub fn new(sources: Vec<AuditSource>) -> Self {
        Self {
            sources,
            generation_enabled: true,
        }
    }

    pub fn single(source_id: impl Into<String>, extraction: ExtractionResult) -> Self {
        Self::new(vec![AuditSource::new(source_id, extraction)])
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnrecognizedClass {
    pub candidate: String,
    pub origin: OriginView,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeadClass {
    pub candidate: String,
    pub diagnostic: DiagnosticView,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditConflict {
    pub origin: OriginView,
    pub first_candidate: String,
    pub second_candidate: String,
    pub first_group: String,
    pub second_group: String,
    pub overlapping_properties: Vec<String>,
    /// Generated source order for the pair. This is not a computed-style claim;
    /// selector specificity still participates in the browser cascade.
    pub emitted_order: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicConstruction {
    pub source_id: String,
    pub byte_offset: usize,
    pub line: usize,
    pub byte_column: usize,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InterpolatedCandidate {
    pub candidate: String,
    pub origin: OriginView,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditNote {
    pub source_id: String,
    pub byte_offset: usize,
    pub line: usize,
    pub byte_column: usize,
    pub kind: String,
    pub text: String,
}

/// Audits extracted occurrences while retaining their stable source identity.
/// The function is pure: it does not read source files or write output.
pub fn audit(input: &AuditInput, config: &WindConfig) -> AuditReport {
    audit_with_token_overrides(input, config, &[])
}

/// Audits source candidates and reports host-over-preset token entries retained
/// by the project config loader.
pub fn audit_with_token_overrides(
    input: &AuditInput,
    config: &WindConfig,
    token_overrides: &[TokenOverride],
) -> AuditReport {
    let mut report = empty_report(
        AuditOutcome::Complete,
        if input.generation_enabled {
            config.spec
        } else {
            SPEC_VERSION
        },
    );
    if !input.generation_enabled {
        report.outcome = AuditOutcome::GenerationDisabled;
        return report;
    }

    let validated = match config.validate() {
        Ok(validated) => validated,
        Err(diagnostics) => {
            report.outcome = AuditOutcome::InvalidConfiguration;
            report.diagnostics = diagnostics.iter().map(diagnostic_view).collect();
            return report;
        }
    };
    append_token_override_diagnostics(&mut report, token_overrides);

    let mut candidates = Vec::new();
    for source in &input.sources {
        append_notes(source, &mut report);
        for candidate in &source.extraction.candidates {
            for occurrence in &candidate.occurrences {
                candidates.push(OriginCandidate {
                    text: candidate.text.clone(),
                    origin: source_origin(&source.source_id, occurrence),
                });
                if occurrence.adjacent_interpolation {
                    report.adjacent_interpolations.push(InterpolatedCandidate {
                        candidate: candidate.text.clone(),
                        origin: origin_view(&source_origin(&source.source_id, occurrence)),
                    });
                }
            }
        }
    }
    sort_report_inputs(&mut report);

    let result = compile_validated(&candidates, &validated);
    report.diagnostics.extend(
        result
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                matches!(diagnostic.origin.as_deref(), Some(Origin::Source { .. }))
            })
            .map(diagnostic_view),
    );
    report.dead_classes = result
        .diagnostics
        .iter()
        .filter_map(|diagnostic| {
            let origin = diagnostic.origin.as_deref()?;
            let Origin::Source {
                position_kind: SourcePositionKind::Class,
                ..
            } = origin
            else {
                return None;
            };
            let candidate = diagnostic.candidate.as_deref()?;
            let is_dead = diagnostic.code == DiagnosticCode::Zw006
                || (diagnostic.code == DiagnosticCode::Zw002
                    && recognized_utility_shape(candidate));
            is_dead.then(|| DeadClass {
                candidate: candidate.to_owned(),
                diagnostic: diagnostic_view(diagnostic),
            })
        })
        .collect();
    report.unrecognized_classes = result
        .ordinary_classes
        .iter()
        .filter_map(|ordinary| {
            let Origin::Source {
                position_kind: SourcePositionKind::Class,
                ..
            } = &ordinary.origin
            else {
                return None;
            };
            if ordinary.text == "group" || ordinary.text == "peer" {
                return None;
            }
            Some(UnrecognizedClass {
                candidate: ordinary.text.clone(),
                origin: origin_view(&ordinary.origin),
            })
        })
        .collect();
    report.conflicts = find_conflicts(&result.rules);
    report
        .diagnostics
        .extend(report.conflicts.iter().map(|conflict| DiagnosticView {
            severity: "auditInfo".to_owned(),
            code: "ZW013".to_owned(),
            candidate: Some(conflict.first_candidate.clone()),
            origin: Some(conflict.origin.clone()),
            message: format!(
                "{} and {} both write {}; emitted rule order is {} then {}",
                conflict.first_candidate,
                conflict.second_candidate,
                conflict.overlapping_properties.join(", "),
                conflict.emitted_order[0],
                conflict.emitted_order[1]
            ),
            suggestion: None,
            rejection_id: None,
        }));
    sort_report_results(&mut report);
    report
}

fn append_token_override_diagnostics(report: &mut AuditReport, token_overrides: &[TokenOverride]) {
    for token_override in token_overrides {
        let category = token_override.category.config_name();
        let key_path = format!("wind.tokens.{category}.{}", token_override.name);
        let diagnostic = Diagnostic {
            severity: Severity::AuditInfo,
            code: DiagnosticCode::Zw015,
            candidate: Some(format!("{category}.{}", token_override.name)),
            origin: Some(Box::new(Origin::Config {
                key_path: key_path.clone(),
            })),
            message: format!(
                "host value overrides preset[{}]: {} -> {}",
                token_override.preset_index,
                token_override.previous_value,
                token_override.final_value
            ),
            suggested_spelling: None,
            rejection_id: None,
        };
        report.diagnostics.push(diagnostic_view(&diagnostic));
    }
}

pub fn render_audit(report: &AuditReport) -> String {
    render_audit_with_grouping(report, false)
}

/// Render a compact text report with repeated names grouped by candidate,
/// diagnostic code, and severity where those fields apply.
pub fn render_audit_grouped(report: &AuditReport) -> String {
    render_audit_with_grouping(report, true)
}

fn render_audit_with_grouping(report: &AuditReport, grouped: bool) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "outcome: {}\n",
        audit_outcome_name(report.outcome)
    ));
    output.push_str(&format!(
        "spec: {} revision {}\n",
        report.spec_version, report.spec_revision
    ));
    if grouped {
        render_section(
            &mut output,
            "unrecognized classes",
            grouped_unrecognized_classes(&report.unrecognized_classes),
        );
    } else {
        render_section(
            &mut output,
            "unrecognized classes",
            report
                .unrecognized_classes
                .iter()
                .map(|entry| format!("{} at {}", entry.candidate, origin_location(&entry.origin))),
        );
    }
    render_section(
        &mut output,
        "conflicts",
        report.conflicts.iter().map(|conflict| {
            format!(
                "{} / {} overlap [{}] at {}",
                conflict.first_candidate,
                conflict.second_candidate,
                conflict.overlapping_properties.join(", "),
                origin_location(&conflict.origin)
            )
        }),
    );
    if grouped {
        render_section(
            &mut output,
            "dead classes",
            grouped_dead_classes(&report.dead_classes),
        );
    } else {
        render_section(
            &mut output,
            "dead classes",
            report.dead_classes.iter().map(|dead| {
                let location = dead
                    .diagnostic
                    .origin
                    .as_ref()
                    .map(|origin| format!(" at {}", origin_location(origin)))
                    .unwrap_or_default();
                format!("{} [{}]{}", dead.candidate, dead.diagnostic.code, location)
            }),
        );
    }
    render_section(
        &mut output,
        "dynamic constructions",
        report.dynamic_constructions.iter().map(|dynamic| {
            format!(
                "{} at {}:{}:{}",
                dynamic.text, dynamic.source_id, dynamic.line, dynamic.byte_column
            )
        }),
    );
    render_section(
        &mut output,
        "tokens adjacent to interpolation",
        report
            .adjacent_interpolations
            .iter()
            .map(|item| format!("{} at {}", item.candidate, origin_location(&item.origin))),
    );
    if grouped {
        render_section(
            &mut output,
            "diagnostics",
            grouped_diagnostics(&report.diagnostics),
        );
    } else {
        render_section(
            &mut output,
            "diagnostics",
            report.diagnostics.iter().map(|diagnostic| {
                let location = diagnostic
                    .origin
                    .as_ref()
                    .map(|origin| format!(" at {}", origin_location(origin)))
                    .unwrap_or_default();
                let candidate = diagnostic
                    .candidate
                    .as_deref()
                    .map(|candidate| format!("{candidate}: "))
                    .unwrap_or_default();
                let suggestion = diagnostic
                    .suggestion
                    .as_deref()
                    .map(|spelling| format!("; suggested spelling: {spelling}"))
                    .unwrap_or_default();
                format!(
                    "{} {}{}: {}{}{}",
                    diagnostic.code,
                    diagnostic.severity,
                    location,
                    candidate,
                    diagnostic.message,
                    suggestion
                )
            }),
        );
    }
    render_section(
        &mut output,
        "extraction notes",
        report.extraction_notes.iter().map(|note| {
            format!(
                "{} at {}:{}:{}: {}",
                note.kind, note.source_id, note.line, note.byte_column, note.text
            )
        }),
    );
    output
}

struct GroupedOccurrence<'a> {
    count: usize,
    first_origin: Option<&'a OriginView>,
}

fn record_grouped_occurrence<'a, K: Ord>(
    groups: &mut BTreeMap<K, GroupedOccurrence<'a>>,
    key: K,
    origin: Option<&'a OriginView>,
) {
    let occurrence = groups.entry(key).or_insert(GroupedOccurrence {
        count: 0,
        first_origin: origin,
    });
    occurrence.count += 1;
    match (occurrence.first_origin, origin) {
        (None, Some(origin)) => occurrence.first_origin = Some(origin),
        (Some(current), Some(origin))
            if diagnostic_origin_order(Some(origin)) < diagnostic_origin_order(Some(current)) =>
        {
            occurrence.first_origin = Some(origin);
        }
        _ => {}
    }
}

fn render_grouped_summary(
    label: String,
    count: usize,
    first_origin: Option<&OriginView>,
) -> String {
    let first_origin = first_origin
        .map(origin_location)
        .unwrap_or_else(|| "unknown".to_owned());
    format!("{label} x{count}, first at {first_origin}")
}

fn render_grouped_occurrence(label: String, occurrence: &GroupedOccurrence<'_>) -> String {
    render_grouped_summary(label, occurrence.count, occurrence.first_origin)
}

fn grouped_unrecognized_classes(entries: &[UnrecognizedClass]) -> Vec<String> {
    let mut groups = BTreeMap::new();
    for entry in entries {
        record_grouped_occurrence(&mut groups, entry.candidate.as_str(), Some(&entry.origin));
    }
    groups
        .into_iter()
        .map(|(candidate, occurrence)| render_grouped_occurrence(candidate.to_owned(), &occurrence))
        .collect()
}

fn grouped_dead_classes(entries: &[DeadClass]) -> Vec<String> {
    let mut groups = BTreeMap::new();
    for entry in entries {
        record_grouped_occurrence(
            &mut groups,
            (
                entry.candidate.as_str(),
                entry.diagnostic.code.as_str(),
                entry.diagnostic.severity.as_str(),
            ),
            entry.diagnostic.origin.as_ref(),
        );
    }
    groups
        .into_iter()
        .map(|((candidate, code, severity), occurrence)| {
            render_grouped_occurrence(format!("{candidate} [{code} {severity}]"), &occurrence)
        })
        .collect()
}

fn grouped_diagnostics(entries: &[DiagnosticView]) -> Vec<String> {
    let mut groups = BTreeMap::new();
    for entry in entries {
        let group = groups
            .entry((
                entry.candidate.as_deref(),
                entry.code.as_str(),
                entry.severity.as_str(),
            ))
            .or_insert_with(|| GroupedDiagnostic {
                count: 0,
                first: entry,
            });
        group.count += 1;
        if diagnostic_precedes(entry, group.first) {
            group.first = entry;
        }
    }
    groups
        .into_iter()
        .map(|((candidate, code, severity), group)| {
            let label = match candidate {
                Some(candidate) => format!("{candidate} [{code} {severity}]"),
                None => format!("[{code} {severity}]"),
            };
            let mut summary =
                render_grouped_summary(label, group.count, group.first.origin.as_ref());
            summary.push_str(": ");
            summary.push_str(&group.first.message);
            if let Some(suggestion) = &group.first.suggestion {
                summary.push_str("; suggested spelling: ");
                summary.push_str(suggestion);
            }
            summary
        })
        .collect()
}

struct GroupedDiagnostic<'a> {
    count: usize,
    first: &'a DiagnosticView,
}

fn diagnostic_precedes(left: &DiagnosticView, right: &DiagnosticView) -> bool {
    match (left.origin.as_ref(), right.origin.as_ref()) {
        (Some(left_origin), Some(right_origin)) => {
            match diagnostic_origin_order(Some(left_origin))
                .cmp(&diagnostic_origin_order(Some(right_origin)))
            {
                std::cmp::Ordering::Less => true,
                std::cmp::Ordering::Equal => diagnostic_details_precede(left, right),
                std::cmp::Ordering::Greater => false,
            }
        }
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => diagnostic_details_precede(left, right),
    }
}

fn diagnostic_details_precede(left: &DiagnosticView, right: &DiagnosticView) -> bool {
    (left.message.as_str(), left.suggestion.as_deref())
        < (right.message.as_str(), right.suggestion.as_deref())
}

/// `source:line:column` for a source origin, the manifest or other owner
/// otherwise. Byte offsets stay in the structured report, never in place of a line.
fn origin_location(origin: &OriginView) -> String {
    match (&origin.source_id, origin.line, origin.byte_column) {
        (Some(source), Some(line), Some(column)) => format!("{source}:{line}:{column}"),
        (Some(source), _, _) => source.clone(),
        _ => match (&origin.producer, &origin.path, origin.index) {
            (Some(producer), _, Some(index)) => format!("manifest {producer}[{index}]"),
            (_, Some(path), _) => path.clone(),
            _ => origin
                .owner
                .as_ref()
                .map(|owner| format!("{} {owner}", origin.kind))
                .or_else(|| origin.key_path.clone())
                .or_else(|| {
                    origin
                        .role_key
                        .as_ref()
                        .map(|key| format!("roleClass {key}"))
                })
                .unwrap_or_else(|| origin.kind.clone()),
        },
    }
}

pub fn audit_json(report: &AuditReport) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(report)
}

fn empty_report(outcome: AuditOutcome, spec_version: u32) -> AuditReport {
    AuditReport {
        outcome,
        spec_version,
        spec_revision: SPEC_REVISION,
        diagnostics: Vec::new(),
        unrecognized_classes: Vec::new(),
        conflicts: Vec::new(),
        dead_classes: Vec::new(),
        dynamic_constructions: Vec::new(),
        adjacent_interpolations: Vec::new(),
        extraction_notes: Vec::new(),
    }
}

fn append_notes(source: &AuditSource, report: &mut AuditReport) {
    for note in &source.extraction.notes {
        let note_kind = note_kind_name(&note.kind);
        report.extraction_notes.push(AuditNote {
            source_id: source.source_id.clone(),
            byte_offset: note.byte_offset,
            line: note.line,
            byte_column: note.byte_column,
            kind: note_kind.to_owned(),
            text: note.text.clone(),
        });
        if note.kind == NoteKind::DynamicConstruction {
            report.dynamic_constructions.push(DynamicConstruction {
                source_id: source.source_id.clone(),
                byte_offset: note.byte_offset,
                line: note.line,
                byte_column: note.byte_column,
                text: note.text.clone(),
            });
        }
        let diagnostic = match &note.kind {
            NoteKind::DynamicConstruction => Some((
                "auditInfo",
                "ZW012",
                None,
                "dynamic class construction cannot be confirmed by static analysis",
            )),
            NoteKind::MalformedClassCandidate => Some((
                "auditInfo",
                "ZW001",
                Some(note.text.clone()),
                "malformed candidate was retained by extraction",
            )),
            NoteKind::InvalidUtf8 => Some((
                "warning",
                "ZW011",
                None,
                "source is not UTF-8 and was skipped by extraction",
            )),
            NoteKind::UnterminatedLiteral => None,
        };
        if let Some((severity, code, candidate, message)) = diagnostic {
            report.diagnostics.push(DiagnosticView {
                severity: severity.to_owned(),
                code: code.to_owned(),
                candidate,
                origin: Some(note_origin_view(&source.source_id, note)),
                message: message.to_owned(),
                suggestion: None,
                rejection_id: None,
            });
        }
    }
}

fn note_origin_view(source_id: &str, note: &crate::ExtractionNote) -> OriginView {
    OriginView {
        kind: "source".to_owned(),
        source_id: Some(source_id.to_owned()),
        byte_offset: Some(note.byte_offset),
        line: Some(note.line),
        byte_column: Some(note.byte_column),
        ..OriginView::default()
    }
}

fn source_origin(source_id: &str, occurrence: &crate::Occurrence) -> Origin {
    Origin::Source {
        source_id: source_id.to_owned(),
        byte_offset: occurrence.byte_offset,
        byte_length: occurrence.byte_length,
        line: occurrence.line,
        byte_column: occurrence.byte_column,
        literal_byte_offset: occurrence.literal_byte_offset,
        literal_byte_length: occurrence.literal_byte_length,
        position_kind: match occurrence.position_kind {
            PositionKind::Class => SourcePositionKind::Class,
            PositionKind::Literal => SourcePositionKind::Literal,
        },
    }
}

fn find_conflicts(rules: &[RuleMetadata]) -> Vec<AuditConflict> {
    let catalog = Catalog::v1();
    let entries = catalog
        .entries()
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut literals: BTreeMap<(String, usize, usize), Vec<RuleOccurrence<'_>>> = BTreeMap::new();
    for rule in rules {
        let Some(resolved) = &rule.resolved else {
            continue;
        };
        let Some(sort_key) = &rule.sort_key else {
            continue;
        };
        let properties = resolved
            .declarations
            .iter()
            .flat_map(|declaration| effective_write_set(&declaration.property))
            .collect::<BTreeSet<_>>();
        for origin in &rule.origins {
            let Origin::Source {
                source_id,
                literal_byte_offset,
                literal_byte_length,
                position_kind: SourcePositionKind::Class,
                ..
            } = origin
            else {
                continue;
            };
            literals
                .entry((
                    source_id.clone(),
                    *literal_byte_offset,
                    *literal_byte_length,
                ))
                .or_default()
                .push(RuleOccurrence {
                    rule,
                    origin,
                    properties: properties.clone(),
                    sort_key,
                });
        }
    }

    let mut conflicts = Vec::new();
    for occurrences in literals.values_mut() {
        occurrences.sort_by(|left, right| {
            left.sort_key
                .cmp(right.sort_key)
                .then(left.rule.candidate.cmp(&right.rule.candidate))
                .then_with(|| origin_position(left.origin).cmp(&origin_position(right.origin)))
        });
        for left_index in 0..occurrences.len() {
            for right_index in (left_index + 1)..occurrences.len() {
                let left = &occurrences[left_index];
                let right = &occurrences[right_index];
                if left.rule.candidate == right.rule.candidate
                    || left.rule.parsed.variants != right.rule.parsed.variants
                {
                    continue;
                }
                let default_overrides = left
                    .rule
                    .resolved
                    .as_ref()
                    .and_then(|resolved| entries.get(resolved.entry_id.as_str()))
                    .map(|entry| entry.default_overrides.as_slice())
                    .unwrap_or_default();
                let right_entry_id = right
                    .rule
                    .resolved
                    .as_ref()
                    .map(|resolved| resolved.entry_id.as_str())
                    .unwrap_or_default();
                let overlapping_properties = left
                    .properties
                    .intersection(&right.properties)
                    .filter(|property| {
                        !default_overrides
                            .iter()
                            .any(|default| default.matches(property, right_entry_id))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if overlapping_properties.is_empty() {
                    continue;
                }
                let (first, second, emitted_order) = if left.rule.candidate <= right.rule.candidate
                {
                    (
                        left,
                        right,
                        vec![left.rule.candidate.clone(), right.rule.candidate.clone()],
                    )
                } else {
                    (
                        right,
                        left,
                        vec![left.rule.candidate.clone(), right.rule.candidate.clone()],
                    )
                };
                let Some(first_resolved) = &first.rule.resolved else {
                    continue;
                };
                let Some(second_resolved) = &second.rule.resolved else {
                    continue;
                };
                conflicts.push(AuditConflict {
                    origin: origin_view(first.origin),
                    first_candidate: first.rule.candidate.clone(),
                    second_candidate: second.rule.candidate.clone(),
                    first_group: first_resolved.conflict_group.to_owned(),
                    second_group: second_resolved.conflict_group.to_owned(),
                    overlapping_properties,
                    emitted_order,
                });
            }
        }
    }
    conflicts
}

struct RuleOccurrence<'a> {
    rule: &'a RuleMetadata,
    origin: &'a Origin,
    properties: BTreeSet<String>,
    sort_key: &'a SortKey,
}

fn origin_position(origin: &Origin) -> (usize, usize) {
    match origin {
        Origin::Source {
            byte_offset,
            byte_length,
            ..
        } => (*byte_offset, *byte_length),
        _ => (0, 0),
    }
}

fn recognized_utility_shape(candidate: &str) -> bool {
    let Ok(split) = structural_split(candidate) else {
        return false;
    };
    let utility = split.slash.map_or(split.utility, |(before, _)| before);
    let utility_name = utility.strip_prefix('-').unwrap_or(utility);
    if ["ring", "animate", "scale", "transform"]
        .iter()
        .any(|root| utility_name == *root || utility_name.starts_with(&format!("{root}-")))
    {
        return true;
    }
    Catalog::v1().entries().iter().any(|entry| {
        utility_name == entry.root || utility_name.starts_with(&format!("{}-", entry.root))
    })
}

fn effective_write_set(property: &str) -> Vec<String> {
    let longhands: &[&str] = match property {
        "padding" => &[
            "padding-top",
            "padding-right",
            "padding-bottom",
            "padding-left",
        ],
        "margin" => &["margin-top", "margin-right", "margin-bottom", "margin-left"],
        "inset" => &["top", "right", "bottom", "left"],
        "gap" => &["row-gap", "column-gap"],
        "overflow" => &["overflow-x", "overflow-y"],
        "overscroll-behavior" => &["overscroll-behavior-x", "overscroll-behavior-y"],
        "border-width" => &[
            "border-top-width",
            "border-right-width",
            "border-bottom-width",
            "border-left-width",
        ],
        "border-style" => &[
            "border-top-style",
            "border-right-style",
            "border-bottom-style",
            "border-left-style",
        ],
        "border-color" => &[
            "border-top-color",
            "border-right-color",
            "border-bottom-color",
            "border-left-color",
        ],
        "border" => &[
            "border-top-width",
            "border-right-width",
            "border-bottom-width",
            "border-left-width",
            "border-top-style",
            "border-right-style",
            "border-bottom-style",
            "border-left-style",
            "border-top-color",
            "border-right-color",
            "border-bottom-color",
            "border-left-color",
        ],
        "border-radius" => &[
            "border-top-left-radius",
            "border-top-right-radius",
            "border-bottom-right-radius",
            "border-bottom-left-radius",
        ],
        "flex" => &["flex-grow", "flex-shrink", "flex-basis"],
        "place-items" => &["align-items", "justify-items"],
        "place-self" => &["align-self", "justify-self"],
        "place-content" => &["align-content", "justify-content"],
        _ => return vec![property.to_owned()],
    };
    longhands
        .iter()
        .map(|property| (*property).to_owned())
        .collect()
}

fn sort_report_inputs(report: &mut AuditReport) {
    report.dynamic_constructions.sort_by(|a, b| {
        a.source_id
            .cmp(&b.source_id)
            .then(a.byte_offset.cmp(&b.byte_offset))
            .then(a.text.cmp(&b.text))
    });
    report.adjacent_interpolations.sort_by(|a, b| {
        a.origin
            .source_id
            .cmp(&b.origin.source_id)
            .then(a.origin.byte_offset.cmp(&b.origin.byte_offset))
            .then(a.candidate.cmp(&b.candidate))
    });
    report.extraction_notes.sort_by(|a, b| {
        a.source_id
            .cmp(&b.source_id)
            .then(a.byte_offset.cmp(&b.byte_offset))
            .then(a.kind.cmp(&b.kind))
            .then(a.text.cmp(&b.text))
    });
}

fn sort_report_results(report: &mut AuditReport) {
    report.unrecognized_classes.sort_by(|a, b| {
        a.origin
            .source_id
            .cmp(&b.origin.source_id)
            .then(a.origin.byte_offset.cmp(&b.origin.byte_offset))
            .then(a.candidate.cmp(&b.candidate))
    });
    report.dead_classes.sort_by(|a, b| {
        let a_origin = a.diagnostic.origin.as_ref();
        let b_origin = b.diagnostic.origin.as_ref();
        a_origin
            .and_then(|origin| origin.source_id.as_ref())
            .cmp(&b_origin.and_then(|origin| origin.source_id.as_ref()))
            .then(
                a_origin
                    .and_then(|origin| origin.byte_offset)
                    .cmp(&b_origin.and_then(|origin| origin.byte_offset)),
            )
            .then(a.diagnostic.code.cmp(&b.diagnostic.code))
            .then(a.candidate.cmp(&b.candidate))
    });
    report.conflicts.sort_by(|a, b| {
        a.origin
            .source_id
            .cmp(&b.origin.source_id)
            .then(
                a.origin
                    .literal_byte_offset
                    .cmp(&b.origin.literal_byte_offset),
            )
            .then(a.first_candidate.cmp(&b.first_candidate))
            .then(a.second_candidate.cmp(&b.second_candidate))
            .then(a.overlapping_properties.cmp(&b.overlapping_properties))
    });
    report.diagnostics.sort_by(|a, b| {
        diagnostic_origin_order(a.origin.as_ref())
            .cmp(&diagnostic_origin_order(b.origin.as_ref()))
            .then(a.code.cmp(&b.code))
            .then(a.candidate.cmp(&b.candidate))
            .then(a.message.cmp(&b.message))
    });
    report.diagnostics.dedup_by(|left, right| {
        left.code == right.code
            && left.candidate == right.candidate
            && diagnostic_origin_order(left.origin.as_ref())
                == diagnostic_origin_order(right.origin.as_ref())
    });
    report.unrecognized_classes.dedup();
    report.dead_classes.dedup();
    report.conflicts.dedup();
}

fn diagnostic_origin_order(origin: Option<&OriginView>) -> (String, usize, String, String, usize) {
    let Some(origin) = origin else {
        return (String::new(), 0, "none".to_owned(), String::new(), 0);
    };
    match origin.kind.as_str() {
        "source" => (
            origin.source_id.clone().unwrap_or_default(),
            origin.byte_offset.unwrap_or_default(),
            origin.kind.clone(),
            String::new(),
            0,
        ),
        "safelist" => (
            origin.owner.clone().unwrap_or_default(),
            origin.index.unwrap_or_default(),
            origin.kind.clone(),
            String::new(),
            0,
        ),
        "manifest" => (
            origin.producer.clone().unwrap_or_default(),
            origin.index.unwrap_or_default(),
            origin.kind.clone(),
            origin.path.clone().unwrap_or_default(),
            0,
        ),
        "config" => (
            origin.key_path.clone().unwrap_or_default(),
            0,
            origin.kind.clone(),
            String::new(),
            0,
        ),
        "roleClass" => (
            origin.role_key.clone().unwrap_or_default(),
            0,
            origin.kind.clone(),
            String::new(),
            0,
        ),
        "stylesheet" => (
            origin.path.clone().unwrap_or_default(),
            origin.byte_offset.unwrap_or_default(),
            origin.kind.clone(),
            String::new(),
            0,
        ),
        _ => (String::new(), 0, origin.kind.clone(), String::new(), 0),
    }
}

fn render_section(output: &mut String, title: &str, items: impl IntoIterator<Item = String>) {
    output.push_str(&format!("{title}:\n"));
    let mut any = false;
    for item in items {
        any = true;
        output.push_str(&format!("  - {item}\n"));
    }
    if !any {
        output.push_str("  (none)\n");
    }
}

fn note_kind_name(kind: &NoteKind) -> &'static str {
    match kind {
        NoteKind::DynamicConstruction => "dynamicConstruction",
        NoteKind::MalformedClassCandidate => "malformedClassCandidate",
        NoteKind::InvalidUtf8 => "invalidUtf8",
        NoteKind::UnterminatedLiteral => "unterminatedLiteral",
    }
}

fn audit_outcome_name(outcome: AuditOutcome) -> &'static str {
    match outcome {
        AuditOutcome::Complete => "complete",
        AuditOutcome::InvalidConfiguration => "invalid configuration",
        AuditOutcome::GenerationDisabled => "generation disabled",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::{extract_candidates, SourceKind, TokenConfig};

    use super::*;

    #[test]
    fn audit_reports_unknown_allowlisted_marker_conflict_dead_dynamic_and_ignores_low_confidence() {
        let classes = extract_candidates(
            br#"<div class="made-up prose group p-4 px-2 p-missing"></div>"#,
            SourceKind::Html,
        );
        let dynamic = extract_candidates(b"const name = `bg-${color}`;", SourceKind::Ts);
        let low_confidence =
            extract_candidates(br#"import value from "made-up-module";"#, SourceKind::Ts);
        let input = AuditInput::new(vec![
            AuditSource::new("src/page.html", classes),
            AuditSource::new("src/page.ts", dynamic),
            AuditSource::new("src/entry.ts", low_confidence),
        ]);
        let config = WindConfig {
            tokens: TokenConfig {
                spacing_unit: Some("0.25rem".to_owned()),
                ..TokenConfig::default()
            },
            authored_classes: BTreeMap::from([("prose".to_owned(), true)]),
            ..WindConfig::default()
        };

        let report = audit(&input, &config);
        assert_eq!(report.outcome, AuditOutcome::Complete);
        assert_eq!(
            report
                .unrecognized_classes
                .iter()
                .map(|class| class.candidate.as_str())
                .collect::<Vec<_>>(),
            ["made-up"]
        );
        assert!(report
            .unrecognized_classes
            .iter()
            .all(|class| class.candidate != "prose" && class.candidate != "group"));
        assert_eq!(report.conflicts.len(), 1);
        assert_eq!(
            report.conflicts[0].overlapping_properties,
            ["padding-left", "padding-right"]
        );
        assert_eq!(report.dead_classes.len(), 1);
        assert_eq!(report.dead_classes[0].candidate, "p-missing");
        assert_eq!(report.dynamic_constructions.len(), 1);
        assert_eq!(report.dynamic_constructions[0].text, "bg-");
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "ZW012" && diagnostic.severity == "auditInfo"
        }));
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "ZW013" && diagnostic.severity == "auditInfo"
        }));
        assert!(!report
            .unrecognized_classes
            .iter()
            .any(|class| class.candidate == "made-up-module"));
    }

    #[test]
    fn catalog_default_overrides_do_not_hide_genuine_conflicts() {
        let markup = br#"<div class="border border-dashed"></div>
<div class="border-2 border-dotted"></div>
<div class="border-b border-dashed"></div>
<div class="outline-2 outline-dashed"></div>
<div class="transition-opacity duration-300 ease-[ease-in]"></div>
<div class="transition-colors duration-500"></div>
<div class="duration-300 transition-all"></div>
<div class="hover:border hover:border-dashed"></div>
<div class="hover:transition-opacity hover:duration-300"></div>
<div class="border-dashed border-dotted"></div>
<div class="duration-300 duration-500"></div>
<div class="transition-opacity transition-all"></div>
<div class="sr-only p-4"></div>
<div class="border border-b-2"></div>
<div class="border hover:border-dashed"></div>"#;
        let input = AuditInput::single(
            "src/defaults.html",
            extract_candidates(markup, SourceKind::Html),
        );
        let config = WindConfig {
            tokens: TokenConfig {
                spacing_unit: Some("0.25rem".to_owned()),
                ..TokenConfig::default()
            },
            ..WindConfig::default()
        };
        let report = audit(&input, &config);
        assert!(report
            .diagnostics
            .iter()
            .all(|item| item.severity != "error"));
        let conflicts = report
            .conflicts
            .iter()
            .map(|conflict| {
                (
                    conflict.first_candidate.as_str(),
                    conflict.second_candidate.as_str(),
                    conflict
                        .overlapping_properties
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            conflicts,
            [
                (
                    "border-dashed",
                    "border-dotted",
                    vec![
                        "border-bottom-style",
                        "border-left-style",
                        "border-right-style",
                        "border-top-style"
                    ]
                ),
                ("duration-300", "duration-500", vec!["transition-duration"]),
                (
                    "transition-all",
                    "transition-opacity",
                    vec![
                        "transition-duration",
                        "transition-property",
                        "transition-timing-function"
                    ]
                ),
                (
                    "p-4",
                    "sr-only",
                    vec![
                        "padding-bottom",
                        "padding-left",
                        "padding-right",
                        "padding-top"
                    ]
                ),
                (
                    "border",
                    "border-b-2",
                    vec!["border-bottom-style", "border-bottom-width"]
                ),
            ]
        );
        assert_eq!(
            report
                .diagnostics
                .iter()
                .filter(|item| item.code == "ZW013")
                .count(),
            conflicts.len()
        );
        let json: serde_json::Value = serde_json::from_str(&audit_json(&report).unwrap()).unwrap();
        assert_eq!(json["conflicts"].as_array().unwrap().len(), conflicts.len());
        assert_eq!(
            json["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["code"] == "ZW013")
                .count(),
            conflicts.len()
        );
        let rendered = render_audit(&report);
        assert_eq!(rendered.matches(" overlap [").count(), conflicts.len());
        assert_eq!(rendered.matches("ZW013 auditInfo").count(), conflicts.len());

        let reversed = br#"<div class="duration-300 transition-opacity border-dashed border outline-dashed outline-2 ease-[ease-in]"></div>"#;
        let forward = br#"<div class="transition-opacity duration-300 border border-dashed outline-2 outline-dashed ease-[ease-in]"></div>"#;
        for markup in [forward.as_slice(), reversed.as_slice()] {
            let report = audit(
                &AuditInput::single(
                    "src/order.html",
                    extract_candidates(markup, SourceKind::Html),
                ),
                &config,
            );
            assert!(report.conflicts.is_empty());
            assert!(report
                .diagnostics
                .iter()
                .all(|item| item.severity != "error"));
            assert!(!render_audit(&report).contains("ZW013"));
            let json: serde_json::Value =
                serde_json::from_str(&audit_json(&report).unwrap()).unwrap();
            assert_eq!(json["conflicts"], serde_json::json!([]));
        }
        let css_for = |markup: &[u8]| {
            let extraction = extract_candidates(markup, SourceKind::Html);
            let candidates = extraction
                .candidates
                .iter()
                .flat_map(|candidate| {
                    candidate
                        .occurrences
                        .iter()
                        .map(|occurrence| OriginCandidate {
                            text: candidate.text.clone(),
                            origin: source_origin("src/order.html", occurrence),
                        })
                })
                .collect();
            let result = crate::compile(&crate::CompileInput {
                candidates,
                config: config.clone(),
            });
            assert!(!result.has_errors());
            result.stylesheet
        };
        assert_eq!(css_for(forward), css_for(reversed));

        // A future rule that also writes a genuine shared property must keep
        // that property visible after its designated default is removed.
        let extraction = extract_candidates(
            br#"<div class="border-b border-dashed"></div>"#,
            SourceKind::Html,
        );
        let candidates = extraction
            .candidates
            .iter()
            .flat_map(|candidate| {
                candidate
                    .occurrences
                    .iter()
                    .map(|occurrence| OriginCandidate {
                        text: candidate.text.clone(),
                        origin: source_origin("src/mixed.html", occurrence),
                    })
            })
            .collect();
        let mut compiled = crate::compile(&crate::CompileInput {
            candidates,
            config: config.clone(),
        });
        for rule in &mut compiled.rules {
            rule.resolved
                .as_mut()
                .unwrap()
                .declarations
                .push(crate::Declaration {
                    property: "border-top-color".to_owned(),
                    value: "red".to_owned(),
                });
        }
        let mixed = find_conflicts(&compiled.rules);
        assert_eq!(mixed.len(), 1);
        assert_eq!(mixed[0].overlapping_properties, ["border-top-color"]);
    }

    #[test]
    fn audit_order_is_independent_of_source_arrival_and_disabled_is_explicit() {
        let first = AuditSource::new(
            "a.tsx",
            extract_candidates(br#"<div class="site-header"></div>"#, SourceKind::Tsx),
        );
        let second = AuditSource::new(
            "b.html",
            extract_candidates(br#"<p class="other"></p>"#, SourceKind::Html),
        );
        let one = audit(
            &AuditInput::new(vec![first.clone(), second.clone()]),
            &WindConfig::default(),
        );
        let two = audit(
            &AuditInput::new(vec![second, first]),
            &WindConfig::default(),
        );
        assert_eq!(one, two);
        assert_eq!(render_audit(&one), render_audit(&two));
        assert_eq!(audit_json(&one).unwrap(), audit_json(&two).unwrap());
        let disabled = audit(
            &AuditInput {
                sources: Vec::new(),
                generation_enabled: false,
            },
            &WindConfig::default(),
        );
        assert_eq!(disabled.outcome, AuditOutcome::GenerationDisabled);
        assert!(disabled.unrecognized_classes.is_empty());
    }

    #[test]
    fn grouped_audit_summarizes_full_candidates_by_first_origin_and_severity() {
        let empty = audit(&AuditInput::default(), &WindConfig::default());
        assert_eq!(
            render_audit(&empty),
            format!(
                "outcome: complete\n\
                 spec: {} revision {}\n\
                 unrecognized classes:\n  (none)\n\
                 conflicts:\n  (none)\n\
                 dead classes:\n  (none)\n\
                 dynamic constructions:\n  (none)\n\
                 tokens adjacent to interpolation:\n  (none)\n\
                 diagnostics:\n  (none)\n\
                 extraction notes:\n  (none)\n",
                SPEC_VERSION, SPEC_REVISION
            )
        );
        let grouped_empty = render_audit_grouped(&empty);
        assert_eq!(grouped_empty, render_audit(&empty));
        for section in [
            "unrecognized classes",
            "conflicts",
            "dead classes",
            "dynamic constructions",
            "tokens adjacent to interpolation",
            "diagnostics",
            "extraction notes",
        ] {
            assert!(
                grouped_empty.contains(&format!("{section}:\n  (none)\n")),
                "missing empty section {section:?}:\n{grouped_empty}"
            );
        }

        const FIRST_SOURCE: &[u8] = b"const saved = 'rounded-lg';\nexport default () => <div class=\"panel-card panel-card rounded-lg rounded-lg hover:rounded-lg\" />;\n";
        const SECOND_SOURCE: &[u8] =
            br#"export default () => <div class="panel-card rounded-lg" />;"#;
        let first_source =
            AuditSource::new("a.tsx", extract_candidates(FIRST_SOURCE, SourceKind::Tsx));
        let second_source =
            AuditSource::new("z.tsx", extract_candidates(SECOND_SOURCE, SourceKind::Tsx));
        // Reverse source arrival so first-origin selection is proven to use
        // stable source order rather than caller order.
        let report = audit(
            &AuditInput::new(vec![second_source, first_source]),
            &WindConfig::default(),
        );
        let grouped = render_audit_grouped(&report);
        assert!(
            grouped.contains("panel-card x3, first at a.tsx:2:"),
            "{grouped}"
        );
        assert!(
            grouped.contains("hover:rounded-lg [ZW006 error] x1, first at a.tsx:2:"),
            "{grouped}"
        );
        assert!(
            grouped.contains("rounded-lg [ZW006 error] x3, first at a.tsx:2:"),
            "{grouped}"
        );
        assert!(
            grouped.contains("rounded-lg [ZW006 auditInfo] x1, first at a.tsx:1:"),
            "{grouped}"
        );
        assert_eq!(grouped.matches("panel-card x3,").count(), 1);

        // Default text keeps the occurrence-oriented text contract.
        let default = render_audit(&report);
        assert_eq!(default.matches("panel-card at ").count(), 3, "{default}");
        assert!(
            !default.contains("panel-card x3,"),
            "default report was grouped:\n{default}"
        );

        let reversed = audit(
            &AuditInput::new(vec![
                AuditSource::new("a.tsx", extract_candidates(FIRST_SOURCE, SourceKind::Tsx)),
                AuditSource::new("z.tsx", extract_candidates(SECOND_SOURCE, SourceKind::Tsx)),
            ]),
            &WindConfig::default(),
        );
        assert_eq!(grouped, render_audit_grouped(&reversed));

        let mut provenance_report = empty.clone();
        provenance_report.diagnostics = vec![
            DiagnosticView {
                severity: "error".to_owned(),
                code: "ZW014".to_owned(),
                candidate: Some("panel-card".to_owned()),
                origin: Some(OriginView {
                    kind: "source".to_owned(),
                    source_id: Some("z.tsx".to_owned()),
                    byte_offset: Some(40),
                    line: Some(2),
                    byte_column: Some(10),
                    ..OriginView::default()
                }),
                message: "later diagnostic message".to_owned(),
                suggestion: Some("later suggestion".to_owned()),
                rejection_id: None,
            },
            DiagnosticView {
                severity: "error".to_owned(),
                code: "ZW014".to_owned(),
                candidate: Some("panel-card".to_owned()),
                origin: Some(OriginView {
                    kind: "source".to_owned(),
                    source_id: Some("a.tsx".to_owned()),
                    byte_offset: Some(20),
                    line: Some(1),
                    byte_column: Some(6),
                    ..OriginView::default()
                }),
                message: "first diagnostic message".to_owned(),
                suggestion: Some("first suggestion".to_owned()),
                rejection_id: None,
            },
        ];
        let provenance = render_audit_grouped(&provenance_report);
        assert!(
            provenance.contains(
                "panel-card [ZW014 error] x2, first at a.tsx:1:6: first diagnostic message; suggested spelling: first suggestion"
            ),
            "{provenance}"
        );
        assert!(
            !provenance.contains("later diagnostic message"),
            "{provenance}"
        );
    }

    #[test]
    fn audit_snapshots_host_token_overrides_as_deterministic_audit_info() {
        let overrides = [
            TokenOverride {
                category: crate::TokenCategory::Spacing,
                name: "gutter".to_owned(),
                preset_index: 1,
                previous_value: serde_json::json!("1rem"),
                final_value: serde_json::json!("2rem"),
            },
            TokenOverride {
                category: crate::TokenCategory::Color,
                name: "bg".to_owned(),
                preset_index: 0,
                previous_value: serde_json::json!("#fff"),
                final_value: serde_json::json!("#f0f"),
            },
        ];
        let report =
            audit_with_token_overrides(&AuditInput::default(), &WindConfig::default(), &overrides);
        assert_eq!(report.outcome, AuditOutcome::Complete);
        assert_eq!(report.diagnostics.len(), 2);
        assert_eq!(
            report
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.candidate.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["colors.bg", "spacing.gutter"]
        );
        assert!(report.diagnostics.iter().all(|diagnostic| {
            diagnostic.code == "ZW015" && diagnostic.severity == "auditInfo"
        }));

        let rendered = render_audit(&report);
        assert!(rendered.contains(
            "ZW015 auditInfo at wind.tokens.colors.bg: colors.bg: host value overrides preset[0]: \"#fff\" -> \"#f0f\"\n"
        ), "{rendered}");
        let grouped = render_audit_grouped(&report);
        assert!(grouped.contains(
            "colors.bg [ZW015 auditInfo] x1, first at wind.tokens.colors.bg: host value overrides preset[0]: \"#fff\" -> \"#f0f\""
        ));
        let json: serde_json::Value = serde_json::from_str(&audit_json(&report).unwrap()).unwrap();
        assert_eq!(json["diagnostics"][0]["code"], "ZW015");
        assert_eq!(json["diagnostics"][0]["severity"], "auditInfo");
        assert_eq!(json["diagnostics"][0]["candidate"], "colors.bg");
        assert_eq!(
            json["diagnostics"][0]["origin"]["keyPath"],
            "wind.tokens.colors.bg"
        );
        assert_eq!(
            json["diagnostics"][0]["message"],
            "host value overrides preset[0]: \"#fff\" -> \"#f0f\""
        );
        assert!(json["diagnostics"][0].get("sourceId").is_none());

        let reversed = audit_with_token_overrides(
            &AuditInput::default(),
            &WindConfig::default(),
            &[overrides[1].clone(), overrides[0].clone()],
        );
        assert_eq!(audit_json(&report).unwrap(), audit_json(&reversed).unwrap());
        let no_overrides = audit(&AuditInput::default(), &WindConfig::default());
        assert!(no_overrides
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "ZW015"));
    }

    #[test]
    fn audit_expands_shorthand_writes_across_groups_and_keeps_interpolation_notes() {
        let classes = extract_candidates(br#"<div class="sr-only p-4"></div>"#, SourceKind::Html);
        let interpolation = extract_candidates(
            b"const name = `last:border-b-0${suffix}`; const other = `bg-${color}`;",
            SourceKind::Ts,
        );
        let report = audit(
            &AuditInput::new(vec![
                AuditSource::new("src/page.html", classes),
                AuditSource::new("src/page.ts", interpolation),
            ]),
            &WindConfig {
                tokens: TokenConfig {
                    spacing_unit: Some("0.25rem".to_owned()),
                    ..TokenConfig::default()
                },
                ..WindConfig::default()
            },
        );
        assert!(report.conflicts.iter().any(|conflict| {
            conflict.first_group != conflict.second_group
                && conflict
                    .overlapping_properties
                    .contains(&"padding-left".to_owned())
        }));
        assert!(report
            .adjacent_interpolations
            .iter()
            .any(|item| item.candidate == "last:border-b-0"));
        assert!(report
            .dynamic_constructions
            .iter()
            .any(|item| item.text == "bg-"));
    }

    #[test]
    fn alignment_shorthand_conflicts_expand_to_longhands_and_keep_disjoint_pairs_clear() {
        let source = extract_candidates(
            br#"<div class="place-self-center self-start justify-self-end place-content-between justify-center items-center justify-items-end place-items-start"></div>"#,
            SourceKind::Html,
        );
        let report = audit(
            &AuditInput::single("src/page.html", source),
            &WindConfig::default(),
        );
        let conflicts = report
            .conflicts
            .iter()
            .map(|conflict| {
                (
                    conflict.first_candidate.as_str(),
                    conflict.second_candidate.as_str(),
                    conflict
                        .overlapping_properties
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(
            conflicts,
            std::collections::BTreeSet::from([
                (
                    "justify-center",
                    "place-content-between",
                    vec!["justify-content"]
                ),
                (
                    "justify-self-end",
                    "place-self-center",
                    vec!["justify-self"]
                ),
                (
                    "justify-items-end",
                    "place-items-start",
                    vec!["justify-items"]
                ),
                ("items-center", "place-items-start", vec!["align-items"]),
                ("place-self-center", "self-start", vec!["align-self"]),
            ])
        );
        assert!(!report.conflicts.iter().any(|conflict| {
            (conflict.first_candidate == "place-content-between"
                && conflict.second_candidate == "items-center")
                || (conflict.first_candidate == "items-center"
                    && conflict.second_candidate == "place-content-between")
        }));
        assert!(!report.conflicts.iter().any(|conflict| {
            (conflict.first_candidate == "place-items-start"
                && conflict.second_candidate == "place-self-center")
                || (conflict.first_candidate == "place-self-center"
                    && conflict.second_candidate == "place-items-start")
        }));
    }

    #[test]
    fn an_unconfigured_breakpoint_on_a_known_utility_is_a_dead_class() {
        let source = extract_candidates(br#"<div class="sm:block"></div>"#, SourceKind::Html);
        let report = audit(
            &AuditInput::single("src/page.html", source),
            &WindConfig::default(),
        );
        assert_eq!(report.dead_classes.len(), 1);
        assert_eq!(report.dead_classes[0].candidate, "sm:block");
        assert_eq!(report.dead_classes[0].diagnostic.code, "ZW002");
    }

    #[test]
    fn audit_locations_are_lines_and_byte_columns_not_offsets() {
        let source = "// 日本語のコメント\r\nconst a = 1;\r\nexport const x = <div class=\"rounded-lg md:block\" />;\r\nconst t = `${a}-suffix`;\r\n";
        let extraction = crate::extract_candidates(source.as_bytes(), crate::SourceKind::Tsx);
        let report = audit(
            &AuditInput::new(vec![AuditSource::new("default/src:a.tsx", extraction)]),
            &WindConfig::default(),
        );
        let rendered = render_audit(&report);
        let column = "export const x = <div class=\"".len() + 1;
        assert!(
            rendered.contains(&format!("default/src:a.tsx:3:{column}: rounded-lg: ")),
            "{rendered}"
        );
        let offset = source.find("rounded-lg").unwrap();
        assert!(!rendered.contains(&format!("a.tsx:{offset}")), "{rendered}");
        let diagnostic = report
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.candidate.as_deref() == Some("rounded-lg"))
            .unwrap();
        let origin = diagnostic.origin.as_ref().unwrap();
        assert_eq!((origin.line, origin.byte_column), (Some(3), Some(column)));
        assert_eq!(origin.byte_offset, Some(offset));
        for note in &report.extraction_notes {
            assert!(note.line >= 1 && note.byte_column >= 1, "{note:?}");
            let line_start = source[..note.byte_offset].rfind('\n').map_or(0, |i| i + 1);
            assert_eq!(
                note.byte_column,
                note.byte_offset - line_start + 1,
                "{note:?}"
            );
        }
    }
}
