//! Granularity policy: how a single filesystem change maps to a rebuild
//! plan.
//!
//! See the crate-level docs for a summary. This module turns a raw
//! [`zfb_watcher::Change`] path into a [`PathClass`], and the
//! [`GranularityPolicy`] then folds that class into a [`crate::RebuildPlan`].
//!
//! The policy is deliberately *defensive* — when in doubt we err on the
//! side of "rebuild more, not less". A misclassified path produces a
//! correct (if slower) build; an over-aggressively-narrowed path produces
//! a stale page. Of those two failure modes we prefer the former.
//!
//! ## Rationale (defensible-policy short version)
//!
//! - **Pages and components live in the dependency graph.** For these
//!   paths we trust the graph: a change to `components/Header.tsx` dirties
//!   exactly the pages that import it (plus the implicit self-edge, so
//!   editing a page source dirties that page).
//! - **Content lives in the graph too.** `content/foo.md` is reached via
//!   a content collection, which the resolver records as a dep. So
//!   editing one markdown file also routes through the graph and only
//!   touches its consumer page(s).
//! - **CSS sources are treated separately.** A CSS source change does not
//!   re-render pages — the renderer's HTML doesn't change, only the
//!   hashed asset URL might. If the CSS pipeline's hash changes and
//!   pages embed that URL, the orchestrator rewrites only the affected
//!   pages on the *next* render — but in this version we keep things
//!   simple and re-render dirty pages whenever CSS-source hash changes
//!   are reflected back into the graph (a future micro-optimisation).
//! - **Islands components are treated separately too.** Editing a
//!   `"use client"` component triggers an islands re-bundle. It does not
//!   trigger a full re-render: the rendered HTML embeds the bundle URL,
//!   not the bundle bytes, and the URL only changes when the islands set
//!   itself changes (add/remove a `"use client"`, or any of the bundled
//!   modules' bytes change).
//! - **Globals are nuclear.** `zfb.config.ts` and the like force a full
//!   rebuild. The dependency graph already exposes this via
//!   `DependencyGraph::is_global`.
//!
//! ## Classification heuristic
//!
//! Path classification is purely *path-pattern based* — it does not read
//! file contents. The orchestrator combines this with the dependency
//! graph (which *does* know real edges) to produce the final plan. So:
//!
//! - We do **not** decide here whether a `.tsx` is an islands component;
//!   we decide whether it's *inside the islands source roots* and let
//!   the orchestrator/graph decide whether the islands set actually
//!   changed.
//! - We do **not** decide here whether a `.tsx` is a page; we let the
//!   graph answer that via `dirty_pages`.

use std::path::{Component, Path, PathBuf};

/// Coarse-grained classification of a changed file. Drives which
/// sub-pipelines the orchestrator considers running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathClass {
    /// Registered as a "global" file in the dependency graph (e.g.
    /// `zfb.config.ts`). Forces a full rebuild.
    Global,

    /// A page source under `pages/**` — re-render this page (the graph's
    /// self-edge will surface it as dirty).
    Page,

    /// A `.md` / `.mdx` content entry under `content/**`. The graph's
    /// content-collection edges decide which pages re-render.
    Content,

    /// A non-page TSX / TS / JSX / JS file (layout, component, lib).
    /// The graph decides which pages re-render. May also be an islands
    /// component — that decision is made by the islands sub-pipeline.
    Module,

    /// A CSS source under `styles/**` (or anywhere with a `.css`
    /// extension). Trigger CSS pipeline rerun.
    Style,

    /// A static data file (JSON / TOML / YAML under `data/**`). Treated
    /// like a `Module`: the graph's data-edges decide consumers.
    Data,

    /// A static asset under `public/**`. Doesn't dirty any page on its
    /// own — the asset pipeline copies it into `dist/` directly, and the
    /// HTML reference is by URL not by content.
    Asset,

    /// Path didn't match any known root. Defensively, the orchestrator
    /// treats this as "consult the graph anyway" — many editors write to
    /// project-relative paths we didn't classify (`tsconfig.json`,
    /// `package.json`, …) but the graph will return an empty dirty set
    /// and nothing happens.
    Unclassified,

    /// A file change outside the project root that came in via the
    /// `extraWatchPaths` channel (issue #368). The user explicitly
    /// opted in to watching that path; the watcher fires on ANY change
    /// there regardless of extension, so we trigger a conservative
    /// rebuild instead of consulting the graph (which has no edges for
    /// out-of-root files and would silently no-op for anything except
    /// the whitelisted extensions — e.g. `logo.png`, `schema.graphql`,
    /// `*.lock`). Deep-review regression fix (PR #376).
    External,
}

