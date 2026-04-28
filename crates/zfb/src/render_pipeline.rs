//! Shared helpers that wire the bundler and renderer
//! (`zfb_build::bundler` + `zfb_build::renderer`) into the `zfb build`
//! and `zfb dev` commands.
//!
//! This module deliberately does **not** spawn miniflare or call
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
//!    will fail at miniflare spawn time with a workerd-side error
//!    message that names the missing `default` export. The CLI
//!    surfaces that error verbatim instead of swallowing it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use zfb_build::renderer::RouteUniverseEntry;
use zfb_content::extract_tsx_frontmatter;
use zfb_render::paths::{
    resolve_paths, PathsCache, PathsError, Segment as PathsSegment,
};
use zfb_render::paths_extract::{extract_paths, PathsExtractError, PathsExtraction};
use zfb_router::{Route, RouteKind, Segment};

/// A dynamic / catchall route surfaced by [`build_route_universe`].
///
/// Carries enough metadata for [`expand_dynamic_routes`] to:
///
/// 1. Read the source file and try static `paths()` extraction.
/// 2. Convert the parsed segments to
///    [`zfb_render::paths::Segment`] so [`zfb_render::paths::resolve_paths`]
///    can reassemble the URLs.
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
pub fn expand_dynamic_routes(
    deferred: &[PendingDynamicRoute],
    project_root: &Path,
    cache: &mut PathsCache,
) -> DynamicExpansion {
    let mut out = DynamicExpansion::default();
    for route in deferred {
        match try_expand_one(route, project_root, cache) {
            Ok(entries) => out.resolved.extend(entries),
            Err(reason) => out.deferred.push(DeferredDynamicRoute {
                source_path: route.source_path.clone(),
                template: route.template.clone(),
                reason,
            }),
        }
    }
    out
}

/// Try to expand a single dynamic route into concrete entries. Returns
/// the resolved entries on success, or a one-line reason string on
/// failure (suitable for direct inclusion in a build warning).
fn try_expand_one(
    route: &PendingDynamicRoute,
    project_root: &Path,
    cache: &mut PathsCache,
) -> Result<Vec<RouteUniverseEntry>, String> {
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
        .map_err(|e| format!("could not read {} ({e})", abs.display()))?;
    let extraction = match extract_paths(&source, &file_name) {
        Ok(x) => x,
        Err(PathsExtractError::Parse { file, message }) => {
            return Err(format!("parse error in {file}: {message}"));
        }
    };
    let json = match extraction {
        PathsExtraction::Literal(v) => v,
        PathsExtraction::Missing => {
            return Err(format!(
                "no top-level `paths` export found in {}; dynamic routes require one",
                abs.display()
            ));
        }
        PathsExtraction::NonLiteral { reason } => {
            return Err(format!(
                "{}: paths() not statically resolvable ({reason}); pending runtime evaluation",
                abs.display()
            ));
        }
    };
    let segs: Vec<PathsSegment> = route
        .segments
        .iter()
        .map(router_segment_to_paths_segment)
        .collect();
    let resolved = resolve_paths(cache, &route.template, &segs, &json)
        .map_err(|e| format!("{}: {}", abs.display(), format_paths_error(&e)))?;
    let mut out = Vec::with_capacity(resolved.len());
    for r in resolved {
        let output_path = build_output_path_for_resolved_url(
            &r.url,
            route.output_extension.as_deref(),
        );
        out.push(RouteUniverseEntry {
            url_path: r.url,
            output_path,
            route_key: route.template.clone(),
        });
    }
    Ok(out)
}

