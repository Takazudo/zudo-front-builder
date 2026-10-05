//! Candidate inspection commands for zudo-wind.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{bail, Context, Result};

use serde::Serialize;

use crate::cli::{
    WindArgs, WindAuditArgs, WindAuditFailOn, WindAuditPlan, WindAuditSeverity, WindCommand,
    WindExplainArgs, WindManifestArgs,
};
use crate::commands::css_support::{
    build_standalone_wind_source_plan, command_project_root, configured_wind,
    index_standalone_wind_sources, load_command_config,
};

/// Dispatch the zudo-wind command family.
pub async fn run(args: &WindArgs) -> Result<()> {
    match &args.command {
        WindCommand::Explain(args) => explain(args).await,
        WindCommand::Audit(args) => audit(args).await,
        WindCommand::Manifest(args) => manifest(args).await,
    }
}

/// Version of the `--json` documents printed by `zfb wind explain` and audit.
const WIND_JSON_SCHEMA_VERSION: u32 = 1;

async fn explain(args: &WindExplainArgs) -> Result<()> {
    let candidates = match &args.candidate {
        Some(candidate) => vec![candidate.clone()],
        None => stdin_candidates(
            &std::io::read_to_string(std::io::stdin()).context("failed to read stdin")?,
        ),
    };
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let project_root =
        command_project_root(&cwd, args.project_root.as_deref(), args.config.as_deref());
    let config = load_command_config(&cwd, &project_root, args.config.as_deref())
        .await
        .context("failed to load project configuration for wind explain")?;
    let (generation_enabled, wind_config) = configured_wind(&config);
    let explanations: Vec<_> = candidates
        .iter()
        .map(|candidate| {
            zfb_css::explain_with_generation(candidate, &wind_config, generation_enabled)
        })
        .collect();
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schemaVersion": WIND_JSON_SCHEMA_VERSION,
                "command": "explain",
                "explanations": explanations,
            }))?
        );
    } else {
        let rendered: Vec<_> = explanations
            .iter()
            .map(zfb_css::render_explanation)
            .collect();
        print!("{}", rendered.join("\n"));
    }
    Ok(())
}

/// One complete candidate per nonempty line, in order, duplicates kept.
fn stdin_candidates(input: &str) -> Vec<String> {
    input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

async fn audit(args: &WindAuditArgs) -> Result<()> {
    let fail_on = args.fail_on;
    let plan_mode = args.plan;
    let output = AuditOutput {
        json: args.json,
        severity: args.severity,
    };
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let project_root =
        command_project_root(&cwd, args.project_root.as_deref(), args.config.as_deref());
    let project_config = load_command_config(&cwd, &project_root, args.config.as_deref())
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
        return print_audit_and_apply_exit_policy(&report, None, output, fail_on);
    }

    let (plan, plugin_virtual_modules) = match plan_mode {
        // The same default plan as `zfb css` without explicit --source
        // arguments: no package-route, mirror, or plugin roots.
        WindAuditPlan::Standalone => {
            let audit_output = project_root.join(".zfb-wind-audit-output.css");
            let (plan, _warnings) = build_standalone_wind_source_plan(
                &project_root,
                &audit_output,
                &project_config,
                true,
                &[],
            )?;
            (plan, Vec::new())
        }
        WindAuditPlan::Build => build_audit_plan(&project_root, &project_config).await?,
    };
    let coverage = plan_coverage(plan_mode, &plan, &plugin_virtual_modules, &project_root);
    let indexed = index_standalone_wind_sources(&plan)?;
    let manifest_owners = add_manifest_candidates_to_audit_config(&mut wind_config, &plan)?;
    let mut audit_sources = indexed.audit_sources;
    append_plugin_audit_sources(&plugin_virtual_modules, &mut audit_sources);
    append_role_class_audit_source(&plan, &mut audit_sources);
    let report = zfb_css::audit(&zfb_css::AuditInput::new(audit_sources), &wind_config);
    let report = rewrite_role_class_origins(rewrite_manifest_origins(report, &manifest_owners));
    print_audit_and_apply_exit_policy(&report, Some(&coverage), output, fail_on)
}

