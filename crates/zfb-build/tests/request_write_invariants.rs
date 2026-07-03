//! Concurrency-sequence tests for the request-time write API
//! (issue #1024: `DevAssetPipeline::request_write` / [`RequestWriter`]).
//!
//! The request path shares the byte-dedup cache and the pipeline-wide
//! tick-vs-request exclusion lock with the tick path. These tests pin
//! the cross-path invariants:
//!
//! - A request-time write never interleaves with a tick's deferred-prune
//!   window (#727) — it blocks until the tick completes.
//! - Vanish eviction wins over a prior request-time write (#804): the
//!   file is deleted AND the dedup bookkeeping is evicted.
//! - Byte-dedup is coherent in both directions across the request path
//!   and the next tick (no spurious rewrite, no wrongly-skipped write).
//! - The ALL-LAZY PRUNE case: a route-structure tick performing zero
//!   renders/writes still deletes vanished route files and evicts their
//!   bookkeeping.
//! - Request-time writes never feed `BuildOutcome::pages_written` (no
//!   SSE) and run the same `validate_output_path` boundary check.
//! - The guarded variant (`request_write_guarded`, issue #1027 lazy
//!   race) runs its revalidation closure strictly under the exclusion
//!   lock and strictly before any other work; a `false` answer skips
//!   the write wholesale (no disk, no dedup commit), so a tick that
//!   superseded the request in the renderer-release → write gap keeps
//!   its fresher bytes authoritative.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use tempfile::tempdir;
use zfb_build::pipeline::{GuardedWriteOutcome, RequestWriteOutcome, RequestWriter};
use zfb_build::{
    AssetPipeline, BuildContext, DevAssetPipeline, PageSelection, RebuildPlan, RefreshOutcome,
    RelDistPath, RenderedPage,
};
use zfb_graph::PageId;

fn pid(s: &str) -> PageId {
    PageId::new(PathBuf::from(s))
}

fn rp(page: &str, output_path: &str, html: &str) -> RenderedPage {
    RenderedPage {
        page: pid(page),
        output_path: RelDistPath::new(output_path).unwrap(),
        html: html.into(),
        content_type: None,
    }
}

fn plan_for(pages: &[&str]) -> RebuildPlan {
    let mut sel = BTreeSet::new();
    for p in pages {
        sel.insert(pid(p));
    }
    let mut plan = RebuildPlan::empty();
    plan.pages = PageSelection::Specific(sel);
    plan
}

fn ctx_rendering(dist_root: PathBuf, rendered: Vec<RenderedPage>) -> BuildContext {
    BuildContext {
        dist_root,
        render_pages: Arc::new(move |_, _| Ok(rendered.clone())),
        run_css: None,
        run_islands: None,
        run_client_scripts: None,
        reload_renderer: None,
    }
}

fn ctx_with_reload(
    dist_root: PathBuf,
    rendered: Vec<RenderedPage>,
    outcome: RefreshOutcome,
) -> BuildContext {
    BuildContext {
        dist_root,
        render_pages: Arc::new(move |_, _| Ok(rendered.clone())),
        run_css: None,
        run_islands: None,
        run_client_scripts: None,
        reload_renderer: Some(Arc::new(move || Ok(outcome.clone()))),
    }
}

/// The request path runs the same `validate_output_path` boundary check
/// as the tick's write loop: escaping relative paths and absolute paths
/// are rejected before anything touches the filesystem.
#[test]
fn request_write_validates_output_path() {
    let dir = tempdir().unwrap();
    let pipeline = DevAssetPipeline::new();

    assert!(
        pipeline
            .request_write(dir.path(), Path::new("../escape.html"), b"x".to_vec())
            .is_err(),
        "`..` escaping dist_root must be rejected",
    );
    assert!(
        pipeline
            .request_write(dir.path(), Path::new("/abs/owned.html"), b"x".to_vec())
            .is_err(),
        "absolute output paths must be rejected",
    );
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "rejected writes must leave dist untouched",
    );

    let ok = pipeline
        .request_write(
            dir.path(),
            Path::new("lazy/index.html"),
            b"<p>ok</p>".to_vec(),
        )
        .unwrap();
    assert!(ok.written);
    assert_eq!(
        std::fs::read(dir.path().join("lazy/index.html")).unwrap(),
        b"<p>ok</p>",
    );
}

