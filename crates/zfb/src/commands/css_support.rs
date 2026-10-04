//! CSS helpers shared by command-layer entrypoints.
//!
//! This module owns the command-side setup that is common to `zfb build` and
//! the standalone `zfb css` entrypoint.  It deliberately returns
//! [`zfb_css::CssEmitterOutput`] rather than the build pipeline's
//! [`zfb_build::pipeline::AssetEmitterPayload`], keeping the CSS command
//! independent of production HTML/asset rewriting.

#![cfg_attr(not(feature = "embed_v8"), allow(unused_imports, dead_code))]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use zfb_css::{
    AuditSource, CandidateIndex, CssDiagnostic, CssDiagnosticOrigin, CssDiagnosticSeverity,
    CssEmitterOutput, CssEngine, CssPipeline, CssPipelineConfig, Origin, OriginCandidate,
    PositiveRoot, SourceId, SourcePlan, SourcePositionKind,
};

use crate::config::{CodeHighlightMode, Config, WindDarkSetting, WindSetting};

pub(crate) struct StandaloneWindIndex {
    pub candidates: BTreeSet<String>,
    pub origins: Vec<OriginCandidate>,
    pub diagnostics: Vec<CssDiagnostic>,
    pub audit_sources: Vec<AuditSource>,
}

/// The project root for `zfb css` and `zfb wind`: `--project-root`, else the
/// `--config` file's directory, else `cwd`.
pub(crate) fn command_project_root(
    cwd: &Path,
    project_root: Option<&Path>,
    config: Option<&Path>,
) -> PathBuf {
    let root = match (project_root, config) {
        (Some(root), _) => cwd.join(root),
        (None, Some(config)) => {
            let config = zfb_types::normalize_path_lexical(&cwd.join(config));
            config.parent().unwrap_or(cwd).to_path_buf()
        }
        (None, None) => cwd.to_path_buf(),
    };
    zfb_types::normalize_path_lexical(&root)
}

/// Load the explicitly selected config file, or discover one in the project
/// root.
pub(crate) async fn load_command_config(
    cwd: &Path,
    project_root: &Path,
    config: Option<&Path>,
) -> Result<Config> {
    match config {
        Some(config) => crate::config::load_from_file(&cwd.join(config)).await,
        None => crate::config::load_from_dir(project_root).await,
    }
}

/// Convert the public project config shape to the pure compiler config.
pub(crate) fn map_wind_config(input: &crate::config::WindConfig) -> zfb_css::WindConfig {
    let mut output = zfb_css::WindConfig {
        spec: input.spec,
        reset: match input.reset.as_str() {
            "minimal-v1" => zfb_css::ResetMode::MinimalV1,
            "owned-v1" => zfb_css::ResetMode::OwnedV1,
            _ => zfb_css::ResetMode::None,
        },
        dark: match &input.dark {
            WindDarkSetting::Enabled(dark) => Some(zfb_css::DarkModeConfig {
                attribute: dark.attribute.clone(),
                value: dark.value.clone(),
            }),
            WindDarkSetting::Disabled => None,
        },
        breakpoints: input
            .breakpoints
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    zfb_css::BreakpointConfig {
                        min_width_px: value.min_width_px as i64,
                    },
                )
            })
            .collect(),
        safelist: input.safelist.clone(),
        authored_classes: input.authored_classes.clone(),
        strict: input.strict,
        utility_placement: match input.utilities.placement {
            crate::config::WindUtilityPlacement::AfterAuthored => {
                zfb_css::UtilityPlacement::AfterAuthored
            }
            crate::config::WindUtilityPlacement::BeforeAuthored => {
                zfb_css::UtilityPlacement::BeforeAuthored
            }
        },
        default_transition_timing_function: input.default_transition_timing_function.clone(),
        ..Default::default()
    };
    output.tokens.spacing_unit = input.tokens.spacing_unit.clone();
    output.tokens.colors = input.tokens.colors.clone();
    output.tokens.spacing = input.tokens.spacing.clone();
    output.tokens.sizes = input.tokens.sizes.clone();
    output.tokens.font_sizes = input
        .tokens
        .font_sizes
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                zfb_css::FontSizeToken {
                    size: value.size.clone(),
                    line_height: value.line_height.clone(),
                },
            )
        })
        .collect();
    output.tokens.font_families = input.tokens.font_families.clone();
    output.tokens.font_weights = input.tokens.font_weights.clone();
    output.tokens.line_heights = input.tokens.line_heights.clone();
    output.tokens.letter_spacings = input.tokens.letter_spacings.clone();
    output.tokens.radii = input.tokens.radii.clone();
    output.tokens.shadows = input.tokens.shadows.clone();
    output.tokens.z_indices = input.tokens.z_indices.clone();
    output.tokens.easings = input.tokens.easings.clone();
    output
}

