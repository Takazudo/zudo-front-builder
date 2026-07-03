//! Part B (issue #1348) — real-filesystem `Watcher` behavior, Level 2.
//!
//! These spawn a real `notify` watcher in a tempdir and pin three families of
//! behavior the dev-loop e2e harnesses otherwise diagnose the expensive way:
//!
//!   1. Watch-registration ordering / the startup dead window. macOS FSEvents
//!      (and Linux inotify to a lesser degree) can silently DROP an event that
//!      lands in the per-stream warmup window after a watch is registered. We
//!      do NOT try to fix that here; we pin the POSITIVE contract the codebase
//!      relies on: once the #1338 condition-keyed handshake confirms the stream
//!      is live, subsequent edits are reliably delivered.
//!   2. Canonical-path reporting — symlink and case handling, with
//!      platform-gated assertions (a `/var → /private/var` tempdir symlink and
//!      case-preservation behave differently across backends).
//!   3. Unwatch/drop cleanliness — after shutting down or dropping the watcher,
//!      the change channel CLOSES and no further events arrive. The close is a
//!      POSITIVE signal (`recv()` returns `None`), not a fixed-sleep guess.
//!
//! ## No bare sleeps
//!
//! Every "is the watcher live yet?" wait goes through
//! `zfb_test_utils::watcher_live_handshake` (#1338): it writes fresh-named
//! marker files until the stream actually delivers one, proving liveness rather
//! than guessing a warmup duration. Every "did my edit arrive?" wait is a
//! condition-keyed drain loop wrapped in a failsafe timeout (`wait_for`), and
//! every teardown assertion keys on the channel CLOSE. There is not a single
//! `sleep(Nms)` used as a synchronization primitive in this file.

use std::fs;
use std::path::Path;
use std::time::Duration;

use tempfile::tempdir;
use tokio::sync::mpsc::Receiver;

use zfb_watcher::{Change, ChangeKind, Watcher};

/// Prove the watcher's event stream is live before the real edit under test,
/// using the shared #1338 condition-keyed handshake. Writes fresh-named marker
/// files under `root` until one is delivered on `rx`. Panics with diagnostics
/// if the stream never delivers within the deadline.
async fn handshake_live(root: &Path, rx: &mut Receiver<Change>) {
    let res = zfb_test_utils::watcher_live_handshake(
        zfb_test_utils::HandshakeOpts::new(Duration::from_secs(10)),
        |idx| {
            fs::write(root.join(format!("__wm_{idx}.tmp")), b"warmup\n").expect("write marker");
        },
        || rx.try_recv().is_ok(),
    )
    .await;
    assert!(
        res.live,
        "watcher never delivered a warmup event within {:?} ({} markers written) — \
         the notify stream never started",
        res.elapsed, res.markers_written,
    );
}

