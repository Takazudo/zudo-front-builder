//! `zfb dev` — boot the dev pipeline + dev HTTP server.
//!
//! Wires four crates together per the doc-comment in
//! [`zfb_server`]'s lib.rs:
//!
//! 1. A [`tokio::sync::broadcast`] channel of [`zfb_server::ReloadEvent`]s
//!    that the SSE live-reload route consumes.
//! 2. A [`zfb_server::PageCache`] of rendered HTML keyed by URL path.
//! 3. A [`zfb_build::BuildOrchestrator`] driving the watcher + dep-graph
//!    + asset pipeline; its `on_outcome` callback translates every
//!      non-noop tick into reload events via
//!      [`zfb_server::outcome_to_events`] and broadcasts them.
//! 4. A long-lived [`zfb_build::renderer::RendererState`] (T7) that
//!    owns the embedded V8 host. The asset pipeline's
//!    [`PageRenderer`] callback drives [`zfb_build::renderer::render_one`]
//!    against this state per affected route, so a single edit triggers
//!    one in-process render — not a fresh host boot.
//!
//! Then it binds the address from `args.host:args.port`, prints the
//! ready banner via [`crate::output::ready`], and runs the axum server
//! until Ctrl+C.
//!
//! ## Lifecycle of the renderer state
//!
//! `start(...)` is called once at boot. The returned
//! [`zfb_build::renderer::RendererState`] is wrapped in a [`Drop`]
//! guard so panics, Ctrl-C, or any other early-exit path tears the
//! embedded V8 host down cleanly. Without that guard a panicking
//! dev loop would leak the host resources.
//!
//! ## Configuration
//!
//! Project configuration is loaded via [`crate::config::load_from_dir`]
//! at startup. Today this resolves to a `zfb.config.json` if present, or
//! sensible defaults otherwise; encountering a `zfb.config.ts` produces a
//! clear "not yet supported" error.
//!
//! **Precedence rule:** CLI args (`--host`, `--port`) override the
//! corresponding config values when supplied.
//!
//! ## v1 → wave-3 transition
//!
//! Earlier waves passed a noop [`PageRenderer`] into the orchestrator
//! that returned an empty `RenderedPage` list — the dev cache stayed
//! empty and every request fell through to the dev 404 body.
//!
//! Wave 3 (T7) replaces that noop with a renderer-backed callback. The
//! callback maps each page id to its [`zfb_build::renderer::RouteUniverseEntry`]
//! and drives [`zfb_build::renderer::render_one`] against the long-
//! lived [`zfb_build::renderer::RendererState`]. The result file is
//! read back into a [`zfb_build::RenderedPage`] so the existing
//! atomic-write + reload-broadcast plumbing keeps working unchanged.
//!
//! ### Same gaps as `zfb build`
//!
//! Dynamic / catchall routes (`paths()` runtime expansion) and the
//! Worker-entry wrapping are still pending. See [`crate::commands::build`]
//! for the detailed gap analysis. Today the dev renderer reuses the
//! same plumbing and inherits the same limitations.

// V8-off (issue #371, sub-task 4.1a): on the `!feature = "embed_v8"`
// path the `pub async fn run` body and the V8-bearing helpers below
// are compiled out. The imports, helper functions, and types they
// reference then look unused — silence the lints in that
// configuration so V8-off builds stay warning-clean. The V8-on path
// continues to surface real unused-item warnings.
#![cfg_attr(not(feature = "embed_v8"), allow(unused_imports, dead_code))]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
#[cfg(feature = "embed_v8")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
#[cfg(feature = "embed_v8")]
use sha2::{Digest as _, Sha256};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use zfb_build::bundler::{bundle_with_session, BundleMode, BundlerOutput, ShadowSession};
use zfb_build::renderer::{
    render_one, shutdown, start, Backend, RendererStartInput, RendererState, RouteUniverseEntry,
};
use zfb_build::{
    BuildContext, BuildOrchestrator, BuildOutcome, ClientScriptsRunner, ContentCollectionId,
    ContentCollectionMembership, ContentProvenance, CssRunner, DevAssetPipeline, DiscoveryOutcome,
    IslandsBundleInfo, IslandsRunner, OrchestratorConfig, PageRenderer, RefreshOutcome,
    RelDistPath, RenderedPage, RendererReloader, TrackedContentRead,
};
use zfb_graph::persist::{load_from_disk, save_to_disk, ManifestDigest};
use zfb_graph::{DependencyGraph, PageDeps, PageId};
use zfb_server::{
    outcome_to_events, serve_with_listener, InjectedRouteSet, PageCache, Redirects,
    RedirectsHandle, ReloadEvent, ServeOpts, SsrDispatcher, SsrRouteRecord, SsrRouteSet,
    SsrRoutesHandle,
};

use crate::cli::DevArgs;
use crate::commands::resolve::{resolve_addr, resolve_host, resolve_port, resolve_under_root};
use crate::config;
use crate::output;
use crate::render_pipeline::{
    build_prerender_map, build_route_universe, check_runtime_installed,
    eval_deferred_paths_via_worker, expand_dynamic_routes, WorkerDispatch,
};
#[cfg(feature = "embed_v8")]
use zfb_render::paths::PathsCache;

/// Default source directories the watcher follows.
//
// Issue #1165 — `public/` is served directly from disk by the dev
// server (static-file middleware) and does NOT feed the dep-graph or
// the renderer. Watching it caused `compute_manifest_digest` to walk
// every file in `public/` via WalkDir+metadata() BEFORE the TCP
// listener bound, producing a visible pre-bind hang on projects with
// large asset directories. Custom `publicDir` values that happen to
// overlap a real source root (e.g. `src/public`) are still watched
// because `derive_watch_roots` adds all configured collection paths
// and source roots independently — only the literal default `"public"`
// is excluded here.
//
// `node_modules` is deliberately absent (S5 / epic #1228 §4).
// Injected-route entrypoints live under `node_modules/@takazudo/…` —
// they are compiled package artifacts, not project source, and are
// therefore **restart-only**: editing the package's own source requires
// a `zfb dev` restart. Content the injected route READS (watched
// collections under `content/`, custom collection paths, …) DOES
// live-refresh — a content edit triggers a tick that rebuilds the
// snapshot, which the route sees. The supported data-loading channel for
// an injected route is a DYNAMIC route whose top-level `paths()` calls
// `getCollection(…)` (a static injected route's own `getStaticProps` is
// NOT forwarded by the static overlay synthesizer — see
// `commands::package_routes::synthesize_static_overlay_module`). The
// per-swap stale-marks force a re-render on the next request:
// `mark_injected_seeds_stale` for STATIC injected seeds, and
// `restale_dynamic_injected` for previously-rendered DYNAMIC injected
// outputs (S5 #1233 / #1227 item (h)).
const DEFAULT_WATCH_ROOTS: &[&str] = &[
    "pages",
    "content",
    "components",
    "layouts",
    "styles",
    "data",
    // `src` is a classification / islands / client-script root (policy.rs) and
    // routes commonly import components from `src/components/**`. Without it,
    // `src/**` edits fire no FS event and no tick at all, so a consuming route
    // never re-renders (#1284 symptom-A, watch half).
    "src",
    "zfb.config.json",
    "zfb.config.ts",
];

/// Watch-root basenames excluded from the missing-target boot warning
/// (issue #1391). These are the mutually-exclusive `zfb.config.*` files:
/// a project has at most one, so warning about the absent variant(s)
/// would be pure noise on every boot. See [`missing_watch_targets`].
const WATCH_WARN_SKIP: &[&str] = &["zfb.config.json", "zfb.config.ts"];

/// Strip `.` components so `./src/mdx` and `src/mdx` compare equal in
/// the dedupe / coverage checks below.
fn normalize_relative(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect()
}

/// The dev watcher's source roots: [`DEFAULT_WATCH_ROOTS`] plus each
/// configured collection's `path`.
///
/// Collections may live anywhere in the project (`CollectionDef::path`
/// is project-root-relative, e.g. `src/mdx/notes`) — only watching the
/// fixed default roots means edits under such a collection never produce
/// a watcher event and the dev server serves stale HTML until restart.
///
/// Rules:
/// - paths are normalized (leading `./` stripped) before comparison;
/// - a collection path already covered by an earlier root is skipped
///   (`content/blog` is inside the default `content` root; nested
///   collections dedupe against each other after the shallow-first
///   sort), avoiding double event delivery from overlapping recursive
///   watches;
/// - missing directories are tolerated downstream — the watcher warns
///   and skips them at registration.
fn derive_watch_roots(cfg: &config::Config) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = DEFAULT_WATCH_ROOTS.iter().map(PathBuf::from).collect();
    let mut collection_paths: Vec<PathBuf> = cfg
        .collections
        .iter()
        .map(|c| normalize_relative(&c.path))
        .filter(|p| !p.as_os_str().is_empty())
        // #1550 — an out-of-root collection (`allowOutsideRoot`, a `..`
        // escape) must NOT ride the relative watch-root list: a literal
        // `project_root.join("../x")` keeps a `..` component that never
        // matches notify's canonical event paths. Those roots are routed to
        // the absolute extras channel via [`ResolvedRoots`] instead.
        .filter(|p| !collection_path_escapes_root(p))
        .collect();
    // Shallow-first so a parent collection root lands before its
    // children and the coverage check below collapses the family to
    // the parent.
    collection_paths.sort_by_key(|p| p.components().count());
    for p in collection_paths {
        let covered = roots.iter().any(|r| p.starts_with(r));
        if !covered {
            roots.push(p);
        }
    }
    roots
}

/// Does a project-root-relative collection path escape the project root
/// via `..`? A purely LEXICAL check (no filesystem access): walk the
/// components tracking depth, and report an escape the moment a
/// `ParentDir` would pop above the root (or an absolute component is
/// seen). `.` components are ignored (already stripped by
/// [`normalize_relative`], but tolerated here too).
///
/// This is the routing predicate for #1550. An escaping collection path
/// is only reachable at all because wave-1's `allowOutsideRoot` opt-in
/// let it pass config validation (#1549); it must ride the absolute
/// extras watch channel rather than the relative watch-root list.
fn collection_path_escapes_root(path: &Path) -> bool {
    let mut depth: i32 = 0;
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            // An absolute component means this was never an in-root
            // project-relative path in the first place.
            Component::Prefix(_) | Component::RootDir => return true,
            Component::Normal(_) => depth += 1,
        }
    }
    false
}

/// Collapse `.` / `..` components lexically (no filesystem access). Used
/// as the fallback when [`canonicalize_or_lexical`]'s target is absent.
fn lexical_normalize(abs: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in abs.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                // Pop the last real segment; a root/prefix stays put.
                if !out.pop() {
                    out.push(comp.as_os_str());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Canonicalise `abs` (an already-absolute path), falling back to a
/// LEXICAL `..`-collapse when the target does not exist yet.
///
/// `notify` delivers canonical event paths, so an out-of-root collection
/// root must be canonical to compare against them. But canonicalisation
/// hits the filesystem and FAILS for a directory the user has not created
/// yet — we must not panic or drop the root (dropping it would silently
/// disable watching once the dir appears). The lexical fallback yields a
/// stable, deterministic absolute path for the missing-target warning and
/// the manifest digest; it won't match live events until the dir exists
/// AND `zfb dev` is restarted — the same "restart after creating" contract
/// the extras channel already documents for a missing target.
fn canonicalize_or_lexical(abs: &Path) -> PathBuf {
    abs.canonicalize()
        .unwrap_or_else(|_| lexical_normalize(abs))
}

/// Boot-time resolved-root inventory (issue #1550).
///
/// Built ONCE at dev boot from `(project_root, cfg)` and threaded through
/// every site that compares a configured collection root against a
/// filesystem event path. The problem it solves: `notify` delivers
/// CANONICAL event paths (macOS canonicalises `/tmp` → `/private/tmp`, and
/// any symlink in the path is resolved), whereas
/// `project_root.join("../pkg/src")` keeps a literal `..` component — so a
/// lexical `strip_prefix` / `starts_with` of an out-of-root collection
/// root against an event path never matches. That silently degraded three
/// dev sites (`derive_tick_candidates` narrowing, `seed_frontmatter_hashes`
/// keying, `make_discovery_hook` created-file acceptance) for a collection
/// living outside the project root.
///
/// Each out-of-root root is canonicalised exactly ONCE here (lexical
/// fallback for a not-yet-created dir) and read back everywhere — no
/// per-site re-canonicalisation, which would drift on deletion or a
/// symlink alias.
pub(crate) struct ResolvedRoots {
    /// Project-root-RELATIVE watch roots handed to `OrchestratorConfig::new`
    /// and the manifest digest: `DEFAULT_WATCH_ROOTS` + every IN-root
    /// collection path, deduped/collapsed by [`derive_watch_roots`].
    /// Out-of-root collections are deliberately absent — they ride the
    /// absolute extras channel (`out_of_root_watch_roots`). For a project
    /// with no out-of-root collection this is byte-identical to the
    /// pre-#1550 `derive_watch_roots(cfg)` output.
    relative_watch_roots: Vec<PathBuf>,
    /// Absolute, CANONICAL roots of out-of-root collections, routed through
    /// `Watcher::start_with_extras` (the #368 absolute channel). Deduped
    /// among themselves; the caller dedupes them against the resolved
    /// `extraWatchPaths` set at the merge point.
    out_of_root_watch_roots: Vec<PathBuf>,
    /// Per-collection resolved ABSOLUTE root, index-aligned with
    /// `cfg.collections`. In-root: the literal `project_root.join(path)`
    /// (byte-identical to the pre-#1550 form). Out-of-root: canonical
    /// (lexical fallback when absent). Read by `seed_frontmatter_hashes`,
    /// `derive_tick_candidates`, and `make_discovery_hook` so each site's
    /// root form matches the notify event paths for that collection.
    collection_roots: Vec<PathBuf>,
}

impl ResolvedRoots {
    /// In-root relative watch roots (for `OrchestratorConfig::new`).
    fn relative_watch_roots(&self) -> &[PathBuf] {
        &self.relative_watch_roots
    }

    /// Canonical absolute out-of-root roots (for the extras channel,
    /// policy `content_roots`, and the manifest digest).
    fn out_of_root_watch_roots(&self) -> &[PathBuf] {
        &self.out_of_root_watch_roots
    }

    /// Per-collection resolved absolute root, index-aligned with
    /// `cfg.collections`.
    fn collection_roots(&self) -> &[PathBuf] {
        &self.collection_roots
    }

    /// Roots for the persisted-graph manifest digest: the relative in-root
    /// roots PLUS the canonical out-of-root roots. Without the latter,
    /// routing out-of-root content through the extras channel would drop it
    /// from `.zfb/graph.bin` invalidation — an external-content edit would
    /// not flip the digest and a stale cached graph could be reused.
    fn manifest_digest_roots(&self) -> Vec<PathBuf> {
        let mut roots = self.relative_watch_roots.clone();
        roots.extend(self.out_of_root_watch_roots.iter().cloned());
        roots
    }
}

/// Build the [`ResolvedRoots`] inventory for a dev session (issue #1550) —
/// the single source of truth every root-vs-event-path comparison reads.
pub(crate) fn resolve_roots(project_root: &Path, cfg: &config::Config) -> ResolvedRoots {
    let relative_watch_roots = derive_watch_roots(cfg);
    let mut out_of_root_watch_roots: Vec<PathBuf> = Vec::new();
    let mut collection_roots: Vec<PathBuf> = Vec::with_capacity(cfg.collections.len());
    for collection in &cfg.collections {
        let norm = normalize_relative(&collection.path);
        let abs_literal = project_root.join(&norm);
        if collection_path_escapes_root(&norm) {
            // Out-of-root: canonicalise once so the root matches notify's
            // canonical event paths, and register it on the absolute extras
            // channel.
            let canonical = canonicalize_or_lexical(&abs_literal);
            if !out_of_root_watch_roots.contains(&canonical) {
                out_of_root_watch_roots.push(canonical.clone());
            }
            collection_roots.push(canonical);
        } else {
            // In-root: the literal join — byte-identical to the pre-#1550
            // form the three content sites used.
            collection_roots.push(abs_literal);
        }
    }
    ResolvedRoots {
        relative_watch_roots,
        out_of_root_watch_roots,
        collection_roots,
    }
}

/// Compute which derived watch roots + `extraWatchPaths` targets are
/// absent from disk at boot (issue #1391).
///
/// `zfb_watcher::Watcher::start_with_extras` already skips a missing
/// path and does NOT re-register it if it appears later — but the only
/// signal is a `tracing::warn!`, which a user running `zfb dev` from a
/// terminal never sees (this crate's user-facing messages go through
/// [`crate::output`]). Absent a warning here, "run `zfb dev` before
/// `mkdir content`" silently degrades into a no-reload mode for that
/// root until the user restarts.
///
/// Kept as a pure function (returns the missing paths rather than
/// printing them) so the boot path's `output::warn` side effect stays a
/// thin, untested wrapper around unit-testable logic — mirroring the
/// `fmt_*` / `warn` split in `crate::output`.
///
/// The two `zfb.config.*` entries in [`DEFAULT_WATCH_ROOTS`] are
/// deliberately EXCLUDED from the warning: they are mutually-exclusive
/// config *files* (a project carries at most one, and a defaults-only
/// project carries neither), so at least one is ALWAYS absent. Warning
/// about them would fire on every single boot and drown out the real
/// signal this exists for — a missing content/source *directory* that
/// silently degrades into no-reload. A missing config file is the normal
/// steady state, not a degraded mode.
fn missing_watch_targets(
    project_root: &Path,
    watch_roots: &[PathBuf],
    extra_watch_paths: &[PathBuf],
) -> Vec<PathBuf> {
    let mut missing = Vec::new();
    for root in watch_roots {
        let is_config_file = root
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| WATCH_WARN_SKIP.contains(&n))
            .unwrap_or(false);
        if is_config_file {
            continue;
        }
        let full = project_root.join(root);
        if !full.exists() {
            missing.push(full);
        }
    }
    // `extra_watch_paths` are already absolute by this point (resolved via
    // `resolve_extra_watch_paths`, `session.out_of_root_watch_targets()`, or
    // `resolve_css_import_watch_targets` — all canonicalise before adding).
    for extra in extra_watch_paths {
        if !extra.exists() {
            missing.push(extra.clone());
        }
    }
    missing
}

/// Load `public/_redirects` for the initial dev-server boot (issue
/// #1546). A missing file is the common case — `_redirects` support is
/// opt-in — so it stays silent and produces an empty [`Redirects`]; any
/// other read failure (permissions, a directory named `_redirects`,
/// etc.) is warned once so it is not silently swallowed. Malformed
/// lines within a readable file are handled by [`Redirects::parse`]
/// itself (warn-and-skip per line).
fn load_redirects_at_boot(path: &Path) -> Redirects {
    match std::fs::read_to_string(path) {
        Ok(contents) => Redirects::parse(&contents),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Redirects::default(),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "_redirects: failed to read file at boot; starting with an empty ruleset"
            );
            Redirects::default()
        }
    }
}

/// Re-parse `public/_redirects` after the targeted watcher (see
/// [`spawn_redirects_watch`]) observes a create/edit/delete of the file.
///
/// Unlike [`load_redirects_at_boot`], every read failure here —
/// including "file no longer exists" (the delete case) — warns exactly
/// once per reload attempt: a watch event firing at all means the
/// file's state just changed, so silently reverting to an empty
/// ruleset would hide a real (if expected) event from the developer.
fn reload_redirects(path: &Path) -> Redirects {
    match std::fs::read_to_string(path) {
        Ok(contents) => Redirects::parse(&contents),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "_redirects: failed to read file on reload; using an empty ruleset"
            );
            Redirects::default()
        }
    }
}

/// Start a targeted, dedicated watch for `public/_redirects` (issue
/// #1546) and return the live [`RedirectsHandle`] the dev router reads
/// from.
///
/// `public/` is deliberately excluded from [`DEFAULT_WATCH_ROOTS`] (the
/// per-request on-disk fallback already serves it live, and a recursive
/// watch of the whole directory would trigger the `BuildOrchestrator`'s
/// rebuild pipeline for every static-asset edit — see the `public_root`
/// leg in `zfb-server`'s `serve_page`). `_redirects` still needs a
/// watch of its own so rule edits take effect without a dev-server
/// restart, but it must NOT ride on the orchestrator's watcher (that
/// would mean a rule change either does nothing or wastefully triggers
/// a full rebuild, depending on how the orchestrator classifies an
/// unrecognised `public/` path). So this spins up a wholly separate
/// [`zfb_watcher::Watcher`] instance — decoupled from the orchestrator
/// and its `on_outcome` reload-broadcast plumbing — dedicated to this
/// one file.
///
/// Implementation mirrors the orchestrator's own
/// [`zfb_watcher::Watcher::watch_additional_files`] pattern used for
/// dynamic dependency files: watch the PARENT directory (`public/`)
/// non-recursively rather than the file itself, so a delete-then-
/// recreate (the shape most editors' "save" produces) stays visible —
/// watching the file's own inode directly can silently stop firing
/// once that inode is unlinked. Because a non-recursive directory watch
/// reports every entry in that directory, not just `_redirects`, the
/// consumer loop below filters events down to the exact filename
/// before reloading (the "event filtering" the extras-channel doc talks
/// about — see the `zfb_watcher` module docs for the channel mechanics,
/// issue #368).
///
/// The returned `Watcher` is moved into the spawned task so it — and
/// therefore the OS-level watch — stays alive for the lifetime of that
/// task (i.e. the dev server process); dropping it would stop the
/// watch immediately.
fn spawn_redirects_watch(project_root: &Path, public_root: &Path) -> RedirectsHandle {
    let redirects_path = public_root.join("_redirects");
    let handle: RedirectsHandle = Arc::new(std::sync::RwLock::new(load_redirects_at_boot(
        &redirects_path,
    )));

    let (mut watcher, mut rx) = match zfb_watcher::Watcher::start_with_debounce(
        project_root,
        std::iter::empty::<&Path>(),
        zfb_watcher::DEFAULT_DEBOUNCE,
    ) {
        Ok(pair) => pair,
        Err(e) => {
            // Non-fatal: the dev server still boots and serves whatever
            // ruleset was parsed at boot — it just won't pick up later
            // edits without a restart. Mirrors `missing_watch_targets`'s
            // "degrade, don't fail the boot" posture for watch setup.
            tracing::warn!(
                error = %e,
                "_redirects: failed to start targeted watcher; edits to public/_redirects \
                 will require a dev-server restart to take effect"
            );
            return handle;
        }
    };
    // `public/` does not exist at all on a fresh project with no static
    // assets — `watch_additional_files` already warns-and-skips a
    // missing parent, matching `load_redirects_at_boot`'s silence on a
    // missing file.
    watcher.watch_additional_files([redirects_path.clone()]);

    let handle_for_task = Arc::clone(&handle);
    tokio::spawn(async move {
        // Keep the watcher (and therefore the OS-level watch) alive for
        // as long as this task runs.
        let _watcher = watcher;
        while let Some(change) = rx.recv().await {
            let is_redirects_file = change
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n == "_redirects")
                .unwrap_or(false);
            if !is_redirects_file {
                continue;
            }
            let updated = reload_redirects(&redirects_path);
            match handle_for_task.write() {
                Ok(mut guard) => *guard = updated,
                Err(poisoned) => *poisoned.into_inner() = updated,
            }
        }
    });

    handle
}

/// Entry point for `zfb dev`.
///
/// Available only when the `embed_v8` cargo feature is on (issue #371,
/// sub-task 4.1a). The V8-off counterpart lives at the bottom of this
/// file and surfaces a clear runtime error explaining that this binary
/// was built without V8 support.
#[cfg(feature = "embed_v8")]
pub async fn run(args: &DevArgs) -> Result<()> {
    // 1. Resolve the project root and load configuration.
    let project_root = std::env::current_dir().context("failed to read current working dir")?;

    let cfg = config::load_from_dir(&project_root)
        .await
        .context("failed to load project configuration")?;

    // Configured `outDir` is dev's read-only production seed; live dev HTML
    // and assets use the isolated scratch roots established below.
    let dist_root = resolve_under_root(&project_root, &cfg.out_dir);
    let public_root = resolve_under_root(&project_root, &cfg.public_dir);

    if !dist_root.exists() {
        std::fs::create_dir_all(&dist_root)
            .with_context(|| format!("failed to create dist dir {}", dist_root.display()))?;
    }

    // Issue #534 — dev's per-route HTML writes must NOT land in the
    // project's `outDir` (`dist/`). When dev was given the same `dist_dir`
    // as the build pipeline, starting `pnpm dev` after a clean `pnpm build`
    // overwrote every prod HTML file with a dev-mode rendering that lacked
    // the production `<link rel="stylesheet">` / islands `<script
    // type="module">` head injections (`prod_head_assets`). The on-disk
    // copy then silently broke a subsequent `pnpm preview`, which serves
    // `dist/` verbatim.
    //
    // The dev server primarily serves HTML from its in-memory `PageCache`
    // (URL-keyed — see `zfb_server::routes::PageCache`), so the on-disk
    // copy is mostly a stale by-product of the read-back-after-render step
    // in `DevRenderSession::render_one_with`. Redirecting it to a
    // dev-only directory under `.zfb-build/` keeps the read-back working
    // while taking dev out of the production output's write set entirely.
    //
    // The dev server's page disk-fallback probes `dev_html_root` FIRST
    // (issue #534 — dev's per-route renders land there, kept out of the
    // production `dist_root` write set), then `public_root`, and finally
    // the prebuilt `dist_root` as a Dev-only, last-resort boot-lazy seed
    // (issues #1057 / #1182 / #1390, in `zfb-server` `serve_page`): on a
    // cold cache it serves whatever the most recent `pnpm build` left
    // there — "build, then dev for a quick check" — and is safe because
    // dev only READS `dist_root`, never mutates it. Probed AFTER
    // `public_root` (#1390) so a live `public/` edit is never shadowed by
    // a stale build copy. The seed self-heals for HTML routes: once dev
    // renders a route into `dev_html_root`, the fresh bytes win.
    let dev_html_root = dev_html_root_for(&project_root);
    // Guard against a pathological `outDir` (`.zfb-build/dev-pages`,
    // its parent, or anything that overlaps): when the dev HTML root
    // collides with `dist_root` we are back to the #534 condition and
    // dev's per-route writes corrupt `pnpm preview`'s output. Refuse
    // to start with a clear error so the user can pick a non-colliding
    // `outDir`.
    if dev_html_root == dist_root
        || dev_html_root.starts_with(&dist_root)
        || dist_root.starts_with(&dev_html_root)
    {
        anyhow::bail!(
            "zfb dev: configured `outDir` ({}) overlaps with the dev HTML \
             scratch root ({}). Pick an `outDir` outside `.zfb-build/` \
             (the default `dist/` is fine) — otherwise dev's per-route \
             HTML writes would corrupt the production build output.",
            dist_root.display(),
            dev_html_root.display(),
        );
    }
    if !dev_html_root.exists() {
        std::fs::create_dir_all(&dev_html_root).with_context(|| {
            format!("failed to create dev html dir {}", dev_html_root.display())
        })?;
    }

    // Issue #1189 — dev's STABLE served assets (`styles.css`, `islands.js`,
    // island chunks, `client/*.js`) must NOT land in the project's `outDir`
    // (`dist/`) either. They used to, which meant a one-off `zfb build`
    // against the shared `dist/` (a quick prod sanity-check while dev is
    // live) wiped them and re-emitted HASHED-only assets — leaving the
    // dev-served `/assets/styles.css` a 404 (unstyled, no self-heal). Write
    // them to an isolated `.zfb-build/dev-assets/` dir instead (mirroring
    // the #534 dev-HTML isolation); the router serves `/assets/*` from there
    // first and falls back to `dist/assets/` for a boot-lazy prebuilt seed.
    let dev_assets_root = dev_assets_root_for(&project_root);
    // Symmetric guard to the dev-HTML one above: a pathological `outDir`
    // that overlaps the dev-assets scratch root re-creates the clobber.
    if dev_assets_root == dist_root
        || dev_assets_root.starts_with(&dist_root)
        || dist_root.starts_with(&dev_assets_root)
    {
        anyhow::bail!(
            "zfb dev: configured `outDir` ({}) overlaps with the dev asset \
             scratch root ({}). Pick an `outDir` outside `.zfb-build/` \
             (the default `dist/` is fine) — otherwise a one-off `zfb build` \
             would clobber the dev-served `/assets/styles.css`.",
            dist_root.display(),
            dev_assets_root.display(),
        );
    }
    if !dev_assets_root.exists() {
        std::fs::create_dir_all(&dev_assets_root).with_context(|| {
            format!(
                "failed to create dev assets dir {}",
                dev_assets_root.display()
            )
        })?;
    }

    let host = resolve_host(args.host.as_deref(), cfg.host.as_deref(), DEFAULT_DEV_HOST);
    let port = resolve_port(args.port, cfg.port, DEFAULT_DEV_PORT);
    let addr = resolve_addr(host.as_str(), port)?;

    let (tx, _rx) = broadcast::channel::<ReloadEvent>(64);
    let pages = PageCache::new();

    // Plugin lifecycle. Spawn the host once at boot so
    // `preBuild` runs before the bundler/renderer start, and so dev-
    // middleware registrations can be installed into the dev server.
    // The host is dropped when this `run` returns (Ctrl+C path), which
    // kills the subprocess.
    let plugin_host = crate::commands::plugins::maybe_spawn_host(&cfg).await?;

    // #255 / #260 / #261 / #268 — shared plugin setup phase:
    // setup → virtual-module prefetch → alias/virtual-module derivation.
    //
    // `SetupCommand::Dev` is the per-command difference (build uses
    // `SetupCommand::Build`).
    let plugin_setup = crate::commands::plugins::run_plugin_setup(
        &plugin_host,
        &project_root,
        &cfg,
        zfb_build::SetupCommand::Dev,
    )
    .await?;

    // preBuild runs before devMiddleware registration and the bundler.
    // Dev exposes the configured production root to plugin hooks for API
    // parity, while zfb's own live render outputs use isolated scratch roots
    // and treat `dist_root` only as a prebuilt seed.
    if let Some(h) = plugin_host.as_ref() {
        let ctx = zfb_build::BuildHookContext {
            project_root: project_root.clone(),
            out_dir: dist_root.clone(),
            config: serde_json::to_value(&cfg)
                .context("plugin lifecycle: serialise config for preBuild ctx")?,
            // dev mode: routes always absent on preBuild (no manifest yet).
            routes: None,
        };
        h.run_pre_build(&ctx)
            .await
            .map_err(zfb_build::annotate_with_plugin_error)
            .context("preBuild lifecycle hook")?;
    }

    // Dev-only: register devMiddleware hooks.
    let plugin_set = if let Some(h) = plugin_host.as_ref() {
        crate::commands::plugins::build_dev_middleware_set(
            h,
            &project_root,
            &cfg,
            zfb_server::ServerMode::Dev,
        )
        .await?
    } else {
        None
    };

    // Dev-only: the InjectedRouteSet handed to the dev server is built from
    // the POST-precedence survivor set, NOT the raw registration list (epic
    // #1228, S3 #1231, §7 / sharp edges 4/7). The survivor selection runs
    // inside `boot_dev_renderer` (same function that stages the dev bundle),
    // so it is read off the dev session below (after the session is built) —
    // a user-shadowed or package-vs-package-dropped pattern is already absent
    // and can never match in the request-time fallback. `None` when the
    // renderer is disabled or no injected route survived (parity).

    let v8_plugin_hooks = plugin_setup.v8_plugin_hooks;
    let dev_plugin_alias_entries = plugin_setup.plugin_alias_entries;
    let dev_plugin_virtual_modules = plugin_setup.plugin_virtual_modules;
    // Keep setup_registries in scope for the lifetime of the dev session —
    // the hook entries hold references into it.
    let setup_registries = plugin_setup.setup_registries;
    // #1196 — package-registered client entries from addClientEntry.
    let registered_client_entries = setup_registries.client_entries.clone();

    // Issue #1182 — decide whether the eager dev bundle is DEFERRED past
    // `TcpListener::bind`. Only in boot-lazy mode with a servable prebuilt
    // `dist/`, and not opted out (#1188): then the prebuilt `dist/` serves every
    // route until the deferred boot task publishes the renderer, so first-accept
    // is O(1) regardless of project size (the residual of #1161 that #1166/#1170
    // left behind). The defer gate is a strict subset of the boot-lazy gate, so a
    // deferred boot always takes the boot-lazy branch in `run_boot_render`; the
    // opt-out (`ZFB_DEV_DEFER_BUNDLE=0`, #1188) lets an SSR-heavy project fall back
    // to the eager pre-bind renderer (no SSR-only 404 window). Decided here (before
    // `boot_dev_renderer`) so the scaffold-vs-eager choice and the deferred task
    // agree on one value.
    let defer_dev_bundle = defer_dev_bundle_decision(
        lazy_dev_render_enabled(),
        std::env::var("ZFB_DEV_BOOT_LAZY").ok().as_deref(),
        dist_is_servable_seed(&dist_root),
        std::env::var("ZFB_DEV_DEFER_BUNDLE").ok().as_deref(),
    );

    // #1550 — build the boot-time resolved-root inventory ONCE, before the
    // renderer boots, and thread it through every root-vs-event-path site
    // (renderer seeding, watch-root partition, policy content roots, and the
    // manifest digest). Out-of-root collections (`allowOutsideRoot`, #1549)
    // are canonicalised here so they match notify's canonical event paths.
    let root_inventory = resolve_roots(&project_root, &cfg);

    // 2. Stand up the long-lived renderer state if the project looks
    //    runnable. We surface failures as a warning + fall back to the
    //    noop renderer so the dev server still boots — the user can
    //    still poke at the dev URL while they fix the underlying
    //    bundler / runtime issue.
    let dev_session = match boot_dev_renderer(
        &project_root,
        &cfg,
        // #1550 — the per-collection resolved roots the renderer's content
        // sites (frontmatter-hash seeding, tick narrowing, created-file
        // discovery) key on; canonical for out-of-root collections.
        root_inventory.collection_roots(),
        v8_plugin_hooks,
        dev_plugin_alias_entries.clone(),
        dev_plugin_virtual_modules.clone(),
        // S2 (#1230) — the package-owned injected routes registered during
        // setup. Boot materialises the survivor set into a session-lifetime
        // staging dir and threads it into the dev bundler so the injected
        // entrypoints land in the dev bundle. Empty on the parity path.
        setup_registries.injected_routes.as_slice(),
        defer_dev_bundle,
    ) {
        Ok(s) => Some(s),
        Err(err) => {
            output::warn(format!(
                "renderer disabled — falling back to empty page cache: {err:#}",
            ));
            None
        }
    };

    // S3 (#1231) — read the POST-precedence survivor InjectedRouteSet off the
    // dev session (built inside `boot_dev_renderer` from the same survivor set
    // that staged the dev bundle and seeded the static routes). `None` when
    // the renderer is disabled (no session) or no injected route survived
    // precedence — both keep `serve_page`'s injected-route leg inert.
    let injected_route_set: Option<zfb_server::InjectedRouteSet> = dev_session
        .as_ref()
        .map(|session| session.injected_route_set())
        .filter(|set| !set.is_empty());

    // 3. Build orchestrator setup.
    //
    // Issues #1166, #1170 — the manifest-digest + persisted-graph load +
    // graph seed + boot render + eager islands bundle are all DEFERRED past
    // `TcpListener::bind` (see the deferred task in step 7 below). The
    // cold-start hang #1161 reports is `compute_manifest_digest`'s
    // `WalkDir`+`metadata()` walk over the watched tree; #1170's is the
    // islands `"use client"` scan + esbuild bundle — both ran before bind
    // and made the port's reachability scale with the watched-tree /
    // dependency-tree SIZE. Binding first and doing this work on a
    // background task makes the server accept connections in O(1) regardless
    // of tree size, completing #1057's "serve immediately" intent.
    //
    // Cold-start optimisation (now performed inside the deferred task):
    // try to reuse a previously persisted graph from `.zfb/graph.bin`.
    // If the manifest digest still matches the current project layout,
    // deserialise and reuse — otherwise build fresh and save the new
    // graph back on shutdown so the *next* cold start is fast.
    // Includes configured IN-root collection paths (e.g. `src/mdx/notes`)
    // so edits there produce watcher events; the manifest digest covers
    // them automatically since it walks the same roots. Out-of-root
    // collections (#1550) are NOT here — they ride the extras channel below
    // and are folded into the digest via `manifest_digest_roots`.
    let watch_roots: Vec<PathBuf> = root_inventory.relative_watch_roots().to_vec();
    let graph_cache_path = project_root.join(".zfb").join("graph.bin");

    // The graph starts EMPTY. The deferred task (step 7) loads the
    // persisted graph (if the digest matches) and seeds it from
    // `session.page_ids()` BEFORE it runs the orchestrator loop or the
    // boot render — so the orchestrator never observes the empty graph
    // during its loop. `graph_for_save` keeps a handle for the shutdown
    // persistence path; the orchestrator owns the other clone.
    //
    // We deliberately do NOT write a fresh empty graph here on a cache
    // miss. If we did, a `zfb dev` killed before the orchestrator's
    // first watcher tick would persist an empty graph tagged with the
    // current digest — and the next cold start would happily reuse that
    // empty cache as authoritative. Save only on shutdown (below), once
    // the graph has actually been populated AND the digest is known.
    let graph = Arc::new(Mutex::new(DependencyGraph::new()));
    let graph_for_save = Arc::clone(&graph);

    // #1284/#1287 — give the dev session a handle to the same graph so every
    // bundle refresh populates per-route `DepKind::Module` edges from esbuild's
    // metafile (`populate_module_edges`). The graph is created here, AFTER
    // `boot_dev_renderer` returns, so the boot bundle's edges are seeded by the
    // first post-boot refresh; from then on a component edit maps to its
    // consuming route via `dirty_pages`.
    #[cfg(feature = "embed_v8")]
    if let Some(session) = dev_session.as_ref() {
        session.set_dep_graph(Arc::clone(&graph));
        // Seed the eager boot bundle's Module edges now the graph exists, so a
        // component edit maps to its consuming route from the first edit tick
        // (#1284/#1287). No-op on the deferred-boot path.
        session.seed_boot_module_edges();
    }

    // Issue #1166 — the manifest digest is now produced inside the
    // deferred task (it is the expensive walk we moved past bind). The
    // shutdown persistence path (step 8) reads it through this shared
    // slot. If Ctrl+C arrives BEFORE the digest completes the slot is
    // still `None` and the save is SKIPPED — never write a graph tagged
    // with a wrong/absent digest.
    let manifest_digest_slot: Arc<Mutex<Option<ManifestDigest>>> = Arc::new(Mutex::new(None));
    // Issue #1025 — wire the stale probe so each tick's BuildOutcome
    // reports the routes the lazy render callback marked stale
    // (`pages_stale`). Always wired when a render session exists; the
    // probe drains an empty buffer on every tick while the lazy switch
    // is off, so behaviour is unchanged today.
    let pipeline = match dev_session.as_ref() {
        Some(session) => {
            let probe_session = session.clone();
            DevAssetPipeline::with_stale_probe(Arc::new(move || {
                probe_session.inner.take_tick_stale()
            }))
        }
        None => DevAssetPipeline::new(),
    };
    // Issue #1026 — clone the request-time write handle out BEFORE the
    // pipeline is moved into the orchestrator below. The lazy render
    // adapter writes stale-route renders through it so request writes
    // share the tick path's validate → dedup → atomic-write → commit
    // discipline and its tick-vs-request exclusion (#1024).
    let request_writer = pipeline.request_writer();
    let mut extra_watch_paths = resolve_extra_watch_paths(&cfg.extra_watch_paths);
    // #1284/#1287 (D4) — register the eager boot bundle's out-of-root real
    // Module deps (canonicalised symlink targets of workspace `.tsx` deps
    // esbuild resolved through `node_modules`) as extra watch targets, so an
    // edit of the real workspace file fires a tick. `notify` does not follow
    // symlinks and `node_modules` is excluded from the recursive watch, so
    // without this a symlinked workspace component edit produces no event. The
    // in-repo `src/**` case needs none of this — it is covered by
    // `DEFAULT_WATCH_ROOTS`. Empty on the deferred-boot path (the boot bundle
    // has not run yet there; that case relies on the in-repo `src` root).
    #[cfg(feature = "embed_v8")]
    if let Some(session) = dev_session.as_ref() {
        // Boot-time-only limitation (#1293): these targets are resolved once
        // from the eager boot bundle's metafile and registered before the
        // `BuildOrchestrator` is constructed.  Any NEW out-of-root dep paths
        // discovered by later bundle refreshes (tick N+1, N+2, …) are NOT
        // dynamically added to the watcher — the watcher's watch set is fixed
        // after `OrchestratorConfig` is built here.  A `zfb dev` restart is
        // required to pick up a newly-symlinked workspace dep as a watch
        // target.  On the deferred-boot path this set is always empty (the
        // boot bundle has not run yet), so this limitation is only relevant
        // on the eager path.
        for target in session.out_of_root_watch_targets() {
            if !extra_watch_paths.contains(&target) {
                extra_watch_paths.push(target);
            }
        }
    }
    // #1288 (D4) — auto-watch the CSS `@import` graph. `notify` does not
    // follow symlinks, and `node_modules` is excluded, so a transitively
    // imported / symlinked-workspace-dep CSS file (`@import './tokens.css'`,
    // `@import '@scope/design-system'`) is never watched and editing it never
    // refreshes `/assets/styles.css`. Resolve the entry's `@import` graph to
    // canonicalised real paths and register them as extra watch targets — no
    // manual `extraWatchPaths` config. The resolver already canonicalises, so
    // these align with the watcher's canonical event paths. The out-of-root
    // `.css` real paths classify as `PathClass::Style` (whitelisted extension),
    // so editing one fires `rerun_css` and refreshes the asset.
    //
    // Boot-time-only limitation (#1293): `resolve_css_import_watch_targets`
    // walks the CSS entry's `@import` graph once at boot and the result is
    // fixed for the lifetime of the `BuildOrchestrator`.  If a new `@import`
    // is added to the CSS entry during a dev session, that new transitive dep
    // is NOT registered as a watch target until `zfb dev` is restarted.
    let resolved_css_imports = resolve_css_import_watch_targets(&project_root);
    for real in &resolved_css_imports {
        if !extra_watch_paths.contains(real) {
            extra_watch_paths.push(real.clone());
        }
    }
    // #1550 — out-of-root collections (`allowOutsideRoot`, #1549) ride the
    // absolute extras channel: their canonical root matches notify's
    // canonical event paths, which a literal `project_root.join("../x")`
    // relative watch root never would. Dedupe against anything an explicit
    // `extraWatchPaths` entry (or the #1284 / #1288 auto-watch resolvers
    // above) already registered, so an out-of-root collection that also
    // appears under `extraWatchPaths` is not double-watched.
    for root in root_inventory.out_of_root_watch_roots() {
        if !extra_watch_paths.contains(root) {
            extra_watch_paths.push(root.clone());
        }
    }
    // Issue #1391 — every watch root + extra target is now finalised;
    // warn about any that don't exist yet so the silent no-reload mode
    // (see `missing_watch_targets` doc) is at least visible in the
    // dev server's own console output before the user hits it.
    for missing in missing_watch_targets(&project_root, &watch_roots, &extra_watch_paths) {
        output::warn(format!(
            "watch target {} does not exist yet — it will not be watched \
             until you restart `zfb dev` after creating it",
            missing.display(),
        ));
    }
    // Configured collection roots classify as Content ahead of the
    // standard root-segment walk — without this, a collection under
    // `src/` (e.g. `src/mdx/notes`) classifies as Module and wastefully
    // re-bundles islands on every entry edit.
    //
    // #1550 — in-root collections stay RELATIVE (matched via the
    // project-relative arm of `classify_change_with_content_roots`, byte-
    // identical to before). Out-of-root collections contribute their
    // CANONICAL ABSOLUTE root instead: the relative `../x` form never
    // prefixes a canonical event path, so the policy's `path.starts_with`
    // arm needs the absolute canonical root to classify an external `.md`
    // as Content.
    let mut content_roots: Vec<PathBuf> = cfg
        .collections
        .iter()
        .map(|c| normalize_relative(&c.path))
        .filter(|p| !p.as_os_str().is_empty())
        .filter(|p| !collection_path_escapes_root(p))
        .collect();
    for root in root_inventory.out_of_root_watch_roots() {
        if !content_roots.contains(root) {
            content_roots.push(root.clone());
        }
    }
    let raw_import_invalidation = zfb_build::RawImportInvalidation::default();

    // #1581 — seed the session-live known-content registry from the boot
    // collection MEMBERSHIP walk (not the frontmatter-hash map, which drops
    // unparseable entries). This is what lets the #1058 spurious-`Created`
    // normalization recognise a pre-existing entry on a COLD boot, where the
    // dependency graph carries no `DepKind::Content` reverse edge for it yet.
    // Without it, the first in-place edit of any collection entry that macOS
    // FSEvents coalesces into `Created` loses the whole tick's #958 eager
    // narrowing and re-stamps every route.
    let known_content = zfb_build::KnownContentEntries::default();
    known_content.insert_many(collect_collection_entries(
        &cfg,
        root_inventory.collection_roots(),
    ));

    let orch_config = OrchestratorConfig::new(&project_root, watch_roots.clone())
        .with_extra_watch_paths(extra_watch_paths)
        .with_policy(
            zfb_build::GranularityPolicy::default()
                .with_content_roots(content_roots)
                .with_raw_import_invalidation(raw_import_invalidation.clone())
                .with_known_content(known_content.clone()),
        );
    let orchestrator = BuildOrchestrator::new(orch_config, graph, pipeline);

    let render_pages: PageRenderer = match dev_session.as_ref() {
        // Issue #534 — pass `dev_html_root` (under `.zfb-build/`), not
        // `dist_root`, so per-route dev renders do not overwrite the
        // production HTML files that `pnpm preview` serves.
        Some(session) => make_render_callback(session.clone(), dev_html_root.clone()),
        None => Arc::new(|_pages: &[PageId], _narrowing| Ok(Vec::new())),
    };

    // Issue #377 — dev-mode initial-load islands bundling.
    //
    // Build the same IslandsPluginConfig the production runner uses so dev
    // and build resolve aliases / virtual modules identically inside the
    // islands esbuild invocation. Then:
    //
    // 1. Run an initial bundle eagerly here, before the dev server starts
    //    accepting requests, so the FIRST GET on a page with an island
    //    already sees a `<script type="module" src="/assets/islands.js">`
    //    in the served HTML (the bug the issue reports — without this
    //    initial pass the orchestrator would only emit the bundle on
    //    the first file-change tick, leaving cold-boot page loads silent).
    // 2. Seed `islands_bundle_url` with the result (or leave it `None` when
    //    the project has no `"use client"` components — projects without
    //    islands must not ship a `<script>` tag pointing at a non-existent
    //    bundle).
    // 3. Wire the same wrapper as the orchestrator's `run_islands` callback
    //    so a watcher tick re-bundles, rewrites the shared URL handle, and
    //    surfaces an `IslandsBundleInfo` that flows through the existing
    //    `outcome_to_events` -> SSE `Islands` event path (the dev livereload
    //    client then re-imports the new bundle in place).
    // Reuse the alias / virtual-module lists already derived from
    // `setup_registries` higher up (so the islands esbuild and the main
    // dev bundler resolve plugin-registered modules identically).
    let islands_plugin_config = crate::commands::build::IslandsPluginConfig {
        alias_entries: dev_plugin_alias_entries.clone(),
        virtual_modules: dev_plugin_virtual_modules.clone(),
    };
    // The dev server only serves assets it mounted itself. For a path-
    // shaped `base` (e.g. `/foo/`) the mount prefix is `Some("/foo")`
    // and the bundle URL must include it. For an absolute-URL `base`
    // (CDN deploy target — `https://cdn.example.com/`)
    // `dev_mount_prefix` returns `None` because the dev server mounts
    // at root; injecting the absolute URL would point the browser at
    // an origin the dev server never serves, breaking first-load
    // hydration for the CDN deploy scenario. Source the prefix from
    // `dev_mount_prefix` (not `asset_url_base_prefix`) so absolute-URL
    // bases collapse cleanly to "no prefix" here.
    let dev_islands_url_prefix: String =
        zfb_types::dev_mount_prefix(cfg.base.as_deref()).unwrap_or_default();
    let islands_bundle_url_handle: zfb_server::IslandsBundleUrl =
        Arc::new(std::sync::RwLock::new(None));

    // Tracks chunk + module-worker companion filenames written by the most
    // recent islands bundle so the next tick can prune stale files.
    let live_companion_filenames: Arc<Mutex<HashSet<String>>> =
        Arc::new(Mutex::new(HashSet::new()));

    // Issue #1170 — the eager initial islands bundle used to run HERE,
    // synchronously, before `TcpListener::bind`. On a large-dependency
    // consumer its `"use client"` scan + esbuild bundle was the dominant
    // pre-bind cost (the last size-bound step #1166 had not yet moved). It
    // now runs in the deferred boot task (step 7b) via `rebundle_islands`,
    // publishing the bundle URL late through `islands_bundle_url_handle`
    // and folding the result into the boot `BuildOutcome` so a browser that
    // loaded during the pre-bundle window gets a `ReloadEvent::Islands` and
    // hydrates. Until the bundle lands the handle stays `None` and the
    // server simply omits the `<script type="module">` (the same contract
    // the project-has-no-islands path already ships).

    let run_islands: Option<IslandsRunner> = {
        let project_root = project_root.clone();
        // Issue #1189/#1501: write islands.js + companions to the isolated
        // dev-assets root, not the build-shared `dist/`.
        let dev_assets_root_for_islands = dev_assets_root.clone();
        let plugin_cfg = islands_plugin_config.clone();
        let framework = cfg.framework;
        let bundle_config = cfg.bundle.clone();
        let url_prefix = dev_islands_url_prefix.clone();
        let url_handle = Arc::clone(&islands_bundle_url_handle);
        let companion_names = Arc::clone(&live_companion_filenames);
        let raw_invalidation = raw_import_invalidation.clone();
        // The watcher tick and the deferred boot build (issue #1170) share
        // ONE implementation — `rebundle_islands` — so the boot-time bundle
        // and every rebundle tick write islands.js, prune companions, and
        // publish the shared URL handle identically (no drift). The watcher
        // path propagates the `Err` (an esbuild / disk failure on a tick
        // should fail loudly); the boot path catches it and warns-and-
        // continues (see step 7b).
        Some(Arc::new(move || -> Result<Option<IslandsBundleInfo>> {
            rebundle_islands(
                &project_root,
                &dev_assets_root_for_islands,
                framework,
                bundle_config.as_ref(),
                &plugin_cfg,
                &url_prefix,
                &url_handle,
                &companion_names,
                &raw_invalidation,
            )
        }))
    };

    // Issue #494 / #498: wire the CSS runner end-to-end, mirroring the
    // islands runner above.
    //
    // Step 1: shared URL handle — the dev server reads from this on every
    // HTML response; the runner writes to it on every CSS rebuild tick.
    let css_bundle_url_handle: zfb_server::CssBundleUrl = Arc::new(std::sync::RwLock::new(None));

    // Step 2: eager initial CSS bundle at boot so the very first page
    // request already carries a `<link rel="stylesheet">` tag.
    // Failures are non-fatal — we warn and let the dev server boot with
    // unstyled HTML. The hot-rebuild path will retry on the next file
    // change.
    let dev_css_url_prefix: String =
        zfb_types::dev_mount_prefix(cfg.base.as_deref()).unwrap_or_default();
    match crate::commands::build::build_default_css_payload(
        &project_root,
        &dev_assets_root,
        &cfg,
        &[],
    ) {
        Ok(Some(payload)) => {
            // Write the bytes to the isolated dev-assets root (issue #1189)
            // so `GET /assets/styles.css` is immediately serveable (unlike
            // islands, the CSS pipeline does not write to disk as a
            // side-effect of building).
            let out_path = dev_assets_root.join(&payload.relative_path);
            if let Some(parent) = out_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::write(&out_path, &payload.bytes).is_ok() {
                let url = if dev_css_url_prefix.is_empty() {
                    payload.stable_url
                } else {
                    format!("{dev_css_url_prefix}{}", payload.stable_url)
                };
                if let Ok(mut guard) = css_bundle_url_handle.write() {
                    *guard = Some(url);
                }
            } else {
                output::warn(
                    "initial CSS bundle: failed to write bytes to dist (no <link> until rebuild)",
                );
            }
        }
        Ok(None) => {
            // No CSS to ship (no authored globals, no CSS Modules, and —
            // when Tailwind is enabled — no scannable sources). Leave the
            // handle at None. Tailwind being disabled no longer implies
            // None on its own: authored CSS still ships (issue #824).
        }
        Err(err) => {
            output::warn(format!(
                "initial CSS bundle failed (no <link rel=\"stylesheet\"> \
                 will be injected until the next successful rebuild): {err:#}"
            ));
        }
    }

    // Step 3: CssRunner closure — re-invokes the payload builder, writes
    // fresh bytes to disk, and updates the shared URL handle.
    let run_css: Option<CssRunner> = {
        let project_root_for_css = project_root.clone();
        // Issue #1189: build + write CSS into the isolated dev-assets root.
        let dev_assets_root_for_css = dev_assets_root.clone();
        let cfg_for_css = cfg.clone();
        let url_prefix = dev_css_url_prefix.clone();
        let url_handle = Arc::clone(&css_bundle_url_handle);
        Some(Arc::new(move || -> Result<bool> {
            let payload = crate::commands::build::build_default_css_payload(
                &project_root_for_css,
                &dev_assets_root_for_css,
                &cfg_for_css,
                &[],
            )?;
            let mut guard = url_handle.write().unwrap_or_else(|p| {
                tracing::warn!(
                    site = "dev.run_css.url_handle",
                    "rwlock poisoned, recovered"
                );
                p.into_inner()
            });
            let Some(payload) = payload else {
                // CSS disabled / no sources this tick — clear the URL so
                // subsequent HTML responses don't reference a stale file.
                *guard = None;
                return Ok(false);
            };
            // Write fresh bytes to the isolated dev-assets root so the dev
            // server serves them immediately. This is the "freshness proof"
            // the acceptance test checks (byte-for-byte match between
            // payload.bytes and GET /assets/styles.css).
            let out_path = dev_assets_root_for_css.join(&payload.relative_path);
            if let Some(parent) = out_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(err) = std::fs::write(&out_path, &payload.bytes) {
                tracing::warn!("css runner: failed to write bytes to dist: {err}");
                return Ok(false);
            }
            let bundle_url = if url_prefix.is_empty() {
                payload.stable_url
            } else {
                format!("{url_prefix}{}", payload.stable_url)
            };
            *guard = Some(bundle_url);
            // Return true unconditionally on a successful emit so the
            // orchestrator marks outcome.css_changed = true and the
            // livereload SSE event fires. The URL is stable so the bytes
            // update in place on disk.
            Ok(true)
        }))
    };

    // Client-scripts: eager initial bundle at boot + watcher-driven
    // rebuild. Mirrors the islands and CSS patterns above.
    //
    // Tracks every entry/worker basename written by the most recent bundle so
    // the next rebuild can prune removed/renamed entries and stale workers.
    let live_client_script_outputs: Arc<Mutex<std::collections::HashSet<String>>> =
        Arc::new(Mutex::new(std::collections::HashSet::new()));

    // Eager boot bundle — non-fatal, mirrors islands / CSS.
    match crate::commands::build::build_dev_client_scripts_to_disk_with_plugin_config(
        &project_root,
        // Issue #1189: client scripts go to the isolated dev-assets root.
        &dev_assets_root,
        cfg.framework,
        cfg.bundle.as_ref(),
        &std::collections::HashSet::new(),
        &registered_client_entries,
        &islands_plugin_config,
    ) {
        Ok((_, outputs, raw_targets, worker_targets)) => {
            if let Ok(mut guard) = live_client_script_outputs.lock() {
                *guard = outputs;
            }
            raw_import_invalidation.replace_client_scripts(raw_targets);
            raw_import_invalidation.replace_client_script_workers(worker_targets);
        }
        Err(err) => {
            output::warn(format!(
                "initial client-scripts bundle failed (no client scripts will be served \
                 until the next successful rebuild): {err:#}"
            ));
        }
    }

    // Watcher-driven rebuild closure.
    let run_client_scripts: Option<ClientScriptsRunner> = {
        let project_root_for_cs = project_root.clone();
        // Issue #1189: rebuild client scripts into the isolated dev-assets root.
        let dev_assets_root_for_cs = dev_assets_root.clone();
        let framework = cfg.framework;
        let bundle_config = cfg.bundle.clone();
        let output_filenames = Arc::clone(&live_client_script_outputs);
        // #1196 — capture registered entries for the watcher closure.
        let registered_for_cs = registered_client_entries.clone();
        let plugin_config_for_cs = islands_plugin_config.clone();
        let raw_invalidation = raw_import_invalidation.clone();
        Some(Arc::new(move || -> Result<bool> {
            let prev = output_filenames
                .lock()
                .unwrap_or_else(|p| {
                    tracing::warn!(
                        site = "dev.run_client_scripts.output_filenames",
                        "mutex poisoned, recovered"
                    );
                    p.into_inner()
                })
                .clone();
            let (changed, new_outputs, raw_targets, worker_targets) =
                crate::commands::build::build_dev_client_scripts_to_disk_with_plugin_config(
                    &project_root_for_cs,
                    &dev_assets_root_for_cs,
                    framework,
                    bundle_config.as_ref(),
                    &prev,
                    &registered_for_cs,
                    &plugin_config_for_cs,
                )?;
            let mut guard = output_filenames.lock().unwrap_or_else(|p| {
                tracing::warn!(
                    site = "dev.run_client_scripts.output_filenames (write)",
                    "mutex poisoned, recovered"
                );
                p.into_inner()
            });
            *guard = new_outputs;
            raw_invalidation.replace_client_scripts(raw_targets);
            raw_invalidation.replace_client_script_workers(worker_targets);
            Ok(changed)
        }))
    };

    // Issue #807 — build the live SSR routes handle here, before the
    // reload_renderer closure, so the closure can hold a clone of the
    // Arc and update it on each tick. The dispatcher is built once and
    // shared across all refreshes (the Arc<Mutex<Option<RendererState>>>
    // it wraps is the same one the refresh swaps the new host into).
    let ssr_route_set = build_ssr_route_set(dev_session.as_ref());

    // Per-tick bundle refresh for EDIT ticks. Without this the renderer
    // stays bound to the boot-time bundle: the content snapshot and page
    // modules are baked in at bundle time, so an in-place save
    // (`ChangeKind::Modified` — what VS Code and most editors emit)
    // re-rendered byte-identical stale HTML forever. Only rename-replace
    // saves worked, by accident, through the watch-ADD discovery path.
    // The pipeline skips this when the tick's plan is already
    // renderer-fresh (boot initial render, watch-ADD discovery
    // re-bundle), so each tick bundles at most once.
    let reload_renderer: Option<RendererReloader> = dev_session.as_ref().map(|session| {
        let session = session.clone();
        let html_root = dev_html_root.clone();
        // Clone the handle so the closure can write a fresh SsrRouteSet
        // into it after each bundle refresh (issue #807). `None` when
        // the renderer is disabled (no SSR in this project).
        let ssr_handle_for_reload = ssr_route_set.clone();
        Arc::new(move || {
            let (changed, vanished_rel) = match session
                .refresh_bundle_and_routes()
                .context("edit-tick bundle refresh failed")?
            {
                // Phase-B skip (issue #940): the live host, route tables,
                // and SSR route set were all left untouched — report
                // `Skipped` so DevAssetPipeline bypasses the render
                // fan-out for the tick (issue #956).
                BundleRefresh::Skipped => return Ok(RefreshOutcome::Skipped),
                BundleRefresh::Refreshed { changed, vanished } => (changed, vanished),
            };
            if !changed.is_empty() {
                tracing::debug!(
                    count = changed.len(),
                    "edit-tick refresh changed route sets"
                );
            }
            // Issue #807 — update the live SSR route set so newly-added or
            // removed `prerender = false` routes are visible to the request
            // dispatcher on the next request, without a dev-server restart.
            if let Some(handle) = &ssr_handle_for_reload {
                refresh_live_ssr_routes(&session, handle);
            }
            // Convert relative vanished output paths to absolute dist paths
            // so DevAssetPipeline can delete them directly.
            let vanished_abs: Vec<std::path::PathBuf> = vanished_rel
                .into_iter()
                .map(|rel| html_root.join(rel))
                .collect();
            if !vanished_abs.is_empty() {
                tracing::debug!(
                    count = vanished_abs.len(),
                    "edit-tick refresh found globally-vanished routes"
                );
            }
            // Issue #958 — propagate the refresh's changed-source set
            // instead of dropping it: a non-empty set means the route
            // structure moved this tick, so the dev pipeline must
            // disable content narrowing (fallback G5).
            Ok(RefreshOutcome::Refreshed {
                vanished: vanished_abs,
                changed_sources: changed,
            })
        }) as RendererReloader
    });

    let ctx = BuildContext {
        // Issue #534 — `DevAssetPipeline::apply` writes
        // `RenderedPage.html` to `<ctx.dist_root>/<output_path>` on every
        // watcher tick (see `crates/zfb-build/src/pipeline/dev.rs`). This
        // is the second HTML writer in the dev pipeline (the first being
        // the renderer itself, redirected above via `make_render_callback`).
        // Both must point at `dev_html_root` for dev to fully stop touching
        // the project's `outDir`; redirecting only the renderer leaves the
        // pipeline's downstream write to clobber `dist/` again. Note that
        // dev's `BuildContext` is only consumed by `DevAssetPipeline`, so
        // this swap is local to dev — production builds use a separate
        // `BuildContext` constructed in `commands::build`.
        dist_root: dev_html_root.clone(),
        render_pages,
        run_css,
        run_islands,
        run_client_scripts,
        reload_renderer,
    };

    // 4. on_outcome — translate each tick into reload events.
    let tx_cb = tx.clone();
    let on_outcome = move |outcome: &BuildOutcome| {
        for ev in outcome_to_events(outcome) {
            let _ = tx_cb.send(ev);
        }
    };

    // 5. Build the watch-ADD discovery hook (issue #659).
    //
    // `discover_hook` makes a content file CREATED after boot
    // discoverable without a `zfb dev` restart: it rebundles the
    // content snapshot, reloads the embedded V8 host in place, re-expands
    // `paths()`, and rebuilds the dev session's source→route table. Built
    // from `dev_session` (the V8-backed renderer); `None` when the
    // renderer is disabled, which keeps the legacy add-needs-restart
    // behaviour.
    let discover_hook: Option<zfb_build::DiscoveryHook> = dev_session.as_ref().map(|session| {
        make_discovery_hook(
            session.clone(),
            dev_html_root.clone(),
            // Issue #807 — clone the live handle so the discovery hook can
            // rewrite it on a watch-ADD tick (the pipeline skips
            // reload_renderer when the discovery refresh marked the
            // renderer fresh).
            ssr_route_set.clone(),
            known_content.clone(),
        )
    });

    // Issue #1166 — handles the deferred boot task needs that ServeOpts
    // also consumes below. Clone them now, before ServeOpts moves the
    // originals: the deferred task computes the manifest digest (which
    // needs `project_root` + the digest roots), loads + seeds the graph, and
    // runs the boot render against `dev_session`.
    let project_root_for_boot = project_root.clone();
    // #1550 — the digest walks the in-root relative roots PLUS the canonical
    // out-of-root roots. Routing out-of-root content through the extras
    // channel removed it from `watch_roots`, so without folding it back in
    // here an external-content edit would not flip the persisted-graph digest
    // and a stale `.zfb/graph.bin` could be reused.
    let digest_watch_roots = root_inventory.manifest_digest_roots();
    let dev_session_for_boot = dev_session.clone();
    let graph_for_seed = Arc::clone(&graph_for_save);
    let graph_cache_path_for_boot = graph_cache_path.clone();
    let manifest_digest_slot_for_boot = Arc::clone(&manifest_digest_slot);
    let dist_root_for_boot = dist_root.clone();
    // Issue #1189: the deferred boot's islands rebundle writes to the
    // isolated dev-assets root (NOT `dist_root_for_boot`, which `run_boot_render`
    // still needs as the real `dist/` for its servable-seed check).
    let dev_assets_root_for_boot = dev_assets_root.clone();
    // Issue #1170 — the deferred boot task also runs the eager islands
    // bundle (the last size-bound step that used to gate the bind). Clone
    // its inputs now, before `ServeOpts` / `run_islands` consume the
    // originals: `rebundle_islands` needs the project + dev-assets roots, the
    // framework, the islands plugin config, the URL prefix, the shared
    // bundle-URL handle, and the live-companion tracker.
    let islands_url_handle_for_boot = Arc::clone(&islands_bundle_url_handle);
    let islands_companion_names_for_boot = Arc::clone(&live_companion_filenames);
    let islands_plugin_config_for_boot = islands_plugin_config.clone();
    let raw_import_invalidation_for_boot = raw_import_invalidation.clone();
    let islands_url_prefix_for_boot = dev_islands_url_prefix.clone();
    let framework_for_boot = cfg.framework;
    let bundle_config_for_boot = cfg.bundle.clone();
    // Issue #1182 — the deferred boot task publishes the live SSR route handle
    // after the deferred bundle lands (`refresh_bundle_and_routes` swaps the
    // session's tables but NOT the server's `ssr_route_set` handle — its
    // callers do that). Clone the handle now, before `ServeOpts` consumes the
    // original below. `None` when the renderer is disabled. Inert unless
    // `defer_dev_bundle` is set.
    let ssr_route_set_for_boot = ssr_route_set.clone();

    // 6. Build the serve options and announce readiness.
    //
    // Issue #229: thread `cfg.base` through so the dev server mounts
    // pages, assets, and live-reload under the same prefix the build
    // pipeline stamps onto asset URLs. Without this the dev HTML emits
    // `<link href="/<base>/assets/styles.css">` while the dev server
    // only knew about unprefixed `/assets/...` — every request 404s.
    // Issue #367 / #807 — `ssr_route_set` was built before the
    // reload_renderer closure above so the closure holds a clone of the
    // Arc and can update it on each tick (making added/removed
    // `prerender = false` routes visible without a restart).

    // Issue #1026 — render-on-request hook (the zfb-side impl of the
    // #1020 seam). ONE persistent handle built at boot: the adapter
    // captures the session clone (whose renderer Arc is swapped in
    // place on every refresh), the pipeline's request writer, and the
    // dev HTML root — so it never needs rewiring from the refresh
    // seams. Installed ONLY when the lazy switch (#1025) is on — the
    // switch is resolved once at boot and immutable for the session, so
    // gating the install keeps the eager (`ZFB_DEV_EAGER=1`) serve path
    // literally hook-free (no per-GET handle snapshot/spawn; review
    // finding on #1026). The adapter's own early-return stays as
    // defense in depth.
    let render_on_request_hook = dev_session
        .as_ref()
        .filter(|session| session.lazy_render_enabled())
        .map(|session| {
            // S4 (#1232) — pass the POST-precedence injected-route set into
            // the adapter so the dynamic-route fallback can match request URLs
            // against injected patterns at request time (design record §2 /
            // sharp edges 4/7). `injected_route_set` is `None` on the parity
            // path (no injected survivors) → `unwrap_or_default()` produces
            // an empty set, which makes the fallback a no-op.
            let injected = injected_route_set.clone().unwrap_or_default();
            crate::lazy_render_adapter::make_render_on_request_handle(
                session.clone(),
                request_writer.clone(),
                dev_html_root.clone(),
                injected,
            )
        });

    // Issue #1546 — `_redirects` dev integration. Loads `public/_redirects`
    // (an empty ruleset when absent) and starts the targeted watch that
    // keeps it live for the rest of this session. Must run before
    // `project_root` / `public_root` are moved into `ServeOpts` below.
    let redirects_handle = spawn_redirects_watch(&project_root, &public_root);

    let opts = ServeOpts {
        project_root,
        dist_root,
        // Issue #1189 — serve `/assets/*` from the isolated dev-assets root
        // first (clobber-proof: a concurrent `zfb build` can't touch
        // `.zfb-build/`), falling back to `dist_root/assets` for a boot-lazy
        // prebuilt seed's hashed assets. `dist_root` above stays the real
        // `dist/` so the seed fallback and `dist_is_servable_seed` still work.
        dev_assets_root: Some(dev_assets_root.clone()),
        // Issue #534 — point the page-cache disk fallback at the dev
        // HTML dir, not the project's `outDir`. With `dist_root` here
        // (the historical wiring) the dev server's `read_from_dist`
        // would serve whatever `pnpm build` last wrote, hiding the
        // dev pipeline's edits because nothing populates
        // `PageCache` for HTML at runtime. Pairing the read with the
        // write side keeps every dev tick observable in the browser.
        html_root: dev_html_root.clone(),
        public_root,
        addr,
        pages,
        broadcast: tx,
        plugins: plugin_set,
        injected_routes: injected_route_set,
        ssr_routes: ssr_route_set,
        base: cfg.base.clone(),
        trailing_slash: cfg.trailing_slash,
        mode: zfb_server::ServerMode::Dev,
        // Issue #377: the dev server holds the same Arc as the
        // `run_islands` callback above; rebuild ticks rewrite the
        // inner Option, page responses read it on every served HTML
        // request. Cloning the Arc is cheap (refcount bump).
        islands_bundle_url: Some(Arc::clone(&islands_bundle_url_handle)),
        // Issue #494 / #498: same pattern as islands — the dev server
        // holds the same Arc as the `run_css` callback above; CSS
        // rebuild ticks rewrite the inner Option, page responses read it
        // on every served HTML request.
        css_bundle_url: Some(Arc::clone(&css_bundle_url_handle)),
        // Issue #931: Host-header allowlist for non-localhost binds.
        // `allowedHosts` config entries plus the explicitly bound host;
        // the server disables enforcement entirely for loopback binds.
        allowed_hosts: cfg.allowed_hosts.clone(),
        bound_host: Some(host.clone()),
        // Issue #1020 seam / #1026 impl: the lazy render adapter built
        // above. `None` when the renderer is disabled (no session to
        // render through) or the lazy switch (#1025) is off (the
        // `ZFB_DEV_EAGER=1` hatch) — which keeps serve_page's hook leg
        // entirely inert.
        render_on_request_hook,
        // Issue #1546 — see `spawn_redirects_watch` above.
        redirects: Some(redirects_handle),
    };

    // 7. Bind the TCP listener FIRST — before the manifest-digest walk,
    //    the persisted-graph load, the graph seed, and the boot render —
    //    so cold-start reachability is independent of the watched-tree
    //    SIZE (issue #1166; completes #1057's "serve immediately"
    //    intent). The expensive `WalkDir`+`metadata()` digest walk
    //    (#1161) and the boot render then run on a background task while
    //    the server already accepts connections.
    //
    //    Ordering guarantees preserved:
    //    - The port-in-use error still surfaces BEFORE the ready banner:
    //      we bind here, return on failure, and only print the banner
    //      after a successful bind.
    //    - No leaked background task on a failed bind: the deferred task
    //      is spawned ONLY after the bind succeeds, so a bind failure
    //      returns with nothing to abort.
    let listener = match TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind dev server to {addr}"))
    {
        Ok(l) => l,
        Err(e) => return Err(e),
    };

    // 7b. Deferred boot task (issues #1166, #1170). Now that the listener
    //     is bound and the server is about to accept connections, run the
    //     work that used to block the bind:
    //       1. compute_manifest_digest (the size-bound walk we moved)
    //       2. load_persisted_graph (digest-gated cache reuse)
    //       3. seed the graph from session.page_ids()
    //       4. boot render (boot-lazy mark-stale vs eager initial_build)
    //       5. eager islands bundle (issue #1170 — the other size-bound
    //          step we moved; folded into the boot outcome so a tab loaded
    //          during the pre-bundle window gets a livereload and hydrates)
    //     then orchestrator.run drains the watcher loop.
    //     The digest is published into `manifest_digest_slot` so the
    //     shutdown persistence path (step 8) can read it; until it lands
    //     the slot stays `None` and an early Ctrl+C skips the save.
    //
    //     REQUEST-BEFORE-RENDER RACE (eager mode): a GET can arrive
    //     before this task's boot render finishes. The dev server's serve
    //     waterfall is `PageCache → html_root → public_root → dist_root →
    //     404` (zfb-server `serve_page`): until the eager render writes a
    //     route's HTML, the request is served from the prebuilt `dist/`
    //     (the Dev-gated `read_from_dist(dist_root, …)` seed leg, last
    //     before the 404) if a servable copy exists, and otherwise gets
    //     the controlled
    //     `DEV_404_BODY` — a complete, well-formed HTML page carrying the
    //     live-reload script that auto-upgrades the moment the real render
    //     lands. It is NEVER a wrong/empty/partial body. This matches the
    //     boot-lazy contract (#1057), which seeds `dist/` and re-renders
    //     on first request via the render-on-request hook installed in
    //     `ServeOpts` above (already live before the first accept, so a
    //     request can never race a not-yet-installed hook).
    let boot_handle = tokio::spawn(async move {
        // Test-only slow-step injection (issue #1166 regression guard):
        // `ZFB_DEV_TEST_SLOW_DIGEST_MS` makes the deferred boot work
        // sleep BEFORE the digest walk, so a test can prove the port
        // accepts connections / answers HTTP while this slow step is
        // still in flight. Reverting the bind-first restructure (binding
        // after this work) makes that assertion fail — the falsifiable
        // proof that bind precedes the walk.
        if let Ok(raw) = std::env::var("ZFB_DEV_TEST_SLOW_DIGEST_MS") {
            if let Ok(ms) = raw.trim().parse::<u64>() {
                if ms > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                }
            }
        }

        // Steps 1-5 (digest walk, persisted-graph load, graph seed, boot
        // render, and the eager islands bundle — issue #1170) now run inside
        // a ONE-SHOT BOOT HOOK that `BuildOrchestrator::run_with_boot`
        // invokes AFTER the notify watch is registered but BEFORE the drain
        // loop consumes events (issue #1166 startup-race fixes):
        //
        //   - Finding 2 (missed-edit window): registering `watch()` first
        //     means a source edit saved during the digest walk / boot
        //     render is buffered by notify and drained by the loop, instead
        //     of being lost until the next FS event. The boot render writes
        //     only to the dev HTML root (not a watched source root), so it
        //     never triggers a spurious self-tick — no boot-vs-watcher
        //     double-render from the boot render's own writes.
        //   - Finding 1 (reload-after-boot-render): the boot render's
        //     outcome is returned from the hook so `run_with_boot`
        //     broadcasts it through the same `on_outcome`/reload path a
        //     watcher tick uses — a browser that got the dev 404 page (with
        //     its live-reload script) during the pre-render window
        //     auto-refreshes the instant the eager render lands.
        let boot = move |orchestrator: &BuildOrchestrator<DevAssetPipeline>,
                         ctx: &BuildContext|
              -> Option<BuildOutcome> {
            // Test-only window injection (issue #1166 Finding 2 regression
            // guard): `ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS` sleeps here — INSIDE
            // the boot hook, which `run_with_boot` calls AFTER the notify
            // watch is already registered but BEFORE the drain loop runs. A
            // test can save a source edit during this window and prove the
            // edit is observed (buffered by notify, drained by the loop)
            // rather than lost. Reverting the watch-first ordering (watch
            // registered only after the boot render) makes that edit fall in
            // an unobserved window and the test fail. Blocking sleep is fine:
            // the dev server's request handling runs on other runtime workers
            // (the multi-thread `#[tokio::main]`), so the port keeps serving.
            if let Ok(raw) = std::env::var("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS") {
                if let Ok(ms) = raw.trim().parse::<u64>() {
                    if ms > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(ms));
                    }
                }
            }

            // 0. Deferred dev bundle (issue #1182). When the eager dev bundle
            //    was deferred past `TcpListener::bind` (boot-lazy + servable
            //    `dist/` — `defer_dev_bundle`), build the renderer + route
            //    tables NOW, on this deferred boot task, and publish them in
            //    place via `refresh_bundle_and_routes` (it swaps the first live
            //    V8 host into the scaffold's `None` renderer slot and swaps the
            //    rebuilt tables in under the route `RwLock`). Until this lands
            //    the prebuilt `dist/` serves every route — that is what makes
            //    first-accept O(1) regardless of project size.
            //
            //    Ordered FIRST in the hook — before the graph seed (step 3) —
            //    because the seed reads `session.page_ids()`, which is empty on
            //    the scaffold until the route tables are published here. Run
            //    before the seed, the graph is populated from a live route
            //    table; run after, every watcher tick would be a cold-start
            //    no-op.
            if defer_dev_bundle {
                // Test-only slow-step injection (issue #1182 regression guard):
                // `ZFB_DEV_TEST_SLOW_BUNDLE_MS` sleeps right before the deferred
                // bundle, so an e2e can prove the port accepts connections /
                // answers HTTP while this slow step is still in flight.
                // Reverting the deferral (bundling before the bind) makes that
                // assertion fail — the falsifiable proof that bind precedes the
                // bundle. Blocking `std::thread::sleep` is fine: request
                // handling runs on other multi-thread runtime workers.
                // Independent of the digest / boot-render / islands slow-steps.
                if let Ok(raw) = std::env::var("ZFB_DEV_TEST_SLOW_BUNDLE_MS") {
                    if let Ok(ms) = raw.trim().parse::<u64>() {
                        if ms > 0 {
                            std::thread::sleep(std::time::Duration::from_millis(ms));
                        }
                    }
                }
                if let Some(ref session) = dev_session_for_boot {
                    match session.refresh_bundle_and_routes() {
                        Ok(_) => {
                            // Publish the live SSR route handle now the route
                            // tables exist: `refresh_bundle_and_routes` swaps
                            // the SESSION tables but not the server's
                            // `ssr_route_set` handle (issue #807 — its callers
                            // do). Without this, `prerender = false` routes keep
                            // resolving against the empty pre-bind set until the
                            // first watcher tick.
                            if let Some(handle) = &ssr_route_set_for_boot {
                                refresh_live_ssr_routes(session, handle);
                            }
                        }
                        Err(e) => {
                            // Same warn-and-continue contract as the eager
                            // islands boot build below: the server stays up
                            // serving the prebuilt `dist/`, the renderer slot
                            // stays `None`, and the next watcher tick's
                            // `reload_renderer` retries the bundle.
                            output::warn(format!(
                                "deferred dev bundle failed (serving prebuilt dist/ until \
                                 the next successful rebuild): {e:#}"
                            ));
                        }
                    }
                }
            }

            // 1. Manifest digest — the size-bound walk moved past bind.
            let manifest_digest =
                compute_manifest_digest(&project_root_for_boot, &digest_watch_roots);

            // 2+3. Load the persisted graph (digest-gated) and assemble the
            //      boot graph under a single lock acquisition.
            //
            //      On a cache hit, `assemble_boot_graph` MERGES the persisted
            //      graph into the live graph instead of replacing it (#1293):
            //      `DepKind::Module` edges already present in the live graph
            //      (populated by `seed_boot_module_edges` on the eager path, or
            //      by `refresh_bundle_and_routes` on the deferred path above)
            //      are preserved; non-Module edges (Content/Style/Data/Other)
            //      are taken from the persisted graph for pages not yet carrying
            //      them. Globals from the persisted graph are merged in too.
            //
            //      The seed step (formerly step 3) is folded in: pages the
            //      router scan knows but the graph does not yet have any record
            //      of are seeded with an empty dep set so `plan_for_changes`
            //      can resolve `PageSelection::All` before the first watcher
            //      tick.  Only pages with NO known record are seeded; pages
            //      that already carry edges (from the merge or from the Module
            //      seed above) are left untouched.
            {
                let persisted =
                    load_persisted_graph(&graph_cache_path_for_boot, manifest_digest.as_ref());
                let page_ids: Vec<PageId> = dev_session_for_boot
                    .as_ref()
                    .map(|s| s.page_ids())
                    .unwrap_or_default();
                if let Ok(mut g) = graph_for_seed.lock() {
                    assemble_boot_graph(&mut g, persisted, page_ids);
                }
            }

            // A persisted graph is reconciliation input only. Drop its Content
            // edges after the merge, then rebuild them from the live worker's
            // boot-time `paths()` and render observations below.
            if let Some(session) = dev_session_for_boot.as_ref() {
                session.clear_boot_content_provenance();
            }

            // Publish the digest so the shutdown path can persist the graph
            // tagged with it. Done AFTER load+seed so a shutdown that races
            // the digest landing still sees a graph the digest agrees with.
            if let Ok(mut slot) = manifest_digest_slot_for_boot.lock() {
                *slot = manifest_digest;
            }

            // 4. Boot render — eager by default (zfb#642 / #644), opt-in
            //    boot-lazy (#1057). Returns the eager outcome (or `None` for
            //    boot-lazy / no-pages / render error) so `run_with_boot` can
            //    broadcast a reload after the render lands. See
            //    `run_boot_render`.
            let render_outcome = run_boot_render(
                orchestrator,
                ctx,
                dev_session_for_boot.as_ref(),
                &dist_root_for_boot,
            );

            // 5. Eager islands bundle (issue #1170). This is the last
            //    size-bound step that used to run synchronously before
            //    `TcpListener::bind`; on a large-dependency consumer its
            //    `"use client"` scan + esbuild bundle was the dominant
            //    pre-bind cost. Running it HERE (post-bind, on the deferred
            //    boot task) keeps cold-start reachability O(1) in the
            //    consumer's dependency-tree size.
            //
            //    Test-only slow-step injection (issue #1170 regression
            //    guard): `ZFB_DEV_TEST_SLOW_ISLANDS_MS` sleeps right before
            //    the islands build, so a test can prove the port accepts
            //    connections / answers HTTP while this slow step is still in
            //    flight. Reverting the deferral (building islands before the
            //    bind) makes that assertion fail. Blocking `std::thread::sleep`
            //    is fine for the same reason the boot-render slow-step above
            //    is: request handling runs on other multi-thread runtime
            //    workers. Independent of the digest / boot-render slow-steps.
            if let Ok(raw) = std::env::var("ZFB_DEV_TEST_SLOW_ISLANDS_MS") {
                if let Ok(ms) = raw.trim().parse::<u64>() {
                    if ms > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(ms));
                    }
                }
            }
            let islands_info = match rebundle_islands(
                &project_root_for_boot,
                &dev_assets_root_for_boot,
                framework_for_boot,
                bundle_config_for_boot.as_ref(),
                &islands_plugin_config_for_boot,
                &islands_url_prefix_for_boot,
                &islands_url_handle_for_boot,
                &islands_companion_names_for_boot,
                &raw_import_invalidation_for_boot,
            ) {
                Ok(info) => info,
                Err(e) => {
                    // Same contract as the pre-#1170 eager build's Err /
                    // write-fail arms: warn and continue. The server stays
                    // up, the bundle-URL handle stays `None` (no
                    // `<script type="module">` injected), and the next
                    // successful watcher rebuild retries.
                    output::warn(format!(
                        "initial islands bundle failed (no <script \
                         type=\"module\"> will be injected until the next \
                         successful rebuild): {e:#}"
                    ));
                    None
                }
            };

            // 6. Fold the islands bundle into the boot outcome so
            //    `run_with_boot` broadcasts a `ReloadEvent::Islands` (via
            //    `outcome_to_events`) — a browser that loaded a page during
            //    the pre-bundle window then re-imports the bundle and
            //    hydrates. CRITICAL: `run_boot_render` returns `None` on
            //    boot-lazy / no-pages / render-error, but the islands reload
            //    must STILL fire on those paths — otherwise a pre-bundle tab
            //    never hydrates. So when the render produced no outcome we
            //    synthesise a `BuildOutcome` carrying only the islands info.
            //    A single merged outcome is returned (one broadcast), never
            //    two.
            // Issues #1182 / #1390 — drain the routes the boot task marked
            // stale and fold them into `pages_stale` so the single
            // `run_with_boot` broadcast emits one `ReloadEvent::Page`
            // (livereload.rs): a tab that loaded the prebuilt `dist/` seed
            // during the pre-render window reloads, and its GET re-renders
            // through the now-live request-time hook. Drained from the same
            // tick buffer the pipeline's stale probe uses; the per-route stale
            // map (claimable for request-time render) is untouched — this is
            // the broadcast, not a second one.
            //
            // UNCONDITIONAL, not gated on `defer_dev_bundle` (was #1182, fixed
            // in #1390). `take_tick_stale()` is the self-describing signal —
            // it returns exactly the routes the boot task left marked stale —
            // so gating on `defer_dev_bundle` was both unnecessary and a bug:
            //   - Eager (non-boot-lazy) boot: `run_boot_render` renders every
            //     route through `initial_build`, whose pipeline stale probe
            //     ALREADY drained `tick_stale` into `outcome.pages_stale`. So
            //     this drain returns empty and the `if !boot_stale.is_empty()`
            //     fold below is inert — no behaviour change, no double-count.
            //   - Deferred boot-lazy: step 0 published the renderer and
            //     `run_boot_render`'s boot-lazy branch staled every route →
            //     this drains + broadcasts them (the original #1182 case).
            //   - Non-deferred boot-lazy (a servable seed present but the #1188
            //     `ZFB_DEV_DEFER_BUNDLE=0` opt-out): `run_boot_render` STILL
            //     takes its boot-lazy branch and calls `mark_all_routes_stale`
            //     AFTER bind — but the old `defer_dev_bundle` gate suppressed
            //     the broadcast, so a tab that loaded the prebuilt `dist/` seed
            //     during the [bind → mark_all_routes_stale] window never got
            //     the reload and stayed on stale seed bytes. Ungating delivers
            //     it (the always-on islands reload above was never enough — it
            //     re-imports the bundle but does not re-fetch the page HTML).
            let boot_stale: Vec<PathBuf> = dev_session_for_boot
                .as_ref()
                .map(|s| s.inner.take_tick_stale())
                .unwrap_or_default();

            let mut outcome = match (render_outcome, islands_info) {
                (Some(mut outcome), Some(info)) => {
                    outcome.islands_rerun = true;
                    outcome.islands_changed = info.changed;
                    outcome.islands_bundle = Some(info);
                    Some(outcome)
                }
                (Some(outcome), None) => Some(outcome),
                (None, Some(info)) => Some(BuildOutcome {
                    islands_rerun: true,
                    islands_changed: info.changed,
                    islands_bundle: Some(info),
                    ..BuildOutcome::default()
                }),
                (None, None) => None,
            };
            if !boot_stale.is_empty() {
                match &mut outcome {
                    Some(o) => o.pages_stale = boot_stale,
                    None => {
                        outcome = Some(BuildOutcome {
                            pages_stale: boot_stale,
                            ..BuildOutcome::default()
                        })
                    }
                }
            }
            outcome
        };

        // Orchestrator watcher loop — registers the watch, runs the boot
        // hook above (steps 1-5), then drains change events until aborted on
        // shutdown.
        if let Err(err) = orchestrator
            .run_with_boot(ctx, discover_hook, on_outcome, Some(boot))
            .await
        {
            output::error(format!("build orchestrator stopped: {err:#}"));
        }
    });

    // Announce the ACTUAL bound port, not the requested one: with
    // `--port 0` the OS picks an ephemeral port, and printing the literal
    // `0` makes the banner unparseable for callers that need to discover
    // the port (e.g. the dev E2E harness, #1018). For a fixed port the
    // two values are identical, so existing UX is unchanged.
    let bound_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    output::ready_with_interfaces("http", &host, bound_port);

    // Run the server until Ctrl+C. Pass Ctrl+C as the graceful-shutdown
    // signal so axum drains in-flight connections before exiting. The
    // renderer guard tears down on drop here — the explicit `shutdown`
    // call belt-and-braces keeps the surface symmetrical (start ↔ shutdown).
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let result = tokio::select! {
        res = serve_with_listener(opts, listener, ctrl_c) => {
            boot_handle.abort();
            res
        }
    };

    if let Some(session) = dev_session {
        session.shutdown_explicit();
    }

    // Tear down the plugin host before exit so the Node
    // subprocess doesn't outlive `zfb dev`. Best-effort: a kill via
    // `kill_on_drop` covers the panic / Ctrl+C path; the explicit
    // shutdown is the graceful one.
    if let Some(h) = plugin_host {
        let _ = h.shutdown().await;
    }

    // 8. Persist the graph one more time before exit so the latest
    // populated state — not just the boot-time fresh one — is what
    // the next cold start sees. Best-effort; warn-and-ignore on
    // failure (don't block shutdown on a disk error).
    //
    // Issue #1166 — the digest is now produced inside the deferred boot
    // task and published through `manifest_digest_slot`. If Ctrl+C
    // arrived BEFORE the digest landed the slot is still `None` and we
    // SKIP the save entirely: writing a graph tagged with an absent /
    // wrong digest would let the next cold start reuse a stale or empty
    // cache as authoritative. The seed runs before the digest is
    // published, so a present digest implies the graph it tags is at
    // least seeded.
    let final_digest = manifest_digest_slot.lock().ok().and_then(|s| s.clone());
    if let Some(d) = final_digest.as_ref() {
        if let Ok(g) = graph_for_save.lock() {
            if let Err(err) = save_to_disk(&g, d, &graph_cache_path) {
                output::warn(format!(
                    "graph persistence: shutdown write to {} failed (ignored): {err:#}",
                    graph_cache_path.display()
                ));
            }
        }
    }

    result
}

/// Re-bundle the project's `"use client"` islands once and publish the
/// result: build the payload, write the stable `islands.js` under
/// `assets_root/assets/` (issue #1189: the isolated `.zfb-build/dev-assets`
/// root, NOT the build-shared `dist/`),
/// refresh / prune chunk and module-worker companions, and rewrite the shared
/// bundle-URL handle. Returns `Some(IslandsBundleInfo { changed: true, .. })` when a
/// bundle was produced (so `outcome_to_events` emits a
/// `ReloadEvent::Islands`), or `None` when the project has no `"use client"`
/// components this run — in which case the handle is cleared and stale
/// companions are pruned so no `<script type="module">` and no dead chunk or
/// worker files keep being served.
///
/// Shared by the watcher-tick `run_islands` callback and the deferred boot
/// build (issue #1170) so both write disk, prune companions, and publish the URL
/// IDENTICALLY — the two paths cannot drift. Lock poisoning is recovered (a
/// writer panic must not strand the watcher loop). Disk / companion-write
/// failures are returned as `Err`: the watcher path propagates it (a tick
/// failure is loud), the boot path warns-and-continues (issue #1170).
#[allow(clippy::too_many_arguments)] // 9 params: #1497 added bundle_config + raw_invalidation; mirrors build_default_islands_payload_with_bundle_options' threaded inputs, a struct would just shuffle the same fields
fn rebundle_islands(
    project_root: &Path,
    // Where dev assets are written + served from (issue #1189: the isolated
    // `.zfb-build/dev-assets` root, NOT the build-shared `dist/`).
    assets_root: &Path,
    framework: crate::config::Framework,
    bundle_config: Option<&crate::config::BundleConfig>,
    plugin_config: &crate::commands::build::IslandsPluginConfig,
    url_prefix: &str,
    url_handle: &zfb_server::IslandsBundleUrl,
    companion_names: &Arc<Mutex<HashSet<String>>>,
    raw_invalidation: &zfb_build::RawImportInvalidation,
) -> anyhow::Result<Option<IslandsBundleInfo>> {
    // Marker names are only needed by the production build pass; dev mode
    // already surfaces unknown-marker warnings in the browser console via
    // the runtime.ts warn path.
    // Dev seeds the islands scanner from the conventional `pages/` root.
    // (Package-owned build routes are a build-time concern; dev's
    // injected routes are served live, not materialised — #1193.) No
    // package-route entrypoints to seed in dev (codex P1 is build-only).
    // Issue #1404 — the islands-shadow `import.meta.glob` fix is applied
    // inside `build_default_islands_payload`, so the dev path gets it for
    // free by routing through the same function: a supported eager
    // string-literal glob reachable from an island is expanded into a
    // per-rebundle shadow and bundles normally. Only the UNSUPPORTED-form
    // remainder falls back to the #1387 stopgap, and here that stopgap is
    // `WarnAndSkip`: it warns and skips this rebundle tick instead of
    // failing the whole watcher loop (unlike `zfb build`'s `HardError`), so
    // the dev server stays up while the author fixes the file and saves
    // again. The shadow TempDir is created and dropped entirely within the
    // call below (esbuild runs synchronously inside it), so no shadow state
    // leaks across dev ticks.
    let (payload, _marker_names) =
        crate::commands::build::build_default_islands_payload_with_bundle_options(
            project_root,
            &project_root.join("pages"),
            &[],
            assets_root,
            framework,
            bundle_config,
            zfb_islands::BundleMode::Development,
            plugin_config,
            crate::commands::build::IslandsGlobPolicy::WarnAndSkip,
            Some(raw_invalidation),
        )?;
    // Rewrite the shared handle so the next initial GET (a fresh browser
    // tab, or a page that has not yet hydrated) sees the current bundle URL.
    // The dev server holds the same Arc, so this is visible without
    // re-routing through ServeOpts.
    //
    // Treat lock poisoning as a soft event: a writer panic should not abort
    // the watcher loop. Recover the inner and continue.
    let mut guard = url_handle.write().unwrap_or_else(|p| {
        tracing::warn!(
            site = "dev.rebundle_islands.url_handle",
            "rwlock poisoned, recovered"
        );
        p.into_inner()
    });
    let Some(payload) = payload else {
        // The project produced no islands bundle this run. Clear the shared
        // URL so the next served HTML response does NOT keep injecting a
        // stale `<script type="module">` tag — without this, removing the
        // last `"use client"` component would leave the previously-emitted
        // bundle URL visible on every page until the dev server restarts.
        *guard = None;
        // Also prune companions from the last bundle — with no islands bundle
        // at all, neither chunks nor module workers should remain served.
        {
            let mut prev = companion_names.lock().unwrap_or_else(|p| {
                tracing::warn!(
                    site = "dev.rebundle_islands.companion_names (clear)",
                    "mutex poisoned, recovered"
                );
                p.into_inner()
            });
            let assets_dir = assets_root.join(zfb_types::DIST_ASSETS_DIR);
            if let Err(e) = refresh_dev_island_chunks(&assets_dir, &[], &prev) {
                tracing::warn!(
                    error = %e,
                    "dev islands: failed to prune stale companions after no-bundle tick (ignored)"
                );
            }
            *prev = HashSet::new();
        }
        return Ok(None);
    };
    // Write the stable `islands.js` bytes to disk so ServeDir can serve
    // `GET /assets/islands.js`. The bundler carries bytes in memory only —
    // the dev caller owns the disk write (same pattern as the CSS path).
    {
        let assets_dir = assets_root.join(zfb_types::DIST_ASSETS_DIR);
        let islands_out_path = assets_dir.join(zfb_types::STABLE_ISLANDS_FILENAME);
        if let Some(parent) = islands_out_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&islands_out_path, &payload.bytes) {
            return Err(anyhow::anyhow!(
                "dev islands: failed to write islands.js to disk: {e:#}"
            ));
        }
    }
    let bundle_url = if url_prefix.is_empty() {
        payload.stable_url
    } else {
        format!("{url_prefix}{}", payload.stable_url)
    };
    // Write / prune chunk and worker companions for this generation.
    {
        let mut prev = companion_names.lock().unwrap_or_else(|p| {
            tracing::warn!(
                site = "dev.rebundle_islands.companion_names",
                "mutex poisoned, recovered"
            );
            p.into_inner()
        });
        let assets_dir = assets_root.join(zfb_types::DIST_ASSETS_DIR);
        match refresh_dev_island_chunks(&assets_dir, &payload.companions, &prev) {
            Ok(names) => *prev = names,
            Err(e) => {
                return Err(e.context("dev islands: failed to refresh companion files"));
            }
        }
    }
    // The bundler does not currently surface a "bytes-changed" bit back
    // through `build_default_islands_payload` — the URL stays stable
    // (`/assets/islands.js`) on every rebuild, the bytes on disk update in
    // place. Report `changed = true` so the SSE layer always emits a reload
    // event after a successful re-bundle; the browser then re-imports the
    // URL with a cache-busting `?v=…` query that picks up the new bytes.
    // `components` is empty because the build-side payload doesn't carry
    // per-island names; the livereload client's empty-component path handles
    // "unknown components" by reloading the whole bundle.
    let info = IslandsBundleInfo {
        changed: true,
        bundle_url: bundle_url.clone(),
        components: Vec::new(),
    };
    *guard = Some(bundle_url);
    Ok(Some(info))
}

/// Boot render — eager by default (zfb#642 / #644), opt-in boot-lazy
/// (#1057). Extracted from `run` so it can run inside the deferred boot
/// task (issue #1166) while staying readable.
///
/// Opt-in boot-lazy mode (issue #1057): with `ZFB_DEV_BOOT_LAZY=1` AND a
/// valid prebuilt `dist/` present, SKIP the eager boot render entirely —
/// mark every route stale and let the dev server serve the prebuilt
/// `dist/` immediately (via the `read_from_dist` cold fallback, which
/// points at `dist_root`), while the request-time render-on-request hook
/// (#1026) re-renders each route on its first GET. Requires lazy
/// rendering (the hook only exists then), enforced by `boot_lazy_enabled`;
/// without a servable `dist/` we fall through to the eager render so the
/// server never serves a wrong/empty body.
///
/// Eager mode request-before-render race (issue #1166): because this now
/// runs on a background task AFTER the listener binds, a GET can arrive
/// before the eager render writes a route's HTML. The dev server's serve
/// waterfall (`PageCache → html_root → public_root → dist_root → 404`)
/// handles it: the request is served from the prebuilt `dist/` seed (the
/// last leg before the 404) if a servable copy exists, otherwise the
/// controlled `DEV_404_BODY` (a
/// complete HTML page carrying the live-reload script, which auto-upgrades
/// the instant the real render lands) — never a wrong/empty/partial body.
#[cfg(feature = "embed_v8")]
fn run_boot_render(
    orchestrator: &BuildOrchestrator<DevAssetPipeline>,
    ctx: &BuildContext,
    dev_session: Option<&DevRenderSession>,
    dist_root: &Path,
) -> Option<BuildOutcome> {
    let boot_lazy = dev_session
        .map(|s| boot_lazy_enabled(s.lazy_render_enabled()))
        .unwrap_or(false)
        && dist_is_servable_seed(dist_root);

    if boot_lazy {
        if let Some(session) = dev_session {
            // Consume the one-shot boot-render-pending flag (the eager boot
            // render that would have consumed it is being skipped) so the
            // FIRST watcher edit takes the normal lazy path, not the
            // boot-eager one.
            let _ = session.inner.take_boot_render_pending();
            let n = session.mark_all_routes_stale();
            output::info(format!(
                "dev: boot-lazy — serving prebuilt dist/ for {n} route(s); each \
                 re-renders on first request (ZFB_DEV_BOOT_LAZY=1)"
            ));
        }
        // Boot-lazy renders nothing eagerly (each route re-renders on its
        // first request), so there is no eager outcome to broadcast. The
        // request-time render path drives its own reload.
        return None;
    }

    // Eager initial render (zfb#642 / #644).
    //
    // `BuildOrchestrator::run` is purely watcher-driven — it renders a
    // page only after a file-change event. Without this eager render a
    // fresh `zfb dev` leaves `.zfb-build/dev-pages/` empty and 404s EVERY
    // route until the user edits a file. Going through the
    // orchestrator/pipeline (not the raw render callback) also primes
    // `DevAssetPipeline.last_bytes` so the first real edit dedups
    // correctly. Runs on the deferred boot task now (#1166) — see the
    // request-before-render race note in the doc comment above.
    match orchestrator.initial_build(ctx) {
        Ok(Some(outcome)) => {
            let expected_routes = dev_session.map(|s| s.route_count()).unwrap_or(0);
            // Surface the previously-silent zero-page failure (zfb#642):
            // the renderer knows about routes, yet produced no HTML. Every
            // route would 404. Make it visible on stderr instead.
            if expected_routes > 0 && outcome.pages_rendered == 0 {
                output::error(format!(
                    "dev initial render produced 0 pages for {expected_routes} known route(s) — \
                     every route will 404. This usually means the renderer failed silently; \
                    check the bundler / runtime output above."
                ));
            }
            if let Some(session) = dev_session {
                if let Err(error) = session.complete_boot_content_provenance() {
                    output::warn(format!(
                        "content provenance unavailable after boot render; \
                         content edits will conservatively rebuild all pages: {error:#}"
                    ));
                }
            }
            // Return the eager outcome so the caller broadcasts a reload
            // through the same `outcome_to_events` path a watcher tick uses
            // (issue #1166 Finding 1): a browser that requested a route
            // during the pre-render window received the dev 404 page (which
            // carries the live-reload script); broadcasting here auto-
            // refreshes that tab the instant the real HTML lands on disk.
            Some(outcome)
        }
        Ok(None) => {
            // No pages in the graph at all (renderer disabled or a
            // project with zero SSG routes). The dev server still boots so
            // the user can poke at it / fix the project; SSR-only routes
            // still work via the request-time path. Nothing rendered → no
            // reload to broadcast.
            None
        }
        Err(err) => {
            output::error(format!(
                "dev initial render failed — every route will 404 until the next \
                 successful rebuild: {err:#}"
            ));
            None
        }
    }
}

/// V8-off stub for `zfb dev` (issue #371, sub-task 4.1a).
///
/// `zfb dev` needs the embedded V8 host to render pages. When the
/// `embed_v8` cargo feature is off this binary was built without that
/// host, so we surface a clear error at the call site instead of
/// compiling a partial pipeline that would silently hand back empty
/// pages.
#[cfg(not(feature = "embed_v8"))]
pub async fn run(_args: &DevArgs) -> Result<()> {
    anyhow::bail!(
        "zfb was built without V8 support (`--no-default-features` / \
         `embed_v8 = off`); `zfb dev` requires the embedded V8 host to \
         render pages. Rebuild with default features (`cargo build`) \
         or with `--features embed_v8` to enable this command."
    )
}

/// Compute the manifest digest for the current project, or return
/// `None` if the digest itself could not be computed (e.g. permission
/// denied while walking sources). On `None` the caller should bypass
/// the persistence layer entirely — never falsely reuse a stale
/// graph.
fn compute_manifest_digest(project_root: &Path, watch_roots: &[PathBuf]) -> Option<ManifestDigest> {
    // Config files that, when changed, must invalidate the graph
    // even though they live next to (not under) the watched roots.
    // Both JSON and TS are listed; missing ones are silently
    // skipped by the digest builder.
    let cfg_files = [
        PathBuf::from("zfb.config.json"),
        PathBuf::from("zfb.config.ts"),
    ];
    match ManifestDigest::compute(project_root, watch_roots, &cfg_files) {
        Ok(d) => Some(d),
        Err(err) => {
            output::warn(format!(
                "graph persistence: manifest digest failed (cache disabled): {err:#}"
            ));
            None
        }
    }
}

/// Try to reuse a persisted graph. Returns `Some(graph)` only when
/// the on-disk file exists and its digest matches the live one. All
/// other outcomes (no digest, missing file, mismatch, IO error) map
/// to `None` so the caller falls back to a fresh graph.
fn load_persisted_graph(
    graph_cache_path: &Path,
    digest: Option<&ManifestDigest>,
) -> Option<DependencyGraph> {
    let d = digest?;
    match load_from_disk(graph_cache_path, d) {
        Ok(Some(g)) => Some(g),
        Ok(None) => None,
        Err(err) => {
            output::warn(format!(
                "graph persistence: load from {} failed (ignored): {err:#}",
                graph_cache_path.display()
            ));
            None
        }
    }
}

/// Assemble the boot graph in-place from an optional persisted snapshot and
/// a list of page ids from the router scan (#1293).
///
/// This is a pure function (no I/O, no locks) so it can be unit-tested
/// directly.  The caller is responsible for acquiring the graph mutex before
/// calling this.
///
/// Behaviour:
///
/// 1. **Merge** — when `persisted` is `Some`, call
///    [`DependencyGraph::merge_from_persisted`] which blends the persisted
///    graph's non-Module edges and globals into `live` while preserving any
///    `DepKind::Module` edges already in `live` (populated earlier by
///    `seed_boot_module_edges` / `refresh_bundle_and_routes`).
/// 2. **Seed** — for every `PageId` in `page_ids`, if the graph has NO
///    existing record for that page (neither from the merge above nor from an
///    earlier `seed_boot_module_edges` call), upsert an empty dep set so
///    `plan_for_changes` can resolve `PageSelection::All` before the first
///    watcher tick.  Pages already tracked (with edges) are left untouched.
pub(crate) fn assemble_boot_graph(
    live: &mut DependencyGraph,
    persisted: Option<DependencyGraph>,
    page_ids: impl IntoIterator<Item = PageId>,
) {
    if let Some(p) = persisted {
        live.merge_from_persisted(p);
    }
    for page_id in page_ids {
        if !live.knows(page_id.path()) {
            live.upsert(PageDeps::new(page_id, vec![]));
        }
    }
}

/// Canonicalise each entry in `cfg.extra_watch_paths`, dropping any
/// entry that does not exist at boot with a user-visible warning.
///
/// The config loader already enforces "absolute path"; this function
/// implements the rest of the documented contract:
///
/// - canonicalise (`Path::canonicalize`) — the watcher's events will
///   match the canonical form, so downstream rebuild logic compares
///   like for like.
/// - skip-with-warning when the path does not exist. We do NOT
///   re-watch later if it appears — the documented escape hatch is
///   "restart `zfb dev`".
///
/// Canonicalisation also implicitly resolves symlinks; that is the
/// behaviour we want for events-match-on-canonical-path. The dev
/// command boots once per session, so this runs exactly once per
/// configured entry.
fn resolve_extra_watch_paths(raw: &[PathBuf]) -> Vec<PathBuf> {
    let mut resolved = Vec::with_capacity(raw.len());
    for p in raw {
        match p.canonicalize() {
            Ok(c) => resolved.push(c),
            Err(err) => {
                output::warn(format!(
                    "extraWatchPaths: skipping {} (canonicalize failed: {}); \
                     the path will NOT be re-watched if it appears later — \
                     restart `zfb dev` after creating it",
                    p.display(),
                    err,
                ));
            }
        }
    }
    resolved
}

/// Resolve the project's CSS `@import` graph to canonicalised real paths the
/// dev watcher should follow (D4 of #1288).
///
/// Anchors on the conventional authored global stylesheet
/// ([`crate::commands::build::resolve_input_global_css`] — `styles/global.css`
/// or `src/styles/global.css`); returns an empty set when the project has no
/// authored global CSS. Delegates the recursive `@import` resolution +
/// canonicalisation (following workspace symlinks) to
/// [`zfb_css::resolve_css_imports`].
fn resolve_css_import_watch_targets(project_root: &Path) -> Vec<PathBuf> {
    let Some(entry) = crate::commands::build::resolve_input_global_css(project_root) else {
        return Vec::new();
    };
    zfb_css::resolve_css_imports(&entry, project_root)
}

// ---------------------------------------------------------------------------
// Dev islands companion helpers
// ---------------------------------------------------------------------------

/// Write new companion files into `assets_dir`, delete companions from the
/// previous generation that are no longer in the new set, and return the new
/// filename set.
///
/// `assets_dir` is the on-disk `<dist_root>/assets/` directory that the dev
/// server already serves via ServeDir. Because companions land in that directory
/// under flat contract basenames (self-hashed `islands-chunk-*.js` chunks and
/// stable `worker-*.js` module workers), the entry's baked-in relative URLs
/// resolve without any additional routing code.
///
/// Errors writing a companion are returned immediately (callers treat them as
/// non-fatal at the boot path, fatal at the watcher tick path).  Errors
/// deleting stale companions are logged and ignored — a stale file that fails to
/// delete is preferable to aborting the rebuild loop.
fn refresh_dev_island_chunks(
    assets_dir: &Path,
    companions: &[zfb_build::pipeline::CompanionFile],
    prev_filenames: &HashSet<String>,
) -> anyhow::Result<HashSet<String>> {
    let new_filenames: HashSet<String> = companions.iter().map(|c| c.filename.clone()).collect();

    // Write each new companion file beside the entry.
    for companion in companions {
        if companion.filename.is_empty()
            || companion.filename.contains('/')
            || companion.filename.contains('\\')
            || companion.filename.contains("..")
        {
            anyhow::bail!(
                "dev islands: companion filename {:?} must be a flat basename \
                 (no path separator or `..`)",
                companion.filename
            );
        }
        let dest = assets_dir.join(&companion.filename);
        std::fs::write(&dest, &companion.bytes).with_context(|| {
            format!(
                "dev islands: failed to write companion file {}",
                dest.display()
            )
        })?;
    }

    // Prune stale companion files from the previous generation.
    for stale in prev_filenames.difference(&new_filenames) {
        let path = assets_dir.join(stale);
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "dev islands: failed to delete stale companion (ignored)"
                );
            }
        }
    }

    Ok(new_filenames)
}

// ---------------------------------------------------------------------------
// Renderer plumbing
// ---------------------------------------------------------------------------

/// Long-lived dev-session state that owns the renderer subprocess and
/// the route table. Cloned by the [`PageRenderer`] callback so each
/// orchestrator tick can map page ids → URLs.
///
/// `pub(crate)` (issue #1026): the lazy render adapter
/// ([`crate::lazy_render_adapter`]) holds a clone as its persistent
/// handle into the session — the inner `Arc<DevRenderInner>` survives
/// every bundle refresh (the renderer state is swapped in place), so
/// the adapter never needs rewiring.
#[derive(Clone)]
pub(crate) struct DevRenderSession {
    inner: Arc<DevRenderInner>,
}

/// Compile-time default for the lazy dev-render switch (issue #1025).
///
/// `true` since the wave-5 activation flip (issue #1027): watcher ticks
/// mark affected routes stale and re-render them on first request
/// instead of fan-out rendering eagerly. `ZFB_DEV_EAGER=1` restores the
/// fully-eager behaviour; `ZFB_LAZY_DEV_RENDER=0|1` remains the precise
/// per-session override (see [`resolve_lazy_dev_render`]).
const LAZY_DEV_RENDER_DEFAULT: bool = true;

/// Resolve the lazy dev-render switch once at boot (issues #1025/#1027).
///
/// Read a single time in [`boot_dev_renderer`] and stored on
/// [`DevRenderInner::lazy_render`] — the tick path never re-reads the
/// environment. Precedence is implemented by [`resolve_lazy_dev_render`].
fn lazy_dev_render_enabled() -> bool {
    resolve_lazy_dev_render(
        std::env::var("ZFB_LAZY_DEV_RENDER").ok().as_deref(),
        std::env::var("ZFB_DEV_EAGER").ok().as_deref(),
    )
}

/// Pure precedence rule for the lazy dev-render switch (issue #1027):
///
/// 1. `ZFB_LAZY_DEV_RENDER=1|true` forces ON, `0|false` forces OFF —
///    the precise override wins over everything when set to a
///    recognized value.
/// 2. Otherwise `ZFB_DEV_EAGER=1|true` forces OFF — the documented
///    user-facing escape hatch, equivalent to `ZFB_LAZY_DEV_RENDER=0`.
/// 3. Otherwise (both unset / unrecognized values) fall back to
///    [`LAZY_DEV_RENDER_DEFAULT`].
fn resolve_lazy_dev_render(lazy_var: Option<&str>, eager_var: Option<&str>) -> bool {
    if let Some(raw) = lazy_var {
        let t = raw.trim();
        if t.eq_ignore_ascii_case("1") || t.eq_ignore_ascii_case("true") {
            return true;
        }
        if t.eq_ignore_ascii_case("0") || t.eq_ignore_ascii_case("false") {
            return false;
        }
    }
    if let Some(raw) = eager_var {
        let t = raw.trim();
        if t.eq_ignore_ascii_case("1") || t.eq_ignore_ascii_case("true") {
            return false;
        }
    }
    LAZY_DEV_RENDER_DEFAULT
}

/// Compile-time default for the opt-in boot-lazy switch (issue #1057). OFF —
/// the default `zfb dev` boot semantics ("every route exists on disk before
/// the server is ready") are preserved exactly.
const BOOT_LAZY_DEFAULT: bool = false;

/// Resolve the opt-in boot-lazy switch once at boot (issue #1057).
///
/// `lazy_render_on` is the resolved lazy dev-render switch. Boot-lazy REUSES
/// the request-time render-on-request hook (#1026), which is only installed
/// when lazy rendering is on — so boot-lazy is force-disabled when lazy
/// rendering is off (otherwise the prebuilt `dist/` would be served forever
/// with no re-render). When enabled, the caller additionally requires a
/// valid prebuilt `dist/` to seed from (see [`dist_is_servable_seed`]);
/// absent that, it falls back to the eager boot render.
fn boot_lazy_enabled(lazy_render_on: bool) -> bool {
    boot_lazy_decision(
        lazy_render_on,
        std::env::var("ZFB_DEV_BOOT_LAZY").ok().as_deref(),
    )
}

/// Pure boot-lazy decision (issue #1057): on only when lazy rendering is on
/// AND `ZFB_DEV_BOOT_LAZY` is truthy. Split from [`boot_lazy_enabled`] so the
/// "requires lazy" rule is unit-testable without process-global env mutation.
fn boot_lazy_decision(lazy_render_on: bool, boot_lazy_var: Option<&str>) -> bool {
    lazy_render_on && resolve_boot_lazy(boot_lazy_var)
}

/// Pure decision for the deferred dev bundle (issue #1182): defer the eager
/// `assemble_and_bundle_dev` (+ V8 host start + `paths()`-expanding route-table
/// build) past `TcpListener::bind` only when
///
/// 1. boot-lazy is active ([`boot_lazy_decision`]) — so the request-time
///    render-on-request hook (#1026) is installed and the prebuilt `dist/` is
///    the serving source for every route until the renderer is published, AND
/// 2. a servable `dist/` seed is present ([`dist_is_servable_seed`]), AND
/// 3. the deferral is not opted out ([`resolve_defer_bundle`], issue #1188).
///
/// The defer gate is a **strict subset** of the boot-lazy gate: deferring always
/// implies boot-lazy (and a servable seed), so a *deferred* boot always takes
/// [`run_boot_render`]'s boot-lazy branch there (mark-stale, no eager render).
/// The opt-out (conjunct 3) and the servable-seed requirement (conjunct 2) only
/// make *some* boot-lazy boots fall back to building the renderer eagerly before
/// bind — exactly the pre-#1182 boot-lazy path. So the gate no longer "matches"
/// the boot-lazy gate one-for-one, but the load-bearing direction (defer ⟹
/// boot-lazy branch) still holds. When the gate is off, `boot_dev_renderer`
/// builds the renderer eagerly before bind — the deferral is strictly additive.
///
/// SSR-window trade-off (issue #1182, accepted; opt-out added in #1188): the gate
/// only proves SOME servable `index.html` exists, not that every route is covered
/// during the deferred-bundle window. SSG routes with a prebuilt
/// `dist/<route>/index.html` serve from the `read_from_dist` leg
/// (`ServeOpts.dist_root`) the whole window; SSR-only (`prerender = false`) routes
/// have no static artifact, so they serve the controlled dev 404 (+ livereload)
/// until the renderer publishes — then the post-publish `pages_stale` broadcast
/// reloads those tabs and they resolve. This is the same request-before-render
/// contract #1166 already ships, just over the bundle window; it does extend the
/// SSR-unavailable window (which was ~0 in non-deferred boot-lazy, where the
/// renderer was live before bind) to the bundle duration. Acceptable for the
/// large-SSG projects this targets. An SSR-heavy project that can't tolerate that
/// window sets `ZFB_DEV_DEFER_BUNDLE=0` (issue #1188) to opt out: the renderer /
/// route tables are built before bind, removing the SSR-only 404 window, at the
/// cost of a slower first-accept. (Opting out does NOT disable boot-lazy itself —
/// the graph/render/islands deferral contracts remain.) A finer per-route
/// dist-coverage gate would need the route tables, which need the very bundle
/// being deferred, so it stays out of scope.
///
/// Split out so the gate is unit-testable without process-global env mutation
/// or a real `dist/` tree.
fn defer_dev_bundle_decision(
    lazy_render_on: bool,
    boot_lazy_var: Option<&str>,
    dist_servable: bool,
    defer_var: Option<&str>,
) -> bool {
    boot_lazy_decision(lazy_render_on, boot_lazy_var)
        && dist_servable
        && resolve_defer_bundle(defer_var)
}

/// Pure precedence rule for the boot-lazy switch (issue #1057):
/// `ZFB_DEV_BOOT_LAZY=1|true` enables it; everything else (unset / `0` /
/// unrecognized) falls back to [`BOOT_LAZY_DEFAULT`] (off).
fn resolve_boot_lazy(var: Option<&str>) -> bool {
    match var {
        Some(raw) => {
            let t = raw.trim();
            t.eq_ignore_ascii_case("1") || t.eq_ignore_ascii_case("true")
        }
        None => BOOT_LAZY_DEFAULT,
    }
}

/// Pure precedence rule for the #1182 dev-bundle deferral opt-out (issue #1188):
/// the deferral is ON by default; `ZFB_DEV_DEFER_BUNDLE=0|false` opts OUT so the
/// renderer is built eagerly before bind (no SSR-only 404 window) at the cost of
/// a slower first-accept. Unset / `1` / `true` / unrecognized keep the default-on
/// deferral. Inverted default vs [`resolve_boot_lazy`]: only an explicit falsey
/// value suppresses the deferral, so a malformed value never silently disables it.
fn resolve_defer_bundle(var: Option<&str>) -> bool {
    match var {
        Some(raw) => {
            let t = raw.trim();
            !(t == "0" || t.eq_ignore_ascii_case("false"))
        }
        None => true,
    }
}

/// Freshness gate for boot-lazy (issue #1057): is `dist_root` a prebuilt
/// site we can safely serve immediately as the cold seed?
///
/// Minimal, conservative check: the directory exists and contains at least
/// one `index.html` (the shape every built route writes). When this is
/// false — no prior `pnpm build`, or an empty/partial `dist/` — boot-lazy is
/// declined and the eager boot render runs instead, so the server never
/// comes up serving 404s. Content staleness is NOT gated here: boot-lazy
/// marks every route stale, so the first request to each route re-renders it
/// fresh through the live host; the prebuilt bytes are only the
/// before-first-request seed.
fn dist_is_servable_seed(dist_root: &Path) -> bool {
    fn has_index_html(dir: &Path, depth: usize) -> bool {
        // Bounded walk: a built site writes `index.html` at the root and/or
        // one per route dir. Cap recursion so a pathological tree can't stall
        // boot.
        if depth > 8 {
            return false;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return false;
        };
        let mut subdirs: Vec<PathBuf> = Vec::new();
        for entry in rd.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(ft)
                    if ft.is_file()
                        && path.file_name().and_then(|n| n.to_str()) == Some("index.html") =>
                {
                    return true;
                }
                Ok(ft) if ft.is_dir() => subdirs.push(path),
                _ => {}
            }
        }
        subdirs.iter().any(|d| has_index_html(d, depth + 1))
    }
    dist_root.is_dir() && has_index_html(dist_root, 0)
}

/// Per-route staleness state for lazy dev rendering (issue #1025).
///
/// Keyed by **relative output path** (under the dev HTML root) — the
/// same key the pipeline's write bookkeeping and the synthetic
/// output-path `PageId`s use, and the value `lookup_by_url` resolves a
/// request to. One mutex guards all three fields so a mark / claim /
/// swap observes a consistent snapshot.
#[derive(Debug, Default)]
struct StaleRoutes {
    /// Monotonic tick generation. Incremented at the P4 route-table
    /// swap in [`DevRenderSession::refresh_bundle_and_routes`] — i.e.
    /// exactly when the route universe moves. Phase-B-skipped ticks
    /// (#956) never reach P4 and leave it untouched.
    generation: u64,
    /// Stale output path → the generation that (most recently) staled
    /// it. An entry means "the on-disk/cached HTML for this route may
    /// not reflect the current source tree; re-render before serving".
    entries: HashMap<PathBuf, u64>,
    /// Output paths marked stale by the CURRENT tick's render callback,
    /// drained once per tick into [`zfb_build::BuildOutcome::pages_stale`]
    /// via the pipeline's stale probe.
    tick_stale: Vec<PathBuf>,
    /// Output paths of DYNAMIC injected routes that have been rendered
    /// request-time at least once (epic #1228, S5 #1233 / #1227 item (h)).
    ///
    /// Why a dedicated set: a dynamic injected route (`/preset-articles/[slug]`)
    /// has no concrete URL at boot, so it is never in `injected_static_seeds`
    /// and never enters `routes_by_source` — `diff_route_tables` therefore
    /// never reports its output_path as `changed`/`vanished`, and
    /// `mark_injected_seeds_stale` (static-only) never touches it. Without
    /// this set, the first request renders the file once, its stale entry is
    /// cleared, and a later content-edit tick leaves it fresh forever → the
    /// served HTML stays stale (the #1234 confirm-gap). The adapter records
    /// every resolved dynamic injected output here via
    /// [`DevRenderInner::note_dynamic_injected`] — UNCONDITIONALLY, so a
    /// route whose output file already existed on disk (rendered in a prior
    /// `zfb dev` run) is tracked too, not only the first-render-in-this-run
    /// case. The per-swap [`DevRenderInner::restale_dynamic_injected`] then
    /// re-claims them so the next request re-renders against the fresh
    /// content snapshot. Empty on the parity path and for static-only
    /// injected presets — zero behavioural change.
    dynamic_injected: HashSet<PathBuf>,
}

/// ABA-safe token for one request-time stale-route render (issue #1025).
///
/// Returned by [`DevRenderInner::claim`]; passed back to
/// [`DevRenderInner::clear_if_current`] after the claimed route was
/// re-rendered. Carries the generation recorded for the entry at claim
/// time, so a tick that re-stales the route mid-render (at a higher
/// generation) keeps it stale — the clear only applies when no newer
/// staling happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StaleClaim {
    /// The claimed route's relative output path.
    output_path: PathBuf,
    /// The stale entry's recorded generation at claim time.
    generation: u64,
}

impl DevRenderInner {
    /// Lazy-mode boot exception (issue #1025, review finding): returns
    /// `true` exactly once — for the session's first render-callback
    /// invocation, i.e. the eager initial build at dev boot — then
    /// `false` for every later tick. See the [`Self::boot_render_done`]
    /// field docs for why the boot render stays eager even with the
    /// lazy switch ON. If the boot build rendered zero pages (empty
    /// graph), the latch is instead consumed by the first watcher tick,
    /// which then renders eagerly once — the safe direction.
    fn take_boot_render_pending(&self) -> bool {
        !self
            .boot_render_done
            .swap(true, std::sync::atomic::Ordering::SeqCst)
    }

    /// Mark `output_paths` stale at the current generation and queue
    /// them for this tick's [`zfb_build::BuildOutcome::pages_stale`]
    /// signal. Re-marking an already-stale route bumps its recorded
    /// generation, which is what defeats the claim/clear ABA race.
    fn mark_stale<I: IntoIterator<Item = PathBuf>>(&self, output_paths: I) {
        let mut stale = self.stale.lock().unwrap_or_else(|p| p.into_inner());
        let generation = stale.generation;
        for path in output_paths {
            stale.entries.insert(path.clone(), generation);
            stale.tick_stale.push(path);
        }
    }

    /// Drop the stale entries for routes that were just rendered
    /// eagerly — fresh output supersedes any earlier staling. (Within a
    /// tick the eager set and the stale remainder are disjoint, so this
    /// can never erase a mark from the same tick.)
    fn clear_stale(&self, output_paths: &[PathBuf]) {
        let mut stale = self.stale.lock().unwrap_or_else(|p| p.into_inner());
        for path in output_paths {
            stale.entries.remove(path);
        }
    }

    /// P4 hook (issue #1025): the route tables just swapped — advance
    /// the tick generation and evict entries whose output routes
    /// vanished from the live route set (#804: a vanished route must
    /// not linger as a stale entry, or a later claim would try to
    /// render a route that no longer resolves). Also drops the vanished
    /// paths from the current tick buffer so they are never announced
    /// as stale.
    fn note_table_swap(&self, vanished: &[PathBuf]) {
        let mut stale = self.stale.lock().unwrap_or_else(|p| p.into_inner());
        stale.generation += 1;
        if !vanished.is_empty() {
            let vanished: HashSet<&PathBuf> = vanished.iter().collect();
            stale.entries.retain(|path, _| !vanished.contains(path));
            stale.tick_stale.retain(|path| !vanished.contains(path));
        }
    }

    /// Drain the routes marked stale by the current tick (consumed by
    /// the dev pipeline's stale probe — see
    /// [`zfb_build::DevAssetPipeline::with_stale_probe`]). Sorted for a
    /// deterministic [`zfb_build::BuildOutcome::pages_stale`].
    fn take_tick_stale(&self) -> Vec<PathBuf> {
        let mut drained = {
            let mut stale = self.stale.lock().unwrap_or_else(|p| p.into_inner());
            std::mem::take(&mut stale.tick_stale)
        };
        drained.sort();
        drained.dedup();
        drained
    }

    /// Claim a stale route for a request-time render (issue #1025).
    ///
    /// `Some(claim)` when `output_path` is currently stale, `None` when
    /// it is fresh (serve the cached/on-disk HTML as-is). The returned
    /// claim records the entry's generation for the ABA check in
    /// [`Self::clear_if_current`].
    ///
    /// Calling discipline (wave-4 adapter): capture the claim while
    /// holding the renderer mutex — the same lock the P2 host swap
    /// takes — so the claim/render pair is serialized against host
    /// swaps and the rendered bytes always come from a host at least as
    /// new as the claimed generation.
    fn claim(&self, output_path: &Path) -> Option<StaleClaim> {
        let stale = self.stale.lock().unwrap_or_else(|p| p.into_inner());
        stale.entries.get(output_path).map(|generation| StaleClaim {
            output_path: output_path.to_path_buf(),
            generation: *generation,
        })
    }

    /// Ensure `output_path` has a stale entry, inserting one at the
    /// current generation if absent. Returns the resulting
    /// [`StaleClaim`].
    ///
    /// Used for dynamic injected routes (epic #1228, S4 #1232) that are
    /// "stale-by-construction" — no boot seed is possible (no concrete
    /// URL), so the first request finds no stale entry yet. Marking
    /// request-time does NOT push to `tick_stale` (only tick-side marks
    /// do that, to populate [`BuildOutcome::pages_stale`]); this is a
    /// pure `entries` insert that keeps the tick channel clean.
    fn claim_or_mark_stale(&self, output_path: &Path) -> StaleClaim {
        let mut stale = self.stale.lock().unwrap_or_else(|p| p.into_inner());
        let generation = stale.generation;
        let gen = stale
            .entries
            .entry(output_path.to_path_buf())
            .or_insert(generation);
        StaleClaim {
            output_path: output_path.to_path_buf(),
            generation: *gen,
        }
    }

    /// Record `output_path` as a DYNAMIC injected route the lazy adapter
    /// has resolved (epic #1228, S5 #1233 / #1227 item (h)).
    ///
    /// Called by the adapter on EVERY dynamic-injected fallback match —
    /// independent of whether the route is marked stale this request.
    /// Crucially this includes the "file already on disk" branch (a route
    /// rendered in a previous `zfb dev` run whose `.zfb-build/dev-pages`
    /// output persisted across the restart): that branch never calls
    /// [`Self::claim_or_mark_stale`], so without this unconditional record
    /// the path would be missing from `dynamic_injected` and a later
    /// content edit could serve the stale on-disk HTML forever. Idempotent
    /// `HashSet` insert under the stale mutex.
    fn note_dynamic_injected(&self, output_path: &Path) {
        let mut stale = self.stale.lock().unwrap_or_else(|p| p.into_inner());
        stale.dynamic_injected.insert(output_path.to_path_buf());
    }

    /// Re-stale every previously-rendered DYNAMIC injected output at the
    /// current generation (epic #1228, S5 #1233 / #1227 item (h)).
    ///
    /// Called from the P4 route-table swap, right after
    /// [`Self::note_table_swap`] bumps the generation — the dynamic
    /// counterpart of [`DevRenderSession::mark_injected_seeds_stale`]
    /// (which covers only the STATIC seeds). A dynamic injected route's
    /// output_path is neither a static seed nor a member of
    /// `routes_by_source`, so nothing else in the swap re-stales it; this
    /// is the only thing that makes a content edit refresh an already-
    /// rendered dynamic injected page.
    ///
    /// Pure `entries` insert at the current (already-bumped) generation —
    /// NOT a `tick_stale` push: like the request-time mark, dynamic
    /// injected routes have no concrete `routes_by_source` entry to fan
    /// out to, so the eager pages_stale channel would do nothing for them.
    /// The next request re-renders them lazily against the fresh snapshot.
    /// The bumped generation defeats the claim/clear ABA race exactly as
    /// the static seed re-stale does.
    fn restale_dynamic_injected(&self) {
        let mut stale = self.stale.lock().unwrap_or_else(|p| p.into_inner());
        let generation = stale.generation;
        let paths: Vec<PathBuf> = stale.dynamic_injected.iter().cloned().collect();
        for path in paths {
            stale.entries.insert(path, generation);
        }
    }

    /// Clear a claimed stale entry after a successful request-time
    /// render — but ONLY if no later tick re-staled the route: the
    /// entry is removed iff `claim.generation >=` the recorded
    /// generation. A route re-staled at a higher generation mid-render
    /// stays stale (ABA safety), so the next request re-renders it
    /// against the newer world.
    fn clear_if_current(&self, claim: &StaleClaim) {
        let mut stale = self.stale.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(recorded) = stale.entries.get(&claim.output_path) {
            if claim.generation >= *recorded {
                stale.entries.remove(&claim.output_path);
            }
        }
    }

    /// Revalidation read for the guarded request-time write (issue
    /// #1027 lazy race): `true` iff the stale entry for the claim's
    /// route still exists AND still records exactly the claim's
    /// generation — i.e. no tick touched the route since the claim was
    /// captured under the renderer mutex.
    ///
    /// Either kind of mid-gap tick interference flips this `false`:
    ///
    /// - an eager re-render EVICTED the entry ([`Self::clear_stale`]) —
    ///   the tick's fresher bytes are on disk and must survive;
    /// - a re-stale at a bumped generation ([`Self::note_table_swap`] +
    ///   [`Self::mark_stale`]) — the request's bytes describe an older
    ///   world; the entry stays stale so the next request re-renders.
    ///
    /// Called by the lazy render adapter from INSIDE the pipeline
    /// exclusion lock (via `request_write_guarded`), where no tick can
    /// be in flight — the answer cannot go stale before the write.
    fn claim_is_current(&self, claim: &StaleClaim) -> bool {
        let stale = self.stale.lock().unwrap_or_else(|p| p.into_inner());
        stale.entries.get(&claim.output_path) == Some(&claim.generation)
    }
}

/// One expanded route plus its `paths()` provenance (issue #958).
///
/// The provenance is what lets a content edit narrow a dynamic source's
/// render fan-out to the routes whose params match the edited entry's
/// slug candidates — see [`compute_tick_narrowing`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct DevRouteEntry {
    entry: RouteUniverseEntry,
    /// Per-URL params retained from `DynamicExpansion::resolved_with_params`
    /// (issue #958). `None` for static routes and for any expansion whose
    /// parallel `resolved` / `resolved_with_params` vectors disagreed in
    /// length (fallback S1 — the whole source then always renders in
    /// full).
    params: Option<crate::render_pipeline::ResolvedRouteParams>,
}

/// The dev session's source→route + SSR route tables.
///
/// Issue #659 — wrapped in a single [`RwLock`] inside [`DevRenderInner`]
/// so a content file CREATED after boot can rebuild them in place (the
/// running render callback / SSR adapter read the SAME tables via the
/// shared `Arc<DevRenderInner>`). The EDIT path never mutates these — it
/// re-renders against the frozen boot tables exactly as before.
struct DevRouteTables {
    /// Mapped from the page module's project-relative source path
    /// (which is what the dependency graph keys on) to the renderer
    /// entries. Seeded at boot from the router scan; rebuilt on a
    /// watch-ADD (#659).
    ///
    /// Issue #367: only pages with `prerender != false` are kept
    /// here. Pages that opted out of SSG go into `ssr_routes`
    /// instead and reach the V8 host at request time.
    ///
    /// Issue #502/#507: the value is a `Vec` because one dynamic SSG
    /// source (`pages/blog/[slug].tsx`) expands via `paths()` into N
    /// concrete URLs that all share the same source `PageId`. A static
    /// route resolves to a single-element vec; a dynamic SSG route
    /// resolves to one entry per `paths()` URL. `render_one` emits one
    /// `RenderedPage` per entry on each tick so every fanned-out URL
    /// reaches `dist/` (and thus the dev server's disk fallback).
    ///
    /// Issue #958: each entry carries its optional `paths()` params
    /// provenance (see [`DevRouteEntry`]).
    routes_by_source: HashMap<PathBuf, Vec<DevRouteEntry>>,
    /// URL patterns for `prerender = false` pages (issue #367 /
    /// Gap 1). Empty when every page in the project SSGs. The dev
    /// server reads this list (via [`DevRenderSession::ssr_patterns`])
    /// and builds an [`zfb_server::SsrRouteSet`] from it.
    ssr_routes: Vec<RouteUniverseEntry>,
    /// Reverse lookup: normalized request URL → SSG route entry (issue
    /// #1019). Keyed by every candidate produced by
    /// [`build_url_index`] so a single `RouteUniverseEntry` is
    /// reachable via `/posts/a`, `/posts/a/`, and
    /// `/posts/a/index.html`. SSR-only routes (`prerender = false`) are
    /// intentionally absent — they are served by the existing SSR leg.
    /// Rebuilt atomically with the other tables at P4 so the index is
    /// never stale. Consumed by the lazy render adapter
    /// ([`crate::lazy_render_adapter`]) on every request-time
    /// stale-route render.
    url_index: HashMap<String, RouteUniverseEntry>,
}

/// Boot-time inputs stashed so a watch-ADD (#659) can re-bundle the SSR
/// worker with a fresh content snapshot and reload the V8 host in place.
///
/// Only compiled in on the V8 path — the discovery hook that consumes
/// these is `embed_v8`-gated like `boot_dev_renderer`.
#[cfg(feature = "embed_v8")]
struct DevRebuildInputs {
    cfg: config::Config,
    v8_plugin_hooks: zfb_render::PluginRegistryHooks,
    plugin_alias_entries: Vec<(String, String)>,
    plugin_virtual_modules: Vec<(String, String)>,
    /// Process-lifetime embedded-esbuild extraction (#994 item A): the
    /// `(TempDir, PathBuf)` pair from one boot-time
    /// [`crate::render_pipeline::embedded_binary`] call, threaded into
    /// `assemble_bundler_input` on every tick so the per-tick tempdir
    /// extraction (the bulk of the measured P1 `asm` cost) is skipped.
    /// The TempDir handle lives as long as the dev session — i.e. it
    /// outlives every `bundle()` call, trivially satisfying the
    /// [`crate::commands::bundler_input::AssembledBundlerInput`]
    /// lifetime contract. `None` when boot-time extraction failed
    /// (non-fatal: warn + per-tick fallback) or when `ZFB_ESBUILD_BIN`
    /// overrides the embedded binary.
    esbuild: Option<(tempfile::TempDir, PathBuf)>,

    /// Session-lifetime staging root for package-owned **injected routes**
    /// (epic #1228, S2 #1230 — the B1 multi-root dev mechanism). When a
    /// preset registered routes whose POST-precedence survivor set is
    /// non-empty, boot materialises the synthesized injected-only module set
    /// ONCE (via [`crate::commands::package_routes::resolve_dev_pages_root`])
    /// into this temp dir — NOT rebuilt per tick — and threads its `pages`
    /// root into the dev bundler on every `assemble_and_bundle_dev` call via
    /// the existing `build_pages_root` seam, so the injected entrypoints (and
    /// their `virtual:` imports) land in the dev bundle.
    ///
    /// `None` (the parity path) when there are no injected routes or every
    /// injected route was shadowed by a user `pages/` route: dev then passes
    /// `build_pages_root = None` and is byte-identical to today (sharp edge
    /// 8). The `TempDir` is held for the whole session so the staged modules
    /// outlive every `bundle()` call; dropping it deletes the staging dir.
    /// The `PathBuf` is the staged `pages` dir the bundler walks.
    ///
    /// NOTE: this does NOT copy real user pages. Conventional sessions retain
    /// `project_root/pages` for the router scan + watcher; #1518 zero-pages
    /// sessions use `empty_user_pages_root` only because no such directory
    /// exists, so no consumer-visible `pages/` directory is created.
    injected_pages_root: Option<(tempfile::TempDir, PathBuf)>,

    /// A session-lifetime empty pages root used only when a consumer has no
    /// real `project_root/pages` directory but package-route survivors make
    /// the dev session runnable. It satisfies the router/bundler's existing
    /// directory contract without creating a user-visible `pages/` directory
    /// in the consumer project. `None` on every conventional-pages path.
    empty_user_pages_root: Option<(tempfile::TempDir, PathBuf)>,
}

#[cfg(feature = "embed_v8")]
impl DevRebuildInputs {
    /// Path of the boot-time extracted esbuild binary (#994 item A), if any.
    fn esbuild_path(&self) -> Option<&Path> {
        self.esbuild.as_ref().map(|(_, path)| path.as_path())
    }

    /// Staged `pages` root for the injected package routes (S2 #1230), if a
    /// non-empty survivor set was materialised at boot. Threaded into the dev
    /// bundler's `build_pages_root` seam on every tick so the injected
    /// entrypoints + their `virtual:` imports are in the dev bundle. `None`
    /// on the parity path (no injected routes / all shadowed) — dev then
    /// passes `build_pages_root = None`, byte-identical to today.
    fn injected_pages_root(&self) -> Option<&Path> {
        self.injected_pages_root
            .as_ref()
            .map(|(_, path)| path.as_path())
    }

    /// The internal empty user-pages root for a true zero-pages project, if
    /// one was needed at boot. `None` means callers retain the conventional
    /// `project_root/pages` path.
    fn empty_user_pages_root(&self) -> Option<&Path> {
        self.empty_user_pages_root
            .as_ref()
            .map(|(_, path)| path.as_path())
    }
}

struct DevRenderInner {
    /// Source→route + SSR route tables (issue #659 — interior-mutable so
    /// a watch-ADD rebuilds them in place; see [`DevRouteTables`]).
    routes: std::sync::RwLock<DevRouteTables>,
    /// Mutex-wrapped renderer state. The orchestrator's callback runs
    /// on the watcher's thread; render_one is sync and short, so a
    /// global lock is fine here.
    ///
    /// Wrapped in an outer Arc (in addition to the surrounding
    /// `Arc<DevRenderInner>`) so the SSR adapter (#367) can hold a
    /// separate handle into the same V8 host without taking a
    /// dependency on `DevRenderInner` itself. The adapter clones
    /// this Arc once at construction and goes through it via
    /// `spawn_blocking` per request.
    renderer: Arc<Mutex<Option<RendererState>>>,
    /// Project root. Passed to `render_one` so it can locate the source
    /// file for static-HTML routes (#409).
    project_root: PathBuf,
    /// Boot-time bundle inputs for the watch-ADD re-bundle + host reload
    /// (issue #659). `None` would mean "no discovery" but boot always
    /// populates it on the V8 path.
    #[cfg(feature = "embed_v8")]
    rebuild_inputs: DevRebuildInputs,

    /// Per-collection resolved absolute root (issue #1550), index-aligned
    /// with `rebuild_inputs.cfg.collections`. In-root collections carry the
    /// literal `project_root.join(path)`; out-of-root collections
    /// (`allowOutsideRoot`, #1549) carry the CANONICAL absolute root. Read
    /// by [`derive_tick_candidates`] and [`make_discovery_hook`] so each
    /// site compares against a root form that matches notify's canonical
    /// event paths. Built once from the boot [`ResolvedRoots`] inventory.
    #[cfg(feature = "embed_v8")]
    collection_roots: Vec<PathBuf>,

    /// Skip key from the last SUCCESSFUL `refresh_bundle_and_routes` call
    /// (issue #940 — Phase B). Hashes bundle bytes + the router scan's
    /// sorted source paths + route templates + static `pages/**.html`
    /// bodies (issue #956) so a no-op tick (identical bundle, identical
    /// route universe) skips V8 host boot + swap and the `paths()`
    /// re-expansion. Stored only after ALL of: host boot, swap, AND
    /// route-table rebuild succeed — a failed refresh must NOT poison
    /// this field so the next byte-identical tick can retry.
    ///
    /// `None` on first tick (cold start) — forces a full refresh.
    /// Wrapped in a `Mutex` so `&self.refresh_bundle_and_routes` can
    /// update it without `&mut self`.
    #[cfg(feature = "embed_v8")]
    last_successful_skip_key: Mutex<Option<[u8; 32]>>,

    /// Frontmatter gate cache for content-edit narrowing (issue #958,
    /// fallback G4): SHA-256 of the canonical JSON of each collection
    /// file's parsed frontmatter, keyed by the file's absolute path
    /// exactly as the watcher delivers it. Seeded at boot
    /// ([`seed_frontmatter_hashes`]) and updated on every narrowing-
    /// candidate Content tick. A missing or differing entry means the
    /// frontmatter may feed cross-page props (sidebar titles, prev/next
    /// labels) — no narrowing that tick.
    #[cfg(feature = "embed_v8")]
    fm_hashes: Mutex<HashMap<PathBuf, [u8; 32]>>,

    /// Persistent dev shadow-tree session (issue #993): the bundler
    /// reuses one shadow tempdir across all ticks, skipping
    /// byte-identical rewrites ("compute always, write only if
    /// changed" — see [`zfb_build::bundler::ShadowSession`] for the
    /// safety model). Created at boot ([`boot_dev_renderer`]) and used
    /// for the boot bundle AND every refresh, so both go through the
    /// identical assembly path + shadow tree (#659 parity by
    /// construction). The lock is held only across the P1 bundle step
    /// of [`DevRenderSession::refresh_bundle_and_routes`] — it never
    /// overlaps the renderer mutex, so SSR latency is unaffected.
    #[cfg(feature = "embed_v8")]
    shadow_session: Mutex<Option<ShadowSession>>,

    /// Dependency graph handle for populating per-route `DepKind::Module`
    /// edges from esbuild's metafile on every bundle refresh (#1284/#1287).
    /// Installed after boot via [`DevRenderSession::set_dep_graph`] — the
    /// graph is created by `run` AFTER `boot_dev_renderer` returns, so it is
    /// `None` until then (and on the test/scaffold constructors that never
    /// run the real refresh). When present, [`Self::refresh_bundle_and_routes`]
    /// upserts each route's transitive module deps so a component edit
    /// (direct or transitive, incl. symlinked workspace `.tsx`) maps to its
    /// consuming route via `dirty_pages`, instead of falling back to a blunt
    /// whole-site re-render.
    #[cfg(feature = "embed_v8")]
    dep_graph: Mutex<Option<Arc<Mutex<DependencyGraph>>>>,

    /// Current worker's actual content-read observations (issue #1600).
    /// Reset after every successful bundle/route-table swap; a fresh worker
    /// must not inherit observations captured against an older snapshot.
    #[cfg(feature = "embed_v8")]
    content_trace: Mutex<DevContentTraceState>,

    /// Real on-disk module-dep paths that live OUTSIDE `project_root` —
    /// canonicalised symlink targets of workspace `.tsx` deps esbuild resolved
    /// through `node_modules` (#1284/#1287, D4). `notify` does not follow
    /// symlinks and `node_modules` is excluded from the recursive watch, so
    /// these must be registered as `extraWatchPaths`-style targets for an edit
    /// of the real workspace file to fire a tick. Accumulated by
    /// [`Self::populate_module_edges`] every refresh; read by `run` to extend
    /// the watcher's extra targets. A `BTreeSet`-backed dedup keeps it stable.
    #[cfg(feature = "embed_v8")]
    out_of_root_watch_targets: Mutex<std::collections::BTreeSet<PathBuf>>,

    /// The eager boot bundle's per-route metafile Module deps (#1284/#1287),
    /// captured in `boot_dev_renderer` before the graph exists. `run` seeds
    /// these into the graph via [`Self::populate_module_edges`] right after
    /// installing the graph handle, so a component edit maps to its consuming
    /// route from the very first edit tick (not one tick late). Empty on the
    /// deferred-boot path (its `refresh_bundle_and_routes` seeds edges itself).
    #[cfg(feature = "embed_v8")]
    boot_route_module_deps: Vec<zfb_build::RouteModuleDeps>,

    /// Cross-tick [`PathsCache`] (#994 item B): seeded at boot and
    /// passed into every route-table build, so a `paths()` JSON output
    /// identical to a previous tick's skips the Rust-side
    /// validate/URL-build in `resolve_paths`. NOTE the corrected scope
    /// (#992): the cache lookup happens AFTER the V8 `/__paths__/`
    /// dispatch — the key includes the hash of the dispatch RESULT — so
    /// persistence does NOT skip the per-tick V8 evals; the saving is
    /// the ~1–5 ms Rust tail only. Sound by construction: a changed
    /// `paths()` output can never hit a stale entry (key = template +
    /// JSON hash) and stale entries are inert. The lock is scoped to
    /// the P3 route-table build; the renderer lock for phase-2 eval is
    /// taken inside the build exactly as before, so renderer-mutex
    /// hold time is unchanged.
    #[cfg(feature = "embed_v8")]
    paths_cache: Mutex<PathsCache>,

    /// Lazy dev render (issue #1025): per-route staleness map + tick
    /// generation. See [`StaleRoutes`]. Present on both V8 paths — the
    /// state machine itself has no V8 dependency; only the callback
    /// split that feeds it is `embed_v8`-gated.
    stale: Mutex<StaleRoutes>,

    /// Lazy dev render switch (issue #1025), resolved once at boot via
    /// [`lazy_dev_render_enabled`]. `true` (the default since the #1027
    /// activation flip) routes ticks through [`lazy_render_tick`]'s
    /// eager-vs-stale split; `false` (the `ZFB_DEV_EAGER=1` escape
    /// hatch) keeps the render callback fully eager.
    lazy_render: bool,

    /// One-shot boot latch for lazy mode (issue #1025, review): `false`
    /// until the FIRST render-callback invocation of the session — the
    /// eager initial build (zfb#642/#644) — which must render fully
    /// eagerly even with the lazy switch ON. The request-time
    /// stale-route adapter lands in a later sub-issue, so a lazy boot
    /// would leave `.zfb-build/dev-pages/` empty and 404 every route.
    /// Consumed via [`Self::take_boot_render_pending`]; only read on
    /// the lazy branch.
    boot_render_done: std::sync::atomic::AtomicBool,

    /// Static injected-route seeds (epic #1228, S3 #1231). One
    /// [`RouteUniverseEntry`] per **static, SSG** package-owned injected
    /// route that survived precedence at boot (URL == pattern, e.g.
    /// `/preset-about`). Computed ONCE from the post-precedence survivor
    /// set ([`crate::commands::package_routes::static_injected_seeds`], so
    /// a user-shadowed or package-vs-package-dropped pattern never leaks —
    /// sharp edges 4/7) and held for the whole session. A `prerender:
    /// false` (SSR-only) static injected route is excluded from this set —
    /// it is not SSG'd to disk, matching `pages/` parity.
    ///
    /// These are merged into `routes_by_source` + `url_index` (and
    /// stale-marked) at boot AND re-merged on EVERY route-table swap
    /// ([`DevRenderSession::reseed_injected_static_routes`]): the swap
    /// rebuilds `routes_by_source` from the router scan alone, which does
    /// NOT walk the staged injected modules (they live outside the real
    /// `pages/`), so without the re-merge the seeded routes would vanish
    /// from the universe on the first tick. Empty on the parity path (no
    /// static injected survivors) — zero behavioural change.
    injected_static_seeds: Vec<RouteUniverseEntry>,

    /// Post-precedence survivor [`InjectedRouteSet`] (epic #1228, S3
    /// #1231, §7). Built from the SAME survivor set that backs the static
    /// seeds and the staged dev bundle, NOT from the raw registration
    /// list — so a user-shadowed (or package-vs-package-dropped) pattern
    /// is already absent and can never match in the request-time fallback
    /// the future S4 wave adds to [`crate::lazy_render_adapter`] (sharp
    /// edges 4/7). `run` reads this off the session
    /// ([`DevRenderSession::injected_route_set`]) to populate
    /// [`zfb_server::ServeOpts::injected_routes`]. Empty (`default`) on
    /// the parity path.
    injected_route_set: InjectedRouteSet,
}

/// Per-source route filter for one narrowed tick (issue #958): which of a
/// source's expanded routes [`DevRenderSession::render_one`] should fan
/// out to this invocation.
#[cfg_attr(not(feature = "embed_v8"), allow(dead_code))]
#[derive(Debug)]
enum RouteFilter {
    /// Render every entry of the source — today's full behaviour.
    All,
    /// Render only the entries whose `output_path` is in the set.
    Only(HashSet<PathBuf>),
}

impl RouteFilter {
    fn allows(&self, output_path: &Path) -> bool {
        match self {
            RouteFilter::All => true,
            RouteFilter::Only(set) => set.contains(output_path),
        }
    }
}

/// The whole tick's narrowing decision, computed once per render-callback
/// invocation by [`compute_tick_narrowing`] (issue #958).
#[cfg_attr(not(feature = "embed_v8"), allow(dead_code))]
#[derive(Debug)]
enum TickNarrowing {
    /// No narrowing — every selected page renders its full fan-out.
    Off,
    /// Per-source filters. The map contains ONLY narrowed sources; a
    /// source absent from the map renders in full ([`RouteFilter::All`]) —
    /// that absence IS the always-rendered set (statics, single-entry
    /// sources, aggregate dynamic consumers that fell back via S1/S2).
    PerSource(HashMap<PathBuf, RouteFilter>),
}

#[cfg(feature = "embed_v8")]
impl DevRenderInner {
    /// Phase B (issue #940) — true when `new_key` matches the skip key of
    /// the last SUCCESSFUL refresh, i.e. the refresh may legally skip the
    /// V8 host boot + swap and the `paths()` re-expansion. `new_key =
    /// None` (key not computable) never skips.
    fn should_skip_refresh(&self, new_key: Option<[u8; 32]>) -> bool {
        let prev = *self
            .last_successful_skip_key
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        new_key.is_some() && prev == new_key
    }

    /// Phase B (issue #940) — record the refresh tick's skip key after a
    /// FULLY successful refresh (host boot + swap + route-table rebuild).
    ///
    /// Must NOT be called on a failed refresh — the previous successful
    /// key has to survive so the next byte-identical tick retries in full
    /// (Correctness Req 1: a failed refresh never poisons the key).
    ///
    /// Passing `None` (key was not computable this tick) CLEARS the stored
    /// key: a stale key no longer describes the live renderer's bundle,
    /// and a later tick matching it would skip against the wrong host
    /// state. Clearing forces the next tick to refresh fully (safe
    /// direction: false-invalidate, never false-reuse).
    fn commit_skip_key(&self, new_key: Option<[u8; 32]>) {
        let mut stored = self
            .last_successful_skip_key
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        *stored = new_key;
    }
}

impl DevRenderSession {
    /// Drive a single source page id against the renderer. Returns one
    /// [`RenderedPage`] per [`RouteUniverseEntry`] mapped to this source
    /// path, each populated with the bytes the renderer just wrote, so
    /// the dev pipeline's atomic-write + cache layer can fold the result
    /// through the existing reload broadcast.
    ///
    /// A static route maps to a single entry; a dynamic SSG route
    /// (`pages/blog/[slug].tsx`) maps to N entries — one per concrete URL
    /// its `paths()` resolved to (issue #502/#507). Returns an empty Vec
    /// when the source path is unknown to the renderer (dynamic route
    /// deferred to SSR, or a page never seen by the router scan).
    ///
    /// `filter` (issue #958) narrows the fan-out to a subset of the
    /// source's entries on a narrowed content-edit tick; pass
    /// [`RouteFilter::All`] for the full fan-out. Filtering happens on
    /// the per-tick clone, strictly BEFORE the render loop — the shared
    /// tables are never mutated and `render_one_with`'s synthetic
    /// output-path `PageId` keying (#507 Guardrail 1) is untouched.
    fn render_one(
        &self,
        page: &PageId,
        dist_dir: &Path,
        filter: &RouteFilter,
    ) -> Result<Vec<RenderedPage>> {
        // Read the (possibly watch-ADD-rebuilt, #659) route table. Clone
        // the entries out so the lock is released before the V8 render —
        // the reload path takes the write lock, and `render_one` may run
        // on the same tick that just rebuilt the table.
        let entries = match self
            .inner
            .routes
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .routes_by_source
            .get(page.path())
        {
            Some(es) => Self::filter_entries(es, filter),
            None => return Ok(Vec::new()),
        };
        let mut lock = self.inner.renderer.lock().unwrap_or_else(|p| {
            tracing::warn!(site = "DevRenderSession", "mutex poisoned, recovered");
            p.into_inner()
        });
        let state = lock
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("renderer not started"))?;
        let project_root = self.inner.project_root.clone();
        // Delegate the loop to `render_one_with`, injecting the real V8
        // render call as the per-entry closure. This keeps the fan-out logic
        // testable without a live renderer (see the seam test below).
        Self::render_one_with(page, &entries, |entry| {
            render_one(state, entry, dist_dir, &project_root).map_err(anyhow::Error::from)
        })
    }

    /// Apply a tick's per-source [`RouteFilter`] to a source's entries,
    /// cloning out the [`RouteUniverseEntry`] payloads that survive
    /// (issue #958). Extracted from [`Self::render_one`] so the narrowing
    /// seam is testable without a live V8 renderer.
    fn filter_entries(entries: &[DevRouteEntry], filter: &RouteFilter) -> Vec<RouteUniverseEntry> {
        entries
            .iter()
            .filter(|de| filter.allows(&de.entry.output_path))
            .map(|de| de.entry.clone())
            .collect()
    }

    /// Fan-out loop: for each `RouteUniverseEntry` in `entries`, call
    /// `render_entry` to produce the path of the written HTML file, read
    /// it back, and wrap it in a `RenderedPage` with a synthetic `PageId`
    /// derived from the entry's `output_path`.
    ///
    /// Extracted from `render_one` so the per-entry render step can be
    /// replaced with a stub in tests, avoiding the need for a live V8
    /// `RendererState`. The public `render_one` delegates here with the
    /// real `zfb_build::renderer::render_one` call as the closure.
    fn render_one_with<F>(
        page: &PageId,
        entries: &[RouteUniverseEntry],
        mut render_entry: F,
    ) -> Result<Vec<RenderedPage>>
    where
        F: FnMut(&RouteUniverseEntry) -> Result<PathBuf>,
    {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let written = render_entry(entry)?;
            let html = std::fs::read_to_string(&written)
                .with_context(|| format!("failed to read rendered page {}", written.display()))?;
            // RouteUniverseEntry::output_path is a PathBuf validated by the
            // router/render_pipeline (relative, no escapes). Wrap it in
            // RelDistPath for the pipeline's type contract. If the path is
            // somehow invalid, surface an error rather than silently skipping.
            let output_path = RelDistPath::new(entry.output_path.clone())
                .with_context(|| format!("renderer returned invalid output_path for {:?}", page))?;
            // Guardrail 1 (#507): `DevAssetPipeline.last_output_path` keys on
            // the `RenderedPage.page` id and prunes the previous artifact
            // when a page's output path changes. N URLs that share one source
            // `PageId` would collide on that key — the pipeline would prune
            // all but the last fanned-out URL each tick, so only one dynamic
            // page would survive on disk. Give each resolved URL a distinct,
            // deterministic synthetic `PageId` derived from its output path so
            // every URL gets its own pipeline bookkeeping slot (stable across
            // ticks → byte-dedup still works). The synthetic id is internal to
            // the pipeline's per-tick bookkeeping; the dependency graph keys
            // on real source paths only (see `page_ids`).
            //
            // Coupling landmine (read before adding watch-time router re-scan):
            // every route — static included — now keys `RenderedPage.page` on
            // `output_path`, not the source path. The pipeline's stale-output
            // prune (DevAssetPipeline.last_output_path) was designed around a
            // *stable source id* whose output_path can flip across ticks (e.g.
            // sitemap.xml → sitemap.rss); output-path keying makes such a flip
            // produce two distinct keys, so that per-page prune mechanism can
            // never fire for it. Since #958 strengthened `diff_route_tables`
            // to full entry-set comparison, a stable-count output_path flip IS
            // re-rendered — but the old artifact is deleted by the GLOBAL
            // vanished-output diff (#804: the old path drops out of the live
            // route set and `route_vanished` prunes it from disk + cache), so
            // no orphan accumulates. If that global diff is ever weakened,
            // restore source-path keying for static routes (or key dynamic
            // entries on (source_path, output_path)).
            out.push(RenderedPage {
                page: PageId::new(entry.output_path.clone()),
                output_path,
                html,
                content_type: None,
            });
        }
        Ok(out)
    }

    /// Clone the shared handle to the embedded V8 host so the SSR
    /// adapter can dispatch requests through the same renderer state
    /// that drives build-time SSG (#367). Cheap — the underlying
    /// renderer is wrapped in an `Arc<Mutex<...>>` already.
    ///
    /// `pub(crate)` (issue #1026): the lazy render adapter renders
    /// stale routes through the same handle.
    pub(crate) fn renderer_handle(&self) -> Arc<Mutex<Option<RendererState>>> {
        Arc::clone(&self.inner.renderer)
    }

    /// Install the dependency-graph handle so every bundle refresh can
    /// populate per-route `DepKind::Module` edges from esbuild's metafile
    /// (#1284/#1287). Called by `run` once the graph exists (it is created
    /// after `boot_dev_renderer` returns). Idempotent — a later call replaces
    /// the handle.
    #[cfg(feature = "embed_v8")]
    pub(crate) fn set_dep_graph(&self, graph: Arc<Mutex<DependencyGraph>>) {
        if let Ok(mut slot) = self.inner.dep_graph.lock() {
            *slot = Some(graph);
        }
    }

    /// Seed the graph with the eager boot bundle's per-route Module edges
    /// (#1284/#1287), captured before the graph existed. Call once right after
    /// [`Self::set_dep_graph`]. No-op on the deferred-boot path (its deps are
    /// empty; the deferred refresh seeds edges itself) and idempotent.
    #[cfg(feature = "embed_v8")]
    pub(crate) fn seed_boot_module_edges(&self) {
        let deps = self.inner.boot_route_module_deps.clone();
        self.populate_module_edges(&deps);
    }

    /// The persisted graph is only a cache. Once the boot graph has been
    /// assembled, discard its Content edges and rebuild them from observations
    /// made by the live worker during the eager boot render.
    #[cfg(feature = "embed_v8")]
    fn clear_boot_content_provenance(&self) {
        if let Ok(mut state) = self.inner.content_trace.lock() {
            state.reads_by_observation.clear();
            state.boot_complete = false;
        }
        self.clear_content_edges();
    }

    /// Adopt a fresh worker's private trace endpoint. In-memory observations
    /// remain as a conservative bridge until the new worker visits each route;
    /// that visit replaces the route's prior read set, including with an empty
    /// set when it no longer reads a collection. Persisted observations are
    /// never used at cold boot.
    #[cfg(feature = "embed_v8")]
    fn begin_content_trace(&self, token: String) {
        if let Ok(mut state) = self.inner.content_trace.lock() {
            state.token = Some(token);
        }
    }

    /// Enable provenance only after the eager boot render has had a chance to
    /// execute both `paths()` and ordinary page reads.
    #[cfg(feature = "embed_v8")]
    fn complete_boot_content_provenance(&self) -> Result<()> {
        if let Ok(mut state) = self.inner.content_trace.lock() {
            state.boot_complete = true;
        }
        self.reconcile_content_provenance()
    }

    #[cfg(feature = "embed_v8")]
    fn content_graph(&self) -> Option<Arc<Mutex<DependencyGraph>>> {
        self.inner
            .dep_graph
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().cloned())
    }

    /// Remove only Content edges, preserving every other dependency kind and
    /// allowing the planner's unknown-path fallback to select all pages.
    #[cfg(feature = "embed_v8")]
    fn clear_content_edges(&self) {
        let Some(graph) = self.content_graph() else {
            return;
        };
        if let Ok(mut graph) = graph.lock() {
            replace_content_edges(
                &mut graph,
                std::iter::empty::<zfb_build::ContentEdgeGroup>(),
            );
        };
    }

    #[cfg(feature = "embed_v8")]
    fn clear_content_edges_if_current(&self, token: &str) {
        let Ok(state) = self.inner.content_trace.lock() else {
            return;
        };
        if state.token.as_deref() != Some(token) {
            return;
        }
        let Some(graph) = self.content_graph() else {
            return;
        };
        if let Ok(mut graph) = graph.lock() {
            replace_content_edges(
                &mut graph,
                std::iter::empty::<zfb_build::ContentEdgeGroup>(),
            );
        };
    }

    /// Drain actual `getCollection()` observations from the current worker and
    /// reconcile the graph atomically. Any failure removes Content edges for
    /// this worker generation so the existing unknown-path fallback remains
    /// conservative.
    #[cfg(feature = "embed_v8")]
    pub(crate) fn reconcile_content_provenance(&self) -> Result<()> {
        let token = {
            let state = self
                .inner
                .content_trace
                .lock()
                .map_err(|_| anyhow::anyhow!("content provenance state mutex poisoned"))?;
            if !state.boot_complete {
                return Ok(());
            }
            state.token.clone()
        };
        let Some(token) = token else {
            return Ok(());
        };

        let result = (|| {
            let mut headers = BTreeMap::new();
            headers.insert(DEV_CONTENT_TRACE_HEADER.to_string(), token.clone());
            let response = {
                let mut renderer = self.inner.renderer.lock().map_err(|_| {
                    anyhow::anyhow!("renderer mutex poisoned while draining content trace")
                })?;
                let renderer = renderer.as_mut().ok_or_else(|| {
                    anyhow::anyhow!("renderer is unavailable while draining content trace")
                })?;
                let host = renderer.embedded_v8_host_mut().ok_or_else(|| {
                    anyhow::anyhow!(
                        "renderer is not backed by embedded V8 while draining content trace"
                    )
                })?;
                host.dispatch_fetch_full(DEV_CONTENT_TRACE_ENDPOINT, "GET", &headers, &[])
                    .map_err(anyhow::Error::from)?
            };
            if response.status != 200 {
                anyhow::bail!("content provenance drain returned HTTP {}", response.status);
            }
            let payload: DevContentTracePayload = serde_json::from_slice(&response.body)
                .context("decoding content provenance trace payload")?;
            if !payload.ready {
                anyhow::bail!("content provenance trace wrapper is not ready");
            }
            if let Some(error) = payload.error {
                anyhow::bail!("content provenance trace wrapper failed: {error}");
            }

            let membership = collect_content_provenance_membership(
                &self.inner.rebuild_inputs.cfg,
                &self.inner.collection_roots,
            )?;
            let current_trace = {
                let tables = self.inner.routes.read().map_err(|_| {
                    anyhow::anyhow!("route table lock poisoned while classifying content trace")
                })?;
                classify_content_trace_events(
                    payload.events,
                    &tables.routes_by_source,
                    &membership,
                    &self.inner.project_root,
                )?
            };

            let mut state = self
                .inner
                .content_trace
                .lock()
                .map_err(|_| anyhow::anyhow!("content provenance state mutex poisoned"))?;
            if !state.boot_complete || state.token.as_deref() != Some(token.as_str()) {
                return Ok(());
            }
            apply_content_trace_observations(
                &mut state.reads_by_observation,
                current_trace.observed,
                current_trace.reads,
            );
            let reads: Vec<TrackedContentRead> = state
                .reads_by_observation
                .values()
                .flatten()
                .cloned()
                .collect();
            let groups = ContentProvenance::from_reads(reads)
                .edge_groups(&membership.membership)
                .map_err(anyhow::Error::from)
                .context("expanding content provenance membership")?;
            if let Some(graph) = self.content_graph() {
                let mut graph = graph
                    .lock()
                    .map_err(|_| anyhow::anyhow!("dependency graph mutex poisoned"))?;
                replace_content_edges(&mut graph, groups);
            }
            Ok(())
        })();

        if result.is_err() {
            self.clear_content_edges_if_current(&token);
        }
        result
    }

    /// Upsert each route's transitive module deps (from esbuild's metafile)
    /// as `DepKind::Module` edges, so a component edit maps to its consuming
    /// route via `dirty_pages`. No-op when the graph handle is not installed
    /// or the bundle carried no metafile deps (e.g. the mock/scaffold path).
    ///
    /// `upsert` REPLACES a page's entire dep set, so to avoid clobbering the
    /// `DepKind::Content` edges the discovery hook records for the SAME page
    /// (a route can import a component AND consume a content collection), this
    /// MERGES: it reads the page's current non-`Module` deps, drops the stale
    /// `Module` edges (re-derived fresh from the authoritative metafile every
    /// refresh, so a removed import drops its edge next tick), and re-upserts
    /// the preserved Content/Style/Data edges alongside the new Module set. The
    /// page self-edge is re-added by `upsert` regardless.
    #[cfg(feature = "embed_v8")]
    fn populate_module_edges(&self, deps: &[zfb_build::RouteModuleDeps]) {
        if deps.is_empty() {
            return;
        }
        let graph = {
            let slot = match self.inner.dep_graph.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            match slot.as_ref() {
                Some(g) => Arc::clone(g),
                None => return,
            }
        };
        let project_root = &self.inner.project_root;
        let mut new_out_of_root: Vec<PathBuf> = Vec::new();
        if let Ok(mut g) = graph.lock() {
            for route in deps {
                // The page key must match how the rest of the dev graph keys
                // pages (project-root-joined absolute path — see the boot seed
                // and the Content upsert path).
                let page_id = PageId::new(project_root.join(&route.source_path));

                // Preserve the page's existing NON-Module deps (Content/Style/
                // Data recorded by the discovery hook); only the Module edges
                // are replaced from the metafile this tick. The self-edge is
                // re-added by `upsert`, so drop it here too to avoid a dup.
                let mut edges: Vec<(PathBuf, zfb_graph::DepKind)> = g
                    .deps_of(&page_id)
                    .into_iter()
                    .filter(|(dep, kind)| {
                        *kind != zfb_graph::DepKind::Module && dep != page_id.path()
                    })
                    .collect();

                for real in &route.module_deps {
                    edges.push((real.clone(), zfb_graph::DepKind::Module));
                    // A real dep path outside `project_root` is a symlinked
                    // workspace dep (esbuild canonicalised it). Collect it so
                    // the watcher can register it as an extra target (#1284 D4).
                    if !real.starts_with(project_root) {
                        new_out_of_root.push(real.clone());
                    }
                }

                g.upsert(PageDeps::new(page_id, edges));
            }
        }
        if !new_out_of_root.is_empty() {
            if let Ok(mut set) = self.inner.out_of_root_watch_targets.lock() {
                set.extend(new_out_of_root);
            }
        }
    }

    /// Snapshot of the out-of-root real Module-dep paths discovered so far
    /// (#1284/#1287, D4) — canonicalised symlink targets of workspace `.tsx`
    /// deps that must be registered as extra watch targets. Read by `run`
    /// after the boot bundle so a symlinked workspace component edit fires a
    /// tick (the in-repo `src/**` case is covered by `DEFAULT_WATCH_ROOTS`).
    #[cfg(feature = "embed_v8")]
    pub(crate) fn out_of_root_watch_targets(&self) -> Vec<PathBuf> {
        self.inner
            .out_of_root_watch_targets
            .lock()
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The boot-resolved lazy dev-render switch (issue #1025/#1026).
    /// Read by the request-time adapter as its cheap "do nothing"
    /// early-return — when `false`, the session behaves byte-identically
    /// to the fully-eager world.
    pub(crate) fn lazy_render_enabled(&self) -> bool {
        self.inner.lazy_render
    }

    /// Project root for [`zfb_build::renderer::render_one`]'s
    /// static-HTML source reads (issue #1026 — request-time renders use
    /// the same render path as the tick fan-out).
    pub(crate) fn project_root(&self) -> &Path {
        &self.inner.project_root
    }

    /// Forward of [`DevRenderInner::claim`] for the lazy render adapter
    /// (issue #1026). See `claim` for the ABA token contract and the
    /// "capture under the renderer mutex" calling discipline.
    pub(crate) fn claim_stale(&self, output_path: &Path) -> Option<StaleClaim> {
        self.inner.claim(output_path)
    }

    /// Forward of [`DevRenderInner::claim_or_mark_stale`] for dynamic
    /// injected routes (epic #1228, S4 #1232). Dynamic injected routes
    /// are stale-by-construction — no boot seed provides a stale entry,
    /// so the first request-time render must mark the route stale and
    /// claim it atomically. Does NOT push to `tick_stale`; the tick
    /// channel stays clean.
    pub(crate) fn claim_or_mark_stale_for_dynamic_route(&self, output_path: &Path) -> StaleClaim {
        self.inner.claim_or_mark_stale(output_path)
    }

    /// Forward of [`DevRenderInner::note_dynamic_injected`] for the lazy
    /// render adapter (epic #1228, S5 #1233 / #1227 item (h)). The adapter
    /// calls this on every dynamic-injected fallback match so a content
    /// edit can later re-stale the route — including when its output file
    /// already existed on disk and `claim_or_mark_stale_for_dynamic_route`
    /// was therefore skipped.
    pub(crate) fn note_dynamic_injected_route(&self, output_path: &Path) {
        self.inner.note_dynamic_injected(output_path)
    }

    /// Forward of [`DevRenderInner::clear_if_current`] for the lazy
    /// render adapter (issue #1026).
    pub(crate) fn clear_stale_claim(&self, claim: &StaleClaim) {
        self.inner.clear_if_current(claim)
    }

    /// Forward of [`DevRenderInner::claim_is_current`] for the lazy
    /// render adapter's guarded-write revalidation (issue #1027 lazy
    /// race). See `claim_is_current` for the exact freshness predicate.
    pub(crate) fn claim_is_current(&self, claim: &StaleClaim) -> bool {
        self.inner.claim_is_current(claim)
    }

    /// Test-only seam: mark routes stale so adapter tests can inject
    /// staleness state without driving a whole watcher tick.
    #[cfg(test)]
    pub(crate) fn mark_routes_stale<I: IntoIterator<Item = PathBuf>>(&self, output_paths: I) {
        self.inner.mark_stale(output_paths)
    }

    /// Test-only seam: simulate the tick-side eager-render eviction
    /// ([`DevRenderInner::clear_stale`]) so adapter tests can replay a
    /// tick interleaving in the renderer-release → write gap (#1027).
    #[cfg(test)]
    pub(crate) fn clear_routes_stale(&self, output_paths: &[PathBuf]) {
        self.inner.clear_stale(output_paths)
    }

    /// Test-only seam: simulate the P4 table-swap generation bump
    /// ([`DevRenderInner::note_table_swap`] with nothing vanished) so
    /// adapter tests can replay a mid-gap re-stale at a newer
    /// generation (#1027).
    #[cfg(test)]
    pub(crate) fn bump_stale_generation(&self) {
        self.inner.note_table_swap(&[])
    }

    /// The post-precedence survivor [`InjectedRouteSet`] (epic #1228, S3
    /// #1231, §7). `run` reads it off the session to populate
    /// [`zfb_server::ServeOpts::injected_routes`] so the dev server (and
    /// the future S4 request-time fallback) only ever sees patterns that
    /// survived precedence — never a user-shadowed or dropped one (sharp
    /// edges 4/7). Cheap clone (`Arc` bump). Empty (`default`) on the
    /// parity path.
    pub(crate) fn injected_route_set(&self) -> InjectedRouteSet {
        self.inner.injected_route_set.clone()
    }

    /// Mark every static injected-route seed's `output_path` stale (epic
    /// #1228, S3 #1231, §3). Called at boot — so the FIRST request for an
    /// injected URL renders rather than 404s on an absent file — and again
    /// after EVERY route-table swap, so a content edit that should refresh
    /// an already-rendered injected page re-claims it on the next request
    /// (the seed stays present in `routes_by_source` across swaps, so
    /// without the per-swap re-stale it would never be re-rendered —
    /// sharp edge 5). No-op on the parity path (no seeds).
    fn mark_injected_seeds_stale(&self) {
        if self.inner.injected_static_seeds.is_empty() {
            return;
        }
        self.inner.mark_stale(
            self.inner
                .injected_static_seeds
                .iter()
                .map(|e| e.output_path.clone()),
        );
    }

    /// Return the URL patterns of every page that exports
    /// `prerender = false` (issue #367 / Gap 1). Used by the dev
    /// command to build the [`zfb_server::SsrRouteSet`] handed to
    /// the dev router.
    ///
    /// `zfb_router::Route::template()` emits Hono-style colon syntax
    /// (`/blog/:slug`, `/docs/:slug{.+}`), but `SsrRouteSet`'s matcher
    /// is `pages/`-style (`/blog/[slug]`, `/docs/[...slug]`). Translate
    /// here so dev SSR matches dynamic-route URLs at all. The two public
    /// grammars (this one via SSR + `with_ssr_handler`'s axum-style via
    /// `embed_handlers`) diverge by historical accident — see the TODO
    /// in `crates/zfb-server/src/embed_handlers.rs` for the unify-later
    /// follow-up.
    fn ssr_patterns(&self) -> Vec<String> {
        self.inner
            .routes
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .ssr_routes
            .iter()
            .map(|e| colon_template_to_bracket(&e.route_key))
            .collect()
    }

    /// Look up the SSG [`RouteUniverseEntry`] for a request URL path
    /// (issue #1019 — reverse URL index).
    ///
    /// `request_path` must be:
    /// - Base-prefix-stripped (the server removes the prefix before
    ///   dispatching — the index never sees it).
    /// - May carry a query string (stripped here before lookup).
    /// - May be percent-encoded (decoded here before lookup so
    ///   `/posts/caf%C3%A9` resolves like `/posts/café`).
    ///
    /// Returns `Some(entry)` when a matching SSG route exists, `None` for
    /// SSR-only routes, dynamic routes that were never expanded, and any
    /// path not in the route universe. The returned entry is cloned out
    /// under a short read lock — no lock is held by the caller.
    pub(crate) fn lookup_by_url(&self, request_path: &str) -> Option<RouteUniverseEntry> {
        // Strip query string (everything from the first `?`).
        let path_only = request_path.split('?').next().unwrap_or(request_path);

        // Percent-decode (consistent with how `read_from_dist` treats paths).
        // On decode error fall back to the raw path so the lookup still
        // proceeds rather than panicking.
        let decoded = percent_decode_url(path_only);

        // Strip a leading slash so `url_index_lookup_keys` can prepend it
        // uniformly (the function expects the path WITHOUT a leading slash).
        let without_leading = decoded.trim_start_matches('/');

        let tables = self.inner.routes.read().unwrap_or_else(|p| p.into_inner());
        for key in url_index_lookup_keys(without_leading) {
            if let Some(entry) = tables.url_index.get(&key) {
                return Some(entry.clone());
            }
        }
        None
    }

    /// Return all known page IDs (source paths) from the router scan.
    /// Used to seed the dependency graph so incremental rebuilds have a
    /// non-empty page set to resolve against.
    fn page_ids(&self) -> Vec<PageId> {
        self.inner
            .routes
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .routes_by_source
            .keys()
            .map(|p| PageId::new(p.clone()))
            .collect()
    }

    /// Number of SSG source routes the renderer knows about. Used by the
    /// eager initial-build step to detect the "0 pages rendered for N
    /// routes" silent-failure case (zfb#642 / #644): if this is non-zero
    /// but the initial render produced nothing, every route would 404.
    fn route_count(&self) -> usize {
        self.inner
            .routes
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .routes_by_source
            .len()
    }

    /// Mark EVERY known SSG route stale (issue #1057 boot-lazy). Used in
    /// place of the eager initial render: the dev server serves the prebuilt
    /// `dist/` for each route via the `read_from_dist` cold-cache fallback,
    /// and the request-time render-on-request hook (#1026) re-renders each
    /// route on its first GET/HEAD. Returns the number of routes marked.
    fn mark_all_routes_stale(&self) -> usize {
        let outputs: Vec<PathBuf> = {
            let tables = self.inner.routes.read().unwrap_or_else(|p| p.into_inner());
            tables
                .routes_by_source
                .values()
                .flat_map(|entries| entries.iter().map(|de| de.entry.output_path.clone()))
                .collect()
        };
        let n = outputs.len();
        self.inner.mark_stale(outputs);
        n
    }

    /// Tear down the underlying [`RendererState`] cleanly. Safe to call
    /// multiple times — subsequent calls are a no-op.
    fn shutdown_explicit(&self) {
        let mut lock = self.inner.renderer.lock().unwrap_or_else(|p| {
            tracing::warn!(site = "DevRenderSession", "mutex poisoned, recovered");
            p.into_inner()
        });
        if let Some(state) = lock.take() {
            let _ = shutdown(state);
        }
    }

    /// Discover content files CREATED after dev-server boot (issue #659).
    ///
    /// Thin gate over [`Self::refresh_bundle_and_routes`] — see there for
    /// what the refresh does. Kept as a named entry point so the
    /// discovery hook's intent stays explicit at the call site.
    ///
    /// Returns the changed source [`PageId`]s and the relative output paths
    /// that vanished from the global live route set. The caller is
    /// responsible for joining the relative paths with the appropriate dist
    /// root before propagating them (issue #804 P2).
    ///
    /// A Phase-B skip during discovery is folded back into the empty
    /// change/vanish tuple here (pre-#956 behaviour): the discovery hook
    /// still reports `renderer_reloaded = true`, so the tick behaves as a
    /// completed refresh (issue #940 Inv 3).
    #[cfg(feature = "embed_v8")]
    fn discover_created(
        &self,
        created: &[PathBuf],
    ) -> Result<(Vec<PageId>, Vec<std::path::PathBuf>)> {
        if created.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        match self.refresh_bundle_and_routes()? {
            BundleRefresh::Skipped => Ok((Vec::new(), Vec::new())),
            BundleRefresh::Refreshed { changed, vanished } => Ok((changed, vanished)),
        }
    }

    /// Re-bundle the SSR worker, swap in a freshly-started embedded V8
    /// host, and rebuild the route tables — the dev server's "make the
    /// running renderer see the source tree as it is NOW" primitive.
    ///
    /// The collection snapshot and every page module are baked into the
    /// SSR bundle, and `routes_by_source` is frozen, at whatever point
    /// this was last run (boot, or a previous refresh). Two paths call
    /// it:
    ///
    /// - the watch-ADD discovery hook (issue #659) — a content file like
    ///   `content/blog/foo.mdx` CREATED after boot is invisible to the
    ///   running host (404 until restart) without a refresh;
    /// - the per-tick `BuildContext::reload_renderer` — an in-place EDIT
    ///   (`ChangeKind::Modified`) re-renders pages, but against the
    ///   stale bundle the render output is byte-identical to the boot
    ///   render, so saves from in-place-writing editors (VS Code et al.)
    ///   never showed up. Rename-replace saves only worked by accident,
    ///   via the Created path above.
    ///
    /// Steps:
    /// 1. re-scan the router (cheap; picks up brand-new `pages/` files),
    /// 2. re-bundle with a fresh content snapshot from disk,
    /// 3. START a new embedded V8 host against the new bundle, swap it
    ///    into the SAME `Arc<Mutex<…>>` the render callback and SSR
    ///    adapter hold, and shut the old host down AFTER the swap.
    ///    Start-before-swap is deliberate: the old take→reload→put
    ///    order consumed the previous state up front, so a host-start
    ///    failure left the slot `None` and every later render broke
    ///    until restart. Acceptable when only rare watch-ADDs hit the
    ///    path; fatal now that every edit tick does. (Bundle errors —
    ///    the common failure, e.g. saving a syntax error — abort in
    ///    step 2 before the renderer is touched either way.)
    /// 4. re-expand `paths()` through the new host and swap the rebuilt
    ///    route tables in under the route `RwLock`. Rebuilding on EDIT
    ///    ticks too (not just Created) means frontmatter edits that
    ///    change a dynamic route's `paths()` output surface without a
    ///    restart.
    ///
    /// Returns [`BundleRefresh::Skipped`] when the Phase-B check (issue
    /// #940) proved nothing observable changed, otherwise
    /// [`BundleRefresh::Refreshed`] with:
    /// - `changed`: source [`PageId`]s whose route set changed
    ///   (empty for a plain content edit).
    /// - `vanished`: relative output paths (under dist) that
    ///   existed in the old live route set but are absent from the new one,
    ///   globally across all sources. Used by the caller to prune stale
    ///   HTML files and invalidate PageCache entries (issue #804).
    ///
    /// `embed_v8`-gated like `boot_dev_renderer` — the host start +
    /// `paths()` runtime eval need the embedded V8 host.
    #[cfg(feature = "embed_v8")]
    fn refresh_bundle_and_routes(&self) -> Result<BundleRefresh> {
        let project_root = &self.inner.project_root;
        let inputs = &self.inner.rebuild_inputs;

        // `ZFB_DEV_TIMING=1` — per-tick phase timing (issue #991).
        // Checked once here; all Instant::now() calls inside are skipped
        // when the flag is unset (zero overhead on the hot path).
        let timing_enabled = dev_timing_enabled();
        let tick_start = if timing_enabled {
            Some(std::time::Instant::now())
        } else {
            None
        };

        // P0 — router re-scan + route-universe plan.
        let p0_start = tick_start.map(|_| std::time::Instant::now());
        let pages_dir = inputs
            .empty_user_pages_root()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| project_root.join("pages"));
        // Re-scan the router. `Router::scan` is unchanged by adding a
        // CONTENT file (the dynamic `[slug].tsx` source is the same), but
        // re-running it is cheap and keeps boot and rebuild symmetrical —
        // and it correctly picks up a brand-new `.tsx`/`.md` page placed
        // directly under `pages/` too.
        let router = zfb_router::Router::scan(&pages_dir).map_err(anyhow::Error::from)?;
        let plan = build_route_universe(router.routes());
        let p0_ms = p0_start.map(|t| t.elapsed().as_millis());

        // P1 — re-bundle with a fresh content snapshot (reads every
        //    configured collection from disk, so created AND edited
        //    entries are in the snapshot).
        //    Sub-phases: snapshot (content walk), assemble (BundlerInput
        //    construction incl. embedded esbuild extraction), bundle/esbuild
        //    (subprocess or embedded runner).
        let p1_snapshot_start = tick_start.map(|_| std::time::Instant::now());
        let bundle_result = {
            // #993 — the persistent shadow session lock is scoped to the
            // P1 bundle step only: it is released before the P2 renderer-
            // mutex work below, so it can never overlap the renderer
            // mutex (SSR latency unaffected). On a bundle error the `?`
            // drops the guard and propagates above the swap — the
            // previous renderer keeps serving and the session's dirty
            // flag (set inside bundle_with_session) forces a from-scratch
            // materialise next tick.
            let mut session_guard = self.inner.shadow_session.lock().unwrap_or_else(|p| {
                tracing::warn!(
                    site = "refresh_bundle_and_routes",
                    "shadow session mutex poisoned, recovered"
                );
                p.into_inner()
            });
            assemble_and_bundle_dev(
                project_root,
                &inputs.cfg,
                inputs.plugin_alias_entries.clone(),
                inputs.plugin_virtual_modules.clone(),
                timing_enabled,
                session_guard.as_mut(),
                inputs.esbuild_path(),
                inputs.empty_user_pages_root(),
                // S2 (#1230) — re-include the injected modules on every tick
                // from the SAME session-lifetime staging dir (not
                // re-materialised). `None` on the parity path.
                inputs.injected_pages_root(),
            )
            .context("dev refresh: re-bundle failed")?
        };
        let p1_total_ms = p1_snapshot_start.map(|t| t.elapsed().as_millis());
        // Sub-phase split is reported by assemble_and_bundle_dev into the
        // BundleSubTiming fields when timing_enabled.
        let p1_snapshot_ms = bundle_result.sub_timing.as_ref().map(|t| t.snapshot_ms);
        let p1_assemble_ms = bundle_result.sub_timing.as_ref().map(|t| t.assemble_ms);
        let p1_bundle_ms = bundle_result.sub_timing.as_ref().map(|t| t.bundle_ms);
        let mut bundler_out = bundle_result.output;

        // #1284/#1287 — populate per-route `DepKind::Module` edges from the
        // bundle's metafile so a component edit (direct or transitive, incl. a
        // symlinked workspace `.tsx`) maps to its consuming route via
        // `dirty_pages`. Done on EVERY refresh (before the skip-key early
        // return below is irrelevant — the edges only change when the bundle
        // changes, and a byte-identical bundle re-asserts identical edges, a
        // cheap idempotent upsert). No-op until the graph handle is installed.
        self.populate_module_edges(&bundler_out.route_module_deps);

        // P1b — skip-key compute (SHA-256 over bundle + router + static HTML).
        // Phase B (issue #940) — skip key check.
        //
        // Compute a digest over bundle bytes + the router-scan signature
        // (sorted source paths + route templates) + static pages/**.html
        // bodies (issue #956 gate (a)). If the new key matches the previous
        // SUCCESSFUL tick's key, nothing observable changed:
        // - bundle bytes ≡ snapshot unchanged ≡ V8 host would observe
        //   identical globals,
        // - router signature unchanged ≡ pages/ universe identical,
        // - static-HTML bodies unchanged ≡ verbatim-copied routes identical.
        // Return `Skipped` — the discovery caller treats this as a
        // completed refresh (Inv 3: renderer_reloaded=true via the
        // DiscoveryOutcome path above us), while the per-tick reloader
        // propagates it so DevAssetPipeline bypasses the render fan-out
        // (issue #956).
        //
        // Key is stored only AFTER the full success path below (host swap +
        // route rebuild). A failed refresh — e.g. host start error — must
        // NOT update last_successful_skip_key so the next byte-identical
        // tick retries in full (Correctness Req 1, issue #940).
        let p1b_start = tick_start.map(|_| std::time::Instant::now());
        let new_skip_key = compute_bundle_skip_key(&bundler_out, router.routes());
        let p1b_ms = p1b_start.map(|t| t.elapsed().as_millis());
        if self.inner.should_skip_refresh(new_skip_key) {
            tracing::debug!(
                site = "refresh_bundle_and_routes",
                "bundle skip: byte-identical bundle + unchanged route universe; \
                 skipping V8 host boot and paths() re-expansion"
            );
            // Early return — the live host and route tables were left
            // untouched. The skip key is already stored from the last
            // successful tick; no update needed.
            return Ok(BundleRefresh::Skipped);
        }

        // Keep the skip key over esbuild's real output. The dev-only wrapper
        // carries a fresh private trace nonce, so hashing it would turn every
        // otherwise-identical refresh into a false miss.
        let trace_token = wrap_dev_bundle_with_content_trace(&mut bundler_out, router.routes())
            .context("dev refresh: install content-provenance worker wrapper failed")?;

        // P2 — V8 host boot, mutex swap, old-host shutdown (three separate
        //      sub-timers: boot vs eval vs teardown; split matters for the
        //      host-reuse decision).
        // 2. Start a NEW embedded V8 host against the rebuilt bundle,
        //    swap it into the existing mutex (the render callback + SSR
        //    adapter share this exact Arc), and shut the old host down
        //    only after the swap — a host-start failure must leave the
        //    previous renderer serving (see the method docs).
        let p2_boot_start = tick_start.map(|_| std::time::Instant::now());
        let started = start(RendererStartInput {
            bundle_path: bundler_out.bundle_path.clone(),
            sourcemap_path: bundler_out.sourcemap_path.clone(),
            backend: Backend::EmbeddedV8 {
                host_factory: crate::v8_host_adapter::make_v8_host_factory_with_hooks(
                    inputs.v8_plugin_hooks.clone(),
                ),
            },
            request_timeout: None,
        })
        .map_err(anyhow::Error::from)
        .context("dev refresh: renderer start failed (previous renderer kept serving)")?;
        let p2_boot_ms = p2_boot_start.map(|t| t.elapsed().as_millis());

        let p2_swap_start = tick_start.map(|_| std::time::Instant::now());
        let previous = {
            let mut lock = self.inner.renderer.lock().unwrap_or_else(|p| {
                tracing::warn!(
                    site = "refresh_bundle_and_routes",
                    "renderer mutex poisoned, recovered"
                );
                p.into_inner()
            });
            lock.replace(started)
        };
        // The new worker is live before its route table is rebuilt. Switch the
        // trace token now so the ensuing `/__paths__/` calls are attributed to
        // this host; prior in-memory observations remain conservative until
        // the new worker has re-observed their routes.
        self.begin_content_trace(trace_token);
        let p2_swap_ms = p2_swap_start.map(|t| t.elapsed().as_millis());

        let p2_shutdown_start = tick_start.map(|_| std::time::Instant::now());
        if let Some(prev) = previous {
            if let Err(err) = shutdown(prev) {
                tracing::warn!(
                    site = "refresh_bundle_and_routes",
                    error = %err,
                    "old renderer shutdown failed; continuing with new host"
                );
            }
        }
        let p2_shutdown_ms = p2_shutdown_start.map(|t| t.elapsed().as_millis());

        // Phase B (issue #940, review fix) — the live renderer just
        // diverged from whatever the stored skip key described. Invalidate
        // it NOW, before the fallible route-table rebuild below: if that
        // rebuild fails, the error `?`-returns without committing, and a
        // stale key from the pre-swap bundle could otherwise match a later
        // tick (e.g. the user undoing the edit) and skip — freezing the
        // failed tick's host in place. Note this is distinct from a
        // host-START failure, which `?`-returns ABOVE the swap and
        // correctly keeps the previous key (the live renderer was never
        // touched, so the old key still describes it).
        self.inner.commit_skip_key(None);

        // P3 — route-table rebuild (re-expands `paths()` through the new
        //      V8 host) with paths()-expansion sub-timing and this tick's
        //      PathsCache hit/miss deltas. The cache persists across
        //      ticks (#994 item B) — note the lookup happens AFTER the
        //      V8 dispatch (keyed on its result), so hits save only the
        //      Rust-side validate/URL-build, not the V8 evals.
        // 3. Rebuild the route tables through the reloaded host (re-expands
        //    `paths()`, so the dynamic source now resolves the new URL).
        let p3_start = tick_start.map(|_| std::time::Instant::now());
        let (
            mut new_routes_by_source,
            new_ssr_routes,
            mut new_url_index,
            p3_cache_hits,
            p3_cache_misses,
        ) = {
            let mut paths_cache = self.inner.paths_cache.lock().unwrap_or_else(|p| {
                tracing::warn!(
                    site = "refresh_bundle_and_routes",
                    "paths cache mutex poisoned, recovered"
                );
                p.into_inner()
            });
            build_dev_route_tables_timed(
                &router,
                &plan,
                project_root,
                &self.inner.renderer,
                &mut paths_cache,
            )
            .context("dev refresh: route-table rebuild failed")?
        };
        // S3 (#1231) — the router scan rebuilds `routes_by_source` from the
        // conventional real `pages/` root, or #1518's private empty root; it
        // never walks the staged injected modules
        // (they live outside `pages/`, node_modules-natured). Re-merge the
        // static injected-route seeds into the freshly-built tables and
        // rebuild `url_index` so the seeded URLs keep resolving across the
        // swap. The seed source key is stable
        // ([`injected_source_key`]), so [`diff_route_tables`] below sees an
        // identical entry set on every tick — no spurious `changed`/
        // `vanished`. No-op on the parity path (no seeds).
        if !self.inner.injected_static_seeds.is_empty() {
            seed_injected_static_routes(
                &mut new_routes_by_source,
                &self.inner.injected_static_seeds,
            );
            new_url_index = build_url_index(&new_routes_by_source);
        }
        let p3_ms = p3_start.map(|t| t.elapsed().as_millis());

        // P4 — diff + RwLock table swap + skip-key commit.
        // 4. Diff against the frozen table — see [`diff_route_tables`]
        //    for the exact semantics (issue #958: the `changed` set is
        //    the G5 narrowing gate, so it compares full entry SETS, not
        //    just counts).
        let p4_start = tick_start.map(|_| std::time::Instant::now());
        let (changed, vanished_output_paths) = {
            let old = self.inner.routes.read().unwrap_or_else(|p| p.into_inner());
            diff_route_tables(&old.routes_by_source, &new_routes_by_source)
        };
        {
            let mut tables = self.inner.routes.write().unwrap_or_else(|p| p.into_inner());
            tables.routes_by_source = new_routes_by_source;
            tables.ssr_routes = new_ssr_routes;
            // Swap the url_index atomically with the other tables (issue #1019).
            tables.url_index = new_url_index;
        }
        // Issue #1025 — the route tables just moved: advance the stale
        // tick generation and evict stale entries whose output routes
        // vanished from the live route set (#804). Runs on EVERY full
        // refresh — including all-lazy ticks with zero eager renders —
        // and never on a Phase-B skip (early return above, #956).
        self.inner.note_table_swap(&vanished_output_paths);
        // S3 (#1231) — re-stale the static injected-route seeds AFTER the
        // swap's generation bump (sharp edge 5). The seed stays in
        // `routes_by_source` across the swap, so the staleness map is the
        // ONLY thing that makes a content edit refresh an already-rendered
        // injected page: without this re-stale the route would stay present
        // but never be re-claimed. The bumped generation defeats the
        // claim/clear ABA race exactly as the eager-route stale-marking does.
        self.mark_injected_seeds_stale();
        // S5 (#1233 / #1227 item (h)) — re-stale every previously-rendered
        // DYNAMIC injected output too. Static seeds live in `routes_by_source`
        // and are handled above; dynamic injected routes have no concrete
        // entry there (their `output_path` is neither a seed nor a member of
        // the diffed tables), so `mark_injected_seeds_stale` + the
        // vanished-eviction in `note_table_swap` both miss them. Without this
        // call a content edit would leave an already-rendered dynamic injected
        // page fresh forever, serving stale content (the #1234 confirm-gap).
        self.inner.restale_dynamic_injected();
        let p4_ms = p4_start.map(|t| t.elapsed().as_millis());

        // Phase B (issue #940) — commit-after-success.
        //
        // Store the skip key only here, after ALL of: host boot, swap, and
        // route-table rebuild have succeeded.  Any failure above returns via
        // `?` without reaching this point, leaving last_successful_skip_key
        // at its previous value so the next byte-identical tick rebuilds
        // fully (Correctness Req 1). See `commit_skip_key` for the
        // `None`-clears-the-key rationale.
        self.inner.commit_skip_key(new_skip_key);

        // Print one stderr line per tick when ZFB_DEV_TIMING=1.
        if let Some(tick_elapsed) = tick_start.map(|t| t.elapsed().as_millis()) {
            let p0 = p0_ms.unwrap_or(0);
            let p1 = p1_total_ms.unwrap_or(0);
            let p1_snap = p1_snapshot_ms.unwrap_or(0);
            let p1_asm = p1_assemble_ms.unwrap_or(0);
            let p1_bnd = p1_bundle_ms.unwrap_or(0);
            let p1b = p1b_ms.unwrap_or(0);
            let p2b = p2_boot_ms.unwrap_or(0);
            let p2s = p2_swap_ms.unwrap_or(0);
            let p2d = p2_shutdown_ms.unwrap_or(0);
            let p3 = p3_ms.unwrap_or(0);
            let p4 = p4_ms.unwrap_or(0);
            eprintln!(
                "[zfb-timing] tick={tick_elapsed}ms \
                 P0(router)={p0}ms \
                 P1(bundle)={p1}ms[snap={p1_snap}ms,asm={p1_asm}ms,esbuild={p1_bnd}ms] \
                 P1b(skip-key)={p1b}ms \
                 P2(host)[boot={p2b}ms,swap={p2s}ms,shutdown={p2d}ms] \
                 P3(routes)={p3}ms[cache-hits={p3_cache_hits},miss={p3_cache_misses}] \
                 P4(diff+swap)={p4}ms"
            );
        }

        Ok(BundleRefresh::Refreshed {
            changed,
            vanished: vanished_output_paths,
        })
    }
}

/// Result of one [`DevRenderSession::refresh_bundle_and_routes`] call —
/// the dev session's internal counterpart of
/// [`zfb_build::RefreshOutcome`] (issue #956), carrying the extra
/// `changed` source set the discovery path needs.
#[cfg(feature = "embed_v8")]
enum BundleRefresh {
    /// Phase-B skip (issue #940): byte-identical bundle + unchanged route
    /// universe (including static `pages/**.html` bodies). The live V8
    /// host and route tables were left untouched.
    Skipped,
    /// Full refresh completed (host swap + route-table rebuild).
    Refreshed {
        /// Source [`PageId`]s whose route-entry set changed (see
        /// [`diff_route_tables`]).
        changed: Vec<PageId>,
        /// Relative output paths (under dist) that vanished from the
        /// global live route set.
        vanished: Vec<std::path::PathBuf>,
    },
}

/// Diff a freshly-rebuilt `routes_by_source` map against the frozen one:
///
/// - `changed`: sources whose route-entry SET changed — new sources, and
///   sources whose entries differ in any way (URL, output path, params).
///   NOT a count-only comparison: issue #958's narrowing gate (fallback
///   G5) rides on this set, and a `paths()` refresh can replace routes
///   without changing their count (e.g. a two-page URL swap), which a
///   count diff reports as unchanged — a narrowed render would then skip
///   the brand-new route (silent under-render; review finding on #958).
///   The full-set comparison also feeds the watch-ADD discovery path,
///   where firing more often only re-renders more (safe direction).
/// - `vanished`: output paths that were live before but are absent from
///   every source in the new table. The diff is GLOBAL on purpose: if
///   route A loses /x while route B simultaneously gains /x, /x must NOT
///   be considered vanished (#727 two-page swap).
#[cfg(feature = "embed_v8")]
fn diff_route_tables(
    old: &HashMap<PathBuf, Vec<DevRouteEntry>>,
    new: &HashMap<PathBuf, Vec<DevRouteEntry>>,
) -> (Vec<PageId>, Vec<std::path::PathBuf>) {
    let changed: Vec<PageId> = new
        .iter()
        .filter(|(src, entries)| old.get(*src).map(|prev| prev != *entries).unwrap_or(true))
        .map(|(src, _)| PageId::new(src.clone()))
        .collect();

    // Collect the globally-live output_path sets for old and new. Use
    // HashSet for O(1) membership checks.
    let old_live: HashSet<&PathBuf> = old
        .values()
        .flat_map(|entries| entries.iter().map(|e| &e.entry.output_path))
        .collect();
    let new_live: HashSet<&PathBuf> = new
        .values()
        .flat_map(|entries| entries.iter().map(|e| &e.entry.output_path))
        .collect();
    let vanished: Vec<std::path::PathBuf> = old_live
        .difference(&new_live)
        .map(|p| (*p).clone())
        .collect();

    (changed, vanished)
}

impl Drop for DevRenderInner {
    fn drop(&mut self) {
        // Defence-in-depth: even if `shutdown_explicit` was missed
        // (panicking dev loop, early ?-return), the inner Arc's drop
        // tears the subprocess down. Errors from shutdown are
        // swallowed here — the process is already going away.
        if let Ok(mut g) = self.renderer.lock() {
            if let Some(state) = g.take() {
                let _ = shutdown(state);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Dev-only content-read tracing (issue #1600)
// ---------------------------------------------------------------------------

/// Private worker endpoint used only by the Rust dev session to drain actual
/// `getCollection()` reads from the current embedded worker.
///
/// The generated wrapper rejects requests without the per-bundle nonce, so a
/// normal browser request cannot read or alter the trace state.
#[cfg(feature = "embed_v8")]
const DEV_CONTENT_TRACE_ENDPOINT: &str = "/__zfb_internal/content-provenance";
#[cfg(feature = "embed_v8")]
const DEV_CONTENT_TRACE_HEADER: &str = "x-zfb-content-provenance-token";

/// Each generated wrapper receives a fresh nonce. It is not a public API or
/// an authentication mechanism; it prevents an ordinary dev-server request
/// from reaching the trace drain endpoint.
#[cfg(feature = "embed_v8")]
static DEV_CONTENT_TRACE_NONCE: AtomicU64 = AtomicU64::new(0);

/// One route descriptor embedded into the private tracing wrapper.
///
/// `source` is deliberately project-relative: it is emitted from the router
/// scan and validated against the live route table before it becomes a graph
/// consumer. That keeps worker-side strings from becoming graph authority.
#[cfg(feature = "embed_v8")]
#[derive(Debug, Clone, serde::Serialize)]
struct DevContentTraceRoute {
    template: String,
    source: String,
    specificity: u32,
}

/// One route visit or tracked read emitted by the generated worker wrapper.
#[cfg(feature = "embed_v8")]
#[derive(Debug, Clone, serde::Deserialize)]
struct DevContentTraceEvent {
    source: String,
    #[serde(default)]
    collection: Option<String>,
    phase: DevContentTracePhase,
    #[serde(default)]
    kind: DevContentTraceEventKind,
}

/// Whether a trace event marks a route as having run, or records one
/// `getCollection()` property access during that route's execution.
#[cfg(feature = "embed_v8")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum DevContentTraceEventKind {
    Visit,
    Read,
}

#[cfg(feature = "embed_v8")]
impl Default for DevContentTraceEventKind {
    fn default() -> Self {
        Self::Read
    }
}

/// The actual worker seam that performed the read.
#[cfg(feature = "embed_v8")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum DevContentTracePhase {
    /// The synthetic `/__paths__/…` evaluation of a dynamic route.
    Paths,
    /// A normal route render (including static `getStaticProps` / layout
    /// reads). Those are always aggregate collection readers.
    Render,
}

/// One independent observation slot in the current worker. A dynamic page can
/// call `getCollection()` while evaluating `paths()` yet make no collection
/// read during its ordinary render; those two executions must not erase one
/// another's provenance.
#[cfg(feature = "embed_v8")]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DevContentTraceObservation {
    consumer: PageId,
    phase: DevContentTracePhase,
}

/// JSON payload returned by [`DEV_CONTENT_TRACE_ENDPOINT`].
#[cfg(feature = "embed_v8")]
#[derive(Debug, serde::Deserialize)]
struct DevContentTracePayload {
    ready: bool,
    #[serde(default)]
    events: Vec<DevContentTraceEvent>,
    #[serde(default)]
    error: Option<String>,
}

/// Provenance observations retained by the dev session.
///
/// A cold boot always clears this state after restoring the persisted graph,
/// then repopulates it from the live worker. Later host swaps retain prior
/// observations only until the corresponding route phase is visited by the
/// new worker. A visit with no tracked read removes that phase's old
/// provenance; an unvisited phase remains conservative until the new worker
/// can establish its current behavior.
#[cfg(feature = "embed_v8")]
#[derive(Default)]
struct DevContentTraceState {
    token: Option<String>,
    reads_by_observation: BTreeMap<DevContentTraceObservation, Vec<TrackedContentRead>>,
    boot_complete: bool,
}

/// Generate a fresh opaque nonce for the worker trace-drain endpoint.
#[cfg(feature = "embed_v8")]
fn make_dev_content_trace_token(bundle_path: &Path) -> String {
    let serial = DEV_CONTENT_TRACE_NONCE.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(bundle_path.to_string_lossy().as_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(serial.to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    format!("{:x}", hasher.finalize())
}

/// Wrap a dev worker bundle with a private, transparent `getCollection()`
/// observer.
///
/// The runtime content package already resolves reads through
/// `globalThis.__zfb.contentSnapshot.collections`. The embedded V8 host only
/// loads its configured source string, so the observer is appended to that
/// self-contained bundle rather than importing it from a sidecar module. For
/// ordinary requests it returns the inner worker response unchanged. The
/// wrapper is dev-only and exists solely to feed the dependency graph with
/// observations made by the running worker.
#[cfg(feature = "embed_v8")]
fn wrap_dev_bundle_with_content_trace(
    output: &mut BundlerOutput,
    routes: &[zfb_router::Route],
) -> Result<String> {
    let bundle_path = output.bundle_path.clone();
    let original_source = std::fs::read_to_string(&bundle_path)
        .with_context(|| format!("reading dev worker bundle {}", bundle_path.display()))?;
    let (mut source, inner_worker_binding) = rewrite_dev_bundle_default_export(&original_source)
        .with_context(|| {
            format!(
                "rewriting default export in dev worker bundle {}",
                bundle_path.display()
            )
        })?;
    let token = make_dev_content_trace_token(&bundle_path);
    let descriptors: Vec<DevContentTraceRoute> = routes
        .iter()
        .filter(|route| !route.static_html)
        .map(|route| DevContentTraceRoute {
            template: route.template(),
            source: route.source_path.to_string_lossy().into_owned(),
            specificity: route.specificity,
        })
        .collect();
    let routes_json = serde_json::to_string(&descriptors)
        .context("serializing dev content-trace route descriptors")?;
    let token_json =
        serde_json::to_string(&token).context("serializing dev content-trace nonce")?;
    let endpoint_json = serde_json::to_string(DEV_CONTENT_TRACE_ENDPOINT)
        .context("serializing dev content-trace endpoint")?;
    let header_json = serde_json::to_string(DEV_CONTENT_TRACE_HEADER)
        .context("serializing dev content-trace header")?;

    source.push_str(&format!(
        r#"
// Generated by zfb dev. Private content-provenance observer.
const __zfb_inner_worker = {inner_worker_binding};
const __zfb_trace_routes = {routes_json};
const __zfb_trace_token = {token_json};
const __zfb_trace_endpoint = {endpoint_json};
const __zfb_trace_header = {header_json};
const __zfb_trace_state = {{ ready: false, events: [], current: undefined, error: undefined }};

function __zfb_normalize_path(path) {{
  if (path.length > 1 && path.endsWith("/")) return path.slice(0, -1);
  return path || "/";
}}

function __zfb_matches_route(template, requestPath) {{
  const routeParts = template.split("/").filter(Boolean);
  const requestParts = requestPath.split("/").filter(Boolean);
  let requestIndex = 0;
  for (let routeIndex = 0; routeIndex < routeParts.length; routeIndex += 1) {{
    const part = routeParts[routeIndex];
    if (part.startsWith(":")) {{
      const optionalCatchall = part.endsWith("{{.+}}?");
      const catchall = optionalCatchall || part.endsWith("{{.+}}");
      if (catchall) {{
        if (!optionalCatchall && requestIndex >= requestParts.length) return false;
        return routeIndex === routeParts.length - 1;
      }}
      if (requestIndex >= requestParts.length) return false;
      requestIndex += 1;
      continue;
    }}
    if (requestParts[requestIndex] !== part) return false;
    requestIndex += 1;
  }}
  return requestIndex === requestParts.length;
}}

function __zfb_pick_render_route(pathname) {{
  const path = __zfb_normalize_path(pathname);
  const exact = __zfb_trace_routes.filter((route) => route.template === path);
  if (exact.length === 1) return exact[0];
  if (exact.length > 1) return undefined;
  const matches = __zfb_trace_routes.filter((route) => __zfb_matches_route(route.template, path));
  if (matches.length === 0) return undefined;
  const maxSpecificity = Math.max(...matches.map((route) => route.specificity));
  const winners = matches.filter((route) => route.specificity === maxSpecificity);
  return winners.length === 1 ? winners[0] : undefined;
}}

function __zfb_trace_context(request) {{
  const url = new URL(request.url);
  const pathname = __zfb_normalize_path(url.pathname);
  const pathsPrefix = "/__paths__/";
  if (pathname.startsWith(pathsPrefix)) {{
    let template;
    try {{
      template = decodeURIComponent(pathname.slice(pathsPrefix.length));
    }} catch (_) {{
      return undefined;
    }}
    const route = __zfb_trace_routes.find((candidate) => candidate.template === template);
    return route ? {{ source: route.source, phase: "paths" }} : undefined;
  }}
  const route = __zfb_pick_render_route(pathname);
  return route ? {{ source: route.source, phase: "render" }} : undefined;
}}

try {{
  const snapshot = globalThis.__zfb?.contentSnapshot;
  if (!snapshot || !snapshot.collections || typeof snapshot.collections !== "object") {{
    __zfb_trace_state.error = "content snapshot is unavailable";
  }} else {{
    const collections = snapshot.collections;
    snapshot.collections = new Proxy(collections, {{
      get(target, property, receiver) {{
        const current = __zfb_trace_state.current;
        if (current && typeof property === "string") {{
          __zfb_trace_state.events.push({{
            source: current.source,
            collection: property,
            phase: current.phase,
            kind: "read",
          }});
        }}
        return Reflect.get(target, property, receiver);
      }},
    }});
    __zfb_trace_state.ready = true;
  }}
}} catch (error) {{
  __zfb_trace_state.error = String(error);
}}

export default {{
  async fetch(request) {{
    const pathname = new URL(request.url).pathname;
    if (pathname === __zfb_trace_endpoint) {{
      if (request.headers.get(__zfb_trace_header) !== __zfb_trace_token) {{
        return new Response("Not Found", {{ status: 404 }});
      }}
      const events = __zfb_trace_state.events.splice(0);
      return new Response(JSON.stringify({{
        ready: __zfb_trace_state.ready,
        events,
        error: __zfb_trace_state.error,
      }}), {{
        status: 200,
        headers: {{ "Content-Type": "application/json; charset=utf-8" }},
      }});
    }}

    const current = __zfb_trace_context(request);
    if (!current) return __zfb_inner_worker.fetch(request);
    const previous = __zfb_trace_state.current;
    __zfb_trace_state.current = current;
    try {{
      const response = await __zfb_inner_worker.fetch(request);
      __zfb_trace_state.events.push({{
        source: current.source,
        phase: current.phase,
        kind: "visit",
      }});
      return response;
    }} finally {{
      __zfb_trace_state.current = previous;
    }}
  }},
}};
"#,
    ));

    std::fs::write(&bundle_path, source).with_context(|| {
        format!(
            "writing self-contained dev content-trace bundle {}",
            bundle_path.display()
        )
    })?;
    Ok(token)
}

/// Replace the final Workers default export so a dev-only wrapper can expose
/// it under a private local binding in the same ESM source string.
///
/// esbuild normally emits `export { worker as default }`, while unbundled
/// test seams can retain `export default ...`; supporting both keeps this
/// boundary independent of an emitter implementation detail.
#[cfg(feature = "embed_v8")]
fn rewrite_dev_bundle_default_export(source: &str) -> Result<(String, String)> {
    const PRIVATE_EXPORT: &str = "__zfb_content_trace_inner_default";

    if let Some(export_start) = source.rfind("export default") {
        let export_end = export_start + "export default".len();
        let mut rewritten = String::with_capacity(source.len() + PRIVATE_EXPORT.len());
        rewritten.push_str(&source[..export_start]);
        rewritten.push_str("const ");
        rewritten.push_str(PRIVATE_EXPORT);
        rewritten.push_str(" =");
        rewritten.push_str(&source[export_end..]);
        return Ok((rewritten, PRIVATE_EXPORT.to_string()));
    }

    const DEFAULT_ALIAS: &str = " as default";
    let alias_start = source
        .rfind(DEFAULT_ALIAS)
        .ok_or_else(|| anyhow::anyhow!("worker bundle has no default export"))?;
    let alias_end = alias_start + DEFAULT_ALIAS.len();
    let export_start = source[..alias_start]
        .rfind("export {")
        .ok_or_else(|| anyhow::anyhow!("default export is not an ESM export list"))?;
    if source[export_start..alias_start].contains('}') {
        anyhow::bail!("default export is outside its ESM export list");
    }
    match source[alias_end..].trim_start().chars().next() {
        Some(',' | '}') => {}
        _ => anyhow::bail!("default export alias has an unsupported ESM shape"),
    }

    let binding_end = alias_start;
    let mut binding_start = binding_end;
    let bytes = source.as_bytes();
    while binding_start > 0
        && matches!(
            bytes[binding_start - 1],
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$'
        )
    {
        binding_start -= 1;
    }
    if binding_start == binding_end {
        anyhow::bail!("default export alias has no local worker binding");
    }

    let binding = source[binding_start..binding_end].to_string();
    let mut rewritten = String::with_capacity(source.len() + PRIVATE_EXPORT.len());
    rewritten.push_str(&source[..alias_start]);
    rewritten.push_str(" as ");
    rewritten.push_str(PRIVATE_EXPORT);
    rewritten.push_str(&source[alias_end..]);
    Ok((rewritten, binding))
}

// ---------------------------------------------------------------------------
// ZFB_DEV_TIMING — per-tick phase timing (issue #991)
// ---------------------------------------------------------------------------

/// Read `ZFB_DEV_TIMING` and decide whether to emit per-tick timing lines.
/// Truthy values: `1`, `true` (case-insensitive). Everything else — including
/// unset, empty, and unrecognized values — is off, so the hot path has zero
/// overhead.
#[cfg(feature = "embed_v8")]
pub(crate) fn dev_timing_enabled() -> bool {
    std::env::var("ZFB_DEV_TIMING")
        .ok()
        .as_deref()
        .map(|raw| {
            let t = raw.trim();
            t.eq_ignore_ascii_case("1") || t.eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
}

/// Per-P1-sub-phase wall-clock durations collected inside
/// [`assemble_and_bundle_dev`] when `ZFB_DEV_TIMING=1`. Only populated when
/// `timing_enabled` is `true`; the caller always has `Some(...)` in that
/// case.
#[cfg(feature = "embed_v8")]
struct BundleSubTiming {
    /// `build_content_snapshot_json` — full content-collection walk.
    snapshot_ms: u128,
    /// `assemble_bundler_input` — BundlerInput construction incl. embedded
    /// esbuild/node_modules tempdir extraction.
    assemble_ms: u128,
    /// `bundle()` — esbuild subprocess / embedded runner.
    bundle_ms: u128,
}

/// Return value of [`assemble_and_bundle_dev`].
///
/// Wraps the `BundlerOutput` with an optional timing record that is `Some`
/// when `ZFB_DEV_TIMING=1` and `None` otherwise (zero allocation on the hot
/// path).
#[cfg(feature = "embed_v8")]
struct AssembledBundleResult {
    output: BundlerOutput,
    sub_timing: Option<BundleSubTiming>,
}

/// Assemble the dev-mode bundler input and run the bundler, returning the
/// fresh [`BundlerOutput`] wrapped in [`AssembledBundleResult`] (issue #659
/// — extracted from `boot_dev_renderer` so the watch-ADD re-bundle reuses
/// the EXACT same configuration the boot bundle used; any drift here would
/// make a newly-added page render differently in dev than it did at boot).
/// The embedded node_modules tempdir handle lives only for the
/// synchronous `bundle()` call (which writes `bundle_path` to disk), so
/// scoping it to this function is correct. The esbuild binary is the
/// boot-time process-lifetime extraction passed via `pre_resolved_esbuild`
/// (#994 item A); the per-call extraction only runs as a fallback when
/// that is `None`.
///
/// `recompute snapshot` is implicit: `build_content_snapshot_json` re-reads
/// the content collections from disk on every call, so a re-bundle here
/// picks up a content file created after boot.
///
/// `timing_enabled` — when `true`, record sub-phase wall-clock durations
/// into the returned [`BundleSubTiming`]. When `false`, no `Instant::now()`
/// calls are made (zero overhead on the hot path).
#[cfg(feature = "embed_v8")]
#[allow(clippy::too_many_arguments)] // 9 params: #1518 added empty_user_pages_root; these mirror assemble_bundler_input's threaded inputs, a struct would just shuffle the same fields
fn assemble_and_bundle_dev(
    project_root: &Path,
    cfg: &config::Config,
    plugin_alias_entries: Vec<(String, String)>,
    plugin_virtual_modules: Vec<(String, String)>,
    timing_enabled: bool,
    // Persistent dev shadow-tree session (issue #993). Both the boot
    // bundle and every refresh pass the SAME session through this seam,
    // so boot and refresh share one assembly path AND one shadow tree —
    // #659 configuration parity by construction.
    shadow_session: Option<&mut ShadowSession>,
    // Boot-time extracted embedded esbuild binary (#994 item A) — see
    // [`DevRebuildInputs::esbuild`]. Both boot and refresh pass the same
    // path so every tick skips the per-call tempdir extraction.
    pre_resolved_esbuild: Option<&Path>,
    // #1518 — a true zero-pages consumer has no real project-root `pages/`
    // directory. When surviving injected routes make it runnable, boot creates
    // a session-lifetime EMPTY internal root and passes it through this
    // replacement seam so the bundler's primary pages walk remains valid.
    // `None` preserves the conventional `project_root/pages` path.
    empty_user_pages_root: Option<&Path>,
    // S2 (#1230) — the session-lifetime injected-route staging `pages` root
    // (see [`DevRebuildInputs::injected_pages_root`]). `Some(root)` points
    // the dev bundler at the staged injected-only modules via the existing
    // `assemble_bundler_input` `build_pages_root` seam, so the injected
    // entrypoints + their `virtual:` imports land in the dev bundle. `None`
    // (no injected routes / all shadowed) keeps the default `pages/` root —
    // byte-identical to today (sharp edge 8). Both boot and refresh pass the
    // SAME root from `DevRebuildInputs`, so every tick re-includes the
    // injected modules without re-materialising them.
    injected_pages_root: Option<&Path>,
) -> Result<AssembledBundleResult> {
    // Embed the content snapshot so a page's `getStaticProps()` (and any
    // runtime `paths()`) sees the same collection data the production
    // build does. The published `zfb/content` `getCollection(...)` reads
    // `globalThis.__zfb.contentSnapshot`, which `createPageRouter`
    // installs from this JSON at worker boot. Without it, dev resolves
    // against the bundler's placeholder empty snapshot and every
    // `getCollection(...)` returns `[]` — the symptom in #493 where the
    // scaffolded homepage rendered an empty post list under `zfb dev`
    // even though `zfb build` produced the full list. Built via the same
    // `build_content_snapshot_json` helper `zfb build` uses, so dev and
    // build stay byte-identical (returns `None` when no collections are
    // declared, matching the previous behaviour for collection-less
    // projects).
    let snap_start = if timing_enabled {
        Some(std::time::Instant::now())
    } else {
        None
    };
    let content_snapshot_json =
        crate::commands::build::build_content_snapshot_json(project_root, cfg);
    let snapshot_ms = snap_start.map(|t| t.elapsed().as_millis()).unwrap_or(0);

    // The full ~25-field BundlerInput assembly is shared with `zfb build`
    // via `commands::bundler_input::assemble_bundler_input`. The two
    // per-command differences passed here:
    //   • BundleMode::Development  (build uses Production)
    //   • CssModuleFailMode::WarnAndEmpty  (build uses HardFail)
    let asm_start = if timing_enabled {
        Some(std::time::Instant::now())
    } else {
        None
    };
    let crate::commands::bundler_input::AssembledBundlerInput {
        bundler_input,
        _node_modules_handle: _embedded_nm_handle,
        _esbuild_handle: _embedded_esbuild_handle,
    } = crate::commands::bundler_input::assemble_bundler_input(
        project_root,
        cfg,
        BundleMode::Development,
        crate::commands::bundler_input::CssModuleFailMode::WarnAndEmpty,
        content_snapshot_json,
        plugin_alias_entries,
        plugin_virtual_modules,
        pre_resolved_esbuild,
        // #1518's internal empty root is the only dev-side replacement. It
        // exists solely for a consumer that has no real user `pages/` tree;
        // conventional dev sessions keep the default project-root path.
        empty_user_pages_root,
        // S2 (#1230) — the injected-route staging root (B1 multi-root). When a
        // preset registered routes whose survivor set is non-empty, this is the
        // session-lifetime staged `pages` dir holding ONLY the synthesized
        // injected modules. It is ADDITIVE: the bundler walks the primary
        // user-pages root (the real tree, or #1518's private empty root) AND
        // this root into the same shadow tree, so conventional user pages stay
        // in the dev bundle (HMR intact) while injected entrypoints + their
        // `virtual:` imports are added. The user's real `pages/` is NOT copied
        // here — conventional dev scan + watcher identity remains untouched.
        // `None` on the parity path is byte-identical to today (sharp edge 8).
        injected_pages_root,
    )?;
    let assemble_ms = asm_start.map(|t| t.elapsed().as_millis()).unwrap_or(0);

    let bnd_start = if timing_enabled {
        Some(std::time::Instant::now())
    } else {
        None
    };
    let bundler_out: BundlerOutput =
        bundle_with_session(bundler_input, shadow_session).context("bundler step failed")?;
    let bundle_ms = bnd_start.map(|t| t.elapsed().as_millis()).unwrap_or(0);

    let sub_timing = if timing_enabled {
        Some(BundleSubTiming {
            snapshot_ms,
            assemble_ms,
            bundle_ms,
        })
    } else {
        None
    };
    Ok(AssembledBundleResult {
        output: bundler_out,
        sub_timing,
    })
}

/// Compute the Phase-B skip key for a single dev refresh tick (issue #940).
///
/// The key is a SHA-256 digest of:
///
/// 1. **Bundle bytes** — the full content of `BundlerOutput.bundle_path` on
///    disk.  The content snapshot JSON is baked into the bundle by
///    `assemble_and_bundle_dev`, so identical bytes imply the snapshot (and
///    therefore everything the V8 host and `paths()` can observe) is
///    unchanged.
///
/// 2. **Router-scan signature** — a sorted list of
///    `(source_path, template)` pairs derived from `router.routes()`.
///    This defeats any `pages/` change that leaves bundle bytes identical
///    (e.g. a `.tsx` page added with no JS-visible impact on the snapshot),
///    satisfying Inv 2 (§3 of the roadmap, issue #935).
///
/// 3. **Static `pages/**.html` bodies** (issue #956 gate (a)) — routes
///    with `static_html = true` bypass the JS bundle entirely: the
///    renderer copies the source file from disk at render time
///    (`zfb-build/src/renderer.rs`), so neither the bundle bytes nor the
///    route signature reflect an edit to such a file's CONTENT. Hashing
///    the bodies here keeps a static-HTML edit from being swallowed by
///    the skip.
///
/// Returns `None` if the bundle file or any static-HTML source cannot be
/// read — the caller treats that as a forced full refresh (safe
/// direction: false-invalidate, never false-reuse).
#[cfg(feature = "embed_v8")]
fn compute_bundle_skip_key(
    bundler_out: &BundlerOutput,
    routes: &[zfb_router::Route],
) -> Option<[u8; 32]> {
    let bundle_bytes = match std::fs::read(&bundler_out.bundle_path) {
        Ok(b) => b,
        Err(err) => {
            tracing::warn!(
                site = "compute_bundle_skip_key",
                path = %bundler_out.bundle_path.display(),
                error = %err,
                "could not read bundle file for skip-key; will perform full refresh"
            );
            return None;
        }
    };

    let mut hasher = Sha256::new();

    // Part 1: bundle bytes.
    hasher.update(b"bundle:");
    hasher.update((bundle_bytes.len() as u64).to_le_bytes());
    hasher.update(b":");
    hasher.update(&bundle_bytes);
    hasher.update(b"\n");

    // Part 2: router-scan signature — sorted (source_path, template) pairs.
    // Sorting by source_path gives a deterministic order regardless of the
    // filesystem walk order.
    let mut pairs: Vec<(String, String)> = routes
        .iter()
        .map(|r| (r.source_path.to_string_lossy().into_owned(), r.template()))
        .collect();
    pairs.sort_unstable();
    hasher.update(b"routes:");
    hasher.update((pairs.len() as u64).to_le_bytes());
    hasher.update(b":");
    for (src, tpl) in &pairs {
        hasher.update(src.as_bytes());
        hasher.update(b"=");
        hasher.update(tpl.as_bytes());
        hasher.update(b"\n");
    }

    // Part 3: static pages/**.html bodies (issue #956 gate (a)). Sorted by
    // source_path for a deterministic order; the paths come straight from
    // the router's WalkDir over `<project_root>/pages`, so reading them
    // here sees exactly the files the renderer would copy at render time.
    let mut static_html_routes: Vec<&zfb_router::Route> =
        routes.iter().filter(|r| r.static_html).collect();
    static_html_routes.sort_unstable_by(|a, b| a.source_path.cmp(&b.source_path));
    hasher.update(b"static-html:");
    hasher.update((static_html_routes.len() as u64).to_le_bytes());
    hasher.update(b":");
    for route in static_html_routes {
        let body = match std::fs::read(&route.source_path) {
            Ok(b) => b,
            Err(err) => {
                tracing::warn!(
                    site = "compute_bundle_skip_key",
                    path = %route.source_path.display(),
                    error = %err,
                    "could not read static .html page for skip-key; \
                     will perform full refresh"
                );
                return None;
            }
        };
        hasher.update(route.source_path.to_string_lossy().as_bytes());
        hasher.update(b"=");
        hasher.update((body.len() as u64).to_le_bytes());
        hasher.update(b":");
        hasher.update(&body);
        hasher.update(b"\n");
    }

    Some(hasher.finalize().into())
}

/// `(routes_by_source, ssr_routes, url_index)` — the triple
/// [`build_dev_route_tables`] produces and [`DevRouteTables`] stores.
#[cfg(feature = "embed_v8")]
type BuiltRouteTables = (
    HashMap<PathBuf, Vec<DevRouteEntry>>,
    Vec<RouteUniverseEntry>,
    HashMap<String, RouteUniverseEntry>,
);

/// `(routes_by_source, ssr_routes, url_index, paths_cache_hits,
/// paths_cache_misses)` — the 5-tuple [`build_dev_route_tables_timed`]
/// returns with PathsCache stats exposed for ZFB_DEV_TIMING instrumentation
/// (issue #991).
#[cfg(feature = "embed_v8")]
type TimedRouteTables = (
    HashMap<PathBuf, Vec<DevRouteEntry>>,
    Vec<RouteUniverseEntry>,
    HashMap<String, RouteUniverseEntry>,
    u64,
    u64,
);

/// Timed variant of [`build_dev_route_tables`]: returns the same tables plus
/// the [`PathsCache`] hit and miss DELTAS for this call, for P3 diagnostics
/// (issue #991). The cache persists across ticks (#994 item B) and its
/// counters are cumulative, so the inner build reports per-call deltas.
#[cfg(feature = "embed_v8")]
fn build_dev_route_tables_timed(
    router: &zfb_router::Router,
    plan: &crate::render_pipeline::RouteUniversePlan,
    project_root: &Path,
    renderer: &Arc<Mutex<Option<RendererState>>>,
    paths_cache: &mut PathsCache,
) -> Result<TimedRouteTables> {
    let (routes_by_source, ssr_routes, url_index, hits, misses) =
        build_dev_route_tables_inner(router, plan, project_root, renderer, paths_cache)?;
    Ok((routes_by_source, ssr_routes, url_index, hits, misses))
}

/// Build the dev session's source→route + SSR route tables from the router
/// scan + the live V8 host (issue #659 — extracted from `boot_dev_renderer`
/// so boot and the watch-ADD rebuild produce byte-identical tables). The
/// `renderer` mutex must already hold a started/reloaded [`RendererState`]
/// because the dynamic-route `paths()` runtime phase borrows the live
/// embedded V8 host out of it.
#[cfg(feature = "embed_v8")]
fn build_dev_route_tables(
    router: &zfb_router::Router,
    plan: &crate::render_pipeline::RouteUniversePlan,
    project_root: &Path,
    renderer: &Arc<Mutex<Option<RendererState>>>,
    paths_cache: &mut PathsCache,
) -> Result<BuiltRouteTables> {
    let (routes_by_source, ssr_routes, url_index, _hits, _misses) =
        build_dev_route_tables_inner(router, plan, project_root, renderer, paths_cache)?;
    Ok((routes_by_source, ssr_routes, url_index))
}

/// Percent-decode a URL path segment, returning a borrowed `str` when the
/// input has no percent-encoded sequences or an owned `String` when decoding
/// produces a different value (issue #1019).
///
/// Consistent with how `read_from_dist` (zfb-server) handles incoming paths:
/// the Axum HTTP layer decodes the path before handing it to the handler, so
/// the index must decode too before doing a lookup. On invalid UTF-8 (malformed
/// percent-sequence) falls back to the raw input so the lookup can still
/// proceed (graceful degradation — the entry probably won't match, which is
/// the correct answer for a malformed URL).
fn percent_decode_url(path: &str) -> std::borrow::Cow<'_, str> {
    // Fast path: no `%` in the path means nothing to decode.
    if !path.contains('%') {
        return std::borrow::Cow::Borrowed(path);
    }
    // Decode percent-encoded sequences; keep the path as UTF-8.
    let bytes: Vec<u8> = percent_decode_bytes(path.as_bytes());
    match String::from_utf8(bytes) {
        Ok(s) => std::borrow::Cow::Owned(s),
        Err(_) => std::borrow::Cow::Borrowed(path),
    }
}

/// Decode percent-encoded sequences in a byte slice.
fn percent_decode_bytes(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' && i + 2 < input.len() {
            if let (Some(h), Some(l)) = (from_hex_digit(input[i + 1]), from_hex_digit(input[i + 2]))
            {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

/// Convert an ASCII hex digit (`0`–`9`, `a`–`f`, `A`–`F`) to its numeric
/// value; returns `None` for any other byte.
fn from_hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Candidate lookup keys for a single SSG route's `url_path` (issue #1019).
///
/// Mirrors `lookup_keys` in `crates/zfb-server/src/routes.rs:1674` so a
/// request URL resolves to the index entry under the same normalisation rules
/// the HTTP layer applies:
///
/// - Trailing slashes are stripped before candidate generation so
///   `/posts/a` and `/posts/a/` map to the same entry.
/// - `/posts/a` → candidates `["/posts/a", "/posts/a/index.html",
///   "/posts/a/"]` covering both slash policies.
/// - Root `/` and the all-slashes edge case get the root candidates
///   `["/", "/index.html"]`.
/// - Routes that already end with a file extension (e.g. `feed.xml`) are
///   returned as-is — no `index.html` or slash variants.
///
/// Query strings and percent-encoding are NOT handled here; the caller
/// (`DevRenderSession::lookup_by_url`) normalises the input before
/// indexing into the table.
fn url_index_lookup_keys(url_path: &str) -> Vec<String> {
    let stripped = url_path.trim_end_matches('/');
    if stripped.is_empty() {
        // Root or all-slashes path.
        return vec!["/".to_string(), "/index.html".to_string()];
    }
    // Routes with an explicit file extension (e.g. `/feed.xml`, `/sitemap.xml`)
    // are served verbatim — no slash/index.html variants.
    let last_segment = stripped.rsplit('/').next().unwrap_or(stripped);
    if last_segment.contains('.') {
        return vec![format!("/{stripped}")];
    }
    vec![
        format!("/{stripped}"),
        format!("/{stripped}/index.html"),
        format!("/{stripped}/"),
    ]
}

/// Build the reverse URL-lookup index for all SSG routes (issue #1019).
///
/// Iterates every `DevRouteEntry` in `routes_by_source` (which contains only
/// SSG routes — SSR routes live in `ssr_routes` and are excluded by
/// construction) and inserts each entry under all candidate keys produced by
/// [`url_index_lookup_keys`]. When two SSG routes would claim the same
/// normalised key (unlikely in a well-formed project but possible if two
/// sources share an output path), the first writer wins and a warning is
/// emitted so the user knows a route is shadowed.
fn build_url_index(
    routes_by_source: &HashMap<PathBuf, Vec<DevRouteEntry>>,
) -> HashMap<String, RouteUniverseEntry> {
    let mut index: HashMap<String, RouteUniverseEntry> = HashMap::new();
    for entries in routes_by_source.values() {
        for dev_entry in entries {
            let entry = &dev_entry.entry;
            // Strip any leading slash before normalising — `url_path` values
            // in `RouteUniverseEntry` already start with `/`; passing them
            // through `url_index_lookup_keys` works because that function
            // re-adds the leading slash in its output.
            let url_no_leading = entry.url_path.trim_start_matches('/');
            for key in url_index_lookup_keys(url_no_leading) {
                if let Some(existing) = index.get(&key) {
                    if existing.url_path != entry.url_path {
                        crate::output::warn(format!(
                            "url_index: key {key:?} claimed by both {:?} and {:?}; \
                             the first entry wins",
                            existing.url_path, entry.url_path,
                        ));
                    }
                } else {
                    index.insert(key, entry.clone());
                }
            }
        }
    }
    index
}

/// Synthetic `routes_by_source` key for a static injected route (epic
/// #1228, S3 #1231). The staged injected module lives outside the real
/// `pages/` and is unwatched (node_modules, restart-only — §4), so it has
/// no project-relative source path the dependency graph would key on.
/// A dedicated, stable synthetic key keyed on the pattern keeps the seed:
///
/// - **distinct** from every real `pages/` source (the `__zfb_injected__`
///   prefix can never be produced by `Router::scan` over the real tree);
/// - **stable** across route-table swaps, so [`diff_route_tables`] sees an
///   identical entry set under the same key on every tick and never
///   reports the seed as `changed` or `vanished` (the seed's staleness is
///   driven by the explicit re-stale in
///   [`DevRenderSession::reseed_injected_static_routes`], not the diff).
fn injected_source_key(pattern: &str) -> PathBuf {
    PathBuf::from("__zfb_injected__").join(pattern.trim_start_matches('/'))
}

/// Merge the static injected-route seeds into a freshly-built
/// `routes_by_source` (epic #1228, S3 #1231, §2). Each seed is filed under
/// its [`injected_source_key`] with `params: None` (static routes carry no
/// `paths()` provenance), exactly like a normal static SSG route. The
/// caller rebuilds `url_index` afterwards so `lookup_by_url` resolves the
/// seeded URLs. No-op (and zero allocation beyond the empty-slice walk) on
/// the parity path.
fn seed_injected_static_routes(
    routes_by_source: &mut HashMap<PathBuf, Vec<DevRouteEntry>>,
    seeds: &[RouteUniverseEntry],
) {
    for entry in seeds {
        routes_by_source
            .entry(injected_source_key(&entry.route_key))
            .or_default()
            .push(DevRouteEntry {
                entry: entry.clone(),
                params: None,
            });
    }
}

/// Inner implementation shared by [`build_dev_route_tables`] and
/// [`build_dev_route_tables_timed`]. Returns the route tables plus this
/// call's PathsCache hit/miss deltas (#991 instrumentation; the cache is
/// caller-owned and persists across ticks per #994 item B, so the
/// cumulative counters are differenced here).
#[cfg(feature = "embed_v8")]
fn build_dev_route_tables_inner(
    router: &zfb_router::Router,
    plan: &crate::render_pipeline::RouteUniversePlan,
    project_root: &Path,
    renderer: &Arc<Mutex<Option<RendererState>>>,
    paths_cache: &mut PathsCache,
) -> Result<TimedRouteTables> {
    let prerender_map = build_prerender_map(router.routes(), project_root, |msg| {
        crate::output::warn(msg)
    });

    // Build the source-path → entries map once. Router source paths are
    // project-relative; PageId keys on the same value (the orchestrator
    // tracks pages by their source path). Each value is a Vec so a dynamic
    // SSG source can hold its N `paths()`-expanded entries (#502/#507);
    // static routes contribute a single-element vec.
    let mut routes_by_source: HashMap<PathBuf, Vec<DevRouteEntry>> = HashMap::new();
    let mut ssr_routes: Vec<RouteUniverseEntry> = Vec::new();
    for route in router.routes() {
        if let Some(entry) = plan
            .static_routes
            .iter()
            .find(|e| e.route_key == route.template())
        {
            // Default to SSG when the prerender map has no entry —
            // delegated to `is_ssr_route` for the single source of truth
            // (matches `crates/zfb/src/commands/build.rs`).
            if crate::render_pipeline::is_ssr_route(&prerender_map, &route.template()) {
                ssr_routes.push(entry.clone());
            } else {
                // Static routes carry no `paths()` provenance (#958) —
                // their sources always render in full (fallback S1).
                routes_by_source
                    .entry(route.source_path.clone())
                    .or_default()
                    .push(DevRouteEntry {
                        entry: entry.clone(),
                        params: None,
                    });
            }
        }
    }
    // Deep-review regression (PR #376): also pick up `prerender = false`
    // routes that live in `plan.deferred_dynamic` — dynamic routes whose
    // `paths()` couldn't be statically expanded. Without this, dev SSR
    // would 404 a `[slug]` route with `export const prerender = false`
    // (the SsrRouteSet has no record for it). Mirrors the build-side
    // chain in `crates/zfb/src/commands/build.rs` for `ssr_route_refs` /
    // `ssr_route_keys_for_runtime_bundle`.
    //
    // The deferred entry has no concrete URL — the template IS the most
    // specific identifier we have. We synthesise a RouteUniverseEntry
    // whose URL fields mirror the template; only the route pattern is
    // load-bearing for SsrRouteSet (output_path / url_path are never
    // touched by the SSR dispatch path).
    for deferred in &plan.deferred_dynamic {
        if !crate::render_pipeline::is_ssr_route(&prerender_map, &deferred.template) {
            continue;
        }
        ssr_routes.push(RouteUniverseEntry {
            url_path: deferred.template.clone(),
            output_path: PathBuf::new(),
            route_key: deferred.template.clone(),
            static_html: false,
            source_path: None,
        });
    }

    // Issue #502/#507 — expand `prerender = true` (SSG) dynamic routes so
    // `zfb dev` serves them. `zfb build` does this two-phase expansion;
    // `boot_dev_renderer` never did, so on a fresh scaffold (no prior
    // `dist/` to fall back to) every `[slug]`/`[...tag]` SSG URL 404'd. This
    // mirrors the build path: phase 1 statically extracts literal `paths()`
    // arrays; phase 2 evaluates the remainder at runtime through the same
    // embedded V8 host the SSG renderer already owns (basic-blog's `paths()`
    // resolves slugs from `getCollection()`, which only the runtime path
    // can answer).
    //
    // SSR (`prerender = false`) dynamic routes are deliberately excluded
    // here — they were already routed into `ssr_routes` above and must reach
    // the V8 host at request time, NOT be SSG'd to disk (a disk artifact
    // would shadow the SSR handler). We collect only the SSG-eligible
    // deferred routes for expansion.
    //
    // Scope (#507): this expands `paths()` once, at dev-server boot. Live
    // re-expansion when collection content is added/removed during a running
    // dev session (watch-time `paths()` invalidation) is OUT of scope and is
    // a documented follow-up — a content edit today requires a `zfb dev`
    // restart to pick up new/removed dynamic URLs.
    let ssg_deferred: Vec<crate::render_pipeline::PendingDynamicRoute> = plan
        .deferred_dynamic
        .iter()
        .filter(|d| !crate::render_pipeline::is_ssr_route(&prerender_map, &d.template))
        .cloned()
        .collect();
    // PathsCache hit/miss DELTAS for this call — issue #991
    // instrumentation. The cache is caller-owned and persists across
    // ticks (#994 item B), so its cumulative counters are snapshotted
    // here and differenced at the end to feed the ZFB_DEV_TIMING P3
    // line.
    let hits_before = paths_cache.hit_count();
    let misses_before = paths_cache.miss_count();

    if !ssg_deferred.is_empty() {
        // Map each route template back to its source path so the resolved
        // (concrete-URL) entries — whose `source_path` is `None` and whose
        // `route_key` is the template — can be filed under the right source
        // `PageId` in `routes_by_source`. The dependency graph and the
        // watcher key on source paths, so the fan-out must land under the
        // dynamic source's path, not under each concrete URL.
        let template_to_source: HashMap<String, PathBuf> = ssg_deferred
            .iter()
            .map(|d| (d.template.clone(), d.source_path.clone()))
            .collect();

        // Phase 1 — literal `paths()` arrays (no runtime needed).
        // A missing `paths()` export on an SSG route is a hard error here
        // too — consistent with `zfb build` (issue #520).
        let static_expansion = expand_dynamic_routes(&ssg_deferred, project_root, paths_cache)?;

        // Phase 2 — evaluate the routes phase 1 couldn't resolve statically
        // through the running embedded V8 host. We borrow the live host out
        // of the dev session's renderer mutex (the same `Arc<Mutex<Option<
        // RendererState>>>` the SSG render callback and the SSR adapter
        // share) and dispatch via `WorkerDispatch::EmbeddedV8`, exactly like
        // `commands/build.rs::eval_deferred_paths`.
        //
        // The renderer mutex is only taken when phase 1 left anything to
        // evaluate (#994 item B): when every `paths()` resolved
        // statically there is nothing to dispatch, so borrowing the live
        // host would be a pointless lock (and previously made an
        // all-literal project hard-require a started renderer here).
        let runtime_expansion = if static_expansion.deferred.is_empty() {
            crate::render_pipeline::DynamicExpansion::default()
        } else {
            let mut lock = renderer.lock().unwrap_or_else(|p| {
                tracing::warn!(
                    site = "boot_dev_renderer.paths",
                    "mutex poisoned, recovered"
                );
                p.into_inner()
            });
            let state = lock
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("renderer not started for paths() evaluation"))?;
            let host = state.embedded_v8_host_mut().ok_or_else(|| {
                anyhow::anyhow!(
                    "embedded V8 host unavailable after start; \
                     Backend::EmbeddedV8 state had no host"
                )
            })?;
            let mut dispatch = WorkerDispatch::EmbeddedV8 { host };
            eval_deferred_paths_via_worker(
                &static_expansion.deferred,
                &mut dispatch,
                paths_cache,
                None,
            )
        };

        // Surface routes that still couldn't be expanded (after both phases)
        // as warnings so the user knows why a `[slug]` route didn't appear —
        // mirrors the build-side diagnostics. These are silently dropped from
        // `dist/` otherwise.
        for d in &runtime_expansion.deferred {
            crate::output::warn(format!(
                "dynamic SSG route {} not expanded in dev: {}",
                d.template, d.reason
            ));
        }

        // File every resolved concrete-URL entry under its dynamic source's
        // `PageId`. A single `[slug].tsx` source thus accumulates N entries,
        // which `render_one` fans out into N `RenderedPage`s per tick.
        //
        // Issue #958 — retain each URL's `paths()` params provenance by
        // zipping `resolved` with the parallel-ordered
        // `resolved_with_params` (documented invariant of
        // `DynamicExpansion`; both are built together in
        // `push_resolved_paths`). A length mismatch means the provenance
        // cannot be trusted: warn and file `params: None` for that whole
        // expansion, which downstream forces the affected sources to
        // always render in full (fallback S1) — degraded performance,
        // never under-rendering.
        for expansion in [static_expansion, runtime_expansion] {
            let params_aligned = expansion.resolved.len() == expansion.resolved_with_params.len();
            if !params_aligned {
                crate::output::warn(format!(
                    "dynamic route expansion: resolved/params length mismatch \
                     ({} vs {}); content-edit narrowing disabled for the \
                     affected source(s)",
                    expansion.resolved.len(),
                    expansion.resolved_with_params.len(),
                ));
            }
            let mut params_iter = expansion.resolved_with_params.into_iter();
            for entry in expansion.resolved {
                let params = if params_aligned {
                    params_iter.next().map(|p| p.params)
                } else {
                    None
                };
                if let Some(source) = template_to_source.get(&entry.route_key) {
                    routes_by_source
                        .entry(source.clone())
                        .or_default()
                        .push(DevRouteEntry { entry, params });
                } else {
                    // Should not happen: every resolved entry's route_key came
                    // from one of the ssg_deferred routes. Warn rather than drop
                    // silently if the invariant is ever violated.
                    crate::output::warn(format!(
                        "resolved dynamic URL {} has no matching source route ({}); skipping",
                        entry.url_path, entry.route_key
                    ));
                }
            }
        }
    }

    // Build the reverse URL-lookup index from all SSG entries (issue #1019).
    // SSR-only routes are already in `ssr_routes`, not `routes_by_source`,
    // so the index is naturally restricted to SSG routes.
    let url_index = build_url_index(&routes_by_source);

    Ok((
        routes_by_source,
        ssr_routes,
        url_index,
        paths_cache.hit_count() - hits_before,
        paths_cache.miss_count() - misses_before,
    ))
}

/// Resolve the root the dev router and primary bundler walk should use.
///
/// Conventional projects keep their real `project_root/pages` directory. A
/// true zero-pages consumer is allowed only when at least one package route
/// survived precedence: its session receives a private empty directory so the
/// router/bundler can keep their directory-based contract without creating a
/// `pages/` directory in the consumer. A project with neither source of routes
/// retains the historical missing-pages error.
#[cfg(feature = "embed_v8")]
fn resolve_dev_user_pages_root(
    project_root: &Path,
    has_surviving_injected_routes: bool,
) -> Result<(PathBuf, Option<tempfile::TempDir>)> {
    let real_pages_dir = project_root.join("pages");
    if real_pages_dir.is_dir() {
        return Ok((real_pages_dir, None));
    }
    if !has_surviving_injected_routes {
        return Err(anyhow::anyhow!(
            "no pages/ directory under {}",
            project_root.display()
        ));
    }

    let guard = tempfile::Builder::new()
        .prefix("zfb-empty-user-pages-")
        .tempdir()
        .context("creating internal empty pages root for injected routes")?;
    let pages_dir = guard.path().join("pages");
    std::fs::create_dir_all(&pages_dir)
        .with_context(|| format!("creating internal empty pages dir {}", pages_dir.display()))?;
    Ok((pages_dir, Some(guard)))
}

/// Keep the historical empty-project rejection while allowing package routes
/// to be the sole source of a true zero-pages dev session.
#[cfg(feature = "embed_v8")]
fn ensure_dev_routes_available(
    plan: &crate::render_pipeline::RouteUniversePlan,
    injected_route_set: &InjectedRouteSet,
) -> Result<()> {
    if plan.static_routes.is_empty()
        && plan.deferred_dynamic.is_empty()
        && injected_route_set.is_empty()
    {
        return Err(anyhow::anyhow!(
            "no routes to render — dev mode skips renderer boot"
        ));
    }
    Ok(())
}

/// Bring up the renderer and the route map for the dev session.
///
/// On any error, returns it unchanged so the caller decides whether to
/// fall back to a noop renderer or hard-fail. Today the dev command
/// chooses to fall back so the user still gets a reachable HTTP server
/// while they fix the underlying issue.
// `boot_dev_renderer` constructs the long-lived dev RendererState
// backed by the in-process V8 host. Compiled in only when the
// `embed_v8` feature is on (issue #371, sub-task 4.1a).
#[cfg(feature = "embed_v8")]
#[allow(clippy::too_many_arguments)] // 8 params: #1550 added collection_roots (index-aligned resolved absolute roots for canonical-path matching); a struct would just shuffle the same threaded fields
fn boot_dev_renderer(
    project_root: &Path,
    cfg: &config::Config,
    // #1550 — per-collection resolved absolute roots, index-aligned with
    // `cfg.collections` (canonical for out-of-root collections). Stored on
    // the session so the frontmatter-hash seed, tick-narrowing, and
    // discovery sites key on a root form that matches notify's canonical
    // event paths. Sourced from the boot-time [`ResolvedRoots`] inventory.
    collection_roots: &[PathBuf],
    v8_plugin_hooks: zfb_render::PluginRegistryHooks,
    // Plugin-registered import aliases from `setup_registries.aliases`.
    // Threaded into `BundlerInput::plugin_alias_entries` so the dev-mode
    // esbuild invocation can resolve plugin aliases from pages / layouts /
    // shared modules (#268).
    plugin_alias_entries: Vec<(String, String)>,
    // Plugin-registered virtual-module `(specifier, source)` pairs.
    // Threaded into `BundlerInput::plugin_virtual_modules` (#268).
    plugin_virtual_modules: Vec<(String, String)>,
    // S2 (#1230) — package-owned injected routes registered during setup.
    // Materialised ONCE here into a session-lifetime staging dir (the B1
    // multi-root mechanism) whose root is threaded into the dev bundler via
    // `build_pages_root` so the injected entrypoints + their `virtual:`
    // imports are in the dev bundle. Empty (or all-shadowed) → no staging
    // dir, byte-identical to today (sharp edge 8).
    injected_routes: &[zfb_build::InjectedRoute],
    // Issue #1182 — when `true`, return a SCAFFOLD session (no V8 host, empty
    // route tables) and SKIP `assemble_and_bundle_dev` + host start +
    // route-table build. The deferred boot task runs that expensive work past
    // `TcpListener::bind` via [`DevRenderSession::refresh_bundle_and_routes`]
    // (it swaps the host into the `None` slot and publishes the rebuilt tables
    // in place). Gated by [`defer_dev_bundle_decision`] in `run`. `false`
    // keeps the eager pre-bind path unchanged.
    defer_bundle: bool,
) -> Result<DevRenderSession> {
    check_runtime_installed(project_root)?;

    // Resolve package routes before deciding whether the user-facing pages
    // directory is required. A true zero-pages consumer has no such directory
    // but is still runnable when an injected route survives precedence.
    let user_pages_dir = project_root.join("pages");
    let resolution =
        crate::commands::package_routes::resolve_dev_pages_root(&user_pages_dir, injected_routes)
            .context("staging package-owned injected routes for the dev bundle")?;
    let (pages_dir, empty_user_pages_guard) =
        resolve_dev_user_pages_root(project_root, resolution.guard.is_some())?;
    let router = zfb_router::Router::scan(&pages_dir).map_err(anyhow::Error::from)?;

    let plan = build_route_universe(router.routes());

    // S2 (#1230) — materialise the package-owned injected routes ONCE into a
    // session-lifetime staging dir (B1 multi-root). `resolve_dev_pages_root`
    // runs the SAME survivor selection + FULL validation the build uses
    // (user-precedence drop, package-vs-package shape-key hard-error,
    // case-insensitive collision guard, `.client`/trailing-`index`
    // rejection), but stages ONLY the synthesized injected modules — it does
    // NOT copy the user's real `pages/`, so conventional dev sessions retain
    // the real `pages/` scan + watcher identity (HMR untouched, sharp edge 1).
    // #1518's zero-pages exception uses the separate private empty root only
    // because no user directory exists. On the empty / all-shadowed path
    // `guard` is `None`, so no staging dir is held and the
    // dev bundler gets no additive injected root — byte-identical to today
    // (sharp edge 8). A materialiser error (an invalid pattern, a
    // package-vs-package collision) is fatal here: the same error `zfb build`
    // would raise, surfaced at dev boot rather than silently dropping the
    // route. The synthesized `.tsx` for a pattern is byte-identical to the
    // build's (the SAME `synthesize_*_overlay_module`), the required parity.
    //
    // KNOWN LIMITATION (S3, #1231): the survivor set is computed ONCE at boot
    // and the staging dir is session-lifetime. If a user CREATES a `pages/`
    // file during `zfb dev` that shadows an injected route which survived at
    // boot, the staging dir is not refreshed, so the next rebuild's bundler
    // walks BOTH the new user page and the staged injected module for the same
    // route → a clean route-collision error (NOT silent corruption); the user
    // page wins only after a dev restart. Re-pruning the survivor set on
    // watch-add/rename events that touch `pages/` is S3's concern (it owns the
    // route-table refresh path); S2 deliberately stays bundle-inclusion-only.
    //
    // S3 (#1231) — the SAME resolution drives the dev route universe: the
    // post-precedence survivor set seeds the STATIC injected routes into
    // `routes_by_source` + `url_index` (URL == pattern) and backs the
    // request-time `InjectedRouteSet` handed to the dev server. Both are
    // built from the survivors, NOT the raw registration list, so a
    // user-shadowed or package-vs-package-dropped pattern never reaches the
    // universe or the fallback (sharp edges 4/7).
    let (injected_pages_root, injected_static_seeds, injected_route_set): (
        Option<(tempfile::TempDir, PathBuf)>,
        Vec<RouteUniverseEntry>,
        InjectedRouteSet,
    ) = {
        // Build the static seeds + the post-precedence survivor set from the
        // SAME `materialized` survivor list (sharp edges 4/7). Both are empty
        // / default on the parity path.
        let seeds = crate::commands::package_routes::static_injected_seeds(
            injected_routes,
            &resolution.materialized,
        )
        .into_iter()
        .map(|s| {
            crate::output::info(format!(
                "injected route `{}` → dev route universe (serves at {})",
                s.pattern,
                s.output_path().display()
            ));
            s.seed_entry
        })
        .collect::<Vec<_>>();
        let survivors = crate::commands::package_routes::surviving_injected_routes(
            injected_routes,
            &resolution.materialized,
        );
        let route_set = if survivors.is_empty() {
            InjectedRouteSet::default()
        } else {
            InjectedRouteSet::new(survivors)
        };
        let pages_root = match resolution.guard {
            Some(guard) => {
                for mr in &resolution.materialized {
                    crate::output::info(format!(
                        "injected route `{}` → dev bundle (staged at pages/{})",
                        mr.pattern,
                        mr.pages_rel.display()
                    ));
                }
                Some((guard, resolution.build_pages_root))
            }
            // No survivors (no injected routes, or all user-shadowed): parity
            // path — no staging dir, dev bundler gets `build_pages_root = None`.
            None => None,
        };
        (pages_root, seeds, route_set)
    };

    // Guardrail 2 (#507): an all-dynamic SSG project (only `paths()`-based
    // routes, no static `/`) has an empty `static_routes` but a non-empty
    // `deferred_dynamic`. A true zero-pages project also has an empty user
    // router plan, but surviving injected routes provide the route universe
    // and request-time fallback. Only skip boot when neither source exists.
    ensure_dev_routes_available(&plan, &injected_route_set)?;

    // Assemble + run the dev bundle (#659: extracted into
    // `assemble_and_bundle_dev` so the watch-ADD rebuild reuses the exact
    // same bundler configuration). Recomputes the content snapshot from
    // disk, so a re-bundle on a created file sees the new content.
    // Stash the bundle inputs BEFORE they are moved into the bundle /
    // host-start calls below, so a watch-ADD (#659) can re-bundle with the
    // identical configuration and reload the host in place. The clones are
    // cheap relative to a bundle (a few small Vecs + the config).
    let rebuild_inputs = DevRebuildInputs {
        cfg: cfg.clone(),
        v8_plugin_hooks: v8_plugin_hooks.clone(),
        plugin_alias_entries: plugin_alias_entries.clone(),
        plugin_virtual_modules: plugin_virtual_modules.clone(),
        // #994 item A — extract the embedded esbuild binary ONCE here;
        // the handle lives on `DevRebuildInputs` for the whole dev
        // session, so every tick's `assemble_bundler_input` skips its
        // per-call tempdir extraction. Skipped when `ZFB_ESBUILD_BIN`
        // overrides the embedded binary (the assemble-side skip
        // condition would ignore the path anyway). Extraction failure
        // is non-fatal, matching the assemble-side handling: warn and
        // fall back to the per-tick extraction by passing `None`.
        esbuild: if std::env::var_os("ZFB_ESBUILD_BIN").is_none() {
            match crate::render_pipeline::embedded_binary("esbuild") {
                Ok(pair) => Some(pair),
                Err(e) => {
                    crate::output::warn(format!(
                        "could not extract embedded esbuild at dev boot ({e}); \
                         falling back to per-tick extraction"
                    ));
                    None
                }
            }
        } else {
            None
        },
        // S2 (#1230) — the injected-route staging dir materialised above.
        // `None` on the parity path; `Some` holds the TempDir for the whole
        // session so the staged modules outlive every `bundle()` call.
        injected_pages_root,
        // #1518 — retain the internal empty primary root when the real
        // consumer project deliberately has no `pages/` directory.
        empty_user_pages_root: empty_user_pages_guard.map(|guard| (guard, pages_dir.clone())),
    };

    // Persistent dev shadow-tree session (issue #993) — created once
    // here, used for the boot bundle below, then stored on
    // `DevRenderInner` so every refresh reuses the same shadow tree.
    let mut shadow_session = ShadowSession::new(project_root)?;

    // #994 item B — the PathsCache is seeded here and stored on
    // `DevRenderInner` below, so every refresh tick reuses the entries
    // this boot-time build populated (boot/refresh parity: both go
    // through the same cache). Built before the eager/deferred branch so the
    // deferred-scaffold path also stores a (still-empty) cache the deferred
    // `refresh_bundle_and_routes` then populates.
    let mut paths_cache = PathsCache::new();

    // Issue #1182 — deferred dev bundle. The eager `assemble_and_bundle_dev`
    // (content-snapshot embed + esbuild over the route graph) is the dominant,
    // size-scaling pre-bind cost — the residual of #1161 that #1166/#1170 left
    // behind. When `defer_bundle` is set (boot-lazy + a servable prebuilt
    // `dist/`, decided by `defer_dev_bundle_decision`), build a SCAFFOLD
    // session here: NO V8 host (renderer slot `None`) and EMPTY route tables.
    // The deferred boot task then runs the bundle + host start +
    // `paths()`-expanding route-table build past `TcpListener::bind` via
    // `refresh_bundle_and_routes`, which swaps the host into this `None` slot
    // and publishes the rebuilt tables in place. The prebuilt `dist/` serves
    // every route until that publish lands, so first-accept is O(1) regardless
    // of project size. When `defer_bundle` is false, the eager pre-bind path
    // below runs exactly as before.
    // #1284/#1287 — the eager boot bundle's per-route metafile Module deps,
    // captured here and seeded into the graph once the session + graph handle
    // exist (see the `set_dep_graph` + populate call in `run`). On the deferred
    // path this stays empty; the deferred `refresh_bundle_and_routes` seeds the
    // edges itself.
    let mut boot_route_module_deps: Vec<zfb_build::RouteModuleDeps> = Vec::new();
    let (renderer, routes_by_source, ssr_routes, url_index, content_trace_token) = if defer_bundle {
        (
            // Scaffold renderer slot — the deferred `refresh_bundle_and_routes`
            // swaps the first live host into this same `Arc` (which the render
            // callback, SSR adapter, and render-on-request hook all already
            // hold a clone of).
            Arc::new(Mutex::new(None)),
            HashMap::new(),
            Vec::new(),
            HashMap::new(),
            None,
        )
    } else {
        // Test-only slow-step injection (issue #1182 falsifiability guard,
        // EAGER half): `ZFB_DEV_TEST_SLOW_BUNDLE_MS` sleeps right before the
        // EAGER pre-bind bundle. This is the co-located twin of the
        // deferred-task seam in `run` — together they make the bind-before-bundle
        // e2e falsifiable wherever the BOOT bundle runs. In the correct ordering
        // a boot-lazy + servable-`dist/` boot takes the scaffold branch above,
        // so this eager seam never fires and only the deferred-task seam does
        // (after bind → banner stays fast). If the deferral is reverted — the
        // boot bundle moved back here, before `TcpListener::bind` — THIS seam
        // fires before bind and delays the ready banner past the e2e's deadline,
        // failing the guard. Without a co-located eager seam, an un-defer revert
        // would silently pass. Runs synchronously in `run` before bind, so the
        // blocking sleep delays bind exactly as a real pre-bind bundle would.
        if let Ok(raw) = std::env::var("ZFB_DEV_TEST_SLOW_BUNDLE_MS") {
            if let Ok(ms) = raw.trim().parse::<u64>() {
                if ms > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(ms));
                }
            }
        }

        // Boot path — timing not collected here (one-shot at startup, not a
        // hot-path tick). `timing_enabled = false` so no Instant::now() overhead.
        let mut bundler_out: BundlerOutput = assemble_and_bundle_dev(
            project_root,
            cfg,
            plugin_alias_entries,
            plugin_virtual_modules,
            false,
            Some(&mut shadow_session),
            rebuild_inputs.esbuild_path(),
            rebuild_inputs.empty_user_pages_root(),
            // S2 (#1230) — include the injected modules in the BOOT bundle from
            // the staging dir materialised above. `None` on the parity path.
            rebuild_inputs.injected_pages_root(),
        )?
        .output;
        let trace_token = wrap_dev_bundle_with_content_trace(&mut bundler_out, router.routes())?;
        // #1284/#1287 — capture the boot bundle's per-route Module deps for
        // post-graph seeding (the graph does not exist yet at this point).
        boot_route_module_deps = bundler_out.route_module_deps.clone();

        let state = start(RendererStartInput {
            bundle_path: bundler_out.bundle_path.clone(),
            sourcemap_path: bundler_out.sourcemap_path.clone(),
            backend: Backend::EmbeddedV8 {
                host_factory: crate::v8_host_adapter::make_v8_host_factory_with_hooks(
                    v8_plugin_hooks,
                ),
            },
            request_timeout: None,
        })
        .map_err(anyhow::Error::from)
        .context("renderer start failed")?;

        // Wrap the renderer state in the shared `Arc<Mutex<Option<...>>>` up
        // front so the SSG `paths()` runtime-evaluation phase below can borrow
        // the live embedded V8 host out of the same handle the SSG render
        // callback and the SSR adapter use later (#502/#507). One host, shared
        // across boot-time paths() eval, build-time SSG render, and request-time
        // SSR.
        let renderer: Arc<Mutex<Option<RendererState>>> = Arc::new(Mutex::new(Some(state)));

        // Issue #367 — extract `export const prerender = …` per page so
        // we can keep SSG-eligible pages in `routes_by_source` (the SSG
        // render callback's lookup table) while routing `prerender =
        // false` pages into the request-time SSR set instead. Without this
        // split the SSG callback would stamp a stale snapshot to disk on
        // every watcher tick and the dist fallback would shadow the SSR
        // handler.
        // Build the route tables from the router scan + the live host (#659:
        // extracted into `build_dev_route_tables` so the watch-ADD rebuild
        // reproduces the boot tables exactly).
        let (mut routes_by_source, ssr_routes, mut url_index) =
            build_dev_route_tables(&router, &plan, project_root, &renderer, &mut paths_cache)?;

        // S3 (#1231) — seed the STATIC injected routes into the boot tables so
        // `lookup_by_url` resolves their URLs (URL == pattern). The router scan
        // above walks the real user-pages tree (or #1518's private empty root)
        // only; staged injected modules live outside it, so they must be merged
        // here (and on every swap, via `refresh_bundle_and_routes`). Rebuild
        // `url_index` to cover the seeds.
        // No-op on the parity path. (Boot stale-marking happens after the
        // session is constructed — see `mark_injected_seeds_stale` below.)
        if !injected_static_seeds.is_empty() {
            seed_injected_static_routes(&mut routes_by_source, &injected_static_seeds);
            url_index = build_url_index(&routes_by_source);
        }

        (
            renderer,
            routes_by_source,
            ssr_routes,
            url_index,
            Some(trace_token),
        )
    };

    // Issue #958 — seed the frontmatter gate cache so the FIRST body-only
    // edit of a pre-existing collection file can already narrow (G4 only
    // fires for genuinely missing/changed frontmatter). ~1 read + parse +
    // hash per collection file; trivial against the bundle step above.
    let fm_hashes = seed_frontmatter_hashes(cfg, collection_roots);

    let session = DevRenderSession {
        inner: Arc::new(DevRenderInner {
            routes: std::sync::RwLock::new(DevRouteTables {
                routes_by_source,
                ssr_routes,
                url_index,
            }),
            renderer,
            project_root: project_root.to_path_buf(),
            rebuild_inputs,
            // #1550 — canonical-for-out-of-root collection roots from the
            // boot inventory; the content sites key on these.
            collection_roots: collection_roots.to_vec(),
            // No successful refresh yet — first tick always runs fully.
            last_successful_skip_key: Mutex::new(None),
            fm_hashes: Mutex::new(fm_hashes),
            shadow_session: Mutex::new(Some(shadow_session)),
            dep_graph: Mutex::new(None),
            content_trace: Mutex::new(DevContentTraceState {
                token: content_trace_token,
                reads_by_observation: BTreeMap::new(),
                boot_complete: false,
            }),
            out_of_root_watch_targets: Mutex::new(std::collections::BTreeSet::new()),
            boot_route_module_deps,
            paths_cache: Mutex::new(paths_cache),
            stale: Mutex::new(StaleRoutes::default()),
            lazy_render: lazy_dev_render_enabled(),
            // The next render-callback invocation is the boot build.
            boot_render_done: std::sync::atomic::AtomicBool::new(false),
            // S3 (#1231) — the static injected-route seeds + post-precedence
            // survivor set, both built from the same survivor list above.
            injected_static_seeds,
            injected_route_set,
        }),
    };

    // S3 (#1231) — stale-mark the static injected seeds at boot so the FIRST
    // request for an injected URL renders through the lazy adapter rather than
    // 404ing on an absent `html_root` file. (On the deferred-boot scaffold
    // path the boot tables are empty and the seeds are merged + stale-marked by
    // the first `refresh_bundle_and_routes` swap instead; this boot mark is the
    // eager-path counterpart. Marking when the tables are still empty is
    // harmless — the stale map is keyed by output_path, independent of the
    // route tables.) No-op on the parity path.
    session.mark_injected_seeds_stale();

    Ok(session)
}

/// SHA-256 over the canonical JSON of a file's parsed frontmatter
/// (issue #958, fallback-G4 gate). `serde_json`'s default BTreeMap-backed
/// objects serialize with sorted keys, so the string is canonical: a YAML
/// reformat that parses to the same value hashes identically and does not
/// trip the gate. `None` when the file is unreadable or unparseable.
#[cfg(feature = "embed_v8")]
fn frontmatter_hash(path: &Path) -> Option<[u8; 32]> {
    let source = std::fs::read_to_string(path).ok()?;
    let uf = zfb_content::frontmatter::extract(path, &source).ok()?;
    let json = serde_json::to_string(&uf.value).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    Some(hasher.finalize().into())
}

/// Walk every configured collection root and yield the path of each file
/// that passes its collection's include/exclude filter — i.e. the session's
/// content-collection MEMBERSHIP, independent of whether the entry's
/// frontmatter happens to be readable.
///
/// Two consumers, and the distinction between them matters (issue #1581):
///
/// - [`seed_frontmatter_hashes`] keeps only the entries it could hash — an
///   unparseable entry has no meaningful G4 gate hash.
/// - The `known_content` registry takes the FULL membership. An entry whose
///   frontmatter is missing or malformed is still a real, already-known
///   collection entry; dropping it would let a spurious FSEvents `Created`
///   for that file fall through to the discovery regime and cost the tick
///   its #958 narrowing — the exact bug #1581 fixes.
///
/// `collection_roots` comes from the boot [`ResolvedRoots`] inventory, so
/// out-of-root roots are already canonical and the walked paths compare
/// equal to the canonical paths `notify` delivers (#1550).
#[cfg(feature = "embed_v8")]
fn collect_collection_entries(cfg: &config::Config, collection_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = Vec::new();
    for (collection, root) in cfg.collections.iter().zip(collection_roots) {
        let filter = match zfb_content::collection::CollectionFilter::new(
            collection.include.as_deref(),
            collection.exclude.as_deref(),
            collection.id_strip_suffix.as_deref(),
        ) {
            Ok(f) => f,
            Err(_) => continue,
        };
        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .flatten()
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if zfb_content::collection::derive_slug_for_file(root, path, &filter).is_none() {
                continue;
            }
            entries.push(path.to_path_buf());
        }
    }
    entries
}

/// Complete current membership plus the route-param candidates that can prove
/// a dynamic `paths()` source is truly one-entry-per-content-entry.
///
/// This is intentionally separate from [`collect_collection_entries`]. The
/// latter keeps its tolerant historical contract for `KnownContentEntries`;
/// provenance must instead fail closed when a configured collection cannot be
/// walked or its filter cannot be compiled.
#[cfg(feature = "embed_v8")]
#[derive(Debug, Clone)]
struct DevContentMembershipSnapshot {
    membership: ContentCollectionMembership,
    slug_candidates: BTreeMap<ContentCollectionId, BTreeMap<PathBuf, BTreeSet<String>>>,
}

/// Re-walk configured collections into a complete provenance membership
/// snapshot. A missing collection root is a known empty collection; any other
/// walk/filter failure makes provenance unavailable so graph callers retain
/// their conservative `All` fallback.
#[cfg(feature = "embed_v8")]
fn collect_content_provenance_membership(
    cfg: &config::Config,
    collection_roots: &[PathBuf],
) -> Result<DevContentMembershipSnapshot> {
    if cfg.collections.len() != collection_roots.len() {
        anyhow::bail!(
            "content collection roots are incomplete ({} configured collections, {} roots)",
            cfg.collections.len(),
            collection_roots.len()
        );
    }

    let mut membership = ContentCollectionMembership::new();
    let mut slug_candidates: BTreeMap<ContentCollectionId, BTreeMap<PathBuf, BTreeSet<String>>> =
        BTreeMap::new();

    for (collection, root) in cfg.collections.iter().zip(collection_roots) {
        let collection_id = ContentCollectionId::new(collection.name.clone());
        let filter = zfb_content::collection::CollectionFilter::new(
            collection.include.as_deref(),
            collection.exclude.as_deref(),
            collection.id_strip_suffix.as_deref(),
        )
        .map_err(anyhow::Error::from)
        .with_context(|| {
            format!(
                "compile collection filter for `{}` while collecting content provenance",
                collection.name
            )
        })?;

        match std::fs::metadata(root) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                membership.insert(collection_id.clone(), std::iter::empty::<PathBuf>());
                slug_candidates.insert(collection_id, BTreeMap::new());
                continue;
            }
            Err(error) => {
                return Err(anyhow::Error::from(error)).with_context(|| {
                    format!(
                        "stat collection root {} while collecting content provenance",
                        root.display()
                    )
                });
            }
        }

        let mut entries = BTreeSet::new();
        let mut candidates_for_collection = BTreeMap::new();
        for entry in walkdir::WalkDir::new(root).follow_links(false) {
            let entry = entry.map_err(anyhow::Error::from).with_context(|| {
                format!(
                    "walk collection root {} while collecting content provenance",
                    root.display()
                )
            })?;
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let Some(slug) = zfb_content::collection::derive_slug_for_file(root, path, &filter)
            else {
                continue;
            };
            let path = path.to_path_buf();
            candidates_for_collection
                .insert(path.clone(), content_entry_slug_candidates(&path, &slug));
            entries.insert(path);
        }
        membership.insert(collection_id.clone(), entries);
        slug_candidates.insert(collection_id, candidates_for_collection);
    }

    Ok(DevContentMembershipSnapshot {
        membership,
        slug_candidates,
    })
}

/// Route values that can identify a collection entry without guessing from
/// source text. The collection filter supplies the canonical slug; a valid
/// frontmatter `slug` override is also an actual runtime path candidate.
#[cfg(feature = "embed_v8")]
fn content_entry_slug_candidates(path: &Path, slug: &str) -> BTreeSet<String> {
    let mut candidates = BTreeSet::new();
    add_content_slug_candidate(&mut candidates, slug);
    if let Ok(source) = std::fs::read_to_string(path) {
        if let Ok(frontmatter) = zfb_content::frontmatter::extract(path, &source) {
            if let Some(value) = frontmatter
                .value
                .get("slug")
                .and_then(|value| value.as_str())
            {
                add_content_slug_candidate(&mut candidates, value);
            }
        }
    }
    candidates
}

#[cfg(feature = "embed_v8")]
fn add_content_slug_candidate(candidates: &mut BTreeSet<String>, value: &str) {
    candidates.insert(value.to_string());
    if let Some(stripped) = value.strip_prefix('/') {
        candidates.insert(stripped.to_string());
    }
    if value == "index" {
        candidates.insert(String::new());
    }
    if let Some(stripped) = value.strip_suffix("/index") {
        candidates.insert(stripped.to_string());
    }
}

/// The classified worker trace input for one reconciliation drain.
///
/// Runtime `paths()` reads qualify as entry reads only when every resolved
/// output has exactly one current member matched by its actual `slug` param,
/// every member is represented exactly once, and no route entry lacked params.
/// Tag/pagination routes, subsets, aliases, and ambiguous output all become
/// aggregate collection reads. This is deliberately narrow: an over-broad
/// aggregate edge is safe; a guessed direct edge can under-render.
#[cfg(feature = "embed_v8")]
struct ClassifiedContentTrace {
    observed: BTreeSet<DevContentTraceObservation>,
    reads: BTreeMap<DevContentTraceObservation, Vec<TrackedContentRead>>,
}

/// Convert worker trace events into route observations and pure provenance
/// reads. A visit is meaningful even when it made no `getCollection()` call:
/// it is the positive evidence needed to drop that route phase's prior worker
/// generation observations.
#[cfg(feature = "embed_v8")]
fn classify_content_trace_events(
    events: impl IntoIterator<Item = DevContentTraceEvent>,
    routes_by_source: &HashMap<PathBuf, Vec<DevRouteEntry>>,
    membership: &DevContentMembershipSnapshot,
    project_root: &Path,
) -> Result<ClassifiedContentTrace> {
    let mut observed = BTreeSet::new();
    let mut reads = BTreeMap::new();
    for event in events {
        let (consumer, entries) =
            resolve_content_trace_consumer(&event.source, routes_by_source, project_root)?;
        let observation = DevContentTraceObservation {
            consumer: consumer.clone(),
            phase: event.phase,
        };
        if event.kind == DevContentTraceEventKind::Visit {
            observed.insert(observation);
            continue;
        }
        let collection = ContentCollectionId::new(event.collection.ok_or_else(|| {
            anyhow::anyhow!(
                "worker content read from {} omitted its collection name",
                event.source
            )
        })?);
        let reads_for_observation = reads.entry(observation).or_insert_with(Vec::new);
        match event.phase {
            DevContentTracePhase::Render => {
                reads_for_observation.push(TrackedContentRead::collection(consumer, collection));
            }
            DevContentTracePhase::Paths => {
                if let Some(entry_paths) =
                    verified_paths_entry_reads(entries, &collection, membership)
                {
                    reads_for_observation.extend(entry_paths.into_iter().map(|entry| {
                        TrackedContentRead::entry(consumer.clone(), collection.clone(), entry)
                    }));
                } else {
                    reads_for_observation
                        .push(TrackedContentRead::collection(consumer, collection));
                }
            }
        }
    }
    Ok(ClassifiedContentTrace { observed, reads })
}

/// Validate a worker-emitted route source and resolve it to the graph's page
/// key plus the current route-table entries that prove a `paths()` read.
#[cfg(feature = "embed_v8")]
fn resolve_content_trace_consumer<'a>(
    raw_source: &str,
    routes_by_source: &'a HashMap<PathBuf, Vec<DevRouteEntry>>,
    project_root: &Path,
) -> Result<(PageId, &'a [DevRouteEntry])> {
    let raw_source = PathBuf::from(raw_source);
    let source = if raw_source.is_absolute() {
        if !raw_source.starts_with(project_root) {
            anyhow::bail!(
                "worker content trace source {} escapes project root {}",
                raw_source.display(),
                project_root.display()
            );
        }
        raw_source
    } else {
        if raw_source.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            anyhow::bail!(
                "worker content trace source {} is not a project-relative route path",
                raw_source.display()
            );
        }
        project_root.join(raw_source)
    };
    let entries = routes_by_source.get(&source).ok_or_else(|| {
        anyhow::anyhow!(
            "worker content trace source {} is absent from the current route table",
            source.display()
        )
    })?;
    Ok((PageId::new(source), entries))
}

/// Apply one current-worker drain to the retained observations. Route phases
/// that the worker actually visited are replaced wholesale; all others remain
/// as a conservative bridge until this worker has executed them too.
#[cfg(feature = "embed_v8")]
fn apply_content_trace_observations(
    reads_by_observation: &mut BTreeMap<DevContentTraceObservation, Vec<TrackedContentRead>>,
    observed: impl IntoIterator<Item = DevContentTraceObservation>,
    reads: impl IntoIterator<Item = (DevContentTraceObservation, Vec<TrackedContentRead>)>,
) {
    for observation in observed {
        reads_by_observation.remove(&observation);
    }
    for (observation, reads) in reads {
        reads_by_observation.insert(observation, reads);
    }
}

/// Return a complete direct-entry mapping for one dynamic route source, or
/// `None` when the route is an aggregate/ambiguous consumer.
#[cfg(feature = "embed_v8")]
fn verified_paths_entry_reads(
    entries: &[DevRouteEntry],
    collection: &ContentCollectionId,
    membership: &DevContentMembershipSnapshot,
) -> Option<BTreeSet<PathBuf>> {
    let members = membership.slug_candidates.get(collection)?;
    if entries.is_empty() || entries.len() != members.len() {
        return None;
    }

    let mut selected = BTreeSet::new();
    for entry in entries {
        let params = entry.params.as_ref()?;
        // A `slug` param is the runtime collection API's entry identity. Other
        // params (tag/page/date/etc.) are deliberately not inferred to be
        // entry IDs, even if they happen to equal a slug in a singleton set.
        let slug = params
            .scalars
            .get("slug")
            .cloned()
            .or_else(|| params.arrays.get("slug").map(|parts| parts.join("/")))?;
        let matches: Vec<&PathBuf> = members
            .iter()
            .filter_map(|(path, candidates)| candidates.contains(&slug).then_some(path))
            .collect();
        if matches.len() != 1 || !selected.insert(matches[0].clone()) {
            return None;
        }
    }

    (selected.len() == members.len()).then_some(selected)
}

/// Replace every graph `Content` edge with the current successful trace
/// groups, preserving all other dependency kinds. This is used for both
/// persisted-graph cleanup and a fresh worker/table generation; no stale
/// `Content` edge can survive a failed current derivation.
#[cfg(feature = "embed_v8")]
fn replace_content_edges(
    graph: &mut DependencyGraph,
    groups: impl IntoIterator<Item = zfb_build::ContentEdgeGroup>,
) {
    let mut desired: BTreeMap<PageId, BTreeSet<PathBuf>> = BTreeMap::new();
    for group in groups {
        desired
            .entry(group.consumer)
            .or_default()
            .extend(group.entries);
    }

    let mut pages: BTreeSet<PageId> = graph.pages().into_iter().collect();
    pages.extend(desired.keys().cloned());
    for page in pages {
        let mut deps: Vec<(PathBuf, zfb_graph::DepKind)> = graph
            .deps_of(&page)
            .into_iter()
            .filter(|(dep, kind)| *kind != zfb_graph::DepKind::Content && dep != page.path())
            .collect();
        if let Some(entries) = desired.get(&page) {
            deps.extend(
                entries
                    .iter()
                    .cloned()
                    .map(|entry| (entry, zfb_graph::DepKind::Content)),
            );
        }
        graph.upsert(PageDeps::new(page, deps));
    }
}

/// Boot-seed the frontmatter gate cache (issue #958): hash every
/// configured collection file's frontmatter, keyed by absolute path.
/// Membership routes through [`collect_collection_entries`] so the seeded
/// set is exactly the walker's. Unreadable / unparseable files (and
/// collections with uncompilable filter globs) are simply not seeded —
/// their first edit falls back to a full render (G4) and seeds the hash
/// then.
///
/// NOTE (#1581): the `known_content` registry deliberately does NOT share
/// this map's key set — it takes the full membership walk instead, because
/// an entry that failed to hash here is still an already-known entry.
///
/// `collection_roots` is the boot inventory's per-collection resolved
/// absolute root (issue #1550), index-aligned with `cfg.collections`:
/// canonical for out-of-root collections so the WalkDir-derived seed keys
/// match the CANONICAL paths `notify` later delivers on an edit (a literal
/// `project_root.join("../x")` would seed keys that never match, so the
/// narrowing gate always tripped for out-of-root content).
#[cfg(feature = "embed_v8")]
fn seed_frontmatter_hashes(
    cfg: &config::Config,
    collection_roots: &[PathBuf],
) -> HashMap<PathBuf, [u8; 32]> {
    let mut hashes: HashMap<PathBuf, [u8; 32]> = HashMap::new();
    for path in collect_collection_entries(cfg, collection_roots) {
        if let Some(hash) = frontmatter_hash(&path) {
            hashes.insert(path, hash);
        }
    }
    hashes
}

/// Per-route HTML output directory for the dev pipeline (issue #534).
///
/// Dev's renderer writes one file per route on each tick (initial scan
/// and every watcher rebuild). Until #534, these writes landed in the
/// project's `outDir` (`dist/`), silently overwriting the production
/// HTML produced by a prior `pnpm build` — stripping the prod-only
/// `<link rel="stylesheet">` / islands `<script type="module">` head
/// injections and breaking subsequent `pnpm preview`.
///
/// Dev now writes to `<project_root>/.zfb-build/dev-pages/`. This sits
/// under the existing `.zfb-build/` intermediate directory (already
/// `.gitignore`d by the project templates) and is read back by
/// `DevRenderSession::render_one_with` to populate the in-memory
/// `PageCache`. End users of the dev server are unaffected — page
/// lookups are URL-keyed and never touch this path.
fn dev_html_root_for(project_root: &Path) -> PathBuf {
    project_root.join(".zfb-build").join("dev-pages")
}

/// The isolated directory `zfb dev` writes its STABLE served assets into
/// (issue #1189) — `styles.css`, `islands.js`, island chunks, and
/// `client/*.js`. Its `assets/` subdir is mounted at `/assets/` FIRST,
/// with the project's `dist/assets/` as a fallback (for a boot-lazy
/// prebuilt seed's hashed assets).
///
/// Why a separate dir, mirroring [`dev_html_root_for`]: dev used to write
/// these stable assets straight into the project's `outDir` (`dist/`),
/// which `zfb build` shares. A one-off `zfb build` against the live `dist/`
/// wipes it and emits HASHED-only assets, so the dev-served
/// `/assets/styles.css` 404s and the site goes unstyled with no self-heal.
/// Writing dev's assets under `.zfb-build/` (already `.gitignore`d) takes
/// them out of the build's write set entirely — the same fix #534 applied
/// to dev HTML, now for assets.
fn dev_assets_root_for(project_root: &Path) -> PathBuf {
    project_root.join(".zfb-build").join("dev-assets")
}

/// `true` when one of the edited entry's slug candidates appears among a
/// route's resolved params (issue #958, spec §4). ANY-param semantics:
/// scalars match by value, catchall arrays match by their `/`-join (an
/// empty catchall joins to `""`, matching the bare-root-index candidate);
/// a locale scalar never blocks a slug-array match. Exact byte equality —
/// no case folding, no percent-decoding.
#[cfg(feature = "embed_v8")]
fn params_match(
    p: &crate::render_pipeline::ResolvedRouteParams,
    candidates: &std::collections::BTreeSet<String>,
) -> bool {
    p.scalars.values().any(|v| candidates.contains(v))
        || p.arrays.values().any(|a| candidates.contains(&a.join("/")))
}

/// Compute the tick's narrowing decision from the plan's content-
/// narrowing hint (issue #958, spec §4). Runs once per render-callback
/// invocation. Every failure mode degrades to [`TickNarrowing::Off`] —
/// i.e. today's full fan-out — never to silent under-rendering:
///
/// - G2: a changed file outside every configured collection (or failing
///   its include/exclude globs, or an uncompilable filter glob),
/// - G3: file read / frontmatter parse error,
/// - G4: frontmatter hash missing or changed (frontmatter feeds
///   cross-page props — titles in sidebars/prev-next — so a frontmatter
///   delta re-renders the full selected set). The new hash is ALWAYS
///   stored, including on the Off path, so the next body-only edit
///   narrows.
///
/// Per-source fallbacks (source simply absent from the returned map ⇒
/// [`RouteFilter::All`]):
///
/// - S1: any entry lacks params provenance (static routes; zip-length
///   mismatch at table build time),
/// - S2: zero entries matched the candidate set (aggregate dynamic
///   consumers — tags/pagination — whose params are not slug-shaped).
#[cfg(feature = "embed_v8")]
fn compute_tick_narrowing(
    session: &DevRenderSession,
    hint: Option<&zfb_build::ContentNarrowing>,
) -> TickNarrowing {
    let Some(hint) = hint else {
        return TickNarrowing::Off;
    };
    if hint.changed_content.is_empty() {
        return TickNarrowing::Off;
    }
    let inner = &session.inner;

    let TickCandidates {
        candidates,
        gate_tripped,
    } = derive_tick_candidates(inner, hint, true);
    if gate_tripped || candidates.is_empty() {
        return TickNarrowing::Off;
    }

    // Steps 4+5 — per-source selection against the POST-refresh route
    // tables (the reloader ran before the render callback, so params are
    // fresh). Only narrowed sources enter the map; everything else — the
    // always-rendered set — renders in full by absence.
    let tables = inner.routes.read().unwrap_or_else(|p| p.into_inner());
    let per_source = match_candidate_routes(&tables, &candidates);
    if per_source.is_empty() {
        return TickNarrowing::Off;
    }
    tracing::debug!(
        narrowed_sources = per_source.len(),
        "content-edit narrowing active for tick (issue #958)"
    );
    TickNarrowing::PerSource(
        per_source
            .into_iter()
            .map(|(source, matched)| (source, RouteFilter::Only(matched)))
            .collect(),
    )
}

/// Result of [`derive_tick_candidates`] — the tick's slug candidate set
/// plus whether any of the #958 whole-tick gates tripped while deriving
/// it.
#[cfg(feature = "embed_v8")]
struct TickCandidates {
    /// Union of slug candidates across the changed files that passed
    /// their per-file gates.
    candidates: std::collections::BTreeSet<String>,
    /// True when any gate tripped: bad collection glob, G2 (file outside
    /// every collection), G3 (read/parse error), or G4 (missing/changed
    /// frontmatter). The #958 eager-narrowing caller treats a trip as
    /// "no narrowing this tick"; the #1025 lazy caller ignores it (a
    /// non-eager route is stale, never under-rendered).
    gate_tripped: bool,
}

/// Per-tick slug-candidate derivation shared by the #958 narrowing gate
/// ([`compute_tick_narrowing`]) and the #1025 lazy eager set
/// ([`compute_lazy_eager_sets`]). Spec §4 steps 1–3.
///
/// `frontmatter_gate_skips_candidates` selects the G4 semantics: `true`
/// (#958) suppresses a frontmatter-changed file's candidates — its
/// cross-page props force the full fan-out; `false` (#1025 lazy mode)
/// still collects them — frontmatter edits eager-render the entry's own
/// routes, and the cross-page fallout is covered by staling the
/// remainder. The gate HASH is stored in both modes (store-then-compare)
/// so the bookkeeping stays warm for whichever mode the next tick runs
/// in.
#[cfg(feature = "embed_v8")]
fn derive_tick_candidates(
    inner: &DevRenderInner,
    hint: &zfb_build::ContentNarrowing,
    frontmatter_gate_skips_candidates: bool,
) -> TickCandidates {
    use std::collections::BTreeSet;

    // Compile each collection's (root, filter) pair once for the tick. A
    // bad glob means membership cannot be evaluated reliably — trip the
    // gate and yield no candidates.
    //
    // #1550 — the root comes from the boot inventory's per-collection
    // `collection_roots` (canonical for out-of-root collections), NOT a
    // fresh `project_root.join(normalize_relative(path))`. `hint.changed_content`
    // holds CANONICAL notify event paths, so an out-of-root collection's
    // literal `../x` root would never `derive_slug_for_file`-match and the
    // G2 gate tripped every tick (full re-render fallback).
    let mut compiled: Vec<(PathBuf, zfb_content::collection::CollectionFilter)> =
        Vec::with_capacity(inner.rebuild_inputs.cfg.collections.len());
    for (collection, root) in inner
        .rebuild_inputs
        .cfg
        .collections
        .iter()
        .zip(&inner.collection_roots)
    {
        match zfb_content::collection::CollectionFilter::new(
            collection.include.as_deref(),
            collection.exclude.as_deref(),
            collection.id_strip_suffix.as_deref(),
        ) {
            Ok(filter) => compiled.push((root.clone(), filter)),
            Err(err) => {
                tracing::warn!(
                    site = "derive_tick_candidates",
                    error = %err,
                    "collection filter failed to compile; no slug candidates this tick"
                );
                return TickCandidates {
                    candidates: BTreeSet::new(),
                    gate_tripped: true,
                };
            }
        }
    }

    let mut gate_tripped = false;
    let mut candidates: BTreeSet<String> = BTreeSet::new();
    let mut fm_hashes = inner.fm_hashes.lock().unwrap_or_else(|p| p.into_inner());
    for file in &hint.changed_content {
        // Step 1 — collection resolution. A file may belong to
        // multiple collections; candidates are unioned. Zero
        // memberships ⇒ G2 (whole tick falls back), but keep
        // processing the other files so their gate hashes update.
        let slugs: Vec<String> = compiled
            .iter()
            .filter_map(|(root, filter)| {
                zfb_content::collection::derive_slug_for_file(root, file, filter)
            })
            .collect();
        if slugs.is_empty() {
            gate_tripped = true; // G2
            continue;
        }

        // Step 2 — frontmatter gate (G3/G4).
        let fm_value = match std::fs::read_to_string(file)
            .map_err(anyhow::Error::from)
            .and_then(|source| {
                zfb_content::frontmatter::extract(file, &source)
                    .map(|uf| uf.value)
                    .map_err(anyhow::Error::from)
            }) {
            Ok(value) => value,
            Err(_) => {
                // G3 — and the stored hash no longer describes the
                // file: drop it so the edit that FIXES the parse
                // error re-seeds via the G4 miss path.
                gate_tripped = true;
                fm_hashes.remove(file);
                continue;
            }
        };
        let hash: [u8; 32] = match serde_json::to_string(&fm_value) {
            Ok(json) => {
                let mut hasher = Sha256::new();
                hasher.update(json.as_bytes());
                hasher.finalize().into()
            }
            Err(_) => {
                gate_tripped = true;
                fm_hashes.remove(file);
                continue;
            }
        };
        // Store-then-compare: the new hash must land even when the
        // gate trips (G4), so the NEXT body-only edit narrows.
        let prev = fm_hashes.insert(file.clone(), hash);
        if prev != Some(hash) {
            gate_tripped = true; // G4 — missing or changed frontmatter.
            if frontmatter_gate_skips_candidates {
                continue;
            }
        }

        // Step 3 — slug candidate set.
        for slug in slugs {
            if slug == "index" {
                candidates.insert(String::new());
            }
            if let Some(stripped) = slug.strip_suffix("/index") {
                candidates.insert(stripped.to_string());
            }
            candidates.insert(slug);
        }
        // Frontmatter `slug:` override candidate (Docusaurus-style):
        // verbatim AND with one leading `/` stripped.
        if let Some(fm_slug) = fm_value.get("slug").and_then(|v| v.as_str()) {
            if let Some(stripped) = fm_slug.strip_prefix('/') {
                candidates.insert(stripped.to_string());
            }
            candidates.insert(fm_slug.to_string());
        }
    }
    TickCandidates {
        candidates,
        gate_tripped,
    }
}

/// Per-source candidate matching shared by the #958 narrowing gate and
/// the #1025 lazy eager set (spec §4 steps 4+5): map each dynamic
/// source to the output paths whose `paths()` params match a candidate.
/// Fallbacks are expressed by ABSENCE from the returned map:
///
/// - S1: any entry lacks params provenance (static routes; zip-length
///   mismatch at table build time),
/// - S2: zero entries matched the candidate set (aggregate dynamic
///   consumers — tags/pagination — whose params are not slug-shaped).
#[cfg(feature = "embed_v8")]
fn match_candidate_routes(
    tables: &DevRouteTables,
    candidates: &std::collections::BTreeSet<String>,
) -> HashMap<PathBuf, HashSet<PathBuf>> {
    let mut per_source: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new();
    for (source, entries) in &tables.routes_by_source {
        if entries.iter().any(|de| de.params.is_none()) {
            continue; // S1
        }
        let matched: HashSet<PathBuf> = entries
            .iter()
            .filter(|de| {
                de.params
                    .as_ref()
                    .is_some_and(|p| params_match(p, candidates))
            })
            .map(|de| de.entry.output_path.clone())
            .collect();
        if matched.is_empty() {
            continue; // S2
        }
        per_source.insert(source.clone(), matched);
    }
    per_source
}

/// Compute the lazy tick's EAGER set (issue #1025): for a content-edit
/// tick, the edited entries' own routes — the same candidate machinery
/// the #958 narrowing gate uses, with two lazy-mode differences:
///
/// - G4 is NOT a fallback: a frontmatter edit still eager-renders the
///   edited entry's own routes. The cross-page fallout frontmatter can
///   have (sidebar titles, prev/next labels) is covered by staling the
///   remainder of the fan-out instead of eagerly re-rendering it.
/// - The remaining gates degrade to "fewer eager routes" instead of a
///   full fan-out — "not eager" is safe in lazy mode because every
///   non-eager selected route is marked stale and re-renders on
///   request, never silently under-rendered.
///
/// Returns source → eager output-path set. An empty map (no hint —
/// `.tsx`/G5/G6/data/discovery ticks —, no candidates, bad globs, or
/// S1/S2-only sources) means a fully-lazy tick: every selected route is
/// marked stale and nothing renders eagerly.
#[cfg(feature = "embed_v8")]
fn compute_lazy_eager_sets(
    session: &DevRenderSession,
    hint: Option<&zfb_build::ContentNarrowing>,
) -> HashMap<PathBuf, HashSet<PathBuf>> {
    let Some(hint) = hint else {
        return HashMap::new();
    };
    if hint.changed_content.is_empty() {
        return HashMap::new();
    }
    let inner = &session.inner;
    let TickCandidates { candidates, .. } = derive_tick_candidates(inner, hint, false);
    if candidates.is_empty() {
        return HashMap::new();
    }
    let tables = inner.routes.read().unwrap_or_else(|p| p.into_inner());
    match_candidate_routes(&tables, &candidates)
}

/// Lazy dev render tick (issue #1025): the switch-ON replacement for
/// the fully-eager fan-out in [`make_render_callback`].
///
/// Eager-vs-stale split per tick class:
///
/// - Content edit with slug-derivable own routes (body OR frontmatter
///   edit): eager = the edited entries' own routes
///   ([`compute_lazy_eager_sets`]); stale = the remainder of the
///   selected fan-out — explicitly including S2 aggregate-heavy sources
///   (tag indexes, paginated lists) and S1 statics.
/// - Everything else (component/page `.tsx`, G5/G6 route-structure,
///   data edits, `renderer_fresh` discovery ticks — the hint is `None`
///   for all of these): eager = nothing; ALL selected routes are marked
///   stale.
///
/// A source unknown to the renderer contributes nothing (same no-op as
/// the eager path). An eager render FAILURE re-marks the eager set
/// stale so the request-time path retries against the live host, and is
/// logged without killing the watcher (same per-page tolerance as the
/// eager path).
///
/// ORDERING: the stale-state commit happens strictly BEFORE this
/// function returns. The return value flows through the pipeline's
/// write loop into the tick's `BuildOutcome`, so by the time the
/// outcome reaches `on_outcome` (the future SSE Page event) the
/// staleness map and the route tables (swapped by the reloader earlier
/// in the tick) already describe the new world — a reloading browser
/// can always resolve the stale route.
#[cfg(feature = "embed_v8")]
fn lazy_render_tick(
    session: &DevRenderSession,
    dist_dir: &Path,
    pages: &[PageId],
    narrowing: Option<&zfb_build::ContentNarrowing>,
) -> Result<Vec<RenderedPage>> {
    let eager_sets = compute_lazy_eager_sets(session, narrowing);

    let mut out: Vec<RenderedPage> = Vec::new();
    let mut stale: HashSet<PathBuf> = HashSet::new();
    let mut rendered_paths: Vec<PathBuf> = Vec::new();

    for page in pages {
        // Snapshot this source's output paths under a short read lock
        // (mirrors `render_one`'s clone-then-release discipline).
        let outputs: Vec<PathBuf> = {
            let tables = session
                .inner
                .routes
                .read()
                .unwrap_or_else(|p| p.into_inner());
            match tables.routes_by_source.get(page.path()) {
                Some(entries) => entries
                    .iter()
                    .map(|de| de.entry.output_path.clone())
                    .collect(),
                None => continue, // unknown to the renderer — no-op
            }
        };
        let eager = eager_sets.get(page.path());
        stale.extend(
            outputs
                .iter()
                .filter(|o| !eager.is_some_and(|set| set.contains(*o)))
                .cloned(),
        );

        let Some(eager) = eager else {
            continue; // fully-lazy source — nothing renders eagerly
        };
        let filter = RouteFilter::Only(eager.clone());
        match session.render_one(page, dist_dir, &filter) {
            Ok(rendered) => {
                rendered_paths.extend(
                    rendered
                        .iter()
                        .map(|r| r.output_path.as_path().to_path_buf()),
                );
                out.extend(rendered);
            }
            Err(err) => {
                // The eager routes did NOT reach disk — mark them stale
                // so the request-time path retries, and keep the
                // watcher alive (same tolerance as the eager callback).
                stale.extend(eager.iter().cloned());
                output::error(format!(
                    "renderer error for {}: {err:#}",
                    page.path().display()
                ));
            }
        }
    }

    // Commit the stale state BEFORE returning (see ORDERING above). The
    // eager-rendered routes are cleared last — fresh output supersedes
    // any staling from earlier ticks; the two sets are disjoint within
    // this tick.
    session.inner.mark_stale(stale);
    session.inner.clear_stale(&rendered_paths);
    Ok(out)
}

/// Build the [`PageRenderer`] callback that the orchestrator hands to
/// [`DevAssetPipeline`].
fn make_render_callback(session: DevRenderSession, dist_dir: PathBuf) -> PageRenderer {
    Arc::new(move |pages: &[PageId], narrowing| {
        // Issue #1025 — lazy dev render. Switch ON (the default since
        // the #1027 activation flip) routes the tick through the
        // eager-vs-stale split; OFF (the `ZFB_DEV_EAGER=1` hatch) falls
        // through to the fully-eager fan-out below, untouched. The
        // session's FIRST invocation — the eager initial build at boot —
        // stays on the eager path even when the switch is ON: the
        // request-time stale-render adapter doesn't exist yet, so a
        // lazy boot would 404 every route (review finding on #1025).
        #[cfg(feature = "embed_v8")]
        if session.inner.lazy_render && !session.inner.take_boot_render_pending() {
            let result = lazy_render_tick(&session, &dist_dir, pages, narrowing);
            if let Err(error) = session.reconcile_content_provenance() {
                output::warn(format!(
                    "content provenance unavailable after lazy render; \
                     content edits will conservatively rebuild all pages: {error:#}"
                ));
            }
            return result;
        }
        // Issue #958 — one narrowing decision per tick; per-page filters
        // fall out of the per-source map. The V8-off path has no
        // collection configs to match against, so it never narrows.
        //
        // Issue #1058 — the hint is now populated permissively (mixed /
        // Created ticks carry the edited content for the lazy eager basis),
        // so the EAGER fan-out narrowing must gate on the strict
        // `fan_out_safe` flag: a co-changed module can affect every page, so
        // a non-fan-out-safe tick falls back to the full fan-out exactly as
        // before. (The lazy path above intentionally consumes the hint
        // regardless of `fan_out_safe`.)
        #[cfg(feature = "embed_v8")]
        let tick_narrowing = compute_tick_narrowing(&session, narrowing.filter(|n| n.fan_out_safe));
        #[cfg(not(feature = "embed_v8"))]
        let tick_narrowing = {
            let _ = narrowing;
            TickNarrowing::Off
        };
        let mut out = Vec::with_capacity(pages.len());
        for page in pages {
            let filter = match &tick_narrowing {
                TickNarrowing::Off => &RouteFilter::All,
                TickNarrowing::PerSource(map) => map.get(page.path()).unwrap_or(&RouteFilter::All),
            };
            match session.render_one(page, &dist_dir, filter) {
                Ok(rendered) => {
                    // A static route yields one page; a dynamic SSG route
                    // yields one page per `paths()`-resolved URL (#502/#507).
                    // An empty Vec means the source path is unknown to the
                    // renderer (dynamic route deferred to SSR, or a page never
                    // in the router scan) — intentionally a no-op so the
                    // watcher tick still succeeds and other pages keep
                    // rebuilding.
                    out.extend(rendered);
                }
                Err(err) => {
                    // One page's render failure must not kill the
                    // watcher. Log and continue.
                    output::error(format!(
                        "renderer error for {}: {err:#}",
                        page.path().display()
                    ));
                }
            }
        }
        #[cfg(feature = "embed_v8")]
        if let Err(error) = session.reconcile_content_provenance() {
            output::warn(format!(
                "content provenance unavailable after render; \
                 content edits will conservatively rebuild all pages: {error:#}"
            ));
        }
        Ok(out)
    })
}

/// Build the live watch-ADD discovery hook handed to
/// [`zfb_build::BuildOrchestrator::run`] (issue #659).
///
/// The orchestrator invokes it with the `Created` subset of a tick's
/// changed paths. We restrict the expensive re-bundle to files created
/// under `content/`, `pages/`, or any configured collection root (the
/// roots that feed the SSR bundle and the route table — collections may
/// live anywhere, e.g. `src/mdx/notes`); a file created elsewhere —
/// `styles/`, `public/` — can never add a content-collection route, so
/// we skip discovery for it and let the normal tick handle it.
///
/// On a relevant create we delegate to
/// [`DevRenderSession::discover_created`] (re-bundle → swap in a fresh
/// host → rebuild route tables). The render batch then drains the new
/// worker's actual reads and reconciles the complete collection membership,
/// giving a new entry both its direct and aggregate consumer edges.
///
/// The returned [`DiscoveryOutcome`] reports `renderer_reloaded: true`
/// whenever the refresh ran, so the pipeline's per-tick
/// `reload_renderer` is skipped and the tick bundles exactly once.
/// Any routes that vanished during the rebuild are propagated in
/// `vanished_output_paths` so the pipeline can prune their HTML files
/// (issue #804 P2).
///
/// `embed_v8`-gated: discovery needs the embedded V8 host.
#[cfg(feature = "embed_v8")]
fn make_discovery_hook(
    session: DevRenderSession,
    html_root: PathBuf,
    // Issue #807 — the live SSR routes handle. The discovery refresh marks
    // the renderer fresh, so the pipeline skips `reload_renderer`; we must
    // rewrite the handle HERE or a newly-created `prerender = false` route
    // 404s until a later edit. `None` when the project has no SSR.
    ssr_routes: Option<SsrRoutesHandle>,
    // Issue #1581 — the session-live known-content registry. A file that
    // discovery ACCEPTS becomes an already-known entry, so a later in-place
    // edit of it that FSEvents coalesces into `Created` normalizes back to
    // `Modified` instead of re-entering the discovery regime.
    known_content: zfb_build::KnownContentEntries,
) -> zfb_build::DiscoveryHook {
    let mut relevant_roots: Vec<PathBuf> = vec![
        session.inner.project_root.join("content"),
        session.inner.project_root.join("pages"),
    ];
    // #1550 — use the boot inventory's resolved collection roots (canonical
    // for out-of-root collections). The created-file check below is a
    // lexical `p.starts_with(root)` against a CANONICAL notify event path, so
    // an out-of-root collection's literal `../x` root would never match and a
    // newly-created external post stayed invisible until a `zfb dev` restart.
    for root in &session.inner.collection_roots {
        if !relevant_roots.contains(root) {
            relevant_roots.push(root.clone());
        }
    }
    Arc::new(move |created: &[PathBuf]| {
        // Only created files under content/, pages/, or a collection
        // root can introduce a new content-collection route; skip the
        // re-bundle otherwise.
        let relevant: Vec<PathBuf> = created
            .iter()
            .filter(|p| relevant_roots.iter().any(|root| p.starts_with(root)))
            .cloned()
            .collect();
        if relevant.is_empty() {
            return Ok(DiscoveryOutcome::default());
        }

        let (changed, vanished_rel) = session.discover_created(&relevant)?;

        // #1581 — discovery succeeded, so re-derive the collection membership
        // and register it. Registering here (and NOT at the top of the
        // closure) keeps the registry meaning "entries of the last SUCCESSFUL
        // collection walk": a `discover_created` that fails returns early
        // above and leaves the registry untouched, so the next tick still
        // treats the file as new.
        //
        // This MUST re-walk rather than insert the raw event paths in
        // `relevant`. The watcher can report a created DIRECTORY (its children
        // never surface as individual events), in which case `relevant` holds
        // only the directory — registering that would leave every entry
        // beneath it unknown, so the next FSEvents-coalesced `Created` for one
        // of those children would be mistaken for a new file all over again.
        // `relevant` is also broader than the collection (it admits `pages/`
        // and `content/` paths that are not collection members at all); the
        // membership walk is the authoritative set.
        known_content.insert_many(collect_collection_entries(
            &session.inner.rebuild_inputs.cfg,
            &session.inner.collection_roots,
        ));

        // Issue #807 — the discovery refresh swapped in a fresh V8 host and
        // rebuilt the route tables, but it reports `renderer_reloaded: true`,
        // so the pipeline will SKIP `reload_renderer` for this tick. That
        // closure is the only OTHER place the live SSR route set is rewritten,
        // so without this call a newly-created `prerender = false` page never
        // reaches the request dispatcher and 404s until a later edit. Rewrite
        // the handle here via the same `make_ssr_route_set` path.
        if let Some(handle) = &ssr_routes {
            refresh_live_ssr_routes(&session, handle);
        }

        // Content edges are reconciled from actual worker reads after this
        // tick's render batch. Do not infer them from raw watcher paths here:
        // a create can add a collection member for every aggregate reader,
        // while an arbitrary `pages/` create may not be content at all.
        // Map relative vanished output paths to absolute dist paths so the
        // orchestrator can forward them to the pipeline's prune loop.
        let vanished_abs: Vec<PathBuf> = vanished_rel
            .into_iter()
            .map(|rel| html_root.join(rel))
            .collect();

        Ok(DiscoveryOutcome {
            pages: changed,
            renderer_reloaded: true,
            vanished_output_paths: vanished_abs,
        })
    })
}

/// Build the initial live [`SsrRoutesHandle`] for the dev server from the
/// dev session (issue #367 / Gap 1, live-update issue #807).
///
/// Returns `None` when the dev session is absent (renderer disabled —
/// the SSR layer would have no V8 host to dispatch through). When every
/// page is SSG (no `prerender = false` routes at boot), the handle still
/// wraps `None` so the request dispatcher does nothing but a later refresh
/// can populate it if the user adds a `prerender = false` route mid-session.
///
/// The handle is an `Arc<RwLock<Option<SsrRouteSet>>>`. The per-tick
/// renderer reload callback holds a clone of this `Arc` and writes a fresh
/// `SsrRouteSet` into it after each bundle refresh — adding or removing
/// `prerender = false` routes mid-session is reflected on the next request
/// without a dev-server restart (issue #807).
///
/// The dispatcher is constructed once from the session's renderer handle
/// and reused across refreshes (the `Arc<Mutex<Option<RendererState>>>`
/// it wraps is the same one the refresh swaps the new host into, so
/// request-time SSR automatically sees the new bundle).
///
/// Compiled in only when the `embed_v8` feature is on (issue #371,
/// sub-task 4.1a) — the SSR adapter requires the V8 host.
#[cfg(feature = "embed_v8")]
fn build_ssr_route_set(session: Option<&DevRenderSession>) -> Option<SsrRoutesHandle> {
    let session = session?;
    let renderer_handle = session.renderer_handle();
    let dispatcher: Arc<dyn SsrDispatcher> = Arc::new(
        crate::ssr_adapter::EmbeddedV8SsrAdapter::new(renderer_handle),
    );
    let initial_set = make_ssr_route_set(session, Arc::clone(&dispatcher));
    Some(Arc::new(std::sync::RwLock::new(initial_set)))
}

/// Build an [`SsrRouteSet`] from the dev session's current SSR patterns.
///
/// Returns `None` when the session has zero `prerender = false` routes,
/// which the request dispatcher treats identically to "no SSR configured."
/// Called at boot (inside [`build_ssr_route_set`]) and after each tick's
/// renderer reload to refresh the live handle (issue #807).
#[cfg(feature = "embed_v8")]
fn make_ssr_route_set(
    session: &DevRenderSession,
    dispatcher: Arc<dyn SsrDispatcher>,
) -> Option<SsrRouteSet> {
    let patterns = session.ssr_patterns();
    if patterns.is_empty() {
        return None;
    }
    let records = patterns
        .into_iter()
        .map(|pattern| SsrRouteRecord { pattern })
        .collect();
    Some(SsrRouteSet::new(records, dispatcher))
}

/// Rewrite the live [`SsrRoutesHandle`] from the dev session's CURRENT SSR
/// patterns (issue #807). Builds a fresh dispatcher over the session's
/// renderer handle and a fresh [`SsrRouteSet`] via [`make_ssr_route_set`],
/// then swaps it into the `RwLock`.
///
/// Shared by BOTH refresh seams so they stay in lock-step:
/// - the per-tick `reload_renderer` (in-place EDIT ticks), and
/// - the watch-ADD discovery hook (`make_discovery_hook`).
///
/// Without the discovery-hook call site, a newly-created `prerender = false`
/// page 404s until a later edit: the discovery refresh marks the renderer
/// fresh, so the pipeline skips `reload_renderer` and the live handle is
/// never updated for that tick.
#[cfg(feature = "embed_v8")]
fn refresh_live_ssr_routes(session: &DevRenderSession, handle: &SsrRoutesHandle) {
    let fresh_dispatcher = {
        let renderer_handle = session.renderer_handle();
        Arc::new(crate::ssr_adapter::EmbeddedV8SsrAdapter::new(
            renderer_handle,
        )) as Arc<dyn SsrDispatcher>
    };
    let new_set = make_ssr_route_set(session, fresh_dispatcher);
    if let Ok(mut lock) = handle.write() {
        *lock = new_set;
    }
}

/// Translate Hono-style colon-syntax templates emitted by
/// `zfb_router::Route::template()` into the `pages/`-style bracket
/// grammar consumed by `zfb_server::SsrRouteSet`'s matcher (which calls
/// into `crate::injected_routes::pattern_matches`).
///
/// Grammar:
/// - `:name`        → `[name]`        (single-segment dynamic param)
/// - `:name{.+}`    → `[...name]`     (catchall — Hono regex quantifier)
/// - `:name{.+}?`   → `[[...name]]`   (optional catchall — zero or more)
/// - literal segments are preserved unchanged
///
/// The root `/` is preserved as `/`.
fn colon_template_to_bracket(template: &str) -> String {
    let segments: Vec<&str> = template.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return "/".to_string();
    }
    let mut out = String::with_capacity(template.len() + 4);
    for seg in &segments {
        out.push('/');
        if let Some(rest) = seg.strip_prefix(':') {
            // Optional catchall (`:name{.+}?`) wins over required
            // (`:name{.+}`), which wins over single-segment.
            if let Some(name) = rest.strip_suffix("{.+}?") {
                out.push_str("[[...");
                out.push_str(name);
                out.push_str("]]");
            } else if let Some(name) = rest.strip_suffix("{.+}") {
                out.push_str("[...");
                out.push_str(name);
                out.push(']');
            } else {
                out.push('[');
                out.push_str(rest);
                out.push(']');
            }
        } else {
            out.push_str(seg);
        }
    }
    out
}

const DEFAULT_DEV_HOST: &str = "localhost";
const DEFAULT_DEV_PORT: u16 = 3000;

/// Test-only stub session factory for the lazy render adapter's
/// direct-invocation tests (issue #1026) — builds a [`DevRenderSession`]
/// over caller-supplied route entries and renderer state, with the
/// reverse URL index derived exactly like boot does
/// ([`build_url_index`]), so `lookup_by_url` behaves identically to a
/// real session. Lives outside `mod tests` because the adapter's test
/// module (a sibling module of `commands`) needs to call it.
#[cfg(all(test, feature = "embed_v8"))]
pub(crate) fn stub_session_for_adapter_tests(
    project_root: PathBuf,
    routes: Vec<(PathBuf, Vec<RouteUniverseEntry>)>,
    renderer: Arc<Mutex<Option<RendererState>>>,
    lazy_render: bool,
) -> DevRenderSession {
    let routes_by_source: HashMap<PathBuf, Vec<DevRouteEntry>> = routes
        .into_iter()
        .map(|(source, entries)| {
            (
                source,
                entries
                    .into_iter()
                    .map(|entry| DevRouteEntry {
                        entry,
                        params: None,
                    })
                    .collect(),
            )
        })
        .collect();
    let url_index = build_url_index(&routes_by_source);
    DevRenderSession {
        inner: Arc::new(DevRenderInner {
            routes: std::sync::RwLock::new(DevRouteTables {
                routes_by_source,
                ssr_routes: Vec::new(),
                url_index,
            }),
            renderer,
            project_root,
            rebuild_inputs: DevRebuildInputs {
                cfg: config::Config::default(),
                v8_plugin_hooks: zfb_render::PluginRegistryHooks::default(),
                plugin_alias_entries: Vec::new(),
                plugin_virtual_modules: Vec::new(),
                esbuild: None,
                injected_pages_root: None,
                empty_user_pages_root: None,
            },
            // Default config carries no collections (#1550).
            collection_roots: Vec::new(),
            last_successful_skip_key: Mutex::new(None),
            fm_hashes: Mutex::new(HashMap::new()),
            shadow_session: Mutex::new(None),
            dep_graph: Mutex::new(None),
            content_trace: Mutex::new(DevContentTraceState::default()),
            out_of_root_watch_targets: Mutex::new(std::collections::BTreeSet::new()),
            boot_route_module_deps: Vec::new(),
            paths_cache: Mutex::new(PathsCache::new()),
            stale: Mutex::new(StaleRoutes::default()),
            lazy_render,
            // Stubs model a session mid-flight: boot already rendered.
            boot_render_done: std::sync::atomic::AtomicBool::new(true),
            // S3 (#1231) — adapter tests inject routes directly; no injected
            // package routes in the stub universe.
            injected_static_seeds: Vec::new(),
            injected_route_set: InjectedRouteSet::default(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[cfg(feature = "embed_v8")]
    #[test]
    fn zero_pages_without_injected_routes_keeps_missing_pages_error() {
        let project = tempfile::tempdir().unwrap();
        let err = resolve_dev_user_pages_root(project.path(), false).unwrap_err();

        assert!(
            err.to_string().contains("no pages/ directory"),
            "a project with neither user nor injected routes must retain the historical error: {err:#}"
        );
        assert!(
            !project.path().join("pages").exists(),
            "the failing path must not create a consumer pages directory"
        );
    }

    #[cfg(feature = "embed_v8")]
    #[test]
    fn zero_pages_with_surviving_injected_routes_gets_internal_empty_root() {
        let project = tempfile::tempdir().unwrap();
        let (internal_pages, guard) = resolve_dev_user_pages_root(project.path(), true).unwrap();

        assert!(
            internal_pages.is_dir(),
            "the router/bundler fallback root must be a usable directory"
        );
        assert!(
            guard.is_some(),
            "the internal root must stay alive for the dev session"
        );
        assert_ne!(
            internal_pages,
            project.path().join("pages"),
            "the internal root must not alias the consumer's missing pages directory"
        );
        assert!(
            !project.path().join("pages").exists(),
            "zero-pages support must not create a user-visible pages directory"
        );
    }

    #[cfg(feature = "embed_v8")]
    #[test]
    fn conventional_empty_pages_without_injection_keeps_no_routes_error() {
        let project = tempfile::tempdir().unwrap();
        let pages = project.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();
        let router = zfb_router::Router::scan(&pages).unwrap();
        let plan = build_route_universe(router.routes());

        let err = ensure_dev_routes_available(&plan, &InjectedRouteSet::default()).unwrap_err();

        assert!(
            err.to_string().contains("no routes to render"),
            "an existing but empty conventional pages directory must retain the historical error: {err:#}"
        );
    }

    // `resolve_host` / `resolve_addr` live in `crate::commands::resolve` (shared
    // with `preview`); their precedence and binding tests live there too.

    /// Wrap bare [`RouteUniverseEntry`]s in provenance-free
    /// [`DevRouteEntry`]s (issue #958) for tests that don't exercise
    /// narrowing.
    fn no_params(entries: Vec<RouteUniverseEntry>) -> Vec<DevRouteEntry> {
        entries
            .into_iter()
            .map(|entry| DevRouteEntry {
                entry,
                params: None,
            })
            .collect()
    }

    /// Build a stub [`DevRenderInner`] for the route-plumbing seam tests
    /// (no live V8 host). The discovery (#659) `rebuild_inputs` are filled
    /// with defaults — these tests never call `discover_created`.
    #[cfg(feature = "embed_v8")]
    fn stub_dev_inner(
        routes_by_source: HashMap<PathBuf, Vec<DevRouteEntry>>,
        ssr_routes: Vec<RouteUniverseEntry>,
    ) -> DevRenderInner {
        stub_dev_inner_at(
            PathBuf::new(),
            config::Config::default(),
            routes_by_source,
            ssr_routes,
        )
    }

    /// [`stub_dev_inner`] with an explicit project root + config, for the
    /// content-narrowing tests (issue #958) that need real collection
    /// files on disk.
    #[cfg(feature = "embed_v8")]
    fn stub_dev_inner_at(
        project_root: PathBuf,
        cfg: config::Config,
        routes_by_source: HashMap<PathBuf, Vec<DevRouteEntry>>,
        ssr_routes: Vec<RouteUniverseEntry>,
    ) -> DevRenderInner {
        let url_index = build_url_index(&routes_by_source);
        // #1550 — mirror the real boot: resolve per-collection roots from the
        // same inventory so narrowing / discovery tests see canonical
        // out-of-root roots. Computed before `cfg` is moved into
        // `rebuild_inputs`.
        let collection_roots = resolve_roots(&project_root, &cfg)
            .collection_roots()
            .to_vec();
        DevRenderInner {
            routes: std::sync::RwLock::new(DevRouteTables {
                routes_by_source,
                ssr_routes,
                url_index,
            }),
            renderer: Arc::new(Mutex::new(None)),
            project_root,
            rebuild_inputs: DevRebuildInputs {
                cfg,
                v8_plugin_hooks: zfb_render::PluginRegistryHooks::default(),
                plugin_alias_entries: Vec::new(),
                plugin_virtual_modules: Vec::new(),
                esbuild: None,
                injected_pages_root: None,
                empty_user_pages_root: None,
            },
            collection_roots,
            last_successful_skip_key: Mutex::new(None),
            fm_hashes: Mutex::new(HashMap::new()),
            shadow_session: Mutex::new(None),
            dep_graph: Mutex::new(None),
            content_trace: Mutex::new(DevContentTraceState::default()),
            out_of_root_watch_targets: Mutex::new(std::collections::BTreeSet::new()),
            boot_route_module_deps: Vec::new(),
            paths_cache: Mutex::new(PathsCache::new()),
            stale: Mutex::new(StaleRoutes::default()),
            lazy_render: false,
            // Stubs model a session mid-flight: boot already rendered.
            boot_render_done: std::sync::atomic::AtomicBool::new(true),
            // S3 (#1231) — route-plumbing seam tests inject routes directly.
            injected_static_seeds: Vec::new(),
            injected_route_set: InjectedRouteSet::default(),
        }
    }

    /// V8-off counterpart of [`stub_dev_inner`] — no `rebuild_inputs`
    /// field exists when `embed_v8` is disabled.
    #[cfg(not(feature = "embed_v8"))]
    fn stub_dev_inner(
        routes_by_source: HashMap<PathBuf, Vec<DevRouteEntry>>,
        ssr_routes: Vec<RouteUniverseEntry>,
    ) -> DevRenderInner {
        let url_index = build_url_index(&routes_by_source);
        DevRenderInner {
            routes: std::sync::RwLock::new(DevRouteTables {
                routes_by_source,
                ssr_routes,
                url_index,
            }),
            renderer: Arc::new(Mutex::new(None)),
            project_root: PathBuf::new(),
            stale: Mutex::new(StaleRoutes::default()),
            lazy_render: false,
            // Stubs model a session mid-flight: boot already rendered.
            boot_render_done: std::sync::atomic::AtomicBool::new(true),
            // S3 (#1231) — route-plumbing seam tests inject routes directly.
            injected_static_seeds: Vec::new(),
            injected_route_set: InjectedRouteSet::default(),
        }
    }

    #[test]
    fn default_watch_roots_includes_zfb_config_json() {
        assert!(DEFAULT_WATCH_ROOTS.contains(&"zfb.config.json"));
        assert!(DEFAULT_WATCH_ROOTS.contains(&"zfb.config.ts"));
    }

    /// S5 (epic #1228, #1233) — node_modules is deliberately excluded from
    /// the watch roots. Injected-route entrypoints live under
    /// `node_modules/@takazudo/…` and are **restart-only**: editing the
    /// package's own source requires a `zfb dev` restart. Content the route
    /// READS (watched collections) DOES live-refresh via the per-swap
    /// stale-mark (`mark_injected_seeds_stale`). The negative half of the HMR
    /// contract is asserted here at the cheapest level (unit-logic) rather than
    /// via a full dev E2E boot: no watcher event for a `node_modules` change is
    /// the correct and observable precondition; a live-boot test cannot reliably
    /// distinguish "no event within N seconds" from "event arrived too late".
    #[test]
    fn default_watch_roots_excludes_node_modules() {
        for root in DEFAULT_WATCH_ROOTS {
            assert!(
                !root.contains("node_modules"),
                "DEFAULT_WATCH_ROOTS must not watch node_modules \
                 (injected entrypoints are restart-only — S5 / epic #1228 §4); \
                 found: {root:?}"
            );
        }
    }

    /// S5 (epic #1228, #1233) — `derive_watch_roots` must not introduce
    /// `node_modules` entries even when collections are configured. Verifies
    /// the composed list still excludes any node_modules-adjacent path.
    #[test]
    fn derive_watch_roots_never_adds_node_modules() {
        // Use a config with a custom collection path; derive_watch_roots must
        // not inject node_modules roots even when extending the default set.
        let cfg = config::Config {
            collections: vec![config::CollectionDef {
                name: "posts".into(),
                path: PathBuf::from("content/posts"),
                schema: None,
                include: None,
                exclude: None,
                id_strip_suffix: None,
                allow_outside_root: false,
            }],
            ..Default::default()
        };
        let roots = derive_watch_roots(&cfg);
        for root in &roots {
            let s = root.to_string_lossy();
            assert!(
                !s.contains("node_modules"),
                "derive_watch_roots must never emit a node_modules root (restart-only — S5); \
                 got: {root:?}"
            );
        }
    }

    /// #994 item B — the PathsCache is caller-owned and persists across
    /// route-table builds: a second build sharing the cache with
    /// unchanged `paths()` JSON reports cache hits (delta counters) and
    /// produces identical tables.
    #[test]
    #[cfg(feature = "embed_v8")]
    fn build_dev_route_tables_shares_paths_cache_across_calls() {
        let dir = tempfile::tempdir().unwrap();
        let pages = dir.path().join("pages");
        std::fs::create_dir_all(pages.join("blog")).unwrap();
        std::fs::write(
            pages.join("index.tsx"),
            "export default function Home() { return null; }",
        )
        .unwrap();
        std::fs::write(
            pages.join("blog").join("[slug].tsx"),
            r#"
            export function paths() {
                return [
                    { params: { slug: "hello" } },
                    { params: { slug: "world" } },
                ];
            }
            export default function P() { return null; }
            "#,
        )
        .unwrap();

        let router = zfb_router::Router::scan(&pages).unwrap();
        let plan = build_route_universe(router.routes());
        // No live V8 host: the literal `paths()` resolves in phase 1, so
        // the renderer mutex is never borrowed (see the phase-2 gate in
        // `build_dev_route_tables_inner`).
        let renderer: Arc<Mutex<Option<RendererState>>> = Arc::new(Mutex::new(None));

        let mut cache = PathsCache::new();
        let (tables1, ssr1, idx1, hits1, misses1) =
            build_dev_route_tables_timed(&router, &plan, dir.path(), &renderer, &mut cache)
                .expect("first build");
        assert_eq!(hits1, 0, "first build runs against a cold cache");
        assert!(misses1 > 0, "first build must record a miss");
        assert_eq!(
            tables1
                .get(&pages.join("blog").join("[slug].tsx"))
                .map(Vec::len),
            Some(2),
            "literal paths() expands to 2 entries: {tables1:?}"
        );

        let (tables2, ssr2, idx2, hits2, misses2) =
            build_dev_route_tables_timed(&router, &plan, dir.path(), &renderer, &mut cache)
                .expect("second build");
        assert!(
            hits2 > 0,
            "unchanged paths() JSON must hit the shared cache"
        );
        assert_eq!(misses2, 0, "second build must add no misses");
        assert_eq!(
            tables1, tables2,
            "cache-sharing builds must produce identical tables"
        );
        assert_eq!(ssr1, ssr2);
        // The url_index must be consistent across both builds for the same input.
        assert_eq!(
            idx1.keys().collect::<std::collections::BTreeSet<_>>(),
            idx2.keys().collect::<std::collections::BTreeSet<_>>(),
            "url_index keys must be identical across cache-sharing builds"
        );
    }

    fn cfg_with_collections(paths: &[&str]) -> config::Config {
        config::Config {
            collections: paths
                .iter()
                .map(|p| config::CollectionDef {
                    name: p.replace('/', "-"),
                    path: PathBuf::from(p),
                    schema: None,
                    include: None,
                    exclude: None,
                    id_strip_suffix: None,
                    allow_outside_root: false,
                })
                .collect(),
            ..config::Config::default()
        }
    }

    /// Regression: a collection configured outside the default watch
    /// roots (e.g. `articles/notes`) was never watched, so edits there
    /// produced no rebuild and the dev server served stale HTML until
    /// restart. Discovered during usage in a consumer project with
    /// custom collection paths.
    ///
    /// Uses `articles/*` (outside every default root) because `src` is now
    /// itself a default watch root (#1284) — a `src/**` collection would be
    /// collapsed into `src` rather than appended (see
    /// `derive_watch_roots_collapses_src_collection_into_src_default_root`).
    #[test]
    fn derive_watch_roots_appends_custom_collection_paths() {
        let cfg = cfg_with_collections(&["articles/notes", "articles/guides"]);
        let roots = derive_watch_roots(&cfg);
        assert!(roots.contains(&PathBuf::from("articles/notes")));
        assert!(roots.contains(&PathBuf::from("articles/guides")));
        // Defaults are preserved in front.
        assert!(roots.contains(&PathBuf::from("pages")));
        assert!(roots.contains(&PathBuf::from("content")));
    }

    /// #1284 — `src` is now a default watch root, so a collection declared
    /// under `src/**` is already covered by the recursive `src` watch and
    /// must collapse into it rather than register a redundant overlapping
    /// root (which would deliver duplicate events for the same write).
    #[test]
    fn derive_watch_roots_collapses_src_collection_into_src_default_root() {
        let cfg = cfg_with_collections(&["src/mdx/notes", "src/mdx"]);
        let roots = derive_watch_roots(&cfg);
        assert!(roots.contains(&PathBuf::from("src")));
        assert!(!roots.contains(&PathBuf::from("src/mdx")));
        assert!(!roots.contains(&PathBuf::from("src/mdx/notes")));
        assert_eq!(
            roots.len(),
            DEFAULT_WATCH_ROOTS.len(),
            "a src-nested collection adds no watch root — `src` already covers it"
        );
    }

    /// Issue #1165 — `public/` must NOT appear in the default watch roots.
    /// It is served directly from disk by the dev server and does not feed
    /// the dep-graph or renderer. Walking it via WalkDir in
    /// `compute_manifest_digest` caused a visible pre-bind hang on projects
    /// with large asset directories. A custom `publicDir` that happens to
    /// overlap a real source root is still watched (via collection/source
    /// root derivation) — only the literal default `"public"` is excluded.
    #[test]
    fn derive_watch_roots_excludes_public_by_default() {
        let cfg = config::Config::default();
        let roots = derive_watch_roots(&cfg);
        assert!(
            !roots.contains(&PathBuf::from("public")),
            "default `public/` must not be a watch root (issue #1165)"
        );
    }

    /// A collection under the default `content/` root must NOT be
    /// re-registered — overlapping recursive watches deliver duplicate
    /// events for the same write.
    #[test]
    fn derive_watch_roots_skips_paths_covered_by_default_roots() {
        let cfg = cfg_with_collections(&["content/blog"]);
        let roots = derive_watch_roots(&cfg);
        assert!(!roots.contains(&PathBuf::from("content/blog")));
        assert_eq!(
            roots.len(),
            DEFAULT_WATCH_ROOTS.len(),
            "covered collection path must not add a watch root"
        );
    }

    /// Nested collections collapse to the shallowest ancestor regardless
    /// of declaration order, and duplicates dedupe.
    ///
    /// Uses `articles/*` (outside every default root) so the nested-collapse
    /// logic is exercised on a path that is genuinely appended — `src/**`
    /// would instead collapse into the `src` default root (#1284).
    #[test]
    fn derive_watch_roots_collapses_nested_and_duplicate_collections() {
        let cfg = cfg_with_collections(&["articles/notes", "articles", "articles/notes"]);
        let roots = derive_watch_roots(&cfg);
        assert!(roots.contains(&PathBuf::from("articles")));
        assert!(!roots.contains(&PathBuf::from("articles/notes")));
        assert_eq!(roots.len(), DEFAULT_WATCH_ROOTS.len() + 1);
    }

    /// Leading `./` is normalized away so `./articles` and `articles`
    /// compare equal in the dedupe/coverage checks.
    #[test]
    fn derive_watch_roots_normalizes_leading_curdir() {
        let cfg = cfg_with_collections(&["./articles", "articles"]);
        let roots = derive_watch_roots(&cfg);
        assert!(roots.contains(&PathBuf::from("articles")));
        assert_eq!(roots.len(), DEFAULT_WATCH_ROOTS.len() + 1);
    }

    // ── resolved-root inventory / out-of-root collections (#1550) ─────────

    /// Build a config with one collection at `path`, flagged
    /// `allowOutsideRoot` (the wave-1 #1549 opt-in that lets an out-of-root
    /// path pass config validation in the first place).
    fn cfg_out_of_root(path: &str) -> config::Config {
        config::Config {
            collections: vec![config::CollectionDef {
                name: "external".into(),
                path: PathBuf::from(path),
                schema: None,
                include: None,
                exclude: None,
                id_strip_suffix: None,
                allow_outside_root: true,
            }],
            ..config::Config::default()
        }
    }

    #[test]
    fn collection_path_escapes_root_detects_escapes() {
        assert!(collection_path_escapes_root(Path::new("../shared")));
        assert!(collection_path_escapes_root(Path::new("../../a/b")));
        assert!(collection_path_escapes_root(Path::new("a/../../b"))); // net escape
        assert!(collection_path_escapes_root(Path::new("/abs/path")));
        assert!(!collection_path_escapes_root(Path::new("content/blog")));
        assert!(!collection_path_escapes_root(Path::new("src/mdx/notes")));
        assert!(!collection_path_escapes_root(Path::new("a/../b"))); // stays in-root
        assert!(!collection_path_escapes_root(Path::new("")));
    }

    #[test]
    fn lexical_normalize_collapses_parent_dir() {
        assert_eq!(
            lexical_normalize(Path::new("/home/user/proj/../shared/x")),
            PathBuf::from("/home/user/shared/x"),
        );
        // Already-normal absolute path is unchanged.
        assert_eq!(
            lexical_normalize(Path::new("/a/b/c")),
            PathBuf::from("/a/b/c"),
        );
    }

    /// An out-of-root (`..`-escaping) collection path must NOT appear in the
    /// relative watch-root list — it rides the absolute extras channel.
    #[test]
    fn derive_watch_roots_excludes_out_of_root_collection() {
        let cfg = cfg_out_of_root("../shared-content");
        let roots = derive_watch_roots(&cfg);
        assert!(!roots.contains(&PathBuf::from("../shared-content")));
        assert_eq!(
            roots.len(),
            DEFAULT_WATCH_ROOTS.len(),
            "an out-of-root collection adds no RELATIVE watch root"
        );
    }

    /// An IN-root collection keeps its pre-#1550 shape: relative watch root
    /// present, no out-of-root entry, and its resolved `collection_roots`
    /// value is the literal `project_root.join(path)` (byte-identical).
    #[test]
    fn resolve_roots_in_root_collection_is_byte_identical() {
        let base = tempfile::tempdir().unwrap();
        let project_root = base.path().canonicalize().unwrap();
        let cfg = cfg_with_collections(&["articles/notes"]);
        let inv = resolve_roots(&project_root, &cfg);

        assert!(inv
            .relative_watch_roots()
            .contains(&PathBuf::from("articles/notes")));
        assert!(
            inv.out_of_root_watch_roots().is_empty(),
            "an in-root collection contributes no extras-channel root"
        );
        assert_eq!(
            inv.collection_roots(),
            [project_root.join("articles/notes")].as_slice(),
            "in-root collection root stays the literal join (pre-#1550 form)"
        );
    }

    /// The heart of #1550: an out-of-root collection resolves to a CANONICAL
    /// ABSOLUTE root that a canonical `notify` event path (what the OS
    /// delivers on macOS symlinked-tmp, and after any symlink resolution)
    /// actually prefixes — while the old literal `project_root.join("../x")`
    /// form does not. Proves the tick-narrowing / discovery / seed matching
    /// at the shared `derive_slug_for_file` level without a V8 host.
    #[test]
    fn resolve_roots_out_of_root_root_matches_canonical_event_path() {
        let base = tempfile::tempdir().unwrap();
        let base = base.path().canonicalize().unwrap();
        let project_root = base.join("proj");
        let external = base.join("shared"); // == ../shared from proj
        std::fs::create_dir_all(project_root.join("pages")).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        let post = external.join("post.md");
        std::fs::write(&post, "---\ntitle: hi\n---\nbody\n").unwrap();

        let cfg = cfg_out_of_root("../shared");
        let inv = resolve_roots(&project_root, &cfg);

        // Routed to the extras channel, absolute + canonical.
        let root = &inv.collection_roots()[0];
        assert_eq!(inv.out_of_root_watch_roots(), std::slice::from_ref(root));
        assert!(root.is_absolute());
        assert_eq!(root, &external, "root must be the canonical external dir");

        // The canonical event path the watcher delivers on an edit.
        let event_path = post.canonicalize().unwrap();
        assert!(
            event_path.starts_with(root),
            "canonical event path {event_path:?} must be under the canonical root {root:?}"
        );

        let filter = zfb_content::collection::CollectionFilter::new(None, None, None).unwrap();
        // The site's actual matcher succeeds with the inventory root...
        assert!(
            zfb_content::collection::derive_slug_for_file(root, &event_path, &filter).is_some(),
            "tick/seed/discovery must resolve a slug for a canonical event path"
        );
        // ...and FAILS with the old literal `project_root.join("../x")` root,
        // which is the bug #1550 fixes (falsifiability guard).
        let literal_root = project_root.join("../shared");
        assert!(
            zfb_content::collection::derive_slug_for_file(&literal_root, &event_path, &filter)
                .is_none(),
            "the pre-#1550 literal `..` root must NOT match a canonical event path"
        );
    }

    /// Frontmatter-hash SEEDING keys the map by the WalkDir-derived path.
    /// With a canonical collection root, those keys equal the canonical
    /// paths `notify` later delivers — so an edit hits the seeded entry
    /// instead of tripping the narrowing gate. (Non-V8 proof of the seed
    /// key/event-path alignment `seed_frontmatter_hashes` relies on.)
    #[test]
    fn resolve_roots_out_of_root_walk_keys_match_canonical_event_paths() {
        let base = tempfile::tempdir().unwrap();
        let base = base.path().canonicalize().unwrap();
        let project_root = base.join("proj");
        let external = base.join("shared");
        std::fs::create_dir_all(&external).unwrap();
        let post = external.join("post.md");
        std::fs::write(&post, "---\ntitle: hi\n---\n").unwrap();

        let inv = resolve_roots(&project_root, &cfg_out_of_root("../shared"));
        let root = &inv.collection_roots()[0];

        let walked: Vec<PathBuf> = walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .flatten()
            .filter(|e| e.file_type().is_file())
            .map(|e| e.path().to_path_buf())
            .collect();
        assert_eq!(
            walked,
            vec![post.canonicalize().unwrap()],
            "seed keys (WalkDir over the canonical root) must equal canonical event paths"
        );
    }

    /// Two collections resolving to the same canonical directory register a
    /// single extras-channel root (dedupe of overlapping collections).
    #[test]
    fn resolve_roots_dedups_overlapping_out_of_root_collections() {
        let base = tempfile::tempdir().unwrap();
        let base = base.path().canonicalize().unwrap();
        let project_root = base.join("proj");
        let external = base.join("shared");
        std::fs::create_dir_all(&external).unwrap();

        let cfg = config::Config {
            collections: vec![
                config::CollectionDef {
                    name: "a".into(),
                    path: PathBuf::from("../shared"),
                    schema: None,
                    include: None,
                    exclude: None,
                    id_strip_suffix: None,
                    allow_outside_root: true,
                },
                config::CollectionDef {
                    name: "b".into(),
                    // ./ prefix + same target ⇒ same canonical dir.
                    path: PathBuf::from(".././shared"),
                    schema: None,
                    include: None,
                    exclude: None,
                    id_strip_suffix: None,
                    allow_outside_root: true,
                },
            ],
            ..config::Config::default()
        };
        let inv = resolve_roots(&project_root, &cfg);
        assert_eq!(
            inv.out_of_root_watch_roots(),
            std::slice::from_ref(&external),
            "overlapping out-of-root collections must not double-register"
        );
        // collection_roots stays index-aligned (one per collection).
        assert_eq!(inv.collection_roots().len(), 2);
    }

    /// A not-yet-created out-of-root dir cannot be canonicalised; the
    /// inventory must fall back to a lexical absolute path (never panic /
    /// drop the root), so the missing-target warning and digest still see a
    /// stable path.
    #[test]
    fn resolve_roots_out_of_root_missing_dir_falls_back_to_lexical() {
        let base = tempfile::tempdir().unwrap();
        let base = base.path().canonicalize().unwrap();
        let project_root = base.join("proj");
        std::fs::create_dir_all(&project_root).unwrap();
        // `../not-yet` does not exist.
        let inv = resolve_roots(&project_root, &cfg_out_of_root("../not-yet"));
        let root = &inv.collection_roots()[0];
        assert!(root.is_absolute());
        assert_eq!(root, &base.join("not-yet"));
        assert!(
            !root.components().any(|c| c == Component::ParentDir),
            "lexical fallback must collapse the `..`"
        );
        assert_eq!(inv.out_of_root_watch_roots(), std::slice::from_ref(root));
    }

    /// The manifest digest must fold in the canonical out-of-root roots
    /// (they left `watch_roots` when routed to the extras channel), and an
    /// external-content edit must change the digest — otherwise a stale
    /// `.zfb/graph.bin` could be reused after an out-of-tree content change.
    #[test]
    fn manifest_digest_changes_on_external_content_edit() {
        let base = tempfile::tempdir().unwrap();
        let base = base.path().canonicalize().unwrap();
        let project_root = base.join("proj");
        let external = base.join("shared");
        std::fs::create_dir_all(&project_root).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        let post = external.join("post.md");
        std::fs::write(&post, "a").unwrap();

        let inv = resolve_roots(&project_root, &cfg_out_of_root("../shared"));
        let roots = inv.manifest_digest_roots();
        assert!(
            roots.contains(&external),
            "digest roots must include the canonical out-of-root dir"
        );

        let d1 = compute_manifest_digest(&project_root, &roots).expect("digest 1");
        // Length-changing edit guarantees a different (mtime+len) fingerprint.
        std::fs::write(&post, "aa-changed").unwrap();
        let d2 = compute_manifest_digest(&project_root, &roots).expect("digest 2");
        assert_ne!(
            d1, d2,
            "editing an out-of-root collection file must flip the manifest digest"
        );
    }

    /// Issue #1391 — a configured-but-missing derived watch root (e.g.
    /// `content/` never created) AND a missing extra watch path must both
    /// be reported so the boot path can warn the user. A present root and
    /// a present extra path must NOT show up.
    #[test]
    fn missing_watch_targets_flags_absent_root_and_extra_path() {
        let dir = tempfile::tempdir().unwrap();
        // `content` is a derived watch root that does not exist under `dir`.
        let watch_roots = vec![PathBuf::from("content"), PathBuf::from("pages")];
        std::fs::create_dir_all(dir.path().join("pages")).unwrap();

        let present_extra = dir.path().join("present-extra");
        std::fs::create_dir_all(&present_extra).unwrap();
        let missing_extra = dir.path().join("missing-extra");
        let extra_watch_paths = vec![present_extra, missing_extra.clone()];

        let missing = missing_watch_targets(dir.path(), &watch_roots, &extra_watch_paths);

        assert_eq!(
            missing,
            vec![dir.path().join("content"), missing_extra],
            "must flag exactly the missing root and the missing extra path"
        );
    }

    /// Everything present ⇒ no warnings — the common case (a freshly
    /// scaffolded project with all default roots created) must not spam
    /// the console.
    #[test]
    fn missing_watch_targets_empty_when_everything_exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("content")).unwrap();
        let missing = missing_watch_targets(dir.path(), &[PathBuf::from("content")], &[]);
        assert!(missing.is_empty());
    }

    /// Issue #1391 — the mutually-exclusive `zfb.config.*` entries in
    /// `DEFAULT_WATCH_ROOTS` must NOT be reported as missing: at least
    /// one is always absent, so warning about them would spam every boot
    /// and bury the real content/source-dir signal. Passing the real
    /// default roots against an empty project must surface the missing
    /// *directories* but never the config files.
    #[test]
    fn missing_watch_targets_never_flags_config_files() {
        let dir = tempfile::tempdir().unwrap();
        // Empty project: no default dirs, no config files exist.
        let roots: Vec<PathBuf> = DEFAULT_WATCH_ROOTS.iter().map(PathBuf::from).collect();
        let missing = missing_watch_targets(dir.path(), &roots, &[]);
        for m in &missing {
            let name = m.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            assert!(
                !WATCH_WARN_SKIP.contains(&name),
                "config file {m:?} must never be reported as a missing watch target"
            );
        }
        // Sanity: the directory roots ARE still reported (e.g. `content`).
        assert!(
            missing.contains(&dir.path().join("content")),
            "a missing content/ dir must still be flagged; got {missing:?}"
        );
    }

    /// Issue #534 regression — dev's per-route HTML output dir must live
    /// under `.zfb-build/`, NOT under the project's `outDir` (`dist/`).
    /// If this contract is broken, `pnpm dev` after a clean `pnpm build`
    /// silently overwrites prod HTML in `dist/`, stripping the prod-only
    /// `<link>` / islands `<script type="module">` head injections and
    /// breaking the subsequent `pnpm preview`.
    ///
    /// Falsifiability: changing the helper to return `dist/dev-pages`
    /// or `dist` itself fails the second / third assertion.
    #[test]
    fn dev_html_root_lives_under_dot_zfb_build_not_outdir() {
        let project_root = PathBuf::from("/tmp/proj");
        let dev_html_root = dev_html_root_for(&project_root);

        // The exact, documented contract:
        //   `<project_root>/.zfb-build/dev-pages`.
        assert_eq!(
            dev_html_root,
            PathBuf::from("/tmp/proj/.zfb-build/dev-pages"),
        );

        // Negative checks against the regressed locations. The default
        // `outDir` is `dist/`; assert the dev path does not collide
        // anywhere under it.
        let dist_root = project_root.join("dist");
        assert_ne!(dev_html_root, dist_root, "must not equal outDir");
        assert!(
            !dev_html_root.starts_with(&dist_root),
            "dev html dir must not live anywhere under outDir ({}); got {}",
            dist_root.display(),
            dev_html_root.display(),
        );
    }

    /// Issue #1189: dev's STABLE served assets must NOT live under the
    /// project's `outDir` (`dist/`) — otherwise a one-off `zfb build` wipes
    /// them and the dev server 404s `/assets/styles.css`. The relocation
    /// target is a dedicated `.zfb-build/dev-assets` dir, distinct from both
    /// `outDir` and the dev-HTML root.
    #[test]
    fn dev_assets_root_lives_under_dot_zfb_build_not_outdir() {
        let project_root = PathBuf::from("/tmp/proj");
        let dev_assets_root = dev_assets_root_for(&project_root);

        // The exact, documented contract: `<project_root>/.zfb-build/dev-assets`.
        assert_eq!(
            dev_assets_root,
            PathBuf::from("/tmp/proj/.zfb-build/dev-assets"),
        );

        // Must not collide with `outDir` (the bug) ...
        let dist_root = project_root.join("dist");
        assert_ne!(dev_assets_root, dist_root, "must not equal outDir");
        assert!(
            !dev_assets_root.starts_with(&dist_root) && !dist_root.starts_with(&dev_assets_root),
            "dev assets dir must not overlap outDir ({}); got {}",
            dist_root.display(),
            dev_assets_root.display(),
        );

        // ... nor with the dev-HTML root (they're siblings under .zfb-build/).
        assert_ne!(
            dev_assets_root,
            dev_html_root_for(&project_root),
            "dev assets and dev html roots must be distinct"
        );
    }

    /// The render callback must:
    /// 1. Be tolerant of genuinely-unknown page ids (a source path that
    ///    maps to no `RouteUniverseEntry` at all) — return an empty list,
    ///    never error.
    /// 2. NOT silently drop a source path that DOES map to entries.
    ///
    /// Contract change (#502/#507): previously every dynamic route was
    /// dropped here because `routes_by_source` only held static routes.
    /// Now a `[slug]` SSG source resolves (via boot-time `paths()`
    /// expansion) to N entries under its source path, so the callback
    /// must fan those out — only a source path absent from the map is
    /// silently dropped. We use a `None` renderer so the absent-key path
    /// returns `Ok(empty)` without ever reaching the lock.
    #[test]
    fn render_callback_drops_unknown_pages_silently() {
        let session = DevRenderSession {
            inner: Arc::new(stub_dev_inner(HashMap::new(), Vec::new())),
        };
        let cb = make_render_callback(session, PathBuf::from("/tmp/dist"));
        // A source path not present in routes_by_source is still dropped.
        let pages = vec![PageId::new(PathBuf::from("pages/index.tsx"))];
        let out = cb(&pages, None).unwrap();
        assert!(out.is_empty());
    }

    /// A dynamic SSG source whose `paths()` expanded into N concrete URLs
    /// fans out through `render_one_with`'s loop: one `RenderedPage` per
    /// entry, each with a synthetic `PageId` derived from the entry's
    /// `output_path` (guardrail 1, #507).
    ///
    /// Uses the `render_one_with` seam directly with a stub closure that
    /// writes known HTML to a tempdir, so the test drives the real loop
    /// without a live V8 `RendererState`. N=3 entries guard against
    /// hard-coded two-item expectations.
    ///
    /// Falsifiability:
    /// - If the loop exits early (e.g. a `break` after one entry), the
    ///   `out.len() == 3` assertion fails.
    /// - If the synthetic `PageId` is derived from the source path instead
    ///   of `entry.output_path`, all three ids are identical and the
    ///   `unique.len() == 3` assertion fails.
    #[test]
    fn dynamic_ssg_source_fans_out_to_multiple_entries_with_distinct_ids() {
        use std::collections::HashSet;
        use tempfile::tempdir;

        let tmp = tempdir().expect("tempdir");

        // Three slugs — one dynamic SSG source → three concrete URLs.
        let slugs = ["hello", "world", "rust"];
        let entries: Vec<RouteUniverseEntry> = slugs
            .iter()
            .map(|slug| RouteUniverseEntry {
                url_path: format!("/blog/{slug}"),
                output_path: PathBuf::from(format!("blog/{slug}/index.html")),
                route_key: "/blog/:slug".into(),
                static_html: false,
                source_path: None,
            })
            .collect();

        let source_page = PageId::new(PathBuf::from("pages/blog/[slug].tsx"));

        // Stub render-fn: create the output file in the tempdir and return
        // its absolute path — exercises the read_to_string round-trip.
        let tmp_path = tmp.path().to_path_buf();
        let out = DevRenderSession::render_one_with(&source_page, &entries, |entry| {
            let dest = tmp_path.join(&entry.output_path);
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::write(&dest, format!("<html>{}</html>", entry.url_path)).unwrap();
            Ok(dest)
        })
        .expect("render_one_with must succeed");

        // One RenderedPage per entry.
        assert_eq!(out.len(), 3, "fan-out must emit one page per entry");

        // Every synthetic PageId is derived from the entry's output_path.
        for (rendered, entry) in out.iter().zip(entries.iter()) {
            assert_eq!(
                rendered.page,
                PageId::new(entry.output_path.clone()),
                "PageId must be derived from entry.output_path, not source path"
            );
            // Verify the loop actually read what the stub wrote.
            assert!(
                rendered.html.contains(&entry.url_path),
                "html must reflect what the stub renderer wrote for {}",
                entry.url_path
            );
        }

        // All synthetic ids are distinct (guardrail 1 property).
        let unique: HashSet<_> = out.iter().map(|r| &r.page).collect();
        assert_eq!(
            unique.len(),
            3,
            "each resolved URL must get a distinct pipeline page id"
        );
    }

    /// On a known page id, but a `None` renderer (boot half-failed),
    /// the callback logs an error and returns an empty list — the
    /// watcher must keep going.
    #[test]
    fn render_callback_keeps_watcher_alive_on_render_error() {
        let mut routes: HashMap<PathBuf, Vec<DevRouteEntry>> = HashMap::new();
        routes.insert(
            PathBuf::from("pages/index.tsx"),
            no_params(vec![RouteUniverseEntry {
                url_path: "/".into(),
                output_path: PathBuf::from("index.html"),
                route_key: "/".into(),
                static_html: false,
                source_path: None,
            }]),
        );
        let session = DevRenderSession {
            inner: Arc::new(stub_dev_inner(routes, Vec::new())),
        };
        let cb = make_render_callback(session, PathBuf::from("/tmp/dist"));
        let pages = vec![PageId::new(PathBuf::from("pages/index.tsx"))];
        let out = cb(&pages, None).unwrap();
        assert!(out.is_empty(), "errors must yield empty list, not panic");
    }

    /// Content-edit narrowing seam tests (issue #958, locked-spec §11).
    ///
    /// These drive `compute_tick_narrowing` + `filter_entries` — the two
    /// seams a narrowed tick traverses before the (untouched)
    /// `render_one_with` fan-out — against real collection files in a
    /// tempdir, exactly as the dev render callback composes them.
    #[cfg(feature = "embed_v8")]
    mod narrowing {
        use super::*;
        use crate::render_pipeline::ResolvedRouteParams;
        use std::collections::BTreeMap;

        fn route_entry(url: &str, out: &str, key: &str) -> RouteUniverseEntry {
            RouteUniverseEntry {
                url_path: url.into(),
                output_path: PathBuf::from(out),
                route_key: key.into(),
                static_html: false,
                source_path: None,
            }
        }

        fn scalar_params(pairs: &[(&str, &str)]) -> ResolvedRouteParams {
            ResolvedRouteParams {
                scalars: pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                arrays: BTreeMap::new(),
            }
        }

        fn array_params(key: &str, parts: &[&str]) -> ResolvedRouteParams {
            ResolvedRouteParams {
                scalars: BTreeMap::new(),
                arrays: BTreeMap::from([(
                    key.to_string(),
                    parts.iter().map(|s| s.to_string()).collect(),
                )]),
            }
        }

        fn with_params(entry: RouteUniverseEntry, params: ResolvedRouteParams) -> DevRouteEntry {
            DevRouteEntry {
                entry,
                params: Some(params),
            }
        }

        /// Project scaffold: tempdir root + one `blog` collection under
        /// `content/blog` with the given `(relative name, frontmatter)`
        /// entry files.
        fn scaffold(files: &[(&str, &str)]) -> (tempfile::TempDir, config::Config) {
            let tmp = tempfile::tempdir().unwrap();
            for (name, fm) in files {
                write_entry(tmp.path(), name, fm, "body");
            }
            let cfg = config::Config {
                collections: vec![config::CollectionDef {
                    name: "blog".into(),
                    path: PathBuf::from("content/blog"),
                    schema: None,
                    include: None,
                    exclude: None,
                    id_strip_suffix: None,
                    allow_outside_root: false,
                }],
                ..config::Config::default()
            };
            (tmp, cfg)
        }

        fn write_entry(project_root: &Path, name: &str, fm: &str, body: &str) -> PathBuf {
            let path = project_root.join("content/blog").join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, format!("---\n{fm}\n---\n\n{body}\n")).unwrap();
            path
        }

        /// #1581 — the `known_content` registry is seeded from this walk, and
        /// the discovery hook RE-walks rather than registering the watcher's
        /// raw event paths. Both rely on the walk being authoritative:
        ///
        /// - It must descend into NESTED directories. The watcher can report a
        ///   created DIRECTORY whose children never surface as individual
        ///   events; registering only the event path would leave those children
        ///   unknown, so the next FSEvents-coalesced `Created` for one of them
        ///   would be mistaken for a new file and lose the tick its narrowing.
        /// - It must yield FILES only, never the directories it walks through.
        #[test]
        fn collect_collection_entries_descends_into_nested_dirs_and_yields_files_only() {
            let (tmp, cfg) = scaffold(&[("top.md", "title: Top"), ("deep/nested.md", "title: N")]);
            let collection_roots = resolve_roots(tmp.path(), &cfg).collection_roots().to_vec();

            let entries = collect_collection_entries(&cfg, &collection_roots);

            let blog = tmp.path().join("content/blog");
            assert!(
                entries.contains(&blog.join("top.md")),
                "top-level entry must be walked; got {entries:?}"
            );
            assert!(
                entries.contains(&blog.join("deep/nested.md")),
                "an entry inside a NESTED dir must be walked — this is the case a \
                 created-directory watcher event cannot enumerate; got {entries:?}"
            );
            assert!(
                !entries.contains(&blog.join("deep")),
                "the walk must yield files only, never the directories it descends"
            );
        }

        /// #1581 — the registry takes the FULL membership, unlike
        /// [`seed_frontmatter_hashes`] which keeps only what it could hash. An
        /// entry with unparseable frontmatter is still an already-known entry:
        /// dropping it would let a spurious FSEvents `Created` for that file
        /// fall through to the discovery regime.
        #[test]
        fn collect_collection_entries_includes_entries_whose_frontmatter_is_unparseable() {
            let (tmp, cfg) = scaffold(&[("good.md", "title: Good")]);
            let blog = tmp.path().join("content/blog");
            let broken = blog.join("broken.md");
            std::fs::write(&broken, "---\n: : not: valid: yaml\n---\n\nbody\n").unwrap();
            let collection_roots = resolve_roots(tmp.path(), &cfg).collection_roots().to_vec();

            let entries = collect_collection_entries(&cfg, &collection_roots);
            assert!(
                entries.contains(&broken),
                "membership does not depend on frontmatter parsing; got {entries:?}"
            );

            let hashed = seed_frontmatter_hashes(&cfg, &collection_roots);
            assert!(
                !hashed.contains_key(&broken),
                "the frontmatter seed, by contrast, skips what it cannot hash — this \
                 divergence is exactly why the registry must not be keyed off it"
            );
            assert!(hashed.contains_key(&blog.join("good.md")));
        }

        /// Stub session with boot-seeded frontmatter hashes, like
        /// `boot_dev_renderer` produces.
        fn session_at(
            tmp: &tempfile::TempDir,
            cfg: config::Config,
            routes: HashMap<PathBuf, Vec<DevRouteEntry>>,
        ) -> DevRenderSession {
            // #1550 — seed against the same resolved collection roots the
            // session's inner will carry (see `stub_dev_inner_at`).
            let collection_roots = resolve_roots(tmp.path(), &cfg).collection_roots().to_vec();
            let seeded = seed_frontmatter_hashes(&cfg, &collection_roots);
            let inner = stub_dev_inner_at(tmp.path().to_path_buf(), cfg, routes, Vec::new());
            *inner.fm_hashes.lock().unwrap() = seeded;
            DevRenderSession {
                inner: Arc::new(inner),
            }
        }

        fn hint_for(paths: &[PathBuf]) -> zfb_build::ContentNarrowing {
            zfb_build::ContentNarrowing {
                changed_content: paths.to_vec(),
                // These unit tests exercise `compute_tick_narrowing` /
                // `compute_lazy_eager_sets` directly; neither reads
                // `fan_out_safe` (the eager-path gate lives in
                // `make_render_callback`). Value is immaterial here.
                fan_out_safe: true,
            }
        }

        /// Run the narrowed filter for `source` against the session's
        /// tables and return the surviving output paths — the set
        /// `render_one` would fan out (its `render_one_with` loop is
        /// byte-identical pre- and post-#958).
        fn surviving_outputs(
            session: &DevRenderSession,
            narrowing: &TickNarrowing,
            source: &Path,
        ) -> Vec<PathBuf> {
            let filter = match narrowing {
                TickNarrowing::Off => &RouteFilter::All,
                TickNarrowing::PerSource(map) => map.get(source).unwrap_or(&RouteFilter::All),
            };
            let tables = session.inner.routes.read().unwrap();
            DevRenderSession::filter_entries(&tables.routes_by_source[source], filter)
                .into_iter()
                .map(|e| e.output_path)
                .collect()
        }

        /// Epic acceptance test 1: a body edit of entry A renders A's
        /// route plus the static (always-rendered) source — NOT sibling
        /// B's route.
        #[test]
        fn narrowed_content_edit_renders_only_matching_routes_plus_statics() {
            let (tmp, cfg) = scaffold(&[("a.mdx", "title: A"), ("b.mdx", "title: B")]);
            let dynamic_source = PathBuf::from("pages/blog/[slug].tsx");
            let static_source = PathBuf::from("pages/index.tsx");
            let mut routes = HashMap::new();
            routes.insert(
                dynamic_source.clone(),
                vec![
                    with_params(
                        route_entry("/blog/a", "blog/a/index.html", "/blog/:slug"),
                        scalar_params(&[("slug", "a")]),
                    ),
                    with_params(
                        route_entry("/blog/b", "blog/b/index.html", "/blog/:slug"),
                        scalar_params(&[("slug", "b")]),
                    ),
                ],
            );
            routes.insert(
                static_source.clone(),
                no_params(vec![route_entry("/", "index.html", "/")]),
            );
            let session = session_at(&tmp, cfg, routes);

            // Body-only edit of A (frontmatter byte-identical).
            let edited = write_entry(tmp.path(), "a.mdx", "title: A", "updated body");

            let narrowing = compute_tick_narrowing(&session, Some(&hint_for(&[edited])));
            assert!(
                matches!(narrowing, TickNarrowing::PerSource(_)),
                "a seeded body-only edit must narrow; got {narrowing:?}"
            );

            assert_eq!(
                surviving_outputs(&session, &narrowing, &dynamic_source),
                vec![PathBuf::from("blog/a/index.html")],
                "dynamic source must render exactly the edited entry's route"
            );
            assert_eq!(
                surviving_outputs(&session, &narrowing, &static_source),
                vec![PathBuf::from("index.html")],
                "the static source must stay in the render set in full (S1)"
            );
            if let TickNarrowing::PerSource(map) = &narrowing {
                assert!(
                    !map.contains_key(&static_source),
                    "always-rendered sources are expressed by ABSENCE from the map"
                );
            }
        }

        /// Epic acceptance test 2 / S2: a dynamic source whose params are
        /// not slug-shaped (tag pages) never matches the edited entry's
        /// candidates and must fall back to its FULL fan-out — that is
        /// the aggregate-page freshness mechanism.
        #[test]
        fn zero_match_dynamic_source_falls_back_to_full_fanout() {
            let (tmp, cfg) = scaffold(&[("a.mdx", "title: A")]);
            let blog_source = PathBuf::from("pages/blog/[slug].tsx");
            let tags_source = PathBuf::from("pages/tags/[tag].tsx");
            let mut routes = HashMap::new();
            routes.insert(
                blog_source.clone(),
                vec![with_params(
                    route_entry("/blog/a", "blog/a/index.html", "/blog/:slug"),
                    scalar_params(&[("slug", "a")]),
                )],
            );
            routes.insert(
                tags_source.clone(),
                vec![
                    with_params(
                        route_entry("/tags/rust", "tags/rust/index.html", "/tags/:tag"),
                        scalar_params(&[("tag", "rust")]),
                    ),
                    with_params(
                        route_entry("/tags/cli", "tags/cli/index.html", "/tags/:tag"),
                        scalar_params(&[("tag", "cli")]),
                    ),
                ],
            );
            let session = session_at(&tmp, cfg, routes);

            let edited = write_entry(tmp.path(), "a.mdx", "title: A", "updated");
            let narrowing = compute_tick_narrowing(&session, Some(&hint_for(&[edited])));

            let mut tag_outputs = surviving_outputs(&session, &narrowing, &tags_source);
            tag_outputs.sort();
            assert_eq!(
                tag_outputs,
                vec![
                    PathBuf::from("tags/cli/index.html"),
                    PathBuf::from("tags/rust/index.html"),
                ],
                "zero-match source must keep its full fan-out (S2)"
            );
            // The slug-shaped source still narrows on the same tick.
            assert_eq!(
                surviving_outputs(&session, &narrowing, &blog_source),
                vec![PathBuf::from("blog/a/index.html")],
            );
        }

        /// S1: a source where ANY entry lacks params provenance (zip-
        /// length mismatch at table-build time files `params: None`)
        /// must render in full even when another entry would match.
        #[test]
        fn missing_params_provenance_forces_source_full_fanout() {
            let (tmp, cfg) = scaffold(&[("a.mdx", "title: A")]);
            let source = PathBuf::from("pages/blog/[slug].tsx");
            let mut routes = HashMap::new();
            routes.insert(
                source.clone(),
                vec![
                    with_params(
                        route_entry("/blog/a", "blog/a/index.html", "/blog/:slug"),
                        scalar_params(&[("slug", "a")]),
                    ),
                    DevRouteEntry {
                        entry: route_entry("/blog/b", "blog/b/index.html", "/blog/:slug"),
                        params: None,
                    },
                ],
            );
            let session = session_at(&tmp, cfg, routes);

            let edited = write_entry(tmp.path(), "a.mdx", "title: A", "updated");
            let narrowing = compute_tick_narrowing(&session, Some(&hint_for(&[edited])));

            let mut outputs = surviving_outputs(&session, &narrowing, &source);
            outputs.sort();
            assert_eq!(
                outputs,
                vec![
                    PathBuf::from("blog/a/index.html"),
                    PathBuf::from("blog/b/index.html"),
                ],
                "missing provenance on any entry must force the source's full fan-out (S1)"
            );
        }

        /// G4, changed direction: a frontmatter delta disables narrowing
        /// for the tick (frontmatter feeds cross-page props) — but the
        /// new hash is stored on the Off path, so the NEXT body-only
        /// edit narrows.
        #[test]
        fn frontmatter_change_disables_narrowing_for_tick() {
            let (tmp, cfg) = scaffold(&[("a.mdx", "title: A")]);
            let source = PathBuf::from("pages/blog/[slug].tsx");
            let mut routes = HashMap::new();
            routes.insert(
                source.clone(),
                vec![with_params(
                    route_entry("/blog/a", "blog/a/index.html", "/blog/:slug"),
                    scalar_params(&[("slug", "a")]),
                )],
            );
            let session = session_at(&tmp, cfg, routes);

            let edited = write_entry(tmp.path(), "a.mdx", "title: A (renamed)", "body");
            let first =
                compute_tick_narrowing(&session, Some(&hint_for(std::slice::from_ref(&edited))));
            assert!(
                matches!(first, TickNarrowing::Off),
                "a frontmatter change must disable narrowing for the tick (G4); got {first:?}"
            );

            // Same file, body-only follow-up: the hash stored on the Off
            // path makes this tick narrow.
            let edited = write_entry(tmp.path(), "a.mdx", "title: A (renamed)", "body v2");
            let second = compute_tick_narrowing(&session, Some(&hint_for(&[edited])));
            assert!(
                matches!(second, TickNarrowing::PerSource(_)),
                "the hash must be stored on the Off path so the next body edit narrows"
            );
        }

        /// G4, missing direction: a file with no seeded hash (boot
        /// seeding failed / session-created file) renders in full on its
        /// first edit and narrows from the second on.
        #[test]
        fn first_edit_without_seeded_hash_renders_full_then_narrows() {
            let (tmp, cfg) = scaffold(&[]);
            let source = PathBuf::from("pages/blog/[slug].tsx");
            let mut routes = HashMap::new();
            routes.insert(
                source.clone(),
                vec![with_params(
                    route_entry("/blog/new", "blog/new/index.html", "/blog/:slug"),
                    scalar_params(&[("slug", "new")]),
                )],
            );
            // Session seeded BEFORE the file exists — like a file created
            // mid-session through the discovery path.
            let session = session_at(&tmp, cfg, routes);
            let created = write_entry(tmp.path(), "new.mdx", "title: New", "body");

            let first =
                compute_tick_narrowing(&session, Some(&hint_for(std::slice::from_ref(&created))));
            assert!(
                matches!(first, TickNarrowing::Off),
                "first edit without a seeded hash must render in full (G4); got {first:?}"
            );

            let created = write_entry(tmp.path(), "new.mdx", "title: New", "body v2");
            let second = compute_tick_narrowing(&session, Some(&hint_for(&[created])));
            assert!(
                matches!(second, TickNarrowing::PerSource(_)),
                "second (body-only) edit must narrow once the hash is seeded"
            );
        }

        /// Spec §10: `x/index.mdx` must match catchall params `["x"]`
        /// via the `/index`-stripped candidate, and a root `index.mdx`
        /// must match the bare root-index `[]` (joins to `""`).
        #[test]
        fn index_entry_slug_variants_match_catchall_params() {
            let (tmp, cfg) = scaffold(&[("x/index.mdx", "title: X"), ("index.mdx", "title: Root")]);
            let source = PathBuf::from("pages/docs/[[...slug]].tsx");
            let mut routes = HashMap::new();
            routes.insert(
                source.clone(),
                vec![
                    with_params(
                        route_entry("/docs/x", "docs/x/index.html", "/docs/:slug{.+}?"),
                        array_params("slug", &["x"]),
                    ),
                    with_params(
                        route_entry("/docs", "docs/index.html", "/docs/:slug{.+}?"),
                        array_params("slug", &[]),
                    ),
                ],
            );
            let session = session_at(&tmp, cfg, routes);

            let edited = write_entry(tmp.path(), "x/index.mdx", "title: X", "v2");
            let narrowing = compute_tick_narrowing(&session, Some(&hint_for(&[edited])));
            assert_eq!(
                surviving_outputs(&session, &narrowing, &source),
                vec![PathBuf::from("docs/x/index.html")],
                "x/index.mdx must match the [\"x\"] catchall route"
            );

            let edited = write_entry(tmp.path(), "index.mdx", "title: Root", "v2");
            let narrowing = compute_tick_narrowing(&session, Some(&hint_for(&[edited])));
            assert_eq!(
                surviving_outputs(&session, &narrowing, &source),
                vec![PathBuf::from("docs/index.html")],
                "root index.mdx must match the empty-catchall route"
            );
        }

        /// Spec §10: `idStripSuffix` applies inside the slug derivation,
        /// so `post.en.mdx` (suffix `.en`) matches params slug `post`.
        #[test]
        fn id_strip_suffix_slug_matches_params() {
            let (tmp, mut cfg) = scaffold(&[("post.en.mdx", "title: Post")]);
            cfg.collections[0].id_strip_suffix = Some(".en".into());
            let source = PathBuf::from("pages/blog/[slug].tsx");
            let mut routes = HashMap::new();
            routes.insert(
                source.clone(),
                vec![
                    with_params(
                        route_entry("/blog/post", "blog/post/index.html", "/blog/:slug"),
                        scalar_params(&[("slug", "post")]),
                    ),
                    with_params(
                        route_entry("/blog/other", "blog/other/index.html", "/blog/:slug"),
                        scalar_params(&[("slug", "other")]),
                    ),
                ],
            );
            let session = session_at(&tmp, cfg, routes);

            let edited = write_entry(tmp.path(), "post.en.mdx", "title: Post", "v2");
            let narrowing = compute_tick_narrowing(&session, Some(&hint_for(&[edited])));
            assert_eq!(
                surviving_outputs(&session, &narrowing, &source),
                vec![PathBuf::from("blog/post/index.html")],
                "the idStripSuffix-stripped slug must match the route params"
            );
        }

        /// Spec §4 step 3: a frontmatter `slug:` override contributes a
        /// candidate (verbatim + leading-`/`-stripped), so a body edit of
        /// a file whose route URL comes from frontmatter still narrows.
        #[test]
        fn frontmatter_slug_override_body_edit_narrows_via_fm_candidate() {
            let (tmp, cfg) = scaffold(&[("weird-file-name.mdx", "title: C\nslug: /custom/path")]);
            let source = PathBuf::from("pages/docs/[[...slug]].tsx");
            let mut routes = HashMap::new();
            routes.insert(
                source.clone(),
                vec![
                    with_params(
                        route_entry(
                            "/docs/custom/path",
                            "docs/custom/path/index.html",
                            "/docs/:slug{.+}?",
                        ),
                        array_params("slug", &["custom", "path"]),
                    ),
                    with_params(
                        route_entry("/docs/other", "docs/other/index.html", "/docs/:slug{.+}?"),
                        array_params("slug", &["other"]),
                    ),
                ],
            );
            let session = session_at(&tmp, cfg, routes);

            let edited = write_entry(
                tmp.path(),
                "weird-file-name.mdx",
                "title: C\nslug: /custom/path",
                "v2",
            );
            let narrowing = compute_tick_narrowing(&session, Some(&hint_for(&[edited])));
            assert_eq!(
                surviving_outputs(&session, &narrowing, &source),
                vec![PathBuf::from("docs/custom/path/index.html")],
                "the frontmatter slug candidate must match the override route"
            );
        }

        /// Review finding on #958 (G5 gate integrity): the refresh diff's
        /// `changed` set must compare full route-entry SETS, not entry
        /// counts — a `paths()` refresh that replaces a route with a new
        /// URL at the same cardinality must mark the source changed, or
        /// a narrowed tick could skip the brand-new route.
        #[test]
        fn diff_route_tables_flags_same_count_route_replacement() {
            let source = PathBuf::from("pages/blog/[slug].tsx");
            let old_entries = vec![with_params(
                route_entry("/blog/x", "blog/x/index.html", "/blog/:slug"),
                scalar_params(&[("slug", "x")]),
            )];
            let new_entries = vec![with_params(
                route_entry("/blog/y", "blog/y/index.html", "/blog/:slug"),
                scalar_params(&[("slug", "y")]),
            )];
            let old = HashMap::from([(source.clone(), old_entries)]);
            let new = HashMap::from([(source.clone(), new_entries)]);

            let (changed, vanished) = diff_route_tables(&old, &new);
            assert_eq!(
                changed,
                vec![PageId::new(source)],
                "a same-count URL replacement must mark the source changed (G5)"
            );
            assert_eq!(
                vanished,
                vec![PathBuf::from("blog/x/index.html")],
                "the replaced output path globally vanished"
            );
        }

        /// `diff_route_tables` reports identical tables as unchanged, and
        /// the vanished diff stays GLOBAL: a route moving between sources
        /// (#727 two-page swap) flags both sources changed but vanishes
        /// nothing.
        #[test]
        fn diff_route_tables_identity_and_cross_source_swap() {
            let src_a = PathBuf::from("pages/a/[slug].tsx");
            let src_b = PathBuf::from("pages/b/[slug].tsx");
            let entry_x = with_params(
                route_entry("/x", "x/index.html", "/a/:slug"),
                scalar_params(&[("slug", "x")]),
            );
            let entry_y = with_params(
                route_entry("/y", "y/index.html", "/b/:slug"),
                scalar_params(&[("slug", "y")]),
            );

            let old = HashMap::from([
                (src_a.clone(), vec![entry_x.clone()]),
                (src_b.clone(), vec![entry_y.clone()]),
            ]);

            // Identity: nothing changed, nothing vanished.
            let (changed, vanished) = diff_route_tables(&old, &old.clone());
            assert!(changed.is_empty(), "identical tables must diff empty");
            assert!(vanished.is_empty());

            // Swap: A now serves /y, B now serves /x.
            let new = HashMap::from([
                (src_a.clone(), vec![entry_y]),
                (src_b.clone(), vec![entry_x]),
            ]);
            let (mut changed, vanished) = diff_route_tables(&old, &new);
            changed.sort_by(|a, b| a.path().cmp(b.path()));
            assert_eq!(changed, vec![PageId::new(src_a), PageId::new(src_b)]);
            assert!(
                vanished.is_empty(),
                "globally-live paths swapped between sources must not vanish"
            );
        }

        /// G2: a Content-classified file outside every configured
        /// collection (e.g. a bare `content/`-segment file with no
        /// matching collection) must disable narrowing for the tick.
        #[test]
        fn file_outside_every_collection_disables_narrowing() {
            let (tmp, cfg) = scaffold(&[("a.mdx", "title: A")]);
            let source = PathBuf::from("pages/blog/[slug].tsx");
            let mut routes = HashMap::new();
            routes.insert(
                source.clone(),
                vec![with_params(
                    route_entry("/blog/a", "blog/a/index.html", "/blog/:slug"),
                    scalar_params(&[("slug", "a")]),
                )],
            );
            let session = session_at(&tmp, cfg, routes);

            let outside = tmp.path().join("content/notes/x.md");
            std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
            std::fs::write(&outside, "---\ntitle: X\n---\n\nbody\n").unwrap();

            let narrowing = compute_tick_narrowing(&session, Some(&hint_for(&[outside])));
            assert!(
                matches!(narrowing, TickNarrowing::Off),
                "a file outside every collection must fall back to full fan-out (G2)"
            );
        }

        /// Staleness model seam tests (issue #1025): the claim/clear
        /// protocol on [`StaleRoutes`] and the lazy eager-vs-stale split
        /// (`compute_lazy_eager_sets` + `lazy_render_tick`), driven
        /// against real collection files exactly like the narrowing
        /// tests above. The orchestrator-layer tick-class matrix lives
        /// in `crates/zfb-build/tests/integration_lazy_staleness.rs`.
        mod stale_model {
            use super::*;

            /// [`session_at`] with the lazy-render switch forced ON.
            fn lazy_session_at(
                tmp: &tempfile::TempDir,
                cfg: config::Config,
                routes: HashMap<PathBuf, Vec<DevRouteEntry>>,
            ) -> DevRenderSession {
                // #1550 — seed against the same resolved collection roots the
                // session's inner will carry (see `stub_dev_inner_at`).
                let collection_roots = resolve_roots(tmp.path(), &cfg).collection_roots().to_vec();
                let seeded = seed_frontmatter_hashes(&cfg, &collection_roots);
                let mut inner =
                    stub_dev_inner_at(tmp.path().to_path_buf(), cfg, routes, Vec::new());
                *inner.fm_hashes.lock().unwrap() = seeded;
                inner.lazy_render = true;
                DevRenderSession {
                    inner: Arc::new(inner),
                }
            }

            fn bare_inner() -> DevRenderInner {
                stub_dev_inner(HashMap::new(), Vec::new())
            }

            fn out(p: &str) -> PathBuf {
                PathBuf::from(p)
            }

            // ── claim / clear_if_current protocol ────────────────────

            /// The wave-5 activation flip (issue #1027): lazy dev
            /// rendering is the default. Reverting the constant to
            /// `false` must fail loudly in review.
            // The constant IS the subject under test.
            #[allow(clippy::assertions_on_constants)]
            #[test]
            fn lazy_dev_render_default_is_on() {
                assert!(
                    LAZY_DEV_RENDER_DEFAULT,
                    "the lazy dev-render switch defaults ON since the \
                     #1027 activation flip"
                );
            }

            /// Issue #1027 — env precedence for the boot-resolved switch:
            /// `ZFB_LAZY_DEV_RENDER` (precise override) beats
            /// `ZFB_DEV_EAGER` (user-facing escape hatch) beats the
            /// compile-time default. Pure-function tests so no
            /// process-global env mutation races other tests.
            #[test]
            fn lazy_switch_env_precedence() {
                // Default: lazy ON.
                assert!(resolve_lazy_dev_render(None, None));
                // The escape hatch turns it off.
                assert!(!resolve_lazy_dev_render(None, Some("1")));
                assert!(!resolve_lazy_dev_render(None, Some("true")));
                // Unrecognized escape-hatch values are ignored (0 does
                // NOT mean "force lazy"; it just falls through).
                assert!(resolve_lazy_dev_render(None, Some("0")));
                assert!(resolve_lazy_dev_render(None, Some("yes")));
                // The precise override wins in both directions...
                assert!(!resolve_lazy_dev_render(Some("0"), None));
                assert!(!resolve_lazy_dev_render(Some("false"), None));
                assert!(resolve_lazy_dev_render(Some("1"), None));
                // ...including over a conflicting escape hatch.
                assert!(resolve_lazy_dev_render(Some("1"), Some("1")));
                assert!(!resolve_lazy_dev_render(Some("0"), Some("1")));
                // An unrecognized precise-override value defers to the
                // escape hatch, then the default.
                assert!(!resolve_lazy_dev_render(Some("banana"), Some("1")));
                assert!(resolve_lazy_dev_render(Some("banana"), None));
            }

            /// Issue #1057 — boot-lazy switch resolution: truthy only for
            /// `1`/`true` (case/whitespace-insensitive); everything else
            /// (unset, `0`, unrecognized) is off.
            #[test]
            fn boot_lazy_switch_resolution() {
                assert!(!resolve_boot_lazy(None));
                assert!(!resolve_boot_lazy(Some("0")));
                assert!(!resolve_boot_lazy(Some("false")));
                assert!(!resolve_boot_lazy(Some("banana")));
                assert!(resolve_boot_lazy(Some("1")));
                assert!(resolve_boot_lazy(Some("true")));
                assert!(resolve_boot_lazy(Some(" TRUE ")));
            }

            /// Issue #1057 — boot-lazy REQUIRES lazy rendering (it reuses the
            /// request-time render-on-request hook, installed only when lazy
            /// is on). With lazy off the mode is force-disabled even when the
            /// env asks for it.
            #[test]
            fn boot_lazy_requires_lazy_rendering() {
                // lazy on + env on => on.
                assert!(boot_lazy_decision(true, Some("1")));
                // lazy OFF + env on => OFF (the key invariant).
                assert!(!boot_lazy_decision(false, Some("1")));
                assert!(!boot_lazy_decision(false, Some("true")));
                // lazy on but env off/unset => off.
                assert!(!boot_lazy_decision(true, None));
                assert!(!boot_lazy_decision(true, Some("0")));
            }

            /// Issue #1182 — the eager dev bundle is deferred past
            /// `TcpListener::bind` ONLY when boot-lazy is active AND a servable
            /// `dist/` seed is present AND the #1188 opt-out is not engaged. All
            /// three conjuncts are load-bearing: drop boot-lazy and there is no
            /// request-time render-on-request hook to recover the deferred
            /// renderer; drop the servable seed and there is nothing to serve
            /// during the pre-renderer window; engage the opt-out and an SSR-heavy
            /// project asked for the eager pre-bind renderer. The gate is
            /// `boot_lazy_decision(..) && dist_servable && resolve_defer_bundle(..)`.
            /// The 4th arg is the `ZFB_DEV_DEFER_BUNDLE` value (`None` = default on).
            #[test]
            fn defer_dev_bundle_requires_boot_lazy_and_servable_dist() {
                // boot-lazy on (lazy on + env on) + servable dist + default opt-in => defer.
                assert!(defer_dev_bundle_decision(true, Some("1"), true, None));
                assert!(defer_dev_bundle_decision(true, Some("true"), true, None));
                // explicit opt-in value is still a defer.
                assert!(defer_dev_bundle_decision(true, Some("1"), true, Some("1")));
                // boot-lazy on but NO servable dist => eager (no safe seed).
                assert!(!defer_dev_bundle_decision(true, Some("1"), false, None));
                // servable dist but boot-lazy OFF (env off) => eager.
                assert!(!defer_dev_bundle_decision(true, None, true, None));
                assert!(!defer_dev_bundle_decision(true, Some("0"), true, None));
                // servable dist + env on but lazy rendering OFF => eager
                // (no render-on-request hook is installed).
                assert!(!defer_dev_bundle_decision(false, Some("1"), true, None));
                // neither => eager.
                assert!(!defer_dev_bundle_decision(false, None, false, None));

                // #1188 opt-out: boot-lazy + servable but ZFB_DEV_DEFER_BUNDLE=0|false
                // => eager pre-bind renderer (no SSR-only 404 window).
                assert!(!defer_dev_bundle_decision(true, Some("1"), true, Some("0")));
                assert!(!defer_dev_bundle_decision(
                    true,
                    Some("1"),
                    true,
                    Some("false")
                ));

                // The opt-out can only SUPPRESS, never force-enable: with lazy off,
                // boot-lazy off, or no servable dist, the gate stays false even when
                // the opt-out is unset/opted-in.
                assert!(!defer_dev_bundle_decision(false, Some("1"), true, None));
                assert!(!defer_dev_bundle_decision(true, None, true, Some("true")));
                assert!(!defer_dev_bundle_decision(true, Some("1"), false, None));
            }

            /// Issue #1188 — the dev-bundle deferral opt-out resolver is ON by
            /// default; ONLY an explicit `0`/`false` (trimmed, case-insensitive)
            /// opts out. Unset and unrecognized values keep the deferral on, so a
            /// malformed value never silently disables #1182. Inverted default vs
            /// `resolve_boot_lazy`.
            #[test]
            fn defer_bundle_optout_resolution() {
                // Default-on: unset / truthy / unrecognized all keep deferral on.
                assert!(resolve_defer_bundle(None));
                assert!(resolve_defer_bundle(Some("1")));
                assert!(resolve_defer_bundle(Some("true")));
                assert!(resolve_defer_bundle(Some("banana")));
                // Only explicit falsey values opt out (trimmed, case-insensitive).
                assert!(!resolve_defer_bundle(Some("0")));
                assert!(!resolve_defer_bundle(Some("false")));
                assert!(!resolve_defer_bundle(Some(" FALSE ")));
            }

            /// Issue #1057 — the freshness gate accepts a `dist/` only when it
            /// contains at least one `index.html` (root or nested), and
            /// declines an absent / empty / html-less directory so boot-lazy
            /// never comes up serving 404s.
            #[test]
            fn dist_servable_seed_gate() {
                use std::fs;
                let tmp = tempfile::tempdir().expect("tempdir");
                let root = tmp.path();

                // Absent dir => not servable.
                assert!(!dist_is_servable_seed(&root.join("does-not-exist")));

                // Empty dir => not servable.
                let empty = root.join("empty");
                fs::create_dir_all(&empty).unwrap();
                assert!(!dist_is_servable_seed(&empty));

                // Dir with non-html files only => not servable.
                let noindex = root.join("noindex");
                fs::create_dir_all(&noindex).unwrap();
                fs::write(noindex.join("styles.css"), b"body{}").unwrap();
                assert!(!dist_is_servable_seed(&noindex));

                // Root index.html => servable.
                let rooted = root.join("rooted");
                fs::create_dir_all(&rooted).unwrap();
                fs::write(rooted.join("index.html"), b"<html></html>").unwrap();
                assert!(dist_is_servable_seed(&rooted));

                // Nested index.html (route dir) only => servable.
                let nested = root.join("nested");
                fs::create_dir_all(nested.join("posts/a")).unwrap();
                fs::write(nested.join("posts/a/index.html"), b"<html></html>").unwrap();
                assert!(dist_is_servable_seed(&nested));
            }

            #[test]
            fn claim_returns_token_for_stale_route_and_none_for_fresh() {
                let inner = bare_inner();
                inner.mark_stale([out("blog/a/index.html")]);

                let claim = inner
                    .claim(Path::new("blog/a/index.html"))
                    .expect("stale route must be claimable");
                assert_eq!(claim.output_path, out("blog/a/index.html"));
                assert_eq!(claim.generation, 0, "boot generation is 0");

                assert!(
                    inner.claim(Path::new("blog/b/index.html")).is_none(),
                    "a route that was never staled must not claim"
                );
            }

            #[test]
            fn clear_if_current_clears_unsuperseded_claim() {
                let inner = bare_inner();
                inner.mark_stale([out("blog/a/index.html")]);
                let claim = inner.claim(Path::new("blog/a/index.html")).unwrap();

                inner.clear_if_current(&claim);
                assert!(
                    inner.claim(Path::new("blog/a/index.html")).is_none(),
                    "an unsuperseded claim must clear the stale entry"
                );
            }

            /// THE ABA case (issue #1025 pinned design): claim at
            /// generation N, tick swap bumps to N+1 and re-stales the
            /// route — `clear_if_current` with the stale N-claim must
            /// NOT clear; the route stays stale for the next request.
            #[test]
            fn aba_re_staled_route_survives_clear_of_older_claim() {
                let inner = bare_inner();
                inner.mark_stale([out("blog/a/index.html")]);
                let old_claim = inner.claim(Path::new("blog/a/index.html")).unwrap();
                assert_eq!(old_claim.generation, 0);

                // A new tick: P4 table swap bumps the generation, then
                // the tick's render callback re-stales the same route.
                inner.note_table_swap(&[]);
                inner.mark_stale([out("blog/a/index.html")]);

                inner.clear_if_current(&old_claim);

                let still = inner
                    .claim(Path::new("blog/a/index.html"))
                    .expect("route re-staled at a newer generation must survive the old clear");
                assert_eq!(
                    still.generation, 1,
                    "the surviving entry is the re-staled one"
                );
            }

            /// #1027 guarded-write revalidation predicate: a claim is
            /// current iff its entry still exists at EXACTLY the
            /// claimed generation. Both mid-gap interference shapes
            /// must flip it false — eviction by a tick's eager
            /// re-render, and a re-stale at a bumped (P4 table-swap)
            /// generation.
            #[test]
            fn claim_is_current_tracks_eviction_and_generation_bump() {
                let inner = bare_inner();
                inner.mark_stale([out("blog/a/index.html")]);
                let claim = inner.claim(Path::new("blog/a/index.html")).unwrap();
                assert!(
                    inner.claim_is_current(&claim),
                    "an untouched claim is current"
                );

                // Mid-gap shape 1: a tick eagerly re-rendered the route
                // and evicted its stale entry.
                inner.clear_stale(&[out("blog/a/index.html")]);
                assert!(
                    !inner.claim_is_current(&claim),
                    "an evicted entry must fail revalidation"
                );

                // Mid-gap shape 2: a tick re-staled the route after its
                // P4 generation bump.
                inner.note_table_swap(&[]);
                inner.mark_stale([out("blog/a/index.html")]);
                assert!(
                    !inner.claim_is_current(&claim),
                    "a re-stale at a newer generation must fail the equality check"
                );

                // A fresh claim at the new generation is current again.
                let new_claim = inner.claim(Path::new("blog/a/index.html")).unwrap();
                assert!(inner.claim_is_current(&new_claim));
            }

            #[test]
            fn table_swap_evicts_vanished_stale_entries() {
                let inner = bare_inner();
                inner.mark_stale([out("gone/index.html"), out("kept/index.html")]);

                inner.note_table_swap(&[out("gone/index.html")]);

                assert!(
                    inner.claim(Path::new("gone/index.html")).is_none(),
                    "a vanished route must be evicted from the stale set (#804)"
                );
                assert!(
                    inner.claim(Path::new("kept/index.html")).is_some(),
                    "unrelated stale entries must survive the swap"
                );
            }

            #[test]
            fn take_tick_stale_drains_once_sorted() {
                let inner = bare_inner();
                inner.mark_stale([out("b.html"), out("a.html"), out("b.html")]);

                assert_eq!(
                    inner.take_tick_stale(),
                    vec![out("a.html"), out("b.html")],
                    "tick buffer drains sorted + deduped"
                );
                assert!(
                    inner.take_tick_stale().is_empty(),
                    "second drain in the same tick must be empty"
                );
                assert!(
                    inner.claim(Path::new("a.html")).is_some(),
                    "draining the tick buffer must not clear the stale entries"
                );
            }

            #[test]
            fn clear_stale_drops_rendered_routes() {
                let inner = bare_inner();
                inner.mark_stale([out("a.html"), out("b.html")]);

                inner.clear_stale(&[out("a.html")]);

                assert!(inner.claim(Path::new("a.html")).is_none());
                assert!(inner.claim(Path::new("b.html")).is_some());
            }

            /// S5 (#1233 / #1227 item (h)) — the dynamic-injected HMR seam.
            ///
            /// A dynamic injected route is resolved request-time: the adapter
            /// records its output_path via `note_dynamic_injected` and (file
            /// absent) marks it stale via `claim_or_mark_stale`. The claim is
            /// then cleared on a successful render. A later content-edit tick
            /// (`note_table_swap` bumps the generation) must RE-STALE that
            /// previously-rendered dynamic injected output via
            /// `restale_dynamic_injected`, so the next request re-renders it
            /// against the fresh content snapshot. A plain output path that
            /// was NEVER a dynamic injected route must stay fresh after the
            /// same swap — proving the re-stale is scoped to the tracked set,
            /// not a blanket "stale everything".
            #[test]
            fn restale_dynamic_injected_refreshes_only_tracked_routes() {
                let inner = bare_inner();

                // First request resolves a dynamic injected URL: the adapter
                // records it (`note_dynamic_injected`) and marks it stale
                // (gen 0) because the file is absent.
                inner.note_dynamic_injected(Path::new("preset-articles/feature/index.html"));
                let claim =
                    inner.claim_or_mark_stale(Path::new("preset-articles/feature/index.html"));
                assert_eq!(claim.generation, 0, "boot generation is 0");
                // A successful render clears the claim → the route is fresh.
                inner.clear_if_current(&claim);
                assert!(
                    inner
                        .claim(Path::new("preset-articles/feature/index.html"))
                        .is_none(),
                    "after a successful render the dynamic injected route is fresh"
                );

                // A content-edit tick: the P4 swap bumps the generation, then
                // re-stales the tracked dynamic injected outputs.
                inner.note_table_swap(&[]);
                inner.restale_dynamic_injected();

                let restaled = inner
                    .claim(Path::new("preset-articles/feature/index.html"))
                    .expect(
                        "a previously-rendered dynamic injected route must be re-staled by a \
                         content-edit tick (#1234 confirm-gap fix)",
                    );
                assert_eq!(
                    restaled.generation, 1,
                    "the re-stale records the post-swap generation"
                );

                // A route that was never a dynamic injected render is untouched
                // by the dynamic re-stale.
                assert!(
                    inner.claim(Path::new("some/other/index.html")).is_none(),
                    "restale_dynamic_injected must only touch tracked dynamic injected outputs"
                );
            }

            /// The codex-found gap: a dynamic injected output whose file
            /// already exists on disk (an output rendered in a PREVIOUS
            /// `zfb dev` run whose `.zfb-build/dev-pages` persisted across a
            /// restart) is resolved through the adapter's `file_on_disk.exists()`
            /// branch, which NEVER calls `claim_or_mark_stale`. The adapter
            /// records it via `note_dynamic_injected` REGARDLESS, so a later
            /// content edit can still re-stale it. Without the unconditional
            /// record the stale on-disk HTML would be served forever.
            #[test]
            fn restale_dynamic_injected_covers_preexisting_on_disk_output() {
                let inner = bare_inner();

                // File already on disk: the adapter records the path but does
                // NOT mark it stale (the on-disk file is served as fresh).
                inner.note_dynamic_injected(Path::new("preset-articles/feature/index.html"));
                assert!(
                    inner
                        .claim(Path::new("preset-articles/feature/index.html"))
                        .is_none(),
                    "an on-disk dynamic injected output is fresh until a tick re-stales it"
                );

                // Content-edit tick: swap + dynamic re-stale.
                inner.note_table_swap(&[]);
                inner.restale_dynamic_injected();

                let restaled = inner
                    .claim(Path::new("preset-articles/feature/index.html"))
                    .expect(
                        "a pre-existing on-disk dynamic injected output must still be re-staled \
                         by a content edit (codex review P2 — restart staleness)",
                    );
                assert_eq!(restaled.generation, 1);
            }

            /// `restale_dynamic_injected` re-marks at the CURRENT generation,
            /// so it composes with the claim/clear ABA token exactly like the
            /// static-seed re-stale: an in-flight render holding the pre-swap
            /// claim must NOT clear the post-swap re-stale.
            #[test]
            fn restale_dynamic_injected_defeats_aba_clear() {
                let inner = bare_inner();
                inner.note_dynamic_injected(Path::new("preset-articles/feature/index.html"));
                let old_claim =
                    inner.claim_or_mark_stale(Path::new("preset-articles/feature/index.html"));
                assert_eq!(old_claim.generation, 0);

                // Tick swap + dynamic re-stale (generation now 1).
                inner.note_table_swap(&[]);
                inner.restale_dynamic_injected();

                // The in-flight render finishes and tries to clear with the
                // STALE generation-0 claim — it must not clear the re-stale.
                inner.clear_if_current(&old_claim);

                let still = inner
                    .claim(Path::new("preset-articles/feature/index.html"))
                    .expect("the dynamic re-stale at a newer generation must survive an old clear");
                assert_eq!(still.generation, 1);
            }

            /// Vanished routes are also dropped from the CURRENT tick's
            /// stale buffer so `pages_stale` never announces a route
            /// that no longer resolves.
            #[test]
            fn table_swap_drops_vanished_from_tick_buffer() {
                let inner = bare_inner();
                inner.mark_stale([out("gone.html"), out("kept.html")]);

                inner.note_table_swap(&[out("gone.html")]);

                assert_eq!(inner.take_tick_stale(), vec![out("kept.html")]);
            }

            // ── lazy eager-vs-stale split ────────────────────────────

            /// Routes for the canonical three-source matrix: a slug
            /// blog source (own routes), an S2 tag-index aggregate
            /// (params not slug-shaped), and an S1 static index.
            fn matrix_routes() -> HashMap<PathBuf, Vec<DevRouteEntry>> {
                let mut routes = HashMap::new();
                routes.insert(
                    PathBuf::from("pages/blog/[slug].tsx"),
                    vec![
                        with_params(
                            route_entry("/blog/a", "blog/a/index.html", "/blog/:slug"),
                            scalar_params(&[("slug", "a")]),
                        ),
                        with_params(
                            route_entry("/blog/b", "blog/b/index.html", "/blog/:slug"),
                            scalar_params(&[("slug", "b")]),
                        ),
                    ],
                );
                routes.insert(
                    PathBuf::from("pages/tags/[tag].tsx"),
                    vec![with_params(
                        route_entry("/tags/rust", "tags/rust/index.html", "/tags/:tag"),
                        scalar_params(&[("tag", "rust")]),
                    )],
                );
                routes.insert(
                    PathBuf::from("pages/index.tsx"),
                    no_params(vec![route_entry("/", "index.html", "/")]),
                );
                routes
            }

            fn matrix_pages() -> Vec<PageId> {
                vec![
                    PageId::new(PathBuf::from("pages/blog/[slug].tsx")),
                    PageId::new(PathBuf::from("pages/tags/[tag].tsx")),
                    PageId::new(PathBuf::from("pages/index.tsx")),
                ]
            }

            /// Body edit: eager = the edited entry's own routes only;
            /// the S2 aggregate and the S1 static are NOT eager (they
            /// become the stale remainder).
            #[test]
            fn lazy_eager_sets_body_edit_selects_own_routes_only() {
                let (tmp, cfg) = scaffold(&[("a.mdx", "title: A")]);
                let session = lazy_session_at(&tmp, cfg, matrix_routes());

                let edited = write_entry(tmp.path(), "a.mdx", "title: A", "updated body");
                let eager = compute_lazy_eager_sets(&session, Some(&hint_for(&[edited])));

                assert_eq!(
                    eager.get(Path::new("pages/blog/[slug].tsx")),
                    Some(&HashSet::from([out("blog/a/index.html")])),
                    "the edited entry's own route is the eager set"
                );
                assert!(
                    !eager.contains_key(Path::new("pages/tags/[tag].tsx")),
                    "S2 aggregate sources must not be eager"
                );
                assert!(
                    !eager.contains_key(Path::new("pages/index.tsx")),
                    "S1 static sources must not be eager"
                );
            }

            /// THE G4 divergence from #958: a FRONTMATTER edit still
            /// eager-renders the edited entry's own routes in lazy mode
            /// (the cross-page fallout is staled, not eagerly
            /// re-rendered) — while the eager-narrowing gate of the
            /// switch-OFF path keeps returning Off for the same tick.
            #[test]
            fn lazy_eager_sets_frontmatter_edit_still_selects_own_routes() {
                let (tmp, cfg) = scaffold(&[("a.mdx", "title: A")]);
                let session = lazy_session_at(&tmp, cfg, matrix_routes());

                let edited = write_entry(tmp.path(), "a.mdx", "title: A (renamed)", "body");
                let eager = compute_lazy_eager_sets(
                    &session,
                    Some(&hint_for(std::slice::from_ref(&edited))),
                );

                assert_eq!(
                    eager.get(Path::new("pages/blog/[slug].tsx")),
                    Some(&HashSet::from([out("blog/a/index.html")])),
                    "a frontmatter edit must still yield the entry's own routes (no G4 fallback in lazy mode)"
                );
            }

            /// No hint (`.tsx`, G5/G6, data, discovery ticks) ⇒ no
            /// eager routes at all.
            #[test]
            fn lazy_eager_sets_empty_without_hint() {
                let (tmp, cfg) = scaffold(&[("a.mdx", "title: A")]);
                let session = lazy_session_at(&tmp, cfg, matrix_routes());

                assert!(compute_lazy_eager_sets(&session, None).is_empty());
            }

            /// `.tsx`-style tick (hint = None): every selected route is
            /// marked stale, nothing renders, unknown sources are
            /// no-ops.
            #[test]
            fn lazy_tick_without_hint_marks_all_selected_stale() {
                let (tmp, cfg) = scaffold(&[("a.mdx", "title: A")]);
                let session = lazy_session_at(&tmp, cfg, matrix_routes());

                let mut pages = matrix_pages();
                pages.push(PageId::new(PathBuf::from("pages/unknown.tsx")));
                let rendered =
                    lazy_render_tick(&session, tmp.path(), &pages, None).expect("tick succeeds");

                assert!(rendered.is_empty(), "an all-lazy tick renders nothing");
                assert_eq!(
                    session.inner.take_tick_stale(),
                    vec![
                        out("blog/a/index.html"),
                        out("blog/b/index.html"),
                        out("index.html"),
                        out("tags/rust/index.html"),
                    ],
                    "every selected route (and only those) is staled"
                );
                assert!(
                    session
                        .inner
                        .claim(Path::new("blog/a/index.html"))
                        .is_some(),
                    "stale entries persist past the tick-buffer drain"
                );
            }

            /// Live stub host (`Backend::Stub`) installed into the
            /// session so a lazy tick's EAGER render can actually
            /// succeed — `render_one` drives the stub closure exactly
            /// like a live V8 host.
            fn install_stub_renderer(session: &DevRenderSession, body: &'static str) {
                use zfb_build::renderer::{start, Backend, HttpResponseLike, RendererStartInput};
                let state = start(RendererStartInput {
                    // Ignored by `Backend::Stub` (no bundle is loaded).
                    bundle_path: PathBuf::from("stub-bundle.mjs"),
                    sourcemap_path: PathBuf::from("stub-bundle.mjs.map"),
                    backend: Backend::Stub {
                        handler: Arc::new(move |_url| HttpResponseLike {
                            status: 200,
                            content_type: "text/html; charset=utf-8".into(),
                            headers: Vec::new(),
                            body: body.as_bytes().to_vec(),
                        }),
                    },
                    request_timeout: None,
                })
                .expect("stub renderer must start");
                *session.inner.renderer.lock().unwrap() = Some(state);
            }

            /// #1027 tick-side pin for the lazy race: a lazy tick that
            /// EAGERLY re-renders a route EVICTS its stale entry —
            /// inside `apply()`'s exclusion window in production
            /// (`lazy_render_tick` runs as the render callback) — so a
            /// request claim captured against the OLDER world fails the
            /// guarded-write revalidation instead of overwriting the
            /// tick's fresher bytes.
            #[test]
            fn lazy_tick_eager_render_evicts_prior_stale_entry() {
                let (tmp, cfg) = scaffold(&[("a.mdx", "title: A")]);
                let session = lazy_session_at(&tmp, cfg, matrix_routes());
                install_stub_renderer(&session, "<html><body>tick-fresh</body></html>");

                // An earlier tick staled the route; a request claimed it.
                session.inner.mark_stale([out("blog/a/index.html")]);
                let _ = session.inner.take_tick_stale();
                let claim = session.inner.claim(Path::new("blog/a/index.html")).unwrap();

                // Body edit tick: blog/a is the eager set.
                let edited = write_entry(tmp.path(), "a.mdx", "title: A", "updated body");
                let hint = hint_for(&[edited]);
                let dist = tmp.path().join("dist");
                std::fs::create_dir_all(&dist).unwrap();
                let rendered = lazy_render_tick(&session, &dist, &matrix_pages(), Some(&hint))
                    .expect("the eager render succeeds against the stub host");

                assert_eq!(
                    rendered.len(),
                    1,
                    "exactly the eager own route rendered: {rendered:?}"
                );
                assert!(
                    session
                        .inner
                        .claim(Path::new("blog/a/index.html"))
                        .is_none(),
                    "the eager render must evict the route's prior stale entry"
                );
                assert!(
                    !session.inner.claim_is_current(&claim),
                    "a request claim captured before the tick must now fail revalidation"
                );
                // The stale remainder is untouched by the eviction.
                assert!(
                    session
                        .inner
                        .claim(Path::new("blog/b/index.html"))
                        .is_some(),
                    "non-eager routes stay stale"
                );
            }

            /// Body edit through the full lazy tick: the S2 aggregate
            /// and the S1 static are marked stale (NOT rendered); the
            /// own route is attempted eagerly — and because this stub
            /// session has no live renderer, the failed eager render
            /// re-stales it so the request path can retry.
            #[test]
            fn lazy_tick_body_edit_stales_remainder_and_failed_eager() {
                let (tmp, cfg) = scaffold(&[("a.mdx", "title: A")]);
                let session = lazy_session_at(&tmp, cfg, matrix_routes());

                let edited = write_entry(tmp.path(), "a.mdx", "title: A", "updated body");
                let hint = hint_for(&[edited]);
                let rendered = lazy_render_tick(&session, tmp.path(), &matrix_pages(), Some(&hint))
                    .expect("a failed eager render keeps the watcher alive");

                assert!(rendered.is_empty(), "no live renderer in this stub");
                assert_eq!(
                    session.inner.take_tick_stale(),
                    vec![
                        // own route: re-staled by the failed eager render
                        out("blog/a/index.html"),
                        // remainder: sibling + S1 static + S2 aggregate
                        out("blog/b/index.html"),
                        out("index.html"),
                        out("tags/rust/index.html"),
                    ],
                );
            }

            /// Boot exception (review finding on #1025): even with the
            /// switch ON, the session's FIRST render-callback
            /// invocation — the eager initial build — takes the eager
            /// path and stales nothing; lazy behaviour starts with the
            /// next tick. (The request-time stale-render adapter lands
            /// in a later sub-issue, so a lazy boot would 404 every
            /// route.)
            #[test]
            fn lazy_boot_invocation_stays_eager_then_goes_lazy() {
                let (tmp, cfg) = scaffold(&[]);
                let session = lazy_session_at(&tmp, cfg, matrix_routes());
                // Rewind the stub's mid-flight default: boot pending.
                session
                    .inner
                    .boot_render_done
                    .store(false, std::sync::atomic::Ordering::SeqCst);
                let cb = make_render_callback(session.clone(), tmp.path().to_path_buf());
                let pages = vec![PageId::new(PathBuf::from("pages/index.tsx"))];

                // Boot invocation: eager path (render error swallowed
                // by the per-page tolerance) — nothing goes stale.
                let first = cb(&pages, None).expect("boot tick succeeds");
                assert!(first.is_empty());
                assert!(
                    session.inner.claim(Path::new("index.html")).is_none(),
                    "the boot render must not mark routes stale"
                );
                assert!(session.inner.take_tick_stale().is_empty());

                // Next tick: the lazy split is active.
                let second = cb(&pages, None).expect("watcher tick succeeds");
                assert!(second.is_empty());
                assert!(
                    session.inner.claim(Path::new("index.html")).is_some(),
                    "post-boot ticks must go through the lazy split"
                );
            }

            /// The switch itself: lazy ON routes the render callback
            /// through the stale-marking split; lazy OFF (default)
            /// leaves the stale set untouched on an identical tick.
            #[test]
            fn render_callback_respects_lazy_switch() {
                // ON: the static route is marked stale instead of rendered.
                let (tmp_on, cfg_on) = scaffold(&[]);
                let session_on = lazy_session_at(&tmp_on, cfg_on, matrix_routes());
                let cb_on = make_render_callback(session_on.clone(), tmp_on.path().to_path_buf());
                let pages = vec![PageId::new(PathBuf::from("pages/index.tsx"))];
                let out_on = cb_on(&pages, None).expect("lazy tick succeeds");
                assert!(out_on.is_empty());
                assert!(
                    session_on.inner.claim(Path::new("index.html")).is_some(),
                    "switch ON must mark the selected route stale"
                );

                // OFF: same tick shape goes down the eager path and the
                // stale set stays empty (render errors are swallowed by
                // the existing per-page tolerance).
                let (tmp_off, cfg_off) = scaffold(&[]);
                let session_off = session_at(&tmp_off, cfg_off, matrix_routes());
                let cb_off =
                    make_render_callback(session_off.clone(), tmp_off.path().to_path_buf());
                let out_off = cb_off(&pages, None).expect("eager tick tolerates render errors");
                assert!(out_off.is_empty());
                assert!(
                    session_off.inner.claim(Path::new("index.html")).is_none(),
                    "switch OFF must never touch the stale set"
                );
                assert!(
                    session_off.inner.take_tick_stale().is_empty(),
                    "switch OFF must never announce stale routes"
                );
            }
        }
    }

    /// Content provenance boundary tests (issue #1600).
    ///
    /// These model the event sequence produced by a cold worker boot and a
    /// watch-ADD refresh, then inspect the dependency graph directly. The
    /// real dev E2E complements them by proving the generated worker wrapper
    /// emits those events in a live V8 process.
    #[cfg(feature = "embed_v8")]
    mod content_provenance {
        use super::*;
        use crate::render_pipeline::ResolvedRouteParams;

        fn posts_config() -> config::Config {
            config::Config {
                collections: vec![config::CollectionDef {
                    name: "posts".into(),
                    path: PathBuf::from("content/posts"),
                    schema: None,
                    include: None,
                    exclude: None,
                    id_strip_suffix: None,
                    allow_outside_root: false,
                }],
                ..config::Config::default()
            }
        }

        fn write_post(root: &Path, name: &str) -> PathBuf {
            let path = root.join("content/posts").join(format!("{name}.md"));
            std::fs::create_dir_all(path.parent().expect("post has a parent")).unwrap();
            std::fs::write(&path, format!("---\ntitle: {name}\n---\n\n{name} body\n")).unwrap();
            path
        }

        fn content_membership(root: &Path, cfg: &config::Config) -> DevContentMembershipSnapshot {
            let roots = resolve_roots(root, cfg).collection_roots().to_vec();
            collect_content_provenance_membership(cfg, &roots).unwrap()
        }

        fn route_entry(
            url: &str,
            output: &str,
            route_key: &str,
            params: Option<&[(&str, &str)]>,
        ) -> DevRouteEntry {
            DevRouteEntry {
                entry: RouteUniverseEntry {
                    url_path: url.into(),
                    output_path: PathBuf::from(output),
                    route_key: route_key.into(),
                    static_html: false,
                    source_path: None,
                },
                params: params.map(|pairs| ResolvedRouteParams {
                    scalars: pairs
                        .iter()
                        .map(|(key, value)| (key.to_string(), value.to_string()))
                        .collect(),
                    arrays: BTreeMap::new(),
                }),
            }
        }

        fn trace(source: &str, phase: DevContentTracePhase) -> DevContentTraceEvent {
            DevContentTraceEvent {
                source: source.into(),
                collection: Some("posts".into()),
                phase,
                kind: DevContentTraceEventKind::Read,
            }
        }

        fn visit(source: &str, phase: DevContentTracePhase) -> DevContentTraceEvent {
            DevContentTraceEvent {
                source: source.into(),
                collection: None,
                phase,
                kind: DevContentTraceEventKind::Visit,
            }
        }

        fn install_trace_edges(
            graph: &mut DependencyGraph,
            routes: &HashMap<PathBuf, Vec<DevRouteEntry>>,
            membership: &DevContentMembershipSnapshot,
            root: &Path,
            events: Vec<DevContentTraceEvent>,
        ) {
            let classified =
                classify_content_trace_events(events, routes, membership, root).unwrap();
            let groups = ContentProvenance::from_reads(classified.reads.into_values().flatten())
                .edge_groups(&membership.membership)
                .unwrap();
            replace_content_edges(graph, groups);
        }

        #[test]
        fn cold_boot_trace_seeds_direct_and_all_aggregate_consumers() {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            let cfg = posts_config();
            let alpha = write_post(root, "alpha");
            let _beta = write_post(root, "beta");

            let entry_source = root.join("pages/posts/[slug].tsx");
            let index_source = root.join("pages/posts/index.tsx");
            let tag_source = root.join("pages/tags/[tag].tsx");
            let pagination_source = root.join("pages/posts/page/[page].tsx");
            let routes = HashMap::from([
                (
                    entry_source.clone(),
                    vec![
                        route_entry(
                            "/posts/alpha",
                            "posts/alpha/index.html",
                            "/posts/:slug",
                            Some(&[("slug", "alpha")]),
                        ),
                        route_entry(
                            "/posts/beta",
                            "posts/beta/index.html",
                            "/posts/:slug",
                            Some(&[("slug", "beta")]),
                        ),
                    ],
                ),
                (
                    index_source.clone(),
                    vec![route_entry("/posts", "posts/index.html", "/posts", None)],
                ),
                (
                    tag_source.clone(),
                    vec![
                        route_entry(
                            "/tags/guide",
                            "tags/guide/index.html",
                            "/tags/:tag",
                            Some(&[("tag", "guide")]),
                        ),
                        route_entry(
                            "/tags/other",
                            "tags/other/index.html",
                            "/tags/:tag",
                            Some(&[("tag", "other")]),
                        ),
                    ],
                ),
                (
                    pagination_source.clone(),
                    vec![
                        route_entry(
                            "/posts/page/1",
                            "posts/page/1/index.html",
                            "/posts/page/:page",
                            Some(&[("page", "1")]),
                        ),
                        route_entry(
                            "/posts/page/2",
                            "posts/page/2/index.html",
                            "/posts/page/:page",
                            Some(&[("page", "2")]),
                        ),
                    ],
                ),
            ]);
            let membership = content_membership(root, &cfg);
            let mut graph = DependencyGraph::new();

            install_trace_edges(
                &mut graph,
                &routes,
                &membership,
                root,
                vec![
                    trace("pages/posts/[slug].tsx", DevContentTracePhase::Paths),
                    trace("pages/posts/index.tsx", DevContentTracePhase::Render),
                    trace("pages/tags/[tag].tsx", DevContentTracePhase::Paths),
                    trace("pages/posts/page/[page].tsx", DevContentTracePhase::Paths),
                ],
            );

            let mut expected = vec![
                PageId::new(entry_source),
                PageId::new(index_source),
                PageId::new(tag_source),
                PageId::new(pagination_source),
            ];
            expected.sort();
            assert_eq!(
                graph.consumers_of(&alpha),
                Some(expected),
                "a cold boot must seed alpha's direct route plus every aggregate reader"
            );
        }

        #[test]
        fn discovery_rewalk_gives_new_entry_direct_and_aggregate_consumers() {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            let cfg = posts_config();
            let _alpha = write_post(root, "alpha");
            let _beta = write_post(root, "beta");

            let entry_source = root.join("pages/posts/[slug].tsx");
            let index_source = root.join("pages/posts/index.tsx");
            let mut routes = HashMap::from([
                (
                    entry_source.clone(),
                    vec![
                        route_entry(
                            "/posts/alpha",
                            "posts/alpha/index.html",
                            "/posts/:slug",
                            Some(&[("slug", "alpha")]),
                        ),
                        route_entry(
                            "/posts/beta",
                            "posts/beta/index.html",
                            "/posts/:slug",
                            Some(&[("slug", "beta")]),
                        ),
                    ],
                ),
                (
                    index_source.clone(),
                    vec![route_entry("/posts", "posts/index.html", "/posts", None)],
                ),
            ]);
            let mut graph = DependencyGraph::new();
            let initial_membership = content_membership(root, &cfg);
            install_trace_edges(
                &mut graph,
                &routes,
                &initial_membership,
                root,
                vec![
                    trace("pages/posts/[slug].tsx", DevContentTracePhase::Paths),
                    trace("pages/posts/index.tsx", DevContentTracePhase::Render),
                ],
            );

            let gamma = write_post(root, "gamma");
            routes
                .get_mut(&entry_source)
                .expect("entry route source exists")
                .push(route_entry(
                    "/posts/gamma",
                    "posts/gamma/index.html",
                    "/posts/:slug",
                    Some(&[("slug", "gamma")]),
                ));
            let refreshed_membership = content_membership(root, &cfg);
            install_trace_edges(
                &mut graph,
                &routes,
                &refreshed_membership,
                root,
                vec![
                    trace("pages/posts/[slug].tsx", DevContentTracePhase::Paths),
                    trace("pages/posts/index.tsx", DevContentTracePhase::Render),
                ],
            );

            let mut expected = vec![PageId::new(entry_source), PageId::new(index_source)];
            expected.sort();
            assert_eq!(
                graph.consumers_of(&gamma),
                Some(expected),
                "the post-discovery membership rewalk must add both the new entry route and the aggregate reader"
            );
        }

        #[test]
        fn current_worker_visit_without_read_replaces_stale_route_provenance() {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            let cfg = posts_config();
            let alpha = write_post(root, "alpha");
            let source = root.join("pages/posts/index.tsx");
            let routes = HashMap::from([(
                source.clone(),
                vec![route_entry("/posts", "posts/index.html", "/posts", None)],
            )]);
            let membership = content_membership(root, &cfg);
            let classified = classify_content_trace_events(
                vec![visit("pages/posts/index.tsx", DevContentTracePhase::Render)],
                &routes,
                &membership,
                root,
            )
            .unwrap();
            assert!(
                classified.reads.is_empty(),
                "the visit made no collection read"
            );
            let render_observation = DevContentTraceObservation {
                consumer: PageId::new(source.clone()),
                phase: DevContentTracePhase::Render,
            };
            assert_eq!(
                classified.observed,
                BTreeSet::from([render_observation.clone()])
            );

            let mut retained = BTreeMap::from([(
                render_observation,
                vec![TrackedContentRead::collection(PageId::new(source), "posts")],
            )]);
            apply_content_trace_observations(&mut retained, classified.observed, classified.reads);
            assert!(
                retained.is_empty(),
                "a current-worker visit with no read must remove that route's stale edge evidence"
            );

            let mut graph = DependencyGraph::new();
            replace_content_edges(
                &mut graph,
                ContentProvenance::from_reads(retained.into_values().flatten())
                    .edge_groups(&membership.membership)
                    .unwrap(),
            );
            assert_eq!(
                graph.consumers_of(&alpha),
                None,
                "the stale collection reader must no longer leave a content edge"
            );
        }

        #[test]
        fn render_visit_preserves_paths_provenance_for_the_same_dynamic_route() {
            let consumer = PageId::new("pages/posts/[slug].tsx");
            let paths_observation = DevContentTraceObservation {
                consumer: consumer.clone(),
                phase: DevContentTracePhase::Paths,
            };
            let render_observation = DevContentTraceObservation {
                consumer: consumer.clone(),
                phase: DevContentTracePhase::Render,
            };
            let mut retained = BTreeMap::from([(
                paths_observation.clone(),
                vec![TrackedContentRead::collection(consumer, "posts")],
            )]);

            apply_content_trace_observations(
                &mut retained,
                BTreeSet::from([render_observation]),
                BTreeMap::new(),
            );

            assert!(
                retained.contains_key(&paths_observation),
                "a render that makes no collection read must not erase the route's prior paths() evidence",
            );
        }

        #[test]
        fn self_contained_wrapper_rewrites_both_supported_default_export_shapes() {
            let named_export = "const worker = { fetch() {} };\nexport { worker as default };\n";
            let (rewritten_named, named_binding) =
                rewrite_dev_bundle_default_export(named_export).unwrap();
            assert_eq!(named_binding, "worker");
            assert!(rewritten_named.contains("worker as __zfb_content_trace_inner_default"));
            assert!(!rewritten_named.contains(" as default"));

            let direct_export = "const value = 1;\nexport default { fetch() {} };\n";
            let (rewritten_direct, direct_binding) =
                rewrite_dev_bundle_default_export(direct_export).unwrap();
            assert_eq!(direct_binding, "__zfb_content_trace_inner_default");
            assert!(rewritten_direct
                .contains("const __zfb_content_trace_inner_default = { fetch() {} };"));
            assert!(!rewritten_direct.contains("export default"));
        }
    }

    /// Deep-review regression (PR #376): `Route::template()` emits
    /// Hono-style colon syntax (`/blog/:slug`, `/docs/:slug{.+}`), but
    /// `SsrRouteSet`'s matcher consumes `pages/`-style brackets
    /// (`/blog/[slug]`, `/docs/[...slug]`). Without translation,
    /// dynamic-route SSR matches never fire in dev.
    #[test]
    fn colon_template_to_bracket_translates_dynamic_and_catchall() {
        assert_eq!(colon_template_to_bracket("/"), "/");
        assert_eq!(colon_template_to_bracket("/about"), "/about");
        assert_eq!(colon_template_to_bracket("/blog/:slug"), "/blog/[slug]");
        assert_eq!(
            colon_template_to_bracket("/docs/:rest{.+}"),
            "/docs/[...rest]",
        );
        // Optional catchall (`:name{.+}?`) → `[[...name]]`.
        assert_eq!(
            colon_template_to_bracket("/docs/:rest{.+}?"),
            "/docs/[[...rest]]",
        );
        assert_eq!(colon_template_to_bracket("/:rest{.+}?"), "/[[...rest]]",);
        assert_eq!(colon_template_to_bracket("/a/:b/c/:d"), "/a/[b]/c/[d]",);
    }

    /// `ssr_patterns()` must hand the SsrRouteSet bracket-grammar
    /// patterns so dynamic SSR routes match in dev.
    #[test]
    fn ssr_patterns_emit_bracket_grammar_for_dynamic_routes() {
        let session = DevRenderSession {
            inner: Arc::new(stub_dev_inner(
                HashMap::new(),
                vec![
                    RouteUniverseEntry {
                        url_path: "/blog/:slug".into(),
                        output_path: PathBuf::new(),
                        route_key: "/blog/:slug".into(),
                        static_html: false,
                        source_path: None,
                    },
                    RouteUniverseEntry {
                        url_path: "/api/x".into(),
                        output_path: PathBuf::new(),
                        route_key: "/api/x".into(),
                        static_html: false,
                        source_path: None,
                    },
                ],
            )),
        };
        let patterns = session.ssr_patterns();
        assert_eq!(
            patterns,
            vec!["/blog/[slug]".to_string(), "/api/x".to_string()]
        );
    }

    /// Review-fix (codex finding 2): a discovery-hook refresh must rewrite
    /// the live `SsrRoutesHandle` so a newly-created `prerender = false`
    /// route is dispatchable WITHOUT a later edit.
    ///
    /// The discovery hook calls `refresh_live_ssr_routes` (it can't rely on
    /// the pipeline's `reload_renderer`, which is skipped when the discovery
    /// refresh marks the renderer fresh). This test drives that exact seam:
    /// a project that booted with ZERO SSR routes (handle wraps `None`),
    /// then a mid-session discovery adds a `prerender = false` route to the
    /// session's tables. After `refresh_live_ssr_routes` the handle must hold
    /// a populated set whose matcher resolves the new route.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn discovery_refresh_updates_live_ssr_routes_without_edit() {
        // Boot state: no SSR routes → the live handle wraps `None`, exactly
        // what `build_ssr_route_set` produces for an all-SSG project.
        let session = DevRenderSession {
            inner: Arc::new(stub_dev_inner(HashMap::new(), Vec::new())),
        };
        let dispatcher: Arc<dyn SsrDispatcher> = Arc::new(
            crate::ssr_adapter::EmbeddedV8SsrAdapter::new(session.renderer_handle()),
        );
        let handle: SsrRoutesHandle = Arc::new(std::sync::RwLock::new(make_ssr_route_set(
            &session,
            Arc::clone(&dispatcher),
        )));
        assert!(
            handle.read().unwrap().is_none(),
            "boot handle must be empty for an all-SSG project"
        );

        // Mid-session discovery: a new `prerender = false` page is found and
        // added to the session's route tables (what `discover_created` does).
        {
            let mut tables = session.inner.routes.write().unwrap();
            tables.ssr_routes.push(RouteUniverseEntry {
                url_path: "/blog/:slug".into(),
                output_path: PathBuf::new(),
                route_key: "/blog/:slug".into(),
                static_html: false,
                source_path: None,
            });
        }

        // The discovery hook's new call (review-fix) — NOT an edit tick.
        refresh_live_ssr_routes(&session, &handle);

        let guard = handle.read().unwrap();
        let set = guard
            .as_ref()
            .expect("handle must now hold a populated SsrRouteSet");
        assert!(
            set.find_match("/blog/anything").is_some(),
            "the newly-created prerender=false route must dispatch without an extra edit"
        );
    }

    // ---------------------------------------------------------------------------
    // refresh_dev_island_chunks unit tests (issue #809)
    // ---------------------------------------------------------------------------

    fn make_companion(filename: &str, bytes: &[u8]) -> zfb_build::pipeline::CompanionFile {
        zfb_build::pipeline::CompanionFile {
            filename: filename.to_string(),
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn writes_chunk_and_worker_companions_to_assets_dir() {
        let dir = tempfile::tempdir().unwrap();
        let assets = dir.path().to_path_buf();

        let companions = vec![
            make_companion("islands-chunk-AAAAAAAA.js", b"chunk-a"),
            make_companion("islands-chunk-BBBBBBBB.js", b"chunk-b"),
            make_companion("worker-src-s-search-d-worker-d-ts.js", b"worker"),
        ];
        let result = refresh_dev_island_chunks(&assets, &companions, &HashSet::new()).unwrap();

        assert_eq!(result.len(), 3);
        assert!(result.contains("islands-chunk-AAAAAAAA.js"));
        assert!(result.contains("islands-chunk-BBBBBBBB.js"));
        assert!(result.contains("worker-src-s-search-d-worker-d-ts.js"));
        assert_eq!(
            std::fs::read(assets.join("islands-chunk-AAAAAAAA.js")).unwrap(),
            b"chunk-a"
        );
        assert_eq!(
            std::fs::read(assets.join("islands-chunk-BBBBBBBB.js")).unwrap(),
            b"chunk-b"
        );
        assert_eq!(
            std::fs::read(assets.join("worker-src-s-search-d-worker-d-ts.js")).unwrap(),
            b"worker"
        );
    }

    #[test]
    fn prunes_stale_chunks_on_rebundle() {
        let dir = tempfile::tempdir().unwrap();
        let assets = dir.path().to_path_buf();

        // Generation 1: two chunks.
        let gen1 = vec![
            make_companion("islands-chunk-GEN1AAAA.js", b"gen1-a"),
            make_companion("islands-chunk-GEN1BBBB.js", b"gen1-b"),
            make_companion("worker-src-s-old-d-ts.js", b"old-worker"),
        ];
        let prev = refresh_dev_island_chunks(&assets, &gen1, &HashSet::new()).unwrap();
        assert!(assets.join("islands-chunk-GEN1AAAA.js").exists());
        assert!(assets.join("islands-chunk-GEN1BBBB.js").exists());
        assert!(assets.join("worker-src-s-old-d-ts.js").exists());

        // Generation 2: different chunks (simulates a dynamically-imported
        // module change so esbuild emits a new content hash).
        let gen2 = vec![make_companion("islands-chunk-GEN2CCCC.js", b"gen2-c")];
        let next = refresh_dev_island_chunks(&assets, &gen2, &prev).unwrap();

        // New chunk exists.
        assert!(assets.join("islands-chunk-GEN2CCCC.js").exists());
        // Stale gen-1 chunks were pruned.
        assert!(
            !assets.join("islands-chunk-GEN1AAAA.js").exists(),
            "stale chunk A must be pruned"
        );
        assert!(
            !assets.join("islands-chunk-GEN1BBBB.js").exists(),
            "stale chunk B must be pruned"
        );
        assert!(
            !assets.join("worker-src-s-old-d-ts.js").exists(),
            "stale worker must be pruned"
        );
        assert_eq!(next.len(), 1);
        assert!(next.contains("islands-chunk-GEN2CCCC.js"));
    }

    #[test]
    fn prunes_all_chunks_when_new_set_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let assets = dir.path().to_path_buf();

        // Seed one chunk.
        let gen1 = vec![make_companion("islands-chunk-STALEAAA.js", b"stale")];
        let prev = refresh_dev_island_chunks(&assets, &gen1, &HashSet::new()).unwrap();
        assert!(assets.join("islands-chunk-STALEAAA.js").exists());

        // Next bundle has no chunks (project switched to zero dynamic imports).
        let next = refresh_dev_island_chunks(&assets, &[], &prev).unwrap();
        assert!(next.is_empty());
        assert!(
            !assets.join("islands-chunk-STALEAAA.js").exists(),
            "stale chunk must be pruned when new set is empty"
        );
    }

    #[test]
    fn zero_dynamic_imports_is_no_op() {
        // A project without dynamic imports: both prev and new companion sets
        // are empty — no files written or deleted, empty set returned.
        let dir = tempfile::tempdir().unwrap();
        let assets = dir.path().to_path_buf();

        let result = refresh_dev_island_chunks(&assets, &[], &HashSet::new()).unwrap();
        assert!(result.is_empty());
        // No files created in the assets dir.
        assert_eq!(std::fs::read_dir(&assets).unwrap().count(), 0);
    }

    #[test]
    fn kept_chunks_survive_rebundle() {
        // A chunk whose hash did not change (same dynamic import, same content)
        // should be retained rather than deleted and re-written.
        let dir = tempfile::tempdir().unwrap();
        let assets = dir.path().to_path_buf();

        let shared_chunk = "islands-chunk-STABLE00.js";
        let gen1 = vec![
            make_companion(shared_chunk, b"stable-content"),
            make_companion("islands-chunk-OLDCHUNK.js", b"old"),
        ];
        let prev = refresh_dev_island_chunks(&assets, &gen1, &HashSet::new()).unwrap();

        // Next bundle still has the stable chunk but dropped the old one.
        let gen2 = vec![make_companion(shared_chunk, b"stable-content")];
        let next = refresh_dev_island_chunks(&assets, &gen2, &prev).unwrap();

        assert!(
            assets.join(shared_chunk).exists(),
            "kept chunk must still exist"
        );
        assert!(
            !assets.join("islands-chunk-OLDCHUNK.js").exists(),
            "old chunk must be pruned"
        );
        assert_eq!(next.len(), 1);
        assert!(next.contains(shared_chunk));
    }

    #[test]
    fn rejects_unsafe_filenames() {
        let dir = tempfile::tempdir().unwrap();
        let assets = dir.path().to_path_buf();

        let bad_names = ["../escape.js", "subdir/chunk.js", "subdir\\chunk.js", ""];
        for name in bad_names {
            let companion = make_companion(name, b"bytes");
            let result = refresh_dev_island_chunks(&assets, &[companion], &HashSet::new());
            assert!(result.is_err(), "should reject unsafe filename {:?}", name);
        }
    }

    // ── Phase B skip-key tests (issue #940) ─────────────────────────────────
    //
    // All `embed_v8`-gated: `compute_bundle_skip_key` and the
    // `last_successful_skip_key` seams only exist on the V8 path.

    /// Helper: write `bytes` to a temp file and return its path.
    #[cfg(feature = "embed_v8")]
    fn write_temp_bundle(dir: &tempfile::TempDir, bytes: &[u8]) -> PathBuf {
        let p = dir.path().join("bundle.js");
        std::fs::write(&p, bytes).unwrap();
        p
    }

    /// Build a minimal `BundlerOutput` pointing at an existing file.
    #[cfg(feature = "embed_v8")]
    fn make_bundler_out(bundle_path: PathBuf) -> BundlerOutput {
        use zfb_build::bundler::BundleManifest;
        BundlerOutput {
            bundle_path: bundle_path.clone(),
            sourcemap_path: bundle_path.with_extension("js.map"),
            manifest: BundleManifest {
                framework: "preact".into(),
                jsx_import_source: "preact".into(),
                hydrate_shim_specifier: "zfb:internal/hydrate".into(),
                bundle_basename: "bundle.js".into(),
                routes: Vec::new(),
            },
            route_module_deps: Vec::new(),
            emitted_wasm_assets: Vec::new(),
        }
    }

    /// `compute_bundle_skip_key` returns `Some` for a valid bundle file.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn bundle_skip_key_returns_some_for_readable_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let out = make_bundler_out(write_temp_bundle(&dir, b"bundle-content"));
        let key = compute_bundle_skip_key(&out, &[]);
        assert!(key.is_some(), "should produce a key for a readable bundle");
    }

    /// `compute_bundle_skip_key` returns `None` when the bundle path does not
    /// exist — the caller must treat this as a forced full refresh.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn bundle_skip_key_returns_none_for_missing_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let out = make_bundler_out(dir.path().join("nonexistent.js"));
        let key = compute_bundle_skip_key(&out, &[]);
        assert!(key.is_none(), "missing bundle must yield None (no skip)");
    }

    /// Same bundle bytes + same routes → identical keys.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn bundle_skip_key_identical_for_same_bundle_and_routes() {
        let dir = tempfile::tempdir().unwrap();
        let out = make_bundler_out(write_temp_bundle(&dir, b"same-bytes"));
        let k1 = compute_bundle_skip_key(&out, &[]);
        let k2 = compute_bundle_skip_key(&out, &[]);
        assert_eq!(k1, k2, "identical inputs must produce identical keys");
    }

    /// Different bundle bytes → different keys (real edit defeats skip).
    #[cfg(feature = "embed_v8")]
    #[test]
    fn bundle_skip_key_changes_on_different_bundle_bytes() {
        let dir = tempfile::tempdir().unwrap();

        let out1 = make_bundler_out(write_temp_bundle(&dir, b"bundle-v1"));
        let k1 = compute_bundle_skip_key(&out1, &[]);

        // Overwrite same file with different content.
        std::fs::write(&out1.bundle_path, b"bundle-v2").unwrap();
        let k2 = compute_bundle_skip_key(&out1, &[]);

        assert_ne!(k1, k2, "changed bundle bytes must change the skip key");
    }

    /// A `pages/` route change (different source paths) defeats the skip even
    /// when bundle bytes are identical — satisfies Inv 2 (issue #935 §3).
    #[cfg(feature = "embed_v8")]
    #[test]
    fn bundle_skip_key_changes_on_route_universe_change() {
        use zfb_router::Route;

        let dir = tempfile::tempdir().unwrap();
        let out = make_bundler_out(write_temp_bundle(&dir, b"identical-bundle"));

        // Build two Route slices with different source paths.
        let make_route = |src: &str, segs: Vec<zfb_router::Segment>| Route {
            source_path: PathBuf::from(src),
            segments: segs,
            kind: zfb_router::RouteKind::Static,
            specificity: 1,
            output_extension: None,
            static_html: false,
        };

        let routes_a = vec![make_route("pages/index.tsx", vec![])];
        let routes_b = vec![
            make_route("pages/index.tsx", vec![]),
            make_route(
                "pages/about.tsx",
                vec![zfb_router::Segment::Static("about".into())],
            ),
        ];

        let k_a = compute_bundle_skip_key(&out, &routes_a);
        let k_b = compute_bundle_skip_key(&out, &routes_b);

        assert!(k_a.is_some());
        assert!(k_b.is_some());
        assert_ne!(
            k_a, k_b,
            "different route universe must change the skip key \
             even when bundle bytes are identical"
        );
    }

    /// Phase B — the first tick after boot must never skip: a fresh
    /// `DevRenderInner` holds no skip key, so `should_skip_refresh` is
    /// false for any computed key.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn fresh_dev_inner_never_skips() {
        let inner = stub_dev_inner(HashMap::new(), Vec::new());
        assert!(
            !inner.should_skip_refresh(Some([0x11u8; 32])),
            "a freshly-constructed DevRenderInner must not skip — \
             the first tick always runs fully"
        );
    }

    /// Phase B — a no-op tick skips: after a successful refresh committed
    /// key K, the next tick computing the same K skips host boot +
    /// `paths()` re-expansion. A different key (real edit) does not skip.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn identical_key_skips_after_successful_commit_and_real_edit_does_not() {
        let inner = stub_dev_inner(HashMap::new(), Vec::new());
        let key_k = [0xAAu8; 32];
        let key_edit = [0xBBu8; 32];

        // Tick 1: full refresh succeeds → commit K.
        inner.commit_skip_key(Some(key_k));

        // Tick 2: byte-identical bundle + unchanged routes → same key → skip.
        assert!(
            inner.should_skip_refresh(Some(key_k)),
            "an identical skip key after a successful commit must skip"
        );

        // Tick 3: real edit → different key → full refresh.
        assert!(
            !inner.should_skip_refresh(Some(key_edit)),
            "a different skip key (real edit) must defeat the skip"
        );
    }

    /// Phase B / Correctness Req 1 — a FAILED refresh must not poison the
    /// skip key: the failure path never calls `commit_skip_key`, so the
    /// next byte-identical tick still rebuilds fully.
    ///
    /// This drives the exact seam `refresh_bundle_and_routes` uses:
    /// `should_skip_refresh` at the top, `commit_skip_key` only on the
    /// all-steps-succeeded path (a host-start error `?`-returns before
    /// reaching it).
    ///
    /// Falsifiability: if the production code committed the key right
    /// after bundling (before host start), the failed tick here would
    /// have stored `key_k` and the second `should_skip_refresh(key_k)`
    /// would return true — freezing the stale renderer in place.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn failed_refresh_does_not_poison_skip_key() {
        let inner = stub_dev_inner(HashMap::new(), Vec::new());
        let key_k = [0xCCu8; 32];

        // Tick A: bundle computed key K, skip check says "no skip" (fresh
        // state) → full refresh attempted → host start FAILS. The error
        // propagates via `?` and commit_skip_key is never reached.
        assert!(!inner.should_skip_refresh(Some(key_k)));
        // (no commit — simulates the host-start failure)

        // Tick B: byte-identical bundle → same key K. MUST still rebuild
        // fully (no skip), because tick A never succeeded.
        assert!(
            !inner.should_skip_refresh(Some(key_k)),
            "a failed refresh must not poison the skip key — the next \
             byte-identical tick must rebuild fully"
        );

        // Tick B succeeds this time → commit. Now tick C may skip.
        inner.commit_skip_key(Some(key_k));
        assert!(
            inner.should_skip_refresh(Some(key_k)),
            "after the retry tick succeeds, the skip key is live again"
        );
    }

    /// Phase B (codex review fix) — a refresh that swapped the live host
    /// but then failed the route-table rebuild must invalidate the stored
    /// key at swap time. Otherwise a later tick that restores the OLD
    /// bundle (e.g. the user undoing the edit) would match the stale key
    /// and skip — freezing the failed tick's host in place.
    ///
    /// Drives the same seam sequence `refresh_bundle_and_routes` executes:
    /// `commit_skip_key(None)` immediately after the swap, final
    /// `commit_skip_key(Some(...))` never reached on the failure path.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn route_rebuild_failure_after_swap_invalidates_skip_key() {
        let inner = stub_dev_inner(HashMap::new(), Vec::new());
        let key_old = [0x11u8; 32]; // tick N's bundle
        let key_new = [0x22u8; 32]; // tick N+1's edited bundle

        // Tick N: full refresh succeeds → commit old key.
        inner.commit_skip_key(Some(key_old));

        // Tick N+1: edit → new key, no skip → host start OK → SWAP →
        // invalidate-at-swap → route-table rebuild FAILS (final commit
        // never reached).
        assert!(!inner.should_skip_refresh(Some(key_new)));
        inner.commit_skip_key(None); // the swap-time invalidation
                                     // (route rebuild fails here — no final commit)

        // Tick N+2: user undoes the edit → old bundle bytes → old key.
        // MUST NOT skip: the live host is tick N+1's renderer, not the
        // one key_old describes.
        assert!(
            !inner.should_skip_refresh(Some(key_old)),
            "a post-swap failure must invalidate the stored key so a \
             restored old bundle cannot skip against the wrong live host"
        );
    }

    /// Phase B — an uncomputable key (`None`) never skips, and committing
    /// `None` (successful refresh whose bundle could not be hashed) CLEARS
    /// a previously-stored key so a later matching tick cannot skip
    /// against the wrong host state.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn uncomputable_key_never_skips_and_clears_stored_key() {
        let inner = stub_dev_inner(HashMap::new(), Vec::new());
        let key_k = [0xDDu8; 32];

        // None never skips, even against a stored key.
        inner.commit_skip_key(Some(key_k));
        assert!(
            !inner.should_skip_refresh(None),
            "an uncomputable key must force a full refresh"
        );

        // A successful refresh that could not hash its bundle clears the
        // stored key — the old key no longer describes the live renderer.
        inner.commit_skip_key(None);
        assert!(
            !inner.should_skip_refresh(Some(key_k)),
            "committing None must clear the stored key so a later tick \
             with the old key cannot skip against the wrong host state"
        );
    }

    // ── Static pages/**.html skip-key coverage (issue #956 gate (a)) ────────
    //
    // Static `.html` page routes bypass the JS bundle entirely (the
    // renderer copies the source body from disk at render time), so their
    // CONTENT is invisible to bundle bytes and to the route signature.
    // `compute_bundle_skip_key` must fold their bodies into the key.

    /// Build a `static_html` route pointing at `src`.
    #[cfg(feature = "embed_v8")]
    fn make_static_html_route(src: PathBuf) -> zfb_router::Route {
        zfb_router::Route {
            source_path: src,
            segments: vec![zfb_router::Segment::Static("about".into())],
            kind: zfb_router::RouteKind::Static,
            specificity: 100,
            output_extension: None,
            static_html: true,
        }
    }

    /// Editing a static `.html` page's bytes must change the skip key even
    /// when bundle bytes and the route signature are identical — otherwise
    /// the edit would be silently swallowed by the Phase-B skip.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn bundle_skip_key_changes_on_static_html_edit() {
        let dir = tempfile::tempdir().unwrap();
        let out = make_bundler_out(write_temp_bundle(&dir, b"identical-bundle"));

        let html_path = dir.path().join("about.html");
        std::fs::write(&html_path, "<h1>v1</h1>").unwrap();
        let routes = vec![make_static_html_route(html_path.clone())];

        let k1 = compute_bundle_skip_key(&out, &routes);
        // Edit ONLY the static html body — same path, same route
        // signature, same bundle bytes.
        std::fs::write(&html_path, "<h1>v2</h1>").unwrap();
        let k2 = compute_bundle_skip_key(&out, &routes);

        assert!(k1.is_some());
        assert!(k2.is_some());
        assert_ne!(
            k1, k2,
            "a static pages/*.html content edit must defeat the skip \
             even when bundle bytes and route signature are unchanged"
        );
    }

    /// Unchanged static `.html` bodies keep the key stable — the skip
    /// still fires for genuine no-op ticks on projects with static pages.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn bundle_skip_key_stable_when_static_html_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let out = make_bundler_out(write_temp_bundle(&dir, b"identical-bundle"));

        let html_path = dir.path().join("about.html");
        std::fs::write(&html_path, "<h1>same</h1>").unwrap();
        let routes = vec![make_static_html_route(html_path)];

        let k1 = compute_bundle_skip_key(&out, &routes);
        let k2 = compute_bundle_skip_key(&out, &routes);
        assert!(k1.is_some());
        assert_eq!(k1, k2, "identical static html bodies → identical keys");
    }

    /// An unreadable static `.html` source forces `None` (full refresh) —
    /// the safe direction, mirroring the unreadable-bundle case.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn bundle_skip_key_returns_none_for_unreadable_static_html() {
        let dir = tempfile::tempdir().unwrap();
        let out = make_bundler_out(write_temp_bundle(&dir, b"bundle"));

        let routes = vec![make_static_html_route(dir.path().join("missing.html"))];
        let key = compute_bundle_skip_key(&out, &routes);
        assert!(
            key.is_none(),
            "an unreadable static html source must force a full refresh"
        );
    }

    // ── URL→route reverse-lookup index tests (issue #1019) ───────────────────

    /// Build a minimal `DevRenderSession` with a single SSG route at
    /// `url_path` so lookup tests don't need to reach `stub_dev_inner`
    /// directly each time.
    fn session_with_route(url_path: &str) -> DevRenderSession {
        let entry = RouteUniverseEntry {
            url_path: url_path.into(),
            output_path: PathBuf::from("index.html"),
            route_key: url_path.into(),
            static_html: false,
            source_path: None,
        };
        let mut routes: HashMap<PathBuf, Vec<DevRouteEntry>> = HashMap::new();
        routes.insert(
            PathBuf::from("pages/index.tsx"),
            vec![DevRouteEntry {
                entry,
                params: None,
            }],
        );
        DevRenderSession {
            inner: Arc::new(stub_dev_inner(routes, Vec::new())),
        }
    }

    /// Helper: assert `lookup_by_url` returns the expected entry `url_path`.
    fn assert_lookup(session: &DevRenderSession, request: &str, expected_url: &str) {
        let result = session.lookup_by_url(request);
        assert_eq!(
            result.as_ref().map(|e| e.url_path.as_str()),
            Some(expected_url),
            "lookup_by_url({request:?}) → expected {expected_url:?}, got {result:?}",
        );
    }

    /// Helper: assert `lookup_by_url` returns `None`.
    fn assert_no_lookup(session: &DevRenderSession, request: &str) {
        let result = session.lookup_by_url(request);
        assert!(
            result.is_none(),
            "lookup_by_url({request:?}) should be None but got {:?}",
            result.map(|e| e.url_path),
        );
    }

    /// Root `/` is reachable as `/` and `/index.html` (issue #1019).
    #[test]
    fn url_index_root_slash_and_index_html() {
        let session = session_with_route("/");
        assert_lookup(&session, "/", "/");
        assert_lookup(&session, "/index.html", "/");
    }

    /// `/posts/a` and `/posts/a/` resolve to the same entry (trailing-slash
    /// policy: both candidate forms are indexed).
    #[test]
    fn url_index_trailing_slash_equivalence() {
        let session = session_with_route("/posts/a");
        assert_lookup(&session, "/posts/a", "/posts/a");
        assert_lookup(&session, "/posts/a/", "/posts/a");
    }

    /// `/posts/a/index.html` resolves like `/posts/a/` (index.html duality).
    #[test]
    fn url_index_index_html_duality() {
        let session = session_with_route("/posts/a");
        assert_lookup(&session, "/posts/a/index.html", "/posts/a");
    }

    /// Query strings are stripped before lookup: `/posts/a/?x=1` → `/posts/a`.
    #[test]
    fn url_index_query_string_ignored() {
        let session = session_with_route("/posts/a");
        assert_lookup(&session, "/posts/a/?x=1", "/posts/a");
        assert_lookup(&session, "/posts/a?x=1&y=2", "/posts/a");
    }

    /// Percent-encoded paths are decoded before lookup:
    /// `/posts/caf%C3%A9` → decoded to `/posts/café` → resolves.
    #[test]
    fn url_index_percent_encoding_decoded() {
        let session = session_with_route("/posts/café");
        assert_lookup(&session, "/posts/caf%C3%A9", "/posts/café");
    }

    /// Non-HTML routes (e.g. `feed.xml`, `sitemap.xml`) are indexed verbatim
    /// because they have an explicit file extension — no slash/index variants.
    #[test]
    fn url_index_non_html_extension_routes() {
        let entry = RouteUniverseEntry {
            url_path: "/feed.xml".into(),
            output_path: PathBuf::from("feed.xml"),
            route_key: "/feed.xml".into(),
            static_html: false,
            source_path: None,
        };
        let mut routes: HashMap<PathBuf, Vec<DevRouteEntry>> = HashMap::new();
        routes.insert(
            PathBuf::from("pages/feed.xml.tsx"),
            vec![DevRouteEntry {
                entry,
                params: None,
            }],
        );
        let session = DevRenderSession {
            inner: Arc::new(stub_dev_inner(routes, Vec::new())),
        };
        assert_lookup(&session, "/feed.xml", "/feed.xml");
        // Trailing-slash normalisation: `/feed.xml/` → stripped → same key.
        assert_lookup(&session, "/feed.xml/", "/feed.xml");
        // Extension routes must NOT generate an `index.html` sub-key.
        assert_no_lookup(&session, "/feed.xml/index.html");
    }

    /// SSR-only routes (`prerender = false`, stored in `ssr_routes`) are NOT
    /// present in the url_index — they are served by the SSR leg.
    #[test]
    fn url_index_excludes_ssr_only_routes() {
        let ssr_entry = RouteUniverseEntry {
            url_path: "/ssr-page".into(),
            output_path: PathBuf::new(),
            route_key: "/ssr-page".into(),
            static_html: false,
            source_path: None,
        };
        // SSR entries go into `ssr_routes`, NOT `routes_by_source`.
        let session = DevRenderSession {
            inner: Arc::new(stub_dev_inner(HashMap::new(), vec![ssr_entry])),
        };
        assert_no_lookup(&session, "/ssr-page");
        assert_no_lookup(&session, "/ssr-page/");
    }

    /// Unknown routes return `None`.
    #[test]
    fn url_index_unknown_route_returns_none() {
        let session = session_with_route("/posts/a");
        assert_no_lookup(&session, "/does-not-exist");
        assert_no_lookup(&session, "/posts/b");
    }

    /// Multiple SSG routes coexist in the index independently.
    #[test]
    fn url_index_multiple_routes() {
        let make_entry = |url: &str| RouteUniverseEntry {
            url_path: url.into(),
            output_path: PathBuf::from(format!("{}/index.html", url.trim_start_matches('/'))),
            route_key: url.into(),
            static_html: false,
            source_path: None,
        };
        let mut routes: HashMap<PathBuf, Vec<DevRouteEntry>> = HashMap::new();
        routes.insert(
            PathBuf::from("pages/about.tsx"),
            vec![DevRouteEntry {
                entry: make_entry("/about"),
                params: None,
            }],
        );
        routes.insert(
            PathBuf::from("pages/blog/hello.tsx"),
            vec![DevRouteEntry {
                entry: make_entry("/blog/hello"),
                params: None,
            }],
        );
        let session = DevRenderSession {
            inner: Arc::new(stub_dev_inner(routes, Vec::new())),
        };
        assert_lookup(&session, "/about", "/about");
        assert_lookup(&session, "/about/", "/about");
        assert_lookup(&session, "/blog/hello", "/blog/hello");
        assert_lookup(&session, "/blog/hello/index.html", "/blog/hello");
    }

    // ── Static injected-route seeding (epic #1228, S3 #1231) ─────────────────

    /// A static injected seed merged via [`seed_injected_static_routes`] is
    /// reachable through `lookup_by_url` under its concrete URL (URL ==
    /// pattern) and all the normalised candidate keys, exactly like a normal
    /// static page — proving the seed lands in `url_index` correctly.
    #[test]
    fn injected_static_seed_is_reachable_via_url_index() {
        let seed = RouteUniverseEntry {
            url_path: "/preset-about".into(),
            output_path: PathBuf::from("preset-about/index.html"),
            route_key: "/preset-about".into(),
            static_html: false,
            source_path: None,
        };
        let mut routes: HashMap<PathBuf, Vec<DevRouteEntry>> = HashMap::new();
        seed_injected_static_routes(&mut routes, std::slice::from_ref(&seed));
        // Filed under the synthetic injected source key, NOT a real pages/ path.
        assert!(routes.contains_key(&injected_source_key("/preset-about")));
        assert!(!routes.keys().any(|k| k.starts_with("pages")));

        let session = DevRenderSession {
            inner: Arc::new(stub_dev_inner(routes, Vec::new())),
        };
        assert_lookup(&session, "/preset-about", "/preset-about");
        assert_lookup(&session, "/preset-about/", "/preset-about");
        assert_lookup(&session, "/preset-about/index.html", "/preset-about");
    }

    /// A user `pages/` route and a static injected route with the SAME URL
    /// must not BOTH reach the seed stage — precedence is enforced upstream in
    /// `resolve_dev_pages_root` (the survivor set drops the shadowed pattern),
    /// so the seed list handed here never contains a user-owned URL. This test
    /// pins the dev-side contract: when a user route already owns the source
    /// key namespace, the injected seed lives under a DISTINCT synthetic key,
    /// so the two never collide in `routes_by_source`, and the user page's own
    /// entry is the one a real (non-shadowed) lookup resolves. (The actual
    /// user-wins drop is asserted at the survivor-set level in
    /// `package_routes::tests::surviving_set_drops_user_shadowed_pattern`.)
    #[test]
    fn injected_seed_source_key_never_collides_with_user_pages() {
        // User owns /guide via a real pages/ source.
        let user_entry = RouteUniverseEntry {
            url_path: "/guide".into(),
            output_path: PathBuf::from("guide/index.html"),
            route_key: "/guide".into(),
            static_html: false,
            source_path: None,
        };
        let mut routes: HashMap<PathBuf, Vec<DevRouteEntry>> = HashMap::new();
        routes.insert(
            PathBuf::from("pages/guide.tsx"),
            vec![DevRouteEntry {
                entry: user_entry,
                params: None,
            }],
        );
        // A DISTINCT injected static route is seeded alongside.
        let seed = RouteUniverseEntry {
            url_path: "/preset-extra".into(),
            output_path: PathBuf::from("preset-extra/index.html"),
            route_key: "/preset-extra".into(),
            static_html: false,
            source_path: None,
        };
        seed_injected_static_routes(&mut routes, std::slice::from_ref(&seed));

        // The synthetic injected key is disjoint from the real pages/ key.
        assert!(routes.contains_key(Path::new("pages/guide.tsx")));
        assert!(routes.contains_key(&injected_source_key("/preset-extra")));
        assert_ne!(
            injected_source_key("/preset-extra"),
            PathBuf::from("pages/guide.tsx")
        );

        let session = DevRenderSession {
            inner: Arc::new(stub_dev_inner(routes, Vec::new())),
        };
        // Both the user page and the injected route resolve independently.
        assert_lookup(&session, "/guide", "/guide");
        assert_lookup(&session, "/preset-extra", "/preset-extra");
    }

    /// Seeding the same static route on a re-built table (simulating a swap)
    /// produces an identical entry under the SAME synthetic key, so the diff
    /// is stable — the seed never registers as `changed`/`vanished` on a tick.
    // `diff_route_tables` is part of the V8-gated refresh path.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn injected_static_seed_is_stable_across_reseed() {
        let seed = RouteUniverseEntry {
            url_path: "/preset-about".into(),
            output_path: PathBuf::from("preset-about/index.html"),
            route_key: "/preset-about".into(),
            static_html: false,
            source_path: None,
        };

        let mut first: HashMap<PathBuf, Vec<DevRouteEntry>> = HashMap::new();
        seed_injected_static_routes(&mut first, std::slice::from_ref(&seed));
        let mut second: HashMap<PathBuf, Vec<DevRouteEntry>> = HashMap::new();
        seed_injected_static_routes(&mut second, std::slice::from_ref(&seed));

        // diff_route_tables must see no churn for the seed source.
        let (changed, vanished) = diff_route_tables(&first, &second);
        assert!(
            changed.is_empty(),
            "re-seeding the same static route must not churn the diff: {changed:?}"
        );
        assert!(vanished.is_empty(), "no output path vanished: {vanished:?}");
    }

    /// `url_index_lookup_keys` normalisation edge cases for the root path.
    #[test]
    fn url_index_lookup_keys_root_variants() {
        // Empty string → root.
        let keys = url_index_lookup_keys("");
        assert!(keys.contains(&"/".to_string()));
        assert!(keys.contains(&"/index.html".to_string()));
        // All-slashes → root.
        let keys2 = url_index_lookup_keys("///");
        assert!(keys2.contains(&"/".to_string()));
    }

    /// Rebuild-swap coherence: after a table swap, lookups reflect the new
    /// tables and not the old ones (issue #1019).
    ///
    /// This test drives the swap seam directly — it constructs an inner with
    /// route A, then writes route B + a new url_index into the RwLock the same
    /// way `refresh_bundle_and_routes` does (without needing a live V8 host),
    /// and asserts the lookup after the swap returns B, not A.
    #[test]
    fn url_index_swap_coherence() {
        // Boot: one route at /old.
        let old_entry = RouteUniverseEntry {
            url_path: "/old".into(),
            output_path: PathBuf::from("old/index.html"),
            route_key: "/old".into(),
            static_html: false,
            source_path: None,
        };
        let mut old_routes: HashMap<PathBuf, Vec<DevRouteEntry>> = HashMap::new();
        old_routes.insert(
            PathBuf::from("pages/old.tsx"),
            vec![DevRouteEntry {
                entry: old_entry,
                params: None,
            }],
        );
        let inner = Arc::new(stub_dev_inner(old_routes, Vec::new()));
        let session = DevRenderSession {
            inner: Arc::clone(&inner),
        };

        // Verify old state.
        assert_lookup(&session, "/old", "/old");
        assert_no_lookup(&session, "/new");

        // Simulate P4 swap: new route at /new, old route removed.
        let new_entry = RouteUniverseEntry {
            url_path: "/new".into(),
            output_path: PathBuf::from("new/index.html"),
            route_key: "/new".into(),
            static_html: false,
            source_path: None,
        };
        let mut new_routes: HashMap<PathBuf, Vec<DevRouteEntry>> = HashMap::new();
        new_routes.insert(
            PathBuf::from("pages/new.tsx"),
            vec![DevRouteEntry {
                entry: new_entry,
                params: None,
            }],
        );
        let new_url_index = build_url_index(&new_routes);
        {
            let mut tables = inner.routes.write().unwrap();
            tables.routes_by_source = new_routes;
            tables.ssr_routes = Vec::new();
            tables.url_index = new_url_index;
        }

        // After swap: old route gone, new route visible.
        assert_no_lookup(&session, "/old");
        assert_lookup(&session, "/new", "/new");
        assert_lookup(&session, "/new/", "/new");
        assert_lookup(&session, "/new/index.html", "/new");
    }
}