/// Build the standalone CLI source plan used by both `zfb css` and `zfb wind
/// audit`. The build/dev plan has additional package-route, mirror and plugin
/// roots; this command owns only its five conventional roots, configured
/// package roots and exclusions, plus explicit CLI declarations.
pub(crate) fn build_standalone_wind_source_plan(
    project_root: &Path,
    output_path: &Path,
    config: &Config,
    include_default_roots: bool,
    explicit_sources: &[(String, PathBuf)],
) -> Result<(SourcePlan, Vec<String>)> {
    let gathered = crate::commands::css_source_plan::gather_css_source_plan_inputs(
        project_root,
        output_path,
        config,
        &[],
        &[],
        &[],
        &zfb_types::ScratchLayout::default_for(project_root).written_roots(),
    )?;
    let exclusion_reasons = explicit_source_exclusion_reasons(&gathered);
    let build_plan = crate::commands::css_source_plan::build_css_source_plan(&gathered);
    let mut plan = SourcePlan {
        roots: build_plan
            .roots
            .into_iter()
            .filter(|root| root.package_root)
            .collect(),
        exclusions: build_plan.exclusions,
        author_exclusions: build_plan.author_exclusions,
        manifests: build_plan.manifests,
        safelist: build_plan.safelist,
        generated_sources: build_plan.generated_sources,
        ..SourcePlan::default()
    };
    // The standalone command explicitly accepts the two extractor kinds that
    // build/dev intentionally do not discover automatically.
    plan.extensions
        .extend(["html".to_owned(), "mjs".to_owned()]);

    if include_default_roots {
        for name in ["pages", "components", "layouts", "content", "src"] {
            plan.roots.push(PositiveRoot {
                label: format!("default/{name}"),
                declaring_dir: project_root.to_path_buf(),
                path: PathBuf::from(name),
                required: false,
                exclusions: BTreeSet::new(),
                package_root: false,
            });
        }
    }

    let mut warnings = Vec::new();
    for (index, (authored, pattern)) in explicit_sources.iter().enumerate() {
        let (roots, warning) =
            explicit_source_roots(project_root, authored, pattern, index, &exclusion_reasons)?;
        plan.roots.extend(roots);
        if let Some(warning) = warning {
            warnings.push(warning);
        }
    }
    plan.roots.sort();
    Ok((plan, warnings))
}

#[derive(Clone, Debug)]
struct ExplicitSourceExclusion {
    canonical_path: PathBuf,
    author: Option<zfb_css::ExclusionMatcher>,
    description: String,
}

