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
//! ### Known precedence limitations (loud failure, narrow intersection)
//!
//! Both of these are edge cases that fail LOUDLY (never silent shadowing)
//! and only with package routes present; the robust fixes thread more
//! config through the materialiser and belong with Z1b / the Confirm wave:
//!
//! - **Optional-catchall cross-length conflict.** The pre-scan drop
//!   compares EXACT shape keys, so it does not cover the router's
//!   cross-length optional-catchall rule: a package `/docs/[[...rest]]`
//!   (shape `/docs/:...`) and a user `pages/docs/index.tsx` (shape `/docs`)
//!   have different keys, so the package route survives and the merged scan
//!   raises `OptionalCatchallConflict`. Robust fix: reproduce
//!   `detect_optional_catchall_conflicts`' prefix/zero-segment logic here.
//! - **`bundle.exclude` on a user `pages/` file.** Copied user pages live
//!   at an overlay path outside `project_root`, so `BundleExcludeMatcher`
//!   (which strips relative to `project_root`) no longer matches them — an
//!   `bundle.exclude: ["pages/x.tsx"]` entry stops excluding the copied
//!   `x.tsx`. Robust fix: thread the resolved exclude globs in and skip
//!   matching pages during the overlay copy.
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
        let module_src = if is_dynamic_pattern(&route.pattern) {
            // Dynamic package route (`[param]` / `[...catchall]`): the
            // overlay must surface a TOP-LEVEL `paths` the syntactic
            // extractor sees, or the route hits `Missing` → hard error
            // (#1194). We read + classify the package entrypoint with the
            // SAME extractor the pipeline uses so literal vs runtime is
            // decided once, here.
            synthesize_dynamic_overlay_module(&route.entrypoint, route.prerender).with_context(
                || {
                    format!(
                        "synthesizing dynamic overlay module for package route `{}` (from plugin `{}`)",
                        route.pattern, route.plugin
                    )
                },
            )?
        } else {
            synthesize_static_overlay_module(&route.entrypoint, route.prerender)
        };
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
            // The scanner collapses a final `index` stem to the parent's
            // route (`pages/index.tsx` → `/`), so materialising
            // `injectRoute("/index", …)` as `index.tsx` would silently
            // serve `/` (or collide with the user's root page) rather than
            // `/index`. There is no non-ambiguous overlay path for a literal
            // trailing `index` segment, so reject it with a clear error
            // (the author should use `"/"` for the root route).
            if *seg == "index" {
                return Err(anyhow!(
                    "pattern must not end in a literal `index` segment — the scanner \
                     collapses it to the parent route; use `\"/\"` for the root route \
                     (got {pattern:?})"
                ));
            }
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
///
/// ## Known limitations (Z1a scope; Z1b sharp edges)
///
/// Because the entrypoint is imported by absolute path (re-export of the
/// default), the package page module itself is bundled from OUTSIDE the
/// bundler's shadow tree. Two consequences, both edge cases for v1 and
/// flagged for Z1b to resolve when it makes the overlay a true namespace
/// proxy:
///
/// - **zfb shadow source transforms are skipped for the package page.**
///   A package page importing a `*.module.css` (CSS-Modules scoping) or
///   using `import.meta.glob(...)` does NOT get those zfb-specific
///   rewrites (they run only on files materialised into the shadow). Plain
///   TSX + relative TS imports + node_modules deps work (verified by the
///   integration tests); CSS-Modules/glob in a package page do not.
/// - **Named page exports other than `default` are not proxied.** A
///   package page's own `export const frontmatter` / `getStaticProps` /
///   (Z1b) `export const paths` are not re-exported — the overlay owns
///   `prerender`/`frontmatter` via the `injectRoute` hint instead. Z1b,
///   which must surface a top-level `paths`, will replace this default-only
///   re-export with a namespace-preserving proxy.
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
    push_inlined_prerender(&mut out, prerender);
    out
}

