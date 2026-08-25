//! `zfb-server` — the dev-mode HTTP server for `zudo-front-builder`.
//!
//! This crate runs an [`axum`] server that serves the in-memory
//! page-cache HTML produced by [`zfb_build`]'s rebuild loop, the built
//! `dist/assets/` tree, and per-request on-disk fallbacks for the
//! project's `dist/` and `public/` directories so static files in
//! `public/` are reachable at the site root (e.g.
//! `public/logo.svg` → `/logo.svg`). Every served HTML
//! response has a small `<script src="/__zfb/livereload.js"></script>`
//! injected before `</body>`. That script opens an SSE connection to
//! `/__zfb/reload` and listens for two event types:
//!
//! - `page` — the browser does a full `location.reload()`.
//! - `css` — the browser bumps the query-string on every
//!   `<link rel="stylesheet">` to swap CSS without losing client-side
//!   state.
//!
//! ## How it plugs into [`zfb_build`]
//!
//! The bin crate that runs `zfb dev` owns:
//!
//! - a [`zfb_build::BuildOrchestrator`] (the rebuild loop),
//! - a [`tokio::sync::broadcast`] channel of [`livereload::ReloadEvent`]s,
//! - a [`routes::PageCache`] of rendered HTML keyed by URL path,
//! - this crate's [`serve`] task.
//!
//! The bin crate wires the orchestrator's `on_outcome` callback so that
//! every non-noop build tick is translated into [`ReloadEvent`]s via
//! [`livereload::outcome_to_events`] and fed into the broadcast
//! channel. The bin crate is also responsible for populating the
//! [`routes::PageCache`] from the orchestrator's render outputs — the
//! server itself only **reads** the cache.
//!
//! See the module docs of [`livereload`] for the full wiring snippet.
//!
//! ## Production caveat
//!
//! This crate is **dev-only**. It always injects the live-reload
//! script, hard-codes `Cache-Control: no-store` on HTML, and exposes a
//! `/__zfb/*` namespace. Production builds emit static files served by
//! a different runtime (Cloudflare Workers, an edge CDN, …) and must
//! not pull in `zfb-server`.

pub mod assets_containment;
pub mod embed;
pub mod embed_handlers;
pub mod host_validation;
pub mod inject;
pub mod injected_routes;
pub mod livereload;
pub mod middleware;
pub mod plugin_middleware;
pub mod redirects;
pub mod render_hook;
pub mod rewrite_prewarm;
pub mod routes;
pub mod ssr;

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::Context;
use tokio::net::TcpListener;
use tracing::info;

/// Publication status for one class of dev asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetSlot {
    /// This project has no asset of this class.
    NotExpected,
    /// Discovery or publication has not completed yet.
    Pending,
    /// The complete current set of publicly served URLs.
    Published { urls: Vec<String> },
}

/// Publication status of the document side of a Dev generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentSlot {
    /// A boot/tick transaction may still expose a partial document set.
    Pending,
    /// The complete eager SSG document set was written successfully.
    Published,
    /// Routes were published and armed for coherent render-on-request.
    ReadyOnRequest,
    /// This project has no SSG document generation to publish.
    NotExpected,
}

/// One atomic snapshot of all dev assets that affect hydration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevPublicationState {
    pub generation: u64,
    pub islands: AssetSlot,
    pub client_scripts: AssetSlot,
    pub documents: DocumentSlot,
    staged_islands: Option<AssetSlot>,
    staged_client_scripts: Option<AssetSlot>,
    retained_islands: Option<AssetSlot>,
    previous_documents: Option<DocumentSlot>,
    uncertain_document_write: bool,
}

impl DevPublicationState {
    /// Initial state while deferred asset publications are unresolved.
    pub fn pending() -> Self {
        Self {
            generation: 0,
            islands: AssetSlot::Pending,
            client_scripts: AssetSlot::Pending,
            documents: DocumentSlot::Pending,
            staged_islands: None,
            staged_client_scripts: None,
            retained_islands: None,
            previous_documents: None,
            uncertain_document_write: false,
        }
    }

    /// Compatibility constructor for callers that only exercise islands.
    pub fn from_islands_url(url: Option<String>) -> Self {
        Self {
            generation: 0,
            islands: Self::slot_from_urls(url.into_iter().collect()),
            client_scripts: AssetSlot::NotExpected,
            documents: DocumentSlot::Published,
            staged_islands: None,
            staged_client_scripts: None,
            retained_islands: None,
            previous_documents: None,
            uncertain_document_write: false,
        }
    }

    /// Begin one coherent document/entry publication transaction.
    pub fn begin_document_update(&mut self) {
        if self.previous_documents.is_none() {
            self.previous_documents = Some(self.documents);
            self.documents = DocumentSlot::Pending;
        }
    }

