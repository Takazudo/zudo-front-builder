//! End-to-end smoke test for `zfb-watcher`.
//!
//! Spawns a real watcher rooted at a `tempfile::TempDir`, mutates a file
//! under one of the watched relative paths, and asserts that a `Change`
//! event arrives within a generous timeout.

use std::fs;
use std::time::Duration;

use tempfile::tempdir;
use tokio::time::timeout;

use zfb_watcher::{ChangeKind, Watcher};

#[tokio::test]
async fn touching_file_emits_change() {
    let root = tempdir().expect("tempdir");
    let content_dir = root.path().join("content");
    fs::create_dir_all(&content_dir).expect("create content dir");

    // Watch `content/` (existing) and `data/` (deliberately missing — the
    // watcher must not crash on this).
    let (_watcher, mut rx) =
        Watcher::start(root.path(), ["content", "data"]).expect("start watcher");

    // Give notify a beat to register its OS-level watch before we start
    // poking the filesystem. Otherwise on slower CI machines the very
    // first event can race the kernel-side registration.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let target = content_dir.join("hello.md");
    fs::write(&target, b"# hi\n").expect("write file");

    // Canonicalize the target once so we can compare against it below.
    // Canonicalizing upfront also ensures the file exists before we try
    // to call canonicalize on reported paths (macOS FSEvents can report
    // the parent directory path in addition to the file path).
    let target_canon = std::fs::canonicalize(&target).expect("canonicalize target");

    // Drain events until we find one whose canonical path matches the
    // file we wrote. We loop because macOS FSEvents may fire a change for
    // the parent directory (e.g. `content/`) in addition to — or instead
    // of — the file itself, and we don't want to fail on those extra
    // events. Overall deadline is 1s: 50ms debounce + worst-case ~25ms
    // tick lag + plenty of slack for noisy CI.
    let file_change = {
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .expect("timed out waiting for target-file change event");

            let change = timeout(remaining, rx.recv())
                .await
                .expect("change arrived in time")
                .expect("channel still open");

            // Canonicalize both sides so that symlinked temp roots (e.g.
            // macOS /tmp -> /private/tmp) do not cause a spurious mismatch.
            // The `canonicalize` call can fail if the path no longer exists
            // (e.g. a transient directory event); treat that as "not our
            // file" and keep draining.
            if let Ok(canon) = std::fs::canonicalize(&change.path) {
                if canon == target_canon {
                    break change;
                }
            }
        }
    };

    // Kind should be Created or Modified — either is acceptable; some
    // platforms collapse the create+write into a single Modify.
    assert!(
        matches!(file_change.kind, ChangeKind::Created | ChangeKind::Modified),
        "unexpected kind: {:?}",
        file_change.kind
    );
}
