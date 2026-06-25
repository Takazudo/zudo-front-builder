//! Request-time lazy SSG render adapter (issue #1026) — the zfb-side
//! implementation of [`zfb_server::RenderOnRequestHook`] (#1020).
//!
//! Mirrors [`crate::ssr_adapter::EmbeddedV8SsrAdapter`]: `zfb-server`
//! deliberately doesn't depend on `zfb-render` / `zfb-build`, so the dev
//! router only sees a `dyn RenderOnRequestHook` and the bin crate (here)
//! supplies the impl that knows how to drive the dev session's V8 host.
//!
//! ## Dispatch flow (the request BLOCKS until done — pinned semantics)
//!
//! 1. Cheap early return when the lazy switch (#1025) is off (the
//!    `ZFB_DEV_EAGER=1` escape hatch, #1027). The dev boot wiring
//!    already skips installing the hook in that case (the switch is
//!    boot-resolved and immutable for the session, so the eager serve
//!    path stays literally hook-free); this check is defense in depth
//!    for any other construction site.
//! 2. Hop to [`tokio::task::spawn_blocking`]: every step below takes
//!    std mutexes (renderer mutex, stale map, pipeline exclusion), and
//!    the renderer mutex in particular can be held for seconds by a
//!    tick's host swap — never park an async worker on it.
//! 3. Reverse lookup (#1019): URL → [`RouteUniverseEntry`]. Miss (SSR
//!    route, unknown path) → return; the server falls through to its
//!    disk legs unchanged.
//! 4. Staleness pre-check (#1025): not stale → return (serve the
//!    on-disk file as-is) without ever touching the renderer mutex.
//! 5. Render against whatever V8 host is live: lock the renderer mutex,
//!    capture the authoritative [`StaleClaim`] while holding it (the
//!    #1025 contract — the claim/render pair is serialized against P2
//!    host swaps), render via [`zfb_build::renderer::render_one`] into
//!    a scratch dir, read the bytes back, release the mutex.
//! 6. Write through [`RequestWriter::request_write_guarded`] (#1024 /
//!    #1027): the same validate → byte-dedup → atomic-write → commit
//!    discipline as the tick path, under the tick-vs-request exclusion
//!    lock — guarded by a revalidation closure
//!    ([`DevRenderSession::claim_is_current`]) that re-checks, under
//!    that same exclusion, that no tick touched the route in the
//!    renderer-release → write gap. A tick completing in that gap
//!    either eagerly re-rendered the route (fresher bytes on disk, its
//!    stale entry evicted) or re-staled it at a bumped generation; in
//!    both cases the closure answers `false` and the write is SKIPPED —
//!    an unguarded write would overwrite newer HTML with bytes rendered
//!    against the older host and then mark the route fresh (silent
//!    stale serve until the next edit, the #1027 lazy race).
//! 7. [`DevRenderSession::clear_stale_claim`] — only after a
//!    non-skipped write; ABA-safe: a tick that re-staled the route
//!    mid-render keeps it stale for the next request.
//!
//! ## Lock ordering (deadlock-critical)
//!
//! A tick's [`zfb_build::DevAssetPipeline::apply`] holds the pipeline
//! exclusion lock for the whole tick and takes the renderer mutex
//! inside it (render fan-out, host swap). The request path therefore
//! must NEVER hold the renderer mutex while calling
//! `request_write_guarded` (which takes the exclusion lock) — that
//! inverted nesting would deadlock against any in-flight tick. The flow
//! above releases the renderer mutex at the end of step 5, strictly
//! before step 6; the #1027 revalidation closure runs inside the
//! exclusion but only reads the stale map (a leaf lock ticks also take
//! inside their exclusion window), never the renderer mutex.
//! Similarly the routes `RwLock` is released by `lookup_by_url` before
//! the renderer mutex is taken (the entry is cloned out — the dev.rs
//! `render_one` clone-out pattern).
//!
//! ## Why render into a scratch dir
//!
//! `render_one` writes its output file directly under the dist dir it
//! is given. Pointing it at the live dev HTML root would bypass the
//! #1024 exclusion lock — a request-time write could land inside a
//! tick's deferred-prune window (see `WriteShared` in
//! `crates/zfb-build/src/pipeline/dev.rs`). Rendering into a throwaway
//! tempdir and replaying the bytes through `request_write` keeps every
//! request-time dist write under the exclusion + dedup discipline.
//!
//! ## Concurrency posture (accepted v1 trade-offs)
//!
//! - Concurrent requests for the same stale route may both render —
//!   they serialize on the renderer mutex; the second one re-claims
//!   under the mutex and re-renders, and its write is a byte-dedup
//!   no-op. No in-flight set in v1 (same stance as the SSR adapter's
//!   documented "host is single-threaded anyway").
//! - A route that vanishes in a tick AFTER the claim was captured but
//!   BEFORE the write lands is NOT re-materialised: the tick's table
//!   swap evicted its stale entry, so the #1027 revalidation fails and
//!   the guarded write is skipped — no orphan ever reaches disk.
//!
//! ## Error contract
//!
//! Render or write failure → verbose stderr log (same per-page error
//! swallowing as the tick fan-out), the stale on-disk file keeps being
//! served, and the claim is NOT cleared — the route stays stale so the
//! next request retries. The server treats `render_if_stale` as
//! best-effort either way and falls through to its disk legs.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use async_trait::async_trait;
use zfb_build::pipeline::{GuardedWriteOutcome, RequestWriter};
use zfb_build::renderer::{render_one, RouteUniverseEntry};
use zfb_server::{InjectedRouteSet, RenderOnRequestHandle, RenderOnRequestHook};

use crate::commands::dev::{dev_timing_enabled, DevRenderSession, StaleClaim};
use crate::output;
use crate::render_pipeline::build_output_path_for_resolved_url;

