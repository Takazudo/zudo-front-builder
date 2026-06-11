//! Shared helpers that wire the bundler and renderer
//! (`zfb_build::bundler` + `zfb_build::renderer`) into the `zfb build`
//! and `zfb dev` commands.
//!
//! This module deliberately does **not** start the embedded V8 host or call
//! [`zfb_build::renderer::render_all`] directly. It owns the pure /
//! testable pieces of the wiring:
//!
//! - turning the [`zfb_router::Router`] scan into a concrete
//!   [`zfb_build::renderer::RouteUniverseEntry`] list (static routes
//!   only, for now — see [`build_route_universe`] for the dynamic
//!   `paths()` follow-up note),
//! - reading `TsxFrontmatter::prerender` for every page module that has
//!   one and folding that into the
//!   [`zfb_build::renderer::RendererInput::prerender_map`] shape, and
//!
//! The renderer call itself happens in [`crate::commands::build`] and
//! [`crate::commands::dev`] so the dev-mode long-lived `RendererState`
//! plumbing stays close to the dev session.
//!
//! ## Why a shared module
//!
//! `zfb build` and `zfb dev` produce the same `route_universe` and
//! `prerender_map`. Keeping the construction in one place prevents the
//! two commands from drifting (e.g. one resolving `prerender = false`
//! and the other not). The split also gives us a unit-test surface that
//! does not need a real renderer.
//!
//! ## Wave-3 (T7) gaps
//!
//! 1. **Dynamic `paths()` expansion.** Routes whose template contains
//!    `[slug]`, `[page]`, `[...rest]`, etc. need each page module's
//!    `paths()` export evaluated to enumerate the concrete URLs. This
//!    module wires the **static fast path**: when the page's `paths()`
//!    return value is a JSON-literal array (the
//!    [`zfb_render::paths_extract`] contract), [`expand_dynamic_routes`]
//!    runs it through [`zfb_render::paths::resolve_paths`] and produces
//!    one [`RouteUniverseEntry`] per resolved URL. Pages whose `paths()`
//!    is non-literal (helper calls, `await import`, runtime data
//!    sources, branching, …) are still surfaced via
//!    [`DeferredDynamicRoute`] with a reason string so callers can warn
//!    clearly. A future sub-task will add a runtime evaluator that
//!    consumes those deferred entries by booting a worker; until then
//!    they are skipped from `dist/`.
//!
//! 2. **Worker entry wrapping.** The bundler ([`zfb_build::bundle`])
//!    emits an ESM bundle that exports `routes` + `hydrateIsland` but
//!    not `default { fetch }`, while
//!    [`zfb_build::renderer::render_all`] expects a Worker-shaped
//!    bundle. Wrapping is its own sub-task; today the renderer call
//!    will fail at embedded V8 host boot time with an error message
//!    that names the missing `default` export. The CLI
//!    surfaces that error verbatim instead of swallowing it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use include_dir::{include_dir, Dir};
use zfb_build::renderer::RouteUniverseEntry;
#[cfg(feature = "embed_v8")]
use zfb_build::EmbeddedV8Host;
use zfb_content::{extract_tsx_frontmatter, TsxFrontmatterError};
use zfb_render::paths::{resolve_paths, PathsCache, PathsError, Segment as PathsSegment};
use zfb_render::paths_extract::{extract_paths, PathsExtractError, PathsExtraction};
use zfb_router::{Route, RouteKind, Segment};

// ---------------------------------------------------------------------------
// Embedded @takazudo/* + framework runtime packages
// ---------------------------------------------------------------------------
//
// The `build.rs` script populates `$OUT_DIR/vendor/` with two groups of
// packages:
//
// 1. `@takazudo/zfb` and `@takazudo/zfb-runtime` (sub #198): TypeScript source
//    of the runtime packages, copied from the `packages/` workspace dirs.
// 2. `preact`, `preact-render-to-string`, `hono` (sub #209): published trees
//    copied from zfb's pnpm-installed `node_modules/.pnpm/<name>@<ver>*/` so
//    consumers without their own node_modules can still resolve framework
//    imports.
//
// Both groups land as siblings under `$OUT_DIR/vendor/`:
//
//   $OUT_DIR/vendor/
//     @takazudo/zfb/             (TS source + package.json)
//     @takazudo/zfb-runtime/     (TS source + package.json)
//     preact/                    (published dist/ + package.json + ...)
//     preact-render-to-string/   (published dist/ + package.json + ...)
//     hono/                      (published dist/ + package.json)
//
// `build.rs` emits `cargo:rustc-env=ZFB_VENDOR_DIR=<this dir>` so the
// `include_dir!` macro below embeds the whole tree at compile time.
//
// At runtime, [`embedded_node_modules`] extracts the tree into a freshly
// allocated tempdir shaped as a proper `node_modules/<pkg>/` layout, ready
// for esbuild resolution. The tempdir is kept alive for the duration of the
// build by returning the `TempDir` handle alongside the path.

/// Compile-time embedding of `$OUT_DIR/vendor/` (`@takazudo/*` + framework
/// runtime packages, staged by `build.rs`).
///
/// `include_dir!` expands `$VAR` using the env var set via `cargo:rustc-env`.
/// `build.rs` emits `cargo:rustc-env=ZFB_VENDOR_DIR=<path>` pointing at
/// `$OUT_DIR/vendor` so this path resolves at macro-expansion time.
static EMBEDDED_VENDOR: Dir<'_> = include_dir!("$ZFB_VENDOR_DIR");

/// Extract the embedded `@takazudo/*` packages and framework runtime packages
/// into a temporary directory structured as a `node_modules/` layout esbuild
/// can resolve.
///
/// Returns a `(TempDir, PathBuf)` pair where:
/// - `TempDir` must be kept alive for as long as esbuild runs (dropping it
///   removes the temp directory).
/// - `PathBuf` is the path to the `node_modules` root (i.e. the directory
///   that should be passed as `BundlerInput::node_modules_dir`).
///
/// # Errors
///
/// Returns an error if the temp directory cannot be created or if extracting
/// any embedded file fails.
pub fn embedded_node_modules() -> Result<(tempfile::TempDir, PathBuf)> {
    let dir = tempfile::tempdir().context("failed to create tempdir for embedded packages")?;
    let node_modules = dir.path().join("node_modules");
    // Skip the top-level `bin/` entry — those are helper binaries (esbuild,
    // tailwindcss-v4) staged by `stage_binaries_into_vendor` for
    // `embedded_binary()` to extract on demand. They have no business inside
    // a node_modules tree, and not extracting them avoids ~100 MB of wasted
    // copies on every esbuild bundler invocation.
    extract_dir_with_prefix_filtered(&EMBEDDED_VENDOR, &node_modules, Path::new(""), &|p| {
        p.iter().next().map(|s| s == "bin").unwrap_or(false)
    })
    .context("failed to extract embedded packages")?;
    let nm_path = node_modules;
    Ok((dir, nm_path))
}

/// Extract a single embedded helper binary (esbuild, tailwindcss-v4, …) from
/// the `bin/` subtree of [`EMBEDDED_VENDOR`] into a fresh tempdir so the TS
/// config loader and the CSS engine can shell out to it without a
/// workspace-relative `crates/zfb/binaries/` slot.
///
/// `name` is the binary's stem (e.g. `"esbuild"`, `"tailwindcss-v4"`). On
/// Windows this function additionally probes for `<name>.exe` so a caller
/// passing `"esbuild"` resolves the Windows variant transparently.
///
/// Returns a `(TempDir, PathBuf)` pair where:
/// - `TempDir` must be kept alive for as long as the subprocess runs
///   (dropping it removes the temp directory and the extracted binary).
/// - `PathBuf` is the path to the extracted binary on disk; on Unix the
///   executable bit is set at write time.
///
/// # Errors
///
/// Returns an error pointing the operator at the build-script staging step
/// (`stage_binaries_into_vendor` in `crates/zfb/build.rs`) when the binary is
/// not present in [`EMBEDDED_VENDOR`].
pub fn embedded_binary(name: &str) -> Result<(tempfile::TempDir, PathBuf)> {
    // Probe `bin/<name>` first; on Windows fall back to `bin/<name>.exe` so
    // callers can pass the stem without worrying about target_os.
    let primary = format!("bin/{name}");
    let file = EMBEDDED_VENDOR.get_file(&primary).or_else(|| {
        if cfg!(target_os = "windows") {
            EMBEDDED_VENDOR.get_file(format!("bin/{name}.exe"))
        } else {
            None
        }
    });
    let file = file.ok_or_else(|| {
        anyhow::anyhow!(
            "embedded binary `{name}` not found under bin/ inside the embedded vendor snapshot. \
             Make sure `crates/zfb/build.rs::stage_binaries_into_vendor` ran during the last \
             build (it copies crates/zfb/binaries/{{esbuild,tailwindcss-v4}} into \
             $OUT_DIR/vendor/bin/ so include_dir! picks them up)."
        )
    })?;

    let dir = tempfile::tempdir()
        .with_context(|| format!("failed to create tempdir for embedded binary `{name}`"))?;

    // Preserve any `.exe` suffix from the matched embedded path so callers
    // don't need to know the host platform.
    let dst_name = file
        .path()
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from(name));
    let dst = dir.path().join(&dst_name);
    std::fs::write(&dst, file.contents()).with_context(|| {
        format!(
            "failed to write embedded binary `{name}` to {}",
            dst.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to set executable bit on {}", dst.display()))?;
    }

    Ok((dir, dst))
}

/// Recursively extract all entries from `embedded` into `dest`, stripping
/// `prefix` from each entry's embedded path so that the layout under `dest`
/// mirrors only the relative structure beneath `prefix`.
///
/// For example, if the embedded root is `$OUT_DIR/vendor` and `prefix` is
/// `""` (empty), the entry `@takazudo/zfb/src/index.ts` is written to
/// `dest/@takazudo/zfb/src/index.ts`.
/// Recursively extract entries from `embedded` into `dest`, stripping
/// `prefix` from each entry's embedded path. Skips any entry whose
/// `prefix`-stripped path satisfies `skip`. Used by [`embedded_node_modules`]
/// to filter out the `bin/` subtree that's only relevant to
/// [`embedded_binary`].
fn extract_dir_with_prefix_filtered(
    embedded: &Dir<'_>,
    dest: &Path,
    prefix: &Path,
    skip: &dyn Fn(&Path) -> bool,
) -> Result<()> {
    use include_dir::DirEntry;
    for entry in embedded.entries() {
        match entry {
            DirEntry::Dir(sub) => {
                let rel = sub.path().strip_prefix(prefix).unwrap_or(sub.path());
                if skip(rel) {
                    continue;
                }
                let target = dest.join(rel);
                std::fs::create_dir_all(&target)
                    .with_context(|| format!("failed to create dir {}", target.display()))?;
                extract_dir_with_prefix_filtered(sub, dest, prefix, skip)?;
            }
            DirEntry::File(file) => {
                let rel = file.path().strip_prefix(prefix).unwrap_or(file.path());
                if skip(rel) {
                    continue;
                }
                let target = dest.join(rel);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("failed to create parent dir {}", parent.display())
                    })?;
                }
                std::fs::write(&target, file.contents()).with_context(|| {
                    format!("failed to write embedded file {}", target.display())
                })?;
            }
        }
    }
    Ok(())
}

