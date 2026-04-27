//! Shared helpers that wire the wave-2 outputs (`zfb_build::bundler` +
//! `zfb_build::renderer`) into the `zfb build` and `zfb dev` commands.
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
//! Two pieces of the SSG-render pipeline are knowingly **not yet
//! wired** by this module:
//!
//! 1. **Dynamic `paths()` expansion.** Routes whose template contains
//!    `[slug]`, `[page]`, `[...rest]`, etc. require running each page
//!    module's exported `paths()` function inside the worker bundle to
//!    enumerate the concrete URLs. The plumbing for that is its own
//!    sub-task — it needs an extra worker endpoint plus an HTTP query
//!    loop. Until that lands, [`build_route_universe`] returns the
//!    dynamic routes via [`PendingDynamicRoute`] so callers can surface
//!    a clear "skipped" warning instead of silently emitting nothing.
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
use zfb_router::{Route, RouteKind, Segment};

/// A dynamic / catchall route that [`build_route_universe`] could not
/// yet expand into concrete URLs (the `paths()` follow-up is pending —
/// see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDynamicRoute {
    /// Source path of the page module, used to point the user at the
    /// offending file in a warning.
    pub source_path: PathBuf,
    /// Route template (e.g. `/blog/:slug`).
    pub template: String,
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
                });
            }
        }
    }
    plan
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

/// Suppress unused-import warning when the renderer module is consumed
/// without `Segment` (kept here so future expansion of dynamic routes
/// has a single import surface to grow).
#[allow(dead_code)]
fn _segment_marker(_: &Segment) {}

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
}
