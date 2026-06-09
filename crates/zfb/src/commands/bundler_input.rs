//! Shared `Config` → [`BundlerInput`] assembly for `zfb build` and `zfb dev`.
//!
//! Both commands build a [`BundlerInput`] from roughly the same ~25 fields
//! drawn from the project [`Config`].  The only genuine per-command
//! differences are documented as parameters or variants here; everything else
//! is byte-identical between the two paths.
//!
//! ## Explicit dev/build differences
//!
//! | Field / aspect | `zfb build` | `zfb dev` |
//! |---|---|---|
//! | `bundle_mode` | [`BundleMode::Production`] | [`BundleMode::Development`] |
//! | CSS Modules failure | hard error (propagated via `?`) | soft warn + empty map |
//!
//! The `bundle_mode` parameter is supplied by the caller.  The CSS-Modules
//! failure policy is selected via [`CssModuleFailMode`].

use std::path::Path;

use anyhow::{Context, Result};
use zfb_build::bundler::{BundleMode, BundlerInput};

use crate::config::Config;

/// How to handle a CSS-Modules class-map computation failure.
///
/// `zfb build` propagates the error (the user must fix the CSS before the
/// build proceeds).  `zfb dev` degrades gracefully — dev mode warns and
/// continues with an empty map so the dev server still boots even when the
/// project's CSS is temporarily broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CssModuleFailMode {
    /// Return the error immediately (`?`). Used by `zfb build`.
    HardFail,
    /// Log a warning and return an empty map. Used by `zfb dev`.
    WarnAndEmpty,
}

/// Assembled [`BundlerInput`] plus the two optional temporary-directory
/// handles that must stay alive as long as the bundler is running.
///
/// * `node_modules_handle` — non-`None` when the embedded `@takazudo`
///   packages were extracted into a temp dir (cargo-install scenario with no
///   project-side `node_modules/`).
/// * `esbuild_handle` — non-`None` when the embedded `esbuild` binary was
///   extracted into a temp dir.
///
/// Both handles must be kept in scope until the [`BundlerInput`] has been
/// consumed by [`zfb_build::bundler::bundle`].  Dropping them early
/// deallocates the temp dirs while esbuild is still reading from them.
pub(crate) struct AssembledBundlerInput {
    pub(crate) bundler_input: BundlerInput,
    /// Keeps the extracted node_modules temp dir alive for the bundle step.
    pub(crate) _node_modules_handle: Option<tempfile::TempDir>,
    /// Keeps the extracted esbuild binary temp dir alive for the bundle step.
    pub(crate) _esbuild_handle: Option<tempfile::TempDir>,
}

