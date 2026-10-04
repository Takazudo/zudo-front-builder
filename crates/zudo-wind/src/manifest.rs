//! Package candidate manifests: classify extracted occurrences with the
//! compiler that consumes them, so a manifest never holds a candidate the
//! consumer's build would reject.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::explain::{diagnostic_view, origin_view};
use crate::{
    compile, CompileInput, DiagnosticView, OriginCandidate, OriginView, RuleKind, WindConfig,
};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestClassification {
    /// Resolved utilities and markers, sorted and deduplicated: the manifest body.
    pub candidates: Vec<String>,
    /// The `group`/`peer` markers among `candidates`.
    pub markers: Vec<String>,
    /// Exact `authoredClasses` matches, which never enter a manifest.
    pub authored_classes: Vec<ExcludedCandidate>,
    /// Words that are not utilities, which never enter a manifest.
    pub ordinary_classes: Vec<ExcludedCandidate>,
    /// Rejected candidates. An error fails generation; warnings and
    /// audit-only low-confidence words are reported and left out.
    pub diagnostics: Vec<DiagnosticView>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcludedCandidate {
    pub candidate: String,
    pub origins: Vec<OriginView>,
}

impl ManifestClassification {
    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == "error")
            .count()
    }
}

/// Classifies source occurrences in one compile. The configured safelist
/// belongs to the configuration, not the sources, so it is not classified.
pub fn classify_manifest_candidates(
    candidates: &[OriginCandidate],
    config: &WindConfig,
) -> ManifestClassification {
    let mut config = config.clone();
    config.safelist.clear();
    let compiled = compile(&CompileInput {
        candidates: candidates.to_vec(),
        config,
    });
    let candidates: BTreeSet<_> = compiled
        .rules
        .iter()
        .map(|rule| rule.candidate.clone())
        .collect();
    let markers = compiled
        .rules
        .iter()
        .filter(|rule| rule.kind == RuleKind::Marker)
        .map(|rule| rule.candidate.clone())
        .collect::<BTreeSet<_>>();
    ManifestClassification {
        candidates: candidates.into_iter().collect(),
        markers: markers.into_iter().collect(),
        authored_classes: excluded(&compiled.authored_classes),
        ordinary_classes: excluded(&compiled.ordinary_classes),
        diagnostics: compiled.diagnostics.iter().map(diagnostic_view).collect(),
    }
}

fn excluded(occurrences: &[OriginCandidate]) -> Vec<ExcludedCandidate> {
    let mut grouped: BTreeMap<&str, Vec<OriginView>> = BTreeMap::new();
    for occurrence in occurrences {
        grouped
            .entry(&occurrence.text)
            .or_default()
            .push(origin_view(&occurrence.origin));
    }
    grouped
        .into_iter()
        .map(|(candidate, origins)| ExcludedCandidate {
            candidate: candidate.to_owned(),
            origins,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Origin, SourcePositionKind};

    fn at(text: &str, offset: usize, position_kind: SourcePositionKind) -> OriginCandidate {
        OriginCandidate {
            text: text.to_owned(),
            origin: Origin::Source {
                source_id: "src:a.tsx".to_owned(),
                byte_offset: offset,
                byte_length: text.len(),
                line: 1,
                byte_column: offset + 1,
                literal_byte_offset: offset,
                literal_byte_length: text.len(),
                position_kind,
            },
        }
    }

    fn config() -> WindConfig {
        let mut config = WindConfig::default();
        config.tokens.colors.insert("surface".into(), "#fff".into());
        config.authored_classes.insert("card".into(), true);
        config
            .safelist
            .insert("app".into(), vec!["bg-unknown".into()]);
        config
    }

    #[test]
    fn keeps_resolved_utilities_and_markers_sorted_once() {
        let class = SourcePositionKind::Class;
        let result = classify_manifest_candidates(
            &[
                at("flex", 30, class),
                at("bg-surface", 0, class),
                at("group", 20, class),
                at("flex", 40, class),
                at("card", 50, class),
                at("hello", 60, class),
            ],
            &config(),
        );
        assert_eq!(result.candidates, ["bg-surface", "flex", "group"]);
        assert_eq!(result.markers, ["group"]);
        assert_eq!(result.authored_classes[0].candidate, "card");
        assert_eq!(result.ordinary_classes[0].candidate, "hello");
        assert_eq!(result.ordinary_classes[0].origins[0].byte_offset, Some(60));
        assert_eq!(
            result.error_count(),
            0,
            "safelist is not classified: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn class_position_invalid_fails_and_literal_invalid_is_reported_only() {
        let class = classify_manifest_candidates(
            &[at("bg-missing", 0, SourcePositionKind::Class)],
            &config(),
        );
        assert_eq!(class.error_count(), 1);
        assert!(class.candidates.is_empty());
        assert_eq!(
            class.diagnostics[0].candidate.as_deref(),
            Some("bg-missing")
        );
        assert_eq!(
            class.diagnostics[0]
                .origin
                .as_ref()
                .and_then(|origin| origin.line),
            Some(1)
        );

        let literal = classify_manifest_candidates(
            &[at("bg-missing", 0, SourcePositionKind::Literal)],
            &config(),
        );
        assert_eq!(literal.error_count(), 0);
        assert!(literal.candidates.is_empty());
        assert_eq!(literal.diagnostics.len(), 1);
        assert_eq!(literal.diagnostics[0].severity, "auditInfo");
    }

    #[test]
    fn without_tokens_token_utilities_fail() {
        let result = classify_manifest_candidates(
            &[
                at("bg-surface", 0, SourcePositionKind::Class),
                at("flex", 20, SourcePositionKind::Class),
            ],
            &WindConfig::default(),
        );
        assert_eq!(result.error_count(), 1);
        assert_eq!(result.candidates, ["flex"]);
    }
}