/// A dynamic / catchall route surfaced by [`build_route_universe`].
///
/// Carries enough metadata for [`expand_dynamic_routes`] to:
///
/// 1. Read the source file and try static `paths()` extraction.
/// 2. Pass the segments (now shared [`zfb_types::Segment`], re-exported
///    by both `zfb_router` and `zfb_render`) to
///    [`zfb_render::paths::resolve_paths`] for URL reassembly.
/// 3. Honour the route's filename-convention `output_extension` when
///    deriving each resolved entry's output path (e.g. a dynamic
///    `[slug].xml.tsx` should still produce `<slug>.xml`, not
///    `<slug>/index.html`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDynamicRoute {
    /// Source path of the page module, used both for static extraction
    /// and to point the user at the offending file in a warning.
    pub source_path: PathBuf,
    /// Route template (e.g. `/blog/:slug`). Used as the
    /// [`RouteUniverseEntry::route_key`] for every resolved URL so the
    /// prerender map lookup keys consistently.
    pub template: String,
    /// Parsed segments from the router. Reused for URL reassembly.
    pub segments: Vec<Segment>,
    /// Filename-convention extension override from the router (None →
    /// the renderer-side default of `html`).
    pub output_extension: Option<String>,
}

/// A dynamic route whose `paths()` couldn't be statically expanded.
/// Surfaced by [`expand_dynamic_routes`] so callers can warn loudly
/// instead of silently dropping the page from `dist/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredDynamicRoute {
    /// Source path of the page module.
    pub source_path: PathBuf,
    /// Route template, e.g. `/blog/:slug`.
    pub template: String,
    /// Parsed segments from the router. Required by
    /// [`eval_deferred_paths_via_worker`] to reassemble concrete URLs
    /// from the `paths()` return value.
    pub segments: Vec<Segment>,
    /// Filename-convention extension override from the router (None →
    /// the renderer-side default of `html`). Carried through so the
    /// runtime evaluator can produce the correct output path.
    pub output_extension: Option<String>,
    /// Why static expansion failed: `paths` export missing, return
    /// value not a literal, etc. Suitable for direct inclusion in a
    /// build warning.
    pub reason: String,
}

/// Output of [`build_route_universe`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteUniversePlan {
    /// Concrete routes the renderer can drive today (every static
    /// route from the router scan, in router-sorted order).
    pub static_routes: Vec<RouteUniverseEntry>,
    /// Dynamic / catchall routes deferred to the `paths()` follow-up.
    pub deferred_dynamic: Vec<PendingDynamicRoute>,
}

/// Build the renderer's `route_universe` input from a [`zfb_router::Router`]
/// scan.
///
/// **Static routes** are translated 1:1 into
/// [`RouteUniverseEntry`]:
///
/// - `url_path` is the route's [`Route::template`] (the URL the worker
///   will be asked to serve).
/// - `output_path` follows [`Route::output_filename`], which honours the
///   filename-extension precedence rule (frontmatter override >
///   filename convention > `html` default).
/// - `route_key` is the same template string — the prerender map keys
///   on it.
///
/// **Dynamic / catchall routes** are collected into
/// [`RouteUniversePlan::deferred_dynamic`] so callers can warn rather
/// than silently dropping them. Wiring `paths()` expansion is a
/// follow-up task; until then dynamic routes don't reach the renderer.
pub fn build_route_universe(routes: &[Route]) -> RouteUniversePlan {
    let mut plan = RouteUniversePlan::default();
    for route in routes {
        match route.kind {
            RouteKind::Static => {
                let template = route.template();
                plan.static_routes.push(RouteUniverseEntry {
                    url_path: template.clone(),
                    output_path: route.output_filename(None),
                    route_key: template,
                    static_html: route.static_html,
                    source_path: if route.static_html {
                        Some(route.source_path.clone())
                    } else {
                        None
                    },
                });
            }
            RouteKind::Dynamic | RouteKind::Catchall => {
                plan.deferred_dynamic.push(PendingDynamicRoute {
                    source_path: route.source_path.clone(),
                    template: route.template(),
                    segments: route.segments.clone(),
                    output_extension: route.output_extension.clone(),
                });
            }
        }
    }
    plan
}

/// Params captured for one expanded dynamic-route URL. Mirrors the
/// `params` field shape exposed on `postBuild`'s `ctx.routes` manifest
/// (#262): dynamic segment values are scalars (`String`), catchall
/// segment values are arrays (`Vec<String>`) because the path parts were
/// joined with `/` for URL assembly but must surface as individual
/// segments in the manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedRouteParams {
    /// Dynamic params (single segment, e.g. `[slug]` → `"hello"`).
    pub scalars: BTreeMap<String, String>,
    /// Catchall params (multi-segment, e.g. `[...rest]` → `["a", "b"]`).
    pub arrays: BTreeMap<String, Vec<String>>,
}

/// One resolved dynamic URL plus its source-path / extension / params,
/// for building the postBuild route manifest (#262).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicResolvedEntry {
    /// Resolved URL path, e.g. `/blog/hello-world`.
    pub url_path: String,
    /// Output path relative to `dist/`, e.g. `blog/hello-world/index.html`.
    pub output_path: PathBuf,
    /// Source page module relative to the project root.
    pub source_path: PathBuf,
    /// Output extension (`html`, `xml`, `rss`, …).
    pub extension: String,
    /// Route template the resolved URL came from, e.g. `/blog/[slug]`.
    /// Join key against the build-time `prerender_map` so the postBuild
    /// manifest builder can populate `PostBuildRouteEntry::prerender`
    /// for dynamic entries. Mirrors `RouteUniverseEntry::route_key`.
    pub route_key: String,
    /// Resolved params for the URL.
    pub params: ResolvedRouteParams,
}

/// Outcome of [`expand_dynamic_routes`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DynamicExpansion {
    /// One [`RouteUniverseEntry`] per concrete URL produced by a
    /// successful static `paths()` extraction. Order: input route
    /// order, then `paths()` array order within each route.
    pub resolved: Vec<RouteUniverseEntry>,
    /// Routes whose `paths()` could not be statically expanded —
    /// non-literal return value, missing `paths` export, source
    /// unreadable, parse error, or [`PathsError`] from the resolver
    /// itself (bad shape, missing param, ambiguous URL, …). Each
    /// carries a short reason suitable for a build warning.
    pub deferred: Vec<DeferredDynamicRoute>,
    /// Params + metadata for each resolved URL, in the same order as
    /// [`DynamicExpansion::resolved`]. Used by the postBuild manifest
    /// builder (#262) to expose params to plugin `postBuild` callbacks.
    pub resolved_with_params: Vec<DynamicResolvedEntry>,
}

/// Walk the deferred dynamic routes from [`build_route_universe`] and
/// try to statically expand each into concrete URLs.
///
/// Pages whose `paths()` is a JSON-literal array (the
/// [`zfb_render::paths_extract`] contract) are handed to
/// [`zfb_render::paths::resolve_paths`] and produce one
/// [`RouteUniverseEntry`] per resolved URL. The `route_key` matches the
/// route's template so the prerender map join still works; the
/// `output_path` honours the dynamic route's filename-convention
/// extension (e.g. `[slug].xml.tsx` resolves to `foo.xml`, not
/// `foo/index.html`).
///
/// Pages whose `paths()` cannot be statically expanded are bundled into
/// [`DynamicExpansion::deferred`] with a one-line reason. A future
/// sub-task will pick those up and run them through a real JS runtime;
/// today they are skipped from `dist/` and surfaced as warnings.
///
/// `cache` is threaded through so callers can reuse a single
/// [`PathsCache`] across multiple invocations (e.g. dev-mode rebuilds);
/// `cache.miss_count()` and `cache.hit_count()` then reflect the whole
/// session.
/// Expand SSG dynamic routes from a static `paths()` extraction.
///
/// All routes in `deferred` are expected to be SSG (`prerender = true`).
/// SSR routes (`prerender = false`) must be pre-filtered by the caller
/// before calling this function — they bypass `paths()` expansion entirely
/// (see `build.rs` Approach B pre-filter and the `ssr_deferred` split).
///
/// Returns an error immediately if a route has no `paths()` export at all
/// (`PathsExtraction::Missing`). SSG dynamic routes require an explicit
/// `paths()` — without one the route would silently produce no pages.
/// Routes with a non-literal `paths()` (e.g. one that calls
/// `getCollection(...)`) are collected into [`DynamicExpansion::deferred`]
/// for Phase-2 runtime evaluation.
pub fn expand_dynamic_routes(
    deferred: &[PendingDynamicRoute],
    project_root: &Path,
    cache: &mut PathsCache,
) -> anyhow::Result<DynamicExpansion> {
    let mut out = DynamicExpansion::default();
    for route in deferred {
        match try_expand_one(route, project_root, cache) {
            Ok((entries, params_entries)) => {
                out.resolved.extend(entries);
                out.resolved_with_params.extend(params_entries);
            }
            // Missing paths() on an SSG dynamic route is a hard build
            // error: without it the route produces no pages and would
            // silently 404 at serve time. Matched on the typed variant
            // — a future reword of the producer's prose reason can't
            // silently downgrade this back to a defer.
            Err(TryExpandFailure::MissingPathsExport { source_display }) => {
                return Err(anyhow::anyhow!(
                    "no top-level `paths` export found in {source_display}; \
                     dynamic routes require one — add an exported `paths()` \
                     function that returns the list of URL params"
                ));
            }
            Err(TryExpandFailure::Other(reason)) => out.deferred.push(DeferredDynamicRoute {
                source_path: route.source_path.clone(),
                template: route.template.clone(),
                segments: route.segments.clone(),
                output_extension: route.output_extension.clone(),
                reason,
            }),
        }
    }
    Ok(out)
}