    /// Stage the complete islands URL set for the current transaction.
    pub fn stage_islands(&mut self, urls: Vec<String>) {
        self.staged_islands = Some(Self::slot_from_urls(urls));
    }

    /// Stage the complete client-script URL set for the current transaction.
    pub fn stage_client_scripts(&mut self, urls: Vec<String>) {
        self.staged_client_scripts = Some(Self::slot_from_urls(urls));
    }

    /// Atomically commit staged entries with a document publication boundary.
    pub fn commit_document_update(&mut self, documents: DocumentSlot) -> bool {
        self.commit_document_update_with_retention(documents, false)
    }

    /// Commit a document boundary while optionally preserving an older
    /// islands entry for lazy request-time fallback bodies.
    pub fn commit_document_update_with_retention(
        &mut self,
        documents: DocumentSlot,
        retain_fallback: bool,
    ) -> bool {
        if self.uncertain_document_write {
            return false;
        }
        if let Some(islands) = self.staged_islands.take() {
            let previous = std::mem::replace(&mut self.islands, islands);
            if !retain_fallback
                || !matches!(self.retained_islands, Some(AssetSlot::Published { .. }))
            {
                self.retained_islands = Some(previous);
            }
        } else if !retain_fallback {
            self.retained_islands = None;
        }
        if let Some(client_scripts) = self.staged_client_scripts.take() {
            self.client_scripts = client_scripts;
        }
        self.documents = documents;
        self.previous_documents = None;
        self.uncertain_document_write = false;
        self.bump_generation();
        true
    }

    /// Resolve the partial-write latch after the command-owned obligation
    /// ledger proves every failed source/output has been repaired or removed.
    pub fn resolve_document_uncertainty(&mut self) {
        self.uncertain_document_write = false;
    }

    /// Complete a transaction whose final outstanding document was written
    /// by the lazy request path. The generation becomes ready immediately,
    /// while the previous island declaration remains selectable until the
    /// next ordinary publication boundary can safely prune dependencies.
    pub fn commit_lazy_repair(&mut self) -> bool {
        self.resolve_document_uncertainty();
        self.commit_document_update_with_retention(DocumentSlot::ReadyOnRequest, true)
    }

    /// Commit staged entries while retaining the prior document semantics.
    /// Returns `false` when a prior document write may have failed partway;
    /// only a later complete document boundary may resolve that uncertainty.
    pub fn commit_entry_update(&mut self) -> bool {
        self.commit_entry_update_with_retention(false)
    }

    /// Entries-only counterpart used while lazy fallback documents still
    /// require the previous framework asset generation.
    pub fn commit_entry_update_with_retention(&mut self, retain_fallback: bool) -> bool {
        if self.uncertain_document_write {
            return false;
        }
        let documents = self.previous_documents.unwrap_or(self.documents);
        self.commit_document_update_with_retention(documents, retain_fallback)
    }

    /// Abort before any document could have changed and restore the last good
    /// public phase. Initial boot has no good phase, so its staged entries stay
    /// available behind `Pending` for a later successful document retry.
    pub fn abort_document_update_before_write(&mut self) {
        if self.uncertain_document_write {
            self.documents = DocumentSlot::Pending;
            return;
        }
        match self.previous_documents.take() {
            Some(DocumentSlot::Pending) | None => {
                self.documents = DocumentSlot::Pending;
            }
            Some(previous) => {
                self.documents = previous;
                self.staged_islands = None;
                self.staged_client_scripts = None;
            }
        }
    }

    /// Leave a possibly partially written document transaction unresolved.
    /// The committed generation/assets remain available for transition-safe
    /// injection, while readiness stays false until a later successful commit.
    pub fn leave_document_update_pending(&mut self) {
        self.documents = DocumentSlot::Pending;
        self.uncertain_document_write = true;
    }

    /// Select an islands URL that is safe on both sides of a document update:
    /// additions may use an already-written staged entry, while removals keep
    /// using the committed entry until the new document set commits.
    pub fn islands_urls_for_response(&self, body_has_islands: bool) -> &[String] {
        let transitioning = self.staged_islands.is_some() || self.retained_islands.is_some();
        if transitioning && !body_has_islands {
            return &[];
        }
        match (
            self.staged_islands.as_ref(),
            &self.islands,
            self.retained_islands.as_ref(),
        ) {
            (Some(AssetSlot::Published { urls }), _, _) => urls,
            (_, AssetSlot::Published { urls }, _) => urls,
            (_, _, Some(AssetSlot::Published { urls })) => urls,
            _ => &[],
        }
    }

