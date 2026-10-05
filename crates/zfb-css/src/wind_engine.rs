//! Adapter from the explicit source plan candidate set to zudo-wind.
use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::Result;
use zudo_wind::{CompileInput, Origin, OriginCandidate, Severity, WindConfig};

use crate::{
    AuthoredCssBundle, CssDiagnostic, CssDiagnosticOrigin, CssDiagnosticSeverity, CssEngine,
    CssEngineId, CssEngineOutput, CssInputDependencyKind, CssProvenance, CssProvenanceKind,
    WindDiagnosticsError,
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
                    let origin = diagnostic_origin(diagnostic.origin.as_deref());
                    CssDiagnostic {
                        severity: if diagnostic.severity == Severity::Error {
                            CssDiagnosticSeverity::Error
                        } else {
                            CssDiagnosticSeverity::Warning
                        },
                        code: format!("{:?}", diagnostic.code).to_ascii_uppercase(),
                        message: match diagnostic.suggested_spelling.as_deref() {
                            Some(spelling) => {
                                format!("{}; suggested spelling: {spelling}", diagnostic.message)
                            }
                            None => diagnostic.message.clone(),
                        },
                        origin,
                        candidate: diagnostic.candidate.clone(),
                    }
                }),
        );
        let errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.severity == CssDiagnosticSeverity::Error)
            .cloned()
            .collect();
        if !errors.is_empty() {
            return Err(WindDiagnosticsError { diagnostics }.into());
        }
        let css = compiled
            .parts
            .with_authored(&self.authored.css, self.config.utility_placement);
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