/// Typed failure shape from `try_expand_one`.
///
/// Splits the semantically-distinct "no `paths` export at all" case
/// (hard error for SSG routes — see `expand_dynamic_routes`) from every
/// other expansion failure (defer to Phase-2 runtime evaluation with the
/// reason as a diagnostic string). The previous design returned a single
/// `Result<_, String>` and the caller had to grep for a specific phrase
/// in the error message; that coupled the caller to the producer's exact
/// wording.
enum TryExpandFailure {
    /// No `paths` export found in the dynamic route's source file.
    /// `source_display` is the source path formatted for diagnostics.
    MissingPathsExport { source_display: String },
    /// Any other failure — non-literal `paths`, parse error, unreadable
    /// source. The string is the existing one-line diagnostic reason
    /// (consumed by the deferred-route reason field and downstream
    /// `warn_deferred_dynamic`).
    Other(String),
}

/// Try to expand a single dynamic route into concrete entries. Returns
/// `(universe_entries, manifest_entries)` on success, or a typed
/// [`TryExpandFailure`] on failure. `manifest_entries` carries params +
/// metadata for the postBuild route manifest (#262).
fn try_expand_one(
    route: &PendingDynamicRoute,
    project_root: &Path,
    cache: &mut PathsCache,
) -> Result<(Vec<RouteUniverseEntry>, Vec<DynamicResolvedEntry>), TryExpandFailure> {
    let abs = if route.source_path.is_absolute() {
        route.source_path.clone()
    } else {
        project_root.join(&route.source_path)
    };
    let file_name = abs
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| route.source_path.display().to_string());
    let source = std::fs::read_to_string(&abs)
        .map_err(|e| TryExpandFailure::Other(format!("could not read {} ({e})", abs.display())))?;
    let extraction = match extract_paths(&source, &file_name) {
        Ok(x) => x,
        Err(PathsExtractError::Parse { file, message }) => {
            return Err(TryExpandFailure::Other(format!(
                "parse error in {file}: {message}"
            )));
        }
    };
    let json = match extraction {
        PathsExtraction::Literal(v) => v,
        PathsExtraction::Missing => {
            return Err(TryExpandFailure::MissingPathsExport {
                source_display: abs.display().to_string(),
            });
        }
        PathsExtraction::NonLiteral { reason } => {
            return Err(TryExpandFailure::Other(format!(
                "{}: paths() not statically resolvable ({reason}); pending runtime evaluation",
                abs.display()
            )));
        }
    };

    let segs: Vec<PathsSegment> = route.segments.to_vec();
    let resolved = resolve_paths(cache, &route.template, &segs, &json).map_err(|e| {
        TryExpandFailure::Other(format!("{}: {}", abs.display(), format_paths_error(&e)))
    })?;
    Ok(expand_resolved_urls(
        &route.template,
        &route.segments,
        route.output_extension.as_deref(),
        &route.source_path,
        resolved,
    ))
}

/// Shared post-`resolve_paths` tail: build `RouteUniverseEntry` and
/// `DynamicResolvedEntry` vectors from a resolved-paths list.
///
/// Both [`try_expand_one`] (static extraction) and [`resolve_json_paths`]
/// (runtime JSON) arrive at the same `Vec<ResolvedPath>` and then perform
/// byte-identical processing. This helper owns that processing so neither
/// call site can drift from the other.
///
/// `output_extension` is the route's override (None → `"html"`).
fn expand_resolved_urls(
    template: &str,
    segments: &[Segment],
    output_extension: Option<&str>,
    source_path: &Path,
    resolved: Vec<zfb_render::paths::ResolvedPath>,
) -> (Vec<RouteUniverseEntry>, Vec<DynamicResolvedEntry>) {
    // Pre-compute which param names are catchall so we can split the
    // joined string back into a `Vec<String>` for the manifest (#262).
    let catchall_names: std::collections::HashSet<&str> = segments
        .iter()
        .filter_map(|seg| {
            if let Segment::Catchall(name) | Segment::OptionalCatchall(name) = seg {
                Some(name.as_str())
            } else {
                None
            }
        })
        .collect();

    let extension = output_extension.unwrap_or("html").to_string();
    let mut universe_out = Vec::with_capacity(resolved.len());
    let mut manifest_out = Vec::with_capacity(resolved.len());
    for r in resolved {
        let output_path = build_output_path_for_resolved_url(&r.url, output_extension);
        universe_out.push(RouteUniverseEntry {
            url_path: r.url.clone(),
            output_path: output_path.clone(),
            route_key: template.to_string(),
            static_html: false,
            source_path: None,
        });

        // Split the flat `HashMap<String, String>` from `ResolvedPath`
        // into scalar vs array buckets, splitting catchall strings on `/`.
        let mut scalars = BTreeMap::new();
        let mut arrays: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (k, v) in &r.params {
            if catchall_names.contains(k.as_str()) {
                // Catchall strings were joined from an array; split back.
                // An optional catchall's zero-segment case is the empty
                // string — that must surface as `[]` in the manifest, not
                // `[""]` (which `"".split('/')` would produce).
                let parts: Vec<String> = if v.is_empty() {
                    Vec::new()
                } else {
                    v.split('/').map(|s| s.to_string()).collect()
                };
                arrays.insert(k.clone(), parts);
            } else {
                scalars.insert(k.clone(), v.clone());
            }
        }

        manifest_out.push(DynamicResolvedEntry {
            url_path: r.url,
            output_path,
            source_path: source_path.to_path_buf(),
            extension: extension.clone(),
            route_key: template.to_string(),
            params: ResolvedRouteParams { scalars, arrays },
        });
    }
    (universe_out, manifest_out)
}

/// Compute the on-disk output path for a resolved dynamic URL, mirroring
/// the [`zfb_router::Route::output_filename`] contract:
///
/// - HTML pages render to `…/index.html` (so `/blog/hello` →
///   `blog/hello/index.html` and the index `/` → `index.html`).
/// - Non-HTML pages render to the bare URL path (so `/feed.xml` →
///   `feed.xml`); the URL itself already carries the extension because
///   the catchall reassembly preserves it.
fn build_output_path_for_resolved_url(url: &str, extension: Option<&str>) -> PathBuf {
    let ext = extension.unwrap_or("html");
    let trimmed = url.trim_start_matches('/');
    if ext == "html" {
        if trimmed.is_empty() {
            PathBuf::from("index.html")
        } else {
            PathBuf::from(trimmed).join("index.html")
        }
    } else {
        // Non-HTML: emit the URL path as-is. If, somehow, the resolved
        // URL is the bare root, fall back to `index.<ext>` so we never
        // emit an empty path.
        if trimmed.is_empty() {
            PathBuf::from(format!("index.{ext}"))
        } else {
            PathBuf::from(trimmed)
        }
    }
}