/// Classify a path against the standard zfb project layout.
///
/// Pattern-based, not content-based. The lookup is purely about the path
/// shape so it stays cheap and predictable.
///
/// `is_global` is consulted first: it lets the dependency graph extend
/// the global set at runtime without changing this function.
///
/// `project_root` is the absolute path to the project's root directory.
/// Notify hands us absolute file paths, so without anchoring at the root
/// a project located under e.g. `/home/me/pages/myproj/...` would have
/// every change misclassified as `PathClass::Page` — the ancestor
/// `pages` segment would match before we reached the project's own
/// `components/Header.tsx`. Stripping `project_root` first ensures we
/// only inspect components inside the project.
pub fn classify_change(
    path: &Path,
    project_root: &Path,
    is_global: impl FnOnce(&Path) -> bool,
) -> PathClass {
    classify_change_with_content_roots(path, project_root, &[], is_global)
}

/// Like [`classify_change`], but consults `content_roots` (project-relative
/// roots of configured content collections, e.g. `src/mdx/notes` from a
/// consumer's `zfb.config.ts`) **before** the standard root-segment walk.
///
/// Without this, a collection configured outside `content/` misclassifies:
/// `src/mdx/notes/foo.mdx` matches the `src` segment in the walk and comes
/// back as [`PathClass::Module`] — which both misses the content semantics
/// and wastefully triggers an islands re-bundle (`src` is a default islands
/// root). The global check still wins (a globally-registered file under a
/// collection root must stay nuclear).
///
/// The content-root override is **gated on content-shaped extensions**
/// (`md` / `mdx` — the same set [`classify_by_extension`] maps to
/// [`PathClass::Content`]). A co-located non-entry file under a collection
/// root — e.g. an islands `Counter.tsx` or a `theme.css` — must NOT be
/// swept up as `Content`; it falls through to the normal root-segment walk
/// so it keeps classifying as [`PathClass::Module`] (preserving islands
/// invalidation) or [`PathClass::Style`] (preserving the CSS rerun).
pub fn classify_change_with_content_roots(
    path: &Path,
    project_root: &Path,
    content_roots: &[PathBuf],
    is_global: impl FnOnce(&Path) -> bool,
) -> PathClass {
    if is_global(path) {
        return PathClass::Global;
    }

    let lower_ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());

    // Only redirect content-shaped files (md / mdx) under a configured
    // collection root to `Content`. Co-located `.tsx` / `.css` / other
    // files must fall through to the normal root-segment walk so islands
    // re-bundling and CSS reruns still fire.
    let is_content_shaped = matches!(lower_ext.as_deref(), Some("md") | Some("mdx"));
    if is_content_shaped && !content_roots.is_empty() {
        // Match against the project-relative form when the event path is
        // inside the root; fall back to a direct prefix match so an
        // absolute (out-of-tree) collection root still classifies.
        let rel = path.strip_prefix(project_root).ok();
        let under_content_root = content_roots.iter().any(|root| {
            rel.map(|r| r.starts_with(root)).unwrap_or(false) || path.starts_with(root)
        });
        if under_content_root {
            return PathClass::Content;
        }
    }

    // Out-of-root paths (the `extraWatchPaths` channel — issue #368)
    // must NOT walk their absolute components looking for in-tree root
    // names. An external path like `/srv/shared/public/foo.md` would
    // otherwise match the `public` segment and silently classify as
    // `Asset` (no rebuild fires), making the advertised feature
    // unreliable for any user whose external tree happens to nest
    // under a directory called `public`, `styles`, `components`, etc.
    //
    // For out-of-root paths we skip the root-segment scan entirely and
    // try the extension sniff. Whitelisted extensions still classify
    // as Content/Style/Module/Data (the Page/Module/Content/Data branch
    // in `plan_for_changes` already falls back to `PageSelection::All`
    // when the graph has no edges, so those re-render).
    //
    // Non-whitelisted extensions (`.png`, `.graphql`, `.lock`, …) used
    // to classify as `Unclassified`, which `plan_for_changes` then
    // silently no-op'd — the user explicitly opted in to watching that
    // path but the watcher tick produced zero rebuild. Re-route those
    // through `External` instead so the orchestrator triggers a
    // conservative full rebuild. Deep-review fix (PR #376).
    let project_relative = match path.strip_prefix(project_root) {
        Ok(rel) => rel,
        Err(_) => {
            let class = classify_by_extension(lower_ext.as_deref());
            return if class == PathClass::Unclassified {
                PathClass::External
            } else {
                class
            };
        }
    };
    let mut comps = project_relative.components().peekable();

    // Skip leading `/` / drive prefixes / `RootDir` components (only
    // possible when the relative path itself started with one — rare
    // but defensible).
    while let Some(c) = comps.peek() {
        if matches!(c, Component::Prefix(_) | Component::RootDir) {
            comps.next();
        } else {
            break;
        }
    }

    // Walk components looking for the first root we recognise.
    for comp in comps {
        let s = match comp {
            Component::Normal(s) => s.to_string_lossy().into_owned(),
            _ => continue,
        };
        match s.as_str() {
            "pages" => return PathClass::Page,
            "content" => return PathClass::Content,
            "styles" => return PathClass::Style,
            "data" => return PathClass::Data,
            "public" => return PathClass::Asset,
            "components" | "layouts" | "lib" | "src" => {
                // Co-located CSS inside a module root (e.g.
                // `components/Button.css`) is still a stylesheet, not
                // a JS module — without this carve-out the rebuild
                // plan never sets `rerun_css` and the new bytes never
                // reach `dist/assets/`.
                if lower_ext.as_deref() == Some("css") {
                    return PathClass::Style;
                }
                return PathClass::Module;
            }
            _ => {}
        }
    }

    // Fall back to extension sniffing.
    classify_by_extension(lower_ext.as_deref())
}

