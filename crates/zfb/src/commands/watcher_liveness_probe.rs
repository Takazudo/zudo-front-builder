//! Watcher-liveness probe lifecycle (issue #1718, epic #1716), extracted
//! from `commands::dev` (issue #1756, sub-task of the tiny-cleanups sweep).
//!
//! `run()` in `commands::dev` spawns [`spawn_watcher_liveness_probe`] once
//! the dev server's listener is bound; everything else in this module is
//! the probe's self-contained lifecycle (scratch-dir resolution,
//! stale-session sweeping, the handshake, and teardown) that lifecycle
//! doesn't need to reach back into `dev.rs` for.

// V8-off (issue #371, sub-task 4.1a): this module's only call site
// (`spawn_watcher_liveness_probe`, from `commands::dev::run`) lives entirely
// behind `#[cfg(feature = "embed_v8")]`, so on the `!feature = "embed_v8"`
// path every item here looks unused — silence the lints in that
// configuration, mirroring `dev.rs`'s own module-level attribute.
#![cfg_attr(not(feature = "embed_v8"), allow(unused_imports, dead_code))]

use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::{Arc, Mutex};

use super::dev::canonicalize_or_lexical;
use crate::output;

/// Decide where the watcher-liveness probe's scratch parent directory
/// should live for this dev session.
///
/// Defaults to a project-local scratch dir alongside this file's other
/// `.zfb-build/` scratch roots (`dev_html_root_for`, `dev_assets_root_for`)
/// — no `extraWatchPaths` config needed. But the probe's own marker writes
/// must never be observable by the REAL orchestrator watcher: an overlap
/// would classify a probe marker as a project change (`PathClass::External`
/// in zfb-build's policy.rs) and trigger a spurious full-rebuild storm on
/// every liveness check. So before settling on the project-local default,
/// this checks it for overlap against every resolved watch root /
/// `extraWatchPaths` entry the caller already computed
/// (`probe_effective_watch_targets` in `run()`); on overlap (e.g. a
/// misconfigured `extraWatchPaths` entry covering the whole project root),
/// it relocates to a dedicated subdirectory of the OS temp dir instead,
/// which the orchestrator's watcher can never reach.
///
/// Returns `Some((parent_dir, relocated))`; `relocated` exists only so tests
/// can assert on the decision — production callers don't need to branch on
/// it. Returns `None` when NEITHER candidate location is provably isolated
/// (see the last check below) — the caller must skip the probe entirely
/// rather than guess.
pub(super) fn resolve_probe_parent_dir(
    project_root: &Path,
    effective_watch_targets: &[PathBuf],
) -> Option<(PathBuf, bool)> {
    // Canonicalise once: a watch target can itself be a symlink whose REAL
    // location is an ancestor of the probe dir even though the raw path
    // strings don't lexically look like it — e.g. a collection or
    // `extraWatchPaths` entry that is a symlink back up to the project
    // root. `zfb_watcher::Watcher` canonicalises every root it registers
    // internally (`watch_aliases`), so comparing only the raw strings here
    // could miss overlap the real watcher actually has. A target that
    // doesn't exist yet fails to canonicalise; fall back to a LEXICAL
    // `..`-collapse via `canonicalize_or_lexical` (the same helper used
    // elsewhere in this file). A raw fallback that kept literal `..`
    // components would weaken the `starts_with` overlap checks below —
    // e.g. `project_root/../project` would not lexically start with
    // `project_root`.
    let canonical_targets: Vec<PathBuf> = effective_watch_targets
        .iter()
        .map(|t| canonicalize_or_lexical(t))
        .collect();

    // The candidate side needs the same treatment: `project_root` (or the
    // OS temp dir) can itself sit behind a symlinked ancestor — e.g. macOS
    // resolves `/tmp` to `/private/tmp` and `/var` to `/private/var`, which
    // is exactly where `std::env::temp_dir()` and many project checkouts
    // live. Comparing a canonical target against a non-canonical candidate
    // (or vice versa) would silently miss a real overlap, so canonicalise
    // the base each candidate is built from before joining the scratch
    // subpath — the base always exists (it's `project_root` /
    // `std::env::temp_dir()`), so this canonicalisation is expected to
    // succeed; `canonicalize_or_lexical` falls back to a LEXICAL
    // `..`-collapse (not the raw base) if it doesn't, keeping the overlap
    // checks below robust for the same reason as the targets above.
    let canonical_project_root = canonicalize_or_lexical(project_root);
    let temp_dir = std::env::temp_dir();
    let canonical_temp_dir = canonicalize_or_lexical(&temp_dir);

    let overlaps = |candidate: &Path, canonical_candidate: &Path| {
        effective_watch_targets
            .iter()
            .chain(canonical_targets.iter())
            .any(|target| {
                candidate.starts_with(target)
                    || target.starts_with(candidate)
                    || canonical_candidate.starts_with(target)
                    || target.starts_with(canonical_candidate)
            })
    };

    let default_parent = project_root
        .join(".zfb-build")
        .join("watcher-liveness-probe");
    let canonical_default_parent = canonical_project_root
        .join(".zfb-build")
        .join("watcher-liveness-probe");
    if !overlaps(&default_parent, &canonical_default_parent) {
        return Some((default_parent, false));
    }

    // Asymmetry worth calling out: unlike the project-local default (one
    // `.zfb-build/` per checkout), this relocated temp parent is a single
    // fixed path SHARED by every `zfb dev` process on the host — across
    // different projects, not just concurrent runs of this one. That is
    // safe: `sweep_stale_probe_sessions` decides staleness purely by the
    // per-session advisory lock, never by project identity, so one project's
    // sweep reclaiming another dead project's abandoned session dir here is
    // correct, not a cross-project leak.
    let temp_parent = temp_dir.join("zfb-dev-watcher-liveness-probe");
    let canonical_temp_parent = canonical_temp_dir.join("zfb-dev-watcher-liveness-probe");
    if !overlaps(&temp_parent, &canonical_temp_parent) {
        return Some((temp_parent, true));
    }

    // Both the project-local default AND the OS temp-dir fallback overlap
    // the resolved watch surface (e.g. an adversarial `extraWatchPaths`
    // entry covering `/` or the OS temp dir itself) — there is no location
    // we can prove is isolated. Skip the probe entirely rather than risk
    // its own marker writes triggering the rebuild storm it exists to
    // detect, not cause.
    None
}