/// Classify a package's source candidates against the selected config and
/// atomically write the resolved utilities and markers as a manifest that
/// consumers declare under `wind.manifests`.
async fn manifest(args: &WindManifestArgs) -> Result<()> {
    if !crate::config::is_wind_owner_id(&args.producer) {
        bail!(
            "invalid --producer {:?}: a producer id must match [A-Za-z0-9][A-Za-z0-9._/-]*",
            args.producer
        );
    }
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let project_root =
        command_project_root(&cwd, args.project_root.as_deref(), args.config.as_deref());
    let output = zfb_types::normalize_path_lexical(&cwd.join(&args.output));
    let mut config = load_command_config(&cwd, &project_root, args.config.as_deref())
        .await
        .context("failed to load project configuration for wind manifest")?;
    let (generation_enabled, wind_config) = configured_wind(&config);
    if !generation_enabled {
        bail!("wind is disabled (`wind: false`); a candidate manifest needs an enabled wind configuration");
    }
    // Configured manifests belong to other producers, and a previous run's
    // output must never feed the next one.
    if let Some(crate::config::WindSetting::Enabled(wind)) = &mut config.wind {
        wind.manifests.clear();
    }
    let explicit_sources =
        crate::commands::css::resolve_explicit_sources(&project_root, &args.source);
    let source_errors = crate::commands::css::explicit_source_errors(&explicit_sources);
    if !source_errors.is_empty() {
        bail!(
            "wind manifest validation failed:\n- {}",
            source_errors.join("\n- ")
        );
    }
    let (plan, warnings) = build_standalone_wind_source_plan(
        &project_root,
        &output,
        &config,
        !args.no_auto_source,
        &explicit_sources,
    )?;
    for warning in warnings {
        crate::output::warn(warning);
    }
    let coverage = plan_coverage(WindAuditPlan::Standalone, &plan, &[], &project_root);
    let indexed = index_standalone_wind_sources(&plan)?;
    let occurrences: Vec<_> = indexed
        .origins
        .into_iter()
        .filter(|candidate| matches!(candidate.origin, zfb_css::Origin::Source { .. }))
        .collect();
    let classification = zfb_css::classify_manifest_candidates(&occurrences, &wind_config);
    let plan_errors = indexed
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == zfb_css::CssDiagnosticSeverity::Error)
        .count();
    let errors = classification.error_count() + plan_errors;

    if errors == 0 {
        let document = serde_json::json!({
            "schemaVersion": MANIFEST_SCHEMA_VERSION,
            "specVersion": zfb_css::SPEC_VERSION,
            "producer": args.producer,
            "candidates": classification.candidates,
        });
        let mut bytes = serde_json::to_vec_pretty(&document)?;
        bytes.push(b'\n');
        zfb_build::atomic::atomic_write(&output, &bytes)
            .with_context(|| format!("failed to write wind manifest {}", output.display()))?;
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schemaVersion": WIND_JSON_SCHEMA_VERSION,
                "command": "manifest",
                "producer": args.producer,
                "output": output.display().to_string(),
                "written": errors == 0,
                "coverage": coverage,
                "planDiagnostics": indexed
                    .diagnostics
                    .iter()
                    .map(|diagnostic| serde_json::json!({
                        "severity": match diagnostic.severity {
                            zfb_css::CssDiagnosticSeverity::Error => "error",
                            zfb_css::CssDiagnosticSeverity::Warning => "warning",
                        },
                        "code": diagnostic.code,
                        "message": diagnostic.message,
                    }))
                    .collect::<Vec<_>>(),
                "classification": classification,
            }))?
        );
    } else {
        print!("{}", render_plan_coverage("manifest", &coverage));
        print!(
            "{}",
            render_manifest_report(&args.producer, &output, &classification, errors == 0)
        );
        for diagnostic in &indexed.diagnostics {
            println!("  {}", diagnostic.render());
        }
    }
    if errors > 0 {
        bail!(
            "wind manifest {} found {errors} error(s); {} was not written",
            args.producer,
            output.display()
        );
    }
    Ok(())
}

