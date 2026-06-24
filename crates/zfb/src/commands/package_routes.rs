//! Build-time materialiser for package-owned routes (#1193, epic #1191).
//!
//! A preset plugin's `setup()` may call `injectRoute(pattern, entrypoint,
//! opts?)`. During a **build** those routes are accepted (the dev-only
//! guard was lifted in #1193) and materialised here into a per-build
//! **overlay pages root** — a real on-disk directory containing the
//! user's real `pages/` tree (a copy) PLUS one synthesized module per
//! surviving package route, written at the route's pattern-derived path
//! under `pages/`.
//!
//! ## Why an on-disk overlay (Option A)
//!
//! The build reads the pages tree from four independent places — the
//! router scan, the bundler's shadow walk, the prerender-map source
//! reader, and the islands scanner — all keyed on a physical file at a
//! `source_path` under a real pages root. The only model that satisfies
//! all four without divergent shadow logic is to write the synthesized
//! module to disk in a pages root that every consumer already walks.
//! `build_pages_root` (resolved in `build::run`) is pointed at this
//! overlay, so package routes flow through the *unchanged* pipeline. See
//! the Z0 decision record on epic #1191 for the full rationale.
//!
//! ## Precedence: user `pages/` wins (pre-scan drop)
//!
//! `detect_ambiguity` (in `zfb-router`) is origin-blind and shape-keyed
//! (`[id]` ≡ `[slug]`), so a merged scan containing a user-vs-package
//! collision hard-errors before any precedence logic. We therefore drop
//! colliding package routes HERE, before the overlay is written, by
//! comparing route shape keys. Package-vs-package collisions are already
//! a hard error at registration (`InjectRouteConflict`).
//!
//! ## Lifetime
//!
//! The overlay is a [`tempfile::TempDir`] whose handle the caller keeps
//! alive for the whole build (it must outlive the bundle + render +
//! any `paths()` V8 eval that reads source by `source_path`). It lives
//! OUTSIDE the user's git tree and `pages/` is NEVER written to.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use zfb_build::InjectedRoute;

/// One package route that was materialised into the overlay.
#[derive(Debug, Clone)]
pub(crate) struct MaterializedRoute {
    /// The original URL pattern (`/preset-page`, `/blog/[slug]`).
    pub(crate) pattern: String,
    /// Path under the overlay `pages/` dir (`preset-page.tsx`,
    /// `blog/[slug].tsx`).
    pub(crate) pages_rel: PathBuf,
}

/// Result of resolving the build pages root.
///
/// When there are no package routes, `guard` is `None` and
/// `build_pages_root == project_root/pages` — the overlay machinery is
/// entirely bypassed (byte-identical parity, required by #1193). When
/// there are package routes, `guard` owns the temp overlay directory and
/// must be kept alive for the whole build.
pub(crate) struct OverlayResolution {
    /// The pages root every build consumer (router scan, bundler
    /// `pages_dir`, islands seed) is pointed at.
    pub(crate) build_pages_root: PathBuf,
    /// RAII handle for the overlay temp dir. `None` in the no-package
    /// route path. Dropping it deletes the overlay — keep it alive until
    /// the build finishes.
    pub(crate) guard: Option<tempfile::TempDir>,
    /// Package routes that were written into the overlay.
    pub(crate) materialized: Vec<MaterializedRoute>,
}