fn explicit_source_exclusion_reasons(
    inputs: &crate::commands::css_source_plan::CssSourcePlanInputs,
) -> Vec<ExplicitSourceExclusion> {
    let mut reasons = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push_reason = |path: &Path, description: String| {
        let canonical_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if seen.insert(canonical_path.clone()) {
            reasons.push(ExplicitSourceExclusion {
                canonical_path,
                author: None,
                description,
            });
        }
    };

    push_reason(
        &inputs.configured_output_dir,
        format!(
            "under the configured outDir `{}`",
            project_relative_display(&inputs.project_root, &inputs.configured_output_dir)
        ),
    );
    push_reason(
        &inputs.pass_output_dir,
        format!(
            "at the --output path `{}`",
            project_relative_display(&inputs.project_root, &inputs.pass_output_dir)
        ),
    );
    let mut scratch_roots = inputs.zfb_written_roots.clone();
    scratch_roots.sort();
    for path in scratch_roots {
        push_reason(
            &path,
            format!(
                "under a zfb scratch root `{}`",
                project_relative_display(&inputs.project_root, &path)
            ),
        );
    }
    for exclusion in &inputs.author_exclusions {
        if let Ok(matcher) = exclusion.compile() {
            reasons.push(ExplicitSourceExclusion {
                canonical_path: PathBuf::new(),
                author: Some(matcher),
                description: format!(
                    "matching the wind.sources.exclude pattern {:?} declared by {}",
                    exclusion.pattern, exclusion.origin
                ),
            });
        }
    }
    reasons
}

fn project_relative_display(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

fn canonical_or_raw(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn matching_exclusion<'a>(
    path: &Path,
    reasons: &'a [ExplicitSourceExclusion],
) -> Option<(usize, &'a ExplicitSourceExclusion)> {
    let canonical_path = canonical_or_raw(path);
    reasons
        .iter()
        .enumerate()
        .find(|(_, reason)| match &reason.author {
            Some(matcher) => matcher.matches_identity(path, Some(&canonical_path)),
            None => canonical_path.starts_with(&reason.canonical_path),
        })
}

fn render_exclusion_counts(
    counts: &BTreeMap<usize, usize>,
    reasons: &[ExplicitSourceExclusion],
) -> String {
    counts
        .iter()
        .map(|(index, count)| format!("{count} {}", reasons[*index].description))
        .collect::<Vec<_>>()
        .join("; ")
}

fn explicit_source_roots(
    project_root: &Path,
    authored: &str,
    pattern: &Path,
    source_index: usize,
    exclusion_reasons: &[ExplicitSourceExclusion],
) -> Result<(Vec<PositiveRoot>, Option<String>)> {
    let matches = if path_has_glob(pattern) {
        glob_source_files(pattern)?
    } else if pattern.is_file() {
        vec![pattern.to_path_buf()]
    } else if pattern.is_dir() {
        // Directory declarations are classified by their root only. Descendant
        // files filtered by exclusions during the walk are out of scope here.
        if let Some((_, reason)) = matching_exclusion(pattern, exclusion_reasons) {
            bail!(
                "ZW010: --source {authored:?} matched 1 directory root, all excluded: 1 {}",
                reason.description
            );
        }
        return Ok((
            vec![PositiveRoot {
                label: format!("cli/source-{source_index:04}"),
                declaring_dir: pattern.to_path_buf(),
                path: PathBuf::from("."),
                required: true,
                exclusions: BTreeSet::new(),
                package_root: false,
            }],
            None,
        ));
    } else {
        Vec::new()
    };

    if matches.is_empty() {
        bail!(
            "--source glob {authored:?} matched zero files (resolved as {})",
            pattern.display()
        );
    }
    let supported = ["tsx", "ts", "jsx", "js", "mdx", "md", "html", "mjs"];
    for path in &matches {
        let extension = path.extension().and_then(|part| part.to_str());
        if !extension.is_some_and(|extension| supported.contains(&extension)) {
            bail!(
                "ZW010: explicit --source file has an unsupported extension: {}",
                path.display()
            );
        }
    }

    // Match the walker identity: aliases and symlinks to the same file count
    // once, with the canonical path used when it is available.
    let mut seen = BTreeSet::new();
    let matches = matches
        .into_iter()
        .filter(|path| seen.insert(canonical_or_raw(path)))
        .collect::<Vec<_>>();
    let mut accepted = Vec::with_capacity(matches.len());
    let mut excluded_counts = BTreeMap::<usize, usize>::new();
    for (match_index, path) in matches.iter().enumerate() {
        if let Some((reason_index, _)) = matching_exclusion(path, exclusion_reasons) {
            *excluded_counts.entry(reason_index).or_default() += 1;
        } else {
            accepted.push((match_index, path));
        }
    }
    if accepted.is_empty() {
        bail!(
            "ZW010: --source {authored:?} matched {} files, all excluded: {}",
            matches.len(),
            render_exclusion_counts(&excluded_counts, exclusion_reasons)
        );
    }

    let excluded_count: usize = excluded_counts.values().sum();
    let warning = (excluded_count > 0).then(|| {
        format!(
            "ZW010: --source {authored:?} matched {} files; accepted {}; excluded: {}",
            matches.len(),
            accepted.len(),
            render_exclusion_counts(&excluded_counts, exclusion_reasons)
        )
    });

    let mut roots = Vec::with_capacity(accepted.len());
    for (match_index, path) in accepted {
        let parent = path.parent().unwrap_or(project_root);
        let name = path
            .file_name()
            .context("explicit --source path has no filename")?;
        let label = if path_has_glob(pattern) {
            format!("cli/source-{source_index:04}/match-{match_index:04}")
        } else {
            format!("cli/source-{source_index:04}")
        };
        roots.push(PositiveRoot {
            label,
            declaring_dir: parent.to_path_buf(),
            path: PathBuf::from(name),
            required: true,
            exclusions: BTreeSet::new(),
            package_root: false,
        });
    }
    Ok((roots, warning))
}

fn path_has_glob(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .bytes()
            .any(|byte| matches!(byte, b'*' | b'?' | b'[' | b'{'))
    })
}

