//! CLI argument parsing for the `zfb` binary.
//!
//! The top-level [`Cli`] type holds the parsed clap state. Each subcommand
//! variant in [`Command`] wraps a dedicated `*Args` struct so that command
//! modules in `crate::commands` can take a single `&FooArgs` parameter rather
//! than a long parameter list. Keep these struct shapes stable — adding
//! fields is fine, but renaming or removing fields is a contract break for
//! each `commands/<name>.rs` handler.

use std::path::PathBuf;
use std::sync::OnceLock;

use clap::{ArgAction, Args, Parser, Subcommand};
use zfb_toolchain_pins::{EXPECTED_ESBUILD_VERSION, EXPECTED_TAILWIND_VERSION};

/// The detailed version report shown by `zfb --version`.
///
/// Keep the short `version` value on [`Cli`] unchanged: clap renders it for
/// `-V`, while this report adds the versions of the external binaries bundled
/// with the executable.
fn long_version() -> &'static str {
    // clap's `Str` stores a static string, so build this report once and keep
    // it for the process lifetime instead of allocating on every command.
    static REPORT: OnceLock<String> = OnceLock::new();
    REPORT
        .get_or_init(|| {
            let release_version =
                option_env!("ZFB_RELEASE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
            format!(
                "{release_version}\nembedded Tailwind CSS: {EXPECTED_TAILWIND_VERSION}\nembedded esbuild: {EXPECTED_ESBUILD_VERSION}"
            )
        })
        .as_str()
}

/// Top-level CLI for the `zfb` binary.
#[derive(Debug, Parser)]
// ZFB_RELEASE_VERSION is set by the release CI (= packages/zfb/package.json version).
// Local builds without the env var fall back to CARGO_PKG_VERSION (0.0.0 placeholder).
#[command(
    name = "zfb",
    version = option_env!("ZFB_RELEASE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
    long_version = long_version(),
    about = "zudo-front-builder"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// Subcommand dispatch. Each variant carries its own argument struct so that
/// command implementations stay decoupled from clap.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Scaffold a new project from a template.
    New(NewArgs),
    /// Run the local development server.
    Dev(DevArgs),
    /// Build the project for production.
    Build(BuildArgs),
    /// Preview a previously built project.
    Preview(PreviewArgs),
    /// Typecheck the project and validate content collections against
    /// their schemas. Equivalent in spirit to Astro's `astro check`.
    Check(CheckArgs),
}

/// Arguments for `zfb new`.
#[derive(Debug, Args)]
pub struct NewArgs {
    /// Name of the new project (used as the destination directory).
    pub name: String,

    /// Template to scaffold from. v0 ships two templates, sourced from
    /// `crates/zfb/templates/<name>/` and baked into the binary at compile
    /// time: `basic-blog` (default, ships a `package.json`) and
    /// `node-free` (no `package.json` / no `pnpm install` step, for
    /// projects run with no Node/pnpm on `PATH`).
    #[arg(long, default_value = "basic-blog")]
    pub template: String,
}

