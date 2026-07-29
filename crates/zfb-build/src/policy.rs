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

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Live dependency sets consumed by the islands and client-script dev
/// sub-pipelines.
///
/// Generated `.zfb-raw-*.mjs` files are ephemeral shadow artifacts and never
/// watcher inputs. For islands the set includes ORIGINAL raw targets plus the
/// complete first-party module-worker graph; client scripts register raw
/// targets plus their module-worker dependencies in separate sets. A
/// successful scanner/preprocess pass replaces each set with both logical and
/// canonical real-path aliases. The granularity policy consults them on every
/// event, and exposes all three sets to the dynamic watcher, so raw edits,
/// worker-graph edits, and symlink retarget/deletes still rerun the owning
/// consumer pipeline.
#[derive(Debug, Clone, Default)]
pub struct RawImportInvalidation {
    islands: Arc<RwLock<BTreeSet<PathBuf>>>,
    client_scripts: Arc<RwLock<BTreeSet<PathBuf>>>,
    client_script_workers: Arc<RwLock<BTreeSet<PathBuf>>>,
    /// Workspace-sibling plain modules materialised into the client-script
    /// preprocess stage (issue #1710) — neither a terminal `?raw` target nor
    /// a worker dependency, so distinct from both sets above.
    client_script_siblings: Arc<RwLock<BTreeSet<PathBuf>>>,

    /// CSS sibling-mirror-root DIRECTORIES the CSS source-plan seam
    /// (`crate::commands::build::build_default_css_payload_with_source_plan`
    /// in the `zfb` crate, issue #1802 / epic #1799) publishes on every CSS
    /// recompute — including when the Tailwind subprocess that consumes
    /// them as `@source` globs later fails. Distinct in KIND from the three
    /// sets above: those hold FILE targets matched by exact-path
    /// containment (`is_*_target`); this holds DIRECTORY roots consumed
    /// wholesale by `zfb_watcher::Watcher::sync_recursive_dir_watches`
    /// (issue #1801), which does its own alias/canonicalisation handling —
    /// so no per-path alias expansion happens at this layer (contrast with
    /// `Self::replace`/`Self::aliases` above, used by the file-shaped sets).
    css_mirror_roots: Arc<RwLock<BTreeSet<PathBuf>>>,

    /// Absolute filesystem paths a plugin virtual-module loader registered
    /// via `addVirtualModule`'s optional `{ watchFiles }` option (issue
    /// #2167 / #2168) — a loader whose output depends on files it reads
    /// directly (e.g. via `node:fs`) rather than static ESM imports the dev
    /// bundler would otherwise notice on its own. Distinct in KIND from the
    /// four sets above: those track BROWSER dependency closures discovered
    /// by the scanner/bundler on every successful pass; this tracks paths a
    /// plugin loader declared once at `setup` time — plugin registrations
    /// are frozen after `setup` runs, so unlike the sets above this is
    /// populated exactly ONCE, at boot, never replaced again mid-session.
    /// File-shaped like the first three sets (exact-path containment via
    /// `is_plugin_watch_target`, folded into `dynamic_dependency_paths()`),
    /// not directory-shaped like `css_mirror_roots`.
    plugin_watch_files: Arc<RwLock<BTreeSet<PathBuf>>>,
}

impl RawImportInvalidation {
    fn resolved_alias(path: &Path) -> Option<PathBuf> {
        let mut cursor = path;
        let mut missing_suffix = Vec::new();
        loop {
            if let Ok(mut canonical) = cursor.canonicalize() {
                for component in missing_suffix.iter().rev() {
                    canonical.push(component);
                }
                return Some(zfb_types::normalize_path_lexical(&canonical));
            }
            missing_suffix.push(cursor.file_name()?.to_os_string());
            cursor = cursor.parent()?;
        }
    }

    fn aliases(path: PathBuf) -> impl Iterator<Item = PathBuf> {
        let lexical = zfb_types::normalize_path_lexical(&path);
        let resolved = Self::resolved_alias(&path);
        std::iter::once(lexical).chain(resolved)
    }

    fn replace(set: &RwLock<BTreeSet<PathBuf>>, paths: impl IntoIterator<Item = PathBuf>) {
        if let Ok(mut set) = set.write() {
            *set = paths.into_iter().flat_map(Self::aliases).collect();
        }
    }

    /// Atomically replace the islands dependency set after a successful scan.
    pub fn replace_islands(&self, paths: impl IntoIterator<Item = PathBuf>) {
        Self::replace(&self.islands, paths);
    }

    /// Snapshot the current islands dependency aliases for dynamic watcher
    /// registration. A clone keeps the registry lock out of notify calls.
    pub fn islands_paths(&self) -> BTreeSet<PathBuf> {
        self.islands
            .read()
            .map(|paths| paths.clone())
            .unwrap_or_default()
    }

    /// Atomically replace the client-script raw-target set after a successful
    /// staging/bundle pass.
    pub fn replace_client_scripts(&self, paths: impl IntoIterator<Item = PathBuf>) {
        Self::replace(&self.client_scripts, paths);
    }

    /// Snapshot the current client-script raw-target aliases for dynamic
    /// watcher registration. Logical aliases remain present across deletion,
    /// so watching their parents preserves recreate recovery.
    pub fn client_script_paths(&self) -> BTreeSet<PathBuf> {
        self.client_scripts
            .read()
            .map(|paths| paths.clone())
            .unwrap_or_default()
    }

    /// Atomically replace the complete first-party invalidation closure for
    /// client-script-owned module workers, including constructor importers.
    pub fn replace_client_script_workers(&self, paths: impl IntoIterator<Item = PathBuf>) {
        Self::replace(&self.client_script_workers, paths);
    }

    /// Snapshot the current client-script worker dependency aliases for
    /// dynamic watcher registration. Constructor importers are included in
    /// the closure supplied by the successful client-script bundle pass.
    pub fn client_script_worker_paths(&self) -> BTreeSet<PathBuf> {
        self.client_script_workers
            .read()
            .map(|paths| paths.clone())
            .unwrap_or_default()
    }

