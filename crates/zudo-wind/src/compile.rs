use std::collections::BTreeMap;

use crate::{
    parse_candidate, Candidate, Catalog, Diagnostic, Origin, Resolution, ResolvedRule, Severity,
    SortKey, SourcePositionKind, Specificity, ValidatedWindConfig, WindConfig,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OriginCandidate {
    pub text: String,
    pub origin: Origin,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompileInput {
    pub candidates: Vec<OriginCandidate>,
    pub config: WindConfig,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuleKind {
    Utility,
    Marker,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuleMetadata {
    pub candidate: String,
    pub parsed: Candidate,
    pub kind: RuleKind,
    /// None for a marker, which emits no selector or declarations.
    pub resolved: Option<ResolvedRule>,
    pub selector: Option<String>,
    pub conditions: Vec<String>,
    pub sort_key: Option<SortKey>,
    pub specificity: Specificity,
    /// All distinct occurrences, sorted independently of source arrival order.
    pub origins: Vec<Origin>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProvenanceKind {
    Generated,
}

/// Generated stylesheet attribution, separate from candidate source locations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedProvenance {
    pub source_id: &'static str,
    pub kind: ProvenanceKind,
    pub spec_version: u32,
    pub spec_revision: u32,
    /// V1 does not generate a source map.
    pub map: Option<String>,
}

/// The integration seam: authored CSS belongs after registrations and before
/// utilities. Highlight/import placement remains the consuming pipeline's job.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StylesheetParts {
    pub prelude: String,
    pub reset: String,
    pub tokens: String,
    pub registrations: String,
    pub utilities: String,
}

impl StylesheetParts {
    pub fn stylesheet(&self) -> String {
        [
            &self.prelude,
            &self.reset,
            &self.tokens,
            &self.registrations,
            &self.utilities,
        ]
        .into_iter()
        .map(String::as_str)
        .collect()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompileResult {
    pub stylesheet: String,
    pub parts: StylesheetParts,
    pub diagnostics: Vec<Diagnostic>,
    /// Utilities in emission order, followed by markers in candidate byte order.
    pub rules: Vec<RuleMetadata>,
    pub ordinary_classes: Vec<OriginCandidate>,
    pub authored_classes: Vec<OriginCandidate>,
    pub provenance: Option<GeneratedProvenance>,
}

impl CompileResult {
    /// Callers must not publish partial CSS when this returns true.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

/// Validates configuration and compiles explicit candidates plus the configured
/// safelist. Returns useful diagnostics and metadata even when candidates fail.
pub fn compile(input: &CompileInput) -> CompileResult {
    let mut config = input.config.clone();
    let mut candidates = input.candidates.clone();
    // Validate owner keys here, but parse safelist candidates alongside other
    // origins so authored allowlists apply and one bad entry cannot hide peers.
    for (owner, entries) in &mut config.safelist {
        for (index, text) in std::mem::take(entries).into_iter().enumerate() {
            candidates.push(OriginCandidate {
                text,
                origin: Origin::Safelist {
                    owner: owner.clone(),
                    index,
                },
            });
        }
    }
    match config.validate() {
        Ok(config) => compile_validated(&candidates, &config),
        Err(mut diagnostics) => {
            sort_diagnostics(&mut diagnostics);
            CompileResult {
                diagnostics,
                ..CompileResult::default()
            }
        }
    }
}

/// Compiles with an already validated configuration, including its safelist.
/// No I/O, global mutable state, minification or source-map generation occurs.
pub fn compile_validated(
    candidates: &[OriginCandidate],
    config: &ValidatedWindConfig,
) -> CompileResult {
    let mut inputs = candidates.to_vec();
    for (owner, candidates) in &config.safelist {
        for (index, candidate) in candidates.iter().enumerate() {
            inputs.push(OriginCandidate {
                text: candidate.raw.clone(),
                origin: Origin::Safelist {
                    owner: owner.clone(),
                    index,
                },
            });
        }
    }
    sort_occurrences(&mut inputs);
    let catalog = Catalog::v1();
    let mut result = CompileResult::default();
    let mut rules: BTreeMap<String, RuleMetadata> = BTreeMap::new();
    for input in inputs {
        if config.authored_classes.contains(&input.text) {
            result.authored_classes.push(input);
            continue;
        }
        let candidate = match parse_candidate(&input.text, &config.vocabulary) {
            Ok(candidate) => candidate,
            Err(mut diagnostic) => {
                if matches!(input.origin, Origin::RoleClass { .. }) {
                    result.ordinary_classes.push(input);
                    continue;
                }
                if matches!(
                    input.origin,
                    Origin::Source {
                        position_kind: SourcePositionKind::Literal,
                        ..
                    }
                ) {
                    diagnostic.severity = Severity::AuditInfo;
                }
                diagnostic.origin = Some(Box::new(input.origin));
                result.diagnostics.push(diagnostic);
                continue;
            }
        };
        if let Some(rule) = rules.get_mut(&input.text) {
            rule.origins.push(input.origin);
            continue;
        }
        let mut metadata = RuleMetadata {
            candidate: input.text.clone(),
            parsed: candidate.clone(),
            kind: RuleKind::Marker,
            resolved: None,
            selector: None,
            conditions: Vec::new(),
            sort_key: None,
            specificity: Specificity::default(),
            origins: vec![input.origin.clone()],
        };
        if input.text == "group" || input.text == "peer" {
            rules.insert(input.text, metadata);
            continue;
        }
        match catalog.resolve(
            &candidate,
            &config.tokens,
            &input.origin,
            &config.authored_classes,
        ) {
            Resolution::Rule(mut rule) => {
                if rule.root == "transition" {
                    if let Some(declaration) = rule
                        .declarations
                        .iter_mut()
                        .find(|declaration| declaration.property == "transition-timing-function")
                    {
                        declaration.value = config.default_transition_timing_function.clone();
                        if config.default_transition_timing_function_status
                            == crate::ValueStatus::CategoryUnverified
                        {
                            rule.value_status = crate::ValueStatus::CategoryUnverified;
                        }
                    }
                }
                let selector = crate::selector::build(&candidate, rule.selector_shape, config);
                metadata.kind = RuleKind::Utility;
                metadata.selector = Some(selector.text);
                metadata.conditions = selector.conditions;
                metadata.specificity = selector.specificity;
                metadata.sort_key =
                    Some(crate::order::sort_key(&candidate, &rule, config, &catalog));
                metadata.resolved = Some(rule);
                rules.insert(input.text, metadata);
            }
            Resolution::Diagnostic(mut diagnostic) => {
                if config.strict
                    && diagnostic.code == crate::DiagnosticCode::Zw014
                    && diagnostic.severity == Severity::Warning
                    && matches!(
                        input.origin,
                        Origin::Source {
                            position_kind: SourcePositionKind::Class,
                            ..
                        }
                    )
                {
                    diagnostic.severity = Severity::Error;
                }
                result.diagnostics.push(diagnostic)
            }
            Resolution::Failure(diagnostic) => result.diagnostics.push(diagnostic),
            Resolution::NotUtility => result.ordinary_classes.push(input),
        }
    }
    result.rules = rules.into_values().collect();
    result.rules.sort_by(|a, b| {
        a.sort_key
            .is_none()
            .cmp(&b.sort_key.is_none())
            .then_with(|| a.sort_key.cmp(&b.sort_key))
            .then_with(|| a.candidate.cmp(&b.candidate))
    });
    sort_diagnostics(&mut result.diagnostics);
    result.parts = crate::emit::stylesheet(config, &result.rules);
    result.stylesheet = result.parts.stylesheet();
    if !result.stylesheet.is_empty() {
        result.provenance = Some(GeneratedProvenance {
            source_id: "zudo-wind://spec/1",
            kind: ProvenanceKind::Generated,
            spec_version: crate::SPEC_VERSION,
            spec_revision: crate::SPEC_REVISION,
            map: None,
        });
    }
    result
}

// The origin id leads, followed by numeric position/index. Remaining fields
// only break ties (e.g. two class spans at the same source position).
fn origin_key(origin: &Origin) -> (&str, usize, u8) {
    let (id, position, kind) = match origin {
        Origin::Source {
            source_id,
            byte_offset,
            ..
        } => (source_id.as_str(), *byte_offset, 0),
        Origin::Safelist { owner, index } => (owner.as_str(), *index, 1),
        Origin::Manifest {
            producer, index, ..
        } => (producer.as_str(), *index, 2),
        Origin::Config { key_path } => (key_path.as_str(), 0, 3),
        Origin::RoleClass { role_key } => (role_key.as_str(), 0, 4),
        Origin::Stylesheet {
            path, byte_offset, ..
        } => (path.as_str(), *byte_offset, 5),
    };
    (id, position, kind)
}

fn sort_occurrences(inputs: &mut Vec<OriginCandidate>) {
    inputs.sort_by(|a, b| {
        origin_key(&a.origin)
            .cmp(&origin_key(&b.origin))
            .then_with(|| a.text.cmp(&b.text))
            .then_with(|| format!("{:?}", a.origin).cmp(&format!("{:?}", b.origin)))
    });
    inputs.dedup();
}

fn sort_diagnostics(diagnostics: &mut Vec<Diagnostic>) {
    diagnostics.sort_by(|a, b| {
        a.origin
            .as_deref()
            .map(origin_key)
            .cmp(&b.origin.as_deref().map(origin_key))
            .then_with(|| (a.code as u8).cmp(&(b.code as u8)))
            .then_with(|| a.candidate.cmp(&b.candidate))
            .then_with(|| format!("{:?}", a.origin).cmp(&format!("{:?}", b.origin)))
    });
    diagnostics
        .dedup_by(|a, b| a.code == b.code && a.candidate == b.candidate && a.origin == b.origin);
}