    /// Whether entries and the document side are coherently published.
    pub fn is_ready(&self) -> bool {
        !matches!(self.islands, AssetSlot::Pending)
            && !matches!(self.client_scripts, AssetSlot::Pending)
            && !matches!(self.documents, DocumentSlot::Pending)
    }

    /// Compatibility immediate islands publication for standalone server
    /// callers. It clears islands candidate/fallback residue so a later
    /// response cannot override this value, but deliberately does not resolve
    /// an in-flight document transaction; transactional callers must use the
    /// stage/commit API as one coherent unit.
    pub fn publish_islands(&mut self, urls: Vec<String>) {
        self.bump_generation();
        self.islands = Self::slot_from_urls(urls);
        self.staged_islands = None;
        self.retained_islands = None;
    }

    /// Compatibility immediate client-script publication. As above, this
    /// clears the corresponding staged value without resolving documents.
    pub fn publish_client_scripts(&mut self, urls: Vec<String>) {
        self.bump_generation();
        self.client_scripts = Self::slot_from_urls(urls);
        self.staged_client_scripts = None;
    }

    fn slot_from_urls(urls: Vec<String>) -> AssetSlot {
        if urls.is_empty() {
            AssetSlot::NotExpected
        } else {
            AssetSlot::Published { urls }
        }
    }

    fn bump_generation(&mut self) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("dev publication generation overflowed");
    }
}

impl Default for DevPublicationState {
    fn default() -> Self {
        Self::pending()
    }
}

#[cfg(test)]
mod publication_state_tests {
    use super::{AssetSlot, DevPublicationState, DocumentSlot};

    #[test]
    fn entries_stay_uncommitted_until_document_boundary() {
        let mut state = DevPublicationState::pending();

        state.begin_document_update();
        state.stage_client_scripts(vec!["/assets/client/main.js".to_string()]);
        state.stage_islands(vec!["/assets/islands.js".to_string()]);

        assert_eq!(state.generation, 0);
        assert!(!state.is_ready());
        assert_eq!(state.client_scripts, AssetSlot::Pending);
        assert_eq!(state.islands, AssetSlot::Pending);
        assert_eq!(
            state.islands_urls_for_response(true),
            &["/assets/islands.js".to_string()]
        );
        assert!(state.islands_urls_for_response(false).is_empty());

        assert!(state.commit_document_update(DocumentSlot::Published));
        assert_eq!(state.generation, 1);
        assert!(state.is_ready());
        assert_eq!(
            state.client_scripts,
            AssetSlot::Published {
                urls: vec!["/assets/client/main.js".to_string()]
            }
        );
        assert_eq!(
            state.islands,
            AssetSlot::Published {
                urls: vec!["/assets/islands.js".to_string()]
            }
        );
    }

    #[test]
    fn empty_successful_publications_are_not_expected_after_no_page_commit() {
        let mut state = DevPublicationState::pending();

        state.begin_document_update();
        state.stage_client_scripts(Vec::new());
        state.stage_islands(Vec::new());
        assert!(state.commit_document_update(DocumentSlot::NotExpected));

        assert_eq!(state.generation, 1);
        assert!(state.is_ready());
        assert_eq!(state.client_scripts, AssetSlot::NotExpected);
        assert_eq!(state.islands, AssetSlot::NotExpected);
        assert_eq!(state.documents, DocumentSlot::NotExpected);
    }

    #[test]
    fn lazy_routes_can_commit_ready_on_request_without_ssg_documents() {
        let mut state = DevPublicationState::pending();
        state.begin_document_update();
        state.stage_client_scripts(Vec::new());
        state.stage_islands(Vec::new());
        assert!(state.commit_document_update(DocumentSlot::ReadyOnRequest));

        assert_eq!(state.generation, 1);
        assert!(state.is_ready());
        assert_eq!(state.documents, DocumentSlot::ReadyOnRequest);
    }

    #[test]
    fn final_lazy_request_repair_recovers_readiness_and_generation() {
        let mut state = DevPublicationState::pending();
        state.begin_document_update();
        state.stage_islands(vec!["/assets/islands.js".to_string()]);
        state.stage_client_scripts(Vec::new());
        state.leave_document_update_pending();

        assert!(!state.is_ready());
        assert_eq!(state.generation, 0);
        assert!(state.commit_lazy_repair());
        assert!(state.is_ready());
        assert_eq!(state.generation, 1);
        assert_eq!(state.documents, DocumentSlot::ReadyOnRequest);
    }

