//! Single source of truth for the markdown-pipeline knob set (zfb#917).
//!
//! Before this module existed, the same ~10 pipeline knobs were
//! hand-mirrored in four places (the snapshot walker's config struct, the
//! bundler's input struct, and two near-duplicate `build_pipeline`
//! implementations), with the snapshot ↔ bundler `content_hash` parity
//! guarantee resting on comments telling authors to keep them in sync.
//! [`PipelineSpec`] is now the one knob list, and
//! [`PipelineSpec::build_pipeline`] the one construction path — both the
//! snapshot walker (`build_snapshot_with_config`) and the bundler's
//! materialise walks build their [`Pipeline`]s here, so the two surfaces
//! structurally cannot diverge.
//!
//! ## Why parity matters
//!
//! The snapshot walker bakes a JSX `content_hash` into every
//! `EntrySnapshot::module_specifier`; the bundler bakes the same hash into
//! its bridge-map keys. Any pipeline-shape divergence between the two
//! shifts one side's hash, every
//! `globalThis.__zfb.content.get(specifier)` lookup misses, and the page
//! silently renders the raw-markdown `<pre data-zfb-content-fallback>`
//! block (zfb#187 / #188).
//!
//! ## Drift guard
//!
//! [`PipelineSpec::build_pipeline`] exhaustively destructures `Self` with
//! NO `..` rest pattern, and the Config→spec assembly
//! (`pipeline_spec_from_config` in `crates/zfb/src/commands/bundler_input.rs`)
//! constructs the struct with an exhaustive literal. Adding a field to
//! `PipelineSpec` therefore fails compilation at both sites until the
//! author wires the new knob into the pipeline construction AND decides
//! how it resolves from `zfb.config.ts` — the same property the
//! fingerprint drift guards in `pipeline.rs` enforce for
//! `MarkdownFeaturesConfig` / GFM fields.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use crate::pipeline::{Pipeline, ResolvedGfmConstructs};

/// Resolved `codeHighlight.mode` — mirrors the `"inline"` / `"class"`
/// string-literal union in `zfb.config.ts` (`CodeHighlightConfig::mode` in
/// `crates/zfb/src/config.rs`). `Default` is [`CodeHighlightMode::Inline`]
/// — the pre-epic behaviour; class-emission is opt-in (Highlight Tokens
/// epic, zfb#1528).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodeHighlightMode {
    /// Per-token inline colors (`style="color:#rrggbb"` or the dual
    /// `--shiki-*` custom properties). Pre-epic behaviour.
    #[default]
    Inline,
    /// Per-token semantic role classes (zfb#1532): every token becomes a
    /// `<span class="…">` carrying a role class from the #1529 taxonomy
    /// (default `{classPrefix}{short_name}`, or the `roleClasses` override),
    /// with NO inline colour; the `<pre>` is classed `{classPrefix}root`.
    /// Routed via [`crate::pipeline::Pipeline::with_defaults_and_full_config_class`],
    /// which also encodes `classPrefix`/`roleClasses` into the compile-cache
    /// fingerprint so class-mode configs never alias an inline/dual entry.
    Class,
}

/// Error from [`PipelineSpec::build_pipeline`].
///
/// The only fallible step is loading custom `.tmTheme` files from
/// [`PipelineSpec::code_highlight_themes_dir`]; every other knob is
/// infallible wiring.
#[derive(Debug, thiserror::Error)]
#[error("codeHighlight.themesDir: failed to load themes from {themes_dir}: {cause}")]
pub struct PipelineSpecError {
    /// Display form of the `code_highlight_themes_dir` that failed to load.
    pub themes_dir: String,
    /// Underlying theme-load failure from syntect.
    pub cause: crate::syntect_highlight::HighlightError,
}