/// ` at <source>:<line>:<column>` so a failing candidate names its occurrence.
fn diagnostic_origin(origin: Option<&Origin>) -> CssDiagnosticOrigin {
    match origin {
        Some(Origin::Source {
            source_id,
            line,
            byte_column,
            byte_offset,
            ..
        }) => CssDiagnosticOrigin {
            source_id: Some(source_id.clone()),
            path: None,
            line: Some(*line),
            column: Some(*byte_column),
            byte_offset: Some(*byte_offset),
            label: None,
        },
        Some(Origin::Stylesheet {
            path, byte_offset, ..
        }) => CssDiagnosticOrigin {
            path: Some(PathBuf::from(path)),
            byte_offset: Some(*byte_offset),
            ..Default::default()
        },
        Some(Origin::Manifest {
            producer,
            path,
            index,
        }) => CssDiagnosticOrigin {
            path: Some(PathBuf::from(path)),
            label: Some(format!("manifest {producer}[{index}]")),
            ..Default::default()
        },
        Some(Origin::Safelist { owner, index }) => CssDiagnosticOrigin {
            label: Some(format!("wind.safelist.{owner}[{index}]")),
            ..Default::default()
        },
        Some(Origin::Config { key_path }) => CssDiagnosticOrigin {
            label: Some(key_path.clone()),
            ..Default::default()
        },
        Some(Origin::RoleClass { role_key }) => CssDiagnosticOrigin {
            label: Some(format!("codeHighlight.roleClasses {role_key}")),
            ..Default::default()
        },
        None => CssDiagnosticOrigin::default(),
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
    fn placed(placement: zudo_wind::UtilityPlacement) -> String {
        let config = WindConfig {
            reset: zudo_wind::ResetMode::MinimalV1,
            utility_placement: placement,
            ..WindConfig::default()
        };
        let authored = AuthoredCssBundle {
            css: "@import url(\"https://fonts.example/a.css\");\n.card { color: red; }\n".into(),
            companions: vec![],
            input_dependencies: vec![],
        };
        let css = WindEngine::new(config, ["block".to_string()].into(), authored)
            .produce_utility_css(&[])
            .unwrap()
            .css;
        crate::pipeline::combine(
            Some("@layer zfb-hi { .hi { color: blue; } }"),
            &css,
            ".module_x { color: green; }",
        )
    }

    #[test]
    fn utility_placement_moves_only_the_utility_stage() {
        let after = placed(zudo_wind::UtilityPlacement::AfterAuthored);
        let before = placed(zudo_wind::UtilityPlacement::BeforeAuthored);
        let order = |css: &str, needles: &[&str]| {
            let positions: Vec<_> = needles
                .iter()
                .map(|needle| {
                    css.find(needle)
                        .unwrap_or_else(|| panic!("{needle}:\n{css}"))
                })
                .collect();
            assert!(
                positions.windows(2).all(|pair| pair[0] < pair[1]),
                "{needles:?}:\n{css}"
            );
        };
        let fixed = [
            "@layer zw-reset, zw-tokens, zfb-hi, base, components;",
            "@import url(",
            "@layer zfb-hi {",
            "@layer zw-reset {",
        ];
        for css in [&after, &before] {
            order(css, &fixed);
            order(css, &["@layer zw-reset {", ".block", ".module_x"]);
            order(css, &["@layer zw-reset {", ".card", ".module_x"]);
        }
        order(&after, &[".card", ".block"]);
        order(&before, &[".block", ".card"]);
        assert_eq!(after.matches("@import").count(), 1);
        assert_eq!(before.len(), after.len());
    }

    #[test]
    fn default_placement_matches_the_previous_stage_order() {
        let compiled = zudo_wind::compile(&zudo_wind::CompileInput {
            candidates: vec![OriginCandidate {
                text: "block".into(),
                origin: Origin::Source {
                    source_id: "s".into(),
                    byte_offset: 0,
                    byte_length: 5,
                    line: 1,
                    byte_column: 1,
                    literal_byte_offset: 0,
                    literal_byte_length: 5,
                    position_kind: zudo_wind::SourcePositionKind::Class,
                },
            }],
            config: WindConfig::default(),
        });
        let parts = compiled.parts;
        let previous = [
            parts.prelude.as_str(),
            &parts.reset,
            &parts.tokens,
            &parts.registrations,
            ".card{}",
            &parts.utilities,
        ]
        .concat();
        assert_eq!(
            parts.with_authored(".card{}", zudo_wind::UtilityPlacement::default()),
            previous
        );
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
    fn failures_keep_structured_repeated_origins() {
        let source = |source_id: &str, line, byte_column, byte_offset| OriginCandidate {
            text: "rounded-missing".into(),
            origin: Origin::Source {
                source_id: source_id.into(),
                byte_offset,
                byte_length: 15,
                line,
                byte_column,
                literal_byte_offset: byte_offset,
                literal_byte_length: 15,
                position_kind: zudo_wind::SourcePositionKind::Class,
            },
        };
        let origins = vec![
            source("default/src:a.tsx", 3, 29, 70),
            // Identical input is one authored occurrence. A different span in
            // the same file must survive alongside the other source origins.
            source("default/src:a.tsx", 3, 29, 70),
            source("default/src:a.tsx", 5, 7, 110),
            source("default/src:b.tsx", 1, 12, 11),
            source("plugin/virtual:widgets", 2, 5, 30),
            OriginCandidate {
                text: "rounded-missing".into(),
                origin: Origin::Manifest {
                    producer: "widgets".into(),
                    path: "node_modules/widgets/wind.json".into(),
                    index: 2,
                },
            },
        ];
        let error = WindEngine::new(
            WindConfig::default(),
            ["rounded-missing".to_string()].into(),
            AuthoredCssBundle {
                css: String::new(),
                companions: vec![],
                input_dependencies: vec![],
            },
        )
        .with_origins(origins)
        .produce_utility_css(&[])
        .unwrap_err();
        let failure = error
            .downcast_ref::<WindDiagnosticsError>()
            .expect("structured wind failure");
        let locations: Vec<_> = failure
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.origin.location().unwrap())
            .collect();
        assert_eq!(
            locations,
            [
                "default/src:a.tsx:3:29",
                "default/src:a.tsx:5:7",
                "default/src:b.tsx:1:12",
                "plugin/virtual:widgets:2:5",
                "node_modules/widgets/wind.json (manifest widgets[2])",
            ],
            "distinct authored occurrences survive; identical input is normalized"
        );
        let first = &failure.diagnostics[0];
        assert_eq!(first.code, "ZW006");
        assert_eq!(first.candidate.as_deref(), Some("rounded-missing"));
        assert_eq!(first.origin.byte_offset, Some(70));
        let rendered = error.to_string();
        assert!(
            rendered.starts_with("wind CSS failed with 5 errors:"),
            "{rendered}"
        );
        assert!(
            rendered.contains("\n  default/src:a.tsx:3:29: ZW006 rounded-missing: "),
            "{rendered}"
        );
        assert_eq!(
            rendered.lines().count(),
            5,
            "one line per diagnostic: {rendered}"
        );
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