    #[test]
    fn failed_partial_update_stays_pending_with_transition_safe_islands() {
        let mut state = DevPublicationState::pending();
        state.begin_document_update();
        state.stage_client_scripts(Vec::new());
        state.stage_islands(Vec::new());
        assert!(state.commit_document_update(DocumentSlot::Published));

        state.begin_document_update();
        state.stage_islands(vec!["/assets/islands.js".to_string()]);
        state.leave_document_update_pending();

        assert_eq!(state.generation, 1);
        assert!(!state.is_ready());
        assert_eq!(
            state.islands_urls_for_response(true),
            &["/assets/islands.js".to_string()]
        );

        assert!(!state.commit_document_update(DocumentSlot::Published));
        state.resolve_document_uncertainty();
        assert!(state.commit_document_update(DocumentSlot::Published));
        assert_eq!(state.generation, 2);
        assert!(state.is_ready());
    }

    #[test]
    fn removal_retains_previous_island_for_response_transition() {
        let mut state =
            DevPublicationState::from_islands_url(Some("/assets/islands.js".to_string()));
        state.begin_document_update();
        state.stage_islands(Vec::new());
        assert_eq!(
            state.islands_urls_for_response(true),
            &["/assets/islands.js".to_string()]
        );
        assert!(state.islands_urls_for_response(false).is_empty());

        assert!(state.commit_document_update(DocumentSlot::Published));
        assert_eq!(state.islands, AssetSlot::NotExpected);
        assert_eq!(
            state.islands_urls_for_response(true),
            &["/assets/islands.js".to_string()]
        );
        assert!(state.islands_urls_for_response(false).is_empty());

        state.begin_document_update();
        assert!(state.commit_entry_update());
        assert!(state.islands_urls_for_response(true).is_empty());
    }

    #[test]
    fn lazy_fallback_survives_later_generations_until_non_preserving_boundary() {
        let mut state =
            DevPublicationState::from_islands_url(Some("/assets/islands.js".to_string()));
        state.begin_document_update();
        state.stage_islands(Vec::new());
        assert!(state.commit_document_update_with_retention(DocumentSlot::ReadyOnRequest, true,));

        state.begin_document_update();
        state.stage_client_scripts(vec!["/assets/client/later.js".to_string()]);
        assert!(state.commit_document_update_with_retention(DocumentSlot::ReadyOnRequest, true,));
        assert_eq!(
            state.islands_urls_for_response(true),
            &["/assets/islands.js".to_string()],
            "later lazy generations must retain the old marker-bearing fallback",
        );

        state.begin_document_update();
        assert!(state.commit_entry_update());
        assert!(state.islands_urls_for_response(true).is_empty());
    }

    #[test]
    fn entries_only_tick_cannot_commit_after_uncertain_partial_document_write() {
        let mut state = DevPublicationState::pending();
        state.begin_document_update();
        state.stage_islands(Vec::new());
        state.stage_client_scripts(Vec::new());
        assert!(state.commit_document_update(DocumentSlot::Published));

        state.begin_document_update();
        state.stage_islands(vec!["/assets/islands-c.js".to_string()]);
        state.leave_document_update_pending();

        state.begin_document_update();
        assert!(!state.commit_entry_update());
        assert_eq!(state.generation, 1);
        assert_eq!(state.documents, DocumentSlot::Pending);
        assert!(!state.is_ready());

        state.resolve_document_uncertainty();
        assert!(state.commit_document_update(DocumentSlot::Published));
        assert_eq!(state.generation, 2);
        assert!(state.is_ready());
    }

    #[test]
    fn pre_write_entry_error_restores_previous_good_generation() {
        let mut state =
            DevPublicationState::from_islands_url(Some("/assets/islands-p.js".to_string()));
        state.begin_document_update();
        state.stage_islands(vec!["/assets/islands-c.js".to_string()]);
        state.abort_document_update_before_write();

        assert_eq!(state.generation, 0);
        assert_eq!(state.documents, DocumentSlot::Published);
        assert!(state.is_ready());
        assert_eq!(
            state.islands_urls_for_response(true),
            &["/assets/islands-p.js".to_string()]
        );
    }

    #[test]
    fn immediate_publish_apis_clear_hidden_transactional_residue() {
        let mut state =
            DevPublicationState::from_islands_url(Some("/assets/islands-p.js".to_string()));
        state.begin_document_update();
        state.stage_islands(vec!["/assets/islands-c.js".to_string()]);
        state.stage_client_scripts(vec!["/assets/client/c.js".to_string()]);

        state.publish_islands(Vec::new());
        state.publish_client_scripts(vec!["/assets/client/immediate.js".to_string()]);
        assert!(state.islands_urls_for_response(true).is_empty());

        state.resolve_document_uncertainty();
        assert!(state.commit_document_update(DocumentSlot::Published));
        assert_eq!(state.islands, AssetSlot::NotExpected);
        assert_eq!(
            state.client_scripts,
            AssetSlot::Published {
                urls: vec!["/assets/client/immediate.js".to_string()]
            }
        );
    }
}