/// Version of the candidate manifest schema the consumer-side reader accepts.
const MANIFEST_SCHEMA_VERSION: u32 = 1;

fn render_manifest_report(
    producer: &str,
    output: &Path,
    classification: &zfb_css::ManifestClassification,
    written: bool,
) -> String {
    let mut out = format!(
        "wind manifest {producer}: {} candidates ({} markers) {}\n",
        classification.candidates.len(),
        classification.markers.len(),
        if written {
            format!("written to {}", output.display())
        } else {
            format!(
                "not written; {} keeps its previous contents",
                output.display()
            )
        }
    );
    let low_confidence = classification
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == "auditInfo")
        .count();
    out.push_str(&format!(
        "  excluded {} authored classes, {} ordinary words, {} low-confidence words\n",
        classification.authored_classes.len(),
        classification.ordinary_classes.len(),
        low_confidence
    ));
    for diagnostic in &classification.diagnostics {
        if diagnostic.severity == "auditInfo" {
            continue;
        }
        let location = diagnostic
            .origin
            .as_ref()
            .map(origin_view_location)
            .unwrap_or_default();
        out.push_str(&format!(
            "  {} {location}: {} {}: {}\n",
            diagnostic.severity,
            diagnostic.code,
            diagnostic.candidate.as_deref().unwrap_or_default(),
            diagnostic.message
        ));
    }
    out
}

fn origin_view_location(origin: &zfb_css::OriginView) -> String {
    match (&origin.source_id, origin.line, origin.byte_column) {
        (Some(source), Some(line), Some(column)) => format!("{source}:{line}:{column}"),
        _ => origin
            .key_path
            .clone()
            .unwrap_or_else(|| origin.kind.clone()),
    }
}

#[derive(Clone, Copy)]
struct AuditOutput {
    json: bool,
    severity: Option<WindAuditSeverity>,
}

/// Discover the build/dev plan the way `zfb build` does — plugin setup,
/// package routes, sibling mirrors, and plugin virtual modules — without
/// running lifecycle hooks or writing build output.
#[cfg(feature = "embed_v8")]
async fn build_audit_plan(
    project_root: &Path,
    config: &crate::config::Config,
) -> Result<(zfb_css::SourcePlan, Vec<(String, String)>)> {
    let outdir = crate::commands::resolve::resolve_outdir(project_root, &config.out_dir);
    let scratch =
        crate::commands::scratch_dir::resolve_from_env(project_root, config, &outdir, None)?;
    let plugin_host = crate::commands::plugins::maybe_spawn_host(config).await?;
    let setup = crate::commands::plugins::run_plugin_setup(
        &plugin_host,
        project_root,
        &scratch.layout().plugin_scratch_dir(),
        config,
        zfb_build::SetupCommand::Build,
    )
    .await;
    if let Some(host) = plugin_host {
        let _ = host.shutdown().await;
    }
    let setup = setup?;
    let overlay = crate::commands::package_routes::resolve_build_pages_root(
        &project_root.join("pages"),
        setup.setup_registries.injected_routes.as_slice(),
    )
    .context("resolving package-owned routes for wind audit --plan build")?;
    let package_route_entrypoints: Vec<std::path::PathBuf> = overlay
        .materialized
        .iter()
        .map(|route| route.entrypoint.clone())
        .collect();
    let (_, sibling_mirror_roots) = crate::commands::build::discover_css_sibling_mirror_roots(
        project_root,
        config,
        &setup.plugin_alias_entries,
        &setup.plugin_virtual_modules,
    )?;
    let inputs = crate::commands::css_source_plan::gather_css_source_plan_inputs(
        project_root,
        &outdir,
        config,
        &package_route_entrypoints,
        &sibling_mirror_roots,
        &setup.plugin_virtual_modules,
        &scratch.layout().written_roots(),
    )?;
    Ok((
        crate::commands::css_source_plan::build_css_source_plan(&inputs),
        setup.plugin_virtual_modules,
    ))
}