/// Arguments for `zfb dev`.
///
/// `host` and `port` are intentionally `Option<_>` (no clap `default_value`)
/// so the command body can layer "CLI > config > built-in default" cleanly.
/// Adding a clap default would erase the distinction between "user passed
/// --port 8080" and "user accepted the default 3000".
///
/// The `after_help` block documents the lazy dev-render environment
/// switches (issue #1027): the dev server defaults to lazy rendering
/// (changed routes are marked stale and re-rendered on first request);
/// `ZFB_DEV_EAGER=1` is the user-facing escape hatch back to fully
/// eager per-tick rendering, and `ZFB_LAZY_DEV_RENDER=0|1` is the
/// precise override that wins over `ZFB_DEV_EAGER` when both are set.
/// `ZFB_DEV_BOOT_LAZY=1` (issue #1057) additionally defers the BOOT
/// render: a valid prebuilt `dist/` is served immediately and each route
/// re-renders on its first request; with no servable `dist/` it warns and
/// falls back to the eager boot render (hinting `cold`). `ZFB_DEV_BOOT_LAZY=cold`
/// (issue #1806) is the seedless variant: it defers the same way with no
/// `dist/` required at all, at the cost of every route serving the dev 404
/// page (with livereload) until its own first request — unless a `dist/`
/// from an unrelated prior build happens to already sit on disk, in which
/// case the server's disk waterfall keeps serving those (possibly stale)
/// bytes for not-yet-rendered routes, same as Auto's seed does today.
/// `ZFB_DEV_DEFER_BUNDLE=0` (issue #1188) opts out of the #1182 bundle
/// deferral under boot-lazy (either variant), so the renderer is built
/// before bind (no SSR-only 404 window) at the cost of a slower
/// first-accept.
#[derive(Debug, Args)]
#[command(after_help = "Environment variables:
  ZFB_DEV_EAGER=1          Disable lazy dev rendering: re-render every affected
                           route eagerly on each file change (escape hatch
                           restoring the pre-lazy behaviour).
  ZFB_LAZY_DEV_RENDER=0|1  Precise override of the lazy dev-render switch
                           (1|true forces lazy, 0|false forces eager). Takes
                           precedence over ZFB_DEV_EAGER when both are set.
  ZFB_DEV_BOOT_LAZY=1      Opt-in fast boot: when a valid prebuilt `dist/` is
                           present, serve it immediately and defer per-route
                           rendering to the first request, instead of rendering
                           every route at boot. Requires lazy rendering (no-op
                           when ZFB_DEV_EAGER is set). Warns and falls back to
                           the eager boot render (hinting `cold`) when no
                           servable `dist/` exists. Off by default.
  ZFB_DEV_BOOT_LAZY=cold   Seedless boot-lazy: defers per-route rendering to
                           the first request the same way, but WITHOUT
                           requiring a prebuilt `dist/` — a route with no
                           fallback artifact on disk serves the dev 404 page
                           (with livereload) until its first request; a
                           leftover `dist/` from an unrelated prior build (not
                           required or checked here) still serves its
                           possibly-stale bytes first, same as Auto's seed
                           does. Use when no `dist/` exists yet; `=1` would
                           warn and fall back to the eager boot render in
                           that case instead.
  ZFB_DEV_DEFER_BUNDLE=0   Opt out of the #1182 boot-lazy bundle deferral: build
                           the renderer before bind (no SSR-only 404 window)
                           at the cost of a slower first-accept. On by default
                           when boot-lazy is active.")]
pub struct DevArgs {
    /// Port to bind the dev server to. Falls back to `port` from
    /// `zfb.config.json`, then to `3000`.
    #[arg(long)]
    pub port: Option<u16>,

    /// Host interface to bind the dev server to. Falls back to `host`
    /// from `zfb.config.json`, then to `localhost`. Bare `--host` (no value)
    /// is a Vite-style shortcut for `0.0.0.0` (expose to the LAN); an absent
    /// flag stays `None` so the "CLI > config > built-in default" layering
    /// above is preserved.
    #[arg(long, num_args = 0..=1, default_missing_value = "0.0.0.0")]
    pub host: Option<String>,
}

/// Arguments for `zfb build`.
#[derive(Debug, Args)]
pub struct BuildArgs {
    /// Output directory for the production build. Falls back to `outDir`
    /// from `zfb.config.*`, then to `dist`.
    #[arg(long)]
    pub outdir: Option<PathBuf>,