/// Byte-dedup coherence in both directions:
///
/// - a request-time write re-sending the bytes a tick wrote is skipped,
/// - the NEXT tick dedups against request-written bytes (no spurious
///   rewrite — and `pages_written` stays empty, pinning "request-time
///   writes never feed SSE"),
/// - a genuinely new tick render is NOT wrongly skipped afterwards.
#[test]
fn byte_dedup_is_coherent_across_request_writes_and_ticks() {
    let dir = tempdir().unwrap();
    let dist = dir.path().to_path_buf();
    let pipeline = DevAssetPipeline::new();
    let page = pid("/p/a.tsx");
    let plan = plan_for(&["/p/a.tsx"]);

    // Tick 1 writes v1.
    let first = pipeline
        .apply(
            &plan,
            &ctx_rendering(
                dist.clone(),
                vec![rp("/p/a.tsx", "a/index.html", "<p>v1</p>")],
            ),
        )
        .unwrap();
    assert_eq!(first.pages_written, vec![page.clone()]);

    // Request path dedups against the tick-written bytes.
    let same = pipeline
        .request_write(&dist, Path::new("a/index.html"), b"<p>v1</p>".to_vec())
        .unwrap();
    assert!(
        !same.written,
        "request write must dedup against the tick-written copy",
    );

    // New bytes at request time go through.
    let upd = pipeline
        .request_write(&dist, Path::new("a/index.html"), b"<p>v2</p>".to_vec())
        .unwrap();
    assert!(upd.written);
    assert_eq!(
        std::fs::read(dist.join("a/index.html")).unwrap(),
        b"<p>v2</p>"
    );

    // Tick 2 re-renders the same v2 bytes: the dedup cache holds the
    // request-written copy, so nothing is rewritten and pages_written
    // stays empty — the request write never surfaces in the SSE channel.
    let second = pipeline
        .apply(
            &plan,
            &ctx_rendering(
                dist.clone(),
                vec![rp("/p/a.tsx", "a/index.html", "<p>v2</p>")],
            ),
        )
        .unwrap();
    assert_eq!(second.pages_rendered, 1);
    assert!(
        second.pages_written.is_empty(),
        "the next tick must byte-dedup against the request-written copy; written={:?}",
        second.pages_written,
    );

    // Tick 3 renders genuinely new bytes — must NOT be wrongly skipped.
    let third = pipeline
        .apply(
            &plan,
            &ctx_rendering(
                dist.clone(),
                vec![rp("/p/a.tsx", "a/index.html", "<p>v3</p>")],
            ),
        )
        .unwrap();
    assert_eq!(third.pages_written, vec![page]);
    assert_eq!(
        std::fs::read(dist.join("a/index.html")).unwrap(),
        b"<p>v3</p>"
    );

    // Round-trip: the request path now dedups against the tick's v3.
    let same3 = pipeline
        .request_write(&dist, Path::new("a/index.html"), b"<p>v3</p>".to_vec())
        .unwrap();
    assert!(!same3.written);
}