#[cfg(not(feature = "embed_v8"))]
async fn build_audit_plan(
    _project_root: &Path,
    _config: &crate::config::Config,
) -> Result<(zfb_css::SourcePlan, Vec<(String, String)>)> {
    anyhow::bail!(
        "zfb was built without V8 support; `zfb wind audit --plan build` needs plugin setup \
         to discover the build source plan. Use --plan standalone or a default build."
    )
}

/// What an audit scanned: mode, roots, virtual modules, manifests and exclusions.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanCoverage {
    mode: &'static str,
    roots: Vec<CoverageRoot>,
    virtual_modules: Vec<String>,
    manifests: Vec<CoverageManifest>,
    exclusions: Vec<CoverageExclusion>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CoverageRoot {
    label: String,
    path: String,
    kind: &'static str,
}

#[derive(Serialize)]
struct CoverageManifest {
    producer: String,
    path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CoverageExclusion {
    pattern: String,
    reason: &'static str,
    origin: Option<String>,
}

fn plan_coverage(
    mode: WindAuditPlan,
    plan: &zfb_css::SourcePlan,
    plugin_virtual_modules: &[(String, String)],
    project_root: &Path,
) -> PlanCoverage {
    let display = |path: &Path| match path.strip_prefix(project_root) {
        Ok(relative) if relative.as_os_str().is_empty() => ".".to_owned(),
        Ok(relative) => relative.display().to_string(),
        Err(_) => path.display().to_string(),
    };
    let mut roots: Vec<_> = plan
        .roots
        .iter()
        .chain(plan.package_sources.values())
        .collect();
    roots.sort();
    PlanCoverage {
        mode: match mode {
            WindAuditPlan::Standalone => "standalone",
            WindAuditPlan::Build => "build",
        },
        roots: roots
            .into_iter()
            .map(|root| CoverageRoot {
                label: root.label.clone(),
                path: display(&zfb_types::normalize_path_lexical(&root.resolved_path())),
                kind: if root.package_root {
                    "packageRoot"
                } else if root.required {
                    "required"
                } else {
                    "optional"
                },
            })
            .collect(),
        virtual_modules: plugin_virtual_modules
            .iter()
            .map(|(specifier, _)| format!("plugin/{specifier}"))
            .collect(),
        manifests: plan
            .manifests
            .iter()
            .map(|(producer, path)| CoverageManifest {
                producer: producer.clone(),
                path: display(path),
            })
            .collect(),
        exclusions: plan
            .exclusions
            .iter()
            .map(|path| CoverageExclusion {
                pattern: display(path),
                reason: "outputOrScratch",
                origin: None,
            })
            .chain(
                plan.author_exclusions
                    .iter()
                    .map(|exclusion| CoverageExclusion {
                        pattern: exclusion.pattern.clone(),
                        reason: "windSourcesExclude",
                        origin: Some(exclusion.origin.clone()),
                    }),
            )
            .chain(std::iter::once(CoverageExclusion {
                pattern: "**/*.d.ts".to_owned(),
                reason: "declarationFiles",
                origin: None,
            }))
            .collect(),
    }
}

/// A human summary of what the audit covers, printed before the report.
fn render_plan_coverage(command: &str, coverage: &PlanCoverage) -> String {
    let mut out = format!("wind {command} plan: {}\n", coverage.mode);
    for root in &coverage.roots {
        let kind = match root.kind {
            "packageRoot" => "package root",
            other => other,
        };
        out.push_str(&format!("  root {} {} ({kind})\n", root.label, root.path));
    }
    for module in &coverage.virtual_modules {
        out.push_str(&format!("  virtual {module}\n"));
    }
    for manifest in &coverage.manifests {
        out.push_str(&format!(
            "  manifest {} {}\n",
            manifest.producer, manifest.path
        ));
    }
    for exclusion in &coverage.exclusions {
        let reason = match &exclusion.origin {
            Some(origin) => format!("wind.sources.exclude from {origin}"),
            None if exclusion.reason == "declarationFiles" => "declaration files".to_owned(),
            None => "output or scratch".to_owned(),
        };
        out.push_str(&format!("  excluded {} ({reason})\n", exclusion.pattern));
    }
    out
}

/// Plugin virtual modules have no file; audit them under the same
/// `plugin/<specifier>` identity the build source plan uses.
fn append_plugin_audit_sources(
    modules: &[(String, String)],
    sources: &mut Vec<zfb_css::AuditSource>,
) {
    for (specifier, source) in modules {
        sources.push(zfb_css::AuditSource::new(
            format!("plugin/{specifier}"),
            zfb_css::extract_candidates(source.as_bytes(), zfb_css::SourceKind::Tsx),
        ));
    }
}

fn print_audit_and_apply_exit_policy(
    report: &zfb_css::AuditReport,
    coverage: Option<&PlanCoverage>,
    output: AuditOutput,
    fail_on: Option<WindAuditFailOn>,
) -> Result<()> {
    let shown = filter_report(report, output.severity);
    if output.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schemaVersion": WIND_JSON_SCHEMA_VERSION,
                "command": "audit",
                "severity": output.severity.map(severity_name),
                "coverage": coverage,
                "report": shown,
            }))?
        );
    } else {
        if let Some(coverage) = coverage {
            print!("{}", render_plan_coverage("audit", coverage));
        }
        print!("{}", zfb_css::render_audit(&shown));
    }
    match audit_exit(report, fail_on) {
        Ok(Some(note)) => {
            eprintln!("{note}");
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(summary) => Err(anyhow::Error::msg(summary)),
    }
}

