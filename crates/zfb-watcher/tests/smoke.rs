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

    // Prove the watch stream is live before writing the real file rather
    // than guessing a fixed warmup duration. macOS FSEvents can drop a
    // brand-new file's create if it lands in the per-stream startup dead
    // window; a fixed `sleep` races that window under load. Instead, write
    // fresh-named marker files under the watched `content/` dir until one
    // is delivered on the channel (#1338 shared helper). The `hello.md`
    // drain loop below skips the leftover warmup events (they don't match
    // the target path), so they don't perturb the assertion.
    let handshake = zfb_test_utils::watcher_live_handshake(
        zfb_test_utils::HandshakeOpts::new(Duration::from_secs(10)),
        |idx| {
            fs::write(
                content_dir.join(format!("__warmup_{idx}.md")),
                b"# warmup\n",
            )
            .expect("write warmup file");
        },
        || rx.try_recv().is_ok(),
    )
    .await;
    assert!(
        handshake.live,
        "watcher never delivered a warmup event within {:?} ({} markers written) — \
         the notify stream never started delivering events",
        handshake.elapsed, handshake.markers_written,
    );

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

/// Issue #1391 — pin the module-doc'd skip behavior for a missing
/// **in-tree relative** watch root passed to [`Watcher::start`] (the
/// counterpart to `extra_paths::missing_extra_path_is_skipped_without_panic`,
/// which covers the absolute-extra-path variant). Starting the watcher
/// must NOT error or panic when one of the requested relative roots does
/// not exist on disk yet — the dev command layer now surfaces a
/// user-visible warning for this case (`crate::commands::dev`'s
/// `missing_watch_targets`, issue #1391) rather than relying on the
/// tracing-only `warn!` this crate logs internally.
#[tokio::test]
async fn missing_relative_root_is_skipped_without_panic() {
    let root = tempdir().expect("tempdir");

    // "content" exists; "data" deliberately does not.
    fs::create_dir_all(root.path().join("content")).expect("create content dir");
    let missing_root = root.path().join("data");
    assert!(
        !missing_root.exists(),
        "test precondition: data dir is missing"
    );

    let result = Watcher::start(root.path(), ["content", "data"]);

    assert!(
        result.is_ok(),
        "starting with a missing in-tree relative root should not error, got: {:?}",
        result.err()
    );
}