/// The pipeline-visible markdown knob set — the ONE place the
/// snapshot ↔ bundler pipeline shape is defined (zfb#917).
///
/// Carried by `zfb_build::bundler::BundlerInput` and accepted by
/// `build_snapshot_with_config`, with the Config→spec mapping owned by
/// `pipeline_spec_from_config` in `crates/zfb/src/commands/bundler_input.rs`.
///
/// `Default` reproduces the pre-config-API behaviour (no theme override,
/// no strip-md-ext, no resolve-links, conservative GFM, CJK on, hard
/// breaks off, no TOC, no external-links plugin, empty feature set) so
/// call sites and tests that don't need any of the optional knobs keep
/// the same effective pipeline shape.
#[derive(Debug, Clone)]
pub struct PipelineSpec {
    /// Optional syntect highlight theme name (e.g. `"InspiredGitHub"`).
    /// `None` keeps the built-in default theme (`base16-ocean.dark`).
    /// Custom themes are loaded via [`Self::code_highlight_themes_dir`].
    /// Mirrors `codeHighlight.theme` in `zfb.config.ts`.
    ///
    /// Mutually exclusive with [`Self::code_highlight_theme_light`] /
    /// [`Self::code_highlight_theme_dark`] — set only one mode per build.
    /// These are SYNTECT theme names (e.g. `"InspiredGitHub"`,
    /// `"Solarized (dark)"`), NOT Shiki names like `"dracula"`.
    pub code_highlight_theme: Option<String>,
    /// Optional absolute path to a directory of `.tmTheme` files, loaded
    /// before the syntect plugin is constructed so custom themes become
    /// addressable by name in [`Self::code_highlight_theme`] (or the dual
    /// pair). `None` keeps syntect's bundled themes only. Mirrors
    /// `codeHighlight.themesDir` in `zfb.config.ts` (resolved to an
    /// absolute path by the command layer before being stored here).
    ///
    /// Applies to both single-theme and dual-theme mode.
    pub code_highlight_themes_dir: Option<PathBuf>,
    /// Light-mode syntect theme name for dual-theme highlighting.
    ///
    /// Active iff BOTH `code_highlight_theme_light` AND
    /// `code_highlight_theme_dark` are `Some`. When the dual pair is
    /// active, every fenced code block is highlighted twice and per-token
    /// colors are emitted as CSS custom properties (`--shiki-light` /
    /// `--shiki-dark`) instead of inline `color:`. The `<pre>` element
    /// carries `class="syntect-dual"` and `--shiki-*-bg` vars in its
    /// `style` attribute.
    ///
    /// Mutually exclusive with [`Self::code_highlight_theme`].
    /// Must be a SYNTECT theme name (e.g. `"base16-ocean.light"`),
    /// NOT a Shiki name like `"dracula"`.
    /// Mirrors `codeHighlight.themeLight` in `zfb.config.ts`.
    pub code_highlight_theme_light: Option<String>,
    /// Dark-mode syntect theme name for dual-theme highlighting.
    ///
    /// See [`Self::code_highlight_theme_light`] for the full dual-mode
    /// contract. Must be a SYNTECT theme name (e.g. `"base16-ocean.dark"`),
    /// NOT a Shiki name. Mirrors `codeHighlight.themeDark` in `zfb.config.ts`.
    pub code_highlight_theme_dark: Option<String>,
    /// Output mode for fenced-code highlighting. Mirrors
    /// `codeHighlight.mode` in `zfb.config.ts`. `Default` is
    /// [`CodeHighlightMode::Inline`]. See [`CodeHighlightMode`] for the
    /// Wave-1 class-mode stub note.
    pub code_highlight_mode: CodeHighlightMode,
    /// Class-name prefix for class-mode role classes (e.g. `"hi-"`).
    /// Mirrors `codeHighlight.classPrefix` in `zfb.config.ts`. Only
    /// consumed when [`Self::code_highlight_mode`] is
    /// [`CodeHighlightMode::Class`]; otherwise inert.
    pub code_highlight_class_prefix: String,
    /// Per-role class overrides for class mode (role name -> class attr
    /// value). Mirrors `codeHighlight.roleClasses` in `zfb.config.ts`. A
    /// `BTreeMap` so its `Debug` formatting (used by the content
    /// fingerprint) is deterministically ordered. Only consumed when
    /// [`Self::code_highlight_mode`] is [`CodeHighlightMode::Class`];
    /// otherwise inert.
    pub code_highlight_role_classes: BTreeMap<String, String>,
    /// When `true`, append [`Pipeline::add_strip_md_ext`] so internal
    /// `[link](other.md)` style hrefs are rewritten to `other/`. Mirrors
    /// the opt-in `stripMdExt` flag in `zfb.config.ts` (zfb#127 / #129).
    pub strip_md_ext: bool,
    /// When `Some`, wire [`Pipeline::add_resolve_links`] with this
    /// `(absolute source path → URL)` map so author-written
    /// `[link](./other.mdx)` references resolve to rendered route URLs.
    /// `None` skips the plugin entirely. The per-file `source_dir` is
    /// still set by the walk loops via
    /// [`Pipeline::set_resolve_links_source_dir`].
    ///
    /// **Shape decision (zfb#917):** `PipelineSpec` stores the
    /// already-BUILT source map — the pipeline-visible knob — not the
    /// route config it derives from. The bundler keeps its
    /// `ResolveMarkdownLinksSpec` (route dirs + `onBrokenLinks` policy)
    /// separately on `BundlerInput` because relative `docs_dir` entries
    /// need the bundler's `PathResolver` and the broken-link policy is a
    /// build policy, not a pipeline-shape knob; `bundle()` derives this
    /// knob from that route spec for its own walks, overwriting whatever
    /// the caller set here. The snapshot path
    /// (`build_content_snapshot_json` in `crates/zfb/src/commands/build.rs`)
    /// fills it eagerly. Both derivations share
    /// `resolve_links_routes_from_config` + `build_docs_source_map`, so
    /// the two surfaces resolve identical URL strings (zfb#188).
    pub resolve_source_map: Option<HashMap<PathBuf, String>>,
    /// Resolved per-construct GFM flags (output of
    /// `zfb::config::resolve_gfm_constructs` /
    /// `MarkdownConfig::resolve_constructs`). `Default` is
    /// [`ResolvedGfmConstructs::CONSERVATIVE`].
    pub gfm_constructs: ResolvedGfmConstructs,
    /// When `Some`, wire `TocPlugin` into the hast phase immediately
    /// after `HeadingLinksPlugin` (so it reads final deduplicated `id`
    /// attributes). Mirrors `markdown.toc` in `zfb.config.ts`. `None`
    /// (the default) leaves the chain byte-for-byte identical to the
    /// pre-TOC build.
    pub toc: Option<crate::plugins::toc::TocConfig>,
    /// When `Some`, append [`Pipeline::add_external_links`] with the
    /// given config and optional site origin so external `<a>` elements
    /// are annotated with `target` / `rel`. Mirrors
    /// `markdown.externalLinks` in `zfb.config.ts`; the site origin
    /// (top-level `config.site`, #254) lets the plugin classify
    /// same-origin absolute URLs as internal. `None` (the default)
    /// skips the plugin entirely.
    pub external_links: Option<(crate::plugins::ExternalLinksConfig, Option<String>)>,
    /// Whether to include `CjkFriendlyPlugin` in the mdast phase.
    /// Mirrors `zfb::config::resolve_cjk_friendly(config.markdown)`.
    /// `Default` is `true` (plugin on).
    pub cjk_friendly: bool,
    /// Whether to include `HardBreaksPlugin` in the mdast phase.
    /// Mirrors `zfb::config::resolve_hard_breaks(config.markdown)`.
    /// `Default` is `false` (plugin off).
    pub hard_breaks: bool,
    /// `markdown.features` from `zfb.config.ts`, threaded into the
    /// feature-aware [`Pipeline::with_defaults_and_full_config`]
    /// constructor. `Default` is `None` — treated as an empty feature
    /// set: the former-Core framework features (mermaid, directives,
    /// heading-marker TOC) are OFF (the post-epic opt-in default,
    /// #583 / #586).
    pub features: Option<zfb_md_extras::MarkdownFeaturesConfig>,
    /// Resolved `(project_root, public_dir)` pair handed to
    /// [`Pipeline::set_build_context_roots`] at construction time so the
    /// context-aware feature plugins (transclude, imageDimensions,
    /// linkValidation) receive a concrete filesystem anchor.
    ///
    /// `None` (the default) leaves the pipeline unarmed — the plugins are
    /// inert and no filesystem reads occur.  Set **only** when a
    /// filesystem-dependent feature is enabled in the config so unarmed
    /// projects keep byte-identical fingerprints and shared cache keys, and
    /// so roots and the #942 `ReadRecorder` always co-occur (zfb#952).
    pub build_context_roots: Option<(PathBuf, PathBuf)>,
}

