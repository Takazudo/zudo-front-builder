//! `zfb build` command — one-shot production build.
//!
//! Contract:
//!   pub async fn run(args: &crate::cli::BuildArgs) -> anyhow::Result<()>
//!
//! `args.outdir` is the production output directory (default `dist`).
//! Resolved relative to the current working directory if not absolute.
//!
//! ## v1 scope (post Wave 2 / Sub 5 + Sub 3 + zfb-router wiring)
//!
//! What this build now does:
//!
//! 1. Validates we're inside something that looks like a project root
//!    (presence of a `pages/` directory).
//! 2. Loads `zfb.config.{json,ts}` via `crate::config::load_from_dir`. JSON
//!    parses; TS surfaces a clear "not yet supported" error. The CLI's
//!    `--outdir` always wins over the loaded config (matches cmd-dev's
//!    "CLI overrides config" contract — clap sets the default to "dist", so
//!    by the time we see the value here it is always present).
//! 3. Runs `zfb_router::Router::scan` on `pages/` to enumerate routes.
//! 4. For every static route, writes a per-route stub HTML at the
//!    conventional output path (`<outdir>/index.html` for `/`,
//!    `<outdir>/<segments...>/index.html` otherwise) using
//!    [`zfb_build::atomic_write_string`].
//! 5. Skips dynamic / catchall routes with a `warn` line — those need a
//!    runtime `paths()` evaluation that requires the real renderer (blocked
//!    on ADR-001: `DenoCoreHost` is still a skeleton).
//! 6. If the project has zero `pages/*.tsx` files we still emit a stub
//!    `index.html` so `zfb preview` has something to serve.
//! 7. Prints the canonical summary line `✓ N pages built in X.XXs` via
//!    `crate::output::success` (which adds the green checkmark).
//!
//! What's still deferred:
//!
//! - Real per-route SSR rendering. `zfb-render` / `zfb-content` need a
//!   working `PageRenderer`, which depends on a working JS runtime
//!   (ADR-001 / `DenoCoreHost` skeleton). Until then each static route's
//!   body is a placeholder that names the route template.
//! - Dynamic and catchall routes. Expanding `[slug].tsx` into concrete
//!   paths needs `paths()` calls evaluated by the renderer; this is the
//!   same blocker as above.
//!
//! The command's `run` signature, the project-root validation, and the
//! summary contract should remain stable across that swap.

use std::env;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use zfb_build::atomic_write_string;
use zfb_router::{Route, RouteKind, Router};

use crate::cli::BuildArgs;
use crate::output;

