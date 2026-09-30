//! Candidate inspection commands for zudo-wind.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::cli::{WindArgs, WindAuditFailOn, WindCommand, WindExplainArgs};
use crate::commands::css_support::{
    build_standalone_wind_source_plan, configured_wind, index_standalone_wind_sources,
};

/// Dispatch the zudo-wind command family.
pub async fn run(args: &WindArgs) -> Result<()> {
    match &args.command {
        WindCommand::Explain(args) => explain(args).await,
        WindCommand::Audit(args) => audit(args.project_root.as_deref(), args.fail_on).await,
    }
}

async fn explain(args: &WindExplainArgs) -> Result<()> {
    let project_root = project_root(args.project_root.as_deref())?;
    let config = crate::config::load_from_dir(&project_root)
        .await
        .context("failed to load project configuration for wind explain")?;
    let (generation_enabled, wind_config) = configured_wind(&config);
    let explanation =
        zfb_css::explain_with_generation(&args.candidate, &wind_config, generation_enabled);
    print!("{}", zfb_css::render_explanation(&explanation));
    Ok(())
}

async fn audit(project_root_arg: Option<&Path>, fail_on: Option<WindAuditFailOn>) -> Result<()> {
    let project_root = project_root(project_root_arg)?;
    let project_config = crate::config::load_from_dir(&project_root)
        .await
        .context("failed to load project configuration for wind audit")?;
    let (generation_enabled, mut wind_config) = configured_wind(&project_config);
    if !generation_enabled {
        let report = zfb_css::audit(
            &zfb_css::AuditInput {
                generation_enabled: false,
                ..Default::default()
            },
            &wind_config,
        );
        return print_audit_and_apply_exit_policy(&report, fail_on);
    }

    // This is the same default standalone plan as `zfb css` without explicit
    // --source arguments. Neither command scans the build/dev package-route,
    // mirror, or plugin roots.
    let audit_output = project_root.join(".zfb-wind-audit-output.css");
    let (plan, _warnings) = build_standalone_wind_source_plan(
        &project_root,
        &audit_output,
        &project_config,
        true,
        &[],
    )?;
    let indexed = index_standalone_wind_sources(&plan)?;
    let manifest_owners = add_manifest_candidates_to_audit_config(&mut wind_config, &plan)?;
    let mut audit_sources = indexed.audit_sources;
    append_role_class_audit_source(&plan, &mut audit_sources);
    let report = zfb_css::audit(&zfb_css::AuditInput::new(audit_sources), &wind_config);
    let report = rewrite_role_class_origins(rewrite_manifest_origins(report, &manifest_owners));
    print_audit_and_apply_exit_policy(&report, fail_on)
}

fn print_audit_and_apply_exit_policy(
    report: &zfb_css::AuditReport,
    fail_on: Option<WindAuditFailOn>,
) -> Result<()> {
    print!("{}", zfb_css::render_audit(report));
    match audit_exit(report, fail_on) {
        Ok(Some(note)) => {
            eprintln!("{note}");
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(summary) => Err(anyhow::Error::msg(summary)),
    }
}

fn audit_exit(
    report: &zfb_css::AuditReport,
    fail_on: Option<WindAuditFailOn>,
) -> std::result::Result<Option<String>, String> {
    match report.outcome {
        zfb_css::AuditOutcome::InvalidConfiguration => {
            return Err("wind audit: invalid configuration".into());
        }
        zfb_css::AuditOutcome::GenerationDisabled => return Ok(None),
        zfb_css::AuditOutcome::Complete => {}
    }

    let (errors, warnings) =
        report
            .diagnostics
            .iter()
            .fold(
                (0usize, 0usize),
                |(errors, warnings), diagnostic| match diagnostic.severity.as_str() {
                    "error" => (errors + 1, warnings),
                    "warning" => (errors, warnings + 1),
                    "auditInfo" => (errors, warnings),
                    _ => (errors, warnings),
                },
            );

    match fail_on {
        Some(WindAuditFailOn::Error) if errors > 0 => Err(format!(
            "wind audit: {errors} error-severity diagnostics met --fail-on error"
        )),
        Some(WindAuditFailOn::Warning) if errors + warnings > 0 => Err(format!(
            "wind audit: {} error- or warning-severity diagnostics met --fail-on warning",
            errors + warnings
        )),
        None if errors > 0 => Ok(Some(format!(
            "wind audit: scan complete with {errors} error-severity diagnostics (pass --fail-on error to fail on them)"
        ))),
        _ => Ok(None),
    }
}

fn project_root(root: Option<&Path>) -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let root = root.unwrap_or(Path::new("."));
    let root = if root.is_absolute() {
        root.to_path_buf()
    } else {
        cwd.join(root)
    };
    Ok(zfb_types::normalize_path_lexical(&root))
}

