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

/// The single Config → [`zfb_content::PipelineSpec`] assembly (zfb#917).
///
/// Every markdown-pipeline knob the bundler AND the snapshot walker see is
/// resolved from the project [`Config`] here — this is the one place the
/// `zfb.config.ts` surface maps onto the shared pipeline knob set, so the
/// two consumers structurally cannot disagree on a knob's value. Both
/// `assemble_bundler_input` (below) and the snapshot path
/// (`build_content_snapshot_json` in `commands/build.rs`) call this.
///
/// The struct literal is intentionally exhaustive (no `..Default`):
/// adding a field to `PipelineSpec` fails compilation HERE until the
/// author decides how the new knob resolves from `Config` — the
/// command-layer half of the drift guard whose pipeline-construction
/// half lives in `PipelineSpec::build_pipeline`.
///
/// `resolve_source_map` is the one knob NOT resolved here (left `None`):
/// it is derivation-owned per surface. The snapshot path fills it via
/// `build_resolve_source_map_for_snapshot`; the bundler derives it inside
/// `zfb_build::bundler::bundle` from `BundlerInput::resolve_markdown_links`
/// (which needs the bundler's path resolver and carries the
/// `on_broken_links` build policy). Both derivations share
/// `resolve_links_routes_from_config` + `build_docs_source_map`, so the
/// resulting `path → URL` maps are identical — required for snapshot ↔
/// bundler `content_hash` parity (zfb#188).
pub(crate) fn pipeline_spec_from_config(
    project_root: &Path,
    config: &Config,
) -> zfb_content::PipelineSpec {
    zfb_content::PipelineSpec {
        // `codeHighlight.theme` — named syntect theme for fenced code
        // blocks instead of the default `base16-ocean.dark`.
        // Mutually exclusive with the dual pair below; validation in
        // config.rs guarantees at most one mode is set.
        code_highlight_theme: config.code_highlight.as_ref().and_then(|c| c.theme.clone()),
        // `codeHighlight.themesDir` — resolved to an absolute path HERE so
        // both surfaces load the same `.tmTheme` files. Applies to both
        // single-theme and dual-theme mode.
        code_highlight_themes_dir: config
            .code_highlight
            .as_ref()
            .and_then(|c| c.themes_dir.as_ref())
            .map(|td| project_root.join(td)),
        // `codeHighlight.themeLight` / `codeHighlight.themeDark` — the
        // dual-theme pair that emits CSS custom properties (`--shiki-light`
        // / `--shiki-dark`) instead of inline `color:`. Active iff BOTH
        // are Some. Mutually exclusive with `code_highlight_theme`.
        code_highlight_theme_light: config
            .code_highlight
            .as_ref()
            .and_then(|c| c.theme_light.clone()),
        code_highlight_theme_dark: config
            .code_highlight
            .as_ref()
            .and_then(|c| c.theme_dark.clone()),
        // `codeHighlight.mode` (Highlight Tokens epic, zfb#1528) — absent
        // `codeHighlight` mirrors the field's own default (inline).
        code_highlight_mode: match config.code_highlight.as_ref().map(|c| c.mode) {
            Some(crate::config::CodeHighlightMode::Class) => zfb_content::CodeHighlightMode::Class,
            Some(crate::config::CodeHighlightMode::Inline) | None => {
                zfb_content::CodeHighlightMode::Inline
            }
        },
        // `codeHighlight.classPrefix` — only meaningful in class mode.
        // Absent `codeHighlight` falls back to the same default
        // `CodeHighlightConfig::class_prefix` resolves to via serde.
        code_highlight_class_prefix: config
            .code_highlight
            .as_ref()
            .map(|c| c.class_prefix.clone())
            .unwrap_or_else(crate::config::default_class_prefix),
        // `codeHighlight.roleClasses` — role -> class attr value overrides;
        // only meaningful in class mode. Absent -> empty map (every role
        // uses `{classPrefix}{role}`).
        code_highlight_role_classes: config
            .code_highlight
            .as_ref()
            .and_then(|c| c.role_classes.clone())
            .unwrap_or_default(),
        // Opt-in `stripMdExt` flag (zfb#127 / #129).
        strip_md_ext: config.strip_md_ext,
        // Derivation-owned — see the doc comment above.
        resolve_source_map: None,
        gfm_constructs: crate::config::resolve_gfm_constructs(config.markdown.as_ref()),
        toc: config.markdown.as_ref().and_then(|m| m.toc.clone()),
        // `markdown.externalLinks`; the `site` origin (top-level
        // `config.site`, #254) lets `ExternalLinksPlugin` classify
        // same-origin absolute URLs as internal.
        external_links: config
            .markdown
            .as_ref()
            .and_then(|m| m.external_links.clone())
            .map(|el| (el.into_content_config(), config.site.clone())),
        cjk_friendly: crate::config::resolve_cjk_friendly(config.markdown.as_ref()),
        hard_breaks: crate::config::resolve_hard_breaks(config.markdown.as_ref()),
        // `markdown.features` (#586) — opt-in feature plugins (mermaid, …).
        features: config.markdown.as_ref().and_then(|m| m.features.clone()),
        // Arm build-context roots iff a filesystem-dependent feature is enabled
        // (transclude / imageDimensions / linkValidation).  Gating rationale:
        // (a) unarmed projects keep byte-identical fingerprints; (b) roots and
        // the #942 ReadRecorder then always co-occur — arming without a recorder
        // stores an empty DependencyManifest and stale-cache hazards follow
        // (zfb#952).
        build_context_roots: {
            let has_fs_feature = config
                .markdown
                .as_ref()
                .and_then(|m| m.features.as_ref())
                .map(|f| {
                    f.transclude.is_some()
                        || f.image_dimensions.is_some()
                        || f.link_validation.is_some()
                })
                .unwrap_or(false);
            has_fs_feature.then(|| {
                let public_dir =
                    crate::commands::resolve::resolve_under_root(project_root, &config.public_dir);
                (project_root.to_path_buf(), public_dir)
            })
        },
    }
}

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
///
/// `pre_resolved_esbuild` (#994 item A) — an already-extracted embedded
/// esbuild binary whose backing directory the CALLER keeps alive for at
/// least as long as the returned [`AssembledBundlerInput`] is in use.
/// `zfb dev` extracts once at boot and passes it on every tick so the
/// per-call tempdir extraction below is skipped; `zfb build` passes
/// `None` and keeps the per-call extraction (one extraction per build is
/// already process-lifetime there). Ignored when an explicit
/// `esbuild_binary` or `ZFB_ESBUILD_BIN` override is in play — the
/// existing precedence is preserved.
#[allow(clippy::too_many_arguments)] // 10 params: #994 added pre_resolved_esbuild, #1193 added build_pages_root, #1230 added injected_pages_root; a struct would obscure the caller-keeps-alive contract documented above
pub(crate) fn assemble_bundler_input(
    project_root: &Path,
    config: &Config,
    bundle_mode: BundleMode,
    css_fail_mode: CssModuleFailMode,
    content_snapshot_json: Option<String>,
    plugin_alias_entries: Vec<(String, String)>,
    plugin_virtual_modules: Vec<(String, String)>,
    pre_resolved_esbuild: Option<&Path>,
    // #1193 — the primary pages root the bundler should walk. `Some(root)`
    // points the bundle at that root (the package-owned-routes overlay when
    // build routes are present, or `project_root/pages` when not; passing the
    // absolute `project_root/pages` is byte-identical to the default relative
    // `"pages"`). `None` keeps the default: conventional `zfb dev` passes
    // `None` because package-owned BUILD routes are a build-time concern. This
    // is a REPLACEMENT seam (it overrides `pages_dir`); the build overlay it
    // points at already contains a copy of the user pages. #1518 true
    // zero-pages dev intentionally supplies a private empty primary root here
    // alongside its additive injected root below.
    build_pages_root: Option<&Path>,
    // S2 (#1230) — an ADDITIVE second pages root for the dev server's
    // package-owned **injected** routes (B1 multi-root). Unlike
    // `build_pages_root`, this does NOT override `pages_dir`: the bundler
    // walks the primary root (real user pages, or #1518's private empty root)
    // AND this root into the same shadow tree, so conventional user pages stay
    // in the bundle (HMR intact) while the injected
    // entrypoints + their `virtual:` imports are added. It holds ONLY the
    // synthesized injected modules — no user-page copy (the conventional dev
    // scan + watcher keep the real `pages/`). `None` (build, and dev with no
    // injected routes) is byte-identical to today. #1518 intentionally passes
    // both roots: `build_pages_root` is the private empty primary root and
    // this remains the additive injected-only root. Other paths preserve the
    // existing single-root behavior.
    injected_pages_root: Option<&Path>,
) -> Result<AssembledBundlerInput> {
    let mut bundler_input = BundlerInput::for_project(
        project_root.to_path_buf(),
        crate::render_pipeline::cfg_framework_to_render(config.framework),
        bundle_mode,
        project_root.join(".zfb-build"),
        content_snapshot_json,
    );

    // #1193 — override the default `pages_dir` ("pages", joined against
    // project_root by the bundler's resolver) with the explicit build
    // pages root so the bundle's per-page imports include package-owned
    // routes. The resolver passes absolute paths through unchanged, so an
    // overlay temp dir works and `project_root/pages` stays byte-identical.
    if let Some(root) = build_pages_root {
        bundler_input.pages_dir = root.to_path_buf();
    }

    // S2 (#1230) — additive injected-route root for `zfb dev`. The bundler
    // walks this root into the SAME shadow `pages/` tree as the primary
    // `pages_dir` (real user pages, or #1518's private empty root), so the dev
    // bundle contains conventional user pages when present plus the synthesized
    // injected modules (B1 multi-root). `None` for `zfb build` and for dev
    // with no injected routes — byte-identical to today.
    bundler_input.injected_pages_root = injected_pages_root.map(|p| p.to_path_buf());

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

    bundler_input.tsconfig_paths = crate::commands::build::read_tsconfig_paths(project_root);

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
        match crate::commands::build::compute_css_module_class_maps(
            project_root,
            &plugin_alias_entries,
        ) {
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

    // The shared markdown-pipeline knob set (zfb#917) — the SAME
    // `PipelineSpec` shape the snapshot path builds via this helper, so
    // the bundler's MDX pipelines and the snapshot walker's pipelines
    // agree on every knob and produce byte-identical `content_hash`
    // values. (`resolve_source_map` stays `None` here; `bundle()` derives
    // it from `resolve_markdown_links` below.)
    bundler_input.pipeline_spec = pipeline_spec_from_config(project_root, config);

    // Thread the opt-in `resolveMarkdownLinks` config into the bundler so
    // the hoisted MDX pre-compile pipeline appends `ResolveLinksPlugin`.
    // Without this wiring the bundler's MDX pipeline only ran
    // `StripMdExtensionPlugin`, and author-written relative `.mdx` links were
    // emitted as relative href values that broke at the file→directory
    // transformation in dist HTML (sub #234 / zudolab/zudo-doc#1577). The
    // shared helper `resolve_links_routes_from_config` builds the same
    // per-route map the snapshot path uses so content_hash stays
    // deterministic. This stays a separate bundler-side input (not a
    // `PipelineSpec` knob) — see the shape-decision note on
    // `BundlerInput::resolve_markdown_links`.
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
        bundler_input.resolve_markdown_links = Some(zfb_build::bundler::ResolveMarkdownLinksSpec {
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

    // #978 — thread `base_prefix` so the bundler emits
    // `globalThis.__zfb.base = "<prefix>"` in `entry.mjs` when the project
    // has at least one `*.client.*` entry. The emission is gated on
    // discovery so zero-script builds remain byte-for-byte identical.
    //
    // Production vs dev differ in how they normalise an absolute-URL `base`
    // (e.g. `https://cdn.example.com/`):
    //   - prod: `asset_url_base_prefix` keeps the full origin prefix
    //     (`"https://cdn.example.com"`) because the rewrite keys embed
    //     the full URL.
    //   - dev: `dev_mount_prefix` collapses absolute-URL bases to `None`
    //     (the dev server cannot serve a CDN origin), which we map to `""`
    //     so `clientScript(name)` still returns the bare stable URL.
    //
    // Collisions (duplicate entry names across roots) are surfaced later
    // by `build_default_client_scripts_payloads`; we silently ignore them
    // here — discovery is just a presence check.
    {
        use zfb_islands::discover_client_scripts;
        let has_client_scripts = discover_client_scripts(project_root)
            .map(|(entries, _)| !entries.is_empty())
            .unwrap_or(false);
        if has_client_scripts {
            bundler_input.base_prefix = Some(match bundle_mode {
                BundleMode::Production => {
                    crate::config::asset_url_base_prefix(config.base.as_deref())
                }
                BundleMode::Development => {
                    zfb_types::dev_mount_prefix(config.base.as_deref()).unwrap_or_default()
                }
            });
        }
    }

    // #664 / #672 — thread `bundle.exclude` so the bundler keeps the listed
    // project-relative globs out of the esbuild graph (both the shadow-tree
    // copy and the #665 import.meta.glob expansion).  Empty → skip nothing.
    bundler_input.bundle_exclude = crate::config::resolve_bundle_exclude(config.bundle.as_ref());

    // #676 — thread `bundle.mainFields` / `bundle.external` so hosts can make
    // the `--platform=neutral` page/SSR pass resolve (or externalize)
    // CJS-main-only deps (e.g. `msw` -> `path-to-regexp@6`).  main_fields
    // applies to every framework when set; external is APPENDED so any
    // framework-required externals are preserved.  Empty → byte-identical.
    bundler_input.main_fields = crate::config::resolve_bundle_main_fields(config.bundle.as_ref());
    bundler_input
        .external
        .extend(crate::config::resolve_bundle_external(
            config.bundle.as_ref(),
        ));

    // #1498 — append validated inline loaders after the SSR bundler's
    // reserved loader flags. `BTreeMap` iteration keeps argv deterministic.
    bundler_input.extra_loader_args = crate::config::resolve_bundle_loaders(config.bundle.as_ref())
        .into_iter()
        .map(|(extension, loader)| format!("--loader:{extension}={loader}"))
        .collect();

    // #1498 — operator-authored raw esbuild expressions. This is distinct
    // from `public_env_vars`: values are not JSON-encoded and keys are not
    // filtered by `PUBLIC_`. Config validation reserves the mode-owned keys.
    bundler_input.define_vars = crate::config::resolve_bundle_define(config.bundle.as_ref());

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
        if let Some(path) = pre_resolved_esbuild {
            // #994 item A — the caller already extracted the embedded
            // esbuild binary and keeps its tempdir alive beyond this
            // input's lifetime (process-lifetime in `zfb dev`), so the
            // per-call extraction is skipped. No handle is stored here:
            // the lifetime contract on `AssembledBundlerInput` is
            // satisfied by the caller's longer-lived handle.
            bundler_input.esbuild_binary = Some(path.to_path_buf());
            _esbuild_handle = None;
        } else {
            match crate::render_pipeline::embedded_binary("esbuild") {
                Ok((handle, path)) => {
                    bundler_input.esbuild_binary = Some(path);
                    _esbuild_handle = Some(handle);
                }
                Err(e) => {
                    // Non-fatal: log and let the bundler's own resolver fall
                    // through to the on-disk slot, which still produces a
                    // useful error message pointing at
                    // `crates/zfb/binaries/esbuild/`.
                    crate::output::warn(format!(
                        "could not extract embedded esbuild ({e}); \
                         falling back to bundler resolver"
                    ));
                    _esbuild_handle = None;
                }
            }
        }
    } else {
        _esbuild_handle = None;
    }

    Ok(AssembledBundlerInput {
        bundler_input,
        _node_modules_handle,
        _esbuild_handle,
    })
}