/// Shared handle to the current atomic dev publication state.
///
/// `None` (outer) means "no publication state configured for this server".
/// The islands slot carries the public URL the dev orchestrator wrote last
/// (`/assets/islands.js` for projects without a base prefix, or
/// `/foo/assets/islands.js` when `base: "/foo/"` is configured).
///
/// Reads happen on every served HTML response in dev mode; writes happen
/// once at boot and again on every islands-rebuild tick. The contention
/// is one writer thread vs many short-lived readers — [`RwLock`] is
/// the right shape (not a plain `Mutex`).
///
/// Cloning the [`Arc`] is cheap; the [`AppState`] holds one clone and the
/// bin crate's `run_islands` callback holds another.
pub type IslandsBundleUrl = Arc<RwLock<DevPublicationState>>;

/// Shared handle to the currently-emitted dev-mode CSS bundle URL
/// (issue #494 / #498).
///
/// Mirrors [`IslandsBundleUrl`] for CSS. `None` (outer) means "no CSS
/// path configured for this server" — the page handler skips `<link>`
/// injection entirely. `Some(url)` inside the lock carries the public
/// URL the dev orchestrator wrote last (`/assets/styles.css` for
/// projects without a base prefix, or `/foo/assets/styles.css` when
/// `base: "/foo/"` is configured).
///
/// Reads happen on every served HTML response in dev mode; writes happen
/// once at boot and again on every CSS-rebuild tick. The contention
/// is one writer thread vs many short-lived readers — [`RwLock`] is
/// the right shape (not a plain `Mutex`).
pub type CssBundleUrl = Arc<RwLock<Option<String>>>;

pub use embed::{Server, ServerBuilder, ServerHandle, ServerMode};
pub use embed_handlers::{
    EmbedHandler, EmbedHandlerFn, EmbedHandlerFuture, EmbedHandlerSet, RouteParams,
};
pub use host_validation::{apply_host_validation_layer, HostValidation};
pub use inject::{inject_livereload, inject_livereload_into_tree, LIVERELOAD_TAG};
pub use injected_routes::{pattern_matches, InjectedRouteSet};
pub use livereload::{outcome_to_events, IslandsBundleInfo, ReloadEvent, ReloadTx};
pub use plugin_middleware::{
    body_limit_layer, dispatch_plugin, origin_gate, path_matches_prefix, plugin_error_response,
    DevMiddlewareDispatcher, DevMiddlewareSet, PluginDispatchAttempt, PluginDispatchError,
    PluginDispatchOutcome, PluginRegistration, PluginRequest, PluginResponse,
    PluginResponseEncoding, PLUGIN_BODY_LIMIT_BYTES,
};
pub use redirects::{RedirectOutcome, Redirects, RedirectsHandle};
pub use render_hook::{RenderOnRequestHandle, RenderOnRequestHook};
pub use rewrite_prewarm::{
    prewarm_rewrite_targets, PrewarmPlan, PrewarmSkip, PrewarmSkipReason, PrewarmTarget,
};
pub use routes::{
    build_router, content_type_for_extension, resolve_content_type, AppState, CachedPage,
    PageCache, DEV_404_BODY,
};
pub use ssr::{
    SsrDispatchError, SsrDispatcher, SsrRequest, SsrResponse, SsrRouteRecord, SsrRouteSet,
    SsrRoutesHandle,
};
pub use zfb_build::InjectedRoute;

/// Options for [`serve`].
///
/// All paths must be absolute. The bin crate is expected to canonicalise
/// `project_root`, `dist_root`, and `public_root` before constructing
/// this struct so static-file serving is independent of the working
/// directory the server is launched from.
#[derive(Clone)]
pub struct ServeOpts {
    /// Project root (the directory `zfb dev` was invoked in). Stored
    /// here for diagnostics and for future use by middleware that
    /// wants to display "served from <project_root>" banners.
    pub project_root: PathBuf,

    /// Build output directory. `/assets/*` is served from
    /// `<dist_root>/assets/` (or as the boot-lazy-seed fallback when
    /// `dev_assets_root` is set — see [`Self::dev_assets_root`]).
    pub dist_root: PathBuf,

    /// Optional isolated dev-assets root (issue #1189). `zfb dev` passes
    /// `Some(<project_root>/.zfb-build/dev-assets)` and writes its STABLE
    /// assets there instead of into `dist/assets/`; the router then serves
    /// `/assets/*` from `<dev_assets_root>/assets/` first, falling back to
    /// `<dist_root>/assets/`. This keeps a one-off `zfb build` against the
    /// shared `dist/` from clobbering the dev-served stylesheet. `None`
    /// (preview / embed) keeps the single-root `dist_root` mount.
    pub dev_assets_root: Option<PathBuf>,

