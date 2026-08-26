//! CSS helpers shared by command-layer entrypoints.
//!
//! This module owns the command-side setup that is common to `zfb build` and
//! the standalone `zfb css` entrypoint.  It deliberately returns
//! [`zfb_css::CssEmitterOutput`] rather than the build pipeline's
//! [`zfb_build::pipeline::AssetEmitterPayload`], keeping the CSS command
//! independent of production HTML/asset rewriting.

#![cfg_attr(not(feature = "embed_v8"), allow(unused_imports, dead_code))]

use std::path::{Path, PathBuf};

use anyhow::Result;
use zfb_css::{
    CssEmitterOutput, CssEngine, CssPipeline, CssPipelineConfig, TailwindSubprocessConfig,
};

use crate::config::{CodeHighlightMode, Config};
use crate::render_pipeline::embedded_binary;

/// Return the default Tailwind content roots rebased to `project_root`.
///
/// The CSS engine stores content globs as strings and resolves them from the
/// synthesised entry CSS.  Supplying absolute paths here keeps command output
/// independent of the directory from which the caller invokes zfb.
pub(crate) fn default_content_globs(project_root: &Path) -> Vec<String> {
    zfb_css::engine::DEFAULT_CONTENT_ROOTS
        .iter()
        .map(|root| project_root.join(root).to_string_lossy().into_owned())
        .collect()
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

/// Install the embedded Tailwind binary when no `ZFB_TAILWIND_BIN` override
/// is present.
///
/// The environment override remains the first precedence tier.  If it is not
/// set, the embedded snapshot is extracted and its temporary-directory handle
/// is retained by [`TailwindSubprocessConfig`] for the engine's lifetime.  If
/// extraction is unavailable, the original config (and its workspace-relative
/// fallback) is preserved exactly as before.
pub(crate) fn with_embedded_tailwind_binary(
    config: TailwindSubprocessConfig,
) -> TailwindSubprocessConfig {
    if std::env::var_os("ZFB_TAILWIND_BIN").is_none() {
        if let Ok((handle, path)) = embedded_binary("tailwindcss-v4") {
            return config.with_embedded_binary(handle, path);
        }
    }
    config
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
