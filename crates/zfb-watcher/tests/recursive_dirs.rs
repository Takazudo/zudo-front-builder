//! Observable-delivery integration tests for
//! [`zfb_watcher::Watcher::sync_recursive_dir_watches`] (issue #1801,
//! epic #1799 — CSS sibling mirror roots).
//!
//! Every contract assertion here is DELIVERY-level: a live `notify`
//! watcher over real tempdir filesystems, asserting what arrives (or
//! provably does not arrive) on the debounced receiver. Registration
//! state alone proves nothing about suppression, so no test stops at
//! "the root is in the watched set".
//!
//! ## How absence is proven without fixed sleeps
//!
//! All waits are condition-keyed (CLAUDE.md deflaking rules; macOS
//! FSEvents has a per-stream startup dead window that fixed sleeps
//! cannot reliably clear):
//!
//! 1. **Prove the stream live first** ([`sentinel_round_trip`], the
//!    `zfb_test_utils::watcher_live_handshake` pattern): fresh-named
//!    marker files are written until one is delivered. Without this, a
//!    "suppressed" write could really have been eaten by the dead
//!    window and the test would pass against a broken filter.
//! 2. **Doubted write, then sentinel round trip**: the doubted write
//!    happens BEFORE the sentinel writes on the same live stream, so if
//!    the doubted path were delivered at all, its debounce entry would
//!    be flushed no later than the drain that releases the sentinel.
//! 3. **Batch-tail drain** ([`drain_batch_tail`]): emission order
//!    within one drain batch is arbitrary (HashMap iteration), so a
//!    same-batch straggler can follow the sentinel; the tail drain
//!    (consecutive-quiet bounded, not wall-clock-from-start) collects
//!    it before the absence assertion runs.
//!
//! Timeouts below are give-up failsafes bounding runtime on a
//! regression — never proof deadlines.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::tempdir;
use tokio::sync::mpsc;
use zfb_test_utils::{watcher_live_handshake, HandshakeOpts};
use zfb_watcher::{Change, Watcher};

const DEBOUNCE: Duration = Duration::from_millis(50);
/// Give-up failsafe for each handshake/sentinel round trip.
const ROUND_TRIP_DEADLINE: Duration = Duration::from_secs(10);
/// The command layer's skip list is private to the `zfb` crate by design
/// (contract point 1) — the API takes the names as a parameter, so the
/// tests supply their own.
const SKIPS: [&str; 3] = ["node_modules", "dist", "target"];

/// Write fresh-named sentinel files directly into `sentinel_dir` until one
/// is delivered, collecting EVERY change seen along the way (absence
/// assertions run over the returned vec). Handshake-shaped so a freshly
/// created or rebuilt FSEvents stream's startup dead window cannot flake
/// the round trip. Also serves as the prove-live step: `sentinel_dir` must
/// be inside the watch scope under test, and a `live` result IS the
/// delivery proof for that scope.
async fn sentinel_round_trip(
    rx: &mut mpsc::Receiver<Change>,
    sentinel_dir: &Path,
    label: &str,
) -> Vec<Change> {
    let mut seen = Vec::new();
    let res = watcher_live_handshake(
        HandshakeOpts::new(ROUND_TRIP_DEADLINE),
        |idx| {
            std::fs::write(
                sentinel_dir.join(format!("sentinel-{label}-{idx}.txt")),
                b"sentinel",
            )
            .expect("write sentinel file");
        },
        || loop {
            match rx.try_recv() {
                Ok(change) => {
                    let hit = change.path.starts_with(sentinel_dir);
                    seen.push(change);
                    if hit {
                        break true;
                    }
                }
                Err(_) => break false,
            }
        },
    )
    .await;
    assert!(
        res.live,
        "sentinel under {sentinel_dir:?} never delivered ({label}); \
         collected so far: {seen:#?}",
    );
    seen
}

/// Drain the tail of the current flush batch: keep receiving until the
/// channel has been quiet for a few debounce windows. Bounded by
/// consecutive silence, not by wall clock from the start, so it always
/// terminates promptly once the pipeline settles.
async fn drain_batch_tail(rx: &mut mpsc::Receiver<Change>) -> Vec<Change> {
    let mut tail = Vec::new();
    while let Ok(Some(change)) = tokio::time::timeout(DEBOUNCE * 4, rx.recv()).await {
        tail.push(change);
    }
    tail
}

fn assert_none_under(changes: &[Change], forbidden: &Path, context: &str) {
    let offenders: Vec<&Change> = changes
        .iter()
        .filter(|change| change.path.starts_with(forbidden))
        .collect();
    assert!(
        offenders.is_empty(),
        "{context}: nothing under {forbidden:?} may be delivered, got {offenders:#?}",
    );
}