/// #804: when a route vanishes, the tick's eviction wins over a prior
/// request-time write — the file is deleted from disk AND the dedup
/// bookkeeping is evicted, so a later identical request write re-runs
/// (a stale dedup entry would wrongly skip it and leave the route
/// 404ing with nothing on disk).
#[test]
fn vanish_eviction_wins_over_prior_request_write() {
    let dir = tempdir().unwrap();
    let dist = dir.path().to_path_buf();
    let pipeline = DevAssetPipeline::new();
    let gone_abs = dist.join("gone/index.html");

    // A lazy request-time write materialises the route.
    let w = pipeline
        .request_write(&dist, Path::new("gone/index.html"), b"<p>lazy</p>".to_vec())
        .unwrap();
    assert!(w.written);
    assert!(gone_abs.exists());

    // Route-structure tick: the reload reports the route vanished; the
    // lazy renderer renders nothing.
    let outcome = pipeline
        .apply(
            &plan_for(&["/p/gone.tsx"]),
            &ctx_with_reload(
                dist.clone(),
                vec![],
                RefreshOutcome::Refreshed {
                    vanished: vec![gone_abs.clone()],
                    changed_sources: vec![],
                },
            ),
        )
        .unwrap();
    assert!(
        !gone_abs.exists(),
        "vanish eviction must delete the lazily-written artifact",
    );
    assert!(outcome.pages_pruned.contains(&gone_abs));

    // Bookkeeping was evicted too: re-writing the SAME bytes is a cache
    // miss and must hit disk again.
    let rewritten = pipeline
        .request_write(&dist, Path::new("gone/index.html"), b"<p>lazy</p>".to_vec())
        .unwrap();
    assert!(
        rewritten.written,
        "eviction must clear the dedup entry — identical bytes re-write after a prune",
    );
    assert_eq!(std::fs::read(&gone_abs).unwrap(), b"<p>lazy</p>");
}

/// #727: a request-time write must never interleave with a tick's
/// deferred-prune window. The tick parks inside its render callback
/// (i.e. inside the exclusion window, before the deferred prune runs)
/// while a request write is fired from another thread — through the
/// clonable [`RequestWriter`] handle, mirroring how the dev server will
/// hold one after the pipeline moved into the orchestrator. The write
/// must block until the tick completes, then land AFTER the prune.
#[test]
fn request_write_never_interleaves_with_a_tick_prune_window() {
    let dir = tempdir().unwrap();
    let dist = dir.path().to_path_buf();
    let pipeline = Arc::new(DevAssetPipeline::new());
    let writer: RequestWriter = pipeline.request_writer();
    let target_abs = dist.join("racy/index.html");

    // The route exists from an earlier lazy write.
    pipeline
        .request_write(&dist, Path::new("racy/index.html"), b"<p>old</p>".to_vec())
        .unwrap();
    assert!(target_abs.exists());

    // The tick's render callback signals entry, then parks until gated.
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let (gate_tx, gate_rx) = mpsc::channel::<()>();
    let entered_tx = Mutex::new(entered_tx);
    let gate_rx = Mutex::new(gate_rx);
    let vanished = vec![target_abs.clone()];
    let ctx = BuildContext {
        dist_root: dist.clone(),
        render_pages: Arc::new(move |_, _| {
            entered_tx.lock().unwrap().send(()).unwrap();
            gate_rx.lock().unwrap().recv().unwrap();
            Ok(vec![])
        }),
        run_css: None,
        run_islands: None,
        run_client_scripts: None,
        reload_renderer: Some(Arc::new(move || {
            Ok(RefreshOutcome::Refreshed {
                vanished: vanished.clone(),
                changed_sources: vec![],
            })
        })),
    };

    let tick_pipeline = pipeline.clone();
    let tick = std::thread::spawn(move || {
        tick_pipeline
            .apply(&plan_for(&["/p/racy.tsx"]), &ctx)
            .unwrap()
    });

    // Wait until the tick is parked inside its exclusion window.
    entered_rx.recv().unwrap();

    // Fire the request write. It must NOT complete while the tick holds
    // the exclusion. (If exclusion is broken the flag flips true and the
    // assertion below fails; if it holds, the assertion passes regardless
    // of scheduling — no flaky timing dependence.)
    let write_done = Arc::new(AtomicBool::new(false));
    let write_done_w = write_done.clone();
    let dist_w = dist.clone();
    // Positive handshake: the writer thread announces it is about to call
    // request_write, whose very first action is to take the exclusion lock.
    // Receiving this proves the competing thread is scheduled and has
    // reached the blocking point — without it, `!write_done` could pass
    // vacuously simply because the thread never ran (scheduler starvation
    // under `cargo test --workspace`).
    let (attempt_tx, attempt_rx) = mpsc::channel::<()>();
    let writer_thread = std::thread::spawn(move || {
        attempt_tx.send(()).unwrap();
        let out = writer
            .request_write(
                &dist_w,
                Path::new("racy/index.html"),
                b"<p>request</p>".to_vec(),
            )
            .unwrap();
        write_done_w.store(true, Ordering::SeqCst);
        out
    });
    attempt_rx.recv().unwrap();

    // wait-ok: absence-window — the exclusion is a std `Mutex` with no
    // observable waiter state, so the sub-instruction gap between the
    // attempt handshake above and the thread actually parking in
    // `lock_exclusion` cannot be positively synchronized on. This window
    // bounds only that gap: with the thread proven live and inside the
    // API, a broken exclusion would let the (fast, non-blocking) write
    // complete and flip `write_done` within it.
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        !write_done.load(Ordering::SeqCst),
        "request_write must block while a tick holds the prune-coherent exclusion (#727)",
    );

    // Release the tick: it completes its deferred prune (deleting the
    // vanished file + evicting bookkeeping); only THEN does the request
    // write run.
    gate_tx.send(()).unwrap();
    let outcome = tick.join().unwrap();
    let write_out = writer_thread.join().unwrap();
    assert!(write_done.load(Ordering::SeqCst));

    assert!(outcome.pages_pruned.contains(&target_abs));
    // Serialized strictly after the prune: the prune can no longer
    // delete the fresh artifact (no write-after-prune orphan), and the
    // evicted dedup entry means the write actually ran.
    assert!(write_out.written);
    assert_eq!(std::fs::read(&target_abs).unwrap(), b"<p>request</p>");
}