/// Drain `rx` until a `Change` whose canonicalized path equals `target`'s
/// canonicalized path arrives, or `deadline` elapses. Condition-keyed: returns
/// the instant the matching event lands. Canonicalizing both sides absorbs the
/// macOS `/var → /private/var` and symlink-root differences so a match means
/// "the event identifies this file", regardless of backend. Returns the
/// matching `Change`, or `None` on timeout.
async fn wait_for(rx: &mut Receiver<Change>, target: &Path, deadline: Duration) -> Option<Change> {
    let want = fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    tokio::time::timeout(deadline, async {
        while let Some(change) = rx.recv().await {
            let got = fs::canonicalize(&change.path).unwrap_or_else(|_| change.path.clone());
            if got == want {
                return Some(change);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

// ---------------------------------------------------------------------------
// 1. Watch-registration ordering / dead window (pinned as the positive contract).
// ---------------------------------------------------------------------------

/// A MODIFY of a file that already existed before the watch started is
/// delivered once the handshake confirms liveness. (The file pre-exists, so
/// this isolates the modify path from the create-keyed dead window.)
#[tokio::test]
async fn modify_of_preexisting_file_after_handshake_is_delivered() {
    let tmp = tempdir().expect("tempdir");
    let root = tmp.path().canonicalize().expect("canonicalize root");
    let target = root.join("edit_me.md");
    fs::write(&target, b"# v1\n").expect("create before watch");

    let (_watcher, mut rx) = Watcher::start(&root, std::iter::once(".")).expect("start");
    handshake_live(&root, &mut rx).await;

    fs::write(&target, b"# v2\n").expect("overwrite");

    let change = wait_for(&mut rx, &target, Duration::from_secs(5)).await;
    let change = change.expect("modify of a pre-existing file must be delivered after handshake");
    assert!(
        matches!(change.kind, ChangeKind::Modified | ChangeKind::Created),
        "overwrite of an existing file should report Modified/Created, got {:?}",
        change.kind,
    );
}

/// A brand-new CREATE after the handshake is delivered. This is the case the
/// startup dead window specifically drops (a create landing in the warmup
/// window); pinning it proves the handshake closes that window in practice.
#[tokio::test]
async fn create_of_new_file_after_handshake_is_delivered() {
    let tmp = tempdir().expect("tempdir");
    let root = tmp.path().canonicalize().expect("canonicalize root");

    let (_watcher, mut rx) = Watcher::start(&root, std::iter::once(".")).expect("start");
    handshake_live(&root, &mut rx).await;

    let target = root.join("brand_new.md");
    fs::write(&target, b"# new\n").expect("create");

    let change = wait_for(&mut rx, &target, Duration::from_secs(5)).await;
    assert!(
        change.is_some(),
        "a fresh create after the handshake must be delivered (dead window pinned closed)",
    );
}

// ---------------------------------------------------------------------------
// 2. Canonical-path reporting — symlink + case handling (platform-gated).
// ---------------------------------------------------------------------------

/// The reported path, canonicalized, identifies the edited file — pinned on
/// every platform (watching an already-canonical root).
#[tokio::test]
async fn reported_path_canonicalizes_to_edited_file() {
    let tmp = tempdir().expect("tempdir");
    let root = tmp.path().canonicalize().expect("canonicalize root");
    let sub = root.join("content");
    fs::create_dir_all(&sub).expect("mkdir content");

    let (_watcher, mut rx) = Watcher::start(&root, std::iter::once(".")).expect("start");
    handshake_live(&root, &mut rx).await;

    let target = sub.join("page.md");
    fs::write(&target, b"# page\n").expect("write");

    let change = wait_for(&mut rx, &target, Duration::from_secs(5))
        .await
        .expect("edit must be delivered");
    let got = fs::canonicalize(&change.path).expect("canonicalize reported path");
    let want = fs::canonicalize(&target).expect("canonicalize target");
    assert_eq!(got, want, "reported path must identify the edited file");
}

/// macOS: `/var/folders/...` tempdirs are symlinks to `/private/var/folders/...`.
/// Watching the NON-canonical `/var` root, FSEvents still reports the canonical
/// `/private` path. Pin that quirk (the smoke test and dev command work around it).
#[cfg(target_os = "macos")]
#[tokio::test]
async fn macos_reports_canonical_private_path_for_symlinked_tempdir_root() {
    let tmp = tempdir().expect("tempdir");
    let raw_root = tmp.path().to_path_buf(); // /var/folders/... (a symlink on macOS)
    let canon_root = raw_root.canonicalize().expect("canonicalize root");
    assert_ne!(
        raw_root, canon_root,
        "precondition: macOS tempdir root is a symlink (/var → /private/var)",
    );

    // Deliberately watch the symlinked (non-canonical) form.
    let (_watcher, mut rx) = Watcher::start(&raw_root, std::iter::once(".")).expect("start");
    handshake_live(&raw_root, &mut rx).await;

    let target = raw_root.join("probe.md");
    fs::write(&target, b"# probe\n").expect("write");

    let change = wait_for(&mut rx, &target, Duration::from_secs(5))
        .await
        .expect("edit must be delivered");
    assert!(
        change.path.starts_with(&canon_root),
        "FSEvents must report the canonical /private path even when watching the \
         symlinked root, got {:?}",
        change.path,
    );
}

/// unix: watch through a symlinked directory root; the reported path,
/// canonicalized, must resolve to the real file. The strong "reports the
/// resolved (non-symlink) path" assertion is macOS-only (FSEvents canonicalizes;
/// inotify may report as-watched), so it is gated inside.
#[cfg(unix)]
#[tokio::test]
async fn symlinked_watch_root_reports_path_identifying_real_file() {
    let tmp = tempdir().expect("tempdir");
    let root = tmp.path().canonicalize().expect("canonicalize root");
    let real = root.join("realdir");
    fs::create_dir_all(&real).expect("mkdir realdir");
    let link = root.join("linkdir");
    std::os::unix::fs::symlink(&real, &link).expect("symlink linkdir → realdir");

    // Watch via the symlink path.
    let (_watcher, mut rx) = Watcher::start(&link, std::iter::once(".")).expect("start");
    handshake_live(&link, &mut rx).await;

    let target = link.join("note.md");
    fs::write(&target, b"# note\n").expect("write via symlink");

    let change = wait_for(&mut rx, &target, Duration::from_secs(5))
        .await
        .expect("edit through a symlinked root must be delivered");

    let got = fs::canonicalize(&change.path).expect("canonicalize reported path");
    let want = fs::canonicalize(real.join("note.md")).expect("canonicalize real target");
    assert_eq!(got, want, "reported path must resolve to the real file");

    #[cfg(target_os = "macos")]
    assert!(
        !change.path.to_string_lossy().contains("linkdir"),
        "macOS FSEvents must report the resolved directory, not the symlink \
         component, got {:?}",
        change.path,
    );
}

/// The reported file name preserves the original case. This is the meaningful
/// case on macOS's case-INSENSITIVE-but-case-PRESERVING default filesystem
/// (the name is never folded to lowercase); it is trivially true on
/// case-sensitive Linux, so the pin holds on both.
#[tokio::test]
async fn reported_path_preserves_filename_case() {
    let tmp = tempdir().expect("tempdir");
    let root = tmp.path().canonicalize().expect("canonicalize root");

    let (_watcher, mut rx) = Watcher::start(&root, std::iter::once(".")).expect("start");
    handshake_live(&root, &mut rx).await;

    let target = root.join("MixedCase.md");
    fs::write(&target, b"# mixed\n").expect("write");

    let change = wait_for(&mut rx, &target, Duration::from_secs(5))
        .await
        .expect("edit must be delivered");
    assert_eq!(
        change.path.file_name().and_then(|n| n.to_str()),
        Some("MixedCase.md"),
        "reported file name must preserve original case, got {:?}",
        change.path,
    );
}

// ---------------------------------------------------------------------------
// 3. Unwatch / drop cleanliness — no events after teardown (positive close signal).
// ---------------------------------------------------------------------------

/// After a graceful `shutdown()`, the change channel closes and an edit made
/// AFTER shutdown is never delivered. The positive signal is the channel close
/// (`recv()` → `None`), not a fixed wait for "nothing to happen".
#[tokio::test]
async fn no_events_delivered_after_shutdown() {
    let tmp = tempdir().expect("tempdir");
    let root = tmp.path().canonicalize().expect("canonicalize root");

    let (watcher, mut rx) = Watcher::start(&root, std::iter::once(".")).expect("start");
    handshake_live(&root, &mut rx).await;

    // Positive baseline: an edit IS delivered while the watcher is live.
    let before = root.join("before.md");
    fs::write(&before, b"# before\n").expect("write before");
    let seen = wait_for(&mut rx, &before, Duration::from_secs(5)).await;
    assert!(
        seen.is_some(),
        "sanity: edits are delivered before shutdown"
    );

    // Graceful shutdown flushes pending, stops the OS watch, and closes the channel.
    watcher.shutdown().await;

    // Edit AFTER shutdown — the OS watch is gone, so this must never surface.
    let after = root.join("after_shutdown.md");
    fs::write(&after, b"# after\n").expect("write after");
    let after_canon = fs::canonicalize(&after).unwrap_or_else(|_| after.clone());

    let mut saw_after = false;
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        // Drain any residual flushed events, then the channel closes (None).
        while let Some(change) = rx.recv().await {
            let got = fs::canonicalize(&change.path).unwrap_or_else(|_| change.path.clone());
            if got == after_canon {
                saw_after = true;
            }
        }
    })
    .await;
    assert!(
        closed.is_ok(),
        "the change channel must close after shutdown (positive close signal)",
    );
    assert!(
        !saw_after,
        "no event may be delivered for an edit made after shutdown",
    );
}

/// Dropping the `Watcher` (the `Drop` impl, distinct from `shutdown()`) tears
/// down the debouncer and closes the channel. `recv()` eventually returns
/// `None`; the failsafe timeout guards against a teardown regression.
#[tokio::test]
async fn dropping_watcher_closes_change_channel() {
    let tmp = tempdir().expect("tempdir");
    let root = tmp.path().canonicalize().expect("canonicalize root");

    let (watcher, mut rx) = Watcher::start(&root, std::iter::once(".")).expect("start");
    handshake_live(&root, &mut rx).await;

    drop(watcher); // exercise the Drop impl's "signal + detach, do not abort" path

    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        while rx.recv().await.is_some() {} // drain residuals until close
    })
    .await;
    assert!(
        closed.is_ok(),
        "Drop must tear down the debouncer and close the change channel",
    );
}