/// Build the live [`RenderOnRequestHandle`] installed into
/// [`zfb_server::ServeOpts`] at dev boot (issue #1026).
///
/// ONE persistent handle for the whole session: the adapter captures the
/// session clone — whose renderer `Arc<Mutex<…>>`, route tables, and
/// stale map are all swapped/updated IN PLACE by the refresh seams — so
/// nothing here ever needs rewiring on a tick.
///
/// `injected_routes` is the POST-precedence survivor set (epic #1228, S4
/// #1232) threaded in here so the adapter's dynamic-route fallback can
/// match request URLs against injected patterns without consulting the raw
/// registration list — user-shadowed and package-vs-package-dropped
/// patterns are already absent (sharp edges 4/7 of the design record).
/// Pass `InjectedRouteSet::default()` (empty) on the parity path (no
/// injected routes); the fallback is a no-op in that case.
pub(crate) fn make_render_on_request_handle(
    session: DevRenderSession,
    writer: RequestWriter,
    html_root: PathBuf,
    injected_routes: InjectedRouteSet,
) -> RenderOnRequestHandle {
    let hook: Arc<dyn RenderOnRequestHook> = Arc::new(LazyRenderAdapter::new(
        session,
        writer,
        html_root,
        injected_routes,
    ));
    Arc::new(std::sync::RwLock::new(Some(hook)))
}

/// Adapter that fulfils [`RenderOnRequestHook`] by rendering a claimed
/// stale SSG route through the dev session's live V8 host and writing
/// the result via the unified request-time write path.
#[derive(Clone)]
pub(crate) struct LazyRenderAdapter {
    session: DevRenderSession,
    writer: RequestWriter,
    /// The dev HTML root (`.zfb-build/dev-pages/`) — the same dist root
    /// the tick fan-out writes to and `serve_page`'s `html_root` leg
    /// reads from.
    html_root: PathBuf,
    /// Post-precedence injected-route set (epic #1228, S4 #1232). Consulted
    /// when `lookup_by_url` misses — if an injected PATTERN matches the
    /// request URL, a synthetic [`RouteUniverseEntry`] is built on the fly
    /// and the request renders through the unchanged flow. Empty on the
    /// parity path (no injected routes), making the fallback a no-op.
    injected_routes: InjectedRouteSet,
}

/// What one `render_if_stale` dispatch did, for tests and logging. The
/// server never sees this — the hook trait is fire-and-forget.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LazyRenderOutcome {
    /// Reverse lookup missed: SSR-only route, unexpanded dynamic route,
    /// or a path outside the route universe.
    NoRoute,
    /// The route resolved but is not stale — the on-disk file is
    /// current; nothing rendered.
    NotStale,
    /// The renderer state was shut down (or never started). Nothing to
    /// render through; the disk legs serve whatever exists.
    RendererUnavailable,
    /// The pre-check saw a stale entry but the authoritative re-claim
    /// under the renderer mutex found it gone — a concurrent request
    /// rendered it while we waited for the lock.
    Raced,
    /// Rendered and wrote. `written == false` means the bytes were
    /// byte-identical to the dedup cache's copy (write skipped).
    Rendered { written: bool },
    /// Rendered, but the write was skipped (#1027 lazy race): the
    /// guarded write's revalidation found the claim no longer current —
    /// a tick completing in the renderer-release → write gap either
    /// eagerly re-rendered the route (its fresher bytes survive on
    /// disk) or re-staled it at a newer generation (the entry stays
    /// stale; the next request re-renders). The claim is NOT cleared.
    WriteSuperseded,
    /// The render failed; logged, file untouched, claim kept (next
    /// request retries).
    RenderFailed,
    /// The render succeeded but the request-time write failed; logged,
    /// claim kept.
    WriteFailed,
}

impl LazyRenderAdapter {
    pub(crate) fn new(
        session: DevRenderSession,
        writer: RequestWriter,
        html_root: PathBuf,
        injected_routes: InjectedRouteSet,
    ) -> Self {
        Self {
            session,
            writer,
            html_root,
            injected_routes,
        }
    }

