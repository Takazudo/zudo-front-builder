//! Adapter from the explicit source plan candidate set to zudo-wind.
use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{bail, Result};
use zudo_wind::{CompileInput, Origin, OriginCandidate, Severity, WindConfig};

use crate::{
    AuthoredCssBundle, CssDiagnostic, CssDiagnosticOrigin, CssDiagnosticSeverity, CssEngine,
    CssEngineId, CssEngineOutput, CssInputDependencyKind, CssProvenance, CssProvenanceKind,
};

#[derive(Debug, Clone)]
pub struct WindEngine {
    config: WindConfig,
    candidates: BTreeSet<String>,
    authored: AuthoredCssBundle,
    diagnostics: Vec<CssDiagnostic>,
    origins: Option<Vec<OriginCandidate>>,
}

impl WindEngine {
    pub fn new(
        config: WindConfig,
        candidates: BTreeSet<String>,
        authored: AuthoredCssBundle,
    ) -> Self {
        Self {
            config,
            candidates,
            authored,
            diagnostics: Vec::new(),
            origins: None,
        }
    }

    pub fn with_origins(mut self, origins: Vec<OriginCandidate>) -> Self {
        self.origins = Some(origins);
        self
    }

    pub fn with_diagnostics(mut self, diagnostics: Vec<CssDiagnostic>) -> Self {
        self.diagnostics = diagnostics;
        self
    }
}