/// #1027 guarded write — the revalidation verdict decides everything:
/// `true` runs the exact `request_write` sequence (write, then dedup
/// no-op on identical bytes); `false` skips wholesale — disk content
/// AND the dedup cache stay exactly as the last passing write left
/// them.
#[test]
fn guarded_write_passes_and_skips_by_revalidation_verdict() {
    let dir = tempdir().unwrap();
    let dist = dir.path().to_path_buf();
    let pipeline = DevAssetPipeline::new();
    let target = Path::new("lazy/index.html");

    // Passing revalidation behaves like request_write...
    assert_eq!(
        pipeline
            .request_write_guarded(&dist, target, b"<p>v1</p>".to_vec(), || true)
            .unwrap(),
        GuardedWriteOutcome::Written(RequestWriteOutcome { written: true }),
    );
    // ...byte-dedup included.
    assert_eq!(
        pipeline
            .request_write_guarded(&dist, target, b"<p>v1</p>".to_vec(), || true)
            .unwrap(),
        GuardedWriteOutcome::Written(RequestWriteOutcome { written: false }),
    );

    // Failing revalidation skips the write wholesale: new bytes never
    // reach disk...
    assert_eq!(
        pipeline
            .request_write_guarded(&dist, target, b"<p>superseded</p>".to_vec(), || false)
            .unwrap(),
        GuardedWriteOutcome::Skipped,
    );
    assert_eq!(std::fs::read(dist.join(target)).unwrap(), b"<p>v1</p>");
    // ...and the dedup cache was not poisoned by the skipped bytes:
    // re-sending v1 still dedups, and the skipped bytes still count as
    // a change.
    assert!(
        !pipeline
            .request_write(&dist, target, b"<p>v1</p>".to_vec())
            .unwrap()
            .written,
        "the cache must still hold the last PASSING write's bytes",
    );
    assert!(
        pipeline
            .request_write(&dist, target, b"<p>superseded</p>".to_vec())
            .unwrap()
            .written,
        "skipped bytes must not have been committed to the dedup cache",
    );
}

