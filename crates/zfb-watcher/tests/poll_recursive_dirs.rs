//! Poll-backend-specific retirement semantics for
//! [`zfb_watcher::Watcher::sync_recursive_dir_watches`] (codex review of
//! sub-issue #2173).
//!
//! PollWatcher registrations are independent exact-key scan entries: an
//! ancestor's `unwatch` removes ONLY its own key, never nested
//! registrations the way inotify's recursive unwatch does. So a root whose
//! coverage transfers to a surviving ancestor must be unwatched AT
//! TRANSFER TIME on the poll backend — otherwise its leftover scan entry
//! keeps delivering after the covering ancestor retires, violating the
//! API's replace semantics ("a root dropped by a plan change stops
//! delivering").
//!
//! Deliberately SEPARATE from `recursive_dirs.rs`, which pins the
//! inotify/FSEvents-specific retirement-repair machinery with no polling
//! analogue and stays native-only.

use std::fs;
use std::path::Path;
use std::time::Duration;

use tempfile::tempdir;
use tokio::sync::mpsc::Receiver;

use zfb_watcher::{Change, WatchOptions, Watcher};

mod common;

/// #1338 condition-keyed liveness handshake against the boot root.
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
        "poll watcher never delivered a warmup event within {:?} ({} markers written)",
        res.elapsed, res.markers_written,
    );
}

/// Drain `rx` until a `Change` canonicalizing to `target` arrives or the
/// deadline elapses.
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

/// The full transfer-then-retire lifecycle: child synced → ancestor
/// replaces it (coverage transfer) → everything retired. After the final
/// retirement NOTHING under the old child may deliver. Without the
/// transfer-time unwatch, the child's leftover poll registration keeps
/// scanning and would deliver the final write within ~2 poll intervals —
/// well inside the bounded negative wait below.
#[tokio::test]
async fn transferred_root_stops_delivering_after_covering_ancestor_retires() {
    let tmp = tempdir().expect("tempdir");
    let root = tmp.path().canonicalize().expect("canonicalize root");
    let src = root.join("src");
    fs::create_dir_all(&src).expect("mkdir src");
    let aux = root.join("aux");
    let child = aux.join("child");
    fs::create_dir_all(&child).expect("mkdir aux/child");

    let (mut watcher, mut rx) = Watcher::start_with_options(
        &root,
        std::iter::once("src"),
        std::iter::empty::<&Path>(),
        WatchOptions::default().with_backend(common::poll_backend()),
    )
    .expect("start poll watcher");
    handshake_live(&src, &mut rx).await;

    // 1. Register the child root; a write under it is delivered.
    let newly = watcher
        .sync_recursive_dir_watches(std::iter::once(child.as_path()), std::iter::empty::<&str>());
    assert_eq!(newly, vec![child.clone()], "child is newly watched");

    fs::write(child.join("a.md"), b"# a\n").expect("write a");
    assert!(
        wait_for(&mut rx, &child.join("a.md"), Duration::from_secs(5))
            .await
            .is_some(),
        "a write under the synced child must be delivered",
    );

    // 2. Replace it with the ancestor — the child's coverage transfers.
    // Positive control: writes under the child still deliver, proving the
    // transfer-time unwatch did not open a coverage gap.
    let newly = watcher
        .sync_recursive_dir_watches(std::iter::once(aux.as_path()), std::iter::empty::<&str>());
    assert_eq!(newly, vec![aux.clone()], "ancestor is newly watched");

    fs::write(child.join("b.md"), b"# b\n").expect("write b");
    assert!(
        wait_for(&mut rx, &child.join("b.md"), Duration::from_secs(5))
            .await
            .is_some(),
        "after the transfer the ancestor must still deliver child writes",
    );

    // 3. Retire everything. The ancestor's unwatch removes only its own
    // poll registration key — the child entry must already be gone.
    let newly =
        watcher.sync_recursive_dir_watches(std::iter::empty::<&Path>(), std::iter::empty::<&str>());
    assert!(newly.is_empty(), "retiring registers nothing new");

    let c = child.join("c.md");
    fs::write(&c, b"# c\n").expect("write c");
    let c_canon = fs::canonicalize(&c).unwrap_or_else(|_| c.clone());

    // Bounded negative wait: ~10 poll intervals (the leftover registration
    // this pins against would deliver within ~2 — scan + debounce). Any
    // event canonicalizing to c.md fails; unrelated residuals are drained.
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
            break;
        };
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(change)) => {
                let got = fs::canonicalize(&change.path).unwrap_or_else(|_| change.path.clone());
                assert_ne!(
                    got, c_canon,
                    "a write under a fully-retired subtree must not be delivered \
                     (leftover poll registration after coverage transfer)",
                );
            }
            Ok(None) => panic!("change channel closed unexpectedly"),
            Err(_) => break, // quiet until the deadline — the desired outcome
        }
    }
}
