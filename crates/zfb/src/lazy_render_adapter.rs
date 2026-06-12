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
//! 1. Cheap early return when the lazy switch (#1025) is off. The dev
//!    boot wiring already skips installing the hook in that case (the
//!    switch is boot-resolved and immutable for the session, so the
//!    default-off serve path stays literally hook-free); this check is
//!    defense in depth for any other construction site.
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
//! 6. Write through [`RequestWriter::request_write`] (#1024): the same
//!    validate → byte-dedup → atomic-write → commit discipline as the
//!    tick path, under the tick-vs-request exclusion lock.
//! 7. [`DevRenderSession::clear_stale_claim`] — ABA-safe: a tick that
//!    re-staled the route mid-render keeps it stale for the next
//!    request.
//!
//! ## Lock ordering (deadlock-critical)
//!
//! A tick's [`zfb_build::DevAssetPipeline::apply`] holds the pipeline
//! exclusion lock for the whole tick and takes the renderer mutex
//! inside it (render fan-out, host swap). The request path therefore
//! must NEVER hold the renderer mutex while calling `request_write`
//! (which takes the exclusion lock) — that inverted nesting would
//! deadlock against any in-flight tick. The flow above releases the
//! renderer mutex at the end of step 5, strictly before step 6.
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
//!   BEFORE the write lands can be re-materialised on disk once; the
//!   tick's table swap evicted its stale entry, so `clear_stale_claim`
//!   is a no-op and the orphan is pruned by the next vanish diff.
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
use zfb_build::pipeline::RequestWriter;
use zfb_build::renderer::{render_one, RouteUniverseEntry};
use zfb_server::{RenderOnRequestHandle, RenderOnRequestHook};

use crate::commands::dev::{dev_timing_enabled, DevRenderSession, StaleClaim};
use crate::output;

/// Build the live [`RenderOnRequestHandle`] installed into
/// [`zfb_server::ServeOpts`] at dev boot (issue #1026).
///
/// ONE persistent handle for the whole session: the adapter captures the
/// session clone — whose renderer `Arc<Mutex<…>>`, route tables, and
/// stale map are all swapped/updated IN PLACE by the refresh seams — so
/// nothing here ever needs rewiring on a tick.
pub(crate) fn make_render_on_request_handle(
    session: DevRenderSession,
    writer: RequestWriter,
    html_root: PathBuf,
) -> RenderOnRequestHandle {
    let hook: Arc<dyn RenderOnRequestHook> =
        Arc::new(LazyRenderAdapter::new(session, writer, html_root));
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
    ) -> Self {
        Self {
            session,
            writer,
            html_root,
        }
    }

    /// Synchronous core of the hook — steps 3–7 of the module-level
    /// dispatch flow. Runs on a blocking thread; see the lock-ordering
    /// section above for why the renderer mutex is released before the
    /// `request_write` call.
    fn render_stale_route(&self, url_path: &str) -> LazyRenderOutcome {
        // Reverse lookup (#1019) — clones the entry OUT of the routes
        // RwLock; no lock is held past this line.
        let Some(entry) = self.session.lookup_by_url(url_path) else {
            return LazyRenderOutcome::NoRoute;
        };

        // Staleness pre-check (#1025): fresh routes skip the renderer
        // mutex entirely, so a hot route being served while a tick holds
        // the renderer for seconds is never blocked here.
        if self.session.claim_stale(&entry.output_path).is_none() {
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
        // released by now — request_write takes the pipeline exclusion
        // lock, and holding both in inverted order would deadlock
        // against an in-flight tick.
        let write_started = Instant::now();
        match self
            .writer
            .request_write(&self.html_root, &entry.output_path, bytes)
        {
            Ok(outcome) => {
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
        // Default-off invariant (#1025): with the lazy switch off this
        // is the ONLY work the hook does — no spawn, no locks, no
        // lookup. The dev boot wiring doesn't even install the hook in
        // that case; this guard is defense in depth.
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
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use zfb_build::renderer::{
        start, Backend, HttpResponseLike, RendererStartInput, RendererState,
    };
    use zfb_build::DevAssetPipeline;

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
            headers: BTreeMap::new(),
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

    struct Harness {
        adapter: LazyRenderAdapter,
        session: DevRenderSession,
        hits: Arc<AtomicUsize>,
        html_root: tempfile::TempDir,
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
        let state = stub_renderer_state(counting_handler(Arc::clone(&hits), resp));
        let session = stub_session_for_adapter_tests(
            PathBuf::new(),
            routes,
            Arc::new(Mutex::new(Some(state))),
            lazy_render,
        );
        let html_root = tempfile::tempdir().expect("html root tempdir");
        let pipeline = DevAssetPipeline::new();
        let adapter = LazyRenderAdapter::new(
            session.clone(),
            pipeline.request_writer(),
            html_root.path().to_path_buf(),
        );
        Harness {
            adapter,
            session,
            hits,
            html_root,
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
            headers: BTreeMap::new(),
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

    /// Default-off invariant (#1025): with the lazy switch off the hook
    /// early-returns before any lookup/claim/render — wiring it at boot
    /// changes nothing for today's eager sessions, even for a route
    /// that somehow carries a stale mark.
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
            headers: BTreeMap::new(),
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
}
