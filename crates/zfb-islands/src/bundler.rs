//! [`ClientBundler`] trait and shared types.
//!
//! Sub 2 of Epic #6 implements the production `EsbuildSubprocessBundler` —
//! today that means shelling out to the esbuild CLI binary. The trait below
//! is the seam between scanning ([`crate::scanner`]) and bundling: anything
//! handed an islands set and a [`BundleConfig`] can produce a
//! `dist/assets/islands-{hash}.js` asset.
//!
//! ## Why a trait?
//!
//! esbuild is delivered as a Go-built CLI. To keep the build pipeline
//! portable we shell out today, but the long-term plan is to swap in a
//! Rust-native bundler (e.g. `oxc_minifier` on `oxc_resolver`, or
//! `rolldown`, or whichever lands first) without touching the scanner or
//! the render orchestrator. See [`crate::future_rust_native`] for the
//! placeholder bundler that documents the swap path.
//!
//! ## Contract
//!
//! - The bundler MUST be deterministic: the same `(islands, config)` pair
//!   must produce the same asset bytes and module-id list. Downstream
//!   hashing in [`zfb_build::pipeline::prod::ProductionAssetPipeline`]
//!   assumes byte-stable bundle output.
//! - The output asset filename is the **stable** name
//!   `{outdir}/assets/islands.js` (per `zfb_types::STABLE_ISLANDS_FILENAME`)
//!   — `ProductionAssetPipeline` is the single source of truth for
//!   content hashing, so emitters under this crate no longer bake
//!   `-<hash>` into the on-disk name. Mirror of `zfb-css`'s
//!   `styles.css` stable-name layout.
//! - `module_ids` returns the bundled module identifiers (typically the
//!   `component_name` of each entry; bundlers MAY widen the list to also
//!   include shared chunks). Order MUST be stable across runs.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use zfb_types::{DIST_ASSETS_DIR, STABLE_ISLANDS_FILENAME, STABLE_ISLANDS_URL};

/// Stable identifier of a bundled module emitted by [`ClientBundler::bundle`].
///
/// Today this is just the component name; reserved as a type alias so future
/// bundlers can carry richer info (chunk index, original specifier, …)
/// without breaking call sites.
pub type ModuleId = String;

/// One `"use client"` component selected for hydration.
///
/// Two islands are equal iff their `(source_path, component_name)` pair is
/// equal — this is the stable component-name identity the scanner promises.
/// The newer [`marker_name`] field is metadata derived by the scanner from
/// the same source file and is intentionally **excluded** from the
/// equality / hashing contract so dedup behaviour is unchanged across
/// scanner upgrades.
#[derive(Debug, Clone)]
pub struct Island {
    /// Exported component name from the source file. For default exports
    /// this is the literal string `"default"`.
    pub component_name: String,
    /// Path to the .tsx/.ts/.jsx/.js source that carried the
    /// `"use client"` directive. Whatever form the scanner returned —
    /// typically the resolver's resolved-path representation.
    pub source_path: PathBuf,
    /// SSR-marker name — the string the SSR side will write into the
    /// `data-zfb-island` / `data-zfb-island-skip-ssr` attribute for this
    /// island. The hydration manifest in the production shared bundle is
    /// keyed on this name (NOT [`component_name`]), because the SSR side
    /// derives the marker via `captureComponentName(child)` =
    /// `child.type.displayName ?? child.type.name`, while the scanner
    /// records [`component_name`] as the export-side name (which is
    /// `"default"` for `export default function Foo()` and is mangled by
    /// esbuild minification at runtime).
    ///
    /// Resolution rules in the scanner (issue #149):
    ///
    /// - `export default function Foo()` / `export default class Foo` →
    ///   `marker_name = "Foo"` (the identifier name), NOT `"default"`.
    /// - `export function Foo()` / `export class Foo` /
    ///   `export const Foo = ...` → `marker_name = "Foo"`.
    /// - When the body of an exported function calls
    ///   `renderSsrSkipPlaceholder("X", ...)` (the host-side SSR-skip
    ///   wrapper convention used by zudo-doc-v2's `*-island.tsx` shims),
    ///   the literal first argument `"X"` overrides the rule above so the
    ///   manifest key matches `data-zfb-island-skip-ssr="X"` rather than
    ///   the wrapper's identifier name.
    /// - Anonymous default expressions (`export default () => …`,
    ///   `export default someFn(…)`) fall back to
    ///   [`component_name`] verbatim (typically `"default"`).
    pub marker_name: String,
}