/// Generate a unique probe-session subdirectory name: `session-<pid>-<serial>-<nanos>`.
///
/// The pid makes ownership legible to a human inspecting `.zfb-build/`;
/// actual uniqueness (needed because a project can run more than one
/// `zfb dev` concurrently) comes from a per-process serial counter plus a
/// nanosecond timestamp — mirroring `make_dev_content_trace_token`'s nonce
/// shape elsewhere in `dev.rs`.
pub(super) fn unique_probe_session_dir_name() -> String {
    static NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let serial = NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("session-{}-{serial}-{nanos}", std::process::id())
}

/// Sweep `parent_dir` for orphaned probe-session directories left behind by
/// a `zfb dev` process that never got to run its own cleanup (e.g. `SIGKILL`,
/// a host crash) — never a directory belonging to another `zfb dev` that is
/// still genuinely running its probe.
///
/// Staleness is decided with an advisory file lock, not by parsing the pid
/// out of the directory name and signalling it: a raw `kill(pid, 0)`-style
/// check would need a new production dependency (this crate's only `libc`
/// dependency is Unix-only and test-only today) AND is vulnerable to pid
/// reuse — a dead session's pid can be recycled by an unrelated live
/// process between that session dying and this sweep running, which would
/// make a truly-stale directory look alive. Every probe session creates a
/// `.owner.lock` file inside its own directory and holds an exclusive lock
/// on it (via `std::fs::File::try_lock`, stable since Rust 1.89) for the
/// directory's entire lifetime; the OS releases that lock unconditionally
/// when the owning process dies, however abruptly. So: if this sweep can
/// itself acquire the lock, no live owner holds it — remove the directory.
/// If the lock attempt is refused, a live session owns it — leave it alone.
///
/// Deliberately only matches the final `session-`-prefixed name, never a
/// `.staging-`-prefixed one (see `run_watcher_liveness_probe_inner`'s
/// staging-then-rename sequence): a `session-` name only exists once its
/// owner already holds the lock, so this sweep never has to reason about
/// a same-named directory that's still mid-construction. An abandoned
/// staging dir (a genuine crash between its creation and the rename) is
/// left as a small, bounded leak rather than risk this sweep racing a
/// sibling process's own in-progress construction.
fn sweep_stale_probe_sessions(parent_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(parent_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let is_probe_session = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with("session-"))
            .unwrap_or(false);
        if !is_probe_session {
            continue;
        }
        let lock_path = path.join(".owner.lock");
        match std::fs::File::open(&lock_path) {
            Ok(lock_file) => match lock_file.try_lock() {
                Ok(()) => {
                    // We just acquired it ourselves -> no live owner.
                    let _ = lock_file.unlock();
                    drop(lock_file);
                    let _ = std::fs::remove_dir_all(&path);
                }
                Err(_) => {
                    // Either genuinely held by a live session (WouldBlock)
                    // or an ambiguous platform error — either way, never
                    // risk deleting a directory we can't prove is dead.
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Lock file genuinely absent: either a crash landed between
                // mkdir and creating the lock (a directory that was never
                // actually watched, safe to remove), or something unrelated
                // created a same-shaped name. Both are safe to remove —
                // nothing holding a lock means nothing to race.
                let _ = std::fs::remove_dir_all(&path);
            }
            Err(error) => {
                // The lock file EXISTS but we could not open it — e.g. fd
                // exhaustion (EMFILE) or a permission error (EACCES). This
                // says nothing about whether a live probe owns the session;
                // treating it as "stale" would let a transient open failure
                // delete a locked directory from under its running owner.
                // Skip the entry and leave it for a future sweep instead.
                tracing::debug!(
                    path = %lock_path.display(),
                    %error,
                    "watcher liveness sweep: could not open session lock file; leaving session dir untouched"
                );
            }
        }
    }
}