fn glob_source_files(pattern: &Path) -> Result<Vec<PathBuf>> {
    let components = pattern.components().collect::<Vec<_>>();
    let wildcard_at = components
        .iter()
        .position(|component| {
            component
                .as_os_str()
                .to_string_lossy()
                .bytes()
                .any(|byte| matches!(byte, b'*' | b'?' | b'[' | b'{'))
        })
        .context("source glob does not contain a wildcard")?;
    let mut root = PathBuf::new();
    for component in &components[..wildcard_at] {
        root.push(component.as_os_str());
    }
    if root.as_os_str().is_empty() {
        root.push(".");
    }
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let suffix = components[wildcard_at..]
        .iter()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    let mut overrides = ignore::overrides::OverrideBuilder::new(&root);
    overrides
        .add(&format!("/{suffix}"))
        .with_context(|| format!("invalid source glob {suffix:?}"))?;
    let overrides = overrides
        .build()
        .with_context(|| format!("invalid source glob {suffix:?}"))?;
    let mut walker = ignore::WalkBuilder::new(&root);
    walker
        .follow_links(false)
        .standard_filters(false)
        .overrides(overrides);
    let mut files = walker
        .build()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    Ok(files)
}

/// Read and index source files, manifests and generated role classes from the
/// same explicit plan used by both CLI commands.
pub(crate) fn index_standalone_wind_sources(plan: &SourcePlan) -> Result<StandaloneWindIndex> {
    let expanded = zfb_css::expand_file_set(plan);
    let mut index = CandidateIndex::default();
    let mut origins = Vec::new();
    let mut diagnostics = expanded
        .diagnostics
        .into_iter()
        .map(|diagnostic| {
            let required = diagnostic.message.starts_with("required source missing");
            CssDiagnostic {
                severity: if required {
                    CssDiagnosticSeverity::Error
                } else {
                    CssDiagnosticSeverity::Warning
                },
                code: if required { "ZW010" } else { "ZW011" }.into(),
                message: format!("{}: {}", diagnostic.root_label, diagnostic.message),
                origin: CssDiagnosticOrigin::default(),
                candidate: None,
            }
        })
        .collect::<Vec<_>>();
    let mut audit_sources = Vec::new();
    for file in expanded.files {
        let extracted = match index.index_file(&file) {
            Ok(extracted) => extracted,
            Err(error) => {
                diagnostics.push(CssDiagnostic {
                    severity: CssDiagnosticSeverity::Warning,
                    code: "ZW011".into(),
                    message: format!("skipped source {}: {error}", file.path.display()),
                    origin: CssDiagnosticOrigin {
                        path: Some(file.path.clone()),
                        ..Default::default()
                    },
                    candidate: None,
                });
                continue;
            }
        };
        if extracted
            .notes
            .iter()
            .any(|note| note.kind == zfb_css::NoteKind::InvalidUtf8)
        {
            diagnostics.push(CssDiagnostic {
                severity: CssDiagnosticSeverity::Warning,
                code: "ZW011".into(),
                message: format!("skipped non-UTF-8 source {}", file.path.display()),
                origin: CssDiagnosticOrigin {
                    path: Some(file.path.clone()),
                    ..Default::default()
                },
                candidate: None,
            });
        }
        let source_id = file.id.render();
        for candidate in &extracted.candidates {
            for occurrence in &candidate.occurrences {
                origins.push(OriginCandidate {
                    text: candidate.text.clone(),
                    origin: Origin::Source {
                        source_id: source_id.clone(),
                        byte_offset: occurrence.byte_offset,
                        byte_length: occurrence.byte_length,
                        line: occurrence.line,
                        byte_column: occurrence.byte_column,
                        literal_byte_offset: occurrence.literal_byte_offset,
                        literal_byte_length: occurrence.literal_byte_length,
                        position_kind: match occurrence.position_kind {
                            zfb_css::PositionKind::Class => SourcePositionKind::Class,
                            zfb_css::PositionKind::Literal => SourcePositionKind::Literal,
                        },
                    },
                });
            }
        }
        audit_sources.push(AuditSource::new(source_id, extracted));
    }

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
        let candidates = entries
            .iter()
            .enumerate()
            .map(|(position, entry)| {
                let text = entry.as_str().with_context(|| {
                    format!("wind manifest {producer} candidate {position} is not a string")
                })?;
                origins.push(OriginCandidate {
                    text: text.to_owned(),
                    origin: Origin::Manifest {
                        producer: producer.clone(),
                        path: path.display().to_string(),
                        index: position,
                    },
                });
                Ok(text.to_owned())
            })
            .collect::<Result<Vec<_>>>()?;
        index.replace_manifest(producer.clone(), candidates);
    }
    for (source, candidates) in &plan.generated_sources {
        let source_id = SourceId::new("generated", source)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("invalid generated source id {source:?}"))?;
        index.upsert(source_id, candidates.iter().cloned());
        if source == "code-highlight/role-classes" {
            for candidate in candidates {
                origins.push(OriginCandidate {
                    text: candidate.clone(),
                    origin: Origin::RoleClass {
                        role_key: candidate.clone(),
                    },
                });
            }
        }
    }
    for (owner, candidates) in &plan.safelist {
        index.replace_safelist(owner.clone(), candidates.iter().cloned());
    }
    diagnostics.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then(left.message.cmp(&right.message))
    });
    Ok(StandaloneWindIndex {
        candidates: index.live_set(),
        origins,
        diagnostics,
        audit_sources,
    })
}