impl Default for PipelineSpec {
    fn default() -> Self {
        Self {
            code_highlight_theme: None,
            code_highlight_themes_dir: None,
            code_highlight_theme_light: None,
            code_highlight_theme_dark: None,
            code_highlight_mode: CodeHighlightMode::Inline,
            code_highlight_class_prefix: "hi-".to_string(),
            code_highlight_role_classes: BTreeMap::new(),
            strip_md_ext: false,
            resolve_source_map: None,
            gfm_constructs: ResolvedGfmConstructs::default(),
            toc: None,
            external_links: None,
            cjk_friendly: true,
            hard_breaks: false,
            features: None,
            build_context_roots: None,
        }
    }
}

impl PipelineSpec {
    /// Construct the [`Pipeline`] this spec describes — the single
    /// construction path shared by the snapshot walker
    /// (`build_snapshot_with_config`) and the bundler's materialise
    /// walks, which is what structurally guarantees byte-identical
    /// MDX `content_hash` values (and compile-cache fingerprints,
    /// zfb#910) across the two surfaces.
    ///
    /// The caller receives a ready-to-use pipeline with all opt-in
    /// plugins already wired. The per-file `source_dir` for
    /// `ResolveLinksPlugin` is per-walk state, set by the walk loops via
    /// [`Pipeline::set_resolve_links_source_dir`] — not part of
    /// construction.
    ///
    /// # Errors
    ///
    /// Returns [`PipelineSpecError`] when
    /// [`Self::code_highlight_themes_dir`] is `Some` and loading the
    /// `.tmTheme` files fails. Every other knob is infallible.
    pub fn build_pipeline(&self) -> Result<Pipeline, PipelineSpecError> {
        // Drift guard (zfb#917): exhaustive destructure — NO `..` rest
        // pattern. Adding a field to `PipelineSpec` must fail compilation
        // HERE until the author wires the new knob into the construction
        // below (the exhaustive `pipeline_spec_from_config` literal in
        // crates/zfb/src/commands/bundler_input.rs breaks too, forcing the
        // Config-resolution decision). Never add `..` to this pattern.
        let Self {
            code_highlight_theme,
            code_highlight_themes_dir,
            code_highlight_theme_light,
            code_highlight_theme_dark,
            code_highlight_mode,
            code_highlight_class_prefix,
            code_highlight_role_classes,
            strip_md_ext,
            resolve_source_map,
            gfm_constructs,
            toc,
            external_links,
            cjk_friendly,
            hard_breaks,
            features,
            build_context_roots,
        } = self;

        // Dual mode is active iff BOTH light and dark are Some.
        let dual_pair = match (
            code_highlight_theme_light.as_deref(),
            code_highlight_theme_dark.as_deref(),
        ) {
            (Some(light), Some(dark)) => Some((light, dark)),
            _ => None,
        };

        // Class / dual / single feature-aware entry point. `features = None`
        // is an empty feature set: the former-Core framework features are
        // off (the post-epic opt-in default, #583 / #586). Class mode takes
        // priority over the dual pair — config.rs validation already makes
        // `mode:"class"` mutually exclusive with every theme knob, so the
        // two branches can never both be live for a config that passed
        // validation; the ordering here is only a defensive tie-break for
        // hand-built specs that bypass config.rs.
        let mut pipeline = if *code_highlight_mode == CodeHighlightMode::Class {
            Pipeline::with_defaults_and_full_config_class(
                *gfm_constructs,
                code_highlight_themes_dir.as_deref(),
                *cjk_friendly,
                *hard_breaks,
                features.as_ref(),
                code_highlight_class_prefix.as_str(),
                code_highlight_role_classes,
            )
        } else if let Some((light, dark)) = dual_pair {
            Pipeline::with_defaults_and_full_config_dual(
                *gfm_constructs,
                code_highlight_themes_dir.as_deref(),
                *cjk_friendly,
                *hard_breaks,
                features.as_ref(),
                light,
                dark,
            )
        } else {
            Pipeline::with_defaults_and_full_config(
                code_highlight_theme.as_deref(),
                *gfm_constructs,
                code_highlight_themes_dir.as_deref(),
                *cjk_friendly,
                *hard_breaks,
                features.as_ref(),
            )
        }
        .map_err(|cause| PipelineSpecError {
            // `Err` only occurs when `themes_dir` is `Some` (theme-file
            // load), so the `unwrap_or_default()` empty-string branch is
            // unreachable.
            themes_dir: code_highlight_themes_dir
                .as_deref()
                .map(|d| d.display().to_string())
                .unwrap_or_default(),
            cause,
        })?;
        // Named config-driven mutators. Call order is NOT significant for
        // the effective visitor chain (`add_toc` inserts at a fixed
        // position; the others append to distinct slots) — pinned by
        // `named_mutator_call_order_does_not_split_the_fingerprint` in
        // crates/zfb-content/tests/pipeline_fingerprint.rs.
        if *strip_md_ext {
            pipeline.add_strip_md_ext();
        }
        if let Some(map) = resolve_source_map {
            pipeline.add_resolve_links(map.clone());
        }
        if let Some(toc_cfg) = toc.clone() {
            pipeline.add_toc(toc_cfg);
        }
        if let Some((cfg, site)) = external_links {
            pipeline.add_external_links(cfg.clone(), site.as_deref());
        }
        if let Some((root, public)) = build_context_roots.clone() {
            pipeline.set_build_context_roots(root, public);
        }
        Ok(pipeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Default` must keep the pre-config-API pipeline shape: same
    /// fingerprint as the bare feature-aware constructor with every
    /// optional knob disabled.
    #[test]
    fn default_spec_matches_bare_full_config_constructor() {
        let from_spec = PipelineSpec::default()
            .build_pipeline()
            .expect("default spec cannot fail")
            .config_fingerprint()
            .expect("default spec is fingerprintable");
        let bare = Pipeline::with_defaults_and_full_config(
            None,
            ResolvedGfmConstructs::CONSERVATIVE,
            None,
            true,
            false,
            None,
        )
        .expect("no themes_dir — cannot fail")
        .config_fingerprint()
        .expect("fingerprintable");
        assert_eq!(from_spec, bare);
    }

    /// Two pipelines built from the same spec must agree on the compile
    /// cache fingerprint — the property dev ticks rely on (zfb#905).
    #[test]
    fn same_spec_builds_fingerprint_equal_pipelines() {
        let spec = PipelineSpec {
            strip_md_ext: true,
            toc: Some(crate::plugins::toc::TocConfig::default()),
            external_links: Some((crate::plugins::ExternalLinksConfig::default(), None)),
            ..Default::default()
        };
        let a = spec.build_pipeline().expect("builds").config_fingerprint();
        let b = spec.build_pipeline().expect("builds").config_fingerprint();
        assert!(a.is_some());
        assert_eq!(a, b);
    }

    /// zfb#939: the production resolveMarkdownLinks wiring path — a spec
    /// carrying `resolve_source_map` — must build a CACHEABLE pipeline
    /// (fingerprint `Some`), and two ticks deriving the same map must
    /// agree so unchanged files hit the compile cache.
    #[test]
    fn resolve_source_map_spec_builds_a_fingerprinted_pipeline() {
        let mut map = HashMap::new();
        map.insert(
            PathBuf::from("/content/docs/other.mdx"),
            "/docs/other/".to_string(),
        );
        let spec = PipelineSpec {
            resolve_source_map: Some(map),
            ..Default::default()
        };
        let a = spec.build_pipeline().expect("builds").config_fingerprint();
        let b = spec.build_pipeline().expect("builds").config_fingerprint();
        assert!(
            a.is_some(),
            "resolveMarkdownLinks wired => config_fingerprint() must be Some (zfb#939)"
        );
        assert_eq!(a, b, "same map across ticks must share one fingerprint");
        assert_ne!(
            a,
            PipelineSpec::default()
                .build_pipeline()
                .expect("builds")
                .config_fingerprint(),
            "resolve-links output differs from the default shape — keys must split"
        );
    }

    /// zfb#952: two PipelineSpecs carrying the same `build_context_roots` pair
    /// must produce byte-identical config fingerprints — this is the property
    /// the bundler-walk and snapshot-walker surfaces rely on (they both call
    /// `PipelineSpec::build_pipeline` on the same spec, so structural parity is
    /// guaranteed, but the fingerprint must also be deterministic across calls).
    #[test]
    fn build_context_roots_armed_spec_is_fingerprint_stable() {
        let spec = PipelineSpec {
            build_context_roots: Some((
                PathBuf::from("/tmp/proj"),
                PathBuf::from("/tmp/proj/public"),
            )),
            ..Default::default()
        };
        let a = spec.build_pipeline().expect("builds").config_fingerprint();
        let b = spec.build_pipeline().expect("builds").config_fingerprint();
        assert!(a.is_some(), "armed spec must be fingerprintable (zfb#952)");
        assert_eq!(a, b, "same roots across builds must share one fingerprint");
        // Armed spec must produce a different fingerprint than an unarmed one.
        assert_ne!(
            a,
            PipelineSpec::default()
                .build_pipeline()
                .expect("builds")
                .config_fingerprint(),
            "armed roots must extend the fingerprint (zfb#952)"
        );
    }

    /// #977 fingerprint-neutrality tripwire: the cross-file anchor side
    /// channels (candidates + per-file headings) are OBSERVATIONAL, like
    /// the `ReadRecorder` — they must not touch the pipeline config
    /// fingerprint or the cache-key shape. An accidental fingerprint
    /// change splits every user's warm compile cache on upgrade, so the
    /// exact pre-#977 digests are pinned here as literals (captured at
    /// HEAD c47e644, before the channels landed).
    ///
    /// If this test fails because you INTENTIONALLY changed a
    /// fingerprint input (new config knob, FINGERPRINT_VERSION bump,
    /// feature-JSON shape change), re-capture the two digests and update
    /// the literals in the same commit — and say so in the commit
    /// message. If you did NOT intend to change fingerprint inputs, your
    /// change is silently invalidating every warm cache: fix it instead.
    #[test]
    fn side_channels_keep_fingerprint_byte_identical_to_pre_977() {
        let default_fp = PipelineSpec::default()
            .build_pipeline()
            .expect("builds")
            .config_fingerprint()
            .expect("fingerprintable");
        assert_eq!(
            default_fp, "0d4dd447ba9696acd7dcb4c12593fe4be396c84102ca0b4d56afc0ecc65a4f21",
            "default-spec fingerprint drifted from pre-#977 HEAD (re-captured for the \
             zfb#2250 githubAutolinks removal — the canonical features JSON shape lost a \
             key, see that PR)"
        );

        let feats: zfb_md_extras::MarkdownFeaturesConfig =
            serde_json::from_value(serde_json::json!({ "linkValidation": {} })).expect("valid");
        let armed = PipelineSpec {
            features: Some(feats),
            build_context_roots: Some((
                PathBuf::from("/tmp/proj"),
                PathBuf::from("/tmp/proj/public"),
            )),
            ..Default::default()
        };
        let armed_fp = armed
            .build_pipeline()
            .expect("builds")
            .config_fingerprint()
            .expect("fingerprintable");
        assert_eq!(
            armed_fp, "91b1274c82b95a09a2b6235956a1d78266a5ee38883e191e10e9a972b6bbe2e6",
            "linkValidation-armed fingerprint drifted from pre-#977 HEAD (re-captured for the \
             zfb#2250 githubAutolinks removal — the canonical features JSON shape lost a \
             key, see that PR)"
        );

        // Draining the channels is part of the observational contract
        // too — it must not perturb the fingerprint.
        let mut p = armed.build_pipeline().expect("builds");
        let _ = p.take_cross_file_link_candidates();
        let _ = p.take_file_headings();
        assert_eq!(p.config_fingerprint().as_deref(), Some(armed_fp.as_str()));
    }

    /// zfb#952: an absolute `publicDir` value is passed through unchanged by
    /// `resolve_under_root` — verified here by checking that the path stored in
    /// `build_context_roots` equals the input absolute path verbatim.
    #[test]
    fn absolute_public_dir_passthrough_in_build_context_roots() {
        // An absolute publicDir must appear unchanged in the fingerprint.
        let absolute_public = PathBuf::from("/var/www/public");
        let spec = PipelineSpec {
            build_context_roots: Some((PathBuf::from("/tmp/proj"), absolute_public.clone())),
            ..Default::default()
        };
        let pipeline = spec.build_pipeline().expect("builds");
        let (_, resolved_public) = pipeline.build_context_roots().expect("roots must be armed");
        assert_eq!(
            resolved_public, absolute_public,
            "absolute publicDir must pass through unchanged (zfb#952)"
        );
        // Relative public dir is joined to project root.
        let root = PathBuf::from("/tmp/proj");
        let relative_public = PathBuf::from("static");
        let expected_joined = root.join(&relative_public);
        let spec_relative = PipelineSpec {
            build_context_roots: Some((root.clone(), expected_joined.clone())),
            ..Default::default()
        };
        let pipeline_rel = spec_relative.build_pipeline().expect("builds");
        let (_, resolved_rel) = pipeline_rel
            .build_context_roots()
            .expect("roots must be armed");
        assert_eq!(
            resolved_rel, expected_joined,
            "relative publicDir must be joined to project root (zfb#952)"
        );
    }
    // Note: `default_spec_matches_bare_full_config_constructor` (above) is the
    // acceptance-criteria test for unarmed configs keeping byte-identical
    // fingerprints — it exercises `build_context_roots: None` (the Default).
}
