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
    let (_watcher, mut rx) = Watcher::start(root.path(), ["content", "data"])
        .expect("start watcher");

    // Give notify a beat to register its OS-level watch before we start
    // poking the filesystem. Otherwise on slower CI machines the very
    // first event can race the kernel-side registration.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let target = content_dir.join("hello.md");
    fs::write(&target, b"# hi\n").expect("write file");

    // 1s is generous: 50ms debounce + worst-case ~25ms tick lag + plenty
    // of slack for noisy CI.
    let change = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("change arrived in time")
        .expect("channel still open");

    // The reported path should be exactly the file we wrote (notify
    // reports absolute paths since we watched an absolute path).
    assert_eq!(change.path, target, "unexpected change path");

    // Kind should be Created or Modified — either is acceptable; some
    // platforms collapse the create+write into a single Modify.
    assert!(
        matches!(change.kind, ChangeKind::Created | ChangeKind::Modified),
        "unexpected kind: {:?}",
        change.kind
    );
}