pub(crate) fn configured_wind(config: &Config) -> (bool, zfb_css::WindConfig) {
    match config.wind.as_ref() {
        Some(WindSetting::Disabled) => (false, zfb_css::WindConfig::default()),
        Some(WindSetting::Enabled(config)) => (true, map_wind_config(config)),
        None => (true, zfb_css::WindConfig::default()),
    }
}

/// Resolve the framework CSS block ([`CssPipelineConfig::framework_css`]) for
/// the current configuration.
///
/// Ships `zfb_css::default_hi_css()` — the framework's default
/// `--zfb-hi-*` token stylesheet for class-mode syntax highlighting — iff the
/// resolved config has `codeHighlight.mode == "class"` and the user has not
/// opted out via `codeHighlight.defaultStylesheet: false`.  `None` in every
/// other case (including the default `mode: "inline"`), preserving the
/// existing build output for inline-mode projects.
///
/// `default_hi_css()` hardcodes `.hi-*` role selectors, matching the default
/// `codeHighlight.classPrefix` of `"hi-"`.  When a project uses a custom
/// prefix, rewrite only the selector prefix; the `--zfb-hi-*` custom
/// properties remain independently namespaced.
pub(crate) fn resolve_framework_css(config: &Config) -> Option<String> {
    resolve_framework_css_with_options(config, None, false)
}