impl Island {
    /// Construct a new island. `marker_name` defaults to `component_name`.
    ///
    /// No path canonicalisation is done here; callers building islands by
    /// hand should pass the same path representation that the bundler's
    /// resolver expects so dedup keys line up with the rest of the
    /// pipeline.
    pub fn new(component_name: impl Into<String>, source_path: impl Into<PathBuf>) -> Self {
        let component_name = component_name.into();
        Self {
            marker_name: component_name.clone(),
            component_name,
            source_path: source_path.into(),
        }
    }

    /// Construct a new island with an explicit `marker_name` distinct
    /// from `component_name`. Use this when the scanner determines the
    /// SSR-marker name differs from the export-side name (default
    /// exports of named functions, SSR-skip wrappers, etc.).
    pub fn with_marker_name(
        component_name: impl Into<String>,
        source_path: impl Into<PathBuf>,
        marker_name: impl Into<String>,
    ) -> Self {
        Self {
            component_name: component_name.into(),
            source_path: source_path.into(),
            marker_name: marker_name.into(),
        }
    }
}

// Manually-implemented equality / hashing contract:
//
// Two islands are equal iff their `(source_path, component_name)` pair is
// equal. `marker_name` is derived metadata — including it would mean a
// scanner upgrade that refines the marker derivation (e.g. detecting a
// new helper) would silently change the dedup behaviour for the same
// source file. Excluding it keeps the contract stable across scanner
// upgrades and matches the historical (`derive(PartialEq)`) shape from
// before issue #149.
impl PartialEq for Island {
    fn eq(&self, other: &Self) -> bool {
        self.component_name == other.component_name && self.source_path == other.source_path
    }
}

impl Eq for Island {}

impl std::hash::Hash for Island {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.component_name.hash(state);
        self.source_path.hash(state);
    }
}

/// Bundle configuration handed to [`ClientBundler::bundle`].
#[derive(Debug, Clone)]
pub struct BundleConfig {
    /// Minify the output. Production: `true`. Dev: `false`.
    pub minify: bool,

    /// Emit a sourcemap alongside the asset (and, for esbuild, a
    /// `//# sourceMappingURL=` comment). Production: `false`. Dev: `true`.
    pub sourcemap: bool,

    /// Output root. The asset lands at the stable path
    /// `{outdir}/assets/islands.js`. (Hashing is the
    /// `ProductionAssetPipeline`'s job — see crate-level contract.)
    /// Default: `dist/`.
    pub outdir: PathBuf,

    /// Public base URL prefix used by [`bundle_link_href`].
    /// Default: `"/"`.
    pub base_url: String,

    /// JSX import source the esbuild subprocess should target via
    /// `--jsx=automatic --jsx-import-source=<value>`. Mirrors
    /// `zfb_render::adapters::Adapter::jsx_import_source()` and
    /// `zfb_build::bundler::BundleConfig::jsx_import_source`.
    ///
    /// Why this matters: without `--jsx=automatic --jsx-import-source=…`
    /// esbuild's default classic JSX transform emits bare
    /// `React.createElement(…)` / `React.Fragment` references in the
    /// bundled island code. When host components have been migrated to
    /// `preact/compat` for hooks (no `React` namespace import), those
    /// references are dangling and the bundle throws
    /// `ReferenceError: React is not defined` at mount time
    /// (issue #151 / zudolab/zudo-doc#1355 Wave 8). Setting this
    /// field to `"preact"` (the default, matching
    /// [`FrameworkKind::Preact`]) routes the JSX transform through
    /// `preact/jsx-runtime` so no `React` symbol is ever emitted.
    pub jsx_import_source: String,