/// Classify a path by its extension alone, without any directory-name
/// inspection. Shared between the in-tree root-segment-walk fallback
/// and the out-of-root extra-watch-path branch.
fn classify_by_extension(ext: Option<&str>) -> PathClass {
    match ext {
        Some("css") => PathClass::Style,
        Some("md") | Some("mdx") => PathClass::Content,
        Some("ts") | Some("tsx") | Some("js") | Some("jsx") => PathClass::Module,
        Some("json") | Some("toml") | Some("yaml") | Some("yml") => PathClass::Data,
        _ => PathClass::Unclassified,
    }
}

/// The granularity policy combines a [`PathClass`] with the dependency
/// graph to decide what sub-pipelines run.
///
/// This is a thin record of decisions; see [`crate::RebuildPlan`] for the
/// shape it folds into.
#[derive(Debug, Clone)]
pub struct GranularityPolicy {
    /// Treat any change under this list of relative roots as triggering
    /// the islands re-bundle. Defaults to `["components", "src"]` —
    /// callers can override with [`GranularityPolicy::with_islands_roots`].
    pub islands_roots: Vec<PathBuf>,

    /// Project-relative roots of configured content collections (each
    /// `collections[].path` from the consumer config). Changes under
    /// these roots classify as [`PathClass::Content`] ahead of the
    /// standard root-segment walk — see
    /// [`classify_change_with_content_roots`]. Empty by default (the
    /// hardcoded `content/` root in the walk keeps covering the
    /// conventional layout).
    pub content_roots: Vec<PathBuf>,
}

impl Default for GranularityPolicy {
    fn default() -> Self {
        Self {
            islands_roots: vec![PathBuf::from("components"), PathBuf::from("src")],
            content_roots: Vec::new(),
        }
    }
}

impl GranularityPolicy {
    /// Override the islands source roots. Used when the project's
    /// `"use client"` components live somewhere non-standard.
    pub fn with_islands_roots<I, P>(mut self, roots: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        self.islands_roots = roots.into_iter().map(Into::into).collect();
        self
    }

    /// Set the configured content-collection roots (chainable). See
    /// [`GranularityPolicy::content_roots`].
    pub fn with_content_roots<I, P>(mut self, roots: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        self.content_roots = roots.into_iter().map(Into::into).collect();
        self
    }