#[derive(Clone)]
struct ManifestAuditOwner {
    producer: String,
    path: String,
    original_indices: Vec<usize>,
}

fn add_manifest_candidates_to_audit_config(
    config: &mut zfb_css::WindConfig,
    plan: &zfb_css::SourcePlan,
) -> Result<BTreeMap<String, ManifestAuditOwner>> {
    let mut owners = BTreeMap::new();
    for (producer, path) in &plan.manifests {
        let bytes = std::fs::read(path).with_context(|| {
            format!(
                "failed to read wind manifest {producer} at {}",
                path.display()
            )
        })?;
        let manifest: serde_json::Value = serde_json::from_slice(&bytes).with_context(|| {
            format!(
                "failed to parse wind manifest {producer} at {}",
                path.display()
            )
        })?;
        let entries = manifest["candidates"]
            .as_array()
            .with_context(|| format!("wind manifest {producer} candidates missing"))?;
        let mut seen = BTreeSet::new();
        let mut candidates = Vec::new();
        let mut original_indices = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            let candidate = entry.as_str().with_context(|| {
                format!("wind manifest {producer} candidate {index} is not a string")
            })?;
            if seen.insert(candidate.to_owned()) {
                candidates.push(candidate.to_owned());
                original_indices.push(index);
            }
        }
        let base_owner = format!("zfb-internal-manifest/{producer}");
        let mut owner = base_owner.clone();
        let mut suffix = 0usize;
        while config.safelist.contains_key(&owner) {
            suffix += 1;
            owner = format!("{base_owner}-{suffix}");
        }
        config.safelist.insert(owner.clone(), candidates);
        owners.insert(
            owner,
            ManifestAuditOwner {
                producer: producer.clone(),
                path: path.display().to_string(),
                original_indices,
            },
        );
    }
    Ok(owners)
}

fn rewrite_manifest_origins(
    mut report: zfb_css::AuditReport,
    owners: &BTreeMap<String, ManifestAuditOwner>,
) -> zfb_css::AuditReport {
    for diagnostic in &mut report.diagnostics {
        rewrite_manifest_origin(&mut diagnostic.origin, owners);
    }
    for unrecognized in &mut report.unrecognized_classes {
        rewrite_manifest_view(&mut unrecognized.origin, owners);
    }
    for conflict in &mut report.conflicts {
        rewrite_manifest_view(&mut conflict.origin, owners);
    }
    for dead in &mut report.dead_classes {
        rewrite_manifest_origin(&mut dead.diagnostic.origin, owners);
    }
    report
}

fn rewrite_manifest_origin(
    origin: &mut Option<zfb_css::OriginView>,
    owners: &BTreeMap<String, ManifestAuditOwner>,
) {
    if let Some(origin) = origin {
        rewrite_manifest_view(origin, owners);
    }
}

fn rewrite_manifest_view(
    origin: &mut zfb_css::OriginView,
    owners: &BTreeMap<String, ManifestAuditOwner>,
) {
    let Some(owner) = origin.owner.as_deref() else {
        return;
    };
    let Some(manifest) = owners.get(owner) else {
        return;
    };
    origin.kind = "manifest".into();
    origin.producer = Some(manifest.producer.clone());
    origin.path = Some(manifest.path.clone());
    if let Some(index) = origin
        .index
        .and_then(|index| manifest.original_indices.get(index))
    {
        origin.index = Some(*index);
    }
    origin.owner = None;
}