    /// Enable production HTML minification for this build.
    #[arg(
        long = "minify-html",
        action = ArgAction::SetTrue,
        conflicts_with = "no_minify_html"
    )]
    minify_html: bool,

    /// Disable production HTML minification for this build.
    #[arg(
        long = "no-minify-html",
        action = ArgAction::SetTrue,
        conflicts_with = "minify_html"
    )]
    no_minify_html: bool,

    /// Fail the build (non-zero exit) when broken links are found.
    ///
    /// Force-enables link validation with its default configuration when
    /// `markdown.features.link_validation` is otherwise unset, so this flag
    /// always has an effect rather than silently doing nothing on a bare
    /// project. See issue #2112 (Link Gating epic).
    #[arg(
        long = "strict-broken",
        action = ArgAction::SetTrue,
        conflicts_with = "no_strict_broken"
    )]
    strict_broken: bool,

    /// Do not fail the build on broken links, even if `strictBrokenLinks` is
    /// enabled in `zfb.config.*`.
    #[arg(
        long = "no-strict-broken",
        action = ArgAction::SetTrue,
        conflicts_with = "strict_broken"
    )]
    no_strict_broken: bool,

    /// Fail the build (non-zero exit) when a content-collection `.md`/`.mdx`
    /// entry falls back to `<pre data-zfb-content-fallback>` because its
    /// compiled JSX does not parse. Off by default: the fallback always
    /// warns and the build always exits 0 unless this (or the config
    /// `strictContentBridge` field) is enabled. See issue #2220.
    #[arg(
        long = "strict-content-bridge",
        action = ArgAction::SetTrue,
        conflicts_with = "no_strict_content_bridge"
    )]
    strict_content_bridge: bool,

    /// Do not fail the build on a content-bridge fallback, even if
    /// `strictContentBridge` is enabled in `zfb.config.*`.
    #[arg(
        long = "no-strict-content-bridge",
        action = ArgAction::SetTrue,
        conflicts_with = "strict_content_bridge"
    )]
    no_strict_content_bridge: bool,

    /// Write a JSON render artifact for every markdown/MDX-backed HTML
    /// route whose page renders exactly one top-level content region. See
    /// the config `emitRenderArtifacts` field for the full contract
    /// (Render Artifact Export epic #2421).
    #[arg(
        long = "emit-render-artifacts",
        action = ArgAction::SetTrue,
        conflicts_with = "no_emit_render_artifacts"
    )]
    emit_render_artifacts: bool,

    /// Do not write render artifacts for this build, even if
    /// `emitRenderArtifacts` is enabled in `zfb.config.*`.
    #[arg(
        long = "no-emit-render-artifacts",
        action = ArgAction::SetTrue,
        conflicts_with = "emit_render_artifacts"
    )]
    no_emit_render_artifacts: bool,
}

impl BuildArgs {
    /// The user-facing HTML-minification CLI state.
    ///
    /// Kept as a tri-state so command orchestration can layer
    /// "explicit CLI > config/preset > default" without treating an omitted
    /// flag as an explicit `false`.
    pub fn minify_html(&self) -> BuildMinifyHtml {
        match (self.minify_html, self.no_minify_html) {
            (true, false) => BuildMinifyHtml::Enabled,
            (false, true) => BuildMinifyHtml::Disabled,
            (false, false) => BuildMinifyHtml::Unspecified,
            // clap rejects this via `conflicts_with`; keep the branch
            // deterministic for direct struct construction in tests.
            (true, true) => BuildMinifyHtml::Disabled,
        }
    }

    /// The user-facing strict-broken-links CLI state.
    ///
    /// Kept as a tri-state so command orchestration can layer
    /// "explicit CLI > config `strictBrokenLinks` > default false" without
    /// treating an omitted flag as an explicit `false`.
    pub fn strict_broken_links(&self) -> BuildStrictBrokenLinks {
        match (self.strict_broken, self.no_strict_broken) {
            (true, false) => BuildStrictBrokenLinks::Enabled,
            (false, true) => BuildStrictBrokenLinks::Disabled,
            (false, false) => BuildStrictBrokenLinks::Unspecified,
            // clap rejects this via `conflicts_with`; keep the branch
            // deterministic for direct struct construction in tests.
            (true, true) => BuildStrictBrokenLinks::Disabled,
        }
    }

    /// The user-facing strict-content-bridge CLI state.
    ///
    /// Kept as a tri-state so command orchestration can layer
    /// "explicit CLI > config `strictContentBridge` > default false"
    /// without treating an omitted flag as an explicit `false`.
    pub fn strict_content_bridge(&self) -> BuildStrictContentBridge {
        match (self.strict_content_bridge, self.no_strict_content_bridge) {
            (true, false) => BuildStrictContentBridge::Enabled,
            (false, true) => BuildStrictContentBridge::Disabled,
            (false, false) => BuildStrictContentBridge::Unspecified,
            // clap rejects this via `conflicts_with`; keep the branch
            // deterministic for direct struct construction in tests.
            (true, true) => BuildStrictContentBridge::Disabled,
        }
    }