/// Inline the `frontmatter` + `prerender` exports top-level when an
/// explicit `prerender` hint was supplied (shared by the static and
/// dynamic synthesizers).
///
/// Inlined top-level (NOT re-exported) so the frontmatter extractor — and
/// the `output: static` gate via `build_prerender_map` — sees the flag.
/// `frontmatter` MUST accompany `prerender` or the extractor returns
/// `MissingFrontmatter` and the flag is silently dropped (#1193).
fn push_inlined_prerender(out: &mut String, prerender: Option<bool>) {
    let Some(prerender) = prerender else { return };
    out.push_str(
        "// Inlined (not re-exported) so the AST extractor sees the prerender flag (#1193).\n",
    );
    out.push_str("export const frontmatter = {};\n");
    out.push_str(&format!("export const prerender = {prerender};\n"));
}

/// `true` when a route pattern has a dynamic (`[param]`) or catchall
/// (`[...rest]` / `[[...rest]]`) segment — i.e. the scanner will classify
/// it `RouteKind::Dynamic`/`Catchall` and the renderer requires a
/// `paths()` export. Keyed on the bracket marker the `pages/` filename
/// grammar uses; consecutive-slash / malformed patterns were already
/// rejected JS-side and again in `pattern_to_pages_rel`.
fn is_dynamic_pattern(pattern: &str) -> bool {
    pattern.contains('[')
}