/// Resolve framework CSS while applying the standalone command's CLI
/// overrides. The class prefix and role configuration always remain owned by
/// `zfb.config`; only mode and default-stylesheet selection are overridden.
pub(crate) fn resolve_framework_css_with_options(
    config: &Config,
    mode_override: Option<CodeHighlightMode>,
    no_default_stylesheet: bool,
) -> Option<String> {
    let code_highlight = config.code_highlight.as_ref();
    let mode = mode_override.or_else(|| code_highlight.map(|value| value.mode))?;
    let default_stylesheet = code_highlight
        .map(|value| value.default_stylesheet)
        .unwrap_or(true);
    if mode != CodeHighlightMode::Class || no_default_stylesheet || !default_stylesheet {
        return None;
    }
    let css = zfb_css::default_hi_css();
    let prefix = code_highlight
        .map(|value| value.class_prefix.as_str())
        .unwrap_or(zfb_content::syntect_highlight::DEFAULT_CLASS_HIGHLIGHT_PREFIX);
    if prefix == "hi-" {
        Some(css.to_string())
    } else {
        Some(css.replace(".hi-", &format!(".{prefix}")))
    }
}

/// Collect the wind utility candidates configured by
/// `codeHighlight.roleClasses`.
///
/// Values are split on whitespace, deduplicated, and sorted so the generated
/// entry CSS (and consequently its asset hash) is deterministic.
pub(crate) fn role_classes_inline_sources(config: &Config) -> Vec<String> {
    let mut classes: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if let Some(role_classes) = config
        .code_highlight
        .as_ref()
        .and_then(|ch| ch.role_classes.as_ref())
    {
        for value in role_classes.values() {
            classes.extend(value.split_whitespace().map(str::to_string));
        }
    }
    classes.into_iter().collect()
}

/// Run the shared CSS pipeline and return its engine-agnostic emitter output.
///
/// The build command adapts the returned [`CssEmitterOutput`] into its
/// `AssetEmitterPayload` at the build-only boundary.  The standalone CSS
/// command can consume this output directly without depending on production
/// asset graph types.
pub(crate) fn run_css_emitter<E: CssEngine>(
    engine: E,
    project_root: &Path,
    outdir: &Path,
    sources: Vec<PathBuf>,
    // `.module.css` files a registered virtual module imports directly.  The
    // build command supplies these for its CSS Modules path; the standalone
    // CSS command passes an empty list because CSS Modules are out of scope.
    explicit_css_modules: Vec<PathBuf>,
    framework_css: Option<String>,
) -> Result<CssEmitterOutput> {
    run_css_emitter_with_module_policy(
        engine,
        project_root,
        outdir,
        sources,
        explicit_css_modules,
        framework_css,
        true,
    )
}

/// Standalone-command variant that enforces the CSS Modules v1 exclusion:
/// no explicit modules and no source scan for implicit `.module.css` imports.
pub(crate) fn run_css_emitter_without_modules<E: CssEngine>(
    engine: E,
    project_root: &Path,
    outdir: &Path,
    framework_css: Option<String>,
) -> Result<CssEmitterOutput> {
    run_css_emitter_with_module_policy(
        engine,
        project_root,
        outdir,
        Vec::new(),
        Vec::new(),
        framework_css,
        false,
    )
}