    /// When `true`, the islands bundler injects a side-effect
    /// `import "@takazudo/zfb-runtime/client-router";` into the synthetic
    /// shared-bundle entry so the `<ClientRouter />` View Transitions
    /// runtime (`init()`) ships in `assets/islands.js` with no
    /// `"use client"` boilerplate on the consumer's side (issue #289).
    ///
    /// The build command sets this from the islands scanner's
    /// [`crate::ScanMeta::uses_client_router`].
    ///
    /// Default `false` — when unset the generated entry source is
    /// byte-identical to a pre-#289 build, so a project not using
    /// `<ClientRouter />` sees zero new bytes in its islands bundle.
    pub client_router: bool,
}

impl Default for BundleConfig {
    fn default() -> Self {
        Self {
            minify: false,
            sourcemap: true,
            outdir: PathBuf::from("dist"),
            base_url: "/".to_string(),
            jsx_import_source: FrameworkKind::default().jsx_import_source().to_string(),
            client_router: false,
        }
    }
}

impl BundleConfig {
    /// Production preset: minify on, sourcemap off.
    pub fn production() -> Self {
        Self {
            minify: true,
            sourcemap: false,
            ..Self::default()
        }
    }

    /// Dev preset: minify off, sourcemap on. Same as [`Default`].
    pub fn dev() -> Self {
        Self::default()
    }

    /// Override the output directory (chainable).
    pub fn with_outdir(mut self, outdir: impl Into<PathBuf>) -> Self {
        self.outdir = outdir.into();
        self
    }

    /// Override the public base URL (chainable).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Toggle minification (chainable).
    pub fn with_minify(mut self, minify: bool) -> Self {
        self.minify = minify;
        self
    }

    /// Toggle sourcemap emission (chainable).
    pub fn with_sourcemap(mut self, sourcemap: bool) -> Self {
        self.sourcemap = sourcemap;
        self
    }

    /// Override the JSX import source the esbuild subprocess targets via
    /// `--jsx-import-source=<value>` (chainable). Use the framework's
    /// `jsx_import_source` accessor (e.g.
    /// `FrameworkKind::Preact.jsx_import_source()`,
    /// `zfb_render::adapters::Adapter::jsx_import_source()`) to derive
    /// the value rather than hardcoding a literal at the call site.
    pub fn with_jsx_import_source(mut self, jsx_import_source: impl Into<String>) -> Self {
        self.jsx_import_source = jsx_import_source.into();
        self
    }

    /// Toggle auto-inclusion of the `<ClientRouter />` runtime (chainable).
    ///
    /// See [`BundleConfig::client_router`].
    pub fn with_client_router(mut self, client_router: bool) -> Self {
        self.client_router = client_router;
        self
    }
}

/// Result of a successful [`ClientBundler::bundle`] call.
#[derive(Debug, Clone)]
pub struct BundleOutput {
    /// Output file path on disk — the stable form
    /// `dist/assets/islands.js`. `ProductionAssetPipeline` reads these
    /// bytes and renames to `dist/assets/islands-<hash>.js` at deploy
    /// time; the name handed to callers here stays unhashed so the
    /// pipeline can drive HTML rewrites against
    /// `STABLE_ISLANDS_URL` without coordinating an extra string
    /// channel.
    pub asset_path: PathBuf,

    /// Public URL the renderer should reference — the stable form
    /// `/assets/islands.js` (see `zfb_types::STABLE_ISLANDS_URL`).
    /// Computed via [`bundle_link_href`] from the asset path and the
    /// `base_url` in [`BundleConfig`].
    pub asset_url: String,

    /// Identifiers of the modules included in the bundle. Order is stable
    /// across runs for a given input.
    pub module_ids: Vec<ModuleId>,
}

/// Abstraction over "bundle this islands set into a single browser-ready
/// JS asset".
///
/// See the module-level docs for the swap-in story behind the trait.
pub trait ClientBundler {
    /// Bundle `islands` according to `config`. Must be deterministic for a
    /// given `(islands, config)` pair.
    fn bundle(&self, islands: &[Island], config: &BundleConfig) -> Result<BundleOutput>;
}