    /// Atomically replace the client-script workspace-sibling plain-module
    /// set after a successful staging/bundle pass (issue #1710).
    pub fn replace_client_script_siblings(&self, paths: impl IntoIterator<Item = PathBuf>) {
        Self::replace(&self.client_script_siblings, paths);
    }

    /// Snapshot the current client-script sibling-module aliases for dynamic
    /// watcher registration.
    pub fn client_script_sibling_paths(&self) -> BTreeSet<PathBuf> {
        self.client_script_siblings
            .read()
            .map(|paths| paths.clone())
            .unwrap_or_default()
    }

    fn contains(set: &RwLock<BTreeSet<PathBuf>>, path: &Path) -> bool {
        let lexical = zfb_types::normalize_path_lexical(path);
        let resolved = Self::resolved_alias(path);
        set.read()
            .map(|paths| {
                paths.contains(path)
                    || paths.contains(&lexical)
                    || resolved.as_ref().is_some_and(|path| paths.contains(path))
            })
            .unwrap_or(false)
    }

    /// Whether `path` participates in the current islands raw/worker graph.
    pub fn is_islands_target(&self, path: &Path) -> bool {
        Self::contains(&self.islands, path)
    }

    /// Whether `path` is a terminal target in the current client-script graph.
    pub fn is_client_script_target(&self, path: &Path) -> bool {
        Self::contains(&self.client_scripts, path)
    }

    /// Whether `path` is part of the current client-script worker graph.
    pub fn is_client_script_worker_target(&self, path: &Path) -> bool {
        Self::contains(&self.client_script_workers, path)
    }

    /// Whether `path` is a workspace-sibling plain module materialised into
    /// the current client-script preprocess stage (issue #1710).
    pub fn is_client_script_sibling_target(&self, path: &Path) -> bool {
        Self::contains(&self.client_script_siblings, path)
    }

    /// Atomically replace the CSS sibling-mirror-root set (issue #1802).
    /// Replace, not append/union — a project whose sibling claim set
    /// shrinks must stop watching the roots it no longer claims, or a stale
    /// root would stay registered forever.
    ///
    /// Note `tailwind.enabled = false` is NOT such a case: that path still
    /// publishes the full claimed set, because `.module.css` discovery runs
    /// through the same claim plan regardless of Tailwind (issue #824).
    ///
    /// Because this is replace semantics, a caller must never publish a
    /// deliberately NARROWED set as a "partial" update — doing so unwatches
    /// the difference, and if the edit that would repair the caller's error
    /// lives under a dropped root, no event can arrive to retry. Publish the
    /// complete set or publish nothing (preserving the last successful one).
    pub fn replace_css_mirror_roots(&self, roots: impl IntoIterator<Item = PathBuf>) {
        if let Ok(mut set) = self.css_mirror_roots.write() {
            *set = roots.into_iter().collect();
        }
    }

    /// Snapshot the current CSS sibling-mirror-root set for dynamic
    /// recursive-directory watch reconciliation
    /// (`crate::orchestrator`'s `register_dynamic_dependency_watches`).
    pub fn css_mirror_roots(&self) -> BTreeSet<PathBuf> {
        self.css_mirror_roots
            .read()
            .map(|roots| roots.clone())
            .unwrap_or_default()
    }

    /// Locate `path` inside the registered CSS sibling-mirror roots (issue
    /// #1819, epic #1995), returning `(matched root alias, path relative to
    /// that root)` — `("", …)`-free and empty-relative when the path IS the
    /// root.
    ///
    /// Containment, not exact membership — contrast with [`Self::contains`],
    /// which backs the file-shaped `is_*_target` predicates. These roots are
    /// DIRECTORIES, and the question this answers is "would
    /// `discover_css_source_files` / Tailwind's `@source` globs have scanned
    /// this file", which is a subtree question.
    ///
    /// `replace_css_mirror_roots` stores roots WITHOUT the alias expansion
    /// `Self::replace` applies to the file-shaped sets (its only other
    /// consumer, `sync_recursive_dir_watches`, does its own canonicalisation),
    /// so both sides are alias-expanded here instead: the incoming watcher
    /// event path can be the canonical real path (macOS FSEvents reports
    /// `/private/var/...`) while the published root is the lexical one, or
    /// vice versa.
    ///
    /// Callers get the matched root and remainder rather than a bare bool so
    /// they can apply their own containment-region rules (the orchestrator
    /// uses both: a degeneracy guard against a root that would swallow the
    /// project, and the `CSS_SIBLING_MIRROR_SKIP_DIRS` infra-dir filter that
    /// the `@source` scan itself applies).
    pub fn css_mirror_root_match(&self, path: &Path) -> Option<(PathBuf, PathBuf)> {
        let roots = self.css_mirror_roots.read().ok()?;
        if roots.is_empty() {
            return None;
        }
        let candidates: Vec<PathBuf> = Self::aliases(path.to_path_buf()).collect();
        for root in roots.iter() {
            for root_alias in Self::aliases(root.clone()) {
                // The emptiness guard must test the form we MATCH on, not the
                // raw stored root: a root spelled `.` or `a/..` normalizes to
                // an empty path, which `starts_with`/`strip_prefix` accept as
                // a prefix of EVERY path — turning the gate into the rejected
                // "rerun for every markdown edit" option (a).
                if root_alias.as_os_str().is_empty() {
                    continue;
                }
                for candidate in &candidates {
                    if let Ok(rel) = candidate.strip_prefix(&root_alias) {
                        return Some((root_alias, rel.to_path_buf()));
                    }
                }
            }
        }
        None
    }

    /// Whether `path` lies inside one of the registered CSS sibling-mirror
    /// roots — the bool-shaped view of [`Self::css_mirror_root_match`].
    pub fn is_under_css_mirror_root(&self, path: &Path) -> bool {
        self.css_mirror_root_match(path).is_some()
    }

