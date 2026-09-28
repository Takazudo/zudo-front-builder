//! CSS helpers shared by command-layer entrypoints.
//!
//! This module owns the command-side setup that is common to `zfb build` and
//! the standalone `zfb css` entrypoint.  It deliberately returns
//! [`zfb_css::CssEmitterOutput`] rather than the build pipeline's
//! [`zfb_build::pipeline::AssetEmitterPayload`], keeping the CSS command
//! independent of production HTML/asset rewriting.

#![cfg_attr(not(feature = "embed_v8"), allow(unused_imports, dead_code))]

use std::collections::BTreeSet;
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
/// roots; this command owns only its five conventional roots plus explicit
/// CLI declarations.
pub(crate) fn build_standalone_wind_source_plan(
    project_root: &Path,
    output_path: &Path,
    config: &Config,
    include_default_roots: bool,
    explicit_sources: &[(String, PathBuf)],
) -> Result<SourcePlan> {
    let gathered = crate::commands::css_source_plan::gather_css_source_plan_inputs(
        project_root,
        output_path,
        config,
        &[],
        &[],
        &[],
    )?;
    let build_plan = crate::commands::css_source_plan::build_css_source_plan(&gathered);
    let mut plan = SourcePlan {
        exclusions: build_plan.exclusions,
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
            });
        }
    }

    for (index, (authored, pattern)) in explicit_sources.iter().enumerate() {
        let roots = explicit_source_roots(project_root, authored, pattern, index)?;
        plan.roots.extend(roots);
    }
    plan.roots.sort();
    Ok(plan)
}

fn explicit_source_roots(
    project_root: &Path,
    authored: &str,
    pattern: &Path,
    source_index: usize,
) -> Result<Vec<PositiveRoot>> {
    let matches = if path_has_glob(pattern) {
        glob_source_files(pattern)?
    } else if pattern.is_file() {
        vec![pattern.to_path_buf()]
    } else if pattern.is_dir() {
        return Ok(vec![PositiveRoot {
            label: format!("cli/source-{source_index:04}"),
            declaring_dir: pattern.to_path_buf(),
            path: PathBuf::from("."),
            required: true,
            exclusions: BTreeSet::new(),
        }]);
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
    let mut roots = Vec::with_capacity(matches.len());
    for (match_index, path) in matches.into_iter().enumerate() {
        let extension = path.extension().and_then(|part| part.to_str());
        if !extension.is_some_and(|extension| supported.contains(&extension)) {
            bail!(
                "ZW010: explicit --source file has an unsupported extension: {}",
                path.display()
            );
        }
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
        });
    }
    Ok(roots)
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

/// Compute the Tailwind `@source inline("...")` safelist for
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
