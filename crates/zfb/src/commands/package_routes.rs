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
//! comparing route shape keys.
//!
//! Package-vs-package collisions: an EXACT-string pattern duplicate is a
//! hard error at registration (`InjectRouteConflict`), but two textually
//! DIFFERENT patterns with the same shape (`/blog/[slug]` vs `/blog/[id]`)
//! pass registration — so the survivor loop here also dedupes package
//! shape keys and hard-errors naming both plugins + patterns before any
//! overlay write (rather than letting the merged scan `AmbiguousShape`
//! with opaque overlay temp paths).
//!
//! On a case-INSENSITIVE filesystem (macOS/Windows) a user `pages/About.tsx`
//! and a package `/about` have distinct shape keys (`/About` vs `/about`)
//! but map to the same on-disk file, so the materialiser additionally
//! guards each overlay write with a `dest.exists()` check: an existing
//! USER file means user-wins (drop), an existing package OVERLAY file means
//! a package-vs-package case-only collision (hard error).
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
    /// Absolute path of the package's REAL entrypoint module (#1191 review,
    /// codex P1). The islands seed walks this real file — not the overlay
    /// copy — so a `"use client"` component the package page imports resolves
    /// against the entrypoint's real location and ships in the islands bundle.
    pub(crate) entrypoint: PathBuf,
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
    //
    // `pkg_shape_keys` tracks the shape keys already claimed by a SURVIVING
    // package route so a package-vs-package shape duplicate (e.g. preset A
    // `/blog/[slug]` + preset B `/blog/[id]`, both shape `/blog/:*`) is
    // caught HERE — naming both plugins + patterns — rather than escaping to
    // the merged scan, which would `AmbiguousShape` with opaque overlay temp
    // paths. (The registration guard only catches EXACT-string pattern
    // duplicates, so shape-equal/textually-different patterns reach here.)
    let mut survivors: Vec<(&InjectedRoute, PathBuf, String)> = Vec::new();
    let mut pkg_shape_keys: std::collections::HashMap<String, (&str, &str)> =
        std::collections::HashMap::new();
    for route in injected_routes {
        let pages_rel = pattern_to_pages_rel(&route.pattern).with_context(|| {
            format!(
                "package route `{}` (from plugin `{}`) is not a valid pages/ route pattern",
                route.pattern, route.plugin
            )
        })?;
        // A pattern whose final segment ends in `.client` (e.g. `/foo.client`)
        // derives a `foo.client.tsx` overlay path, which `is_client_script_file`
        // treats as a client-script entry — the scanner SKIPS those (scan.rs),
        // so the route would be silently dropped (no page) AND, on a
        // case-insensitive FS, could clobber a copied user `pages/foo.client.tsx`.
        // Reject loudly instead, naming the plugin + pattern (mirrors the
        // trailing-`index` rejection in `pattern_to_pages_rel`).
        if zfb_types::is_client_script_file(&pages_rel) {
            return Err(anyhow!(
                "package route `{}` (from plugin `{}`) derives the pages/ path `{}`, which \
                 matches the `*.client.{{ts,tsx,js,jsx}}` client-script contract — the route \
                 scanner skips those, so the route would silently produce no page. A package \
                 page route must not end in a `.client` segment.",
                route.pattern,
                route.plugin,
                pages_rel.display()
            ));
        }
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
        if let Some((first_plugin, first_pattern)) = pkg_shape_keys.get(&shape_key) {
            return Err(anyhow!(
                "package routes collide on the same route shape `{}`: `{}` (from plugin `{}`) \
                 and `{}` (from plugin `{}`). Two package routes may not resolve to the same \
                 shape (dynamic params like `[slug]`/`[id]` are shape-equal). Rename one pattern \
                 or have a single plugin own the route.",
                shape_key,
                first_pattern,
                first_plugin,
                route.pattern,
                route.plugin
            ));
        }
        pkg_shape_keys.insert(
            shape_key.clone(),
            (route.plugin.as_str(), route.pattern.as_str()),
        );
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

    // Track the absolute overlay paths this loop has WRITTEN (lowercased on
    // case-insensitive comparison) so a package-vs-package case-only path
    // collision (`/foo` + `/Foo`, distinct shape keys but the same on-disk
    // file on macOS/Windows) is caught loudly instead of one route silently
    // truncating the other.
    let mut written_dests: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut materialized = Vec::with_capacity(survivors.len());
    for (route, pages_rel, _shape_key) in &survivors {
        let dest = overlay_pages.join(pages_rel);
        // Precedence guard (#1191 fix-A [14]): on a case-INSENSITIVE,
        // case-preserving filesystem (macOS/Windows) a user `pages/About.tsx`
        // (shape `/About`) and a package `/about` (shape `/about`) have
        // DIFFERENT shape keys — so the user-wins pre-scan drop above does not
        // fire — yet `about.tsx` resolves to the same on-disk inode as the
        // copied `About.tsx`. A blind `fs::write` would truncate the user's
        // page and ship the package content under it (silent precedence
        // INVERSION, divergent across case-sensitive Linux CI). Detect the
        // collision via `dest.exists()` (which matches case-insensitively on
        // those filesystems) and:
        //   - if the existing file was a copied USER page → drop the package
        //     route, user wins (matches the documented precedence + the
        //     pre-scan info message);
        //   - if it was a previously-written package OVERLAY module → hard
        //     error naming both, since two package routes cannot share a file.
        let dest_key = dest.to_string_lossy().to_lowercase();
        if dest.exists() {
            if let Some(prev_pattern) = written_dests.get(&dest_key) {
                return Err(anyhow!(
                    "package routes `{}` and `{}` (from plugin `{}`) resolve to the same overlay \
                     file `{}` on a case-insensitive filesystem. Rename one pattern so the two \
                     do not differ only by letter case.",
                    prev_pattern,
                    route.pattern,
                    route.plugin,
                    pages_rel.display()
                ));
            }
            crate::output::info(format!(
                "package route `{}` (from plugin `{}`) would overwrite the user pages/ file `{}` \
                 (case-insensitive filesystem match); a user page wins — skipping",
                route.pattern,
                route.plugin,
                pages_rel.display()
            ));
            continue;
        }
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
        written_dests.insert(dest_key, route.pattern.clone());
        materialized.push(MaterializedRoute {
            pattern: route.pattern.clone(),
            pages_rel: pages_rel.clone(),
            entrypoint: route.entrypoint.clone(),
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
///
/// Uses `zfb_router::user_page_shape_keys`, which — unlike `scan_pages` —
/// also records the shape keys of DYNAMIC `.md`/`.html` pages the scanner
/// skips as v1-unsupported. Those are still routes the user authored, so
/// including them lets the user-wins pre-scan drop a same-shape package
/// route instead of letting it silently shadow the user's page (#1201).
fn collect_user_pages_shape_keys(real_pages_dir: &Path) -> Result<HashSet<String>> {
    // Package routes intentionally allow an absent user `pages/` — keep the
    // empty early-return so an absent dir is "no collisions", not an error.
    if !real_pages_dir.is_dir() {
        return Ok(HashSet::new());
    }
    // `user_page_shape_keys` (NOT `scan_pages`) so a user's DYNAMIC `.md`/`.html`
    // page — which `scan_pages` skips as v1-unsupported — still contributes its
    // shape key here. Without it, a same-shape package route would NOT be dropped
    // by the user-wins pre-scan and would silently shadow the user's page (#1201).
    zfb_router::user_page_shape_keys(real_pages_dir)
        .map_err(|e| anyhow!("user pages/ scan failed: {e}"))
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
/// Why both are inlined (rather than re-exported): the overlay re-exports
/// only the package page's `default`, so the page's own `prerender` /
/// `frontmatter` exports are invisible to the syntactic extractor — to give
/// the overlay a `prerender` flag at all, it must be physically present
/// top-level. As of #1198 a lone inlined `export const prerender = false`
/// (no `frontmatter`) IS surfaced — `build_prerender_map` records it on the
/// `MissingFrontmatter` path — so the empty `frontmatter` is no longer
/// strictly required for the flag to reach the gate. We still inline it
/// alongside `prerender` so the overlay presents a complete, conventional
/// page shape. With no hint, both are omitted → SSG default (the desired
/// default for package routes).
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
/// the `output: static` gate via `build_prerender_map` — sees the flag; a
/// re-export of the package page's own exports would be invisible to the
/// syntactic extractor (#1193). The empty `frontmatter` is inlined as a
/// conventional sibling: as of #1198 a lone `prerender` is honored even
/// without it, but keeping it presents a complete page shape.
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

/// Recursively copy a directory tree (files only) into the overlay,
/// mirroring the router/bundler walk policy EXACTLY so the overlay's
/// routed/bundled set is byte-identical to the no-overlay baseline.
///
/// The walk uses [`walkdir::WalkDir`] with `follow_links(false)`, the same
/// policy as the route scanner (`zfb-router` `scan_pages`) and the
/// bundler's pages walk. The consequences, all intentional parity with the
/// baseline (#1191 fix-A):
///
/// - **Symlinked subdirs are NOT descended.** `follow_links(false)` yields a
///   symlinked dir as a symlink entry and does not recurse into it — exactly
///   what the scanner does, so a `pages/shared -> ../pkg/pages` symlink
///   contributes no routes whether or not a package route is present. (The
///   old hand-rolled recursion dereferenced symlinked dirs, silently growing
///   the route table the moment a preset registered a route, and infinitely
///   recursing on a symlink cycle.)
/// - **Dangling / broken symlinks are skipped, not stat-errored.** A broken
///   symlink is neither a file nor a dir, so it is ignored — matching the
///   scanner, which simply skips non-file entries. (The old code `stat`'d the
///   target and hard-failed the whole build on a dangling link.)
/// - **Symlinked FILES are not dereferenced.** They are non-file entries
///   under `follow_links(false)`, so they are skipped too — again matching
///   the scanner's `entry.file_type().is_file()` gate. Real (non-symlink)
///   files are copied as content, which is all esbuild needs from the
///   overlay seed.
///
/// Used to seed the overlay with the user's real `pages/`.
fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest).with_context(|| format!("creating dir {}", dest.display()))?;
    for entry in walkdir::WalkDir::new(src).follow_links(false) {
        let entry = entry
            .with_context(|| format!("walking user pages/ {} for overlay copy", src.display()))?;
        // Skip everything that is not a regular file: directories are created
        // lazily from each file's parent below, and symlinks (dangling,
        // file-target, or dir-target) are ignored to match the scanner's
        // `follow_links(false)` + `is_file()` policy.
        if !entry.file_type().is_file() {
            continue;
        }
        let from = entry.path();
        let rel = from.strip_prefix(src).map_err(|_| {
            anyhow!(
                "overlay copy walked outside the user pages/ root: {}",
                from.display()
            )
        })?;
        let to = dest.join(rel);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating overlay dir {}", parent.display()))?;
        }
        std::fs::copy(from, &to)
            .with_context(|| format!("copying {} -> {}", from.display(), to.display()))?;
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
    fn user_dynamic_md_wins_over_same_shape_package_route() {
        // #1201: a user's DYNAMIC `.md` page is skipped by `scan_pages` (v1-
        // unsupported) but still OWNS the `/docs/:*` shape. A same-shape package
        // route must be dropped (user-wins), not silently shadow the user's page.
        let tmp = tempfile::tempdir().unwrap();
        let pages = tmp.path().join("pages");
        let docs = pages.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("[slug].md"), "# user docs page\n").unwrap();

        let routes = vec![route("/docs/[id]", "/pkg/docs-id.tsx")];
        let res = resolve_build_pages_root(&pages, &routes).unwrap();
        assert!(
            res.materialized.is_empty(),
            "package route at a user dynamic .md page's shape must be dropped"
        );
    }

    #[test]
    fn user_dynamic_html_wins_over_same_shape_package_route() {
        // Same as above for a dynamic `.html` page.
        let tmp = tempfile::tempdir().unwrap();
        let pages = tmp.path().join("pages");
        let docs = pages.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("[slug].html"), "<h1>user</h1>\n").unwrap();

        let routes = vec![route("/docs/[id]", "/pkg/docs-id.tsx")];
        let res = resolve_build_pages_root(&pages, &routes).unwrap();
        assert!(
            res.materialized.is_empty(),
            "package route at a user dynamic .html page's shape must be dropped"
        );
    }

    #[test]
    fn user_optional_catchall_md_wins_over_bare_url_package_route() {
        // #1201 (codex review): a user's optional-catchall `.md` page
        // (`pages/docs/[[...rest]].md`) serves the bare `/docs` URL too. A package
        // route at the bare `/docs` must be dropped (user-wins), not silently
        // shadow the zero-segment URL the optional catchall owns.
        let tmp = tempfile::tempdir().unwrap();
        let pages = tmp.path().join("pages");
        let docs = pages.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("[[...rest]].md"), "# user docs\n").unwrap();

        let routes = vec![route("/docs", "/pkg/docs.tsx")];
        let res = resolve_build_pages_root(&pages, &routes).unwrap();
        assert!(
            res.materialized.is_empty(),
            "package route at the bare URL of a user optional-catchall .md page must be dropped"
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

    // ── fix-A [3][7]: `.client`-suffixed package route is rejected loudly ──

    #[test]
    fn client_suffixed_package_route_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let pages = tmp.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();

        // `/foo.client` → `foo.client.tsx`, which the scanner skips as a
        // client-script entry — it would silently produce no page. Reject.
        let routes = vec![route("/foo.client", "/pkg/foo.tsx")];
        let msg = match resolve_build_pages_root(&pages, &routes) {
            Ok(_) => panic!("a `.client`-suffixed package route must be rejected"),
            Err(e) => format!("{e:#}"),
        };
        assert!(
            msg.contains("client-script") && msg.contains("/foo.client"),
            "error must name the client-script contract and the pattern; got:\n{msg}"
        );
    }

    #[test]
    fn client_suffixed_package_route_does_not_clobber_user_client_script() {
        // A user `pages/widget.client.tsx` (a real client script) must NOT be
        // overwritten by a `/widget.client` package route. The route is
        // rejected before the overlay is even built, so the user file is safe.
        let tmp = tempfile::tempdir().unwrap();
        let pages = tmp.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();
        let user_client = pages.join("widget.client.tsx");
        let user_body = "export default function Widget() { return null; } // USER\n";
        std::fs::write(&user_client, user_body).unwrap();

        let routes = vec![route("/widget.client", "/pkg/widget.tsx")];
        assert!(resolve_build_pages_root(&pages, &routes).is_err());
        // User's real client script untouched.
        assert_eq!(std::fs::read_to_string(&user_client).unwrap(), user_body);
    }

    // ── fix-A [16]: package-vs-package shape duplicate hard-errors here ──

    #[test]
    fn package_vs_package_shape_duplicate_hard_errors_with_attribution() {
        let tmp = tempfile::tempdir().unwrap();
        let pages = tmp.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();

        // Two presets register shape-equal but textually-different patterns.
        let a = InjectedRoute {
            pattern: "/blog/[slug]".into(),
            entrypoint: PathBuf::from("/pkg-a/blog.tsx"),
            plugin: "preset-a".into(),
            prerender: None,
        };
        let b = InjectedRoute {
            pattern: "/blog/[id]".into(),
            entrypoint: PathBuf::from("/pkg-b/blog.tsx"),
            plugin: "preset-b".into(),
            prerender: None,
        };
        let msg = match resolve_build_pages_root(&pages, &[a, b]) {
            Ok(_) => panic!("package-vs-package shape duplicate must hard-error"),
            Err(e) => format!("{e:#}"),
        };
        // Both plugins AND both patterns must be named (no opaque temp paths).
        assert!(
            msg.contains("preset-a")
                && msg.contains("preset-b")
                && msg.contains("/blog/[slug]")
                && msg.contains("/blog/[id]"),
            "shape-duplicate error must name both plugins + patterns; got:\n{msg}"
        );
    }

    // ── fix-A [14]: case-insensitive user-vs-package precedence ──

    #[cfg(unix)]
    #[test]
    fn case_insensitive_user_page_wins_over_package_route() {
        // On a case-insensitive FS (macOS/Windows) `pages/About.tsx` and a
        // package `/about` map to the same file. The user page must win
        // (deterministic: either dropped here, or — on a case-SENSITIVE FS —
        // both survive as distinct routes). Either outcome must NOT silently
        // overwrite the user's page content with the package re-export.
        let tmp = tempfile::tempdir().unwrap();
        let pages = tmp.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();
        let user_about = pages.join("About.tsx");
        let user_body = "export default function About() { return null; } // USER\n";
        std::fs::write(&user_about, user_body).unwrap();

        let routes = vec![route("/about", "/pkg/about.tsx")];
        let res = resolve_build_pages_root(&pages, &routes).unwrap();

        // The user's About.tsx content (in the overlay copy or the real dir)
        // must remain the USER page, never the package re-export.
        let about_in_overlay = res.build_pages_root.join("About.tsx");
        if about_in_overlay.is_file() {
            assert_eq!(
                std::fs::read_to_string(&about_in_overlay).unwrap(),
                user_body,
                "user page content must not be replaced by the package re-export"
            );
        }
        // And the lowercased `about.tsx` overlay module must NOT carry the
        // package re-export on a case-insensitive FS (it would be the same
        // inode as About.tsx). On a case-sensitive FS a distinct about.tsx may
        // exist with package content — that is the documented divergence and
        // both routes ship; what matters is the user file is never destroyed.
        let lower = res.build_pages_root.join("about.tsx");
        if lower.is_file() {
            // Same inode as About.tsx on case-insensitive FS → must be USER.
            // Distinct file on case-sensitive FS → package content is fine.
            let body = std::fs::read_to_string(&lower).unwrap();
            let same_inode = match (
                std::fs::metadata(&user_about).ok(),
                std::fs::metadata(&lower).ok(),
            ) {
                (Some(a), Some(b)) => {
                    use std::os::unix::fs::MetadataExt;
                    a.ino() == b.ino()
                }
                _ => false,
            };
            if same_inode {
                assert_eq!(body, user_body, "case-insensitive FS: user page wins");
            }
        }
    }

    // ── fix-A [1][2][4]: copy_dir_recursive symlink policy mirrors scanner ──

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_under_pages_is_skipped_not_errored() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let pages = tmp.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();
        std::fs::write(pages.join("index.tsx"), "export default () => null;").unwrap();
        // A dangling symlink (target does not exist).
        symlink(pages.join("missing-target.tsx"), pages.join("broken.tsx")).unwrap();

        // A package route forces the overlay copy to run over the real pages/.
        let routes = vec![route("/preset-page", "/pkg/preset-page.tsx")];
        let res = resolve_build_pages_root(&pages, &routes)
            .expect("a dangling symlink under pages/ must not fail the overlay copy");
        // The real file copied; the dangling symlink skipped (parity w/ scanner).
        assert!(res.build_pages_root.join("index.tsx").is_file());
        assert!(
            !res.build_pages_root.join("broken.tsx").exists(),
            "dangling symlink must be skipped, not materialised"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_subdir_under_pages_is_not_recursed() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let pages = tmp.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();
        std::fs::write(pages.join("index.tsx"), "export default () => null;").unwrap();
        // A real dir OUTSIDE pages/ with a page in it, symlinked under pages/.
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("leaked.tsx"), "export default () => null;").unwrap();
        symlink(&outside, pages.join("shared")).unwrap();

        let routes = vec![route("/preset-page", "/pkg/preset-page.tsx")];
        let res = resolve_build_pages_root(&pages, &routes).unwrap();
        // follow_links(false): the symlinked subdir is NOT descended, matching
        // the scanner — so `leaked.tsx` does NOT appear in the overlay.
        assert!(res.build_pages_root.join("index.tsx").is_file());
        assert!(
            !res.build_pages_root
                .join("shared")
                .join("leaked.tsx")
                .exists(),
            "symlinked subdir must not be recursed (parity with follow_links(false))"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cycle_under_pages_does_not_infinitely_recurse() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let pages = tmp.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();
        std::fs::write(pages.join("index.tsx"), "export default () => null;").unwrap();
        // A self-referential symlink cycle: pages/loop -> pages (an ancestor).
        symlink(&pages, pages.join("loop")).unwrap();

        let routes = vec![route("/preset-page", "/pkg/preset-page.tsx")];
        // Must terminate (follow_links(false) never descends the symlink).
        let res = resolve_build_pages_root(&pages, &routes)
            .expect("a symlink cycle must not crash/hang the overlay copy");
        assert!(res.build_pages_root.join("index.tsx").is_file());
    }
}