    /// Expose the lexical/canonical alias expansion this registry applies to
    /// every path it stores or tests, so callers comparing a path against a
    /// matched mirror root use the SAME notion of path identity (macOS
    /// FSEvents reports `/private/var/...` where the published root is
    /// `/var/...`, or vice versa) instead of re-deriving a second one.
    pub fn path_aliases(path: &Path) -> Vec<PathBuf> {
        Self::aliases(path.to_path_buf()).collect()
    }

    /// Atomically replace the plugin virtual-module watch-file set (issue
    /// #2168). Populated exactly ONCE, at boot, from every registered
    /// loader's `watch_files` — plugin registrations are frozen after
    /// `setup` runs, so unlike `replace_islands` / `replace_client_scripts`
    /// there is no later tick that would call this again.
    pub fn replace_plugin_watch_files(&self, paths: impl IntoIterator<Item = PathBuf>) {
        Self::replace(&self.plugin_watch_files, paths);
    }

    /// Snapshot the current plugin watch-file aliases for dynamic watcher
    /// registration (folded into [`GranularityPolicy::dynamic_dependency_paths`]).
    pub fn plugin_watch_file_paths(&self) -> BTreeSet<PathBuf> {
        self.plugin_watch_files
            .read()
            .map(|paths| paths.clone())
            .unwrap_or_default()
    }

    /// Whether `path` is a registered plugin virtual-module watch file.
    pub fn is_plugin_watch_target(&self, path: &Path) -> bool {
        Self::contains(&self.plugin_watch_files, path)
    }
}

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

/// Returns `true` when the filename looks like a `*.client.{ts,tsx,js,jsx}`
/// client-script entry.
///
/// Delegates to the canonical filename predicate in `zfb-types`
/// (`zfb_types::is_client_script_file`) — the single source of truth for the
/// `.client.` infix and accepted extensions, shared with `zfb-islands`'s
/// discovery and `zfb-router`'s page-scan skip. The check is purely
/// filename-based (no filesystem access).
pub(crate) fn is_client_script_path(path: &Path) -> bool {
    zfb_types::is_client_script_file(path)
}

/// Client-script discovery roots (project-root-relative).
///
/// Mirrors `zfb_islands::client_scripts::CLIENT_SCRIPT_DISCOVERY_ROOTS`.
/// Kept in sync by convention: if the discovery roots ever change in
/// `zfb-islands`, this constant must be updated too.
pub(crate) const CLIENT_SCRIPT_ROOTS: &[&str] = &["pages", "components", "src"];

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

/// Session-live set of content-collection entries the dev server ALREADY
/// knows about — "every file in the last successful collection membership
/// walk" (issue #1581).
///
/// Sole consumer: the #1058 spurious-`Created` normalization in
/// [`crate::BuildOrchestrator::tick_with_kinds`]. macOS FSEvents coalesces
/// an in-place edit of an EXISTING file into `Created` (see
/// `zfb_watcher::merge_kind`), which poisons the strict all-`Modified`
/// `fan_out_safe` gate and costs the whole tick its #958 eager narrowing.
/// #1058 normalizes that artifact away, but keyed only on the dependency
/// graph having a non-empty `consumers_of` reverse edge — and NO boot-time
/// collection entry has one on a cold start (the dev server's only
/// `DepKind::Content` writer is the discovery hook, which fires just for
/// newly-CREATED files). So on a clean boot the normalization never fired
/// and the FIRST edit of any pre-existing entry lost its narrowing.
///
/// This registry is the authoritative "already known" oracle the graph
/// could not be. It deliberately does NOT feed `dirty_pages`: an unknown
/// content path must keep tripping the planner's `PageSelection::All`
/// fallback, which is the only thing re-rendering AGGREGATE pages (a post
/// index listing every entry) on a content edit.
#[derive(Debug, Clone, Default)]
pub struct KnownContentEntries {
    entries: Arc<RwLock<BTreeSet<PathBuf>>>,
}

impl KnownContentEntries {
    /// Lexical `.`/`..` collapse so a configured root that could not be
    /// canonicalised (the not-yet-created-dir fallback) still compares
    /// equal to the canonical path `notify` delivers.
    fn key(path: &Path) -> PathBuf {
        zfb_types::normalize_path_lexical(path)
    }

    /// Add collection entries — the boot membership walk, and each entry a
    /// successful discovery pass accepted.
    pub fn insert_many(&self, paths: impl IntoIterator<Item = PathBuf>) {
        if let Ok(mut set) = self.entries.write() {
            set.extend(paths.into_iter().map(|p| Self::key(&p)));
        }
    }

    /// Forget `path` and everything beneath it (a removed directory takes
    /// its entries with it). Called for every raw `Removed` change BEFORE
    /// the `Created` normalization runs, so a delete→recreate is not
    /// mistaken for an in-place edit and still routes through discovery.
    pub fn remove_path_and_descendants(&self, path: &Path) {
        let prefix = Self::key(path);
        if let Ok(mut set) = self.entries.write() {
            set.retain(|known| known != &prefix && !known.starts_with(&prefix));
        }
    }