fn append_role_class_audit_source(
    plan: &zfb_css::SourcePlan,
    sources: &mut Vec<zfb_css::AuditSource>,
) {
    let Some(candidates) = plan.generated_sources.get("code-highlight/role-classes") else {
        return;
    };
    if candidates.is_empty() {
        return;
    }
    let extraction = zfb_css::ExtractionResult {
        candidates: candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| zfb_css::ExtractedCandidate {
                text: candidate.clone(),
                occurrences: vec![zfb_css::Occurrence {
                    byte_offset: index,
                    byte_length: candidate.len(),
                    line: index + 1,
                    byte_column: 1,
                    literal_byte_offset: index,
                    literal_byte_length: candidate.len(),
                    position_kind: zfb_css::PositionKind::Literal,
                    adjacent_interpolation: false,
                }],
            })
            .collect(),
        notes: Vec::new(),
    };
    sources.push(zfb_css::AuditSource::new(
        "generated/code-highlight/role-classes",
        extraction,
    ));
}

fn rewrite_role_class_origins(mut report: zfb_css::AuditReport) -> zfb_css::AuditReport {
    for diagnostic in &mut report.diagnostics {
        if let Some(origin) = &mut diagnostic.origin {
            rewrite_role_class_view(origin, diagnostic.candidate.as_deref());
        }
    }
    for unrecognized in &mut report.unrecognized_classes {
        rewrite_role_class_view(&mut unrecognized.origin, Some(&unrecognized.candidate));
    }
    for conflict in &mut report.conflicts {
        rewrite_role_class_view(&mut conflict.origin, Some(&conflict.first_candidate));
    }
    for dead in &mut report.dead_classes {
        if let Some(origin) = &mut dead.diagnostic.origin {
            rewrite_role_class_view(origin, dead.diagnostic.candidate.as_deref());
        }
    }
    report
}