/// Workspace layout shared by the tests: a canonicalized tempdir holding a
/// project (`app/` with a `pages` boot root) plus whatever sibling dirs a
/// test creates next to it. Canonicalized up front because FSEvents
/// reports canonical paths (`/var/…` → `/private/var/…`).
fn workspace() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempdir().expect("tempdir");
    let ws = tmp.path().canonicalize().expect("canonicalize tempdir");
    let project = ws.join("app");
    std::fs::create_dir_all(project.join("pages")).expect("pages boot root");
    (tmp, ws, project)
}

fn start_watcher(project: &Path) -> (Watcher, mpsc::Receiver<Change>) {
    Watcher::start_with_debounce(project, std::iter::once("pages"), DEBOUNCE)
        .expect("watcher start")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn synced_root_delivers_nested_allowed_edit_and_reports_new_root() {
    let (_tmp, ws, project) = workspace();
    let sib = ws.join("sib");
    std::fs::create_dir_all(sib.join("src/deep")).expect("sibling dirs");

    let (mut watcher, mut rx) = start_watcher(&project);
    let newly = watcher.sync_recursive_dir_watches([&sib], SKIPS);
    assert_eq!(newly, vec![sib.clone()], "a fresh root must report as new");

    // The sentinel round trip in the nested dir IS the delivery proof: a
    // deep allowed edit reaches the receiver.
    sentinel_round_trip(&mut rx, &sib.join("src/deep"), "nested-allowed").await;

    watcher.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skip_dir_edits_at_any_depth_are_suppressed() {
    let (_tmp, ws, project) = workspace();
    let sib = ws.join("sib");
    let shallow_skip = sib.join("node_modules");
    let deep_skip = sib.join("a/b/target");
    std::fs::create_dir_all(sib.join("src")).expect("src dir");
    std::fs::create_dir_all(shallow_skip.join("pkg")).expect("node_modules dir");
    std::fs::create_dir_all(deep_skip.join("deep")).expect("deep target dir");

    let (mut watcher, mut rx) = start_watcher(&project);
    watcher.sync_recursive_dir_watches([&sib], SKIPS);

    // Prove the sibling stream is live BEFORE the doubted writes — without
    // this, suppression is indistinguishable from the FSEvents dead window.
    sentinel_round_trip(&mut rx, &sib.join("src"), "live").await;

    // Doubted writes: one directly under a skip dir, one deep below a skip
    // dir that itself sits deep below the root.
    std::fs::write(shallow_skip.join("pkg/index.js"), b"module").expect("write shallow skip");
    std::fs::write(deep_skip.join("deep/out.o"), b"obj").expect("write deep skip");

    let mut seen = sentinel_round_trip(&mut rx, &sib.join("src"), "after-doubt").await;
    seen.extend(drain_batch_tail(&mut rx).await);
    assert_none_under(&seen, &shallow_skip, "skip dir at depth 1");
    assert_none_under(&seen, &deep_skip, "skip dir at depth 3");

    watcher.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skip_dir_created_after_registration_stays_suppressed() {
    let (_tmp, ws, project) = workspace();
    let sib = ws.join("sib");
    std::fs::create_dir_all(sib.join("src")).expect("src dir");

    let (mut watcher, mut rx) = start_watcher(&project);
    watcher.sync_recursive_dir_watches([&sib], SKIPS);
    sentinel_round_trip(&mut rx, &sib.join("src"), "live").await;

    // The skip dir is born AFTER registration: both the directory-create
    // events and the file write beneath it must stay suppressed (matching
    // is by component name on delivery, not by a registration-time walk).
    let late_dist = sib.join("dist");
    std::fs::create_dir_all(late_dist.join("assets")).expect("late dist dir");
    std::fs::write(late_dist.join("assets/bundle.css"), b".x{}").expect("write in late dist");

    let mut seen = sentinel_round_trip(&mut rx, &sib.join("src"), "after-doubt").await;
    seen.extend(drain_batch_tail(&mut rx).await);
    assert_none_under(&seen, &late_dist, "skip dir created after registration");

    watcher.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn near_name_directories_are_not_suppressed() {
    let (_tmp, ws, project) = workspace();
    let sib = ws.join("sib");
    std::fs::create_dir_all(sib.join("distress")).expect("distress dir");
    std::fs::create_dir_all(sib.join("node_module")).expect("node_module dir");

    let (mut watcher, mut rx) = start_watcher(&project);
    watcher.sync_recursive_dir_watches([&sib], SKIPS);

    // Component matching is exact: `distress` (superstring) and
    // `node_module` (substring) must both deliver. The round trips write
    // INTO those dirs, so arrival is the proof.
    sentinel_round_trip(&mut rx, &sib.join("distress"), "distress").await;
    sentinel_round_trip(&mut rx, &sib.join("node_module"), "node-module").await;

    watcher.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dependency_parent_root_upgrades_to_recursive() {
    let (_tmp, ws, project) = workspace();
    let sib_lib = ws.join("sib/lib");
    std::fs::create_dir_all(sib_lib.join("deep")).expect("sibling lib dirs");
    let dependency = sib_lib.join("helper.ts");
    std::fs::write(&dependency, "export const marker = 'one';\n").expect("seed dep");

    let (mut watcher, mut rx) = start_watcher(&project);

    // Baseline: the #1678 file-parent, NON-recursive registration.
    let dep_parents = watcher.watch_additional_files([&dependency]);
    assert_eq!(dep_parents, vec![sib_lib.clone()]);
    sentinel_round_trip(&mut rx, &sib_lib, "baseline-live").await;

    // Grandchildren are NOT delivered by the non-recursive parent watch.
    std::fs::write(sib_lib.join("deep/inner.ts"), b"deep").expect("write deep pre-upgrade");
    let mut seen = sentinel_round_trip(&mut rx, &sib_lib, "pre-upgrade").await;
    seen.extend(drain_batch_tail(&mut rx).await);
    assert_none_under(&seen, &sib_lib.join("deep"), "non-recursive baseline");

    // Upgrade: the same dir as a recursive root. It must NOT be deduped
    // away by the existing non-recursive registration, and must report.
    let newly = watcher.sync_recursive_dir_watches([&sib_lib], SKIPS);
    assert_eq!(
        newly,
        vec![sib_lib.clone()],
        "a previously non-recursive dir must upgrade and report as new"
    );

    // A deep sentinel arriving proves the upgrade delivered recursion
    // (handshake retries absorb the rebuilt stream's startup dead window).
    sentinel_round_trip(&mut rx, &sib_lib.join("deep"), "post-upgrade").await;

    watcher.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overlapping_and_canonical_alias_roots_register_once() {
    let tmp = tempdir().expect("tempdir");
    // Deliberately NOT canonicalized: on macOS `/var/…` vs `/private/var/…`
    // gives a real canonical alias pair for free. (On hosts where the
    // tempdir is already canonical the spellings coincide and this test
    // still passes — the alias arm is then covered by the macOS runs.)
    let ws_raw = tmp.path().to_path_buf();
    let ws = tmp.path().canonicalize().expect("canonicalize tempdir");
    let project = ws.join("app");
    std::fs::create_dir_all(project.join("pages")).expect("pages boot root");
    let sib_raw = ws_raw.join("sib");
    let sib = ws.join("sib");
    std::fs::create_dir_all(sib.join("nested/src")).expect("sibling dirs");
    std::fs::create_dir_all(sib.join("src")).expect("sibling src");
    std::fs::create_dir_all(sib.join("node_modules")).expect("sibling node_modules");

    let (mut watcher, mut rx) = start_watcher(&project);

    // Three spellings of overlapping coverage → exactly ONE registration.
    let newly = watcher.sync_recursive_dir_watches([&sib, &sib_raw, &sib.join("nested")], SKIPS);
    assert_eq!(
        newly,
        vec![sib.clone()],
        "alias + nested roots must collapse to one registration"
    );

    // Idempotent: same desired set again registers nothing.
    let again = watcher.sync_recursive_dir_watches([&sib, &sib_raw, &sib.join("nested")], SKIPS);
    assert!(
        again.is_empty(),
        "repeat sync must be a no-op, got {again:?}"
    );

    // Re-spelled desired set (raw alias only) keeps the registration.
    let respelled = watcher.sync_recursive_dir_watches([&sib_raw], SKIPS);
    assert!(
        respelled.is_empty(),
        "an alias spelling of a kept root must not re-register, got {respelled:?}"
    );

    // Delivery survives the re-spelled sync, and the skip filter still
    // matches the canonical event paths.
    sentinel_round_trip(&mut rx, &sib.join("nested/src"), "live").await;
    std::fs::write(sib.join("node_modules/pkg.js"), b"m").expect("write skip doubt");
    let mut seen = sentinel_round_trip(&mut rx, &sib.join("src"), "after-doubt").await;
    seen.extend(drain_batch_tail(&mut rx).await);
    assert_none_under(
        &seen,
        &sib.join("node_modules"),
        "skip under alias-kept root",
    );

    watcher.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconciled_away_root_stops_delivering_and_only_new_roots_report() {
    let (_tmp, ws, project) = workspace();
    let alpha = ws.join("alpha");
    let beta = ws.join("beta");
    std::fs::create_dir_all(alpha.join("src")).expect("alpha dirs");
    std::fs::create_dir_all(beta.join("src")).expect("beta dirs");

    let (mut watcher, mut rx) = start_watcher(&project);

    // Growth reporting: the kept root never re-reports.
    let first = watcher.sync_recursive_dir_watches([&beta], SKIPS);
    assert_eq!(first, vec![beta.clone()]);
    let second = watcher.sync_recursive_dir_watches([&alpha, &beta], SKIPS);
    assert_eq!(
        second,
        vec![alpha.clone()],
        "only the genuinely new root reports on a grown set"
    );

    // Prove BOTH streams live, then settle so no straggler marker from the
    // alpha round trip can pollute the absence collection below.
    sentinel_round_trip(&mut rx, &alpha.join("src"), "alpha-live").await;
    sentinel_round_trip(&mut rx, &beta.join("src"), "beta-live").await;
    drain_batch_tail(&mut rx).await;

    // Replace semantics: alpha falls out of the desired set.
    let third = watcher.sync_recursive_dir_watches([&beta], SKIPS);
    assert!(
        third.is_empty(),
        "shrinking must report nothing, got {third:?}"
    );

    // The doubted write lands under the reconciled-away root; the sentinel
    // rides the still-live beta stream.
    std::fs::write(alpha.join("src/gone.ts"), b"dead root").expect("write under retired root");
    let mut seen = sentinel_round_trip(&mut rx, &beta.join("src"), "after-retire").await;
    seen.extend(drain_batch_tail(&mut rx).await);
    assert_none_under(&seen, &alpha, "reconciled-away root");

    watcher.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boot_covered_root_registers_nothing_and_boot_delivery_is_unfiltered() {
    let (_tmp, _ws, project) = workspace();
    let pages_sub = project.join("pages/sub");
    // A dir literally named like a skip entry, inside the boot root.
    std::fs::create_dir_all(pages_sub.join("dist")).expect("pages sub dirs");

    let (mut watcher, mut rx) = start_watcher(&project);

    // Covered by the boot recursive root: nothing to register, nothing to
    // report — and crucially, nothing to filter.
    let newly = watcher.sync_recursive_dir_watches([&pages_sub], SKIPS);
    assert!(
        newly.is_empty(),
        "a boot-covered root must not register, got {newly:?}"
    );

    sentinel_round_trip(&mut rx, &project.join("pages"), "boot-live").await;

    // The superset invariant, observed: boot-root traffic is NEVER
    // narrowed, even under a `dist` component the skip list names. The
    // round trip writes INTO that dir, so arrival is the proof.
    sentinel_round_trip(&mut rx, &pages_sub.join("dist"), "boot-dist-delivers").await;

    watcher.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retired_root_that_is_a_dependency_parent_keeps_direct_child_delivery() {
    let (_tmp, ws, project) = workspace();
    let sib_lib = ws.join("sib/lib");
    std::fs::create_dir_all(sib_lib.join("deep")).expect("sibling lib dirs");
    let dependency = sib_lib.join("helper.ts");
    std::fs::write(&dependency, "export const marker = 'one';\n").expect("seed dep");

    let (mut watcher, mut rx) = start_watcher(&project);
    assert_eq!(
        watcher.watch_additional_files([&dependency]),
        vec![sib_lib.clone()]
    );

    // Upgrade to recursive, prove deep delivery, then settle so straggler
    // deep markers cannot pollute the post-retire absence collection.
    assert_eq!(
        watcher.sync_recursive_dir_watches([&sib_lib], SKIPS),
        vec![sib_lib.clone()]
    );
    sentinel_round_trip(&mut rx, &sib_lib.join("deep"), "recursive-live").await;
    drain_batch_tail(&mut rx).await;

    // Retire the root. Because the same dir doubles as a #1678 dependency
    // parent, retirement must DOWNGRADE (not unwatch): direct children keep
    // delivering, recursion stops.
    let retired = watcher.sync_recursive_dir_watches(std::iter::empty::<&Path>(), SKIPS);
    assert!(
        retired.is_empty(),
        "retiring reports nothing, got {retired:?}"
    );

    // Direct-child delivery survives (the dependency consumer's coverage).
    sentinel_round_trip(&mut rx, &sib_lib, "post-retire-live").await;

    // Recursive delivery is gone.
    std::fs::write(sib_lib.join("deep/gone.ts"), b"deep").expect("write deep post-retire");
    let mut seen = sentinel_round_trip(&mut rx, &sib_lib, "after-doubt").await;
    seen.extend(drain_batch_tail(&mut rx).await);
    assert_none_under(&seen, &sib_lib.join("deep"), "retired recursion");

    watcher.shutdown().await;
}