/// One per-island bundle output, produced by
/// [`crate::EsbuildSubprocessBundler::bundle_per_island`].
///
/// Per-island bundles land at the stable path
/// `{outdir}/islands/{component}.js` so the runtime can dynamic-import
/// each island's JS independently. Sharing is the bundler's concern —
/// at this layer we just record one entry per island. The
/// content-hash field is still computed and exposed (see
/// [`IslandBundle::hash`]) for dev-mode change detection and for
/// downstream consumers that wrap this output through
/// `ProductionAssetPipeline`, but it is **not** baked into the
/// filename or URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IslandBundle {
    /// Component export name. Mirrors [`Island::component_name`] of the
    /// input island so callers can pair entries by name.
    pub component_name: String,
    /// Output file path on disk — the stable form
    /// `dist/islands/{component}.js`.
    pub asset_path: PathBuf,
    /// Public URL the runtime should `import()` from — the stable form
    /// `/islands/{component}.js`.
    pub asset_url: String,
    /// 8-char content hash (lowercase hex) of the bundled JS. Reported
    /// for dev-mode change detection and for downstream consumers
    /// that delegate hashing to `ProductionAssetPipeline`. The hash is
    /// **not** part of the on-disk filename or URL — those are
    /// stable-named per the S0 single-source-of-truth-for-hashing
    /// contract.
    pub hash: String,
}

/// Result of a successful per-island bundle pass.
///
/// In addition to the per-island bundles, the per-island pipeline emits
/// a small **runtime** bundle — the framework-agnostic shim that walks
/// `[data-zfb-island]` / `[data-zfb-island-skip-ssr]` elements in the
/// DOM, dynamic-imports the matching per-island bundle, and dispatches
/// to `hydrate` / `render`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerIslandBundleOutput {
    /// One entry per island, in the same order the input slice provided.
    pub islands: Vec<IslandBundle>,
    /// Runtime bundle file path — the stable form
    /// `dist/islands/islands-runtime.js`. Hashing, when needed, is
    /// applied later by `ProductionAssetPipeline`.
    pub runtime_asset_path: PathBuf,
    /// Runtime bundle public URL — the `<script type="module" src="…">`
    /// the page-router HTML pass injects into `<head>`. Stable form:
    /// `/islands/islands-runtime.js`.
    pub runtime_asset_url: String,
}

/// Which JS framework the islands pipeline should target.
///
/// This is intentionally a small enum local to `zfb-islands` so the
/// crate stays free of a `zfb-render` dependency (mirroring the
/// `Adapter` contract from there). The orchestrator wires the two
/// together at the seam where it constructs the bundler.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrameworkKind {
    /// Preact — bare `preact` + `preact/jsx-runtime`.
    #[default]
    Preact,
    /// React 18+ — `react` + `react-dom/client`.
    React,
}

impl FrameworkKind {
    /// Stable lowercase name. Mirrors `zfb_render::Adapter::name()`.
    pub fn name(self) -> &'static str {
        match self {
            FrameworkKind::Preact => "preact",
            FrameworkKind::React => "react",
        }
    }

    /// JSX import source for esbuild's automatic JSX transform.
    /// Mirrors `zfb_render::adapters::Adapter::jsx_import_source()` —
    /// the bundler passes this to esbuild as
    /// `--jsx-import-source=<value>` so the compiled output routes
    /// through `<value>/jsx-runtime` instead of the classic
    /// `React.createElement` shape.
    pub fn jsx_import_source(self) -> &'static str {
        match self {
            FrameworkKind::Preact => "preact",
            FrameworkKind::React => "react",
        }
    }

    /// Recover the [`FrameworkKind`] from a `jsx_import_source` string
    /// (the inverse of [`Self::jsx_import_source`]). Used by the
    /// shared-bundle path to derive the mount-glue framework from
    /// [`BundleConfig::jsx_import_source`] — the single source of truth
    /// the orchestrator already sets via `with_jsx_import_source`. Keeping
    /// one field drive both the esbuild `--jsx-import-source` flag AND the
    /// emitted hydration glue makes the two structurally incapable of
    /// diverging (a React JSX transform with a Preact `h()` mount thunk
    /// would crash at hydrate time). Anything other than `"react"` maps to
    /// `Preact`, matching the default.
    pub fn from_jsx_import_source(s: &str) -> Self {
        match s {
            "react" => FrameworkKind::React,
            _ => FrameworkKind::Preact,
        }
    }
}