/// #1027: the revalidation closure short-circuits BEFORE the
/// `validate_output_path` boundary check — a failing guard skips ALL
/// work, so even a path that would be rejected returns `Skipped`
/// rather than an error, while a passing guard still hits the
/// validation wall.
#[test]
fn guarded_write_revalidates_before_path_validation() {
    let dir = tempdir().unwrap();
    let pipeline = DevAssetPipeline::new();

    assert_eq!(
        pipeline
            .request_write_guarded(
                dir.path(),
                Path::new("../escape.html"),
                b"x".to_vec(),
                || { false }
            )
            .unwrap(),
        GuardedWriteOutcome::Skipped,
        "a failing guard must short-circuit before validation",
    );
    assert!(
        pipeline
            .request_write_guarded(
                dir.path(),
                Path::new("../escape.html"),
                b"x".to_vec(),
                || { true }
            )
            .is_err(),
        "a passing guard still runs the write-boundary validation",
    );
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "neither call may touch dist",
    );
}

/// THE #1027 lazy race at this layer: a tick is in flight while the
/// request's guarded write fires. The revalidation must NOT run until
/// the tick completes (it answers under the exclusion lock, never
/// against mid-tick state); composed with a claim re-check that the
/// tick invalidates, the late write is skipped and the tick's fresher
/// bytes survive on disk and in the dedup cache.
#[test]
fn guarded_revalidation_runs_only_after_in_flight_tick_completes() {
    let dir = tempdir().unwrap();
    let dist = dir.path().to_path_buf();
    let pipeline = Arc::new(DevAssetPipeline::new());
    let writer: RequestWriter = pipeline.request_writer();
    let target_abs = dist.join("racy/index.html");

    // The route exists from an earlier lazy write (the "older world"
    // bytes the request just re-rendered).
    pipeline
        .request_write(&dist, Path::new("racy/index.html"), b"<p>old</p>".to_vec())
        .unwrap();

    // The tick parks inside its render callback (inside the exclusion
    // window), then eagerly re-renders the same route with fresher
    // bytes.
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let (gate_tx, gate_rx) = mpsc::channel::<()>();
    let entered_tx = Mutex::new(entered_tx);
    let gate_rx = Mutex::new(gate_rx);
    let rendered = vec![rp("/p/racy.tsx", "racy/index.html", "<p>tick</p>")];
    let ctx = BuildContext {
        dist_root: dist.clone(),
        render_pages: Arc::new(move |_, _| {
            entered_tx.lock().unwrap().send(()).unwrap();
            gate_rx.lock().unwrap().recv().unwrap();
            Ok(rendered.clone())
        }),
        run_css: None,
        run_islands: None,
        run_client_scripts: None,
        reload_renderer: None,
    };

    let tick_pipeline = pipeline.clone();
    let tick = std::thread::spawn(move || {
        tick_pipeline
            .apply(&plan_for(&["/p/racy.tsx"]), &ctx)
            .unwrap()
    });
    entered_rx.recv().unwrap();

    // Fire the request's guarded write: bytes from the older world,
    // and a revalidation that records when it runs and answers "the
    // claim is gone" (as the eager re-render's eviction would make it).
    let revalidated = Arc::new(AtomicBool::new(false));
    let revalidated_w = revalidated.clone();
    let dist_w = dist.clone();
    // Positive handshake: the writer thread announces it is about to call
    // request_write_guarded, whose first action is to take the exclusion
    // lock (the revalidation closure runs strictly under it). Receiving
    // this proves the competing thread reached the blocking point, so the
    // `!revalidated` assertion below is not a vacuous scheduler artifact.
    let (attempt_tx, attempt_rx) = mpsc::channel::<()>();
    let writer_thread = std::thread::spawn(move || {
        attempt_tx.send(()).unwrap();
        writer
            .request_write_guarded(
                &dist_w,
                Path::new("racy/index.html"),
                b"<p>request-old</p>".to_vec(),
                move || {
                    revalidated_w.store(true, Ordering::SeqCst);
                    false
                },
            )
            .unwrap()
    });
    attempt_rx.recv().unwrap();

    // wait-ok: absence-window — the exclusion is a std `Mutex` with no
    // observable waiter state, so the gap between the attempt handshake
    // and the thread parking in `lock_exclusion` cannot be positively
    // synchronized on. This window bounds only that gap: with the thread
    // proven live and inside the API, a broken exclusion would run the
    // revalidation closure against mid-tick state and flip `revalidated`.
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        !revalidated.load(Ordering::SeqCst),
        "revalidation must not run while a tick holds the exclusion — \
         it would answer against mid-tick state",
    );

    gate_tx.send(()).unwrap();
    tick.join().unwrap();
    let outcome = writer_thread.join().unwrap();
    assert!(revalidated.load(Ordering::SeqCst));
    assert_eq!(
        outcome,
        GuardedWriteOutcome::Skipped,
        "the superseded request write must be skipped",
    );
    assert_eq!(
        std::fs::read(&target_abs).unwrap(),
        b"<p>tick</p>",
        "the tick's fresher bytes survive the late request write",
    );
    // Dedup coherence: the cache holds the tick's copy, not the
    // skipped request bytes.
    assert!(
        !pipeline
            .request_write(&dist, Path::new("racy/index.html"), b"<p>tick</p>".to_vec())
            .unwrap()
            .written,
        "the dedup cache must hold the tick's bytes after the skip",
    );
}