    /// The user-facing emit-render-artifacts CLI state.
    ///
    /// Kept as a tri-state so command orchestration can layer
    /// "explicit CLI > config `emitRenderArtifacts` > default false"
    /// without treating an omitted flag as an explicit `false`.
    pub fn emit_render_artifacts(&self) -> BuildEmitRenderArtifacts {
        match (self.emit_render_artifacts, self.no_emit_render_artifacts) {
            (true, false) => BuildEmitRenderArtifacts::Enabled,
            (false, true) => BuildEmitRenderArtifacts::Disabled,
            (false, false) => BuildEmitRenderArtifacts::Unspecified,
            // clap rejects this via `conflicts_with`; keep the branch
            // deterministic for direct struct construction in tests.
            (true, true) => BuildEmitRenderArtifacts::Disabled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildMinifyHtml {
    Unspecified,
    Enabled,
    Disabled,
}

impl BuildMinifyHtml {
    pub fn as_option(self) -> Option<bool> {
        match self {
            Self::Unspecified => None,
            Self::Enabled => Some(true),
            Self::Disabled => Some(false),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStrictBrokenLinks {
    Unspecified,
    Enabled,
    Disabled,
}

impl BuildStrictBrokenLinks {
    pub fn as_option(self) -> Option<bool> {
        match self {
            Self::Unspecified => None,
            Self::Enabled => Some(true),
            Self::Disabled => Some(false),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStrictContentBridge {
    Unspecified,
    Enabled,
    Disabled,
}

impl BuildStrictContentBridge {
    pub fn as_option(self) -> Option<bool> {
        match self {
            Self::Unspecified => None,
            Self::Enabled => Some(true),
            Self::Disabled => Some(false),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildEmitRenderArtifacts {
    Unspecified,
    Enabled,
    Disabled,
}

impl BuildEmitRenderArtifacts {
    pub fn as_option(self) -> Option<bool> {
        match self {
            Self::Unspecified => None,
            Self::Enabled => Some(true),
            Self::Disabled => Some(false),
        }
    }
}

/// Arguments for `zfb preview`.
///
/// `port`, `host`, and `outdir` are `Option<_>` for the same reason as the
/// `DevArgs` fields — see the doc-comment there — so the command body can
/// layer "CLI > config > built-in default" cleanly.
#[derive(Debug, Args)]
pub struct PreviewArgs {
    /// Port to bind the preview server to. Falls back to `port` from
    /// `zfb.config.json`, then to `4321`.
    #[arg(long)]
    pub port: Option<u16>,

    /// Host interface to bind the preview server to. Falls back to `host`
    /// from `zfb.config.json`, then to `localhost`. Pass `0.0.0.0` to expose
    /// the built site to other devices on the LAN — or just bare `--host`
    /// (no value), a Vite-style shortcut for the same `0.0.0.0`. An absent
    /// flag stays `None` so the "CLI > config > built-in default" layering
    /// is preserved.
    #[arg(long, num_args = 0..=1, default_missing_value = "0.0.0.0")]
    pub host: Option<String>,

    /// Directory to serve the previously built artifacts from. Falls back to
    /// `outDir` from `zfb.config.*`, then to `dist`. In adapter mode this is
    /// only an existence pre-check — `wrangler dev` serves the directories
    /// named in the project's wrangler config, and a non-default selected
    /// value triggers a warning to that effect.
    #[arg(long)]
    pub outdir: Option<PathBuf>,
}

/// Arguments for `zfb check`.
///
/// Two failure modes:
///
/// 1. TypeScript errors — `tsc --noEmit` is invoked as a subprocess on
///    the project. Anything tsc would flag in normal CI.
/// 2. Content collection schema violations — every entry's frontmatter
///    is validated against the JSON Schema declared in
///    `zfb.config.json`'s `collections[].schema` field (when present).
///
/// Either failure mode produces a non-zero exit. `--skip-tsc` is
/// useful when the project hasn't installed TypeScript yet (or for
/// schema-only CI lanes).
#[derive(Debug, Args)]
pub struct CheckArgs {
    /// Skip the `tsc --noEmit` subprocess. Schema validation still runs.
    /// Useful when the project has no TypeScript dependency installed
    /// yet but still wants schema enforcement in CI.
    #[arg(long)]
    pub skip_tsc: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev_host(argv: &[&str]) -> Option<String> {
        match Cli::try_parse_from(argv).expect("parse").command {
            Command::Dev(args) => args.host,
            other => panic!("expected dev subcommand, got {other:?}"),
        }
    }

    fn preview_host(argv: &[&str]) -> Option<String> {
        match Cli::try_parse_from(argv).expect("parse").command {
            Command::Preview(args) => args.host,
            other => panic!("expected preview subcommand, got {other:?}"),
        }
    }

    fn build_outdir(argv: &[&str]) -> Option<PathBuf> {
        match Cli::try_parse_from(argv).expect("parse").command {
            Command::Build(args) => args.outdir,
            other => panic!("expected build subcommand, got {other:?}"),
        }
    }

    fn preview_outdir(argv: &[&str]) -> Option<PathBuf> {
        match Cli::try_parse_from(argv).expect("parse").command {
            Command::Preview(args) => args.outdir,
            other => panic!("expected preview subcommand, got {other:?}"),
        }
    }

    fn build_minify_html(argv: &[&str]) -> BuildMinifyHtml {
        match Cli::try_parse_from(argv).expect("parse").command {
            Command::Build(args) => args.minify_html(),
            other => panic!("expected build subcommand, got {other:?}"),
        }
    }

    fn build_strict_broken_links(argv: &[&str]) -> BuildStrictBrokenLinks {
        match Cli::try_parse_from(argv).expect("parse").command {
            Command::Build(args) => args.strict_broken_links(),
            other => panic!("expected build subcommand, got {other:?}"),
        }
    }

    fn build_strict_content_bridge(argv: &[&str]) -> BuildStrictContentBridge {
        match Cli::try_parse_from(argv).expect("parse").command {
            Command::Build(args) => args.strict_content_bridge(),
            other => panic!("expected build subcommand, got {other:?}"),
        }
    }

    fn build_emit_render_artifacts(argv: &[&str]) -> BuildEmitRenderArtifacts {
        match Cli::try_parse_from(argv).expect("parse").command {
            Command::Build(args) => args.emit_render_artifacts(),
            other => panic!("expected build subcommand, got {other:?}"),
        }
    }

    // Absent `--host` must stay `None` so the command body's
    // "CLI > config > built-in default" layering still kicks in.
    #[test]
    fn dev_host_absent_is_none() {
        assert_eq!(dev_host(&["zfb", "dev"]), None);
    }

    // Bare `--host` is the Vite-style LAN shortcut → `0.0.0.0`.
    #[test]
    fn dev_host_bare_defaults_to_all_interfaces() {
        assert_eq!(dev_host(&["zfb", "dev", "--host"]), Some("0.0.0.0".into()));
    }

    #[test]
    fn dev_host_explicit_value_is_preserved() {
        assert_eq!(
            dev_host(&["zfb", "dev", "--host", "1.2.3.4"]),
            Some("1.2.3.4".into())
        );
    }

    #[test]
    fn preview_host_absent_is_none() {
        assert_eq!(preview_host(&["zfb", "preview"]), None);
    }

    #[test]
    fn preview_host_bare_defaults_to_all_interfaces() {
        assert_eq!(
            preview_host(&["zfb", "preview", "--host"]),
            Some("0.0.0.0".into())
        );
    }

    #[test]
    fn preview_host_explicit_value_is_preserved() {
        assert_eq!(
            preview_host(&["zfb", "preview", "--host", "10.0.0.1"]),
            Some("10.0.0.1".into())
        );
    }

    #[test]
    fn build_outdir_absent_is_none() {
        assert_eq!(build_outdir(&["zfb", "build"]), None);
    }

    #[test]
    fn build_outdir_explicit_value_is_preserved() {
        assert_eq!(
            build_outdir(&["zfb", "build", "--outdir", "custom"]),
            Some(PathBuf::from("custom"))
        );
    }

    #[test]
    fn preview_outdir_absent_is_none() {
        assert_eq!(preview_outdir(&["zfb", "preview"]), None);
    }

    #[test]
    fn preview_outdir_explicit_value_is_preserved() {
        assert_eq!(
            preview_outdir(&["zfb", "preview", "--outdir", "custom"]),
            Some(PathBuf::from("custom"))
        );
    }

    #[test]
    fn build_minify_html_absent_is_unspecified() {
        assert_eq!(
            build_minify_html(&["zfb", "build"]),
            BuildMinifyHtml::Unspecified
        );
    }

    #[test]
    fn build_minify_html_flag_enables() {
        assert_eq!(
            build_minify_html(&["zfb", "build", "--minify-html"]),
            BuildMinifyHtml::Enabled
        );
    }

    #[test]
    fn build_no_minify_html_flag_disables() {
        assert_eq!(
            build_minify_html(&["zfb", "build", "--no-minify-html"]),
            BuildMinifyHtml::Disabled
        );
    }

    #[test]
    fn build_minify_html_flags_conflict() {
        let err = Cli::try_parse_from(["zfb", "build", "--minify-html", "--no-minify-html"])
            .expect_err("conflicting minify flags must be rejected");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn build_help_documents_html_minify_flags() {
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        let build = cmd
            .find_subcommand_mut("build")
            .expect("build subcommand exists");
        let help = build.render_long_help().to_string();
        assert!(
            help.contains("--minify-html"),
            "build help must document --minify-html:\n{help}"
        );
        assert!(
            help.contains("--no-minify-html"),
            "build help must document --no-minify-html:\n{help}"
        );
    }

    #[test]
    fn build_strict_broken_links_absent_is_unspecified() {
        assert_eq!(
            build_strict_broken_links(&["zfb", "build"]),
            BuildStrictBrokenLinks::Unspecified
        );
    }

    #[test]
    fn build_strict_broken_links_flag_enables() {
        assert_eq!(
            build_strict_broken_links(&["zfb", "build", "--strict-broken"]),
            BuildStrictBrokenLinks::Enabled
        );
    }

    #[test]
    fn build_no_strict_broken_links_flag_disables() {
        assert_eq!(
            build_strict_broken_links(&["zfb", "build", "--no-strict-broken"]),
            BuildStrictBrokenLinks::Disabled
        );
    }

    #[test]
    fn build_strict_broken_links_flags_conflict() {
        let err = Cli::try_parse_from(["zfb", "build", "--strict-broken", "--no-strict-broken"])
            .expect_err("conflicting strict-broken flags must be rejected");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn build_help_documents_strict_broken_links_flags() {
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        let build = cmd
            .find_subcommand_mut("build")
            .expect("build subcommand exists");
        let help = build.render_long_help().to_string();
        assert!(
            help.contains("--strict-broken"),
            "build help must document --strict-broken:\n{help}"
        );
        assert!(
            help.contains("--no-strict-broken"),
            "build help must document --no-strict-broken:\n{help}"
        );
    }

    #[test]
    fn build_strict_content_bridge_absent_is_unspecified() {
        assert_eq!(
            build_strict_content_bridge(&["zfb", "build"]),
            BuildStrictContentBridge::Unspecified
        );
    }

    #[test]
    fn build_strict_content_bridge_flag_enables() {
        assert_eq!(
            build_strict_content_bridge(&["zfb", "build", "--strict-content-bridge"]),
            BuildStrictContentBridge::Enabled
        );
    }

    #[test]
    fn build_no_strict_content_bridge_flag_disables() {
        assert_eq!(
            build_strict_content_bridge(&["zfb", "build", "--no-strict-content-bridge"]),
            BuildStrictContentBridge::Disabled
        );
    }

    #[test]
    fn build_strict_content_bridge_flags_conflict() {
        let err = Cli::try_parse_from([
            "zfb",
            "build",
            "--strict-content-bridge",
            "--no-strict-content-bridge",
        ])
        .expect_err("conflicting strict-content-bridge flags must be rejected");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn build_help_documents_strict_content_bridge_flags() {
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        let build = cmd
            .find_subcommand_mut("build")
            .expect("build subcommand exists");
        let help = build.render_long_help().to_string();
        assert!(
            help.contains("--strict-content-bridge"),
            "build help must document --strict-content-bridge:\n{help}"
        );
        assert!(
            help.contains("--no-strict-content-bridge"),
            "build help must document --no-strict-content-bridge:\n{help}"
        );
    }

    #[test]
    fn build_emit_render_artifacts_absent_is_unspecified() {
        assert_eq!(
            build_emit_render_artifacts(&["zfb", "build"]),
            BuildEmitRenderArtifacts::Unspecified
        );
    }

    #[test]
    fn build_emit_render_artifacts_flag_enables() {
        assert_eq!(
            build_emit_render_artifacts(&["zfb", "build", "--emit-render-artifacts"]),
            BuildEmitRenderArtifacts::Enabled
        );
    }

    #[test]
    fn build_no_emit_render_artifacts_flag_disables() {
        assert_eq!(
            build_emit_render_artifacts(&["zfb", "build", "--no-emit-render-artifacts"]),
            BuildEmitRenderArtifacts::Disabled
        );
    }

    #[test]
    fn build_emit_render_artifacts_flags_conflict() {
        let err = Cli::try_parse_from([
            "zfb",
            "build",
            "--emit-render-artifacts",
            "--no-emit-render-artifacts",
        ])
        .expect_err("conflicting emit-render-artifacts flags must be rejected");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn build_help_documents_emit_render_artifacts_flags() {
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        let build = cmd
            .find_subcommand_mut("build")
            .expect("build subcommand exists");
        let help = build.render_long_help().to_string();
        assert!(
            help.contains("--emit-render-artifacts"),
            "build help must document --emit-render-artifacts:\n{help}"
        );
        assert!(
            help.contains("--no-emit-render-artifacts"),
            "build help must document --no-emit-render-artifacts:\n{help}"
        );
    }

    /// Issue #1027 — `zfb dev --help` documents the lazy-render env
    /// switches: the `ZFB_DEV_EAGER=1` escape hatch and the
    /// `ZFB_LAZY_DEV_RENDER` precise override (with its precedence).
    #[test]
    fn dev_help_documents_lazy_render_env_switches() {
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        let dev = cmd
            .find_subcommand_mut("dev")
            .expect("dev subcommand exists");
        let help = dev.render_long_help().to_string();
        assert!(
            help.contains("ZFB_DEV_EAGER=1"),
            "dev help must document the ZFB_DEV_EAGER escape hatch:\n{help}"
        );
        assert!(
            help.contains("ZFB_LAZY_DEV_RENDER"),
            "dev help must document the ZFB_LAZY_DEV_RENDER override:\n{help}"
        );
        assert!(
            help.contains("precedence over ZFB_DEV_EAGER"),
            "dev help must state the precedence rule:\n{help}"
        );
    }

    /// Issue #1057 — `zfb dev --help` documents the opt-in boot-lazy switch.
    /// Issue #1806/#1810 — also documents the seedless `cold` variant.
    #[test]
    fn dev_help_documents_boot_lazy_env_switch() {
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        let dev = cmd
            .find_subcommand_mut("dev")
            .expect("dev subcommand exists");
        let help = dev.render_long_help().to_string();
        assert!(
            help.contains("ZFB_DEV_BOOT_LAZY=1"),
            "dev help must document the ZFB_DEV_BOOT_LAZY opt-in switch:\n{help}"
        );
        assert!(
            help.contains("ZFB_DEV_BOOT_LAZY=cold"),
            "dev help must document the ZFB_DEV_BOOT_LAZY=cold seedless variant:\n{help}"
        );
    }

    /// Issue #1188 — `zfb dev --help` documents the bundle-deferral opt-out.
    #[test]
    fn dev_help_documents_defer_bundle_optout() {
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        let dev = cmd
            .find_subcommand_mut("dev")
            .expect("dev subcommand exists");
        let help = dev.render_long_help().to_string();
        assert!(
            help.contains("ZFB_DEV_DEFER_BUNDLE=0"),
            "dev help must document the ZFB_DEV_DEFER_BUNDLE opt-out:\n{help}"
        );
    }
}