/// Build the public URL for a per-island JS asset.
///
/// Mirrors [`bundle_link_href`] but lives under `/islands/` instead of
/// `/assets/` so per-island and shared bundles can share an outdir
/// without colliding. With S0's stable-naming contract the typical
/// inputs look like `dist/islands/Counter.js` →
/// `/islands/Counter.js`; `ProductionAssetPipeline` is the only
/// component allowed to substitute hashed forms.
pub fn island_link_href(base_url: &str, asset_path: &Path) -> String {
    let filename = asset_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let trimmed = base_url.trim_end_matches('/');
    format!("{trimmed}/islands/{filename}")
}

/// Build the public URL for the islands JS asset.
///
/// Mirrors `zfb_css::link_href` exactly — we don't depend on `zfb-css`,
/// but the behaviour is intentionally identical so the renderer can reach
/// for either helper interchangeably.
///
/// Examples:
/// ```
/// use std::path::PathBuf;
/// use zfb_islands::bundle_link_href;
///
/// let p = PathBuf::from("dist/assets/islands.js");
/// assert_eq!(bundle_link_href("/", &p), "/assets/islands.js");
/// assert_eq!(
///     bundle_link_href("https://cdn.example.com", &p),
///     "https://cdn.example.com/assets/islands.js",
/// );
/// ```
pub fn bundle_link_href(base_url: &str, asset_path: &Path) -> String {
    let filename = asset_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let trimmed = base_url.trim_end_matches('/');
    format!("{trimmed}/assets/{filename}")
}

/// Bytes-only payload describing the islands client bundle as a single
/// hashable production asset, ready for plugging into
/// `zfb_build::pipeline::prod::ProductionAssetPipeline` (S4 wiring).
///
/// The shape mirrors `zfb_build`'s `EmittedAsset` field-by-field but
/// stays in this crate to avoid a `zfb-islands → zfb-build` dep cycle
/// (`zfb-build` already imports — or will import via S4 — the islands
/// pipeline; the reverse direction would be circular). S4 lifts these
/// fields into a real `EmittedAsset` via the `Fn() -> Result<Option<…>>`
/// blanket impl on `AssetEmitter`, so no trait wrapper is needed here.
///
/// Per the S0 single-source-of-truth-for-hashing contract:
///
/// - `relative_path` is **stable** (`assets/islands.js`); the pipeline
///   inserts the content hash before the extension.
/// - `stable_url` is the unhashed public URL the renderer embeds
///   (`/assets/islands.js`); the pipeline rewrites every match in the
///   rendered HTML to the hashed form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductionIslandsAsset {
    /// Bundled JS bytes — already minified per `BundleConfig::production`
    /// when constructed that way. The pipeline is opaque to the contents;
    /// it only hashes and ships them.
    pub bytes: Vec<u8>,

    /// Output path relative to `dist_root` — the stable form
    /// `assets/islands.js`. Must include the file extension; the
    /// pipeline splices the hash in before the extension.
    pub relative_path: PathBuf,

    /// The unhashed public URL the rendered HTML uses by default
    /// (`/assets/islands.js`). The pipeline rewrites every match of this
    /// string in HTML bodies with the hashed equivalent.
    pub stable_url: String,
}