/// Resolve the build pages root, materialising an overlay when there are
/// package routes.
///
/// * `real_pages_dir` — `project_root/pages` (may be absent/empty).
/// * `injected_routes` — the build's accepted package routes.
///
/// Returns the resolved root + (optional) overlay guard. On the empty
/// `injected_routes` fast path this returns `real_pages_dir` with no
/// allocation beyond the `PathBuf`.
pub(crate) fn resolve_build_pages_root(
    real_pages_dir: &Path,
    injected_routes: &[InjectedRoute],
) -> Result<OverlayResolution> {
    if injected_routes.is_empty() {
        // Parity path: no overlay, byte-identical to a pre-#1193 build.
        return Ok(OverlayResolution {
            build_pages_root: real_pages_dir.to_path_buf(),
            guard: None,
            materialized: Vec::new(),
        });
    }

    // Pre-scan the real `pages/` for the set of route shape keys it
    // already owns, so a package route colliding with a user route is
    // dropped (user wins) BEFORE the merged scan that would otherwise
    // hard-error on the shape duplicate.
    let user_shape_keys = collect_user_pages_shape_keys(real_pages_dir)
        .context("scanning user pages/ for package-route precedence")?;

    // Build the survivor set, dropping user-shadowed routes.
    let mut survivors: Vec<(&InjectedRoute, PathBuf, String)> = Vec::new();
    for route in injected_routes {
        let pages_rel = pattern_to_pages_rel(&route.pattern).with_context(|| {
            format!(
                "package route `{}` (from plugin `{}`) is not a valid pages/ route pattern",
                route.pattern, route.plugin
            )
        })?;
        let shape_key = zfb_router::route_shape_key_for_pages_rel(&pages_rel).map_err(|e| {
            anyhow!(
                "package route `{}` (from plugin `{}`) could not be parsed: {e}",
                route.pattern,
                route.plugin
            )
        })?;
        if user_shape_keys.contains(&shape_key) {
            crate::output::info(format!(
                "package route `{}` (from plugin `{}`) is shadowed by a user pages/ route (user wins); skipping",
                route.pattern, route.plugin
            ));
            continue;
        }
        survivors.push((route, pages_rel, shape_key));
    }

    if survivors.is_empty() {
        // Every package route was shadowed by a user route. There is no
        // overlay to build — fall back to the real pages dir (the user's
        // own routes cover everything). This keeps the build behaving
        // exactly as if no package routes had been registered.
        return Ok(OverlayResolution {
            build_pages_root: real_pages_dir.to_path_buf(),
            guard: None,
            materialized: Vec::new(),
        });
    }

    // Materialise the overlay. The temp dir holds a `pages/` subdir
    // (named exactly "pages" — the bundler's `is_pages_dir` detection and
    // the scan's route derivation both key on that) into which we first
    // copy the user's real `pages/` (when present) and then write the
    // synthesized package modules.
    let guard = tempfile::Builder::new()
        .prefix("zfb-pkg-routes-")
        .tempdir()
        .context("creating overlay pages-root temp dir")?;
    let overlay_pages = guard.path().join("pages");
    std::fs::create_dir_all(&overlay_pages)
        .with_context(|| format!("creating overlay pages dir {}", overlay_pages.display()))?;

    if real_pages_dir.is_dir() {
        copy_dir_recursive(real_pages_dir, &overlay_pages).with_context(|| {
            format!(
                "copying user pages/ {} into overlay {}",
                real_pages_dir.display(),
                overlay_pages.display()
            )
        })?;
    }

    let mut materialized = Vec::with_capacity(survivors.len());
    for (route, pages_rel, _shape_key) in &survivors {
        let dest = overlay_pages.join(pages_rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating overlay route dir {}", parent.display()))?;
        }
        let module_src = synthesize_static_overlay_module(&route.entrypoint, route.prerender);
        std::fs::write(&dest, module_src.as_bytes())
            .with_context(|| format!("writing overlay route module {}", dest.display()))?;
        materialized.push(MaterializedRoute {
            pattern: route.pattern.clone(),
            pages_rel: pages_rel.clone(),
        });
    }

    Ok(OverlayResolution {
        build_pages_root: overlay_pages,
        guard: Some(guard),
        materialized,
    })
}

/// Walk the real `pages/` (if it exists) and collect the set of route
/// shape keys it owns. An absent/empty dir yields an empty set (no
/// collisions possible). Routing errors are surfaced — a broken user
/// `pages/` would fail the scan anyway, so failing here gives the same
/// outcome with clearer context.
fn collect_user_pages_shape_keys(real_pages_dir: &Path) -> Result<HashSet<String>> {
    let mut keys = HashSet::new();
    if !real_pages_dir.is_dir() {
        return Ok(keys);
    }
    let routes = zfb_router::scan_pages(real_pages_dir)
        .map_err(|e| anyhow!("user pages/ scan failed: {e}"))?;
    for route in routes {
        keys.insert(zfb_router::shape_key(&route.segments));
    }
    Ok(keys)
}