/// Synthesize the overlay module source for a **dynamic** package route
/// (`[param]` / `[...catchall]`). #1194, epic #1191.
///
/// A dynamic route requires a TOP-LEVEL `paths` export the syntactic
/// extractor (`zfb_render::paths_extract`) can see, or it classifies the
/// overlay module `Missing` → the `render_pipeline` hard error fires (the
/// same parity invariant as a `pages/` dynamic route with no `paths()`).
/// A re-export (`export { paths } from "<pkg>"`) is invisible to the
/// extractor (it matches only top-level `ExportDecl`), so the `paths` must
/// be PHYSICALLY top-level here.
///
/// We classify the package entrypoint's own `paths` with the **same**
/// extractor the build pipeline uses, so the literal-vs-runtime decision
/// is made once and can't drift:
///
/// - **Literal** (`extract_paths` → `Literal`): inline a literal-returning
///   `export function paths() { return <json>; }` carrying the package's
///   resolved JSON. The pipeline re-extracts `Literal` from the overlay →
///   `try_expand_one` expands statically, **no V8 round-trip**.
/// - **Runtime / non-literal** (`NonLiteral`, e.g. `getCollection(...)`, OR
///   a `PathsExtractError::Parse` the SWC static extractor rejects but
///   esbuild/V8 can still bundle): import the package's real `paths` under
///   an alias and re-declare a top-level wrapper `export async function
///   paths() { return __zfb_pkg_paths(...); }`. The extractor sees a
///   top-level `paths` whose body is a CALL → `NonLiteral` → deferred to
///   `eval_deferred_paths_via_worker`, which runs the **bundled** module's
///   real `paths()` in V8 via `GET /__paths__/<route_key>`. Because the
///   overlay module IS what the bundler bundles, the imported
///   `__zfb_pkg_paths` resolves to the real package `paths` inside the
///   worker. Folding a parse error into the defer path keeps parity with
///   the user-page flow (`try_expand_one` defers a parse error rather than
///   hard-erroring — only a genuinely-`Missing` `paths` hard-errors).
/// - **Missing** (`extract_paths` → `Missing`): the package author shipped
///   a dynamic route with NO `paths` export. We inline NO `paths` (only the
///   default re-export), so the pipeline's own `extract_paths` on the
///   overlay also returns `Missing` → the existing hard error fires with a
///   clear message. This is deliberate: it keeps the missing-`paths()`
///   hard-error parity flowing through the single canonical error path
///   rather than synthesizing a wrapper that would fail later/worse.
///
/// The default page component is re-exported (a re-export is fine for the
/// `default` — it doesn't pass through a syntactic extractor) and the
/// entrypoint is imported by absolute path (esbuild resolves it as-is, so
/// this is independent of where the overlay physically lives).
///
/// ## Known limitations (shared with the static synthesizer)
///
/// The package page is bundled from OUTSIDE the bundler's shadow tree
/// (imported by absolute path), so zfb's shadow source transforms
/// (CSS-Modules scoping, `import.meta.glob`) are NOT applied to it. Plain
/// TSX + relative TS imports + node_modules deps work. This is a Z1a-noted
/// v1 limitation; do not author CSS-Modules/glob in a package route page.
pub(crate) fn synthesize_dynamic_overlay_module(
    entrypoint: &Path,
    prerender: Option<bool>,
) -> Result<String> {
    let spec = json_string(&entrypoint.to_string_lossy());

    // Read + classify the package entrypoint's `paths` with the canonical
    // extractor. An unreadable entrypoint is a hard error here (the build
    // could not have rendered the route anyway).
    let source = std::fs::read_to_string(entrypoint).with_context(|| {
        format!(
            "reading package route entrypoint {} to extract its `paths`",
            entrypoint.display()
        )
    })?;
    let file_name = entrypoint
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| entrypoint.display().to_string());
    // Classify the entrypoint's `paths`. A PARSE error is NOT a hard
    // failure here: the user-page dynamic flow (`try_expand_one` →
    // `expand_dynamic_routes`) treats a `PathsExtractError::Parse` as a
    // `TryExpandFailure::Other` → DEFERRED to the V8 worker, not a build
    // error (only a genuinely-`Missing` `paths` hard-errors). For parity
    // we fold a parse error into the runtime-wrapper path: the SWC static
    // extractor may reject syntax esbuild/V8 can still bundle, and the
    // worker re-runs the real `paths()` regardless.
    let extraction = zfb_render::paths_extract::extract_paths(&source, &file_name);

    let mut out = String::new();
    out.push_str("// AUTO-GENERATED by zfb (package-owned routes, #1194). Do not edit.\n");
    out.push_str("// Dynamic package route: re-exports the entrypoint's default page\n");
    out.push_str("// component and surfaces a TOP-LEVEL `paths` the extractor can see.\n");
    out.push_str(&format!("export {{ default }} from {spec};\n"));

    match extraction {
        Ok(zfb_render::paths_extract::PathsExtraction::Literal(json)) => {
            // Inline the resolved JSON as a literal-returning function so
            // the pipeline re-classifies it `Literal` and expands with no
            // V8. `serde_json` emits valid JS-literal syntax (a JSON array
            // of objects is a valid JS expression).
            let literal = serde_json::to_string(&json).with_context(|| {
                format!(
                    "serializing the literal `paths` extracted from {}",
                    entrypoint.display()
                )
            })?;
            out.push_str("// Literal paths() inlined top-level (no runtime/V8 needed) (#1194).\n");
            out.push_str(&format!(
                "export function paths() {{ return {literal}; }}\n"
            ));
        }
        // NonLiteral OR a parse error the static extractor couldn't handle:
        // import the package's real `paths` and wrap it top-level. The
        // extractor sees a CALL body → NonLiteral → deferred to the V8
        // `__paths__` worker, which runs THIS module (and thus the imported
        // real `paths`) inside the bundle. A parse error is deferred here
        // exactly as the user-page flow defers it (parity).
        Ok(zfb_render::paths_extract::PathsExtraction::NonLiteral { .. }) | Err(_) => {
            out.push_str(
                "// Runtime paths() wrapper: imports the package's real `paths` and re-declares\n",
            );
            out.push_str(
                "// it top-level so the extractor defers to the V8 `__paths__` worker (#1194).\n",
            );
            out.push_str(&format!(
                "import {{ paths as __zfb_pkg_paths }} from {spec};\n"
            ));
            out.push_str("export async function paths() { return await __zfb_pkg_paths(); }\n");
        }
        Ok(zfb_render::paths_extract::PathsExtraction::Missing) => {
            // No `paths` in the package entrypoint. Inline NONE, so the
            // pipeline's own extractor returns Missing → the canonical
            // hard error fires (parity with a pages/ dynamic route that
            // forgot paths()). A comment records WHY nothing was inlined.
            out.push_str(
                "// The package entrypoint exports no top-level `paths`; intentionally inlining\n",
            );
            out.push_str(
                "// none so the build's hard error for a missing paths() fires (parity) (#1194).\n",
            );
        }
    }

    push_inlined_prerender(&mut out, prerender);
    Ok(out)
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
    fn pattern_to_pages_rel_rejects_trailing_index() {
        // A final literal `index` segment would collapse to the parent
        // route, silently serving `/` instead of `/index` — reject it.
        assert!(pattern_to_pages_rel("/index").is_err());
        assert!(pattern_to_pages_rel("/docs/index").is_err());
        // `index` as a NON-final segment is a normal directory name.
        assert_eq!(
            pattern_to_pages_rel("/index/x").unwrap(),
            PathBuf::from("index/x.tsx")
        );
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
    fn is_dynamic_pattern_classifies_bracket_segments() {
        assert!(!is_dynamic_pattern("/"));
        assert!(!is_dynamic_pattern("/preset-page"));
        assert!(!is_dynamic_pattern("/a/b/c"));
        assert!(is_dynamic_pattern("/blog/[slug]"));
        assert!(is_dynamic_pattern("/docs/[...slug]"));
        assert!(is_dynamic_pattern("/docs/[[...slug]]"));
    }

    /// Write `body` to a temp `.tsx` entrypoint and return its path + the
    /// owning TempDir (kept alive by the caller).
    fn entrypoint_with(body: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("entry.tsx");
        std::fs::write(&p, body).unwrap();
        (dir, p)
    }

    /// Re-run the canonical extractor over a synthesized overlay module to
    /// prove how the build pipeline will classify it. This is the
    /// load-bearing property: the overlay's `paths` must round-trip to the
    /// SAME classification the materializer intended.
    fn classify(module_src: &str) -> zfb_render::paths_extract::PathsExtraction {
        zfb_render::paths_extract::extract_paths(module_src, "overlay.tsx").unwrap()
    }

    #[test]
    fn dynamic_overlay_inlines_literal_paths_no_v8() {
        // A package entrypoint whose paths() is a literal-returning function.
        let (_d, entry) = entrypoint_with(
            r#"export function paths() {
  return [{ params: { slug: "a" } }, { params: { slug: "b" } }];
}
export default function Page() { return null; }
"#,
        );
        let m = synthesize_dynamic_overlay_module(&entry, None).unwrap();
        // Default re-exported; literal paths() inlined top-level.
        assert!(m.contains("export { default } from"));
        assert!(
            m.contains("export function paths()") && m.contains("return ["),
            "literal paths must be inlined as a literal-returning function; got:\n{m}"
        );
        // The crux: the overlay re-classifies Literal, so the pipeline
        // expands statically with NO V8 round-trip.
        match classify(&m) {
            zfb_render::paths_extract::PathsExtraction::Literal(json) => {
                let s = serde_json::to_string(&json).unwrap();
                assert!(s.contains("\"slug\":\"a\"") && s.contains("\"slug\":\"b\""));
            }
            other => panic!("expected Literal classification, got {other:?}"),
        }
    }

    #[test]
    fn dynamic_overlay_wraps_runtime_paths_for_v8_defer() {
        // A runtime paths() (calls getCollection) → NonLiteral in the package.
        let (_d, entry) = entrypoint_with(
            r#"import { getCollection } from "@takazudo/zfb-runtime";
export async function paths() {
  const items = await getCollection("docs");
  return items.map((it) => ({ params: { slug: it.slug } }));
}
export default function Page() { return null; }
"#,
        );
        let m = synthesize_dynamic_overlay_module(&entry, None).unwrap();
        // Imports the package's real paths under an alias and re-declares a
        // top-level wrapper.
        assert!(
            m.contains("import { paths as __zfb_pkg_paths } from"),
            "runtime paths must be imported under an alias; got:\n{m}"
        );
        assert!(
            m.contains("export async function paths()") && m.contains("__zfb_pkg_paths()"),
            "runtime wrapper must call the imported real paths; got:\n{m}"
        );
        // The crux: the overlay re-classifies NonLiteral, so the pipeline
        // DEFERS it to the V8 `__paths__` worker (which runs the real paths).
        assert!(
            matches!(
                classify(&m),
                zfb_render::paths_extract::PathsExtraction::NonLiteral { .. }
            ),
            "runtime wrapper must classify NonLiteral (deferred to V8); got: {:?}",
            classify(&m)
        );
    }

    #[test]
    fn dynamic_overlay_missing_paths_yields_missing_for_hard_error() {
        // A dynamic package route whose entrypoint forgot paths().
        let (_d, entry) = entrypoint_with("export default function Page() { return null; }\n");
        let m = synthesize_dynamic_overlay_module(&entry, None).unwrap();
        // No `paths` inlined → the pipeline's extractor returns Missing →
        // the canonical hard error fires (parity with a pages/ route).
        assert!(
            !m.contains("export function paths") && !m.contains("export async function paths"),
            "no paths must be inlined when the package omits it; got:\n{m}"
        );
        assert!(
            matches!(
                classify(&m),
                zfb_render::paths_extract::PathsExtraction::Missing
            ),
            "overlay must classify Missing so the hard error fires; got: {:?}",
            classify(&m)
        );
    }

    #[test]
    fn dynamic_overlay_defers_on_parse_error_for_parity() {
        // Syntax the SWC static extractor cannot parse. The user-page flow
        // (`try_expand_one`) treats a parse error as a DEFER (runtime V8),
        // NOT a hard error — only a genuinely-Missing paths() hard-errors.
        // The materializer must match: synthesize the runtime wrapper, not
        // abort. (codex P2.)
        let broken = "export function paths( { return [ ;\nexport default function Page() {}\n";
        // Precondition: this source really IS a parse error for the extractor
        // (so the test exercises the Err branch, not Missing/NonLiteral).
        assert!(
            zfb_render::paths_extract::extract_paths(broken, "broken.tsx").is_err(),
            "test precondition: the chosen source must be a parse error"
        );
        let (_d, entry) = entrypoint_with(broken);
        // Must NOT return Err — a parse error defers, it does not abort.
        let m = synthesize_dynamic_overlay_module(&entry, None)
            .expect("a parse error must defer to the runtime wrapper, not hard-error");
        assert!(
            m.contains("import { paths as __zfb_pkg_paths } from")
                && m.contains("export async function paths()"),
            "parse error must synthesize the runtime wrapper (deferred to V8); got:\n{m}"
        );
        // And the wrapper classifies NonLiteral so the pipeline defers it.
        assert!(matches!(
            classify(&m),
            zfb_render::paths_extract::PathsExtraction::NonLiteral { .. }
        ));
    }

    #[test]
    fn dynamic_overlay_threads_prerender_hint() {
        let (_d, entry) = entrypoint_with(
            r#"export function paths() { return [{ params: { slug: "a" } }]; }
export default function Page() { return null; }
"#,
        );
        let m = synthesize_dynamic_overlay_module(&entry, Some(false)).unwrap();
        assert!(m.contains("export const prerender = false;"));
        assert!(m.contains("export const frontmatter = {}"));
    }

    #[test]
    fn materializes_dynamic_literal_route_into_overlay() {
        let tmp = tempfile::tempdir().unwrap();
        let pages = tmp.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();

        // The package's dynamic entrypoint with a literal paths().
        let pkg_dir = tmp.path().join("pkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        let entry = pkg_dir.join("blog.tsx");
        std::fs::write(
            &entry,
            r#"export function paths() { return [{ params: { slug: "hello" } }]; }
export default function Page() { return null; }
"#,
        )
        .unwrap();

        let r = InjectedRoute {
            pattern: "/blog/[slug]".into(),
            entrypoint: entry,
            plugin: "preset".into(),
            prerender: None,
        };
        let res = resolve_build_pages_root(&pages, std::slice::from_ref(&r)).unwrap();
        assert!(res.guard.is_some());
        let overlay = res.build_pages_root.join("blog").join("[slug].tsx");
        assert!(overlay.is_file(), "dynamic overlay module must be written");
        let body = std::fs::read_to_string(&overlay).unwrap();
        assert!(body.contains("export function paths()"));
        assert!(body.contains("\"hello\""));
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