/// Render a [`PathsError`] without the long `Display` prefixes, since
/// the caller already prepends the source path and we don't want
/// `route` strings doubled in the warning.
fn format_paths_error(e: &PathsError) -> String {
    match e {
        PathsError::MissingParam { name, provided, .. } => {
            let pretty = provided
                .iter()
                .map(|k| format!("`{k}`"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "paths() entry is missing required param `{name}`: \
                 params must include `{name}`, got [{pretty}]"
            )
        }
        PathsError::ExtraParam { name, expected, .. } => {
            let pretty = expected
                .iter()
                .map(|k| format!("`{k}`"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "paths() entry has extra param `{name}` not in the route template: \
                 expected one of [{pretty}], got `{name}`"
            )
        }
        PathsError::InvalidParamType { name, reason, .. } => {
            format!("paths() entry has invalid param `{name}`: {reason}")
        }
        PathsError::InvalidPathsExport {
            field,
            reason,
            expected,
            ..
        } => {
            let field_note = match field {
                Some(f) => format!(" at `{f}`"),
                None => String::new(),
            };
            format!("paths() export is malformed{field_note}: {reason} (expected {expected})")
        }
        PathsError::AmbiguousResolution { reason, .. } => {
            format!("paths() produced ambiguous URLs: {reason}")
        }
    }
}

/// Dispatch handle for [`eval_deferred_paths_via_worker`].
///
/// Abstracts over the two ways the worker can be reached:
///
/// - `Http { base_url }` — base URL of a pre-running HTTP server (e.g.
///   `http://127.0.0.1:54321/`). Requests go through `reqwest::blocking`.
///   Use this when `Backend::Existing` is active; pass
///   `state.base_url().unwrap()` from the [`RendererState`].
///
/// - `EmbeddedV8 { host }` — in-process V8 host. Requests call
///   `host.dispatch_fetch` directly without a TCP hop. Pass a mutable
///   reference to the host extracted from the active [`RendererState`]
///   via `state.embedded_v8_host_mut()`. The adapter is `Send` via
///   [`crate::v8_host_adapter::ThreadedV8Host`].
///
/// The `__paths__` bundle registration in
/// `packages/zfb-runtime/src/router.ts` is unchanged — only the Rust
/// dispatch mechanism differs.
pub enum WorkerDispatch<'h> {
    /// HTTP path: `Backend::Existing` (pre-running server).
    Http {
        /// Base URL of the running worker (e.g. `http://127.0.0.1:54321/`).
        base_url: String,
        /// PhantomData carrying the `'h` lifetime so the enum's
        /// lifetime parameter stays in use even when the V8-only
        /// `EmbeddedV8` variant is feature-gated off (issue #371,
        /// sub-task 4.1a). Construct with `WorkerDispatch::http(...)`.
        #[doc(hidden)]
        _marker: std::marker::PhantomData<&'h ()>,
    },
    /// In-process V8 host (Sub 2 — `EmbeddedV8RenderHost`).
    ///
    /// Compiled in only when the `embed_v8` cargo feature is on
    /// (issue #371, sub-task 4.1a). On the V8-off path the variant
    /// disappears from the enum entirely so the `EmbeddedV8Host`
    /// trait is not referenced.
    #[cfg(feature = "embed_v8")]
    EmbeddedV8 {
        /// Mutable reference to the live host. The caller retains
        /// ownership; this function borrows it only for the duration of
        /// the call.
        host: &'h mut dyn EmbeddedV8Host,
    },
}

impl<'h> WorkerDispatch<'h> {
    /// Construct a `WorkerDispatch::Http` value.
    ///
    /// Prefer this over a struct literal because the variant carries a
    /// hidden `PhantomData<&'h ()>` field needed to keep the lifetime
    /// parameter in use when the V8-only `EmbeddedV8` variant is
    /// feature-gated out (issue #371, sub-task 4.1a). Using this
    /// constructor lets callers stay source-compatible across the
    /// feature toggle.
    pub fn http(base_url: String) -> Self {
        WorkerDispatch::Http {
            base_url,
            _marker: std::marker::PhantomData,
        }
    }
}

/// Evaluate non-literal `paths()` exports by querying the running worker's
/// synthetic `/__paths__/<encoded-route-key>` endpoint.
///
/// The endpoint is provided by `@takazudo/zfb-runtime`'s `createPageRouter`
/// when the bundle is loaded by the embedded V8 host. For each
/// [`DeferredDynamicRoute`] in `deferred`, this function:
///
/// 1. Percent-encodes the route key so it is safe as a URL path segment.
/// 2. Dispatches `GET /__paths__/<encoded-route-key>` to the running worker
///    — either via reqwest (HTTP path) or via `host.dispatch_fetch` (V8
///    embedded path), depending on `dispatch`.
/// 3. Parses the JSON response array and feeds it through [`resolve_paths`]
///    with the route's segment list.
/// 4. Folds the resolved [`RouteUniverseEntry`]s into
///    [`DynamicExpansion::resolved`]; failures go into
///    [`DynamicExpansion::deferred`].
///
/// **Dispatch dual-path:** the production embedded V8 path uses
/// `WorkerDispatch::EmbeddedV8 { host: ... }` to call the host's
/// `dispatch_fetch` directly. `WorkerDispatch::Http` is kept for
/// `Backend::Existing` callers (e.g. test fixtures that hand the renderer a
/// pre-running URL). Production code uses the in-process embedded V8 host;
/// there is no HTTP server or base_url on the SSG path.
///
/// The timeout applies per-request (HTTP path only). A generous default (30 s)
/// is used because `paths()` may run a content-collection query. For the V8
/// path, timeout is enforced by the host implementation itself.
pub fn eval_deferred_paths_via_worker(
    deferred: &[DeferredDynamicRoute],
    dispatch: &mut WorkerDispatch<'_>,
    cache: &mut PathsCache,
    timeout: Option<Duration>,
) -> DynamicExpansion {
    if deferred.is_empty() {
        return DynamicExpansion::default();
    }

    match dispatch {
        WorkerDispatch::Http { base_url, .. } => {
            eval_deferred_paths_http(deferred, base_url, cache, timeout)
        }
        #[cfg(feature = "embed_v8")]
        WorkerDispatch::EmbeddedV8 { host } => eval_deferred_paths_embedded(deferred, *host, cache),
    }
}

/// HTTP path: builds a reqwest client and drives one GET per route.
fn eval_deferred_paths_http(
    deferred: &[DeferredDynamicRoute],
    base_url: &str,
    cache: &mut PathsCache,
    timeout: Option<Duration>,
) -> DynamicExpansion {
    let per_request_timeout = timeout.unwrap_or(Duration::from_secs(30));
    // Construct on a fresh OS thread so reqwest's internal tokio
    // runtime drops cleanly — see the matching note in
    // `zfb_build::renderer::build_http_client`. Without this, the build
    // CLI's outer `tokio::main` poisons the drop and we panic with
    // `Cannot drop a runtime in a context where blocking is not allowed`.
    let client = match std::thread::scope(|s| {
        s.spawn(|| {
            reqwest::blocking::Client::builder()
                .timeout(per_request_timeout)
                .no_proxy()
                .build()
        })
        .join()
        .expect("client-builder thread panicked")
    }) {
        Ok(c) => c,
        Err(e) => {
            // Extremely unlikely — only happens if TLS init fails.
            let reason = format!("could not build HTTP client for paths evaluation: {e}");
            let deferred_out: Vec<DeferredDynamicRoute> = deferred
                .iter()
                .map(|r| DeferredDynamicRoute {
                    source_path: r.source_path.clone(),
                    template: r.template.clone(),
                    segments: r.segments.clone(),
                    output_extension: r.output_extension.clone(),
                    reason: reason.clone(),
                })
                .collect();
            return DynamicExpansion {
                resolved: Vec::new(),
                deferred: deferred_out,
                ..Default::default()
            };
        }
    };

    let base = base_url.trim_end_matches('/');
    let mut out = DynamicExpansion::default();

    for route in deferred {
        match eval_one_deferred_path_http(&client, base, route, cache) {
            Ok((entries, params_entries)) => {
                out.resolved.extend(entries);
                out.resolved_with_params.extend(params_entries);
            }
            Err(reason) => out.deferred.push(DeferredDynamicRoute {
                source_path: route.source_path.clone(),
                template: route.template.clone(),
                segments: route.segments.clone(),
                output_extension: route.output_extension.clone(),
                reason,
            }),
        }
    }

    out
}

/// Embedded V8 path: calls `host.dispatch_fetch` for each route.
#[cfg(feature = "embed_v8")]
fn eval_deferred_paths_embedded(
    deferred: &[DeferredDynamicRoute],
    host: &mut dyn EmbeddedV8Host,
    cache: &mut PathsCache,
) -> DynamicExpansion {
    let mut out = DynamicExpansion::default();

    for route in deferred {
        match eval_one_deferred_path_embedded(host, route, cache) {
            Ok((entries, params_entries)) => {
                out.resolved.extend(entries);
                out.resolved_with_params.extend(params_entries);
            }
            Err(reason) => out.deferred.push(DeferredDynamicRoute {
                source_path: route.source_path.clone(),
                template: route.template.clone(),
                segments: route.segments.clone(),
                output_extension: route.output_extension.clone(),
                reason,
            }),
        }
    }

    out
}

/// Query one route's `paths()` via HTTP and resolve it into
/// `(universe_entries, manifest_entries)`. Returns a one-line reason string on any failure.
fn eval_one_deferred_path_http(
    client: &reqwest::blocking::Client,
    base: &str,
    route: &DeferredDynamicRoute,
    cache: &mut PathsCache,
) -> Result<(Vec<RouteUniverseEntry>, Vec<DynamicResolvedEntry>), String> {
    // Percent-encode the route key. The worker's `/__paths__/:routeKey{.+}`
    // pattern captures the rest of the path (including encoded slashes),
    // and the TS side does `decodeURIComponent` on it. We must encode `/`
    // and `:` so they are not misinterpreted as path separators / Hono
    // parameter delimiters.
    let encoded = encode_route_key(&route.template);
    let url = format!("{base}/__paths__/{encoded}");

    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("HTTP request to {} failed: {}", url, e))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "worker returned {} for /__paths__/{}: {}",
            status.as_u16(),
            route.template,
            body.trim()
        ));
    }

    let json: serde_json::Value = resp.json().map_err(|e| {
        format!(
            "could not parse JSON from /__paths__/{}: {}",
            route.template, e
        )
    })?;

    resolve_json_paths(json, route, cache)
}

/// Query one route's `paths()` via the embedded V8 host and resolve it.
/// Returns `(universe_entries, manifest_entries)`, or a one-line reason string on any failure.
#[cfg(feature = "embed_v8")]
fn eval_one_deferred_path_embedded(
    host: &mut dyn EmbeddedV8Host,
    route: &DeferredDynamicRoute,
    cache: &mut PathsCache,
) -> Result<(Vec<RouteUniverseEntry>, Vec<DynamicResolvedEntry>), String> {
    let encoded = encode_route_key(&route.template);
    let url_path = format!("/__paths__/{encoded}");

    let resp = host.dispatch_fetch(&url_path).map_err(|e| {
        format!(
            "embedded V8 dispatch for /__paths__/{} failed: {e}",
            route.template
        )
    })?;

    if !(200..300).contains(&resp.status) {
        let body = String::from_utf8_lossy(&resp.body).into_owned();
        return Err(format!(
            "worker returned {} for /__paths__/{}: {}",
            resp.status,
            route.template,
            body.trim()
        ));
    }

    let json: serde_json::Value = serde_json::from_slice(&resp.body).map_err(|e| {
        format!(
            "could not parse JSON from /__paths__/{}: {}",
            route.template, e
        )
    })?;

    resolve_json_paths(json, route, cache)
}

/// Shared path resolution from a parsed JSON value. Returns
/// `(universe_entries, manifest_entries)` so both the renderer
/// input and the postBuild manifest (#262) can be built from one call.
fn resolve_json_paths(
    json: serde_json::Value,
    route: &DeferredDynamicRoute,
    cache: &mut PathsCache,
) -> Result<(Vec<RouteUniverseEntry>, Vec<DynamicResolvedEntry>), String> {
    let segs: Vec<PathsSegment> = route.segments.to_vec();
    let resolved = resolve_paths(cache, &route.template, &segs, &json)
        .map_err(|e| format!("{}: {}", route.template, format_paths_error(&e)))?;
    Ok(expand_resolved_urls(
        &route.template,
        &route.segments,
        route.output_extension.as_deref(),
        &route.source_path,
        resolved,
    ))
}