/// Convert a route pattern (`pages/`-filename grammar, leading `/`) to its
/// path under `pages/` — the inverse of the scanner's `derive_route`.
///
/// - `/`                 → `index.tsx`
/// - `/preset-page`      → `preset-page.tsx`
/// - `/a/b/c`            → `a/b/c.tsx`
/// - `/blog/[slug]`      → `blog/[slug].tsx`
/// - `/docs/[...rest]`   → `docs/[...rest].tsx`
///
/// `.tsx` is the chosen overlay source extension (the synthesized module
/// is always TSX). The JS host already rejected malformed patterns
/// (empty bracket segments, consecutive slashes, missing leading `/`),
/// so this stays a structural conversion; it still guards against path
/// traversal (`.`/`..` segments) defensively since the path is written
/// to disk.
pub(crate) fn pattern_to_pages_rel(pattern: &str) -> Result<PathBuf> {
    if !pattern.starts_with('/') {
        return Err(anyhow!("pattern must start with `/` (got {pattern:?})"));
    }
    let trimmed = pattern.trim_start_matches('/');
    if trimmed.is_empty() {
        // Root route → overlay index page.
        return Ok(PathBuf::from("index.tsx"));
    }

    let segments: Vec<&str> = trimmed.split('/').collect();
    let mut rel = PathBuf::new();
    let last = segments.len() - 1;
    for (i, seg) in segments.iter().enumerate() {
        if seg.is_empty() {
            // A trailing slash or `//` — the latter was already rejected
            // JS-side, the former is a malformed pattern.
            return Err(anyhow!(
                "pattern has an empty path segment (got {pattern:?})"
            ));
        }
        if *seg == "." || *seg == ".." {
            return Err(anyhow!(
                "pattern segment must not be `.` or `..` (got {pattern:?})"
            ));
        }
        if i == last {
            rel.push(format!("{seg}.tsx"));
        } else {
            rel.push(seg);
        }
    }
    Ok(rel)
}

/// Synthesize the overlay module source for a **static** package route.
///
/// The module re-exports the package entrypoint's default page component
/// (a re-export is fine for the *default* — it doesn't pass through a
/// syntactic extractor) and, when the `injectRoute` call supplied a
/// `prerender` hint, INLINES `export const frontmatter` + `export const
/// prerender = …` so the frontmatter extractor and the `output: static`
/// safety gate actually SEE the prerender flag.
///
/// Why BOTH must be inlined: `extract_tsx_frontmatter` only honours a
/// top-level `export const prerender` when an `export const frontmatter`
/// is ALSO present — without `frontmatter` the extractor returns
/// `MissingFrontmatter`, which `build_prerender_map` swallows, defaulting
/// the route to SSG and silently dropping the `prerender = false`. So a
/// `prerender` hint requires the `frontmatter` sibling for the flag to be
/// effective (re-exports are invisible to the syntactic extractor — both
/// must be physically present here). With no hint, both are omitted →
/// SSG default (the desired default for package routes).
///
/// The entrypoint is imported by its absolute path; esbuild resolves an
/// absolute specifier as-is, so this is independent of where the overlay
/// physically lives.
pub(crate) fn synthesize_static_overlay_module(
    entrypoint: &Path,
    prerender: Option<bool>,
) -> String {
    let spec = json_string(&entrypoint.to_string_lossy());
    let mut out = String::new();
    out.push_str("// AUTO-GENERATED by zfb (package-owned routes, #1193). Do not edit.\n");
    out.push_str("// Re-exports the package entrypoint's default page component;\n");
    out.push_str("// the build renders this overlay module like any pages/ file.\n");
    out.push_str(&format!("export {{ default }} from {spec};\n"));
    if let Some(prerender) = prerender {
        // Inlined top-level (NOT re-exported) so the frontmatter extractor
        // — and the output:static gate via build_prerender_map — sees the
        // flag. `frontmatter` must accompany `prerender` or the extractor
        // returns MissingFrontmatter and the flag is dropped (#1193).
        out.push_str(
            "// Inlined (not re-exported) so the AST extractor sees the prerender flag (#1193).\n",
        );
        out.push_str("export const frontmatter = {};\n");
        out.push_str(&format!("export const prerender = {prerender};\n"));
    }
    out
}