    /// Decide whether a `Module` change is inside an islands root.
    ///
    /// We do not parse the file here to check for the `"use client"`
    /// directive — that's the islands sub-pipeline's job (and it is
    /// stable enough to be re-run unconditionally on any module change
    /// inside an islands root: the scanner is fast and the bundler
    /// re-emits the same hashed asset if the islands set is unchanged).
    pub fn is_islands_candidate(&self, path: &Path) -> bool {
        for root in &self.islands_roots {
            if path_starts_with_segment(path, root) {
                return true;
            }
        }
        false
    }
}

/// True iff `path` contains `segment` as one of its components.
///
/// We don't require an absolute prefix match because filesystem watcher
/// events come back as absolute paths and the policy's "roots" are
/// project-relative.
fn path_starts_with_segment(path: &Path, segment: &Path) -> bool {
    let segment_components: Vec<_> = segment.components().collect();
    if segment_components.is_empty() {
        return false;
    }

    let path_components: Vec<_> = path.components().collect();
    if path_components.len() < segment_components.len() {
        return false;
    }

    // Slide the segment over path looking for a contiguous match.
    for start in 0..=path_components.len() - segment_components.len() {
        if path_components[start..start + segment_components.len()] == segment_components[..] {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never_global(_: &Path) -> bool {
        false
    }

    fn proj() -> &'static Path {
        Path::new("/proj")
    }

    /// Regression: a collection configured under `src/` (e.g.
    /// `src/mdx/notes` in a consumer's `zfb.config.ts`) classified as
    /// `Module` because the root-segment walk matched `src` — missing
    /// the content semantics and (since `src` is a default islands
    /// root) wastefully re-bundling islands on every entry edit.
    #[test]
    fn configured_content_root_wins_over_module_segment_walk() {
        let roots = vec![PathBuf::from("src/mdx/notes")];
        assert_eq!(
            classify_change_with_content_roots(
                Path::new("/proj/src/mdx/notes/foo.mdx"),
                proj(),
                &roots,
                never_global,
            ),
            PathClass::Content
        );
        // A sibling module file outside the collection root keeps the
        // normal Module classification.
        assert_eq!(
            classify_change_with_content_roots(
                Path::new("/proj/src/components/button.tsx"),
                proj(),
                &roots,
                never_global,
            ),
            PathClass::Module
        );
    }

    /// A co-located islands `.tsx` INSIDE the collection root must NOT be
    /// swept up as `Content` by the content-root override — the override is
    /// gated on content-shaped extensions (md / mdx). Otherwise the islands
    /// re-bundle the default `src` islands root would trigger gets skipped
    /// and the client bundle goes stale.
    #[test]
    fn content_root_does_not_swallow_colocated_tsx() {
        let roots = vec![PathBuf::from("src/mdx/notes")];
        assert_eq!(
            classify_change_with_content_roots(
                Path::new("/proj/src/mdx/notes/Counter.tsx"),
                proj(),
                &roots,
                never_global,
            ),
            PathClass::Module
        );
    }

    /// A co-located `.css` INSIDE the collection root must fall through to
    /// the normal root-segment walk and classify as `Style`, not `Content`
    /// — otherwise the CSS rerun for an edit to it is skipped and the
    /// styles go stale.
    #[test]
    fn content_root_does_not_swallow_colocated_css() {
        let roots = vec![PathBuf::from("src/mdx/notes")];
        assert_eq!(
            classify_change_with_content_roots(
                Path::new("/proj/src/mdx/notes/theme.css"),
                proj(),
                &roots,
                never_global,
            ),
            PathClass::Style
        );
    }

    /// The global check must stay ahead of the content-root check — a
    /// globally-registered file under a collection root is still
    /// nuclear.
    #[test]
    fn global_check_wins_over_configured_content_root() {
        let roots = vec![PathBuf::from("src/mdx")];
        assert_eq!(
            classify_change_with_content_roots(
                Path::new("/proj/src/mdx/zfb.config.ts"),
                proj(),
                &roots,
                |_| true,
            ),
            PathClass::Global
        );
    }

    /// An absolute (out-of-tree) collection root still classifies via
    /// the direct prefix fallback.
    #[test]
    fn absolute_content_root_classifies_via_prefix_fallback() {
        let roots = vec![PathBuf::from("/srv/shared/posts")];
        assert_eq!(
            classify_change_with_content_roots(
                Path::new("/srv/shared/posts/a.mdx"),
                proj(),
                &roots,
                never_global,
            ),
            PathClass::Content
        );
    }

    /// `classify_change` (no content roots) keeps its exact legacy
    /// behaviour — `src/**.mdx` still walks to `Module`.
    #[test]
    fn no_content_roots_preserves_legacy_module_walk() {
        assert_eq!(
            classify_change(
                Path::new("/proj/src/mdx/notes/foo.mdx"),
                proj(),
                never_global
            ),
            PathClass::Module
        );
    }

    #[test]
    fn classifies_pages() {
        assert_eq!(
            classify_change(Path::new("/proj/pages/index.tsx"), proj(), never_global),
            PathClass::Page
        );
    }

    #[test]
    fn non_html_pages_still_classify_as_page() {
        // Sub 49 regression: a `.tsx` page that emits non-HTML
        // (e.g. `pages/sitemap.xml.tsx`) is still a page. The
        // "pages/" segment match wins over the extension fallback,
        // so the orchestrator dispatches it to the page renderer
        // rather than misclassifying it as Data or anything else.
        assert_eq!(
            classify_change(
                Path::new("/proj/pages/sitemap.xml.tsx"),
                proj(),
                never_global,
            ),
            PathClass::Page,
        );
        assert_eq!(
            classify_change(
                Path::new("/proj/pages/api.v2.json.tsx"),
                proj(),
                never_global,
            ),
            PathClass::Page,
        );
        assert_eq!(
            classify_change(Path::new("/proj/pages/llms.txt.tsx"), proj(), never_global,),
            PathClass::Page,
        );
    }

    #[test]
    fn classifies_content() {
        assert_eq!(
            classify_change(Path::new("/proj/content/post.md"), proj(), never_global),
            PathClass::Content
        );
    }

    #[test]
    fn classifies_styles() {
        assert_eq!(
            classify_change(Path::new("/proj/styles/main.css"), proj(), never_global),
            PathClass::Style
        );
        assert_eq!(
            classify_change(Path::new("/proj/some/loose.css"), proj(), never_global),
            PathClass::Style
        );
    }

    #[test]
    fn classifies_modules() {
        assert_eq!(
            classify_change(Path::new("/proj/components/X.tsx"), proj(), never_global),
            PathClass::Module
        );
        assert_eq!(
            classify_change(Path::new("/proj/layouts/Y.tsx"), proj(), never_global),
            PathClass::Module
        );
    }

    #[test]
    fn classifies_data_and_assets() {
        assert_eq!(
            classify_change(Path::new("/proj/data/config.json"), proj(), never_global),
            PathClass::Data
        );
        assert_eq!(
            classify_change(Path::new("/proj/public/logo.svg"), proj(), never_global),
            PathClass::Asset
        );
    }

    #[test]
    fn unknown_root_falls_back_to_extension() {
        assert_eq!(
            classify_change(Path::new("/proj/whatever/x.css"), proj(), never_global),
            PathClass::Style
        );
        assert_eq!(
            classify_change(Path::new("/proj/whatever/x.tsx"), proj(), never_global),
            PathClass::Module
        );
        assert_eq!(
            classify_change(Path::new("/proj/whatever/x.md"), proj(), never_global),
            PathClass::Content
        );
        assert_eq!(
            classify_change(Path::new("/proj/whatever/x.bin"), proj(), never_global),
            PathClass::Unclassified
        );
    }

    #[test]
    fn global_wins() {
        let p = Path::new("/proj/zfb.config.ts");
        let cls = classify_change(p, proj(), |q| q == p);
        assert_eq!(cls, PathClass::Global);
    }

    #[test]
    fn css_inside_components_is_style_not_module() {
        // Regression: `components/Button.css` used to classify as
        // Module because the directory match outranked the extension
        // fallback. The rebuild plan never set `rerun_css` and the new
        // bytes never reached `dist/assets/`.
        assert_eq!(
            classify_change(
                Path::new("/proj/components/Button.css"),
                proj(),
                never_global,
            ),
            PathClass::Style,
        );
        assert_eq!(
            classify_change(
                Path::new("/proj/src/widgets/picker.css"),
                proj(),
                never_global,
            ),
            PathClass::Style,
        );
        // Sanity: the `.tsx` companion stays Module.
        assert_eq!(
            classify_change(
                Path::new("/proj/components/Button.tsx"),
                proj(),
                never_global,
            ),
            PathClass::Module,
        );
    }

    #[test]
    fn ancestor_named_pages_does_not_misclassify_components() {
        // Regression: a project hosted under a directory named "pages"
        // (e.g. `/home/me/pages/myproj/...`) used to misclassify every
        // change as PathClass::Page because the walker found the
        // ancestor `pages` first. With project-root anchoring the
        // ancestor segment is stripped and the classifier sees the
        // real `components/Header.tsx` shape.
        let root = Path::new("/home/me/pages/myproj");
        assert_eq!(
            classify_change(
                Path::new("/home/me/pages/myproj/components/Header.tsx"),
                root,
                never_global,
            ),
            PathClass::Module,
        );
        assert_eq!(
            classify_change(
                Path::new("/home/me/pages/myproj/pages/index.tsx"),
                root,
                never_global,
            ),
            PathClass::Page,
        );
    }

    #[test]
    fn islands_candidate_matches_components_root() {
        let pol = GranularityPolicy::default();
        assert!(pol.is_islands_candidate(Path::new("/proj/components/Counter.tsx")));
        assert!(!pol.is_islands_candidate(Path::new("/proj/pages/index.tsx")));
        assert!(!pol.is_islands_candidate(Path::new("/proj/content/x.md")));
    }

    #[test]
    fn out_of_root_paths_skip_root_segment_walk() {
        // Issue #368 regression: an `extraWatchPaths` entry like
        // `/srv/shared/public/foo.md` is OUTSIDE the project root, and
        // the segment named `public` in its absolute path must NOT
        // make the classifier return `PathClass::Asset` — that branch
        // does nothing in the orchestrator and the live reload never
        // fires. The classifier must skip the root-segment walk for
        // out-of-root paths and drop straight to extension sniff.
        assert_eq!(
            classify_change(Path::new("/srv/shared/public/foo.md"), proj(), never_global,),
            PathClass::Content,
        );
        assert_eq!(
            classify_change(
                Path::new("/srv/shared/styles/site.css"),
                proj(),
                never_global,
            ),
            PathClass::Style,
        );
        assert_eq!(
            classify_change(
                Path::new("/srv/shared/components/Widget.tsx"),
                proj(),
                never_global,
            ),
            PathClass::Module,
        );
        assert_eq!(
            classify_change(
                Path::new("/srv/shared/data/site.json"),
                proj(),
                never_global,
            ),
            PathClass::Data,
        );
        // Unknown extension on an out-of-root path used to classify as
        // `Unclassified` — but the `Unclassified` branch in
        // `plan_for_changes` does NOT fall back to `PageSelection::All`,
        // so edits to e.g. `logo.png` or `schema.graphql` under an
        // extra watch root silently produced no rebuild. Deep-review
        // fix (PR #376) re-routes those to `External`, which the
        // orchestrator maps to a conservative full rebuild.
        assert_eq!(
            classify_change(
                Path::new("/srv/shared/pages/notes.bin"),
                proj(),
                never_global,
            ),
            PathClass::External,
        );
    }

    /// Deep-review regression (PR #376): files under an extra watch
    /// path with non-whitelisted extensions classify as `External`.
    /// Cover the documented cases — `logo.png`, `schema.graphql`,
    /// `*.lock` files — that the previous Unclassified behaviour
    /// silently no-op'd in `plan_for_changes`.
    #[test]
    fn out_of_root_non_whitelisted_extensions_are_external() {
        assert_eq!(
            classify_change(
                Path::new("/srv/shared/assets/logo.png"),
                proj(),
                never_global,
            ),
            PathClass::External,
        );
        assert_eq!(
            classify_change(
                Path::new("/srv/shared/api/schema.graphql"),
                proj(),
                never_global,
            ),
            PathClass::External,
        );
        assert_eq!(
            classify_change(
                Path::new("/srv/shared/lockfiles/pnpm-lock.yaml"),
                proj(),
                never_global,
            ),
            // .yaml IS in the whitelist (Data), so this stays Data. The
            // assertion confirms whitelisted extensions are unaffected
            // by the External re-route.
            PathClass::Data,
        );
        // No extension at all under an extra watch root → External.
        assert_eq!(
            classify_change(Path::new("/srv/shared/Makefile"), proj(), never_global,),
            PathClass::External,
        );
    }
}