/// RAII guard for a live probe-session directory: unlocks + closes the
/// `.owner.lock` file first (some platforms refuse to delete an open file),
/// then removes the whole directory. This is a plain destructor, so it
/// still runs if the owning task is `.abort()`ed mid-handshake (Ctrl+C
/// during the deadline) — the task's stack still unwinds and drops its
/// locals even on cancellation, unlike cleanup code written after an
/// `.await` that a cancellation can skip entirely.
struct ProbeSessionGuard {
    dir: PathBuf,
    lock_file: Option<std::fs::File>,
}

impl Drop for ProbeSessionGuard {
    fn drop(&mut self) {
        if let Some(f) = self.lock_file.take() {
            let _ = f.unlock();
            drop(f);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Pure decision: what should happen for a given liveness verdict?
///
/// Returns `Some(warning_text)` only for `TimedOut` — the loud "hot-reload
/// looks dead" case. `Live` has nothing to report; `SetupFailed` gets its
/// own quiet `tracing::debug!` side effect in
/// [`react_to_liveness_outcome`] instead, because a probe SETUP failure
/// (can't create a scratch dir, can't start a watcher) says nothing about
/// whether the real FSEvents/inotify stream is alive — reporting it as a
/// dead-watcher warning would misattribute a disk/permission problem.
///
/// Kept as a pure function (mirroring the `fmt_*` / print split in
/// `crate::output`) so a test can assert on the decision for each verdict
/// by injecting it directly, without booting a real dev server or
/// simulating a broken host.
fn watcher_liveness_warning_for(outcome: &zfb_watcher::LivenessOutcome) -> Option<String> {
    match outcome {
        zfb_watcher::LivenessOutcome::TimedOut => Some(output::fmt_watcher_liveness_timed_out()),
        zfb_watcher::LivenessOutcome::Live | zfb_watcher::LivenessOutcome::SetupFailed(_) => None,
    }
}

/// Print/log the side effect for a completed liveness check.
fn react_to_liveness_outcome(outcome: zfb_watcher::LivenessOutcome) {
    if let Some(warning) = watcher_liveness_warning_for(&outcome) {
        output::warn(warning);
    }
    match outcome {
        zfb_watcher::LivenessOutcome::Live => {
            tracing::debug!("watcher liveness probe: OK (observed its own probe marker)");
        }
        zfb_watcher::LivenessOutcome::TimedOut => {}
        zfb_watcher::LivenessOutcome::SetupFailed(reason) => {
            tracing::debug!(
                reason = %reason,
                "watcher liveness probe: setup failed; skipping dead-watcher verdict"
            );
        }
    }
}

/// Core probe body, factored out from [`spawn_watcher_liveness_probe`] so a
/// test can `.await` it directly with an injected `sleep_before` instead of
/// booting a real dev server — proving the ready banner is never delayed by
/// a slow probe without depending on real notify/FSEvents timing (which
/// resolves fast on a healthy host and so can't be used to deterministically
/// simulate "slow").
///
/// `sleep_before` is always `Duration::ZERO` in production
/// ([`spawn_watcher_liveness_probe`]); a nonzero value exists only for that
/// test.
async fn run_watcher_liveness_probe_inner(
    project_root: PathBuf,
    effective_watch_targets: Vec<PathBuf>,
    opts: zfb_watcher::LivenessOpts,
    sleep_before: std::time::Duration,
) {
    if !sleep_before.is_zero() {
        tokio::time::sleep(sleep_before).await;
    }

    // The probe body is entirely synchronous filesystem + advisory-lock I/O
    // (canonicalize, create_dir_all, read_dir, File::create/try_lock, rename,
    // remove_dir_all) plus the up-to-`opts.deadline` handshake in
    // `check_liveness`. Run all of it on a dedicated blocking thread so it
    // never parks a tokio async worker. `check_liveness` is itself async (it
    // sleeps between marker writes), so the blocking body drives it to
    // completion via the captured runtime handle's `block_on` — permitted
    // because a `spawn_blocking` thread is outside any async context.
    //
    // Shutdown (issue #1718): `spawn_blocking` is NOT cancellable. `run()`'s
    // Ctrl+C path `abort()`s the OUTER task; that promptly cancels THIS
    // future at the `.await` below (so server drain / shutdown is never held
    // up), but the blocking closure detaches and keeps running to completion
    // on its own thread, where `ProbeSessionGuard` still unlocks + removes
    // the session dir. The residual cost is at PROCESS EXIT: the
    // `#[tokio::main]` runtime waits for started `spawn_blocking` tasks when
    // it is dropped, so a Ctrl+C landing mid-handshake can delay exit by up
    // to `opts.deadline` (10s default). That is bounded (never an indefinite
    // hang) and only materializes on a genuinely broken-watcher host, where
    // the healthy-host handshake would instead have resolved in well under a
    // second. Cleanup is never skipped: the detached body finishes its guard
    // drop, and at worst the next boot's stale sweep reclaims a session dir
    // that outlived the process by a few syscalls. This bounded-await
    // tradeoff is a deliberate consequence of running the whole handshake off
    // the async workers; a cancellable handshake would require threading a
    // cancel signal through the `zfb-watcher` primitive (tracked separately,
    // issue #1738).
    let handle = tokio::runtime::Handle::current();
    let _ = tokio::task::spawn_blocking(move || {
        run_watcher_liveness_probe_blocking(&handle, project_root, effective_watch_targets, opts);
    })
    .await;
}

/// Synchronous body of [`run_watcher_liveness_probe_inner`], run on a
/// `spawn_blocking` thread so its blocking `std::fs` / flock / `remove_dir_all`
/// calls and the up-to-`opts.deadline` handshake never park an async worker.
///
/// Drives the async [`zfb_watcher::check_liveness`] to completion via
/// `handle.block_on` — safe here because a blocking-pool thread is not an
/// async execution context. `handle` must belong to a multi-threaded runtime
/// (production runs under `#[tokio::main]`; the direct tests use the
/// `multi_thread` flavor) — a `current_thread` runtime whose sole thread is
/// already parked awaiting this closure's `JoinHandle` would deadlock on the
/// nested `block_on`.
fn run_watcher_liveness_probe_blocking(
    handle: &tokio::runtime::Handle,
    project_root: PathBuf,
    effective_watch_targets: Vec<PathBuf>,
    opts: zfb_watcher::LivenessOpts,
) {
    let Some((parent_dir, _relocated)) =
        resolve_probe_parent_dir(&project_root, &effective_watch_targets)
    else {
        tracing::debug!(
            "watcher liveness probe: no isolated scratch location available; skipping this run's self-check"
        );
        return;
    };
    if std::fs::create_dir_all(&parent_dir).is_err() {
        tracing::debug!(
            path = %parent_dir.display(),
            "watcher liveness probe: failed to create scratch parent dir; skipping this run's self-check"
        );
        return;
    }

    sweep_stale_probe_sessions(&parent_dir);

    // Build the session under a hidden staging name FIRST, acquire the
    // lock there, and only THEN atomically rename it to the
    // `session-`-prefixed name `sweep_stale_probe_sessions` matches (code
    // review finding, issue #1718). Without this, a concurrently-starting
    // sibling `zfb dev` process's own sweep could observe a `session-`
    // directory in the brief window after `create_dir_all` but before its
    // lock file exists — see the "no lock file present" branch in
    // `sweep_stale_probe_sessions` — and delete a session that was about
    // to go live. Renaming means no observer ever sees a `session-`-named
    // directory whose lock isn't already held. The residual race (another
    // sweep landing in the couple of syscalls between staging-dir creation
    // and lock acquisition) is left unguarded on purpose: the staging name
    // is never matched by the sweep at all, so the worst case is a tiny
    // abandoned staging dir on a genuine crash mid-construction — a bounded
    // leak, not a live session getting deleted.
    let session_name = unique_probe_session_dir_name();
    let staging_dir = parent_dir.join(format!(".staging-{session_name}"));
    if std::fs::create_dir_all(&staging_dir).is_err() {
        tracing::debug!(
            path = %staging_dir.display(),
            "watcher liveness probe: failed to create session scratch dir; skipping this run's self-check"
        );
        return;
    }
    let lock_path = staging_dir.join(".owner.lock");
    let lock_file = match std::fs::File::create(&lock_path) {
        Ok(f) => f,
        Err(_) => {
            let _ = std::fs::remove_dir_all(&staging_dir);
            return;
        }
    };
    if lock_file.try_lock().is_err() {
        // Vanishingly unlikely for a freshly-generated unique dir, but
        // never proceed without proof of exclusive ownership.
        drop(lock_file);
        let _ = std::fs::remove_dir_all(&staging_dir);
        return;
    }
    let session_dir = parent_dir.join(session_name);
    if std::fs::rename(&staging_dir, &session_dir).is_err() {
        drop(lock_file);
        let _ = std::fs::remove_dir_all(&staging_dir);
        return;
    }
    let _guard = ProbeSessionGuard {
        dir: session_dir.clone(),
        lock_file: Some(lock_file),
    };

    let outcome = handle.block_on(zfb_watcher::check_liveness(&session_dir, opts));
    react_to_liveness_outcome(outcome);
    // `_guard` drops here on every return path above: unlocks + removes
    // `session_dir` regardless of verdict. Because this runs on a
    // `spawn_blocking` thread — which is NOT cancelled when `run()`'s
    // shutdown `abort()`s the outer task — the drop still executes even on a
    // mid-handshake Ctrl+C, so cleanup is never skipped by cancellation.
}

/// Spawn the watcher-liveness self-check (issue #1718, epic #1716) as a
/// standalone background task. See `run()`'s call site for the ordering
/// contract this must uphold (spawned AFTER the listener binds, never
/// awaited before the ready banner) and the shutdown wiring that cancels +
/// awaits the returned handle.
pub(super) fn spawn_watcher_liveness_probe(
    project_root: PathBuf,
    effective_watch_targets: Vec<PathBuf>,
    opts: zfb_watcher::LivenessOpts,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run_watcher_liveness_probe_inner(
        project_root,
        effective_watch_targets,
        opts,
        std::time::Duration::ZERO,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // Watcher liveness probe (issue #1718, epic #1716)
    // -----------------------------------------------------------------

    #[test]
    fn probe_parent_stays_in_project_when_no_overlap() {
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path().to_path_buf();

        let (parent, relocated) = resolve_probe_parent_dir(&project_root, &[]).unwrap();

        assert!(!relocated, "no overlap should never relocate");
        assert!(
            parent.starts_with(&project_root),
            "default probe parent must live under the project root: {parent:?}"
        );
        assert!(
            parent.ends_with("watcher-liveness-probe"),
            "expected the documented scratch dir name: {parent:?}"
        );
    }

    #[test]
    fn probe_parent_relocates_when_it_overlaps_an_extra_watch_path() {
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path().to_path_buf();
        // Adversarial config: an `extraWatchPaths` entry covering the ENTIRE
        // project root would otherwise swallow the default
        // `.zfb-build/watcher-liveness-probe` location into the
        // orchestrator's watch set.
        let effective_targets = vec![project_root.clone()];

        let (parent, relocated) =
            resolve_probe_parent_dir(&project_root, &effective_targets).unwrap();

        assert!(relocated, "an overlapping extraWatchPaths must relocate");
        assert!(
            !parent.starts_with(&project_root),
            "a relocated probe parent must escape the project root entirely: {parent:?}"
        );
        assert!(
            parent.starts_with(std::env::temp_dir()),
            "a relocated probe parent must live under the OS temp dir: {parent:?}"
        );
    }

    #[test]
    fn probe_parent_relocates_when_a_relative_watch_root_covers_it() {
        // Mirrors the overlap check using a project-relative watch root
        // (joined against `project_root` the same way `run()` does before
        // calling this), not just an already-absolute `extraWatchPaths`
        // entry.
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path().to_path_buf();
        let effective_targets = vec![project_root.join(".zfb-build")];

        let (_parent, relocated) =
            resolve_probe_parent_dir(&project_root, &effective_targets).unwrap();

        assert!(
            relocated,
            "a watch root covering `.zfb-build` must relocate the probe"
        );
    }

    #[cfg(unix)]
    #[test]
    fn probe_parent_relocates_when_a_symlinked_watch_root_resolves_to_an_ancestor() {
        // Code-review finding (issue #1718): a watch root can be a symlink
        // whose REAL location is an ancestor of `.zfb-build` even though the
        // raw path string doesn't lexically look like it — e.g. a
        // collection or `extraWatchPaths` entry that is a symlink back up
        // to the project root. A lexical-only overlap check would miss
        // this even though the real watcher (which canonicalises every
        // root it registers) would cover the probe dir.
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path().to_path_buf();
        let alias = project_root.join("current");
        std::os::unix::fs::symlink(&project_root, &alias).unwrap();

        let (_parent, relocated) = resolve_probe_parent_dir(&project_root, &[alias]).unwrap();

        assert!(
            relocated,
            "a symlinked watch root resolving to an ancestor of `.zfb-build` must relocate the probe"
        );
    }

    #[test]
    fn probe_skips_entirely_when_both_candidate_locations_overlap() {
        // Code-review finding (issue #1718): the OS-temp-dir fallback
        // itself needs to be checked for overlap too — an adversarial
        // `extraWatchPaths` entry covering the whole project root AND the
        // OS temp dir leaves no isolated location to relocate to.
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path().to_path_buf();
        let effective_targets = vec![project_root.clone(), std::env::temp_dir()];

        let result = resolve_probe_parent_dir(&project_root, &effective_targets);

        assert!(
            result.is_none(),
            "with no isolated candidate location, the probe must be skipped, not guess: {result:?}"
        );
    }

    #[test]
    fn probe_session_dir_names_are_unique() {
        let a = unique_probe_session_dir_name();
        let b = unique_probe_session_dir_name();
        assert_ne!(a, b, "two calls in the same process must not collide");
        assert!(a.starts_with("session-"), "got: {a}");
    }

    #[test]
    fn stale_sweep_removes_dead_session_but_keeps_live_locked_session() {
        let parent = tempfile::tempdir().unwrap();

        // "Dead": a session dir whose lock file exists but is NOT held by
        // anyone right now — simulates a process that died without
        // releasing it (the OS releases the lock unconditionally on exit).
        let dead_dir = parent.path().join("session-dead-1-1");
        std::fs::create_dir_all(&dead_dir).unwrap();
        std::fs::File::create(dead_dir.join(".owner.lock")).unwrap();

        // "Live": a session dir whose lock file IS held for the duration
        // of this test, simulating a concurrently-running `zfb dev`. Use a
        // deliberately different fake pid in the name to prove staleness is
        // decided by the lock, not by matching this test process's own pid.
        let live_dir = parent.path().join("session-999999999-2-2");
        std::fs::create_dir_all(&live_dir).unwrap();
        let live_lock = std::fs::File::create(live_dir.join(".owner.lock")).unwrap();
        live_lock.try_lock().expect("test process holds the lock");

        // A directory that doesn't match the `session-` prefix must never
        // be touched by the sweep, regardless of its contents.
        let unrelated_dir = parent.path().join("not-a-probe-session");
        std::fs::create_dir_all(&unrelated_dir).unwrap();

        sweep_stale_probe_sessions(parent.path());

        assert!(!dead_dir.exists(), "a dead session's dir must be removed");
        assert!(live_dir.exists(), "a live session's dir must survive");
        assert!(
            unrelated_dir.exists(),
            "a non-`session-`-prefixed dir must never be touched"
        );

        drop(live_lock);
    }

    /// Regression: the stale sweep must only treat a GENUINELY-ABSENT lock
    /// file (`NotFound`) as "safe to remove". If `File::open` fails for any
    /// other reason — fd exhaustion (EMFILE), a permission error (EACCES) —
    /// a LIVE, locked session's dir could be removed from under its running
    /// probe. Simulate a non-`NotFound` open failure with an unreadable
    /// (mode 000) lock file and assert the session dir survives.
    #[cfg(unix)]
    #[test]
    fn stale_sweep_keeps_session_when_lock_open_fails_non_notfound() {
        use std::os::unix::fs::PermissionsExt;

        let parent = tempfile::tempdir().unwrap();
        let session_dir = parent.path().join("session-locked-1-1");
        std::fs::create_dir_all(&session_dir).unwrap();
        let lock_path = session_dir.join(".owner.lock");
        std::fs::File::create(&lock_path).unwrap();

        let mut perms = std::fs::metadata(&lock_path).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&lock_path, perms).unwrap();

        // Some environments (root, certain sandboxes) bypass file-permission
        // checks entirely. Only assert when the open actually fails with a
        // non-`NotFound` error, so this test skips rather than false-passing
        // where it can't exercise the path it targets.
        let enforced = matches!(
            std::fs::File::open(&lock_path),
            Err(ref e) if e.kind() != std::io::ErrorKind::NotFound
        );

        if enforced {
            sweep_stale_probe_sessions(parent.path());
            assert!(
                session_dir.exists(),
                "a session whose lock file can't be opened (non-NotFound, e.g. EACCES/EMFILE) \
                 must survive the sweep"
            );
        }

        // Restore perms so the tempdir's own Drop cleanup can remove it.
        let mut perms = std::fs::metadata(&lock_path).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&lock_path, perms).unwrap();
    }

    #[test]
    fn timed_out_verdict_produces_the_loud_warning_text() {
        let warning = watcher_liveness_warning_for(&zfb_watcher::LivenessOutcome::TimedOut);
        let text = warning.expect("TimedOut must produce a warning");
        assert!(text.contains("hot-reload"), "got: {text}");
    }

    #[test]
    fn live_and_setup_failed_verdicts_produce_no_warning() {
        assert!(watcher_liveness_warning_for(&zfb_watcher::LivenessOutcome::Live).is_none());
        assert!(
            watcher_liveness_warning_for(&zfb_watcher::LivenessOutcome::SetupFailed(
                "disk full".to_string()
            ))
            .is_none()
        );
    }

    /// Deterministic proof that spawning the probe never delays whatever
    /// runs right after it (the ready banner, in `run()`) — even when the
    /// probe body itself is slow. Uses an injected `sleep_before` rather
    /// than a real broken-watcher scenario: a real `check_liveness` call
    /// resolves fast (`Live`) on a healthy CI host (see
    /// `zfb-watcher`'s own `live_fast_on_a_healthy_host` test), so it can't
    /// be used to deterministically simulate "slow" without flaking.
    // multi_thread flavor: the probe body now drives `check_liveness` via
    // `Handle::block_on` inside `spawn_blocking`, which deadlocks on a
    // single-threaded runtime whose only thread is parked awaiting the
    // blocking JoinHandle. Production runs multi_thread (`#[tokio::main]`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ready_banner_ordering_is_not_delayed_by_a_slow_probe() {
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path().to_path_buf();
        let events: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

        let events_for_probe = Arc::clone(&events);
        let probe_project_root = project_root.clone();
        let handle = tokio::spawn(async move {
            run_watcher_liveness_probe_inner(
                probe_project_root,
                Vec::new(),
                zfb_watcher::LivenessOpts::default(),
                std::time::Duration::from_millis(200),
            )
            .await;
            events_for_probe.lock().unwrap().push("probe_done");
        });

        // Mirrors `run()`'s ordering exactly: the probe is spawned above
        // WITHOUT being awaited, then the next statement (standing in for
        // `output::ready_with_interfaces`) records immediately.
        events.lock().unwrap().push("ready_banner");

        handle.await.unwrap();

        let order = events.lock().unwrap().clone();
        assert_eq!(
            order,
            vec!["ready_banner", "probe_done"],
            "the ready banner must be recorded before a slow probe completes"
        );
    }

    /// End-to-end lifecycle proof: a real probe run against a healthy host
    /// creates its scratch parent, resolves (fast — `Live`, per
    /// `zfb-watcher`'s own healthy-host test), and cleans up its OWN
    /// session dir afterward without needing the caller to do anything.
    // multi_thread flavor: see `ready_banner_ordering_is_not_delayed_by_a_slow_probe`
    // — the probe drives `check_liveness` via `Handle::block_on` inside
    // `spawn_blocking`, which needs a runtime that isn't the single blocked thread.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_watcher_liveness_probe_cleans_up_its_session_dir_on_success() {
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path().to_path_buf();

        let handle = spawn_watcher_liveness_probe(
            project_root.clone(),
            Vec::new(),
            zfb_watcher::LivenessOpts::new(std::time::Duration::from_secs(10))
                .with_marker_interval(std::time::Duration::from_millis(100))
                .with_poll_interval(std::time::Duration::from_millis(10)),
        );
        handle.await.unwrap();

        let (parent_dir, relocated) = resolve_probe_parent_dir(&project_root, &[]).unwrap();
        assert!(!relocated);
        assert!(
            parent_dir.exists(),
            "the scratch parent dir itself is left in place for the next session"
        );
        let leftover_sessions: Vec<_> = std::fs::read_dir(&parent_dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        assert!(
            leftover_sessions.is_empty(),
            "the completed session's own scratch dir must be cleaned up: {leftover_sessions:?}"
        );
    }
}