/// Recursively copy a directory tree (files + subdirs), following the
/// same "real files" semantics esbuild needs in the shadow tree. Symlinks
/// to files are dereferenced (copied as content); symlinked subdirs are
/// walked. Used to seed the overlay with the user's real `pages/`.
fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest).with_context(|| format!("creating dir {}", dest.display()))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("reading dir {}", src.display()))? {
        let entry = entry?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_file() {
            std::fs::copy(&from, &to)
                .with_context(|| format!("copying {} -> {}", from.display(), to.display()))?;
        } else if file_type.is_symlink() {
            // Resolve the symlink target's type and copy accordingly.
            let meta = std::fs::metadata(&from)
                .with_context(|| format!("stat symlink target {}", from.display()))?;
            if meta.is_dir() {
                copy_dir_recursive(&from, &to)?;
            } else if meta.is_file() {
                std::fs::copy(&from, &to).with_context(|| {
                    format!("copying symlinked {} -> {}", from.display(), to.display())
                })?;
            }
        }
    }
    Ok(())
}

/// Minimal JSON string encoder for emitting a JS import specifier. Only
/// the characters that can appear in a filesystem path and would break a
/// double-quoted JS string literal are escaped.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_to_pages_rel_static() {
        assert_eq!(
            pattern_to_pages_rel("/").unwrap(),
            PathBuf::from("index.tsx")
        );
        assert_eq!(
            pattern_to_pages_rel("/preset-page").unwrap(),
            PathBuf::from("preset-page.tsx")
        );
        assert_eq!(
            pattern_to_pages_rel("/a/b/c").unwrap(),
            PathBuf::from("a/b/c.tsx")
        );
    }

    #[test]
    fn pattern_to_pages_rel_dynamic() {
        assert_eq!(
            pattern_to_pages_rel("/blog/[slug]").unwrap(),
            PathBuf::from("blog/[slug].tsx")
        );
        assert_eq!(
            pattern_to_pages_rel("/docs/[...rest]").unwrap(),
            PathBuf::from("docs/[...rest].tsx")
        );
    }

    #[test]
    fn pattern_to_pages_rel_rejects_traversal() {
        assert!(pattern_to_pages_rel("/a/../b").is_err());
        assert!(pattern_to_pages_rel("/./x").is_err());
        assert!(pattern_to_pages_rel("no-leading-slash").is_err());
    }

    #[test]
    fn overlay_module_omits_prerender_by_default() {
        let m = synthesize_static_overlay_module(Path::new("/pkg/page.tsx"), None);
        assert!(m.contains("export { default } from \"/pkg/page.tsx\""));
        // No prerender EXPORT statement → SSG default. (Assert on the
        // statement, not the bare substring — a comment may mention the
        // word.)
        assert!(!m.contains("export const prerender"));
    }

    #[test]
    fn overlay_module_inlines_prerender_when_set() {
        let m = synthesize_static_overlay_module(Path::new("/pkg/ssr.tsx"), Some(false));
        // Inlined top-level so the AST extractor sees it — and `frontmatter`
        // MUST accompany `prerender` or the extractor drops the flag.
        assert!(m.contains("export const prerender = false;"));
        assert!(
            m.contains("export const frontmatter = {}"),
            "prerender requires a sibling frontmatter export to be effective"
        );
        let m_true = synthesize_static_overlay_module(Path::new("/pkg/p.tsx"), Some(true));
        assert!(m_true.contains("export const prerender = true;"));
        assert!(m_true.contains("export const frontmatter = {}"));
    }

    #[test]
    fn no_package_routes_bypasses_overlay() {
        let tmp = tempfile::tempdir().unwrap();
        let pages = tmp.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();
        let res = resolve_build_pages_root(&pages, &[]).unwrap();
        assert!(res.guard.is_none());
        assert_eq!(res.build_pages_root, pages);
        assert!(res.materialized.is_empty());
    }

    fn route(pattern: &str, entrypoint: &str) -> InjectedRoute {
        InjectedRoute {
            pattern: pattern.into(),
            entrypoint: PathBuf::from(entrypoint),
            plugin: "preset".into(),
            prerender: None,
        }
    }

    #[test]
    fn materializes_static_route_into_overlay() {
        let tmp = tempfile::tempdir().unwrap();
        let pages = tmp.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();
        std::fs::write(pages.join("index.tsx"), "export default () => null;").unwrap();

        let routes = vec![route("/preset-page", "/pkg/preset-page.tsx")];
        let res = resolve_build_pages_root(&pages, &routes).unwrap();

        assert!(res.guard.is_some(), "overlay temp dir must be held");
        assert_ne!(res.build_pages_root, pages);
        // User's index page copied in.
        assert!(res.build_pages_root.join("index.tsx").is_file());
        // Package route written.
        let pkg = res.build_pages_root.join("preset-page.tsx");
        assert!(pkg.is_file());
        let body = std::fs::read_to_string(&pkg).unwrap();
        assert!(body.contains("/pkg/preset-page.tsx"));
        assert_eq!(res.materialized.len(), 1);
    }

    #[test]
    fn nested_route_written_at_correct_path() {
        let tmp = tempfile::tempdir().unwrap();
        let pages = tmp.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();

        let routes = vec![route("/a/b/c", "/pkg/deep.tsx")];
        let res = resolve_build_pages_root(&pages, &routes).unwrap();
        let nested = res.build_pages_root.join("a").join("b").join("c.tsx");
        assert!(
            nested.is_file(),
            "nested overlay module must exist at a/b/c.tsx"
        );
    }

    #[test]
    fn user_route_wins_collision_dropped_pre_scan() {
        let tmp = tempfile::tempdir().unwrap();
        let pages = tmp.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();
        // User owns /about.
        std::fs::write(pages.join("about.tsx"), "export default () => null;").unwrap();

        // Package tries to also own /about → dropped (user wins).
        let routes = vec![route("/about", "/pkg/about.tsx")];
        let res = resolve_build_pages_root(&pages, &routes).unwrap();
        assert!(
            res.materialized.is_empty(),
            "colliding package route must be dropped"
        );
        // All routes shadowed → no overlay, real pages dir returned.
        assert!(res.guard.is_none());
        assert_eq!(res.build_pages_root, pages);
    }

    #[test]
    fn user_route_wins_shape_collision_id_vs_slug() {
        let tmp = tempfile::tempdir().unwrap();
        let pages = tmp.path().join("pages");
        let blog = pages.join("blog");
        std::fs::create_dir_all(&blog).unwrap();
        // User owns /blog/[id].
        std::fs::write(blog.join("[id].tsx"), "export default () => null;").unwrap();

        // Package tries /blog/[slug] — same SHAPE (`/blog/:*`) → dropped.
        let routes = vec![route("/blog/[slug]", "/pkg/blog-slug.tsx")];
        let res = resolve_build_pages_root(&pages, &routes).unwrap();
        assert!(
            res.materialized.is_empty(),
            "shape-duplicate package route ([id] vs [slug]) must be dropped"
        );
    }

    #[test]
    fn empty_pages_with_root_package_route() {
        let tmp = tempfile::tempdir().unwrap();
        // No real pages/ dir at all.
        let pages = tmp.path().join("pages");

        let routes = vec![route("/", "/pkg/home.tsx")];
        let res = resolve_build_pages_root(&pages, &routes).unwrap();
        assert!(res.guard.is_some());
        // `/` → overlay index.tsx.
        assert!(res.build_pages_root.join("index.tsx").is_file());
        assert_eq!(res.materialized.len(), 1);
    }
}