    /// Page (HTML) on-disk root used as the page-cache fallback inside
    /// the page handler. Issue #534: this used to alias `dist_root` for
    /// every caller, but in `zfb dev` we must serve dev-rendered HTML
    /// from a directory that the build pipeline does NOT touch (and
    /// vice-versa). Embed and preview-style callers pass the same value
    /// as `dist_root`; `zfb dev` passes its dev-only HTML root
    /// (`<project_root>/.zfb-build/dev-pages/`) so dev's per-route
    /// renders never overwrite the production output `pnpm preview`
    /// later serves.
    pub html_root: PathBuf,

    /// Project public-static directory. Files here are served at the
    /// site root via a per-request on-disk fallback inside the page
    /// handler — `public/logo.svg` → `GET /logo.svg`. The same shape
    /// `zfb build` produces (it copies `public/*` straight into
    /// `dist/`), so dev and prod URLs match.
    pub public_root: PathBuf,

    /// Address to bind. Defaults to `127.0.0.1:3000`.
    pub addr: SocketAddr,

    /// Page cache populated by the bin crate's render loop.
    pub pages: PageCache,

    /// Broadcast sender feeding the SSE live-reload channel. The bin
    /// crate sends [`ReloadEvent`]s into this from
    /// [`zfb_build::BuildOrchestrator::run`]'s `on_outcome` callback.
    pub broadcast: ReloadTx,

    /// Dev-middleware registrations from user plugins (Sub 3 / #108).
    /// `None` = no plugins declared a `devMiddleware` hook; the dev
    /// router skips the plugin-dispatch leg entirely. The bin crate
    /// (`zfb dev`) builds this from the long-lived plugin host.
    pub plugins: Option<DevMiddlewareSet>,

    /// Injected synthetic routes from user plugins' new `setup` hook
    /// (#255). `None` = no plugin called `injectRoute`. The dev
    /// router checks this set on every page-cache miss so the
    /// matched entrypoint is visible to follow-up renderer work
    /// without re-plumbing.
    pub injected_routes: Option<InjectedRouteSet>,

    /// Request-time SSR routes (issue #367 / Gap 1). `None` = the
    /// project has no `prerender = false` pages. When `Some`, the
    /// dev router checks every page-cache miss against this set —
    /// a hit dispatches through the V8 host and returns the rendered
    /// HTML at request time, matching the Cloudflare adapter's
    /// production semantics. See [`crate::ssr`] for the wire shape
    /// and precedence contract.
    ///
    /// The handle is an `Arc<RwLock<Option<SsrRouteSet>>>` (issue #807) so
    /// the per-tick renderer reload can swap in a fresh route set — adding
    /// or removing `prerender = false` routes mid-session becomes visible
    /// to the request dispatcher without a dev-server restart.
    pub ssr_routes: Option<crate::ssr::SsrRoutesHandle>,

    /// User-supplied `base` config value from `zfb.config.ts` (issue
    /// #229). Passed through verbatim — the dev server normalises it
    /// internally via [`zfb_types::dev_mount_prefix`] into the
    /// canonical `Some("/foo")` / `None` mount-prefix shape.
    ///
    /// When this resolves to a `Some(prefix)` the entire route table
    /// (page cache, assets, public, livereload, plugin middleware)
    /// mounts under `<prefix>/...`; bare `/` redirects to `<prefix>/`,
    /// other unprefixed paths fall through to a 404 with a hint.
    /// When this resolves to `None` (i.e. `base` is `None`, `""`,
    /// `"/"`, or an absolute URL) the route table is identical to the
    /// pre-`base` server byte-for-byte.
    pub base: Option<String>,

    /// Mirror of `zfb.config.ts`'s `trailingSlash` field. Threaded
    /// through to [`crate::routes::AppState::trailing_slash`] so the
    /// in-flight base-rewrite pass appends `/` to extensionless
    /// absolute hrefs the same way the production build does
    /// (sub #234 / zudolab/zudo-doc#1579).
    pub trailing_slash: bool,

    /// Server mode (Dev/Preview/Embed). Threaded through to
    /// [`crate::routes::AppState::mode`] so the router can gate Dev-only
    /// surface — `/__zfb/livereload.js`, `/__zfb/reload`, the
    /// livereload `<script>` injection into HTML, and the default
    /// `Cache-Control: no-store` shaping — to Dev only.
    ///
    /// Defaults to [`crate::ServerMode::Dev`] for byte-for-byte parity
    /// with the historical `serve_with_listener` shape used by `zfb dev`
    /// and the integration-test surface. Embed builds bypass this
    /// struct entirely (they go through [`crate::ServerBuilder`]).
    #[doc(alias = "ServerMode")]
    pub mode: crate::ServerMode,

