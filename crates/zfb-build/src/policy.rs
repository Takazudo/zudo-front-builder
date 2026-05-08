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
    if is_global(path) {
        return PathClass::Global;
    }

    // Anchor at project_root when possible. Absolute paths from notify
    // typically start with the project root; relative paths and paths
    // outside the root fall through to the unstripped walk.
    let project_relative = path.strip_prefix(project_root).unwrap_or(path);
    let mut comps = project_relative.components().peekable();

    // Skip leading `/` / drive prefixes / `RootDir` components (only
    // possible when strip_prefix did not fire).
    while let Some(c) = comps.peek() {
        if matches!(c, Component::Prefix(_) | Component::RootDir) {
            comps.next();
        } else {
            break;
        }
    }

    let lower_ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());

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
    match lower_ext.as_deref() {
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
}

impl Default for GranularityPolicy {
    fn default() -> Self {
        Self {
            islands_roots: vec![PathBuf::from("components"), PathBuf::from("src")],
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
            classify_change(
                Path::new("/proj/pages/llms.txt.tsx"),
                proj(),
                never_global,
            ),
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
}