/// Assemble a fully-configured [`BundlerInput`] from the project [`Config`].
///
/// All ~25 fields are set here from the shared configuration; the two
/// per-command differences are supplied as parameters:
///
/// * `bundle_mode` — [`BundleMode::Production`] for `zfb build`,
///   [`BundleMode::Development`] for `zfb dev`.
/// * `css_fail_mode` — [`CssModuleFailMode::HardFail`] for `zfb build`,
///   [`CssModuleFailMode::WarnAndEmpty`] for `zfb dev`.
///
/// The caller supplies the pre-fetched plugin lists
/// (`plugin_alias_entries`, `plugin_virtual_modules`) from the plugin
/// lifecycle setup step ([`super::plugins::run_plugin_setup`]).
pub(crate) fn assemble_bundler_input(
    project_root: &Path,
    config: &Config,
    bundle_mode: BundleMode,
    css_fail_mode: CssModuleFailMode,
    content_snapshot_json: Option<String>,
    plugin_alias_entries: Vec<(String, String)>,
    plugin_virtual_modules: Vec<(String, String)>,
) -> Result<AssembledBundlerInput> {
    let mut bundler_input = BundlerInput::for_project(
        project_root.to_path_buf(),
        crate::render_pipeline::cfg_framework_to_render(config.framework),
        bundle_mode,
        project_root.join(".zfb-build"),
        content_snapshot_json,
    );

    // Discover the Next-style root `mdx-components.tsx` convention (#616):
    // a project-wide element→component override map applied to every
    // `<Content>`. Gated on the file existing so a project without it gets
    // byte-for-byte identical output. The bundler copies it into the shadow
    // root and emits the `globalThis.__zfb.mdxComponents` installer.
    bundler_input.mdx_components_file =
        crate::commands::build::discover_mdx_components_file(project_root);

    // Inject project-side resolution context so esbuild can find user
    // dependencies + path aliases. Without these the shadow tempdir has no
    // `node_modules` to walk into and no tsconfig `paths` to honour, so
    // anything beyond a self-contained page module fails to resolve.
    //
    // When the project has no node_modules at all (cargo-install scenario),
    // fall back to the binary-embedded @takazudo packages so esbuild can
    // still resolve `@takazudo/zfb` and `@takazudo/zfb-runtime`. The
    // `_node_modules_handle` keeps the tempdir alive for the duration of
    // the bundle step; it is dropped after `bundle(...)` returns.
    let _node_modules_handle: Option<tempfile::TempDir>;
    if let Some(nm) = crate::commands::build::detect_project_node_modules(project_root) {
        bundler_input.node_modules_dir = Some(nm);
        _node_modules_handle = None;
    } else {
        match crate::render_pipeline::embedded_node_modules() {
            Ok((handle, nm_path)) => {
                bundler_input.node_modules_dir = Some(nm_path);
                // Vendored / cargo-install mode: the project has no
                // `node_modules`, so the bundler extracted one into a
                // tempdir.  esbuild must STAY at the shadow path during
                // resolution — see the `--preserve-symlinks` block in
                // `run_esbuild` and `BundlerInput::node_modules_preserve_symlinks`
                // for the full rationale (issues #443 / #450).
                bundler_input.node_modules_preserve_symlinks = true;
                _node_modules_handle = Some(handle);
            }
            Err(e) => {
                // Non-fatal: log a warning and continue without injecting a
                // node_modules_dir.  The build will likely fail later if the
                // project also has no ancestor node_modules, but that failure
                // produces a more useful esbuild error than aborting here.
                crate::output::warn(format!(
                    "could not extract embedded @takazudo packages ({e}); \
                     falling back to node_modules walk"
                ));
                _node_modules_handle = None;
            }
        }
    }

    bundler_input.tsconfig_paths =
        crate::commands::build::read_tsconfig_paths(project_root);

    // Per-collection content materialisation feeds the MDX content bridge
    // (#506) — without this every doc page would render as raw markdown text
    // in a `<pre data-zfb-content-fallback>` block because
    // `globalThis.__zfb.content.get(specifier)` would return `undefined`.
    bundler_input.content_collections = config
        .collections
        .iter()
        .map(|c| zfb_build::ContentCollectionSpec {
            name: c.name.clone(),
            root: c.path.clone(),
            include: c.include.clone(),
            exclude: c.exclude.clone(),
            id_strip_suffix: c.id_strip_suffix.clone(),
        })
        .collect();

    // CSS Modules — compute the scoped class-name maps and hand them to the
    // bundler so `import styles from "./x.module.css"` resolves to the scoped
    // class strings at bundle time.
    //
    // Error handling differs by mode:
    // - HardFail (build): propagate the error; a broken `.module.css` must be
    //   fixed before `zfb build` proceeds.
    // - WarnAndEmpty (dev): log a warning and continue with an empty map so
    //   the dev server still boots even when CSS is temporarily broken.
    bundler_input.css_module_class_maps =
        match crate::commands::build::compute_css_module_class_maps(project_root) {
            Ok(maps) => maps,
            Err(e) => match css_fail_mode {
                CssModuleFailMode::HardFail => {
                    return Err(e).context("CSS Modules class-map computation failed");
                }
                CssModuleFailMode::WarnAndEmpty => {
                    crate::output::warn(format!(
                        "CSS Modules class-map computation failed ({e}); \
                         `.module.css` imports will resolve to empty maps in dev"
                    ));
                    std::collections::HashMap::new()
                }
            },
        };

    // Thread the opt-in `stripMdExt` flag from `zfb.config.ts` into the
    // bundler so the hoisted MDX pre-compile pipeline appends
    // `StripMdExtensionPlugin`. Mirrored in both commands so dev and build
    // produce the same href shape (zfb#127 / #129).
    bundler_input.strip_md_ext = config.strip_md_ext;

    // Thread the opt-in `resolveMarkdownLinks` config into the bundler so
    // the hoisted MDX pre-compile pipeline appends `ResolveLinksPlugin`.
    // Without this wiring the bundler's MDX pipeline only ran
    // `StripMdExtensionPlugin`, and author-written relative `.mdx` links were
    // emitted as relative href values that broke at the file→directory
    // transformation in dist HTML (sub #234 / zudolab/zudo-doc#1577). The
    // shared helper `resolve_links_routes_from_config` builds the same
    // per-route map the snapshot path uses so content_hash stays
    // deterministic.
    if let Some(routes) =
        crate::commands::build::resolve_links_routes_from_config(project_root, config)
    {
        let on_broken_links = match config
            .resolve_markdown_links
            .as_ref()
            .map(|r| r.on_broken_links)
            .unwrap_or_default()
        {
            crate::config::OnBrokenLinks::Warn => zfb_build::bundler::OnBrokenLinks::Warn,
            crate::config::OnBrokenLinks::Error => zfb_build::bundler::OnBrokenLinks::Error,
            crate::config::OnBrokenLinks::Ignore => zfb_build::bundler::OnBrokenLinks::Ignore,
        };
        bundler_input.resolve_markdown_links =
            Some(zfb_build::bundler::ResolveMarkdownLinksSpec {
                routes: routes
                    .into_iter()
                    .map(|r| zfb_build::bundler::ResolveMarkdownLinksRoute {
                        docs_dir: r.dir,
                        route_prefix: r.route_prefix,
                    })
                    .collect(),
                on_broken_links,
            });
    }

    // Thread the optional `codeHighlight.theme` from `zfb.config.ts`
    // so the hoisted MDX pre-compile pipeline uses the configured
    // syntect theme instead of the default `base16-ocean.dark`.
    bundler_input.code_highlight_theme =
        config.code_highlight.as_ref().and_then(|c| c.theme.clone());

    // Thread the optional `codeHighlight.themesDir` (resolved to an
    // absolute path here) so the bundler loads custom .tmTheme files
    // before constructing the SyntectPlugin.  MUST stay in sync with
    // the snapshot wiring so both content_hash inputs agree.
    bundler_input.code_highlight_themes_dir = config
        .code_highlight
        .as_ref()
        .and_then(|c| c.themes_dir.as_ref())
        .map(|td| project_root.join(td));

    // Thread the optional `markdown.gfm` constructs config into the bundler
    // so the hoisted MDX pre-compile pipeline parses the same GFM constructs
    // the snapshot walker uses.  The snapshot wiring resolves from the same
    // source, so both `content_hash` inputs stay byte-identical (the
    // snapshot ↔ bundler land mine called out at
    // `crates/zfb-content/src/content_bridge.rs:118-153`).
    bundler_input.gfm_constructs =
        crate::config::resolve_gfm_constructs(config.markdown.as_ref());

    // Thread the optional `site` canonical-origin URL from `zfb.config.ts`
    // so the bundler emits `globalThis.__zfb.site` in `entry.mjs` for
    // layout-side canonical tag, OG URL, and sitemap construction (sub #254).
    bundler_input.site = config.site.clone();

    // Thread `prefetch.disabled` so the bundler emits
    // `globalThis.__zfb.prefetchDisabled = true` in `entry.mjs` when the
    // user sets `prefetch: { disabled: true }` in `zfb.config.ts` (sub #277).
    bundler_input.prefetch_disabled = config
        .prefetch
        .as_ref()
        .and_then(|p| p.disabled)
        .unwrap_or(false);

    bundler_input.toc = config.markdown.as_ref().and_then(|m| m.toc.clone());

    // Thread `markdown.externalLinks` into the bundler so the hoisted MDX
    // pre-compile pipeline appends `ExternalLinksPlugin`. MUST mirror the
    // snapshot wiring; divergence shifts `content_hash` and breaks the
    // snapshot ↔ bundler bridge lookup.
    // `site` (top-level config.site, #254) lets `ExternalLinksPlugin`
    // classify same-origin absolute URLs as internal.
    bundler_input.external_links = config
        .markdown
        .as_ref()
        .and_then(|m| m.external_links.clone())
        .map(|el| (el.into_content_config(), config.site.clone()));

    bundler_input.cjk_friendly =
        crate::config::resolve_cjk_friendly(config.markdown.as_ref());

    bundler_input.hard_breaks =
        crate::config::resolve_hard_breaks(config.markdown.as_ref());

    // #664 / #672 — thread `bundle.exclude` so the bundler keeps the listed
    // project-relative globs out of the esbuild graph (both the shadow-tree
    // copy and the #665 import.meta.glob expansion).  Empty → skip nothing.
    bundler_input.bundle_exclude =
        crate::config::resolve_bundle_exclude(config.bundle.as_ref());

    // #676 — thread `bundle.mainFields` / `bundle.external` so hosts can make
    // the `--platform=neutral` page/SSR pass resolve (or externalize)
    // CJS-main-only deps (e.g. `msw` -> `path-to-regexp@6`).  main_fields
    // applies to every framework when set; external is APPENDED so any
    // framework-required externals are preserved.  Empty → byte-identical.
    bundler_input.main_fields =
        crate::config::resolve_bundle_main_fields(config.bundle.as_ref());
    bundler_input
        .external
        .extend(crate::config::resolve_bundle_external(config.bundle.as_ref()));

    // #586 — thread `markdown.features` into the bundler so opt-in feature
    // plugins (mermaid, …) fire per the configured toggles.
    // `None` keeps the legacy always-on chain, byte-identical to today.
    bundler_input.markdown_features =
        config.markdown.as_ref().and_then(|m| m.features.clone());

    // #268 — thread plugin-registered aliases and virtual modules into the
    // main bundler's esbuild invocation so page / layout / shared SSR-only
    // modules can consume them.  Both build and dev derive these from the same
    // setup_registries so both paths produce identical alias resolution.
    bundler_input.plugin_alias_entries = plugin_alias_entries;
    bundler_input.plugin_virtual_modules = plugin_virtual_modules;

    // Sub #212 follow-up — pre-extract the embedded esbuild binary and pin
    // its path on the input so consumer projects without the
    // `crates/zfb/binaries/esbuild/` slot don't blow up.
    // Skip when an explicit override is in play (input field or env var).
    let _esbuild_handle: Option<tempfile::TempDir>;
    if bundler_input.esbuild_binary.is_none() && std::env::var_os("ZFB_ESBUILD_BIN").is_none() {
        match crate::render_pipeline::embedded_binary("esbuild") {
            Ok((handle, path)) => {
                bundler_input.esbuild_binary = Some(path);
                _esbuild_handle = Some(handle);
            }
            Err(e) => {
                // Non-fatal: log and let the bundler's own resolver fall
                // through to the on-disk slot, which still produces a useful
                // error message pointing at `crates/zfb/binaries/esbuild/`.
                crate::output::warn(format!(
                    "could not extract embedded esbuild ({e}); \
                     falling back to bundler resolver"
                ));
                _esbuild_handle = None;
            }
        }
    } else {
        _esbuild_handle = None;
    }

    Ok(AssembledBundlerInput {
        bundler_input,
        _node_modules_handle: _node_modules_handle,
        _esbuild_handle: _esbuild_handle,
    })
}