    /// Shared handle to the current dev-mode islands bundle URL
    /// (issue #377). `None` for projects with no `"use client"`
    /// components (or when the bin crate has not seeded publication state);
    /// `Some(<arc>)` carrying the atomic publication state whose islands slot
    /// may contain a URL like `/assets/islands.js` that the
    /// page handler splices into every served HTML response's `<head>`
    /// via the byte-level helper in
    /// [`zfb_build::head_inject::inject_prod_head_assets`].
    ///
    /// Dev-only: the page handler only consults this field when
    /// `mode == ServerMode::Dev`. In Preview and Embed modes the dev
    /// orchestrator does not seed the bundle URL anyway, but the gate
    /// is also enforced at the response-shaping side
    /// (see [`crate::routes::page_response_bytes`]) so an Embed caller
    /// that accidentally passes a non-None value still gets
    /// production-shaped output.
    ///
    /// The bin crate (`zfb dev`) builds this handle once at boot, hands
    /// the same `Arc` to the orchestrator's `run_islands` callback (so
    /// rebuild ticks rewrite the URL), and threads it into `ServeOpts`
    /// before calling [`serve`].
    pub islands_bundle_url: Option<crate::IslandsBundleUrl>,

    /// Shared handle to the current dev-mode CSS bundle URL
    /// (issue #494 / #498). Mirrors `islands_bundle_url` for CSS.
    /// `None` for projects with Tailwind disabled, or when the bin crate
    /// has not yet seeded a bundle. When `Some`, the page handler splices
    /// a `<link rel="stylesheet" href="<url>">` tag into every served
    /// HTML response's `<head>` via
    /// [`zfb_build::head_inject::inject_prod_head_assets`].
    ///
    /// Dev-only: gated the same way as `islands_bundle_url` — Preview
    /// and Embed callers never inject even if they pass a non-`None` handle.
    pub css_bundle_url: Option<crate::CssBundleUrl>,

    /// User-supplied `allowedHosts` entries from `zfb.config.ts`
    /// (issue #931 / #919). Only consulted when the server is bound to
    /// a non-loopback interface — see [`crate::host_validation`] for
    /// the matching rules (exact, case-insensitive, port-stripped,
    /// leading-dot suffix). Empty for the default localhost bind.
    pub allowed_hosts: Vec<String>,

    /// The host string the user explicitly bound (`--host mydev.local`
    /// / `host` in config), always allowed by the host validator so the
    /// URL the CLI banner prints never 403s. `None` when the caller has
    /// no user-supplied host string in scope (tests, embed).
    pub bound_host: Option<String>,

    /// Optional render-on-request hook (issue #1020).
    ///
    /// When `Some` and the server is in [`crate::ServerMode::Dev`] mode,
    /// `serve_page` awaits this hook for every GET/HEAD request **before**
    /// consulting the in-memory page cache and the `html_root` disk read.
    /// The hook is responsible for making `html_root` fresh as a side
    /// effect; after it returns the server falls through to the existing
    /// `PageCache → html_root → public_root → dist_root (Dev boot-lazy
    /// seed)` waterfall unchanged.
    ///
    /// `None` in Preview, Embed, and all non-Dev modes — no hook fires and
    /// behaviour is byte-identical to the pre-hook server. `None` is also
    /// the correct value for the embed builder path (`embed.rs`) and any
    /// test that does not exercise the hook seam.
    ///
    /// See [`crate::render_hook`] for the trait contract and threading
    /// model.
    pub render_on_request_hook: Option<crate::render_hook::RenderOnRequestHandle>,

    /// Shared handle to the active `_redirects` ruleset (issue #1546).
    ///
    /// `serve_page` evaluates it on every GET/HEAD request, right after
    /// the plugin dev-middleware leg but ahead of embed handlers,
    /// request-time SSR, the render-on-request hook, and the
    /// `PageCache → html_root → public_root → dist_root` waterfall —
    /// mirroring Cloudflare Workers Static Assets, where `_redirects`
    /// is evaluated by the assets layer before the Worker script ever
    /// sees the request.
    ///
    /// `None` = no `_redirects` support for this caller (Preview /
    /// Embed / most tests) — the leg is skipped entirely, byte-identical
    /// to the pre-#1546 server. `zfb dev` builds `Some(handle)` at boot
    /// from `public/_redirects` (an empty [`Redirects`] when the file is
    /// missing) and rewrites the handle in place whenever its targeted
    /// watch observes a create/edit/delete of that file.
    pub redirects: Option<crate::RedirectsHandle>,
}

impl ServeOpts {
    /// The default bind address: `127.0.0.1:3000`.
    pub fn default_addr() -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], 3000))
    }
}