pub async fn run(args: &BuildArgs) -> Result<()> {
    let started = Instant::now();

    let project_root = env::current_dir().context("failed to read current working directory")?;

    // v1 project-root sanity check. The config loader gives us a richer
    // schema downstream, but the cheapest "is this a zfb project?" signal
    // is still the presence of a `pages/` directory (matches the watcher's
    // default roots and the router's scan target).
    let pages_dir = project_root.join("pages");
    if !pages_dir.is_dir() {
        return Err(anyhow!(
            "no `pages/` directory found in {}; run `zfb build` from a project root",
            project_root.display()
        ));
    }

    // Load the project config. We do not currently consume any field other
    // than to surface load errors — `outdir` is taken from `args` because
    // the CLI flag wins (clap injects the default "dist" if the user didn't
    // pass `--outdir`). When more config consumers land (e.g. `publicDir`
    // copy, `tailwind.enabled`), they'll read off the loaded `Config`.
    let _config = match crate::config::load_from_dir(&project_root).await {
        Ok(cfg) => cfg,
        Err(e) => {
            // Render the multi-line user-friendly error block before
            // returning. `format_error` already includes a trailing newline,
            // so trim the right side before handing it to `output::error`
            // (which uses `eprintln!` and would otherwise add a second
            // blank line). main() will still print a terse fallback line —
            // a small redundancy that keeps the contract straightforward.
            output::error(output::format_error(&e).trim_end());
            return Err(e);
        }
    };

    let outdir = resolve_outdir(&project_root, &args.outdir);

    // Enumerate routes. RouterError surfaces ambiguous routes with full
    // source paths; we wrap with a one-line context so the user can tell
    // where the failure happened in the build pipeline.
    let router = Router::scan(&pages_dir)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("scanning routes under {}", pages_dir.display()))?;

    let routes = router.routes();
    let mut pages_built: usize = 0;

    if routes.is_empty() {
        // Empty `pages/` is unusual but legal — the user might be
        // bootstrapping. Still emit a stub index.html so `zfb preview`
        // has something to serve.
        output::warn(format!(
            "no pages found under {}; emitting placeholder index.html only",
            pages_dir.display()
        ));
        let index_path = outdir.join("index.html");
        let stub = stub_route_html("/");
        atomic_write_string(&index_path, &stub)
            .with_context(|| format!("failed to write {}", index_path.display()))?;
        pages_built = 1;
    } else {
        for route in routes {
            match route.kind {
                RouteKind::Static => {
                    // route_output_path returns Some(_) for static routes
                    // (see helper docs); destructure with a defensive
                    // fallback so we never silently lose a static route.
                    let Some(out_path) = route_output_path(&outdir, route) else {
                        // Should not happen — keep the path narrow rather
                        // than panicking.
                        output::warn(format!(
                            "static route {} produced no output path; skipping",
                            route.template()
                        ));
                        continue;
                    };
                    let body = stub_route_html(&route.template());
                    atomic_write_string(&out_path, &body)
                        .with_context(|| format!("failed to write {}", out_path.display()))?;
                    pages_built += 1;
                }
                RouteKind::Dynamic | RouteKind::Catchall => {
                    output::warn(format!(
                        "skipping {} — dynamic / catchall routes require runtime paths() \
                         evaluation, pending real renderer (ADR-001)",
                        route.template()
                    ));
                }
            }
        }
    }

    let elapsed = started.elapsed().as_secs_f64();
    // `output::success` adds the green ✓ prefix automatically; the canonical
    // line text "✓ N pages built in X.XXs" is preserved.
    output::success(format!("{pages_built} pages built in {elapsed:.2}s"));

    Ok(())
}

/// Resolve `outdir` against `project_root`. If `outdir` is absolute it is
/// used as-is; if relative it is joined onto `project_root`. The result is
/// returned without canonicalising — the directory may not exist yet, and
/// `atomic_write_string` will create parents on demand.
fn resolve_outdir(project_root: &Path, outdir: &Path) -> PathBuf {
    if outdir.is_absolute() {
        outdir.to_path_buf()
    } else {
        project_root.join(outdir)
    }
}

/// Compute the on-disk output path for a route under `outdir`.
///
/// - The index route (`/`) maps to `<outdir>/index.html`.
/// - A static route with template `/about` maps to `<outdir>/about/index.html`.
/// - A multi-segment static route like `/blog/intro` maps to
///   `<outdir>/blog/intro/index.html`.
/// - Dynamic and catchall routes return `None` — they need runtime
///   `paths()` expansion which v1 does not have.
fn route_output_path(outdir: &Path, route: &Route) -> Option<PathBuf> {
    if route.kind != RouteKind::Static {
        return None;
    }
    if route.segments.is_empty() {
        return Some(outdir.join("index.html"));
    }
    let mut path = outdir.to_path_buf();
    for seg in &route.segments {
        // Static-only by virtue of the kind check above; defensive match
        // keeps the helper honest if `RouteKind::Static` ever grows new
        // segment shapes.
        match seg {
            zfb_router::Segment::Static(s) => path.push(s),
            _ => return None,
        }
    }
    path.push("index.html");
    Some(path)
}