/// ALL-LAZY PRUNE (#1024): a route-structure tick whose renderer
/// performs ZERO renders/writes still deletes vanished route files from
/// disk AND evicts their dedup bookkeeping — pruning must not depend on
/// the render/write loop having produced anything. Surviving lazily-
/// written routes are untouched on disk and keep their dedup entries.
#[test]
fn all_lazy_route_structure_tick_prunes_files_and_evicts_bookkeeping() {
    let dir = tempdir().unwrap();
    let dist = dir.path().to_path_buf();
    let pipeline = DevAssetPipeline::new();
    let gone_abs = dist.join("gone/index.html");
    let kept_abs = dist.join("kept/index.html");

    // Both routes were materialised lazily — the tick's write loop has
    // never run for them.
    pipeline
        .request_write(&dist, Path::new("gone/index.html"), b"<p>gone</p>".to_vec())
        .unwrap();
    pipeline
        .request_write(&dist, Path::new("kept/index.html"), b"<p>kept</p>".to_vec())
        .unwrap();

    // Route-structure tick: reload reports gone/ vanished; the lazy
    // renderer returns an empty set (zero renders, zero writes).
    let outcome = pipeline
        .apply(
            &plan_for(&["/p/gone.tsx", "/p/kept.tsx"]),
            &ctx_with_reload(
                dist.clone(),
                vec![],
                RefreshOutcome::Refreshed {
                    vanished: vec![gone_abs.clone()],
                    changed_sources: vec![],
                },
            ),
        )
        .unwrap();

    // Zero renders, zero writes — but the prune still ran.
    assert_eq!(outcome.pages_rendered, 0);
    assert!(outcome.pages_written.is_empty());
    assert_eq!(outcome.pages_pruned, vec![gone_abs.clone()]);
    assert!(
        !gone_abs.exists(),
        "vanished route file must be deleted even on an all-lazy tick",
    );
    assert!(kept_abs.exists(), "surviving route must be untouched");
    assert_eq!(std::fs::read(&kept_abs).unwrap(), b"<p>kept</p>");

    // Bookkeeping: the vanished path was evicted (same-bytes re-write
    // is a cache miss)...
    assert!(
        pipeline
            .request_write(&dist, Path::new("gone/index.html"), b"<p>gone</p>".to_vec())
            .unwrap()
            .written,
        "vanished path's dedup entry must be evicted",
    );
    // ...while the surviving path's dedup entry survived (same-bytes
    // re-write is a hit).
    assert!(
        !pipeline
            .request_write(&dist, Path::new("kept/index.html"), b"<p>kept</p>".to_vec())
            .unwrap()
            .written,
        "surviving path's dedup entry must survive the tick",
    );
}