    /// Whether this exact path is an already-known collection entry.
    pub fn contains(&self, path: &Path) -> bool {
        let key = Self::key(path);
        self.entries
            .read()
            .map(|set| set.contains(&key))
            .unwrap_or(false)
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

    /// Session-live raw-target and module-worker dependency sets. Empty by
    /// default.
    pub raw_import_invalidation: RawImportInvalidation,

    /// Session-live set of already-known content-collection entries
    /// (issue #1581). Empty by default — an empty registry simply leaves
    /// the #1058 normalization keyed on the graph alone, i.e. exactly the
    /// pre-#1581 behaviour.
    pub known_content: KnownContentEntries,
}

impl Default for GranularityPolicy {
    fn default() -> Self {
        Self {
            islands_roots: vec![PathBuf::from("components"), PathBuf::from("src")],
            content_roots: Vec::new(),
            raw_import_invalidation: RawImportInvalidation::default(),
            known_content: KnownContentEntries::default(),
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

    /// Attach the live dependency registry shared with the dev bundlers.
    pub fn with_raw_import_invalidation(mut self, invalidation: RawImportInvalidation) -> Self {
        self.raw_import_invalidation = invalidation;
        self
    }

    /// Attach the live known-content-entry registry shared with the dev
    /// server's boot seed and discovery hook (issue #1581).
    pub fn with_known_content(mut self, known_content: KnownContentEntries) -> Self {
        self.known_content = known_content;
        self
    }

    /// Whether this changed path is a content-collection entry the dev
    /// session already knew about (so a `Created` for it is an FSEvents
    /// coalescing artifact, not a genuinely new file).
    pub fn is_known_content_entry(&self, path: &Path) -> bool {
        self.known_content.contains(path)
    }

    /// Whether this exact changed path is in the live islands raw/worker
    /// dependency closure.
    pub fn is_islands_dependency(&self, path: &Path) -> bool {
        self.raw_import_invalidation.is_islands_target(path)
    }

    /// Backward-compatible name for callers that only register islands raw
    /// targets. The live set may now also contain module-worker dependencies;
    /// new code should use [`Self::is_islands_dependency`].
    pub fn is_islands_raw_target(&self, path: &Path) -> bool {
        self.is_islands_dependency(path)
    }

    /// Snapshot live islands dependency aliases for dynamic watch roots.
    pub fn islands_dependency_paths(&self) -> BTreeSet<PathBuf> {
        self.raw_import_invalidation.islands_paths()
    }

    /// Snapshot the browser dependency aliases that need dynamic parent
    /// watches. Invalidation predicates remain pipeline-specific; this union
    /// only controls which filesystem events can reach the orchestrator.
    ///
    /// Also folds in the plugin virtual-module watch-file set (issue #2168)
    /// — those paths are file-shaped exactly like the three browser-closure
    /// sets above, so `register_dynamic_dependency_watches`
    /// (`crate::orchestrator`) offers them to `watch_additional_files` with
    /// no watcher-crate changes needed.
    pub fn dynamic_dependency_paths(&self) -> BTreeSet<PathBuf> {
        let mut paths = self.raw_import_invalidation.islands_paths();
        paths.extend(self.raw_import_invalidation.client_script_paths());
        paths.extend(self.raw_import_invalidation.client_script_worker_paths());
        paths.extend(self.raw_import_invalidation.client_script_sibling_paths());
        paths.extend(self.raw_import_invalidation.plugin_watch_file_paths());
        paths
    }

    /// Whether this exact changed path is a client-script terminal raw target.
    pub fn is_client_script_raw_target(&self, path: &Path) -> bool {
        self.raw_import_invalidation.is_client_script_target(path)
    }

    /// Whether this exact changed path is a registered plugin virtual-module
    /// watch file (issue #2168) — a loader's `addVirtualModule(..., {
    /// watchFiles })` entry (issue #2167).
    pub fn is_plugin_watch_target(&self, path: &Path) -> bool {
        self.raw_import_invalidation.is_plugin_watch_target(path)
    }

    /// Whether this exact changed path belongs to a client-script module
    /// worker graph captured by the latest successful bundle.
    pub fn is_client_script_worker_target(&self, path: &Path) -> bool {
        self.raw_import_invalidation
            .is_client_script_worker_target(path)
    }

    /// Whether this exact changed path is a workspace-sibling plain module
    /// materialised into the current client-script preprocess stage (issue
    /// #1710) — neither a terminal raw target nor a worker dependency.
    pub fn is_client_script_sibling_target(&self, path: &Path) -> bool {
        self.raw_import_invalidation
            .is_client_script_sibling_target(path)
    }

    /// Snapshot the live CSS sibling-mirror-root set (issue #1802) for
    /// dynamic recursive-directory watch reconciliation. Deliberately a
    /// DISTINCT accessor from [`Self::dynamic_dependency_paths`]: that
    /// union feeds the non-recursive `watch_additional_files` API and is
    /// file-shaped, while these are directory roots consumed by
    /// `zfb_watcher::Watcher::sync_recursive_dir_watches` — folding them
    /// into `dynamic_dependency_paths` would register a directory root as
    /// a non-recursive file-parent watch, which is not what that API does.
    pub fn css_mirror_root_paths(&self) -> BTreeSet<PathBuf> {
        self.raw_import_invalidation.css_mirror_roots()
    }

    /// Whether `path` lies inside one of the registered CSS sibling-mirror
    /// roots (issue #1819, epic #1995) — the option-(b) gate that lets a
    /// `PathClass::Content` change rerun the Tailwind content scan without
    /// making EVERY markdown edit pay for one.
    pub fn is_under_css_mirror_root(&self, path: &Path) -> bool {
        self.raw_import_invalidation.is_under_css_mirror_root(path)
    }

    /// Locate `path` inside the registered CSS sibling-mirror roots,
    /// returning `(matched root alias, path relative to that root)` — see
    /// [`RawImportInvalidation::css_mirror_root_match`].
    pub fn css_mirror_root_match(&self, path: &Path) -> Option<(PathBuf, PathBuf)> {
        self.raw_import_invalidation.css_mirror_root_match(path)
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

    /// Returns `true` when `path` is a `*.client.{ts,tsx,js,jsx}` file
    /// under one of the three conventional client-script discovery roots
    /// (`pages/`, `components/`, `src/`).
    ///
    /// This check is **path-classification-independent**: a
    /// `*.client.ts` file under `pages/` classifies as `PathClass::Page`
    /// (not `Module`), so the normal `is_islands_candidate` gate does not
    /// fire. We call this predicate *after* the `PathClass` switch so it
    /// covers all three roots regardless of classification.
    pub fn is_client_script_candidate(&self, path: &Path) -> bool {
        if !is_client_script_path(path) {
            return false;
        }
        for root in CLIENT_SCRIPT_ROOTS {
            if path_starts_with_segment(path, Path::new(root)) {
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

    #[test]
    fn client_script_worker_invalidation_replaces_stale_graph_between_ticks() {
        let invalidation = RawImportInvalidation::default();
        let worker = PathBuf::from("/proj/src/search.worker.ts");
        let importer = PathBuf::from("/proj/src/start.ts");
        invalidation.replace_client_script_workers([worker.clone(), importer.clone()]);
        assert!(invalidation.is_client_script_worker_target(&worker));
        assert!(invalidation.is_client_script_worker_target(&importer));

        // A successful second scan replaces (rather than appends to) the
        // graph, so removing the Worker constructor drops stale triggers.
        invalidation.replace_client_script_workers(Vec::new());
        assert!(!invalidation.is_client_script_worker_target(&worker));
        assert!(!invalidation.is_client_script_worker_target(&importer));
    }

    /// Issue #1802: `css_mirror_roots` follows the same replace-not-union
    /// contract as the other `RawImportInvalidation` sets — a root dropped
    /// by a later CSS recompute (the sibling claim shrank, or Tailwind got
    /// disabled) must not linger, and a full clear must empty the set.
    #[test]
    fn css_mirror_roots_replace_semantics_drop_stale_roots() {
        let invalidation = RawImportInvalidation::default();
        let first_root = PathBuf::from("/workspace/lib/shared");
        let second_root = PathBuf::from("/workspace/lib/other");
        invalidation.replace_css_mirror_roots([first_root.clone(), second_root.clone()]);
        let policy =
            GranularityPolicy::default().with_raw_import_invalidation(invalidation.clone());
        let first = policy.css_mirror_root_paths();
        assert!(first.contains(&first_root));
        assert!(first.contains(&second_root));
        // Deliberately NOT folded into the file-shaped union (see both
        // accessors' doc comments) — a directory root must never be
        // registered as a non-recursive file-parent watch.
        assert!(!policy.dynamic_dependency_paths().contains(&first_root));

        // A later recompute that no longer claims `second_root` must retire
        // it — replace, not union.
        invalidation.replace_css_mirror_roots([first_root.clone()]);
        let second = policy.css_mirror_root_paths();
        assert!(second.contains(&first_root));
        assert!(
            !second.contains(&second_root),
            "a replaced mirror-root set must not retain a stale root: {second:?}"
        );

        // Clearing entirely (e.g. Tailwind gets disabled) empties the set.
        invalidation.replace_css_mirror_roots(Vec::new());
        assert!(
            policy.css_mirror_root_paths().is_empty(),
            "an empty replace must clear every previously published root"
        );
    }

    /// Issue #1819 (epic #1995): `is_under_css_mirror_root` is a SUBTREE
    /// question, not the exact-path membership the file-shaped `is_*_target`
    /// predicates answer — and it must match on whole path components, so a
    /// sibling directory whose name merely shares a prefix is not swept in.
    /// An empty registry answers `false` for everything, which is what keeps
    /// the orchestrator gate inert on projects claiming no sibling.
    #[test]
    fn is_under_css_mirror_root_matches_subtree_by_component() {
        let invalidation = RawImportInvalidation::default();
        let root = PathBuf::from("/workspace/lib/shared");
        let policy =
            GranularityPolicy::default().with_raw_import_invalidation(invalidation.clone());

        assert!(
            !policy.is_under_css_mirror_root(&root.join("notes.mdx")),
            "an empty registry must never claim containment"
        );

        invalidation.replace_css_mirror_roots([root.clone()]);
        assert!(policy.is_under_css_mirror_root(&root.join("notes.mdx")));
        assert!(policy.is_under_css_mirror_root(&root.join("deep/nested/notes.md")));
        assert!(
            policy.is_under_css_mirror_root(&root),
            "the root itself is inside its own subtree"
        );
        assert!(
            !policy.is_under_css_mirror_root(Path::new("/workspace/lib/shared-other/notes.mdx")),
            "component-wise matching must not treat `shared-other` as inside `shared`"
        );
        assert!(!policy.is_under_css_mirror_root(Path::new("/workspace/lib/notes.mdx")));
        assert!(!policy.is_under_css_mirror_root(Path::new("/elsewhere/notes.mdx")));
    }

    /// The empty-root guard must test the NORMALIZED form, because that is
    /// the form containment matches on.
    ///
    /// A root spelled `.` or `a/..` is non-empty as stored but normalizes to
    /// an empty path, and an empty path is a prefix of EVERY path — so a
    /// guard that inspected only the raw spelling would let such a root
    /// swallow the whole filesystem, silently converting the orchestrator's
    /// option-(b) gate into the rejected option (a). Not reachable from
    /// today's `SiblingMirrorPlan` (which publishes absolute, normalized
    /// directories), which is exactly why the guard has to say what it means.
    #[test]
    fn root_normalizing_to_empty_never_matches_everything() {
        let invalidation = RawImportInvalidation::default();
        let policy =
            GranularityPolicy::default().with_raw_import_invalidation(invalidation.clone());

        for degenerate in [".", "a/..", "./."] {
            invalidation.replace_css_mirror_roots([PathBuf::from(degenerate)]);
            assert!(
                !policy.css_mirror_root_paths().is_empty(),
                "fixture sanity: `{degenerate}` IS stored — this is not the \
                 empty-registry short-circuit"
            );
            assert!(
                !policy.is_under_css_mirror_root(Path::new("/workspace/host/content/post.mdx")),
                "a root spelled `{degenerate}` normalizes to an empty path and \
                 must not be treated as containing every path"
            );
        }

        // The guard must skip only the degenerate root, not the whole call: a
        // real root published alongside one still matches.
        let real = PathBuf::from("/workspace/lib/shared");
        invalidation.replace_css_mirror_roots([PathBuf::from("."), real.clone()]);
        assert!(policy.is_under_css_mirror_root(&real.join("notes.mdx")));
        assert!(!policy.is_under_css_mirror_root(Path::new("/workspace/host/content/post.mdx")));
    }

    /// The registry stores roots WITHOUT alias expansion (unlike the
    /// file-shaped sets), so containment resolves aliases on BOTH sides —
    /// otherwise a macOS FSEvents path (`/private/var/...`) would miss a
    /// root published in its lexical (`/var/...`) spelling.
    #[test]
    #[cfg(unix)]
    fn is_under_css_mirror_root_resolves_symlink_aliases_on_both_sides() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let physical = base.join("physical/shared");
        std::fs::create_dir_all(physical.join("nested")).unwrap();
        std::fs::write(physical.join("nested/notes.mdx"), "# notes\n").unwrap();
        let alias = base.join("alias-shared");
        std::os::unix::fs::symlink(&physical, &alias).unwrap();

        // Root published through the SYMLINK, event path arriving as the
        // physical spelling.
        let invalidation = RawImportInvalidation::default();
        invalidation.replace_css_mirror_roots([alias.clone()]);
        let policy =
            GranularityPolicy::default().with_raw_import_invalidation(invalidation.clone());
        assert!(policy.is_under_css_mirror_root(&physical.join("nested/notes.mdx")));

        // And the reverse: root published physically, event path arriving
        // through the symlink.
        invalidation.replace_css_mirror_roots([physical.clone()]);
        assert!(policy.is_under_css_mirror_root(&alias.join("nested/notes.mdx")));
    }

    #[test]
    fn dynamic_dependency_paths_union_browser_closures_without_cross_classifying() {
        let invalidation = RawImportInvalidation::default();
        let island_helper = PathBuf::from("/proj/lib/island-helper.ts");
        let client_raw = PathBuf::from("/proj/lib/client-payload.txt");
        let client_helper = PathBuf::from("/proj/lib/client-helper.ts");
        let client_sibling = PathBuf::from("/workspace/lib/shared/plain.ts");
        let plugin_watch = PathBuf::from("/proj/data/plugin-source.json");
        let next_client_raw = PathBuf::from("/proj/lib/next-client-payload.txt");
        let next_client_helper = PathBuf::from("/proj/lib/next-client-helper.ts");
        let next_client_sibling = PathBuf::from("/workspace/lib/shared/next-plain.ts");
        invalidation.replace_islands([island_helper.clone()]);
        invalidation.replace_client_scripts([client_raw.clone()]);
        invalidation.replace_client_script_workers([client_helper.clone()]);
        invalidation.replace_client_script_siblings([client_sibling.clone()]);
        invalidation.replace_plugin_watch_files([plugin_watch.clone()]);
        let policy =
            GranularityPolicy::default().with_raw_import_invalidation(invalidation.clone());

        let first = policy.dynamic_dependency_paths();
        assert!(first.contains(&island_helper));
        assert!(first.contains(&client_raw));
        assert!(first.contains(&client_helper));
        assert!(first.contains(&client_sibling));
        assert!(first.contains(&plugin_watch));
        assert!(policy.is_islands_dependency(&island_helper));
        assert!(!policy.is_islands_dependency(&client_raw));
        assert!(!policy.is_islands_dependency(&client_helper));
        assert!(!policy.is_islands_dependency(&client_sibling));
        assert!(!policy.is_islands_dependency(&plugin_watch));
        assert!(policy.is_client_script_raw_target(&client_raw));
        assert!(!policy.is_client_script_raw_target(&island_helper));
        assert!(!policy.is_client_script_raw_target(&client_sibling));
        assert!(!policy.is_client_script_raw_target(&plugin_watch));
        assert!(policy.is_client_script_worker_target(&client_helper));
        assert!(!policy.is_client_script_worker_target(&island_helper));
        assert!(!policy.is_client_script_worker_target(&client_sibling));
        assert!(!policy.is_client_script_worker_target(&plugin_watch));
        assert!(policy.is_client_script_sibling_target(&client_sibling));
        assert!(!policy.is_client_script_sibling_target(&island_helper));
        assert!(!policy.is_client_script_sibling_target(&client_raw));
        assert!(!policy.is_client_script_sibling_target(&client_helper));
        assert!(!policy.is_client_script_sibling_target(&plugin_watch));
        assert!(policy.is_plugin_watch_target(&plugin_watch));
        assert!(!policy.is_plugin_watch_target(&island_helper));
        assert!(!policy.is_plugin_watch_target(&client_raw));
        assert!(!policy.is_plugin_watch_target(&client_helper));
        assert!(!policy.is_plugin_watch_target(&client_sibling));

        invalidation.replace_client_scripts([next_client_raw.clone()]);
        invalidation.replace_client_script_workers([next_client_helper.clone()]);
        invalidation.replace_client_script_siblings([next_client_sibling.clone()]);
        let second = policy.dynamic_dependency_paths();
        assert!(second.contains(&island_helper));
        assert!(second.contains(&next_client_raw));
        assert!(second.contains(&next_client_helper));
        assert!(second.contains(&next_client_sibling));
        assert!(
            second.contains(&plugin_watch),
            "the plugin watch-file set is populated once at boot and is never \
             replaced again mid-session, unlike the browser-closure sets"
        );
        assert!(
            !second.contains(&client_raw),
            "a replaced client raw graph must not retain stale watch aliases"
        );
        assert!(
            !second.contains(&client_helper),
            "a replaced client worker graph must not retain stale watch aliases"
        );
        assert!(
            !second.contains(&client_sibling),
            "a replaced client sibling graph must not retain stale watch aliases"
        );
    }

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

    /// #1550 — a collection living OUTSIDE the project root
    /// (`allowOutsideRoot`, e.g. a monorepo-shared content dir) is routed
    /// through the extras channel and passed to the policy as its CANONICAL
    /// ABSOLUTE root. A `.md`/`.mdx` in a NESTED subdir of that root must
    /// classify as `Content` via the `path.starts_with(root)` arm — the
    /// project-relative arm can't help (the event path never strips under
    /// `project_root`).
    #[test]
    fn out_of_root_absolute_content_root_classifies_nested_md() {
        // Canonical absolute root as the #1550 inventory would produce it.
        let roots = vec![PathBuf::from("/home/user/packages/ui/src/posts")];
        for f in [
            "/home/user/packages/ui/src/posts/a.md",
            "/home/user/packages/ui/src/posts/nested/deep/b.mdx",
        ] {
            assert_eq!(
                classify_change_with_content_roots(Path::new(f), proj(), &roots, never_global),
                PathClass::Content,
                "out-of-root nested content file {f:?} must classify as Content"
            );
        }
    }

    /// #1550 — a co-located `.tsx` under an out-of-root collection root must
    /// NOT be swept up as `Content` (the override is content-shaped-only).
    /// Being out-of-tree, it falls through to the extension sniff and stays
    /// `Module`, preserving islands invalidation for external component
    /// edits.
    #[test]
    fn out_of_root_content_root_does_not_swallow_colocated_tsx() {
        let roots = vec![PathBuf::from("/home/user/packages/ui/src/posts")];
        assert_eq!(
            classify_change_with_content_roots(
                Path::new("/home/user/packages/ui/src/posts/Counter.tsx"),
                proj(),
                &roots,
                never_global,
            ),
            PathClass::Module
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

    // ── Client-script candidate detection (issue #979) ──────────────────

    /// Editing a `*.client.ts` under `pages/` must trigger
    /// `rerun_client_scripts`, even though the file also classifies as
    /// `PathClass::Page`. This is the BLOCKING acceptance requirement:
    /// the orchestrator checks `is_client_script_candidate` independently
    /// of the `PathClass` match so `pages/` files are NOT excluded.
    #[test]
    fn client_script_under_pages_is_candidate() {
        let policy = GranularityPolicy::default();
        assert!(
            policy.is_client_script_candidate(Path::new("/proj/pages/analytics.client.ts")),
            "pages/ *.client.ts must be a client-script candidate"
        );
        // All supported extensions.
        for ext in ["ts", "tsx", "js", "jsx"] {
            let path = format!("/proj/pages/widget.client.{ext}");
            assert!(
                policy.is_client_script_candidate(Path::new(&path)),
                "pages/ *.client.{ext} must be a candidate"
            );
        }
    }

    /// Editing a `*.client.ts` under `components/` must trigger
    /// `rerun_client_scripts`. `components/` is a default islands root so
    /// the file also classifies as `Module` — but the client-scripts pass
    /// must fire regardless of the islands bundler.
    #[test]
    fn client_script_under_components_is_candidate() {
        let policy = GranularityPolicy::default();
        assert!(
            policy
                .is_client_script_candidate(Path::new("/proj/components/search-widget.client.ts")),
            "components/ *.client.ts must be a client-script candidate"
        );
        for ext in ["ts", "tsx", "js", "jsx"] {
            let path = format!("/proj/components/widget.client.{ext}");
            assert!(
                policy.is_client_script_candidate(Path::new(&path)),
                "components/ *.client.{ext} must be a candidate"
            );
        }
    }

    /// Editing a `*.client.ts` under `src/` must trigger
    /// `rerun_client_scripts`. `src/` is a default islands root too — the
    /// client-scripts trigger is independent.
    #[test]
    fn client_script_under_src_is_candidate() {
        let policy = GranularityPolicy::default();
        assert!(
            policy.is_client_script_candidate(Path::new("/proj/src/my-lib.client.ts")),
            "src/ *.client.ts must be a client-script candidate"
        );
        // Nested subdirectory (should still match via path_starts_with_segment).
        assert!(
            policy.is_client_script_candidate(Path::new("/proj/src/widgets/fancy.client.tsx")),
            "src/widgets/ *.client.tsx must be a candidate (nested)"
        );
    }

    /// Non-client files under the discovery roots must NOT trigger the
    /// client-scripts pass.
    #[test]
    fn regular_tsx_files_are_not_client_script_candidates() {
        let policy = GranularityPolicy::default();
        for path in [
            "/proj/pages/index.tsx",
            "/proj/components/Button.tsx",
            "/proj/src/utils.ts",
            "/proj/content/post.md",
            "/proj/layouts/base.tsx",
        ] {
            assert!(
                !policy.is_client_script_candidate(Path::new(path)),
                "regular file must NOT be a client-script candidate: {path}"
            );
        }
    }

    /// Files outside the three conventional roots are NOT candidates,
    /// even if they have the `.client.ts` suffix.
    #[test]
    fn client_script_outside_discovery_roots_is_not_candidate() {
        let policy = GranularityPolicy::default();
        // `layouts/` is explicitly excluded from the discovery roots.
        assert!(
            !policy.is_client_script_candidate(Path::new("/proj/layouts/header-toggle.client.ts")),
            "layouts/ must NOT be a client-script candidate (excluded from discovery)"
        );
        // A file outside all known roots.
        assert!(
            !policy.is_client_script_candidate(Path::new("/proj/lib/util.client.ts")),
            "lib/ must NOT be a client-script candidate (not a discovery root)"
        );
    }

    #[cfg(unix)]
    #[test]
    fn raw_invalidation_keeps_lexical_and_canonical_symlink_aliases() {
        let project = tempfile::tempdir().unwrap();
        let first = project.path().join("first.txt");
        let second = project.path().join("second.txt");
        let alias = project.path().join("current.txt");
        std::fs::write(&first, "first").unwrap();
        std::fs::write(&second, "second").unwrap();
        std::os::unix::fs::symlink(&first, &alias).unwrap();

        let invalidation = RawImportInvalidation::default();
        invalidation.replace_islands([alias.clone()]);
        assert!(invalidation.is_islands_target(&alias));
        assert!(invalidation.is_islands_target(&first));

        // Deleting/retargeting the symlink makes canonicalize(alias) fail or
        // point somewhere new. The retained lexical alias must still trigger
        // the rebuild that refreshes the canonical side of the registry.
        std::fs::remove_file(&alias).unwrap();
        assert!(invalidation.is_islands_target(&alias));
        std::os::unix::fs::symlink(&second, &alias).unwrap();
        assert!(invalidation.is_islands_target(&alias));
        invalidation.replace_islands([alias.clone()]);
        assert!(invalidation.is_islands_target(&second));
        assert!(!invalidation.is_islands_target(&first));
    }

    #[cfg(unix)]
    #[test]
    fn raw_invalidation_aliases_missing_candidates_through_canonical_parent() {
        let project = tempfile::tempdir().unwrap();
        let physical = project.path().join("physical");
        let linked = project.path().join("linked");
        std::fs::create_dir_all(physical.join("src")).unwrap();
        std::os::unix::fs::symlink(&physical, &linked).unwrap();
        let logical_candidate = linked.join("src/tsconfig.json");
        let physical_candidate = physical.join("src/tsconfig.json");

        let invalidation = RawImportInvalidation::default();
        invalidation.replace_client_script_workers([logical_candidate.clone()]);
        assert!(invalidation
            .client_script_worker_paths()
            .contains(&zfb_types::normalize_path_lexical(&logical_candidate)));
        let resolved_candidate =
            RawImportInvalidation::resolved_alias(&physical_candidate).unwrap();
        assert!(invalidation
            .client_script_worker_paths()
            .contains(&resolved_candidate));
        assert!(invalidation.is_client_script_worker_target(&physical_candidate));

        std::fs::write(&physical_candidate, r#"{"compilerOptions":{}}"#).unwrap();
        assert!(invalidation.is_client_script_worker_target(&physical_candidate));
        assert!(invalidation.is_client_script_worker_target(&logical_candidate));
    }

    /// Issue #1710 — the `client_script_siblings` registry mirrors the
    /// canonical/missing-path aliasing and stale-set-replacement coverage the
    /// three pre-existing registries already have above.
    #[cfg(unix)]
    #[test]
    fn client_script_sibling_invalidation_aliases_missing_candidates_through_canonical_parent() {
        let project = tempfile::tempdir().unwrap();
        let physical = project.path().join("physical");
        let linked = project.path().join("linked");
        std::fs::create_dir_all(physical.join("lib/shared")).unwrap();
        std::os::unix::fs::symlink(&physical, &linked).unwrap();
        let logical_candidate = linked.join("lib/shared/plain.ts");
        let physical_candidate = physical.join("lib/shared/plain.ts");

        let invalidation = RawImportInvalidation::default();
        invalidation.replace_client_script_siblings([logical_candidate.clone()]);
        assert!(invalidation
            .client_script_sibling_paths()
            .contains(&zfb_types::normalize_path_lexical(&logical_candidate)));
        let resolved_candidate =
            RawImportInvalidation::resolved_alias(&physical_candidate).unwrap();
        assert!(invalidation
            .client_script_sibling_paths()
            .contains(&resolved_candidate));
        assert!(invalidation.is_client_script_sibling_target(&physical_candidate));

        std::fs::write(&physical_candidate, "export const plain = 'ZFB_SIBLING';\n").unwrap();
        assert!(invalidation.is_client_script_sibling_target(&physical_candidate));
        assert!(invalidation.is_client_script_sibling_target(&logical_candidate));

        // A successful second scan replaces (rather than appends to) the
        // graph — dropping a sibling that stopped being reachable must stop
        // triggering client-script reruns.
        invalidation.replace_client_script_siblings(Vec::new());
        assert!(!invalidation.is_client_script_sibling_target(&physical_candidate));
        assert!(!invalidation.is_client_script_sibling_target(&logical_candidate));
    }

    /// #1581 — the registry's whole job: recognise an entry the dev session
    /// already walked, so the orchestrator can tell a spurious FSEvents
    /// `Created` apart from a genuinely new file.
    #[test]
    fn known_content_entries_recognises_seeded_paths() {
        let known = KnownContentEntries::default();
        known.insert_many([PathBuf::from("/proj/content/post.md")]);

        assert!(known.contains(Path::new("/proj/content/post.md")));
        assert!(!known.contains(Path::new("/proj/content/never-seen.md")));
    }

    /// #1581 — an out-of-root collection root that could not be canonicalised
    /// keeps a literal `..` component (the `canonicalize_or_lexical` fallback
    /// for a not-yet-created dir). The registry collapses it lexically on both
    /// insert and lookup, so it still matches the path `notify` delivers.
    #[test]
    fn known_content_entries_collapses_dot_dot_on_both_sides() {
        let known = KnownContentEntries::default();
        known.insert_many([PathBuf::from("/proj/../shared-content/posts/alpha.mdx")]);

        assert!(
            known.contains(Path::new("/shared-content/posts/alpha.mdx")),
            "a `..`-carrying out-of-root entry must match its collapsed form"
        );
    }

    /// #1581 — a removed directory takes its entries with it, so a file
    /// recreated underneath it is genuinely new again and routes through
    /// discovery rather than normalizing to an in-place edit.
    #[test]
    fn known_content_entries_removes_descendants_of_a_removed_directory() {
        let known = KnownContentEntries::default();
        known.insert_many([
            PathBuf::from("/proj/content/nested/x.md"),
            PathBuf::from("/proj/content/nested/deep/y.md"),
            PathBuf::from("/proj/content/keep.md"),
        ]);

        known.remove_path_and_descendants(Path::new("/proj/content/nested"));

        assert!(!known.contains(Path::new("/proj/content/nested/x.md")));
        assert!(!known.contains(Path::new("/proj/content/nested/deep/y.md")));
        assert!(
            known.contains(Path::new("/proj/content/keep.md")),
            "a sibling outside the removed directory must survive the purge"
        );
    }
}