fn severity_name(severity: WindAuditSeverity) -> &'static str {
    match severity {
        WindAuditSeverity::AuditInfo => "auditInfo",
        WindAuditSeverity::Warning => "warning",
        WindAuditSeverity::Error => "error",
    }
}

fn severity_rank(severity: &str) -> Option<WindAuditSeverity> {
    match severity {
        "auditInfo" => Some(WindAuditSeverity::AuditInfo),
        "warning" => Some(WindAuditSeverity::Warning),
        "error" => Some(WindAuditSeverity::Error),
        _ => None,
    }
}

/// The report with diagnostics below `minimum` hidden. Display only; the
/// exit verdict always reads the complete report.
fn filter_report(
    report: &zfb_css::AuditReport,
    minimum: Option<WindAuditSeverity>,
) -> zfb_css::AuditReport {
    let mut shown = report.clone();
    if let Some(minimum) = minimum {
        let keep = |severity: &str| severity_rank(severity).is_none_or(|rank| rank >= minimum);
        shown
            .diagnostics
            .retain(|diagnostic| keep(&diagnostic.severity));
        shown
            .dead_classes
            .retain(|dead| keep(&dead.diagnostic.severity));
    }
    shown
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
    use std::path::PathBuf;

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

    #[test]
    fn plan_coverage_names_mode_roots_virtual_sources_and_exclusion_reasons() {
        let project = Path::new("/project");
        let mut plan = zfb_css::SourcePlan::default();
        plan.roots.push(zfb_css::PositiveRoot {
            label: "default/src".into(),
            declaring_dir: project.join("src"),
            path: PathBuf::from("."),
            required: false,
            exclusions: BTreeSet::new(),
            package_root: false,
        });
        plan.roots.push(zfb_css::PositiveRoot {
            label: "package-root/node_modules/@x/ui".into(),
            declaring_dir: project.join("node_modules/@x/ui"),
            path: PathBuf::from("."),
            required: true,
            exclusions: BTreeSet::new(),
            package_root: true,
        });
        plan.package_sources.insert(
            "package-route/node_modules/@x/docs".into(),
            zfb_css::PositiveRoot {
                label: "package-route/node_modules/@x/docs".into(),
                declaring_dir: project.join("node_modules/@x/docs"),
                path: PathBuf::from("."),
                required: true,
                exclusions: BTreeSet::new(),
                package_root: false,
            },
        );
        plan.exclusions.insert(project.join("dist"));
        plan.author_exclusions.push(zfb_css::SourceExclusion {
            origin: "project".into(),
            declaring_dir: project.to_path_buf(),
            pattern: "src/**/__tests__/**".into(),
        });
        let modules = vec![(
            "virtual:menu".to_owned(),
            "<a className=\"p-1\" />".to_owned(),
        )];

        let structured = plan_coverage(WindAuditPlan::Build, &plan, &modules, project);
        assert_eq!(
            serde_json::to_value(&structured).unwrap()["roots"][1],
            serde_json::json!({
                "label": "package-root/node_modules/@x/ui",
                "path": "node_modules/@x/ui",
                "kind": "packageRoot"
            })
        );
        assert_eq!(
            serde_json::to_value(&structured).unwrap()["exclusions"][1],
            serde_json::json!({
                "pattern": "src/**/__tests__/**",
                "reason": "windSourcesExclude",
                "origin": "project"
            })
        );
        assert_eq!(
            serde_json::to_value(&structured).unwrap()["exclusions"][2],
            serde_json::json!({
                "pattern": "**/*.d.ts",
                "reason": "declarationFiles",
                "origin": null
            })
        );
        let coverage = render_plan_coverage("audit", &structured);
        assert_eq!(
            coverage,
            "wind audit plan: build\n\
             \x20 root default/src src (optional)\n\
             \x20 root package-root/node_modules/@x/ui node_modules/@x/ui (package root)\n\
             \x20 root package-route/node_modules/@x/docs node_modules/@x/docs (required)\n\
             \x20 virtual plugin/virtual:menu\n\
             \x20 excluded dist (output or scratch)\n\
             \x20 excluded src/**/__tests__/** (wind.sources.exclude from project)\n\
             \x20 excluded **/*.d.ts (declaration files)\n"
        );
        assert!(render_plan_coverage(
            "audit",
            &plan_coverage(
                WindAuditPlan::Standalone,
                &zfb_css::SourcePlan::default(),
                &[],
                project
            )
        )
        .starts_with("wind audit plan: standalone\n"));

        let mut sources = Vec::new();
        append_plugin_audit_sources(&modules, &mut sources);
        assert_eq!(sources[0].source_id, "plugin/virtual:menu");
        assert_eq!(sources[0].extraction.candidates[0].text, "p-1");
    }

    #[test]
    fn severity_filter_hides_lower_diagnostics_without_changing_the_verdict() {
        let report = audit_report(
            zfb_css::AuditOutcome::Complete,
            &["auditInfo", "warning", "error", "auditInfo"],
        );
        let names = |report: &zfb_css::AuditReport| {
            report
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.severity.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&filter_report(&report, None)).len(), 4);
        assert_eq!(
            names(&filter_report(&report, Some(WindAuditSeverity::Warning))),
            ["warning", "error"]
        );
        assert_eq!(
            names(&filter_report(&report, Some(WindAuditSeverity::Error))),
            ["error"]
        );
        // The CLI always passes the complete report to the exit policy;
        // filtering to errors alone must not hide the warning verdict.
        assert!(audit_exit(&report, Some(WindAuditFailOn::Warning))
            .unwrap_err()
            .contains("2 error- or warning-severity"));
    }

    #[test]
    fn stdin_candidates_keep_order_and_duplicates() {
        assert_eq!(
            stdin_candidates("  p-4\r\n\n-mt-2\np-4\n   \nhover:block"),
            ["p-4", "-mt-2", "p-4", "hover:block"]
        );
    }
}