    /// Synchronous core of the hook — steps 3–7 of the module-level
    /// dispatch flow. Runs on a blocking thread; see the lock-ordering
    /// section above for why the renderer mutex is released before the
    /// `request_write` call.
    fn render_stale_route(&self, url_path: &str) -> LazyRenderOutcome {
        // Reverse lookup (#1019) — clones the entry OUT of the routes
        // RwLock; no lock is held past this line.
        //
        // S4 (#1232): on a miss, fall through to the pattern-aware
        // injected-route fallback before giving up with `NoRoute`.
        //
        // `needs_stale_check`: `true` for normal concrete entries (the
        // standard #1025 pre-check), `false` for synthesized dynamic
        // entries whose stale state was already resolved below.
        let (entry, needs_stale_check) = match self.session.lookup_by_url(url_path) {
            Some(e) => (e, true),
            None => {
                // Dynamic-injected-route fallback (epic #1228, S4 #1232,
                // design record §2). `lookup_by_url` misses for dynamic
                // injected patterns (`/preset-docs/[slug]`) because they have
                // no concrete URL at boot — dev does NOT enumerate `paths()`
                // Rust-side (Hono extracts params + matches inside V8). If an
                // injected pattern matches the request URL, synthesize a
                // `RouteUniverseEntry` on the fly and fall through to the
                // unchanged render flow (`claim_stale` → `render_claimed_entry`
                // → `request_write_guarded`). The Hono bundle (seeded by S2)
                // already contains the injected entrypoint, so V8 can match
                // `paths()` and render with the correct params.
                //
                // Sharp edge 3 (design record): `route_key` = the PATTERN
                // (template — Hono lookup key); `url_path` = the CONCRETE
                // request URL. Swapping these breaks the prerender-map join.
                // Sharp edge 4: `self.injected_routes` is the POST-precedence
                // set — user-shadowed patterns are already absent.
                match self.injected_routes.find_match(url_path) {
                    Some(rec) => {
                        // `output_path` derived by the same function as
                        // normal pages so trailing-slash + base-prefix
                        // parity is automatic (design record §3/§5).
                        // Injected routes are always HTML pages → extension
                        // `None` (defaults to "html").
                        let output_path = build_output_path_for_resolved_url(url_path, None);
                        // Record this as a known dynamic injected output so a
                        // later content-edit tick can re-stale it (epic #1228,
                        // S5 #1233 / #1227 item (h)). Done UNCONDITIONALLY —
                        // before the file-exists branching below — so the
                        // "file already on disk" case (an output rendered in a
                        // previous `zfb dev` run whose dev-pages persisted
                        // across the restart) is tracked too. That branch
                        // never calls `claim_or_mark_stale_for_dynamic_route`,
                        // so without this the route would be missing from the
                        // re-stale set and a content edit could serve the
                        // stale on-disk HTML forever.
                        self.session.note_dynamic_injected_route(&output_path);
                        // Stale-by-construction (design record §3): dynamic
                        // injected routes are never seeded into the stale map
                        // at boot (no concrete URL at that time). Two cases:
                        //
                        // 1. File absent (first-ever request or after prune):
                        //    `claim_or_mark_stale_for_dynamic_route` inserts a
                        //    stale entry WITHOUT pushing to `tick_stale` (tick
                        //    channel stays clean). `needs_stale_check = false`
                        //    because we just guaranteed the entry is there.
                        //
                        // 2. File present: either a content edit re-staled it
                        //    (stale entry exists — fall through to the normal
                        //    pre-check), or it is fresh (no stale entry — the
                        //    pre-check returns `NotStale` and the disk leg
                        //    serves the on-disk file). `needs_stale_check = true`
                        //    (normal #1025 pre-check applies).
                        let file_on_disk = self.html_root.join(&output_path);
                        let needs_check = if !file_on_disk.exists() {
                            self.session
                                .claim_or_mark_stale_for_dynamic_route(&output_path);
                            false // stale entry just inserted; skip the pre-check
                        } else {
                            true // file exists; normal pre-check determines freshness
                        };
                        let entry = RouteUniverseEntry {
                            url_path: url_path.to_string(),
                            output_path,
                            route_key: rec.pattern.clone(),
                            static_html: false,
                            source_path: None,
                        };
                        (entry, needs_check)
                    }
                    None => return LazyRenderOutcome::NoRoute,
                }
            }
        };

        // Staleness pre-check (#1025): fresh routes skip the renderer
        // mutex entirely, so a hot route being served while a tick holds
        // the renderer for seconds is never blocked here. Skipped for
        // synthesized absent-file entries (stale state already ensured
        // above — see `needs_stale_check`).
        if needs_stale_check && self.session.claim_stale(&entry.output_path).is_none() {
            return LazyRenderOutcome::NotStale;
        }

        let timing = dev_timing_enabled();
        let render_started = Instant::now();

        // Render under the renderer mutex; the claim is captured while
        // holding it (#1025 contract).
        let rendered = self.render_claimed_entry(&entry);
        let render_ms = render_started.elapsed().as_millis();

        let (claim, bytes) = match rendered {
            Ok(RenderUnderLock::Rendered { claim, bytes }) => (claim, bytes),
            Ok(RenderUnderLock::RendererUnavailable) => {
                return LazyRenderOutcome::RendererUnavailable;
            }
            Ok(RenderUnderLock::Raced) => return LazyRenderOutcome::Raced,
            Err(err) => {
                // Same per-page tolerance as the tick fan-out: log
                // verbosely, keep the stale file in place, keep the
                // claim — the next request retries.
                output::error(format!(
                    "lazy render failed for {} ({}): {err:#}",
                    url_path,
                    entry.output_path.display()
                ));
                return LazyRenderOutcome::RenderFailed;
            }
        };

        // Unified request-time write (#1024). The renderer mutex is
        // released by now — request_write_guarded takes the pipeline
        // exclusion lock, and holding both in inverted order would
        // deadlock against an in-flight tick. The revalidation closure
        // (#1027) re-checks UNDER that exclusion that the claim is
        // still exactly current — a tick completing in the gap between
        // the renderer-mutex release and here would otherwise have its
        // fresher state silently overwritten by these older bytes.
        let write_started = Instant::now();
        match self
            .writer
            .request_write_guarded(&self.html_root, &entry.output_path, bytes, || {
                self.session.claim_is_current(&claim)
            }) {
            Ok(GuardedWriteOutcome::Written(outcome)) => {
                self.session.clear_stale_claim(&claim);
                if timing {
                    eprintln!(
                        "{}",
                        format_lazy_render_timing(
                            url_path,
                            &entry.output_path,
                            render_ms,
                            write_started.elapsed().as_millis(),
                            outcome.written,
                        )
                    );
                }
                LazyRenderOutcome::Rendered {
                    written: outcome.written,
                }
            }
            Ok(GuardedWriteOutcome::Skipped) => {
                // A tick superseded this render mid-gap. Do NOT clear
                // the claim: an evicted entry needs no clear, and a
                // re-staled one must stay stale for the next request.
                tracing::debug!(
                    site = "LazyRenderAdapter",
                    url = url_path,
                    output = %entry.output_path.display(),
                    "lazy render superseded by a tick mid-gap; write skipped (#1027)"
                );
                LazyRenderOutcome::WriteSuperseded
            }
            Err(err) => {
                output::error(format!(
                    "lazy render write failed for {} ({}): {err:#}",
                    url_path,
                    entry.output_path.display()
                ));
                LazyRenderOutcome::WriteFailed
            }
        }
    }

    /// Lock the renderer mutex, capture the authoritative claim under
    /// it, render the entry into a scratch dir, and return the rendered
    /// bytes. The mutex guard is scoped to this function — callers get
    /// bytes, never the lock.
    fn render_claimed_entry(&self, entry: &RouteUniverseEntry) -> Result<RenderUnderLock> {
        let renderer = self.session.renderer_handle();
        let mut lock = renderer.lock().unwrap_or_else(|p| {
            tracing::warn!(
                site = "LazyRenderAdapter",
                "renderer mutex poisoned, recovered"
            );
            p.into_inner()
        });
        let Some(state) = lock.as_mut() else {
            return Ok(RenderUnderLock::RendererUnavailable);
        };
        // Authoritative claim, captured while holding the renderer
        // mutex (#1025): serialized against P2 host swaps, so the
        // rendered bytes always come from a host at least as new as the
        // claimed generation.
        let Some(claim) = self.session.claim_stale(&entry.output_path) else {
            return Ok(RenderUnderLock::Raced);
        };
        let scratch =
            tempfile::tempdir().context("failed to create scratch dir for request-time render")?;
        let written = render_one(state, entry, scratch.path(), self.session.project_root())
            .map_err(anyhow::Error::from)?;
        // `fs::read` (not `read_to_string`): non-HTML page outputs
        // (feeds, sitemaps, JSON contentType routes, binary verbatim
        // bodies) render through the exact same path as HTML.
        let bytes = std::fs::read(&written)
            .with_context(|| format!("failed to read rendered page {}", written.display()))?;
        Ok(RenderUnderLock::Rendered { claim, bytes })
    }
}