/// Convert a `zfb_router::Segment` (the canonical router segment) into
/// the local stub used by [`zfb_render::paths::Segment`]. The variants
/// match exactly today; the conversion exists because `zfb-render` does
/// not depend on `zfb-router` (per the deliberate cyclical-dep
/// avoidance in the workspace layout).
fn router_segment_to_paths_segment(seg: &Segment) -> PathsSegment {
    match seg {
        Segment::Static(s) => PathsSegment::Static(s.clone()),
        Segment::Dynamic(name) => PathsSegment::Dynamic(name.clone()),
        Segment::Catchall(name) => PathsSegment::Catchall(name.clone()),
    }
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
        PathsError::InvalidPathsExport { field, reason, expected, .. } => {
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
/// - TSX pages whose extraction fails are still skipped from the map
///   (so the renderer's missing-key default of `true` applies), but
///   the failure is reported through `warn_unreadable` so the user
///   can fix typos in their `frontmatter` / `prerender` exports
///   instead of staring at a silent default.
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

/// Verify that `@takazudo/zfb-runtime` is resolvable from `project_root`
/// (i.e. a `node_modules/@takazudo/zfb-runtime` exists somewhere up the
/// directory tree). The bundle the renderer drives imports the runtime
/// at module load time; without it, miniflare boots and immediately
/// throws a module-resolution error that's harder for users to map back
/// to a fixable action.
///
/// Returns `Ok(())` when the runtime is present, `Err(anyhow!)` with a
/// "run pnpm install …" hint when it isn't. The check is best-effort —
/// custom `node_modules` layouts (yarn pnp, …) are accepted as long as
/// the conventional path exists.
pub fn check_runtime_installed(project_root: &Path) -> Result<()> {
    let mut cur: Option<&Path> = Some(project_root);
    while let Some(p) = cur {
        if p
            .join("node_modules")
            .join("@takazudo")
            .join("zfb-runtime")
            .exists()
        {
            return Ok(());
        }
        cur = p.parent();
    }
    Err(anyhow::anyhow!(
        "could not find `node_modules/@takazudo/zfb-runtime` under {} or any parent. \
         Run `pnpm install` (or your package manager's equivalent) in the project root \
         so the SSG-render bundle can resolve `@takazudo/zfb-runtime` at miniflare load time.",
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
        }
    }

    fn dynamic_route(name: &str, source: &str) -> Route {
        Route {
            source_path: PathBuf::from(source),
            segments: vec![Segment::Dynamic(name.to_string())],
            kind: RouteKind::Dynamic,
            specificity: 0,
            output_extension: None,
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
        assert_eq!(plan.static_routes[0].output_path, PathBuf::from("index.html"));
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
    fn build_prerender_map_reads_tsx_frontmatter_and_warns_on_unparseable() {
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
        // No frontmatter — extraction fails → no entry inserted, but a
        // warning IS emitted so the user can find their typo.
        std::fs::write(
            pages.join("broken.tsx"),
            "export default function() { return null; }\n",
        )
        .unwrap();

        let routes = vec![
            static_route(vec!["about"], "pages/about.tsx"),
            static_route(vec!["preview"], "pages/preview.tsx"),
            static_route(vec!["broken"], "pages/broken.tsx"),
        ];

        let mut warnings: Vec<String> = Vec::new();
        let map = build_prerender_map(&routes, dir.path(), |msg| warnings.push(msg.to_string()));
        assert_eq!(map.get("/about"), Some(&true));
        assert_eq!(map.get("/preview"), Some(&false));
        assert!(!map.contains_key("/broken"));
        // broken.tsx triggered exactly one warning naming the file.
        assert_eq!(warnings.len(), 1, "warnings: {warnings:?}");
        assert!(
            warnings[0].contains("broken.tsx"),
            "expected file name in warning, got: {}",
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
        assert!(map.is_empty(), "MDX should be left to the default-true path");
        // Non-TSX files are skipped silently (no warning).
        assert!(warnings.is_empty(), "warnings: {warnings:?}");
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
        let nested = dir.path().join("examples/basic-blog");
        std::fs::create_dir_all(&nested).unwrap();
        check_runtime_installed(&nested).unwrap();
    }

    #[test]
    fn check_runtime_installed_errors_when_runtime_missing() {
        let dir = tempdir().unwrap();
        let err = check_runtime_installed(dir.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("@takazudo/zfb-runtime"), "{msg}");
        assert!(msg.contains("pnpm install"), "{msg}");
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
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache);

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
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache);

        assert_eq!(out.resolved.len(), 1);
        assert_eq!(out.resolved[0].url_path, "/feed-a");
        assert_eq!(out.resolved[0].output_path, PathBuf::from("feed-a"));
    }

    #[test]
    fn expand_dynamic_routes_defers_non_literal_paths_with_reason() {
        // Mirrors the real basic-blog page: `paths()` does an
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
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache);

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

    #[test]
    fn expand_dynamic_routes_defers_when_paths_export_missing() {
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
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache);
        assert_eq!(out.resolved.len(), 0);
        assert_eq!(out.deferred.len(), 1);
        assert!(
            out.deferred[0]
                .reason
                .contains("no top-level `paths` export"),
            "reason: {}",
            out.deferred[0].reason,
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
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache);
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
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache);
        assert_eq!(out.resolved.len(), 0);
        assert_eq!(out.deferred.len(), 1);
        assert!(
            out.deferred[0].reason.contains("missing required param `slug`"),
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
            "/docs/:slug*",
            body,
        );
        let mut cache = PathsCache::new();
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache);

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

    /// Integration-style fixture: stage a tiny project with both a
    /// static page and a dynamic page that has a literal `paths()`,
    /// then walk through `build_route_universe` →
    /// `expand_dynamic_routes` and assert the combined renderer-shaped
    /// route list. This is the closest we can get to end-to-end without
    /// booting miniflare (which is gated by the sibling worker-entry
    /// topic).
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
        let expansion =
            expand_dynamic_routes(&plan.deferred_dynamic, dir.path(), &mut cache);
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
}