fn rewrite_role_class_view(origin: &mut zfb_css::OriginView, candidate: Option<&str>) {
    if origin.source_id.as_deref() != Some("generated/code-highlight/role-classes") {
        return;
    }
    origin.kind = "roleClass".into();
    origin.role_key = candidate.map(str::to_owned);
    origin.source_id = None;
    origin.byte_offset = None;
    origin.byte_length = None;
    origin.line = None;
    origin.byte_column = None;
    origin.literal_byte_offset = None;
    origin.literal_byte_length = None;
    origin.position_kind = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audit_report(outcome: zfb_css::AuditOutcome, severities: &[&str]) -> zfb_css::AuditReport {
        zfb_css::AuditReport {
            outcome,
            spec_version: 1,
            spec_revision: 1,
            diagnostics: severities
                .iter()
                .map(|severity| zfb_css::DiagnosticView {
                    severity: (*severity).into(),
                    code: "ZW001".into(),
                    candidate: None,
                    origin: None,
                    message: "synthetic diagnostic".into(),
                    suggestion: None,
                    rejection_id: None,
                })
                .collect(),
            unrecognized_classes: Vec::new(),
            conflicts: Vec::new(),
            dead_classes: Vec::new(),
            dynamic_constructions: Vec::new(),
            adjacent_interpolations: Vec::new(),
            extraction_notes: Vec::new(),
        }
    }

    #[test]
    fn audit_exit_applies_the_outcome_and_severity_policy() {
        use zfb_css::AuditOutcome::{Complete, GenerationDisabled, InvalidConfiguration};

        let thresholds = [
            None,
            Some(WindAuditFailOn::Error),
            Some(WindAuditFailOn::Warning),
        ];

        let invalid = audit_report(InvalidConfiguration, &["error"]);
        for threshold in thresholds {
            assert_eq!(
                audit_exit(&invalid, threshold),
                Err("wind audit: invalid configuration".into())
            );
        }

        let disabled = audit_report(GenerationDisabled, &["error", "warning"]);
        for threshold in thresholds {
            assert_eq!(audit_exit(&disabled, threshold), Ok(None));
        }

        let errors = audit_report(Complete, &["error", "error"]);
        assert_eq!(
            audit_exit(&errors, None),
            Ok(Some("wind audit: scan complete with 2 error-severity diagnostics (pass --fail-on error to fail on them)".into()))
        );
        assert_eq!(
            audit_exit(&errors, Some(WindAuditFailOn::Error)),
            Err("wind audit: 2 error-severity diagnostics met --fail-on error".into())
        );
        assert_eq!(
            audit_exit(&errors, Some(WindAuditFailOn::Warning)),
            Err(
                "wind audit: 2 error- or warning-severity diagnostics met --fail-on warning".into()
            )
        );

        let warnings = audit_report(Complete, &["warning", "warning"]);
        assert_eq!(audit_exit(&warnings, None), Ok(None));
        assert_eq!(
            audit_exit(&warnings, Some(WindAuditFailOn::Error)),
            Ok(None)
        );
        assert_eq!(
            audit_exit(&warnings, Some(WindAuditFailOn::Warning)),
            Err(
                "wind audit: 2 error- or warning-severity diagnostics met --fail-on warning".into()
            )
        );

        let informational_and_unknown = audit_report(Complete, &["auditInfo", "futureSeverity"]);
        for threshold in thresholds {
            assert_eq!(audit_exit(&informational_and_unknown, threshold), Ok(None));
        }

        let empty = audit_report(Complete, &[]);
        for threshold in thresholds {
            assert_eq!(audit_exit(&empty, threshold), Ok(None));
        }
    }

    #[test]
    fn manifest_candidates_keep_manifest_origins_in_audit_report() {
        let owner = "zfb-internal-manifest/widgets".to_owned();
        let mut report = zfb_css::AuditReport {
            outcome: zfb_css::AuditOutcome::Complete,
            spec_version: 1,
            spec_revision: 2,
            diagnostics: vec![zfb_css::DiagnosticView {
                severity: "error".into(),
                code: "ZW008".into(),
                candidate: Some("unknown".into()),
                origin: Some(zfb_css::OriginView {
                    kind: "safelist".into(),
                    owner: Some(owner.clone()),
                    index: Some(0),
                    ..Default::default()
                }),
                message: "unknown".into(),
                suggestion: None,
                rejection_id: None,
            }],
            unrecognized_classes: Vec::new(),
            conflicts: Vec::new(),
            dead_classes: Vec::new(),
            dynamic_constructions: Vec::new(),
            adjacent_interpolations: Vec::new(),
            extraction_notes: Vec::new(),
        };
        let owners = BTreeMap::from([(
            owner,
            ManifestAuditOwner {
                producer: "widgets".into(),
                path: "node_modules/widgets/wind.json".into(),
                original_indices: vec![3],
            },
        )]);

        report = rewrite_manifest_origins(report, &owners);
        let origin = report.diagnostics[0].origin.as_ref().unwrap();
        assert_eq!(origin.kind, "manifest");
        assert_eq!(origin.producer.as_deref(), Some("widgets"));
        assert_eq!(
            origin.path.as_deref(),
            Some("node_modules/widgets/wind.json")
        );
        assert_eq!(origin.index, Some(3));
        assert!(origin.owner.is_none());
    }

    #[test]
    fn disabled_explain_is_an_explicit_report() {
        let explanation = zfb_css::explain_disabled("block");
        assert_eq!(
            explanation.outcome,
            zfb_css::ExplanationOutcome::GenerationDisabled
        );
        assert!(explanation.declarations.is_empty());
    }

    #[test]
    fn generated_role_candidates_report_their_role_origin() {
        let mut report = zfb_css::AuditReport {
            outcome: zfb_css::AuditOutcome::Complete,
            spec_version: 1,
            spec_revision: 2,
            diagnostics: Vec::new(),
            unrecognized_classes: vec![zfb_css::UnrecognizedClass {
                candidate: "role-class".into(),
                origin: zfb_css::OriginView {
                    kind: "source".into(),
                    source_id: Some("generated/code-highlight/role-classes".into()),
                    position_kind: Some("literal".into()),
                    ..Default::default()
                },
            }],
            conflicts: Vec::new(),
            dead_classes: Vec::new(),
            dynamic_constructions: Vec::new(),
            adjacent_interpolations: Vec::new(),
            extraction_notes: Vec::new(),
        };

        report = rewrite_role_class_origins(report);
        let origin = &report.unrecognized_classes[0].origin;
        assert_eq!(origin.kind, "roleClass");
        assert_eq!(origin.role_key.as_deref(), Some("role-class"));
        assert!(origin.source_id.is_none());
        assert!(origin.position_kind.is_none());
    }
}