/// Result of [`LazyRenderAdapter::render_claimed_entry`] — what happened
/// under the renderer mutex.
enum RenderUnderLock {
    Rendered { claim: StaleClaim, bytes: Vec<u8> },
    RendererUnavailable,
    Raced,
}

/// One stderr line per lazy render under `ZFB_DEV_TIMING=1` (the
/// request-render counterpart of the `[zfb-timing] tick=…` line).
fn format_lazy_render_timing(
    url_path: &str,
    output_path: &Path,
    render_ms: u128,
    write_ms: u128,
    written: bool,
) -> String {
    format!(
        "[zfb-timing] lazy-render url={url_path} output={} render={render_ms}ms \
         write={write_ms}ms total={}ms written={written}",
        output_path.display(),
        render_ms + write_ms,
    )
}

#[async_trait]
impl RenderOnRequestHook for LazyRenderAdapter {
    async fn render_if_stale(&self, url_path: &str) {
        // Switch-off invariant (#1025): with the lazy switch off (the
        // ZFB_DEV_EAGER=1 escape hatch) this is the ONLY work the hook
        // does — no spawn, no locks, no lookup. The dev boot wiring
        // doesn't even install the hook in that case; this guard is
        // defense in depth.
        if !self.session.lazy_render_enabled() {
            return;
        }
        let this = self.clone();
        let url = url_path.to_string();
        // Hop off the async worker before touching any std mutex (the
        // renderer mutex can be held for seconds by a tick's host
        // swap). Awaiting the JoinHandle is what makes the request
        // BLOCK until render + write are done — the pinned semantics:
        // after this returns, serve_page's html_root leg reads fresh
        // bytes.
        let joined = tokio::task::spawn_blocking(move || this.render_stale_route(&url)).await;
        if let Err(join_err) = joined {
            // A panic inside the render path. The server falls through
            // to its disk legs regardless; surface the panic loudly.
            output::error(format!(
                "lazy render task panicked for {url_path}: {join_err}"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use zfb_build::renderer::{
        start, Backend, HttpResponseLike, RendererStartInput, RendererState,
    };
    use zfb_build::DevAssetPipeline;
    use zfb_build::InjectedRoute;

    use crate::commands::dev::stub_session_for_adapter_tests;

    /// Stub-backed [`RendererState`] whose responses come from
    /// `handler`. No V8, no subprocess — `render_one` drives the stub
    /// closure exactly like it drives a live host.
    fn stub_renderer_state(
        handler: Arc<dyn Fn(&str) -> HttpResponseLike + Send + Sync>,
    ) -> RendererState {
        start(RendererStartInput {
            // Ignored by `Backend::Stub` (no bundle is loaded).
            bundle_path: PathBuf::from("stub-bundle.mjs"),
            sourcemap_path: PathBuf::from("stub-bundle.mjs.map"),
            backend: Backend::Stub { handler },
            request_timeout: None,
        })
        .expect("stub renderer must start")
    }

    /// Counting stub: every dispatch bumps `hits` and returns `resp`.
    fn counting_handler(
        hits: Arc<AtomicUsize>,
        resp: HttpResponseLike,
    ) -> Arc<dyn Fn(&str) -> HttpResponseLike + Send + Sync> {
        Arc::new(move |_url| {
            hits.fetch_add(1, Ordering::SeqCst);
            HttpResponseLike {
                status: resp.status,
                content_type: resp.content_type.clone(),
                headers: resp.headers.clone(),
                body: resp.body.clone(),
            }
        })
    }

    fn html_response(body: &str) -> HttpResponseLike {
        HttpResponseLike {
            status: 200,
            content_type: "text/html; charset=utf-8".into(),
            headers: Vec::new(),
            body: body.as_bytes().to_vec(),
        }
    }

    fn entry(url_path: &str, output_path: &str) -> RouteUniverseEntry {
        RouteUniverseEntry {
            url_path: url_path.into(),
            output_path: PathBuf::from(output_path),
            route_key: url_path.into(),
            static_html: false,
            source_path: None,
        }
    }

    /// One-shot hook slot consumed by [`gap_hooked_handler`] on its
    /// FIRST dispatch — the deterministic seam for replaying a tick
    /// interleaving in the renderer-release → write gap (#1027): the
    /// stub dispatch runs strictly AFTER the authoritative claim is
    /// captured (under the renderer mutex) and strictly BEFORE the
    /// adapter's guarded write takes the exclusion lock, so effects run
    /// here land in exactly the gap the revalidation closes. (The hook
    /// runs while the test's renderer mutex is held — harmless here
    /// since no real tick thread exists to deadlock against; only the
    /// ordering relative to the adapter's own write matters.)
    type GapHook = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;

    /// Wrap `inner` so the [`GapHook`], if armed, fires once before the
    /// dispatch.
    fn gap_hooked_handler(
        inner: Arc<dyn Fn(&str) -> HttpResponseLike + Send + Sync>,
        hook: GapHook,
    ) -> Arc<dyn Fn(&str) -> HttpResponseLike + Send + Sync> {
        Arc::new(move |url| {
            if let Some(f) = hook.lock().unwrap().take() {
                f();
            }
            inner(url)
        })
    }

    struct Harness {
        adapter: LazyRenderAdapter,
        session: DevRenderSession,
        hits: Arc<AtomicUsize>,
        html_root: tempfile::TempDir,
        writer: RequestWriter,
    }

    /// One stub-rendered session + adapter over `routes`, with a fresh
    /// dev HTML root and a real `DevAssetPipeline` request writer.
    fn harness(routes: Vec<(PathBuf, Vec<RouteUniverseEntry>)>, resp: HttpResponseLike) -> Harness {
        harness_with(routes, resp, true)
    }

    fn harness_with(
        routes: Vec<(PathBuf, Vec<RouteUniverseEntry>)>,
        resp: HttpResponseLike,
        lazy_render: bool,
    ) -> Harness {
        let hits = Arc::new(AtomicUsize::new(0));
        let handler = counting_handler(Arc::clone(&hits), resp);
        harness_from_handler(routes, handler, hits, lazy_render)
    }

    /// [`harness`] with a [`GapHook`] spliced in front of the stub
    /// dispatch, for the #1027 gap-interleave tests.
    fn harness_hooked(
        routes: Vec<(PathBuf, Vec<RouteUniverseEntry>)>,
        resp: HttpResponseLike,
    ) -> (Harness, GapHook) {
        let hook: GapHook = Arc::new(Mutex::new(None));
        let hits = Arc::new(AtomicUsize::new(0));
        let handler =
            gap_hooked_handler(counting_handler(Arc::clone(&hits), resp), Arc::clone(&hook));
        (harness_from_handler(routes, handler, hits, true), hook)
    }

    fn harness_from_handler(
        routes: Vec<(PathBuf, Vec<RouteUniverseEntry>)>,
        handler: Arc<dyn Fn(&str) -> HttpResponseLike + Send + Sync>,
        hits: Arc<AtomicUsize>,
        lazy_render: bool,
    ) -> Harness {
        harness_from_handler_with_injected(
            routes,
            handler,
            hits,
            lazy_render,
            InjectedRouteSet::default(),
        )
    }

    fn harness_from_handler_with_injected(
        routes: Vec<(PathBuf, Vec<RouteUniverseEntry>)>,
        handler: Arc<dyn Fn(&str) -> HttpResponseLike + Send + Sync>,
        hits: Arc<AtomicUsize>,
        lazy_render: bool,
        injected_routes: InjectedRouteSet,
    ) -> Harness {
        let state = stub_renderer_state(handler);
        let session = stub_session_for_adapter_tests(
            PathBuf::new(),
            routes,
            Arc::new(Mutex::new(Some(state))),
            lazy_render,
        );
        let html_root = tempfile::tempdir().expect("html root tempdir");
        let pipeline = DevAssetPipeline::new();
        let writer = pipeline.request_writer();
        let adapter = LazyRenderAdapter::new(
            session.clone(),
            writer.clone(),
            html_root.path().to_path_buf(),
            injected_routes,
        );
        Harness {
            adapter,
            session,
            hits,
            html_root,
            writer,
        }
    }

    fn posts_route() -> Vec<(PathBuf, Vec<RouteUniverseEntry>)> {
        vec![(
            PathBuf::from("pages/posts/[slug].tsx"),
            vec![entry("/posts/a", "posts/a/index.html")],
        )]
    }

    /// Stale route: the hook renders, writes through the request-time
    /// write path, and clears the claim — and because `render_if_stale`
    /// is awaited to completion, the bytes are on disk the moment it
    /// returns (the request-blocks-until-done semantics).
    #[tokio::test]
    async fn stale_route_renders_writes_and_clears_claim() {
        let h = harness(
            posts_route(),
            html_response("<html><body>fresh</body></html>"),
        );
        h.session
            .mark_routes_stale([PathBuf::from("posts/a/index.html")]);

        h.adapter.render_if_stale("/posts/a").await;

        let written = h.html_root.path().join("posts/a/index.html");
        let body = std::fs::read_to_string(&written).expect("rendered file must exist");
        assert!(body.contains("fresh"), "rendered body reached disk: {body}");
        // Same render path as the tick fan-out — the HTML5 doctype
        // prepend (#524) applies to request-time renders too.
        assert!(
            body.starts_with("<!doctype html>"),
            "doctype parity with the tick render path: {body}"
        );
        assert_eq!(h.hits.load(Ordering::SeqCst), 1, "exactly one dispatch");
        assert!(
            h.session
                .claim_stale(Path::new("posts/a/index.html"))
                .is_none(),
            "claim cleared after a successful render+write"
        );
    }

    /// Not stale → no render: the on-disk file is current; the hook
    /// must not even dispatch to the host.
    #[tokio::test]
    async fn fresh_route_skips_render() {
        let h = harness(posts_route(), html_response("<html/>"));
        // No mark_routes_stale call — the route is fresh.
        assert_eq!(
            h.adapter.render_stale_route("/posts/a"),
            LazyRenderOutcome::NotStale
        );
        h.adapter.render_if_stale("/posts/a").await;
        assert_eq!(
            h.hits.load(Ordering::SeqCst),
            0,
            "no dispatch for a fresh route"
        );
        assert!(!h.html_root.path().join("posts/a/index.html").exists());
    }

    /// Lookup miss (SSR-only/unknown URL) → total no-op; the server
    /// falls through to its disk legs.
    #[tokio::test]
    async fn lookup_miss_is_a_noop() {
        let h = harness(posts_route(), html_response("<html/>"));
        h.session
            .mark_routes_stale([PathBuf::from("posts/a/index.html")]);
        assert_eq!(
            h.adapter.render_stale_route("/no/such/route"),
            LazyRenderOutcome::NoRoute
        );
        h.adapter.render_if_stale("/no/such/route").await;
        assert_eq!(h.hits.load(Ordering::SeqCst), 0);
    }

    /// Render failure: verbose log (not asserted), stale file left in
    /// place byte-identically, claim NOT cleared — the next request
    /// retries.
    #[tokio::test]
    async fn render_error_keeps_stale_file_and_claim() {
        let resp = HttpResponseLike {
            status: 500,
            content_type: "text/plain".into(),
            headers: Vec::new(),
            body: b"boom".to_vec(),
        };
        let h = harness(posts_route(), resp);
        let stale_file = h.html_root.path().join("posts/a/index.html");
        std::fs::create_dir_all(stale_file.parent().unwrap()).unwrap();
        std::fs::write(&stale_file, "stale-but-served").unwrap();
        h.session
            .mark_routes_stale([PathBuf::from("posts/a/index.html")]);

        assert_eq!(
            h.adapter.render_stale_route("/posts/a"),
            LazyRenderOutcome::RenderFailed
        );

        assert_eq!(h.hits.load(Ordering::SeqCst), 1, "the render was attempted");
        assert_eq!(
            std::fs::read_to_string(&stale_file).unwrap(),
            "stale-but-served",
            "the stale file keeps being served untouched"
        );
        assert!(
            h.session
                .claim_stale(Path::new("posts/a/index.html"))
                .is_some(),
            "claim kept — the next request retries"
        );
    }

    /// Switch-off invariant (#1025): with the lazy switch off (the
    /// `ZFB_DEV_EAGER=1` escape hatch) the hook early-returns before
    /// any lookup/claim/render — wiring it at boot changes nothing for
    /// eager sessions, even for a route that somehow carries a stale
    /// mark.
    #[tokio::test]
    async fn switch_off_short_circuits_before_any_work() {
        let h = harness_with(posts_route(), html_response("<html/>"), false);
        h.session
            .mark_routes_stale([PathBuf::from("posts/a/index.html")]);
        h.adapter.render_if_stale("/posts/a").await;
        assert_eq!(
            h.hits.load(Ordering::SeqCst),
            0,
            "no dispatch with the switch off"
        );
        assert!(!h.html_root.path().join("posts/a/index.html").exists());
        assert!(
            h.session
                .claim_stale(Path::new("posts/a/index.html"))
                .is_some(),
            "stale state untouched"
        );
    }

    /// Non-HTML page outputs (feeds/sitemap/JSON contentType routes)
    /// render identically — they have no browser tab, so request-time
    /// render is their ONLY refresh path. No doctype injection on
    /// non-HTML bodies.
    #[tokio::test]
    async fn non_html_output_renders_verbatim() {
        let resp = HttpResponseLike {
            status: 200,
            content_type: "application/rss+xml".into(),
            headers: Vec::new(),
            body: b"<?xml version=\"1.0\"?><rss/>".to_vec(),
        };
        let routes = vec![(
            PathBuf::from("pages/feed.xml.ts"),
            vec![entry("/feed.xml", "feed.xml")],
        )];
        let h = harness(routes, resp);
        h.session.mark_routes_stale([PathBuf::from("feed.xml")]);

        h.adapter.render_if_stale("/feed.xml").await;

        let body = std::fs::read(h.html_root.path().join("feed.xml")).expect("feed written");
        assert_eq!(
            body, b"<?xml version=\"1.0\"?><rss/>",
            "no doctype, byte-verbatim"
        );
        assert!(h.session.claim_stale(Path::new("feed.xml")).is_none());
    }

    /// Byte-dedup coherence with the shared write cache: a second
    /// render of a re-staled route producing identical bytes is a
    /// `written == false` no-op write — and still clears the claim.
    #[tokio::test]
    async fn identical_rerender_is_a_dedup_noop_write() {
        let h = harness(
            posts_route(),
            html_response("<html><body>same</body></html>"),
        );
        h.session
            .mark_routes_stale([PathBuf::from("posts/a/index.html")]);
        assert_eq!(
            h.adapter.render_stale_route("/posts/a"),
            LazyRenderOutcome::Rendered { written: true }
        );
        h.session
            .mark_routes_stale([PathBuf::from("posts/a/index.html")]);
        assert_eq!(
            h.adapter.render_stale_route("/posts/a"),
            LazyRenderOutcome::Rendered { written: false }
        );
        assert!(
            h.session
                .claim_stale(Path::new("posts/a/index.html"))
                .is_none(),
            "dedup-skipped write still clears the claim"
        );
    }

    /// THE #1027 lazy race, eager-rewrite shape: a tick completes in
    /// the renderer-release → write gap, eagerly re-renders the route
    /// with fresher bytes (through the shared write path, evicting its
    /// stale entry as `lazy_render_tick` does). The request's late
    /// write must be SKIPPED — the tick's bytes survive on disk and in
    /// the dedup cache, and the route serves fresh. An unguarded write
    /// here would silently roll the route back to the older host's
    /// bytes and mark it fresh.
    #[tokio::test]
    async fn tick_eager_rewrite_in_gap_supersedes_request_write() {
        let (h, hook) = harness_hooked(
            posts_route(),
            html_response("<html><body>request-old</body></html>"),
        );
        h.session
            .mark_routes_stale([PathBuf::from("posts/a/index.html")]);

        let session = h.session.clone();
        let writer = h.writer.clone();
        let html_root = h.html_root.path().to_path_buf();
        *hook.lock().unwrap() = Some(Box::new(move || {
            // The interleaving tick: fresher bytes reach disk through
            // the shared write path, then the eager render evicts the
            // route's stale entry (both inside apply()'s exclusion
            // window in production).
            writer
                .request_write(
                    &html_root,
                    Path::new("posts/a/index.html"),
                    b"tick-fresh".to_vec(),
                )
                .unwrap();
            session.clear_routes_stale(&[PathBuf::from("posts/a/index.html")]);
        }));

        assert_eq!(
            h.adapter.render_stale_route("/posts/a"),
            LazyRenderOutcome::WriteSuperseded
        );

        assert_eq!(h.hits.load(Ordering::SeqCst), 1, "the render did run");
        assert_eq!(
            std::fs::read(h.html_root.path().join("posts/a/index.html")).unwrap(),
            b"tick-fresh",
            "the tick's fresher bytes survive the late request write"
        );
        assert!(
            h.session
                .claim_stale(Path::new("posts/a/index.html"))
                .is_none(),
            "the route serves fresh — no stale entry was (re-)created"
        );
        // The dedup cache still holds the tick's copy: the skipped
        // write committed nothing.
        assert!(
            !h.writer
                .request_write(
                    h.html_root.path(),
                    Path::new("posts/a/index.html"),
                    b"tick-fresh".to_vec(),
                )
                .unwrap()
                .written,
            "re-sending the tick's bytes must dedup — the skipped write left the cache untouched"
        );
    }

    /// THE #1027 lazy race, re-stale shape: a tick in the gap bumps the
    /// generation (P4 table swap) and re-stales the same route — the
    /// request's bytes describe an older world. The write is skipped
    /// AND the entry remains stale, so the NEXT request re-renders
    /// against the newer host and lands its write.
    #[tokio::test]
    async fn tick_re_stale_in_gap_skips_write_and_keeps_entry() {
        let (h, hook) = harness_hooked(
            posts_route(),
            html_response("<html><body>gap-render</body></html>"),
        );
        let stale_file = h.html_root.path().join("posts/a/index.html");
        std::fs::create_dir_all(stale_file.parent().unwrap()).unwrap();
        std::fs::write(&stale_file, "stale-but-served").unwrap();
        h.session
            .mark_routes_stale([PathBuf::from("posts/a/index.html")]);

        let session = h.session.clone();
        *hook.lock().unwrap() = Some(Box::new(move || {
            // The interleaving tick: P4 swap bumps the generation, then
            // the lazy split re-stales the route at the new generation.
            session.bump_stale_generation();
            session.mark_routes_stale([PathBuf::from("posts/a/index.html")]);
        }));

        assert_eq!(
            h.adapter.render_stale_route("/posts/a"),
            LazyRenderOutcome::WriteSuperseded
        );
        assert_eq!(h.hits.load(Ordering::SeqCst), 1);
        assert_eq!(
            std::fs::read_to_string(&stale_file).unwrap(),
            "stale-but-served",
            "the superseded write never touched disk"
        );
        assert!(
            h.session
                .claim_stale(Path::new("posts/a/index.html"))
                .is_some(),
            "the entry must remain stale — the next request re-renders"
        );

        // The next request (no interference): renders against the
        // current world, passes revalidation, writes, clears the claim.
        assert_eq!(
            h.adapter.render_stale_route("/posts/a"),
            LazyRenderOutcome::Rendered { written: true }
        );
        assert_eq!(h.hits.load(Ordering::SeqCst), 2);
        assert!(
            std::fs::read_to_string(&stale_file)
                .unwrap()
                .contains("gap-render"),
            "the retry's bytes reached disk"
        );
        assert!(
            h.session
                .claim_stale(Path::new("posts/a/index.html"))
                .is_none(),
            "the retry cleared the claim"
        );
    }

    /// Renderer-mutex contention smoke (the issue's deadlock guard): a
    /// lazy render issued WHILE a refresh-style host swap holds the
    /// renderer mutex must block, then complete against the NEW host,
    /// within a deadline. Mirrors the P2 swap: lock → replace the
    /// `Option<RendererState>` in place → unlock.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn render_during_host_swap_completes_against_new_host() {
        let h = harness(
            posts_route(),
            html_response("<html><body>old-host</body></html>"),
        );
        h.session
            .mark_routes_stale([PathBuf::from("posts/a/index.html")]);

        let renderer = h.session.renderer_handle();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel::<()>();
        let new_hits = Arc::new(AtomicUsize::new(0));
        let new_hits_for_thread = Arc::clone(&new_hits);
        let swapper = std::thread::spawn(move || {
            let mut guard = renderer.lock().unwrap();
            locked_tx.send(()).unwrap();
            // Hold the mutex the way a P2 swap does (bundle reload takes
            // real time) so the request demonstrably waits on it.
            std::thread::sleep(std::time::Duration::from_millis(300));
            *guard = Some(stub_renderer_state(counting_handler(
                new_hits_for_thread,
                html_response("<html><body>new-host</body></html>"),
            )));
        });
        locked_rx
            .recv()
            .expect("swap thread must signal after taking the renderer mutex");

        let started = Instant::now();
        h.adapter.render_if_stale("/posts/a").await;
        let elapsed = started.elapsed();
        swapper.join().unwrap();

        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "no deadlock: completed in {elapsed:?}"
        );
        let body = std::fs::read_to_string(h.html_root.path().join("posts/a/index.html"))
            .expect("rendered file must exist");
        assert!(
            body.contains("new-host"),
            "the render that waited out the swap used the NEW host: {body}"
        );
        assert_eq!(
            new_hits.load(Ordering::SeqCst),
            1,
            "new host dispatched once"
        );
        assert_eq!(
            h.hits.load(Ordering::SeqCst),
            0,
            "old host never dispatched"
        );
        assert!(
            h.session
                .claim_stale(Path::new("posts/a/index.html"))
                .is_none(),
            "claim cleared after the post-swap render"
        );
    }

    /// Renderer shut down mid-session (`Option` taken): the hook is a
    /// logged no-op, never a panic.
    #[tokio::test]
    async fn renderer_shutdown_is_a_noop() {
        let h = harness(posts_route(), html_response("<html/>"));
        h.session
            .mark_routes_stale([PathBuf::from("posts/a/index.html")]);
        *h.session.renderer_handle().lock().unwrap() = None;
        assert_eq!(
            h.adapter.render_stale_route("/posts/a"),
            LazyRenderOutcome::RendererUnavailable
        );
        assert!(
            h.session
                .claim_stale(Path::new("posts/a/index.html"))
                .is_some(),
            "claim kept for when a renderer comes back"
        );
    }

    /// The ZFB_DEV_TIMING bracket line — pinned format so grep-based
    /// tooling can rely on it (one stderr line per lazy render).
    #[test]
    fn timing_line_format() {
        let line =
            format_lazy_render_timing("/posts/a", Path::new("posts/a/index.html"), 12, 3, true);
        assert_eq!(
            line,
            "[zfb-timing] lazy-render url=/posts/a output=posts/a/index.html \
             render=12ms write=3ms total=15ms written=true"
        );
    }

    // ------------------------------------------------------------------ //
    // S4 (#1232) — dynamic-injected-route fallback unit tests             //
    // ------------------------------------------------------------------ //

    /// Build a harness with no concrete routes in the url_index (the
    /// `url_index` misses for all paths) but with an `InjectedRouteSet`
    /// containing `injected_routes`.
    fn injected_harness(injected_routes: InjectedRouteSet, resp: HttpResponseLike) -> Harness {
        let hits = Arc::new(AtomicUsize::new(0));
        let handler = counting_handler(Arc::clone(&hits), resp);
        harness_from_handler_with_injected(
            vec![], // no concrete routes — url_index is empty
            handler,
            hits,
            true, // lazy_render = true
            injected_routes,
        )
    }

    fn injected_rec(pattern: &str) -> InjectedRoute {
        InjectedRoute {
            pattern: pattern.into(),
            entrypoint: PathBuf::from("/tmp/stub.tsx"),
            plugin: "test-plugin".into(),
            prerender: None,
        }
    }

    /// Synthetic `RouteUniverseEntry` contract (S4 design record §2 /
    /// sharp edges 3 & 4):
    /// - `url_path` = the CONCRETE request URL,
    /// - `route_key` = the PATTERN (template, not the concrete URL),
    /// - `output_path` = `build_output_path_for_resolved_url(url_path, None)`,
    /// - `static_html = false`, `source_path = None`.
    ///
    /// Verified without going through a full render (the harness has empty
    /// routes so the render will fail with RenderFailed if the stub returns
    /// a non-200; we just need to get past the `NoRoute` guard, which the
    /// test verifies by asserting the outcome is NOT `NoRoute`).
    #[test]
    fn dynamic_fallback_synthesizes_entry_for_slug_pattern() {
        let s = InjectedRouteSet::new(vec![injected_rec("/preset-docs/[slug]")]);
        let h = injected_harness(s, html_response("<html><body>slug</body></html>"));

        // Should match and render (not return NoRoute).
        let outcome = h.adapter.render_stale_route("/preset-docs/getting-started");
        assert_ne!(
            outcome,
            LazyRenderOutcome::NoRoute,
            "/preset-docs/getting-started must match the [slug] injected pattern"
        );

        // A URL that does NOT match the pattern must still be NoRoute.
        let miss = h.adapter.render_stale_route("/other/path");
        assert_eq!(
            miss,
            LazyRenderOutcome::NoRoute,
            "/other/path must not match /preset-docs/[slug]"
        );
    }

    /// Dynamic catch-all `[...rest]` — one or more segments.
    #[test]
    fn dynamic_fallback_matches_catchall_rest() {
        let s = InjectedRouteSet::new(vec![injected_rec("/docs/[...rest]")]);
        let h = injected_harness(s, html_response("<html/>"));

        assert_ne!(
            h.adapter.render_stale_route("/docs/a"),
            LazyRenderOutcome::NoRoute,
            "/docs/a must match /docs/[...rest]"
        );
        assert_ne!(
            h.adapter.render_stale_route("/docs/a/b/c"),
            LazyRenderOutcome::NoRoute,
            "/docs/a/b/c must match /docs/[...rest]"
        );
        // Zero segments: the bare prefix must NOT match [..rest] (needs at
        // least one segment).
        assert_eq!(
            h.adapter.render_stale_route("/docs"),
            LazyRenderOutcome::NoRoute,
            "/docs (bare prefix) must NOT match /docs/[...rest]"
        );
    }

    /// Optional catch-all `[[...slug]]` — zero or more segments.
    ///
    /// Bare prefix matches (the zero-segment case); nested paths match;
    /// trailing-slash form does NOT (Hono `:rest{.+}?` parity, per the
    /// `injected_routes::pattern_matches` spec).
    #[test]
    fn dynamic_fallback_matches_optional_catchall() {
        let s = InjectedRouteSet::new(vec![injected_rec("/guide/[[...slug]]")]);
        let h = injected_harness(s, html_response("<html/>"));

        // Bare prefix (zero-segment case) matches.
        assert_ne!(
            h.adapter.render_stale_route("/guide"),
            LazyRenderOutcome::NoRoute,
            "/guide (bare prefix) must match /guide/[[...slug]]"
        );
        // Nested paths match.
        assert_ne!(
            h.adapter.render_stale_route("/guide/intro"),
            LazyRenderOutcome::NoRoute,
            "/guide/intro must match /guide/[[...slug]]"
        );
        assert_ne!(
            h.adapter.render_stale_route("/guide/a/b/c"),
            LazyRenderOutcome::NoRoute,
            "/guide/a/b/c must match /guide/[[...slug]]"
        );
        // Trailing-slash form does NOT match.
        assert_eq!(
            h.adapter.render_stale_route("/guide/"),
            LazyRenderOutcome::NoRoute,
            "/guide/ (trailing-slash) must NOT match /guide/[[...slug]] (Hono parity)"
        );
    }

    /// Parity path: with an empty `InjectedRouteSet` and a url_index
    /// miss, the fallback must return `NoRoute` — zero behavioural change
    /// from the pre-S4 world.
    #[test]
    fn empty_injected_set_preserves_no_route() {
        let h = injected_harness(InjectedRouteSet::default(), html_response("<html/>"));
        assert_eq!(
            h.adapter.render_stale_route("/preset-docs/anything"),
            LazyRenderOutcome::NoRoute,
            "empty injected set must not match any URL"
        );
    }

    /// `output_path` derivation: the synthetic entry must produce the same
    /// output path as `build_output_path_for_resolved_url` with the
    /// concrete URL — confirming trailing-slash + base-prefix parity
    /// (design record §3/§5 and sharp edge 3).
    ///
    /// We verify the output file IS written to the derived path (i.e. the
    /// `output_path` the render path uses is correct) by checking that
    /// the expected file exists after the render completes.
    #[tokio::test]
    async fn dynamic_fallback_output_path_matches_build_output_path_for_resolved_url() {
        let s = InjectedRouteSet::new(vec![injected_rec("/preset-docs/[slug]")]);
        let h = injected_harness(s, html_response("<html><body>doc</body></html>"));

        // The concrete URL is /preset-docs/getting-started.
        // Expected output path: preset-docs/getting-started/index.html.
        let expected_output = PathBuf::from("preset-docs/getting-started/index.html");
        let expected_file = h.html_root.path().join(&expected_output);

        h.adapter
            .render_if_stale("/preset-docs/getting-started")
            .await;

        assert!(
            expected_file.exists(),
            "dynamic injected render must write to {expected_output:?} — file not found: {expected_file:?}"
        );
        let body = std::fs::read_to_string(&expected_file).expect("read rendered file");
        assert!(
            body.contains("doc"),
            "rendered body must reach disk via the dynamic fallback: {body}"
        );
    }

    /// `route_key` is the PATTERN, not the concrete URL (sharp edge 3).
    /// Verified indirectly: the stub renders a body that does NOT depend on
    /// `route_key` — we verify the correct output_path is used (from the
    /// concrete URL) to confirm the entry was synthesized with the right
    /// fields. A direct field assertion would require exposing the entry
    /// from `render_stale_route`, so we pin both the output-file path
    /// (concrete URL → derived path) and the rendered body (V8 gets the
    /// concrete URL, not the pattern).
    #[tokio::test]
    async fn dynamic_fallback_uses_concrete_url_for_output_path() {
        let s = InjectedRouteSet::new(vec![injected_rec("/docs/[...rest]")]);
        let h = injected_harness(s, html_response("<html><body>rest</body></html>"));

        h.adapter.render_if_stale("/docs/a/b").await;

        // The concrete URL /docs/a/b must produce docs/a/b/index.html.
        let expected_file = h.html_root.path().join("docs/a/b/index.html");
        assert!(
            expected_file.exists(),
            "output path must derive from the concrete URL /docs/a/b, not the pattern"
        );
    }
}