// Keep the old `eval_one_deferred_path` name as a private alias so any
// existing direct callers inside this crate still compile.
// This shim is intentionally not pub — only `eval_deferred_paths_via_worker`
// is the public surface.
#[allow(dead_code)]
fn eval_one_deferred_path(
    client: &reqwest::blocking::Client,
    base: &str,
    route: &DeferredDynamicRoute,
    cache: &mut PathsCache,
) -> Result<(Vec<RouteUniverseEntry>, Vec<DynamicResolvedEntry>), String> {
    eval_one_deferred_path_http(client, base, route, cache)
}

/// Percent-encode a route key so it is safe as a URL path segment.
///
/// We encode every byte that is not an ASCII alphanumeric or one of
/// `- _ . ~` (the RFC 3986 unreserved set). In particular `/` and `:`
/// are encoded so they don't confuse the worker's path router.
fn encode_route_key(key: &str) -> String {
    let mut out = String::with_capacity(key.len() * 3);
    for byte in key.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(hex_nibble(byte >> 4));
            out.push(hex_nibble(byte & 0xF));
        }
    }
    out
}

fn hex_nibble(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        _ => (b'A' + n - 10) as char,
    }
}

/// Read every TSX page's frontmatter and fold the
/// `export const prerender = …` flag into a map keyed on the route
/// template (matching [`RouteUniverseEntry::route_key`]).
///
/// `warn_unreadable` is invoked once per TSX page whose source can't be
/// read or whose frontmatter extraction fails. The closure exists so
/// callers can route the message through their own logging surface
/// (e.g. `crate::output::warn`) instead of forcing this helper to
/// know about CLI-side I/O.
///
/// Behaviour notes:
///
/// - Non-TSX pages (e.g. `.mdx`) are skipped silently — TSX
///   frontmatter extraction does not apply to them. The renderer
///   treats a missing prerender entry as `true` (SSG), which is the
///   documented default for MDX too.
/// - TSX pages with **malformed** frontmatter (present but unparseable /
///   computed / wrong-shape) are skipped from the map (so the renderer's
///   missing-key default of `true` applies) and the failure is reported
///   through `warn_unreadable` so the user can fix the mistake instead of
///   staring at a silent default.
/// - TSX pages with **no** `export const frontmatter` are NOT a failure —
///   the export is optional and absence means "use the SSG default". These
///   are skipped silently (no warning). Warning on every frontmatter-less
///   page was misleading noise — see #505.
///
/// Returns `true` if the route identified by `template` is SSR
/// (`prerender = false` in its frontmatter).
///
/// The companion to [`build_prerender_map`]: a missing key in the map
/// means no `prerender` value was explicitly set, which defaults to SSG
/// (`true`). So a missing key is **not** SSR — the helper inverts the
/// lookup with that default folded in.
///
/// The same predicate was previously inlined at 4+ call sites in
/// `commands/build.rs` and `commands/dev.rs`; centralising the
/// "missing key → SSG default" rule here prevents drift if the default
/// ever changes (and made the post-#520 pre-filter, where this rule is
/// load-bearing, easier to read).
pub fn is_ssr_route(prerender_map: &BTreeMap<String, bool>, template: &str) -> bool {
    !prerender_map.get(template).copied().unwrap_or(true)
}

pub fn build_prerender_map(
    routes: &[Route],
    project_root: &Path,
    mut warn_unreadable: impl FnMut(&str),
) -> BTreeMap<String, bool> {
    let mut map = BTreeMap::new();
    for route in routes {
        let abs = if route.source_path.is_absolute() {
            route.source_path.clone()
        } else {
            project_root.join(&route.source_path)
        };
        let Some(ext) = abs.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        // Only TSX (and TS) carries the `export const prerender = …`
        // shape this extractor understands. MDX is left at the default.
        if !matches!(ext, "tsx" | "ts") {
            continue;
        }
        let source = match std::fs::read_to_string(&abs) {
            Ok(s) => s,
            Err(e) => {
                warn_unreadable(&format!(
                    "could not read {} for prerender extraction ({}); defaulting to SSG",
                    abs.display(),
                    e
                ));
                continue;
            }
        };
        let file_name = abs
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "<unknown>".into());
        match extract_tsx_frontmatter(&source, &file_name) {
            Ok(fm) => {
                map.insert(route.template(), fm.prerender);
            }
            // A route page with no `export const frontmatter` is valid — the
            // export is optional and an absent one simply means "use the SSG
            // default". Warning on it is misleading (it reads as "your page is
            // broken") and fires on every frontmatter-less page, so stay silent
            // and let the missing-key default of `true` (SSG) apply. See #505.
            Err(TsxFrontmatterError::MissingFrontmatter { .. }) => {}
            // Any other extraction error means the frontmatter IS present but
            // malformed (parse error, duplicate/computed/wrong-shape export).
            // That's a real mistake worth surfacing.
            Err(e) => {
                warn_unreadable(&format!(
                    "frontmatter extraction failed for {} ({}); defaulting to SSG",
                    abs.display(),
                    e
                ));
            }
        }
    }
    map
}

/// Verify that `@takazudo/zfb-runtime` is resolvable for the current build.
///
/// Resolution order:
/// 1. **Embedded vendor snapshot** — `EMBEDDED_VENDOR` (the
///    `include_dir!`-baked tree populated by `build.rs`) ships
///    `@takazudo/zfb-runtime/package.json`. When that file is present in the
///    snapshot, the build flow's `embedded_node_modules()` fallback in
///    `commands::build` will materialise it into a tempdir at bundle time, so
///    the pre-check passes without requiring an on-disk `node_modules`.
/// 2. **Binary-adjacent path** — `<dir of current executable>/node_modules/@takazudo/zfb-runtime`.
///    A `cargo install`-ed `zfb` binary may ship a vendored `node_modules/` tree
///    next to it; checked next so layouts that explicitly stage a sibling
///    tree still pass.
/// 3. **Ancestor `node_modules` walk** starting at `project_root` — the
///    conventional pnpm/npm layout used during dev / local-checkout flows.
///
/// Returns `Ok(())` when the runtime is found via any path.
/// Returns `Err` with an actionable hint when none resolves.
pub fn check_runtime_installed(project_root: &Path) -> Result<()> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    check_runtime_installed_with_exe_dir(project_root, exe_dir.as_deref())
}

/// Inner implementation for [`check_runtime_installed`], accepting an optional
/// `exe_dir` override so tests can supply a synthetic binary directory without
/// depending on the test binary's actual location.
///
/// Defaults `embedded_has_runtime` from the compile-time `EMBEDDED_VENDOR`
/// snapshot. Tests that need to simulate a build without the embedded vendor
/// (e.g. to exercise the legacy on-disk resolution paths in isolation) call
/// [`check_runtime_installed_with_overrides`] directly.
pub(crate) fn check_runtime_installed_with_exe_dir(
    project_root: &Path,
    exe_dir: Option<&Path>,
) -> Result<()> {
    let embedded_has_runtime = EMBEDDED_VENDOR
        .get_file("@takazudo/zfb-runtime/package.json")
        .is_some();
    check_runtime_installed_with_overrides(project_root, exe_dir, embedded_has_runtime)
}

/// Resolution-logic core for [`check_runtime_installed`]. Splits out the
/// embedded-vendor probe behind a boolean so tests can drive both the
/// "embedded snapshot ships the runtime" and the legacy "on-disk only" paths
/// without relying on the test binary's compile-time vendor contents.
pub(crate) fn check_runtime_installed_with_overrides(
    project_root: &Path,
    exe_dir: Option<&Path>,
    embedded_has_runtime: bool,
) -> Result<()> {
    // 1. Embedded vendor snapshot — this binary already carries the runtime
    //    (sub #198) and `commands::build` extracts it on demand when no
    //    on-disk node_modules is found. Accept this case so the pre-check is
    //    not strictly more restrictive than the actual build flow.
    if embedded_has_runtime {
        return Ok(());
    }

    // 2. Binary-adjacent path (cargo-installed binary with sibling
    //    node_modules tree, or test fixtures).
    if let Some(dir) = exe_dir {
        if dir
            .join("node_modules")
            .join("@takazudo")
            .join("zfb-runtime")
            .exists()
        {
            return Ok(());
        }
    }

    // 3. Ancestor node_modules walk (dev / local-checkout scenario).
    let mut cur: Option<&Path> = Some(project_root);
    while let Some(p) = cur {
        if p.join("node_modules")
            .join("@takazudo")
            .join("zfb-runtime")
            .exists()
        {
            return Ok(());
        }
        cur = p.parent();
    }

    // No path found — emit an actionable error.
    Err(anyhow::anyhow!(
        "could not find `node_modules/@takazudo/zfb-runtime` under {} or any parent, \
         no binary-adjacent `node_modules/@takazudo/zfb-runtime` was found next to the `zfb` executable, \
         and the binary's embedded vendor snapshot does not contain it either. \
         Either run `pnpm install` (or your package manager's equivalent) in the project root, \
         or install `zfb` via `cargo install` (which bundles the runtime automatically).",
        project_root.display()
    ))
    .context("zfb runtime resolution check failed")
}