/// Stub HTML emitted for each static route in the v1 build. Includes the
/// route template in both `<title>` and `<body>` so the user can tell which
/// file they're looking at while the real renderer is still pending.
fn stub_route_html(template: &str) -> String {
    // Minimal HTML escaping for the route template: only `<`, `>`, `&` need
    // attention here, and segments are constrained by the router to ASCII
    // identifiers + `:` / `*`. Doing it explicitly keeps the helper safe if
    // that contract loosens.
    let escaped = template
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<!doctype html>\n\
         <html lang=\"en\">\n\
         <head>\n\
         <meta charset=\"utf-8\">\n\
         <title>zfb build — {escaped}</title>\n\
         </head>\n\
         <body>\n\
         <h1>zfb build (v1 stub)</h1>\n\
         <p>Route: <code>{escaped}</code></p>\n\
         <p>The production renderer is not yet wired in. This page is a placeholder so <code>zfb preview</code> has something to serve.</p>\n\
         </body>\n\
         </html>\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use zfb_router::Segment;

    fn static_route(segments: Vec<&str>) -> Route {
        let segs: Vec<Segment> = segments
            .into_iter()
            .map(|s| Segment::Static(s.to_string()))
            .collect();
        Route {
            source_path: PathBuf::from("pages/dummy.tsx"),
            segments: segs,
            kind: RouteKind::Static,
            specificity: 0,
        }
    }

    fn dynamic_route() -> Route {
        Route {
            source_path: PathBuf::from("pages/[slug].tsx"),
            segments: vec![Segment::Dynamic("slug".to_string())],
            kind: RouteKind::Dynamic,
            specificity: 0,
        }
    }

    fn catchall_route() -> Route {
        Route {
            source_path: PathBuf::from("pages/[...rest].tsx"),
            segments: vec![Segment::Catchall("rest".to_string())],
            kind: RouteKind::Catchall,
            specificity: 0,
        }
    }

    #[test]
    fn resolve_outdir_keeps_absolute_paths() {
        let root = Path::new("/proj");
        let abs = PathBuf::from("/tmp/zfb-out");
        assert_eq!(resolve_outdir(root, &abs), abs);
    }

    #[test]
    fn resolve_outdir_joins_relative_paths_onto_root() {
        let root = Path::new("/proj");
        let rel = PathBuf::from("dist");
        assert_eq!(resolve_outdir(root, &rel), PathBuf::from("/proj/dist"));
    }

    #[test]
    fn stub_route_html_contains_template_and_is_well_formed() {
        let html = stub_route_html("/about");
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("/about"));
        assert!(html.contains("<title>zfb build — /about</title>"));
        assert!(html.trim_end().ends_with("</html>"));
    }

    #[test]
    fn stub_route_html_escapes_template() {
        // Defensive: even though the router constrains segment shapes, the
        // helper itself must not let `<`/`>`/`&` flow into HTML unescaped.
        let html = stub_route_html("/<x>&");
        assert!(html.contains("&lt;x&gt;&amp;"));
        assert!(!html.contains("/<x>"));
    }

    #[test]
    fn route_output_path_index() {
        let outdir = Path::new("/out");
        let route = static_route(vec![]);
        assert_eq!(
            route_output_path(outdir, &route),
            Some(PathBuf::from("/out/index.html"))
        );
    }

    #[test]
    fn route_output_path_single_segment_static() {
        let outdir = Path::new("/out");
        let route = static_route(vec!["about"]);
        assert_eq!(
            route_output_path(outdir, &route),
            Some(PathBuf::from("/out/about/index.html"))
        );
    }

    #[test]
    fn route_output_path_multi_segment_static() {
        let outdir = Path::new("/out");
        let route = static_route(vec!["blog", "intro"]);
        assert_eq!(
            route_output_path(outdir, &route),
            Some(PathBuf::from("/out/blog/intro/index.html"))
        );
    }

    #[test]
    fn route_output_path_dynamic_returns_none() {
        let outdir = Path::new("/out");
        assert_eq!(route_output_path(outdir, &dynamic_route()), None);
    }

    #[test]
    fn route_output_path_catchall_returns_none() {
        let outdir = Path::new("/out");
        assert_eq!(route_output_path(outdir, &catchall_route()), None);
    }
}