fn run_css_emitter_with_module_policy<E: CssEngine>(
    engine: E,
    project_root: &Path,
    outdir: &Path,
    sources: Vec<PathBuf>,
    explicit_css_modules: Vec<PathBuf>,
    framework_css: Option<String>,
    auto_discover_modules: bool,
) -> Result<CssEmitterOutput> {
    let pipe_cfg = CssPipelineConfig {
        sources,
        css_modules: explicit_css_modules,
        // The on-disk class-map JSON writer is not used: the build-time CSS
        // Modules rewrite consumes maps in memory.
        class_map_dir: None,
        // `output_root` is unused by `build_emitter` while `class_map_dir` is
        // unset, but pin it to the configured outdir for forward-compatibility.
        output_root: outdir.to_path_buf(),
        // Keep the CSS Modules hash root aligned with the class-map producer.
        modules_config: zfb_css::modules::CssModulesConfig::for_project_and_first_party_roots(
            project_root,
            &zfb_types::first_party_root_for(project_root),
        ),
        auto_discover_modules,
        framework_css,
        ..CssPipelineConfig::default()
    };

    CssPipeline::new(engine, pipe_cfg).build_emitter()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_project_root_defaults_to_the_config_directory() {
        let cwd = Path::new("/work/pkg");
        let config = Some(Path::new("../shared/./zfb.config.ts"));
        assert_eq!(
            command_project_root(cwd, None, config),
            Path::new("/work/shared")
        );
        assert_eq!(command_project_root(cwd, Some(Path::new(".")), config), cwd);
        assert_eq!(command_project_root(cwd, None, None), cwd);
        assert_eq!(
            command_project_root(cwd, Some(Path::new("/abs/root")), None),
            Path::new("/abs/root")
        );
    }

    fn project() -> (tempfile::TempDir, PathBuf, Config) {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        (temp, project, Config::default())
    }

    fn write_file(project: &Path, relative: &str) -> PathBuf {
        let path = project.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "<div class=\"block\" />").unwrap();
        path
    }

    fn build_plan(
        project: &Path,
        config: &Config,
        authored: &str,
    ) -> Result<(SourcePlan, Vec<String>)> {
        build_standalone_wind_source_plan(
            project,
            &project.join("dist/out.css"),
            config,
            false,
            &[(authored.to_owned(), project.join(authored))],
        )
    }

    #[test]
    fn default_out_dir_explicit_match_fails_with_exclusion_count() {
        let (_temp, project, config) = project();
        write_file(&project, "dist/app.tsx");

        let error = build_plan(&project, &config, "dist/**/*.tsx").unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("ZW010"), "{message}");
        assert!(message.contains("matched 1 files"), "{message}");
        assert!(
            message.contains("1 under the configured outDir `dist`"),
            "{message}"
        );
    }

    #[test]
    fn author_exclusions_win_explicit_sources_and_package_roots_are_standalone_roots() {
        let (_temp, project, mut config) = project();
        write_file(&project, "src/a.tsx");
        write_file(&project, "src/__tests__/b.test.tsx");
        write_file(&project, "packages/ui/view.tsx");
        let wind = crate::config::WindConfig {
            sources: serde_json::from_value(serde_json::json!({
                "exclude": ["src/**/__tests__/**"],
                "packageRoots": ["./packages/ui"]
            }))
            .unwrap(),
            ..Default::default()
        };
        config.wind = Some(WindSetting::Enabled(Box::new(wind)));

        let message = format!(
            "{:#}",
            build_plan(&project, &config, "src/__tests__/b.test.tsx").unwrap_err()
        );
        assert!(message.contains("ZW010"), "{message}");
        assert!(
            message.contains(
                "1 matching the wind.sources.exclude pattern \"src/**/__tests__/**\" declared by project"
            ),
            "{message}"
        );

        let (plan, warnings) = build_plan(&project, &config, "src/**/*.tsx").unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("accepted 1"), "{}", warnings[0]);
        let roots: Vec<_> = plan
            .roots
            .iter()
            .map(|root| (root.package_root, root.resolved_path()))
            .collect();
        assert_eq!(
            roots,
            [
                (false, project.join("src/a.tsx")),
                (true, project.join("packages/ui")),
            ]
        );
        assert_eq!(plan.author_exclusions.len(), 1);
    }

    #[test]
    fn mixed_explicit_matches_keep_sources_and_return_one_warning() {
        let (_temp, project, config) = project();
        write_file(&project, "src/a.tsx");
        write_file(&project, "dist/b.tsx");

        let (plan, warnings) = build_plan(&project, &config, "**/*.tsx").unwrap();
        assert_eq!(plan.roots.len(), 1);
        assert_eq!(plan.roots[0].resolved_path(), project.join("src/a.tsx"));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("matched 2 files"), "{}", warnings[0]);
        assert!(warnings[0].contains("accepted 1"), "{}", warnings[0]);
        assert!(warnings[0].contains("dist"), "{}", warnings[0]);
    }

    #[test]
    fn custom_out_dir_does_not_exclude_the_default_dist_directory() {
        let (_temp, project, mut config) = project();
        config.out_dir = PathBuf::from("out");
        write_file(&project, "dist/app.tsx");

        let (plan, warnings) = build_plan(&project, &config, "dist/**/*.tsx").unwrap();
        assert_eq!(plan.roots.len(), 1);
        assert!(warnings.is_empty());
    }

    #[test]
    fn package_dist_and_build_directories_remain_valid_sources() {
        let (_temp, project, config) = project();
        write_file(&project, "node_modules/pkg/dist/index.js");
        write_file(&project, "build/page.tsx");

        let (package_plan, package_warnings) =
            build_plan(&project, &config, "node_modules/pkg/dist/*.js").unwrap();
        let (build_plan, build_warnings) = build_plan(&project, &config, "build/**/*.tsx").unwrap();
        assert_eq!(package_plan.roots.len(), 1);
        assert_eq!(build_plan.roots.len(), 1);
        assert!(package_warnings.is_empty());
        assert!(build_warnings.is_empty());
    }

    #[test]
    fn scratch_root_match_reports_its_reason() {
        let (_temp, project, config) = project();
        let source = write_file(&project, ".zfb/cache.tsx");
        let error = build_standalone_wind_source_plan(
            &project,
            &project.join("dist/out.css"),
            &config,
            false,
            &[(".zfb/cache.tsx".to_owned(), source)],
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("under a zfb scratch root `.zfb`"),
            "{message}"
        );
    }

    #[test]
    fn explicit_directory_root_is_classified_without_scanning_descendants() {
        let (_temp, project, config) = project();
        write_file(&project, "dist/app.tsx");

        let error = build_plan(&project, &config, "dist").unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("matched 1 directory root"), "{message}");
        assert!(message.contains("all excluded"), "{message}");
        assert!(message.contains("configured outDir `dist`"), "{message}");
    }

    #[cfg(unix)]
    #[test]
    fn direct_symlink_match_is_classified_by_its_canonical_target() {
        use std::os::unix::fs::symlink;

        let (_temp, project, config) = project();
        let target = write_file(&project, "dist/target.tsx");
        let link = project.join("src/link.tsx");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(target, &link).unwrap();

        let error = build_standalone_wind_source_plan(
            &project,
            &project.join("dist/out.css"),
            &config,
            false,
            &[("src/link.tsx".to_owned(), link)],
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("configured outDir `dist`"), "{message}");
        assert!(message.contains("all excluded"), "{message}");
    }

    #[test]
    fn unsupported_extension_error_precedes_exclusion_classification() {
        let (_temp, project, config) = project();
        let source = write_file(&project, "dist/notes.txt");

        let error = build_standalone_wind_source_plan(
            &project,
            &project.join("dist/out.css"),
            &config,
            false,
            &[("dist/notes.txt".to_owned(), source)],
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("unsupported extension"), "{message}");
        assert!(!message.contains("all excluded"), "{message}");
    }

    #[test]
    fn output_path_match_is_grouped_under_the_output_reason() {
        let (_temp, project, config) = project();
        let output = write_file(&project, "src/generated.tsx");
        let error = build_standalone_wind_source_plan(
            &project,
            &output,
            &config,
            false,
            &[("src/generated.tsx".to_owned(), output.clone())],
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("at the --output path `src/generated.tsx`"),
            "{message}"
        );
    }
}