impl CssEngine for WindEngine {
    fn produce_utility_css(&self, _sources: &[PathBuf]) -> Result<CssEngineOutput> {
        let compiled = zudo_wind::compile(&CompileInput {
            candidates: if let Some(origins) = &self.origins {
                origins
                    .iter()
                    .filter(|input| self.candidates.contains(&input.text))
                    .cloned()
                    .collect()
            } else {
                self.candidates
                    .iter()
                    .map(|text| OriginCandidate {
                        text: text.clone(),
                        origin: Origin::Source {
                            source_id: "source-plan".into(),
                            byte_offset: 0,
                            byte_length: text.len(),
                            line: 1,
                            byte_column: 1,
                            literal_byte_offset: 0,
                            literal_byte_length: text.len(),
                            position_kind: zudo_wind::SourcePositionKind::Class,
                        },
                    })
                    .collect()
            },
            config: self.config.clone(),
        });
        let mut diagnostics = self.diagnostics.clone();
        diagnostics.extend(
            compiled
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.severity != Severity::AuditInfo)
                .map(|diagnostic| {
                    let origin = match diagnostic.origin.as_deref() {
                        Some(Origin::Source {
                            source_id,
                            line,
                            byte_column,
                            ..
                        }) => CssDiagnosticOrigin {
                            path: Some(PathBuf::from(source_id)),
                            line: Some(*line),
                            column: Some(*byte_column),
                        },
                        Some(Origin::Stylesheet { path, .. }) => CssDiagnosticOrigin {
                            path: Some(PathBuf::from(path)),
                            ..Default::default()
                        },
                        _ => CssDiagnosticOrigin::default(),
                    };
                    CssDiagnostic {
                        severity: if diagnostic.severity == Severity::Error {
                            CssDiagnosticSeverity::Error
                        } else {
                            CssDiagnosticSeverity::Warning
                        },
                        code: format!("{:?}", diagnostic.code).to_ascii_uppercase(),
                        message: diagnostic.message.clone(),
                        origin,
                        candidate: diagnostic.candidate.clone(),
                    }
                }),
        );
        if diagnostics
            .iter()
            .any(|d| d.severity == CssDiagnosticSeverity::Error)
        {
            bail!(
                "wind CSS failed: {}",
                diagnostics
                    .iter()
                    .filter(|d| d.severity == CssDiagnosticSeverity::Error)
                    .map(|d| format!(
                        "{}: {}{}",
                        d.code,
                        d.message,
                        d.candidate
                            .as_deref()
                            .map(|c| format!(" ({c})"))
                            .unwrap_or_default()
                    ))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
        let parts = compiled.parts;
        let mut css = parts.prelude;
        css.push_str(&parts.reset);
        css.push_str(&parts.tokens);
        css.push_str(&parts.registrations);
        css.push_str(&self.authored.css);
        css.push_str(&parts.utilities);
        let mut output = CssEngineOutput::new(
            css,
            CssEngineId::new(
                "zudo-wind",
                Some(format!(
                    "{}.{}",
                    zudo_wind::SPEC_VERSION,
                    zudo_wind::SPEC_REVISION
                )),
            ),
        );
        output.companions = self.authored.companions.clone();
        output.input_dependencies = self.authored.input_dependencies.clone();
        output.diagnostics = diagnostics;
        if compiled.provenance.is_some() {
            output.provenance = Some(CssProvenance {
                source_id: "zudo-wind://spec/1".into(),
                kind: CssProvenanceKind::Generated,
                spec_version: zudo_wind::SPEC_VERSION,
                spec_revision: zudo_wind::SPEC_REVISION,
                map: None,
                authored_stylesheets: self
                    .authored
                    .input_dependencies
                    .iter()
                    .filter(|dependency| dependency.kind == CssInputDependencyKind::Stylesheet)
                    .map(|dependency| dependency.path.clone())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            });
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_candidates_only() {
        let engine = WindEngine::new(
            WindConfig::default(),
            ["block".to_string()].into(),
            AuthoredCssBundle {
                css: String::new(),
                companions: vec![],
                input_dependencies: vec![],
            },
        );
        let result = engine
            .produce_utility_css(&[PathBuf::from("unread.tsx")])
            .unwrap();
        assert!(result.css.contains(".block"));
        assert!(!result.css.contains("unread"));
        assert_eq!(
            result.provenance.as_ref().unwrap().source_id,
            "zudo-wind://spec/1"
        );
        assert_eq!(
            result.provenance.as_ref().unwrap().kind,
            CssProvenanceKind::Generated
        );
        assert_eq!(result.provenance.as_ref().unwrap().spec_version, 1);
        assert_eq!(
            result.provenance.as_ref().unwrap().spec_revision,
            zudo_wind::SPEC_REVISION
        );
        assert!(result.provenance.as_ref().unwrap().map.is_none());
    }
    #[test]
    fn empty_output_has_no_generated_provenance() {
        let engine = WindEngine::new(
            WindConfig::default(),
            BTreeSet::new(),
            AuthoredCssBundle {
                css: String::new(),
                companions: vec![],
                input_dependencies: vec![],
            },
        );
        let result = engine.produce_utility_css(&[]).unwrap();
        assert!(result.css.is_empty());
        assert!(result.provenance.is_none());
    }
    #[test]
    fn literal_invalid_shape_is_audit_only_but_class_invalid_fails() {
        let bundle = AuthoredCssBundle {
            css: String::new(),
            companions: vec![],
            input_dependencies: vec![],
        };
        let origin = |position_kind| Origin::Source {
            source_id: "default/pages:index.tsx".into(),
            byte_offset: 0,
            byte_length: 5,
            line: 1,
            byte_column: 1,
            literal_byte_offset: 0,
            literal_byte_length: 5,
            position_kind,
        };
        let candidate = "w-1/0".to_string();
        let literal = WindEngine::new(
            WindConfig::default(),
            [candidate.clone()].into(),
            bundle.clone(),
        )
        .with_origins(vec![OriginCandidate {
            text: candidate.clone(),
            origin: origin(zudo_wind::SourcePositionKind::Literal),
        }]);
        assert!(literal
            .produce_utility_css(&[])
            .unwrap()
            .diagnostics
            .is_empty());
        let class = WindEngine::new(WindConfig::default(), [candidate.clone()].into(), bundle)
            .with_origins(vec![OriginCandidate {
                text: candidate,
                origin: origin(zudo_wind::SourcePositionKind::Class),
            }]);
        assert!(class.produce_utility_css(&[]).is_err());
    }
    #[test]
    fn mixed_output_preserves_generated_and_authored_sources() {
        let authored = PathBuf::from("/project/styles/global.css");
        let manifest = PathBuf::from("/workspace/theme/package.json");
        let engine = WindEngine::new(
            WindConfig::default(),
            ["block".to_string()].into(),
            AuthoredCssBundle {
                css: ".authored { color: red; }".into(),
                companions: vec![],
                input_dependencies: vec![
                    crate::CssInputDependency {
                        path: authored.clone(),
                        kind: CssInputDependencyKind::Stylesheet,
                    },
                    crate::CssInputDependency {
                        path: manifest.clone(),
                        kind: CssInputDependencyKind::PackageManifest,
                    },
                ],
            },
        );
        let result = engine.produce_utility_css(&[]).unwrap();
        assert_eq!(
            result.input_dependencies,
            vec![
                crate::CssInputDependency {
                    path: authored.clone(),
                    kind: CssInputDependencyKind::Stylesheet,
                },
                crate::CssInputDependency {
                    path: manifest,
                    kind: CssInputDependencyKind::PackageManifest,
                },
            ]
        );
        let provenance = result.provenance.unwrap();
        assert_eq!(provenance.source_id, "zudo-wind://spec/1");
        assert_eq!(provenance.authored_stylesheets, vec![authored]);
        assert!(provenance.map.is_none());
    }
}
