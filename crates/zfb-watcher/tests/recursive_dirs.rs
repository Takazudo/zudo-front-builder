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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shrinking_to_a_nested_root_keeps_nested_coverage_and_drops_the_rest() {
    // Overlap transition /sib → /sib/sub: the outer root's unwatch must not
    // strip the surviving nested root's coverage (on inotify an unwatch
    // removes every watch entry underneath it — the retire path re-registers
    // surviving roots nested under a retired one).
    let (_tmp, ws, project) = workspace();
    let sib = ws.join("sib");
    let sub = sib.join("sub");
    std::fs::create_dir_all(sub.join("deep")).expect("nested dirs");
    std::fs::create_dir_all(sib.join("other")).expect("outer-only dir");

    let (mut watcher, mut rx) = start_watcher(&project);
    assert_eq!(
        watcher.sync_recursive_dir_watches([&sib], SKIPS),
        vec![sib.clone()]
    );
    sentinel_round_trip(&mut rx, &sub.join("deep"), "outer-live").await;
    drain_batch_tail(&mut rx).await;

    // Shrink: only the nested root stays desired. It was covered by the
    // outer root before, so it registers (and reports) now.
    assert_eq!(
        watcher.sync_recursive_dir_watches([&sub], SKIPS),
        vec![sub.clone()]
    );

    // Nested coverage survives the outer root's retirement.
    sentinel_round_trip(&mut rx, &sub.join("deep"), "nested-live").await;

    // The outer-only subtree stops delivering.
    std::fs::write(sib.join("other/gone.ts"), b"outer only").expect("write outer-only");
    let mut seen = sentinel_round_trip(&mut rx, &sub.join("deep"), "after-doubt").await;
    seen.extend(drain_batch_tail(&mut rx).await);
    assert_none_under(&seen, &sib.join("other"), "retired outer subtree");

    watcher.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn growing_to_an_outer_root_transfers_nested_coverage_without_unwatch() {
    // Overlap transition /sib/sub → /sib: the nested root's registration is
    // covered by the new outer root, so retirement is a pure coverage
    // transfer (no unwatch — on inotify it would strip shared descriptors).
    let (_tmp, ws, project) = workspace();
    let sib = ws.join("sib");
    let sub = sib.join("sub");
    std::fs::create_dir_all(sub.join("deep")).expect("nested dirs");
    std::fs::create_dir_all(sib.join("other")).expect("outer-only dir");

    let (mut watcher, mut rx) = start_watcher(&project);
    assert_eq!(
        watcher.sync_recursive_dir_watches([&sub], SKIPS),
        vec![sub.clone()]
    );
    sentinel_round_trip(&mut rx, &sub.join("deep"), "nested-live").await;

    assert_eq!(
        watcher.sync_recursive_dir_watches([&sib], SKIPS),
        vec![sib.clone()]
    );

    // Both the previously-nested subtree and the newly-covered outer
    // subtree deliver after the transition.
    sentinel_round_trip(&mut rx, &sub.join("deep"), "still-live").await;
    sentinel_round_trip(&mut rx, &sib.join("other"), "outer-live").await;

    watcher.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retiring_outer_root_restores_nested_dependency_parent_watch() {
    // A #1678 dependency parent NESTED under a retired outer root: on
    // inotify the outer unwatch removes the dependency dir's entry too, so
    // the retire path must re-register it (collateral repair).
    let (_tmp, ws, project) = workspace();
    let sib = ws.join("sib");
    let sib_lib = sib.join("lib");
    std::fs::create_dir_all(sib_lib.join("deep")).expect("sibling lib dirs");
    let dependency = sib_lib.join("helper.ts");
    std::fs::write(&dependency, "export const marker = 'one';\n").expect("seed dep");

    let (mut watcher, mut rx) = start_watcher(&project);
    assert_eq!(
        watcher.watch_additional_files([&dependency]),
        vec![sib_lib.clone()]
    );
    assert_eq!(
        watcher.sync_recursive_dir_watches([&sib], SKIPS),
        vec![sib.clone()]
    );
    sentinel_round_trip(&mut rx, &sib_lib.join("deep"), "recursive-live").await;
    drain_batch_tail(&mut rx).await;

    let retired = watcher.sync_recursive_dir_watches(std::iter::empty::<&Path>(), SKIPS);
    assert!(
        retired.is_empty(),
        "retiring reports nothing, got {retired:?}"
    );

    // The dependency parent's direct-children delivery survives the outer
    // root's retirement.
    sentinel_round_trip(&mut rx, &sib_lib, "dep-live").await;

    // Recursion below the dependency parent (and the rest of the retired
    // subtree) is gone.
    std::fs::write(sib_lib.join("deep/gone.ts"), b"deep").expect("write deep post-retire");
    std::fs::write(sib.join("elsewhere.ts"), b"outer").expect("write outer post-retire");
    let mut seen = sentinel_round_trip(&mut rx, &sib_lib, "after-doubt").await;
    seen.extend(drain_batch_tail(&mut rx).await);
    assert_none_under(
        &seen,
        &sib_lib.join("deep"),
        "retired recursion below dep parent",
    );
    assert!(
        !seen.iter().any(|c| c.path == sib.join("elsewhere.ts")),
        "retired outer root must stop delivering, got {seen:#?}",
    );

    watcher.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dependency_registration_after_sync_does_not_downgrade_recursion() {
    // Reverse order of the upgrade test: the root is synced FIRST, then a
    // dependency file directly inside it is registered. notify backends
    // replace a re-watched path's mode, so without the guarded watcher the
    // NonRecursive dependency-parent registration would strip deep delivery
    // from the active synced root.
    let (_tmp, ws, project) = workspace();
    let sib_lib = ws.join("sib/lib");
    std::fs::create_dir_all(sib_lib.join("deep")).expect("sibling lib dirs");
    let dependency = sib_lib.join("helper.ts");
    std::fs::write(&dependency, "export const marker = 'one';\n").expect("seed dep");

    let (mut watcher, mut rx) = start_watcher(&project);
    assert_eq!(
        watcher.sync_recursive_dir_watches([&sib_lib], SKIPS),
        vec![sib_lib.clone()]
    );
    sentinel_round_trip(&mut rx, &sib_lib.join("deep"), "recursive-live").await;

    // The dependency registration must not disturb the recursive watch.
    watcher.watch_additional_files([&dependency]);
    sentinel_round_trip(&mut rx, &sib_lib.join("deep"), "still-recursive").await;
    drain_batch_tail(&mut rx).await;

    // And the recorded dependency ownership still drives the retire path:
    // direct children keep delivering, recursion stops.
    let retired = watcher.sync_recursive_dir_watches(std::iter::empty::<&Path>(), SKIPS);
    assert!(
        retired.is_empty(),
        "retiring reports nothing, got {retired:?}"
    );
    sentinel_round_trip(&mut rx, &sib_lib, "dep-live").await;
    std::fs::write(sib_lib.join("deep/gone.ts"), b"deep").expect("write deep post-retire");
    let mut seen = sentinel_round_trip(&mut rx, &sib_lib, "after-doubt").await;
    seen.extend(drain_batch_tail(&mut rx).await);
    assert_none_under(&seen, &sib_lib.join("deep"), "retired recursion");

    watcher.shutdown().await;
}

/// Like [`sentinel_round_trip`], but matches the sentinel by FILE NAME
/// instead of directory prefix. Needed by the symlink-alias repair tests
/// below: delivery SPELLING for a symlink-registered watch differs by
/// backend — notify's inotify backend joins the as-given watch spelling,
/// while its FSEvents backend delivers canonicalized paths — so pinning
/// either spelling here would deadlock the round trip on the other
/// platform. `sentinel_write_dir` may be any spelling of the watched dir
/// (same inode either way).
async fn sentinel_round_trip_any_spelling(
    rx: &mut mpsc::Receiver<Change>,
    sentinel_write_dir: &Path,
    label: &str,
) -> Vec<Change> {
    let mut seen = Vec::new();
    let marker = format!("sentinel-{label}-");
    let res = watcher_live_handshake(
        HandshakeOpts::new(ROUND_TRIP_DEADLINE),
        |idx| {
            std::fs::write(
                sentinel_write_dir.join(format!("sentinel-{label}-{idx}.txt")),
                b"sentinel",
            )
            .expect("write sentinel file");
        },
        || loop {
            match rx.try_recv() {
                Ok(change) => {
                    let hit = change
                        .path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(&marker));
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
        "sentinel written into {sentinel_write_dir:?} never delivered ({label}); \
         collected so far: {seen:#?}",
    );
    seen
}

/// Issue #1843 site (c): a dependency parent registered through a
/// symlink-aliased spelling is re-registered at most ONCE — under its
/// registration spelling — when a covering synced root retires.
///
/// What each platform can observe (probed against notify 8.2.0):
///
/// - The `watch()` CALL LOG is the cross-platform pin: the pre-fix
///   per-alias repair issued one call per spelling of the same directory.
/// - inotify additionally FLIPS the delivered-path spelling: both
///   spellings resolve to one inode and share a watch descriptor, and the
///   last `watch()` wins the backend's wd→path map. The spelling
///   assertions below pin that half, gated to Linux — on FSEvents
///   delivery is always canonical-spelled and per-alias duplicate
///   registrations coalesce invisibly (the backend keys `recursive_info`
///   by canonicalized path and delivers each raw event at most once), so
///   no delivery-level assertion can catch the duplicate there.
///
/// Red-first (recorded 2026-07-22, macOS/FSEvents): with the repair loops
/// temporarily reverted to iterate the alias sets — swap the dependency
/// loop back to the `watched_dependency_dirs` snapshot and the boot loop
/// back to `watched_recursive_roots`, the pre-#1843 shape (see this
/// commit's diff of `sync_recursive_dir_watches`) — this test failed with:
/// `dependency parent must be re-watched exactly once, got [(".../aa/lib",
/// false), (".../zz/lib", false)]`.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retired_root_repair_rewatches_symlink_aliased_dependency_parent_once() {
    let (_tmp, ws, project) = workspace();
    let zz = ws.join("zz");
    std::fs::create_dir_all(zz.join("lib")).expect("physical dirs");
    let aa = ws.join("aa");
    std::os::unix::fs::symlink(&zz, &aa).expect("symlink alias");
    // Claimed through the symlink spelling: the alias set has two members
    // (`aa/lib` as-given + `zz/lib` canonical) while exactly one
    // `notify.watch()` was issued.
    let dep_parent = aa.join("lib");
    let dependency = dep_parent.join("helper.ts");
    std::fs::write(&dependency, "export const marker = 'one';\n").expect("seed dep");

    let (mut watcher, mut rx) = start_watcher(&project);
    assert_eq!(
        watcher.watch_additional_files([&dependency]),
        vec![dep_parent.clone()]
    );
    assert_eq!(
        watcher.sync_recursive_dir_watches([&aa], SKIPS),
        vec![aa.clone()]
    );
    sentinel_round_trip_any_spelling(&mut rx, &zz.join("lib"), "synced-live").await;
    drain_batch_tail(&mut rx).await;

    // Retire the covering root with the call log armed: the collateral
    // repair scan re-registers the nested dependency parent.
    watcher.enable_watch_call_log();
    assert!(watcher
        .sync_recursive_dir_watches(std::iter::empty::<&Path>(), SKIPS)
        .is_empty());
    let repair_calls = watcher.take_watch_call_log();
    let dep_rewatches: Vec<_> = repair_calls
        .iter()
        .filter(|(path, _)| path.canonicalize().ok() == Some(zz.join("lib")))
        .collect();
    assert_eq!(
        dep_rewatches.len(),
        1,
        "dependency parent must be re-watched exactly once, got {repair_calls:#?}",
    );
    assert_eq!(
        dep_rewatches[0],
        &(dep_parent.clone(), false),
        "the re-watch must reuse the registration spelling, non-recursively",
    );

    // Delivery window: doubted write → sentinel round trip → batch tail.
    std::fs::write(zz.join("lib/edited.ts"), b"post-retire edit").expect("write post-retire");
    let mut seen = sentinel_round_trip_any_spelling(&mut rx, &zz.join("lib"), "after-retire").await;
    seen.extend(drain_batch_tail(&mut rx).await);

    // The edit is delivered on the repaired dependency-parent watch.
    assert!(
        seen.iter()
            .any(|c| c.path.file_name().is_some_and(|n| n == "edited.ts")),
        "post-retire edit must be delivered, got {seen:#?}",
    );
    // inotify only (see the header comment): delivery keeps the
    // registration (project) spelling — the pre-fix per-alias repair
    // flipped the shared watch descriptor's spelling to the physical
    // alias. On FSEvents delivery is canonical-spelled even when correct,
    // so these would be false failures there.
    #[cfg(target_os = "linux")]
    {
        assert_none_under(&seen, &zz, "symlink-aliased dependency parent repair");
        assert!(
            seen.iter().any(|c| c.path == dep_parent.join("edited.ts")),
            "the edit must be delivered under the registration spelling, got {seen:#?}",
        );
    }

    watcher.shutdown().await;
}

/// Issue #1843 site (b): a BOOT root nested under a retired synced root is
/// re-registered at most ONCE — under its boot registration spelling — by
/// the collateral repair scan (which previously had no coverage at all).
/// Platform observability and the Linux gating rationale are identical to
/// [`retired_root_repair_rewatches_symlink_aliased_dependency_parent_once`].
///
/// Red-first (recorded 2026-07-22, macOS/FSEvents): with the repair loops
/// temporarily reverted to the pre-#1843 per-alias shape (same revert
/// procedure as the dependency-parent test above), this test failed with:
/// `boot root must be re-watched exactly once, got [(".../aa/pages",
/// true), (".../zz/pages", true)]`.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retired_root_repair_rewatches_symlink_aliased_boot_root_once() {
    let (_tmp, ws, project) = workspace();
    let zz = ws.join("zz");
    std::fs::create_dir_all(zz.join("pages")).expect("physical dirs");
    let aa = ws.join("aa");
    std::os::unix::fs::symlink(&zz, &aa).expect("symlink alias");
    // Boot-registered through the symlink spelling via the extras path.
    let boot_extra = aa.join("pages");

    let (mut watcher, mut rx) = Watcher::start_with_extras(
        &project,
        std::iter::once("pages"),
        [boot_extra.clone()],
        DEBOUNCE,
    )
    .expect("watcher start");

    assert_eq!(
        watcher.sync_recursive_dir_watches([&aa], SKIPS),
        vec![aa.clone()]
    );
    sentinel_round_trip_any_spelling(&mut rx, &zz.join("pages"), "synced-live").await;
    drain_batch_tail(&mut rx).await;

    watcher.enable_watch_call_log();
    assert!(watcher
        .sync_recursive_dir_watches(std::iter::empty::<&Path>(), SKIPS)
        .is_empty());
    let repair_calls = watcher.take_watch_call_log();
    let boot_rewatches: Vec<_> = repair_calls
        .iter()
        .filter(|(path, _)| path.canonicalize().ok() == Some(zz.join("pages")))
        .collect();
    assert_eq!(
        boot_rewatches.len(),
        1,
        "boot root must be re-watched exactly once, got {repair_calls:#?}",
    );
    assert_eq!(
        boot_rewatches[0],
        &(boot_extra.clone(), true),
        "the re-watch must reuse the boot registration spelling, recursively",
    );

    // Delivery window: boot coverage must survive the retirement.
    std::fs::write(zz.join("pages/edited.md"), b"post-retire edit").expect("write post-retire");
    let mut seen =
        sentinel_round_trip_any_spelling(&mut rx, &zz.join("pages"), "after-retire").await;
    seen.extend(drain_batch_tail(&mut rx).await);
    assert!(
        seen.iter()
            .any(|c| c.path.file_name().is_some_and(|n| n == "edited.md")),
        "post-retire boot-root edit must be delivered, got {seen:#?}",
    );
    #[cfg(target_os = "linux")]
    {
        assert_none_under(&seen, &zz, "symlink-aliased boot root repair");
        assert!(
            seen.iter().any(|c| c.path == boot_extra.join("edited.md")),
            "the edit must be delivered under the boot registration spelling, got {seen:#?}",
        );
    }

    watcher.shutdown().await;
}

/// Issue #1843, codex-review finding: when ONE physical boot directory was
/// boot-registered under SEVERAL spellings, the repair's single re-watch
/// must replay the LAST-registered spelling — on inotify the most recent
/// registration controls the shared watch descriptor's delivered-path
/// spelling, so re-watching an earlier (or lexicographically first)
/// spelling would silently flip delivery after a covering root retires.
/// The extras below register `aa/pages` THEN `zz/pages`: the last
/// registration (`zz/pages`) must win, while `aa/pages` sorts first both
/// lexicographically and in registration order — discriminating against
/// map-ordered and first-registered-wins implementations alike.
///
/// Red-first (recorded 2026-07-22, macOS/FSEvents): with the repair's
/// boot loop iterating in forward registration order (drop the `.rev()` in
/// `sync_recursive_dir_watches`), this test failed with `the re-watch must
/// replay the LAST-registered boot spelling, got ("…/aa/pages", true)`.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_spelling_boot_roots_repair_to_the_last_registered_spelling() {
    let (_tmp, ws, project) = workspace();
    let zz = ws.join("zz");
    std::fs::create_dir_all(zz.join("pages")).expect("physical dirs");
    let aa = ws.join("aa");
    std::os::unix::fs::symlink(&zz, &aa).expect("symlink alias");

    let (mut watcher, mut rx) = Watcher::start_with_extras(
        &project,
        std::iter::once("pages"),
        [aa.join("pages"), zz.join("pages")],
        DEBOUNCE,
    )
    .expect("watcher start");

    assert_eq!(
        watcher.sync_recursive_dir_watches([&aa], SKIPS),
        vec![aa.clone()]
    );
    sentinel_round_trip_any_spelling(&mut rx, &zz.join("pages"), "synced-live").await;
    drain_batch_tail(&mut rx).await;

    watcher.enable_watch_call_log();
    assert!(watcher
        .sync_recursive_dir_watches(std::iter::empty::<&Path>(), SKIPS)
        .is_empty());
    let repair_calls = watcher.take_watch_call_log();
    let boot_rewatches: Vec<_> = repair_calls
        .iter()
        .filter(|(path, _)| path.canonicalize().ok() == Some(zz.join("pages")))
        .collect();
    assert_eq!(
        boot_rewatches.len(),
        1,
        "the duplicate-spelling boot dir must be re-watched exactly once, got {repair_calls:#?}",
    );
    assert_eq!(
        boot_rewatches[0],
        &(zz.join("pages"), true),
        "the re-watch must replay the LAST-registered boot spelling",
    );

    // Boot coverage stays live after the retirement.
    sentinel_round_trip_any_spelling(&mut rx, &zz.join("pages"), "after-retire").await;

    watcher.shutdown().await;
}

/// Alias-boundary pin for the second issue #1799 fix (previously unpinned;
/// issue #1843 verification note): a dependency dir that IS the retired
/// root under a DIFFERENT spelling must not resurrect the just-retired
/// root from the dependency repair loop. Its dependency coverage is
/// restored by the downgrade branch alone — exactly one non-recursive
/// `watch()` reaches the retired directory's identity — so direct
/// children keep delivering while recursion stays gone.
///
/// Red-first (recorded 2026-07-22, macOS/FSEvents): with the dependency
/// repair loop's exclusion reverted to LITERAL-SPELLING comparison against
/// the retired root only (the pre-#1799-review `dep != root` shape), the
/// call-log assertion failed with `only the downgrade restore may
/// re-watch, got [(".../aa/lib", false), (".../zz/lib", false)]` — the
/// physical spelling resurrected the root beside the restore.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dependency_dir_spelled_as_retired_root_alias_does_not_resurrect_it() {
    let (_tmp, ws, project) = workspace();
    let zz = ws.join("zz");
    std::fs::create_dir_all(zz.join("lib/deep")).expect("physical dirs");
    let aa = ws.join("aa");
    std::os::unix::fs::symlink(&zz, &aa).expect("symlink alias");
    // The dependency is claimed under the PHYSICAL spelling...
    let dependency = zz.join("lib/helper.ts");
    std::fs::write(&dependency, "export const marker = 'one';\n").expect("seed dep");

    let (mut watcher, mut rx) = start_watcher(&project);
    assert_eq!(
        watcher.watch_additional_files([&dependency]),
        vec![zz.join("lib")]
    );
    // ...while the synced root covers the SAME dir through the symlink
    // spelling.
    assert_eq!(
        watcher.sync_recursive_dir_watches([aa.join("lib")], SKIPS),
        vec![aa.join("lib")]
    );
    sentinel_round_trip_any_spelling(&mut rx, &zz.join("lib/deep"), "synced-live").await;
    drain_batch_tail(&mut rx).await;

    watcher.enable_watch_call_log();
    assert!(watcher
        .sync_recursive_dir_watches(std::iter::empty::<&Path>(), SKIPS)
        .is_empty());
    let repair_calls = watcher.take_watch_call_log();
    assert_eq!(
        repair_calls.len(),
        1,
        "only the downgrade restore may re-watch, got {repair_calls:#?}",
    );
    assert!(
        !repair_calls[0].1,
        "the restore must be non-recursive, got {repair_calls:#?}",
    );

    // Direct children keep delivering (the dependency consumer's coverage)...
    sentinel_round_trip_any_spelling(&mut rx, &zz.join("lib"), "post-retire-live").await;
    // ...and recursion is gone.
    std::fs::write(zz.join("lib/deep/gone.ts"), b"deep").expect("write deep post-retire");
    let mut seen = sentinel_round_trip_any_spelling(&mut rx, &zz.join("lib"), "after-doubt").await;
    seen.extend(drain_batch_tail(&mut rx).await);
    assert!(
        !seen
            .iter()
            .any(|c| c.path.file_name().is_some_and(|n| n == "gone.ts")),
        "retired recursion must not deliver, got {seen:#?}",
    );

    watcher.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn symlinked_descendant_and_physical_ancestor_collapse_to_one() {
    // Order-independence of the overlap reduction: a symlink-spelled
    // descendant can sort BEFORE its physical ancestor by raw path, so the
    // reduction ranks by canonical path instead. `alink/sub` canonicalizes
    // under `zreal`, which must win as the single outermost registration.
    let (_tmp, ws, project) = workspace();
    let zreal = ws.join("zreal");
    std::fs::create_dir_all(zreal.join("sub/deep")).expect("physical dirs");
    let alink = ws.join("alink");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&zreal, &alink).expect("symlink alias");
    #[cfg(not(unix))]
    {
        // No symlink primitive to exercise on this platform; the canonical
        // ranking still holds trivially.
        std::fs::create_dir_all(&alink).expect("plain dir");
    }
    let linked_sub = alink.join("sub");

    let (mut watcher, mut rx) = start_watcher(&project);
    let newly = watcher.sync_recursive_dir_watches([linked_sub.clone(), zreal.clone()], SKIPS);
    assert_eq!(
        newly,
        vec![zreal.clone()],
        "the physical ancestor must be the single registration"
    );
    let again = watcher.sync_recursive_dir_watches([linked_sub, zreal.clone()], SKIPS);
    assert!(
        again.is_empty(),
        "repeat sync must be a no-op, got {again:?}"
    );

    sentinel_round_trip(&mut rx, &zreal.join("sub/deep"), "live").await;

    watcher.shutdown().await;
}