/// Run `bundler` over `islands` and return a bytes-only adapter payload
/// suitable for `ProductionAssetPipeline`.
///
/// **Empty input is a no-op** (with one exception). When `islands` is
/// empty (project carries no `"use client"` components) *and*
/// `config.client_router` is `false`, this returns `Ok(None)` *without*
/// invoking the bundler — so esbuild is never asked to write a
/// zero-entry-points bundle, no empty `dist/assets/islands.js` lands on
/// disk, and the rendered HTML stays free of a `<script>` tag pointing
/// at a non-existent asset (S3 acceptance criterion: islands emitter is
/// a no-op when the project has no islands).
///
/// The exception: when `config.client_router` is `true` the bundler runs
/// even on an empty `islands` slice, because the synthetic entry still
/// has to carry the `<ClientRouter />` runtime's side-effect import
/// (issue #289).
///
/// On a non-empty `islands` slice the bundler writes the stable
/// `<outdir>/assets/islands.js` file as part of its normal contract;
/// this adapter then reads those bytes back into memory and packages
/// them with the canonical relative path + stable URL constants from
/// `zfb-types`. The stable filename / URL match exactly what
/// `STABLE_ISLANDS_FILENAME` / `STABLE_ISLANDS_URL` declare so S1's head
/// injection and S4's pipeline rewrite agree on the same key.
pub fn build_production_islands_asset(
    bundler: &dyn ClientBundler,
    islands: &[Island],
    config: &BundleConfig,
) -> Result<Option<ProductionIslandsAsset>> {
    // Empty input is a no-op — UNLESS the project opts into
    // `<ClientRouter />` (issue #289). A purely static page that wants
    // View Transitions has no `"use client"` islands but still needs the
    // client-router runtime bundled, so the islands asset must be emitted
    // for the side-effect import alone.
    if islands.is_empty() && !config.client_router {
        return Ok(None);
    }

    let output = bundler
        .bundle(islands, config)
        .context("islands production emitter: bundler.bundle() failed")?;

    let bytes = std::fs::read(&output.asset_path).with_context(|| {
        format!(
            "islands production emitter: failed to read bundled asset bytes from {}",
            output.asset_path.display()
        )
    })?;

    let relative_path = PathBuf::from(DIST_ASSETS_DIR).join(STABLE_ISLANDS_FILENAME);

    Ok(Some(ProductionIslandsAsset {
        bytes,
        relative_path,
        stable_url: STABLE_ISLANDS_URL.to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn island_equality_uses_path_and_name() {
        let a = Island::new("Counter", "/abs/components/Counter.tsx");
        let b = Island::new("Counter", "/abs/components/Counter.tsx");
        let c = Island::new("Counter", "/abs/components/Other.tsx");
        let d = Island::new("Other", "/abs/components/Counter.tsx");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
    }

    #[test]
    fn bundle_config_presets() {
        let prod = BundleConfig::production();
        assert!(prod.minify);
        assert!(!prod.sourcemap);

        let dev = BundleConfig::dev();
        assert!(!dev.minify);
        assert!(dev.sourcemap);
    }

    #[test]
    fn bundle_config_chainable() {
        let cfg = BundleConfig::production()
            .with_outdir("build")
            .with_base_url("https://cdn.example.com/")
            .with_minify(false)
            .with_sourcemap(true);
        assert_eq!(cfg.outdir, PathBuf::from("build"));
        assert_eq!(cfg.base_url, "https://cdn.example.com/");
        assert!(!cfg.minify);
        assert!(cfg.sourcemap);
    }

    #[test]
    fn bundle_config_default_jsx_import_source_is_preact() {
        // Issue #151: the default JSX import source must match the
        // default `FrameworkKind` (Preact) so esbuild's
        // --jsx=automatic transform routes through `preact/jsx-runtime`
        // instead of emitting bare `React.createElement` references.
        assert_eq!(BundleConfig::default().jsx_import_source, "preact");
        assert_eq!(BundleConfig::production().jsx_import_source, "preact");
        assert_eq!(BundleConfig::dev().jsx_import_source, "preact");
    }

    #[test]
    fn bundle_config_with_jsx_import_source_overrides() {
        let cfg = BundleConfig::default().with_jsx_import_source("react");
        assert_eq!(cfg.jsx_import_source, "react");
    }

    #[test]
    fn framework_kind_jsx_import_source_matches_zfb_render_adapter_contract() {
        // Mirrors `zfb_render::adapters::Adapter::jsx_import_source()` —
        // the value the bundler hands to esbuild via
        // `--jsx-import-source=<value>` must agree with what the
        // renderer's SWC pipeline targets, otherwise the SSR'd HTML
        // and the hydrated bundle disagree on how JSX compiles.
        assert_eq!(FrameworkKind::Preact.jsx_import_source(), "preact");
        assert_eq!(FrameworkKind::React.jsx_import_source(), "react");
    }

    #[test]
    fn framework_kind_from_jsx_import_source_round_trips() {
        // `from_jsx_import_source` is the inverse used by the shared-bundle
        // path to derive the mount-glue framework from
        // `BundleConfig::jsx_import_source` (one field driving both the
        // esbuild flag and the emitted glue). Round-trip both variants and
        // confirm the default fallback for unknown sources.
        for fw in [FrameworkKind::Preact, FrameworkKind::React] {
            assert_eq!(
                FrameworkKind::from_jsx_import_source(fw.jsx_import_source()),
                fw
            );
        }
        assert_eq!(
            FrameworkKind::from_jsx_import_source("react"),
            FrameworkKind::React
        );
        assert_eq!(
            FrameworkKind::from_jsx_import_source("preact"),
            FrameworkKind::Preact
        );
        // Unknown / empty falls back to the Preact default.
        assert_eq!(
            FrameworkKind::from_jsx_import_source("solid"),
            FrameworkKind::Preact
        );
        assert_eq!(
            FrameworkKind::from_jsx_import_source(""),
            FrameworkKind::Preact
        );
    }

    #[test]
    fn link_href_normalises_trailing_slash() {
        // S0 contract: emitter writes the stable filename
        // `assets/islands.js`. Hashed forms are produced by
        // `ProductionAssetPipeline`, not by the bundler.
        let p = PathBuf::from("dist/assets/islands.js");
        assert_eq!(bundle_link_href("/", &p), "/assets/islands.js");
        assert_eq!(bundle_link_href("", &p), "/assets/islands.js");
        assert_eq!(
            bundle_link_href("https://cdn.example.com/", &p),
            "https://cdn.example.com/assets/islands.js"
        );
        assert_eq!(
            bundle_link_href("https://cdn.example.com", &p),
            "https://cdn.example.com/assets/islands.js"
        );
    }

    /// Test double for the S3 production-emitter adapter.
    ///
    /// Records every `bundle()` invocation so the no-islands no-op test
    /// can assert the bundler was never called, and writes a configured
    /// payload to the stable `<outdir>/assets/islands.js` path so the
    /// happy-path test can read deterministic bytes back.
    struct RecordingBundler {
        payload: Vec<u8>,
        calls: std::cell::Cell<usize>,
    }

    impl RecordingBundler {
        fn new(payload: impl Into<Vec<u8>>) -> Self {
            Self {
                payload: payload.into(),
                calls: std::cell::Cell::new(0),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.get()
        }
    }

    impl ClientBundler for RecordingBundler {
        fn bundle(&self, islands: &[Island], config: &BundleConfig) -> Result<BundleOutput> {
            self.calls.set(self.calls.get() + 1);

            let asset_path = config
                .outdir
                .join(zfb_types::DIST_ASSETS_DIR)
                .join(zfb_types::STABLE_ISLANDS_FILENAME);
            if let Some(parent) = asset_path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&asset_path, &self.payload).unwrap();

            let asset_url = bundle_link_href(&config.base_url, &asset_path);
            let module_ids = islands.iter().map(|i| i.component_name.clone()).collect();
            Ok(BundleOutput {
                asset_path,
                asset_url,
                module_ids,
            })
        }
    }

    #[test]
    fn build_production_islands_asset_returns_none_when_no_islands() {
        // Acceptance criterion: with no islands the adapter must be a
        // no-op — the bundler is not invoked, no empty asset lands on
        // disk, and the rendered HTML keeps no stale `<script>` tag.
        let dir = tempfile::tempdir().unwrap();
        let bundler = RecordingBundler::new(b"unused".to_vec());
        let cfg = BundleConfig::default().with_outdir(dir.path().to_path_buf());

        let out = build_production_islands_asset(&bundler, &[], &cfg).unwrap();
        assert!(out.is_none(), "no islands → adapter must return None");
        assert_eq!(
            bundler.call_count(),
            0,
            "bundler.bundle() must not be invoked when islands slice is empty"
        );
        // No `assets/` directory was created — nothing was written.
        assert!(
            !dir.path().join(zfb_types::DIST_ASSETS_DIR).exists(),
            "no-islands case must not touch dist/assets/"
        );
    }

    #[test]
    fn build_production_islands_asset_runs_bundler_for_client_router_with_no_islands() {
        // Issue #289: a project that uses `<ClientRouter />` but has no
        // `"use client"` islands still needs the islands asset emitted so
        // the client-router runtime's side-effect import ships. The
        // empty-islands no-op must NOT fire when `config.client_router`
        // is true.
        let dir = tempfile::tempdir().unwrap();
        let payload = b"import \"@takazudo/zfb-runtime/client-router\";\n".to_vec();
        let bundler = RecordingBundler::new(payload.clone());
        let cfg = BundleConfig::default()
            .with_outdir(dir.path().to_path_buf())
            .with_client_router(true);

        let asset = build_production_islands_asset(&bundler, &[], &cfg)
            .unwrap()
            .expect("client_router=true → adapter runs even with zero islands");

        assert_eq!(
            bundler.call_count(),
            1,
            "bundler must run for a client-router-only project"
        );
        assert_eq!(asset.bytes, payload);
    }

    #[test]
    fn build_production_islands_asset_returns_bytes_and_stable_url() {
        // Happy path: non-empty islands → adapter runs the bundler,
        // reads the stable `assets/islands.js` bytes back, and packages
        // them with the canonical relative path + stable URL constants
        // from `zfb-types`. Hashing is `ProductionAssetPipeline`'s job
        // (S4); this adapter must NOT bake any hash into the URL or
        // path.
        let dir = tempfile::tempdir().unwrap();
        let payload = b"// bundled islands payload\nexport const x = 1;\n".to_vec();
        let bundler = RecordingBundler::new(payload.clone());
        let cfg = BundleConfig::default().with_outdir(dir.path().to_path_buf());
        let islands = vec![Island::new("Counter", "/abs/components/Counter.tsx")];

        let asset = build_production_islands_asset(&bundler, &islands, &cfg)
            .unwrap()
            .expect("non-empty islands → adapter returns Some(_)");

        assert_eq!(bundler.call_count(), 1);
        assert_eq!(asset.bytes, payload);
        assert_eq!(
            asset.relative_path,
            PathBuf::from(zfb_types::DIST_ASSETS_DIR).join(zfb_types::STABLE_ISLANDS_FILENAME)
        );
        assert_eq!(asset.stable_url, zfb_types::STABLE_ISLANDS_URL);
        // Stable URL must NOT carry a hash — the pipeline owns hashing.
        assert!(
            !asset.stable_url.contains('-') || asset.stable_url == zfb_types::STABLE_ISLANDS_URL,
            "adapter must emit the unhashed stable URL: got {}",
            asset.stable_url
        );
    }

    #[test]
    fn build_production_islands_asset_propagates_bundler_errors() {
        // Bundler failure surfaces as an error from the adapter (with
        // context wrapping it for the diagnostic chain). The adapter
        // must not swallow the failure into `Ok(None)` — that would be
        // indistinguishable from "no islands" and silently drop the
        // `<script>` tag.
        struct FailingBundler;
        impl ClientBundler for FailingBundler {
            fn bundle(&self, _: &[Island], _: &BundleConfig) -> Result<BundleOutput> {
                Err(anyhow::anyhow!("synthetic bundler failure"))
            }
        }

        let cfg = BundleConfig::default();
        let islands = vec![Island::new("Counter", "/abs/components/Counter.tsx")];
        let err = build_production_islands_asset(&FailingBundler, &islands, &cfg)
            .expect_err("bundler error must propagate");
        let chain: String = err
            .chain()
            .map(|e| format!("{e}"))
            .collect::<Vec<_>>()
            .join(" / ");
        assert!(
            chain.contains("synthetic bundler failure"),
            "underlying error must be in the chain: {chain}"
        );
    }
}
