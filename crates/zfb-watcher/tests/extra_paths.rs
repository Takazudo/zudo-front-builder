//! Integration test for [`zfb_watcher::Watcher::start_with_extras`].
//!
//! Spawns a watcher rooted at one tempdir, registers a second tempdir
//! as an "extra" absolute path, mutates a file inside that second dir,
//! and asserts a debounced rebuild event arrives within a generous
//! timeout. The test deliberately does NOT assert exact event counts —
//! the watcher's debounce window collapses bursts unpredictably across
//! platforms.

use std::fs;
use std::time::Duration;

use tempfile::tempdir;
use tokio::time::timeout;

use zfb_watcher::Watcher;

#[tokio::test]
async fn extra_absolute_path_fires_rebuild_on_write() {
    let project = tempdir().expect("project tempdir");
    let extra = tempdir().expect("extra tempdir");

    // Canonicalise the extra root the same way the dev command would
    // before passing the path to the watcher. On macOS especially,
    // `/var/...` tempdirs canonicalise to `/private/var/...`; without
    // this the path the watcher reports for events would differ from
    // the path the test passed in.
    let extra_canonical = extra.path().canonicalize().expect("canonicalize extra");

    // Use a non-existent in-project relative path so the watcher's
    // in-tree branch is exercised but produces no events of its own.
    let (_watcher, mut rx) = Watcher::start_with_extras(
        project.path(),
        std::iter::empty::<&str>(),
        std::iter::once(extra_canonical.as_path()),
        Duration::from_millis(50),
    )
    .expect("start watcher with extras");

    // Nest the edited file under a sub-directory with a deliberately
    // neutral name (no collision with any in-tree root names such as
    // `pages` / `content` / `styles` / `public` / `components` /
    // `src` / `data` / `layouts` / `lib`). The watcher itself does not
    // care about the path shape, but this keeps the test future-proof
    // against downstream classifier changes (issue #368 fix added a
    // strip-prefix check that would have made an unlucky `tempXXX`
    // dir name interact with this test).
    let notes_dir = extra_canonical.join("notes");
    fs::create_dir_all(&notes_dir).expect("create notes dir");

    // Give notify a beat to register its OS-level watch on the extra
    // path before we mutate it. Otherwise on slower CI machines the
    // first event can race the kernel-side registration.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let target = notes_dir.join("a.md");
    fs::write(&target, b"# extra-path edit\n").expect("write extra-path file");

    // Assertion: a rebuild event fires. We do NOT assert exact kind or
    // count — the watcher's debounce window collapses create+write
    // bursts unpredictably across platforms.
    let change = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("a debounced change arrives within 2s")
        .expect("watcher channel still open");

    // The reported path should live under the canonical extra root.
    assert!(
        change.path.starts_with(&extra_canonical),
        "expected change path {:?} to be under extra root {:?}",
        change.path,
        extra_canonical
    );
}

#[tokio::test]
async fn missing_extra_path_is_skipped_without_panic() {
    let project = tempdir().expect("project tempdir");

    // Construct a path that definitely doesn't exist. The watcher must
    // log a warning and continue — not return Err — so misconfiguration
    // in one project doesn't take the dev server down at boot.
    let bogus = project.path().join("does-not-exist-on-disk-12345");
    assert!(!bogus.exists(), "test precondition: bogus path is missing");

    let result = Watcher::start_with_extras(
        project.path(),
        std::iter::empty::<&str>(),
        std::iter::once(bogus.as_path()),
        Duration::from_millis(50),
    );

    assert!(
        result.is_ok(),
        "starting with a missing extra path should not error, got: {:?}",
        result.err()
    );
}