/// Run the dev server until `shutdown` resolves or the process exits.
///
/// Binds [`ServeOpts::addr`], builds the axum router via
/// [`build_router`], and serves it. The future resolves with `Ok(())`
/// once either the `shutdown` future completes (graceful stop) or axum
/// returns an error.
///
/// Pass `std::future::pending()` to run indefinitely until the process
/// is killed (matches the old behaviour).
///
/// Errors:
///
/// - failure to bind the address (port in use, permission denied, …),
/// - axum's serve loop returns an error.
///
/// This is a thin wrapper around [`serve_with_listener`]: it binds
/// `opts.addr` itself and hands the resulting [`TcpListener`] off.
/// Callers that need to know the actual bound port (e.g. integration
/// tests using ephemeral port 0) should bind their own listener and
/// call [`serve_with_listener`] directly.
pub async fn serve<S>(opts: ServeOpts, shutdown: S) -> anyhow::Result<()>
where
    S: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind(opts.addr)
        .await
        .with_context(|| format!("failed to bind dev server to {}", opts.addr))?;
    serve_with_listener(opts, listener, shutdown).await
}

/// Run the dev server on a pre-bound [`TcpListener`].
///
/// Useful when the caller needs to know the actual bound socket address
/// before the server starts accepting connections — for example
/// integration tests that bind `127.0.0.1:0` and then read
/// [`TcpListener::local_addr`] to learn the OS-chosen port.
///
/// `opts.addr` is ignored in favour of the listener's actual local
/// address (which is what gets logged).
///
/// The server runs until `shutdown` resolves. Pass
/// `std::future::pending()` to run until the process exits.
pub async fn serve_with_listener<S>(
    opts: ServeOpts,
    listener: TcpListener,
    shutdown: S,
) -> anyhow::Result<()>
where
    S: Future<Output = ()> + Send + 'static,
{
    // Issue #229: normalise `base` once at boot. The shared helper
    // (`zfb_types::dev_mount_prefix`) collapses every accepted shape
    // (None / "" / "/" / "/foo" / "/foo/" / "https://cdn…") into the
    // single canonical form the routing layer wants, so the rest of
    // the server only deals with `Option<String>` of the
    // leading-slash, no-trailing-slash kind.
    let base_prefix = zfb_types::dev_mount_prefix(opts.base.as_deref());
    // Issue #931: host validation keys off the listener's ACTUAL bound
    // address (not `opts.addr`, which is ignored by this entry point) —
    // a loopback bind disables enforcement entirely, anything else
    // enforces the allowlist.
    let actual = listener.local_addr().unwrap_or(opts.addr);
    let host_validation = HostValidation::for_bind(
        actual.ip(),
        opts.bound_host.as_deref(),
        &opts.allowed_hosts,
        opts.mode,
    );
    let state = AppState {
        mode: opts.mode,
        pages: opts.pages,
        broadcast: opts.broadcast,
        plugins: opts.plugins,
        injected_routes: opts.injected_routes,
        ssr_routes: opts.ssr_routes,
        // Rust-side embed handlers are an embed-API only seam — the
        // legacy `serve` / `serve_with_listener` entry points used by
        // `zfb dev` / `zfb preview` never register any. The embed
        // builder threads its own `AppState` directly.
        embed_handlers: None,
        dist_root: opts.dist_root.clone(),
        // Issue #1189 — isolated dev-assets root (or `None` for preview /
        // embed). See `ServeOpts::dev_assets_root`.
        dev_assets_root: opts.dev_assets_root.clone(),
        // Issue #534 — see `ServeOpts::html_root` for the dev / preview
        // / embed contract.
        html_root: opts.html_root.clone(),
        public_root: opts.public_root.clone(),
        base_prefix,
        trailing_slash: opts.trailing_slash,
        islands_bundle_url: opts.islands_bundle_url,
        css_bundle_url: opts.css_bundle_url,
        host_validation,
        render_on_request_hook: opts.render_on_request_hook,
        redirects: opts.redirects,
        // Perf #1145-3: pre-canonicalize stable root paths once at startup so
        // resolve_within_root skips the tokio::fs::canonicalize(root) syscall
        // on every disk-fallback hit.  std::fs is used here (blocking, startup
        // only); failures are non-fatal — the field stays None and the per-call
        // fallback path takes over.
        canonical_html_root: std::fs::canonicalize(&opts.html_root).ok(),
        canonical_dist_root: std::fs::canonicalize(&opts.dist_root).ok(),
        canonical_public_root: std::fs::canonicalize(&opts.public_root).ok(),
    };
    let router = build_router(state);

    info!(
        addr = %actual,
        project_root = %opts.project_root.display(),
        dist_root = %opts.dist_root.display(),
        public_root = %opts.public_root.display(),
        "zfb-server listening"
    );

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
        .context("zfb-server: axum::serve returned an error")?;

    Ok(())
}