/// Convert the project's [`crate::config::Framework`] into the
/// renderer/bundler-facing [`zfb_render::adapters::Framework`].
pub fn cfg_framework_to_render(f: crate::config::Framework) -> zfb_render::adapters::Framework {
    match f {
        crate::config::Framework::Preact => zfb_render::adapters::Framework::Preact,
        crate::config::Framework::React => zfb_render::adapters::Framework::React,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;
    use zfb_router::{Route, RouteKind, Segment};

    fn static_route(segments: Vec<&str>, source: &str) -> Route {
        let segs: Vec<Segment> = segments
            .into_iter()
            .map(|s| Segment::Static(s.to_string()))
            .collect();
        Route {
            source_path: PathBuf::from(source),
            segments: segs,
            kind: RouteKind::Static,
            specificity: 0,
            output_extension: None,
            static_html: false,
        }
    }

    fn dynamic_route(name: &str, source: &str) -> Route {
        Route {
            source_path: PathBuf::from(source),
            segments: vec![Segment::Dynamic(name.to_string())],
            kind: RouteKind::Dynamic,
            specificity: 0,
            output_extension: None,
            static_html: false,
        }
    }

    /// Build a multi-segment route mixing static + dynamic segments,
    /// e.g. `route(["blog", ":slug"], "pages/blog/[slug].tsx")` builds
    /// `/blog/[slug]`.
    fn route(segments: Vec<&str>, source: &str, kind: RouteKind) -> Route {
        let segs: Vec<Segment> = segments
            .into_iter()
            .map(|s| {
                if let Some(name) = s.strip_prefix(":") {
                    if let Some(name) = name.strip_suffix("*") {
                        Segment::Catchall(name.to_string())
                    } else {
                        Segment::Dynamic(name.to_string())
                    }
                } else {
                    Segment::Static(s.to_string())
                }
            })
            .collect();
        Route {
            source_path: PathBuf::from(source),
            segments: segs,
            kind,
            specificity: 0,
            output_extension: None,
            static_html: false,
        }
    }

    #[test]
    fn build_route_universe_partitions_static_and_dynamic() {
        let routes = vec![
            static_route(vec![], "pages/index.tsx"),
            static_route(vec!["about"], "pages/about.tsx"),
            dynamic_route("slug", "pages/[slug].tsx"),
        ];
        let plan = build_route_universe(&routes);
        assert_eq!(plan.static_routes.len(), 2);
        assert_eq!(plan.static_routes[0].url_path, "/");
        assert_eq!(
            plan.static_routes[0].output_path,
            PathBuf::from("index.html")
        );
        assert_eq!(plan.static_routes[0].route_key, "/");
        assert_eq!(plan.static_routes[1].url_path, "/about");
        assert_eq!(
            plan.static_routes[1].output_path,
            PathBuf::from("about/index.html")
        );

        assert_eq!(plan.deferred_dynamic.len(), 1);
        assert_eq!(plan.deferred_dynamic[0].template, "/:slug");
        assert_eq!(
            plan.deferred_dynamic[0].source_path,
            PathBuf::from("pages/[slug].tsx")
        );
    }

    #[test]
    fn build_prerender_map_warns_on_malformed_but_not_on_absent_frontmatter() {
        let dir = tempdir().unwrap();
        let pages = dir.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();

        // SSG (default true)
        std::fs::write(
            pages.join("about.tsx"),
            "export const frontmatter = { title: 'A' };\nexport default function() { return null; }\n",
        )
        .unwrap();
        // SSR-only (prerender=false)
        std::fs::write(
            pages.join("preview.tsx"),
            "export const frontmatter = { title: 'P' };\nexport const prerender = false;\nexport default function() { return null; }\n",
        )
        .unwrap();
        // No frontmatter at all — this is VALID (the export is optional). It
        // must NOT warn and simply defaults to SSG via the missing-key path. (#505)
        std::fs::write(
            pages.join("nofm.tsx"),
            "export default function() { return null; }\n",
        )
        .unwrap();
        // Frontmatter IS present but malformed (non-literal/computed value) —
        // a real mistake that should still surface a warning.
        std::fs::write(
            pages.join("malformed.tsx"),
            "const titleVar = 'x';\nexport const frontmatter = { title: titleVar };\nexport default function() { return null; }\n",
        )
        .unwrap();

        let routes = vec![
            static_route(vec!["about"], "pages/about.tsx"),
            static_route(vec!["preview"], "pages/preview.tsx"),
            static_route(vec!["nofm"], "pages/nofm.tsx"),
            static_route(vec!["malformed"], "pages/malformed.tsx"),
        ];

        let mut warnings: Vec<String> = Vec::new();
        let map = build_prerender_map(&routes, dir.path(), |msg| warnings.push(msg.to_string()));
        assert_eq!(map.get("/about"), Some(&true));
        assert_eq!(map.get("/preview"), Some(&false));
        // Absent frontmatter: no entry (default SSG), and crucially no warning.
        assert!(!map.contains_key("/nofm"));
        // Malformed frontmatter: no entry, and exactly one warning naming it.
        assert!(!map.contains_key("/malformed"));
        assert_eq!(
            warnings.len(),
            1,
            "only the malformed page should warn; got: {warnings:?}"
        );
        assert!(
            warnings[0].contains("malformed.tsx"),
            "expected malformed.tsx in warning, got: {}",
            warnings[0]
        );
    }

    #[test]
    fn build_prerender_map_skips_non_tsx_sources() {
        let dir = tempdir().unwrap();
        let pages = dir.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();
        std::fs::write(pages.join("post.mdx"), "# hello\n").unwrap();

        let routes = vec![static_route(vec!["post"], "pages/post.mdx")];
        let mut warnings: Vec<String> = Vec::new();
        let map = build_prerender_map(&routes, dir.path(), |msg| warnings.push(msg.into()));
        assert!(
            map.is_empty(),
            "MDX should be left to the default-true path"
        );
        // Non-TSX files are skipped silently (no warning).
        assert!(warnings.is_empty(), "warnings: {warnings:?}");
    }

    /// Embedded vendor short-circuits resolution: when the `cargo install`-ed
    /// binary's `EMBEDDED_VENDOR` ships `@takazudo/zfb-runtime`, no on-disk
    /// `node_modules` is needed. Exercises the path added so consumer
    /// projects (e.g. ccresdoc post-zfb#212) can build with no node_modules
    /// and no env-var prefixes.
    #[test]
    fn check_runtime_installed_succeeds_via_embedded_vendor() {
        let project_root = tempdir().unwrap();
        let fake_exe_dir = tempdir().unwrap(); // no node_modules inside
        check_runtime_installed_with_overrides(
            project_root.path(),
            Some(fake_exe_dir.path()),
            true, // embedded vendor has the runtime
        )
        .unwrap();
    }

    #[test]
    fn check_runtime_installed_finds_runtime_in_parent_node_modules() {
        let dir = tempdir().unwrap();
        let runtime = dir
            .path()
            .join("node_modules")
            .join("@takazudo")
            .join("zfb-runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        // Arbitrary nested path; simulates a project that lives inside a parent dir.
        let nested = dir.path().join("projects/my-site");
        std::fs::create_dir_all(&nested).unwrap();
        // Force `embedded_has_runtime=false` so this test exercises the
        // ancestor-walk path in isolation, regardless of what the test
        // binary's compile-time vendor contains.
        check_runtime_installed_with_overrides(&nested, None, false).unwrap();
    }

    #[test]
    fn check_runtime_installed_errors_when_runtime_missing() {
        let dir = tempdir().unwrap();
        // Drive the legacy on-disk-only branch so the error path remains
        // exercised even though the production `check_runtime_installed`
        // now short-circuits via the embedded vendor.
        let err = check_runtime_installed_with_overrides(dir.path(), None, false).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("@takazudo/zfb-runtime"), "{msg}");
        assert!(msg.contains("pnpm install"), "{msg}");
    }

    /// Binary-adjacent path: runtime found next to a synthetic executable dir.
    ///
    /// Mirrors `check_runtime_installed_finds_runtime_in_parent_node_modules`
    /// but exercises the exe-adjacent resolution path introduced by Sub 198.
    #[test]
    fn check_runtime_installed_finds_runtime_adjacent_to_binary() {
        let dir = tempdir().unwrap();
        // Create the binary-adjacent node_modules layout.
        let runtime = dir
            .path()
            .join("node_modules")
            .join("@takazudo")
            .join("zfb-runtime");
        std::fs::create_dir_all(&runtime).unwrap();

        // project_root has no node_modules — only the exe_dir does.
        let project_root = tempdir().unwrap();
        // Pin `embedded_has_runtime=false` so this test isolates the
        // exe-adjacent branch without dependence on the embedded snapshot.
        check_runtime_installed_with_overrides(project_root.path(), Some(dir.path()), false)
            .unwrap();
    }

    /// All paths missing → error that mentions every remedy.
    #[test]
    fn check_runtime_installed_errors_with_all_paths_missing() {
        let project_root = tempdir().unwrap();
        let fake_exe_dir = tempdir().unwrap(); // no node_modules inside
        let err = check_runtime_installed_with_overrides(
            project_root.path(),
            Some(fake_exe_dir.path()),
            false,
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("@takazudo/zfb-runtime"), "{msg}");
        assert!(msg.contains("pnpm install"), "{msg}");
        assert!(msg.contains("cargo install"), "{msg}");
        assert!(msg.contains("embedded vendor snapshot"), "{msg}");
    }

    /// Smoke-test that [`embedded_node_modules`] extracts a proper
    /// `node_modules/@takazudo/zfb/package.json`,
    /// `node_modules/@takazudo/zfb-runtime/package.json`, and the framework
    /// runtime packages (`preact`, `preact-render-to-string`, `hono`) layout
    /// that `check_runtime_installed_with_exe_dir` (and esbuild) can resolve.
    #[test]
    fn embedded_node_modules_extracts_runtime_layout() {
        let (handle, nm_path) =
            embedded_node_modules().expect("embedded_node_modules should not fail");

        // @takazudo/* package root markers must exist.
        assert!(
            nm_path.join("@takazudo/zfb/package.json").exists(),
            "missing @takazudo/zfb/package.json in extracted layout"
        );
        assert!(
            nm_path.join("@takazudo/zfb-runtime/package.json").exists(),
            "missing @takazudo/zfb-runtime/package.json in extracted layout"
        );

        // @takazudo/* entry-point source files must be present.
        assert!(
            nm_path.join("@takazudo/zfb/src/index.ts").exists(),
            "missing @takazudo/zfb/src/index.ts"
        );
        assert!(
            nm_path.join("@takazudo/zfb-runtime/src/index.ts").exists(),
            "missing @takazudo/zfb-runtime/src/index.ts"
        );

        // Sub #209 — framework runtime package roots must exist alongside.
        for pkg in ["preact", "preact-render-to-string", "hono"] {
            let pkg_json = nm_path.join(pkg).join("package.json");
            assert!(
                pkg_json.exists(),
                "missing {pkg}/package.json in extracted layout: {}",
                pkg_json.display()
            );
        }

        // Verify check_runtime_installed sees the embedded runtime via the
        // exe-dir path (the nm_path is the node_modules dir; its parent is
        // the synthetic "binary dir" we pass as exe_dir).
        let exe_dir = nm_path.parent().expect("nm_path should have a parent");
        let project_root = tempdir().unwrap();
        check_runtime_installed_with_exe_dir(project_root.path(), Some(exe_dir))
            .expect("runtime should resolve via embedded nm_path");

        drop(handle); // explicit: tempdir is removed here
    }

    /// Smoke-test [`embedded_binary`] for the `esbuild` slot:
    ///
    /// 1. The path returned exists on disk.
    /// 2. On Unix the executable bit is set so a subprocess can spawn it.
    /// 3. The file is non-empty (we don't shell out — that's the
    ///    `#[ignore]`-gated integration test's job).
    ///
    /// Mirrors `embedded_node_modules_extracts_runtime_layout` for the
    /// `bin/` half of the embedded vendor tree (sub #212).
    #[test]
    fn embedded_binary_extracts_executable_esbuild_path() {
        let (handle, path) =
            embedded_binary("esbuild").expect("embedded_binary(\"esbuild\") should succeed");
        assert!(
            path.exists(),
            "extracted esbuild path should exist: {}",
            path.display()
        );
        let meta = std::fs::metadata(&path).expect("metadata should be readable");
        assert!(
            meta.len() > 0,
            "extracted esbuild binary should not be empty: {}",
            path.display()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = meta.permissions().mode();
            assert!(
                mode & 0o111 != 0,
                "extracted esbuild binary should be executable: mode = {mode:o}"
            );
        }
        drop(handle);
    }

    /// `embedded_binary` returns a clear error when the requested name is
    /// not present in `EMBEDDED_VENDOR/bin/`. The error message must point
    /// the operator at the build-script staging step.
    #[test]
    fn embedded_binary_errors_with_actionable_hint_when_missing() {
        let err = embedded_binary("does-not-exist-xyz").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("does-not-exist-xyz"),
            "msg should name the requested binary: {msg}"
        );
        assert!(
            msg.contains("stage_binaries_into_vendor"),
            "msg should point at the build-script staging step: {msg}"
        );
    }

    // ---- expand_dynamic_routes -------------------------------------------

    /// Stage a single dynamic page on disk with the given source so
    /// [`expand_dynamic_routes`] can read it. Returns the project root
    /// (caller keeps the [`tempfile::TempDir`] alive) and the
    /// [`PendingDynamicRoute`] pointing at the staged page.
    fn stage_dynamic_page(
        page_relative: &str,
        segments: Vec<Segment>,
        template: &str,
        body: &str,
    ) -> (tempfile::TempDir, PendingDynamicRoute) {
        let dir = tempdir().unwrap();
        let abs = dir.path().join(page_relative);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, body).unwrap();
        let pending = PendingDynamicRoute {
            source_path: PathBuf::from(page_relative),
            template: template.to_string(),
            segments,
            output_extension: None,
        };
        (dir, pending)
    }

    #[test]
    fn expand_dynamic_routes_resolves_literal_paths_into_entries() {
        let body = r#"
            export function paths() {
                return [
                    { params: { slug: "hello" }, props: { i: 1 } },
                    { params: { slug: "world" }, props: { i: 2 } },
                ];
            }
            export default function P() { return null; }
        "#;
        let (dir, pending) = stage_dynamic_page(
            "pages/blog/[slug].tsx",
            vec![
                Segment::Static("blog".into()),
                Segment::Dynamic("slug".into()),
            ],
            "/blog/:slug",
            body,
        );

        let mut cache = PathsCache::new();
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache)
            .expect("expansion should succeed for a literal paths()");

        assert_eq!(out.deferred.len(), 0, "deferred: {:?}", out.deferred);
        assert_eq!(out.resolved.len(), 2);

        assert_eq!(out.resolved[0].url_path, "/blog/hello");
        assert_eq!(
            out.resolved[0].output_path,
            PathBuf::from("blog/hello/index.html")
        );
        assert_eq!(out.resolved[0].route_key, "/blog/:slug");

        assert_eq!(out.resolved[1].url_path, "/blog/world");
        assert_eq!(
            out.resolved[1].output_path,
            PathBuf::from("blog/world/index.html")
        );
        assert_eq!(out.resolved[1].route_key, "/blog/:slug");

        // Cache miss for the first call — the second route in the same
        // call is a different `slug` value but shares the same JSON
        // shape (single resolve_paths call), so we still expect exactly
        // one miss for this page.
        assert_eq!(cache.miss_count(), 1);
        assert_eq!(cache.hit_count(), 0);
    }

    /// Issue #974 — PathsCache hit/miss counters guard the once-per-route
    /// extraction contract for a route with many entries.
    ///
    /// Expanding a route with N entries calls `resolve_paths` once
    /// (one cache miss). Re-expanding the same route with the identical
    /// paths() payload hits the cache (one cache hit). This pins the
    /// Rust-side once-per-route invariant that the JS-side memo parallels.
    #[test]
    fn expand_dynamic_routes_paths_cache_once_per_route_many_entries() {
        // Build a paths() that returns 10 entries — enough to confirm the
        // counter isn't incremented per-entry.
        let entries: String = (0..10)
            .map(|i| format!("{{ params: {{ slug: \"post-{i}\" }} }}"))
            .collect::<Vec<_>>()
            .join(", ");
        let body = format!("export function paths() {{ return [{entries}]; }}");
        let (dir, pending) = stage_dynamic_page(
            "pages/blog/[slug].tsx",
            vec![
                Segment::Static("blog".into()),
                Segment::Dynamic("slug".into()),
            ],
            "/blog/:slug",
            &body,
        );

        let mut cache = PathsCache::new();

        // First expansion: one cache miss, no hits.
        let out = expand_dynamic_routes(std::slice::from_ref(&pending), dir.path(), &mut cache)
            .expect("expansion should succeed");
        assert_eq!(out.resolved.len(), 10);
        assert_eq!(cache.miss_count(), 1, "first expand: expected 1 miss");
        assert_eq!(cache.hit_count(), 0, "first expand: expected 0 hits");

        // Second expansion with the same route/payload: one cache hit.
        let out2 = expand_dynamic_routes(&[pending], dir.path(), &mut cache)
            .expect("second expansion should succeed");
        assert_eq!(out2.resolved.len(), 10);
        assert_eq!(cache.miss_count(), 1, "second expand: miss_count unchanged");
        assert_eq!(cache.hit_count(), 1, "second expand: expected 1 hit");
    }

    #[test]
    fn expand_dynamic_routes_respects_output_extension_for_non_html() {
        // `[slug].xml.tsx` should resolve to `<slug>.xml`, not
        // `<slug>/index.html`. The router would have set
        // `output_extension = Some("xml")`; we mirror that here.
        let body = r#"
            export function paths() {
                return [{ params: { slug: "feed-a" } }];
            }
        "#;
        let (dir, mut pending) = stage_dynamic_page(
            "pages/[slug].xml.tsx",
            vec![Segment::Dynamic("slug".into())],
            "/:slug",
            body,
        );
        pending.output_extension = Some("xml".into());

        let mut cache = PathsCache::new();
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache)
            .expect("expansion should succeed for a literal paths()");

        assert_eq!(out.resolved.len(), 1);
        assert_eq!(out.resolved[0].url_path, "/feed-a");
        assert_eq!(out.resolved[0].output_path, PathBuf::from("feed-a"));
    }

    #[test]
    fn expand_dynamic_routes_defers_non_literal_paths_with_reason() {
        // Mirrors the bundled basic-blog template page: `paths()` does an
        // `await import` + collection query, which is not statically
        // resolvable. Must defer with a reason that the build can
        // surface verbatim.
        let body = r#"
            export async function paths() {
                const { getCollection } = await import("zfb/content");
                const posts = await getCollection("blog");
                return posts.map((p) => ({ params: { slug: p.slug } }));
            }
        "#;
        let (dir, pending) = stage_dynamic_page(
            "pages/blog/[slug].tsx",
            vec![
                Segment::Static("blog".into()),
                Segment::Dynamic("slug".into()),
            ],
            "/blog/:slug",
            body,
        );

        let mut cache = PathsCache::new();
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache)
            .expect("non-literal paths() should defer, not hard-error");

        assert_eq!(out.resolved.len(), 0);
        assert_eq!(out.deferred.len(), 1);
        assert!(
            out.deferred[0].reason.contains("not statically resolvable"),
            "reason: {}",
            out.deferred[0].reason,
        );
        assert!(
            out.deferred[0].reason.contains("pages/blog/[slug].tsx"),
            "reason should name the source path, got: {}",
            out.deferred[0].reason,
        );
        assert_eq!(out.deferred[0].template, "/blog/:slug");
    }

    /// A missing `paths()` export on an SSG dynamic route is now a hard
    /// build error (issue #520). SSR routes must be pre-filtered by the
    /// caller before reaching `expand_dynamic_routes` — this function only
    /// ever sees SSG routes and a missing `paths()` there means the page
    /// would produce zero concrete URLs.
    #[test]
    fn expand_dynamic_routes_errors_when_paths_export_missing() {
        let body = r#"
            export default function P() { return null; }
        "#;
        let (dir, pending) = stage_dynamic_page(
            "pages/[slug].tsx",
            vec![Segment::Dynamic("slug".into())],
            "/:slug",
            body,
        );

        let mut cache = PathsCache::new();
        let err = expand_dynamic_routes(&[pending], dir.path(), &mut cache)
            .expect_err("missing paths() on SSG route should be a hard error");
        let msg = format!("{err}");
        assert!(
            msg.contains("no top-level `paths` export"),
            "error message should mention missing paths export, got: {msg}",
        );
        assert!(
            msg.contains("add an exported `paths()` function"),
            "error message should guide the user, got: {msg}",
        );
    }

    #[test]
    fn expand_dynamic_routes_defers_unreadable_source() {
        // Point at a file that doesn't exist; should defer with an
        // I/O-flavoured reason rather than panic.
        let dir = tempdir().unwrap();
        let pending = PendingDynamicRoute {
            source_path: PathBuf::from("pages/no-such.tsx"),
            template: "/no-such".into(),
            segments: vec![Segment::Dynamic("x".into())],
            output_extension: None,
        };
        let mut cache = PathsCache::new();
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache)
            .expect("unreadable source should defer, not hard-error");
        assert_eq!(out.resolved.len(), 0);
        assert_eq!(out.deferred.len(), 1);
        assert!(
            out.deferred[0].reason.contains("could not read"),
            "reason: {}",
            out.deferred[0].reason,
        );
    }

    #[test]
    fn expand_dynamic_routes_defers_with_resolver_error_when_param_missing() {
        // Literal extraction succeeds — but the entry's `params` is
        // missing the required `slug` key. The resolver returns
        // MissingParam; we surface that as a deferred entry with the
        // user-facing reason.
        let body = r#"
            export function paths() {
                return [{ params: { wrong: "x" } }];
            }
        "#;
        let (dir, pending) = stage_dynamic_page(
            "pages/[slug].tsx",
            vec![Segment::Dynamic("slug".into())],
            "/:slug",
            body,
        );
        let mut cache = PathsCache::new();
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache)
            .expect("resolver error (wrong param name) should defer, not hard-error");
        assert_eq!(out.resolved.len(), 0);
        assert_eq!(out.deferred.len(), 1);
        assert!(
            out.deferred[0]
                .reason
                .contains("missing required param `slug`"),
            "reason: {}",
            out.deferred[0].reason,
        );
    }

    #[test]
    fn expand_dynamic_routes_handles_catchall_with_array_value() {
        let body = r#"
            export function paths() {
                return [
                    { params: { slug: ["a", "b"] } },
                    { params: { slug: ["x", "y", "z"] } },
                ];
            }
        "#;
        let (dir, pending) = stage_dynamic_page(
            "pages/docs/[...slug].tsx",
            vec![
                Segment::Static("docs".into()),
                Segment::Catchall("slug".into()),
            ],
            "/docs/:slug{.+}",
            body,
        );
        let mut cache = PathsCache::new();
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache)
            .expect("catchall with literal paths() should succeed");

        assert_eq!(out.deferred.len(), 0, "deferred: {:?}", out.deferred);
        assert_eq!(out.resolved.len(), 2);
        assert_eq!(out.resolved[0].url_path, "/docs/a/b");
        assert_eq!(
            out.resolved[0].output_path,
            PathBuf::from("docs/a/b/index.html")
        );
        assert_eq!(out.resolved[1].url_path, "/docs/x/y/z");
        assert_eq!(
            out.resolved[1].output_path,
            PathBuf::from("docs/x/y/z/index.html")
        );
    }

    #[test]
    fn expand_dynamic_routes_handles_optional_catchall_zero_segments() {
        // `[[...slug]]` with `slug: []` must produce the bare directory
        // URL (`/docs`, no trailing slash — matching the existing
        // url_path style) and `docs/index.html`, and the manifest must
        // report `slug: []` (an empty array, not `[""]`). #812.
        let body = r#"
            export function paths() {
                return [
                    { params: { slug: [] } },
                    { params: { slug: ["guides", "install"] } },
                ];
            }
        "#;
        let (dir, pending) = stage_dynamic_page(
            "pages/docs/[[...slug]].tsx",
            vec![
                Segment::Static("docs".into()),
                Segment::OptionalCatchall("slug".into()),
            ],
            "/docs/:slug{.+}?",
            body,
        );
        let mut cache = PathsCache::new();
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache)
            .expect("optional catchall with literal paths() should succeed");

        assert_eq!(out.deferred.len(), 0, "deferred: {:?}", out.deferred);
        assert_eq!(out.resolved.len(), 2);

        // Zero-segment entry: bare URL + directory-index output.
        assert_eq!(out.resolved[0].url_path, "/docs");
        assert_eq!(
            out.resolved[0].output_path,
            PathBuf::from("docs/index.html"),
        );
        assert_eq!(out.resolved[0].route_key, "/docs/:slug{.+}?");

        // Nested entry unchanged.
        assert_eq!(out.resolved[1].url_path, "/docs/guides/install");
        assert_eq!(
            out.resolved[1].output_path,
            PathBuf::from("docs/guides/install/index.html"),
        );

        // Manifest params: the zero case is an EMPTY array, never `[""]`.
        assert_eq!(out.resolved_with_params.len(), 2);
        assert_eq!(
            out.resolved_with_params[0].params.arrays.get("slug"),
            Some(&Vec::<String>::new()),
            "zero-segment optional catchall must surface `slug: []`",
        );
        assert_eq!(
            out.resolved_with_params[1].params.arrays.get("slug"),
            Some(&vec!["guides".to_string(), "install".to_string()]),
        );
    }

    /// Integration-style fixture: stage a tiny project with both a
    /// static page and a dynamic page that has a literal `paths()`,
    /// then walk through `build_route_universe` →
    /// `expand_dynamic_routes` and assert the combined renderer-shaped
    /// route list. This is the closest we can get to end-to-end without
    /// booting the embedded V8 host (which is gated by the sibling
    /// worker-entry topic).
    #[test]
    fn build_then_expand_combined_route_universe() {
        let dir = tempdir().unwrap();
        // Static page.
        std::fs::create_dir_all(dir.path().join("pages")).unwrap();
        std::fs::write(
            dir.path().join("pages/about.tsx"),
            "export const frontmatter = { title: \"About\" };\n\
             export default function P() { return null; }\n",
        )
        .unwrap();
        // Dynamic page with a literal paths().
        std::fs::create_dir_all(dir.path().join("pages/blog")).unwrap();
        std::fs::write(
            dir.path().join("pages/blog/[slug].tsx"),
            "export function paths() {\n\
                return [\n\
                    { params: { slug: \"hello\" } },\n\
                    { params: { slug: \"world\" } }\n\
                ];\n\
             }\n\
             export default function P() { return null; }\n",
        )
        .unwrap();

        let routes = vec![
            static_route(vec!["about"], "pages/about.tsx"),
            route(
                vec!["blog", ":slug"],
                "pages/blog/[slug].tsx",
                RouteKind::Dynamic,
            ),
        ];

        let plan = build_route_universe(&routes);
        assert_eq!(plan.static_routes.len(), 1);
        assert_eq!(plan.deferred_dynamic.len(), 1);

        let mut cache = PathsCache::new();
        let expansion = expand_dynamic_routes(&plan.deferred_dynamic, dir.path(), &mut cache)
            .expect("combined route universe expansion should succeed");
        assert_eq!(expansion.deferred.len(), 0, "{:?}", expansion.deferred);
        assert_eq!(expansion.resolved.len(), 2);

        // Final renderer-shaped list: statics first, then resolved
        // dynamics in input order. The build orchestration concatenates
        // in this same order; assert the combined shape end-to-end.
        let mut combined = plan.static_routes.clone();
        combined.extend(expansion.resolved);
        assert_eq!(combined.len(), 3);
        assert_eq!(combined[0].url_path, "/about");
        assert_eq!(combined[1].url_path, "/blog/hello");
        assert_eq!(combined[2].url_path, "/blog/world");
        // route_key of resolved entries must be the dynamic template,
        // not the resolved URL — that's how the prerender map joins.
        assert_eq!(combined[1].route_key, "/blog/:slug");
        assert_eq!(combined[2].route_key, "/blog/:slug");
    }

    // -------------------------------------------------------------------------
    // WorkerDispatch::EmbeddedV8 integration test
    //
    // This test exercises a page whose `paths()` export is non-literal (reads
    // a content collection at runtime). It is gated with `#[ignore]` because
    // it depends on `EmbeddedV8RenderHost` (Sub 2 — embed-v8/sub-162) which
    // is not yet merged into this worktree. Run after Sub 2 is merged with:
    //
    //     cargo test -p zfb -- \
    //         --include-ignored \
    //         eval_deferred_paths_via_worker_embedded_v8_non_literal_paths
    //
    // The test proves that the `__paths__` synthetic endpoint in the bundle
    // keeps working unchanged: only the dispatch mechanism differs (no HTTP).
    // -------------------------------------------------------------------------

    /// Integration test: `eval_deferred_paths_via_worker` drives the embedded
    /// V8 host's `dispatch_fetch` for a page whose `paths()` is non-literal.
    ///
    /// **Cannot run until Sub 2 (`EmbeddedV8RenderHost`) is merged.**
    /// Gated with `#[ignore]` — see module-level comment above.
    #[test]
    #[ignore = "depends on EmbeddedV8RenderHost (Sub 2, embed-v8/sub-162); run after merge"]
    fn eval_deferred_paths_via_worker_embedded_v8_non_literal_paths() {
        // This test intentionally left as a skeleton to be filled in by the
        // integration manager after Sub 2 is merged. The shape is:
        //
        //   1. Build a basic-blog bundle from a fixture or the standalone demo
        //      (https://github.com/Takazudo/zfb-example-blog).
        //   2. Construct EmbeddedV8RenderHost::new(&bundle_path).
        //   3. Build a DeferredDynamicRoute for `/blog/:slug` (non-literal
        //      paths() that reads the blog content collection).
        //   4. Call eval_deferred_paths_via_worker with
        //      WorkerDispatch::EmbeddedV8 { host: &mut host }.
        //   5. Assert the resolved entries match the blog posts in the
        //      fixture content collection.
        //
        // Skeleton test — fill in after EmbeddedV8RenderHost (Sub 2) is merged.
        // (assert!(true) removed; test body will be added in a follow-up.)
    }
}
