//! Plugin bundling helper — Wave 3 of the Plugin TS Bundling epic (#2303).
//!
//! Given a plugin entry path, an esbuild path/getter, and a project root,
//! produce a bundled `.mjs` artifact esbuild can resolve and transform on
//! its own. This module is a **pure helper**: it does not wire into the
//! plugin spawn path (`plugin_runner.rs`) or widen the plugin API surface —
//! that is Wave 4's job. See #2307 and the Wave-2 decision record
//! (#2303#issuecomment-5188860914) for the locked spec this module
//! implements verbatim.
//!
//! Keeper architecture (from `l-lessons-client-bundling`): esbuild is the
//! resolver. This module does no TypeScript or `paths` resolution of its
//! own — it shells out to the pinned esbuild binary and lets esbuild's own
//! tsconfig discovery (walking up from the entry's real on-disk location)
//! govern `jsx`/`paths` semantics.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, bail, Context, Result};
use tempfile::{NamedTempFile, TempDir};
use tokio::process::Command;
use tokio::time;
use tracing::warn;

use crate::bundler::{resolve_esbuild_binary_with_env, DEFAULT_ESBUILD_SLOT};

/// Filename prefix for staged plugin bundle artifacts
/// (`.zfb-plugin-bundle-*.mjs`), following the #1976 temp-file naming
/// convention (see `zfb_plugin_resolver::VIRTUAL_MODULE_TEMP_PREFIX`).
pub const PLUGIN_BUNDLE_TEMP_PREFIX: &str = ".zfb-plugin-bundle-";

/// Filename suffix for staged plugin bundle artifacts.
pub const PLUGIN_BUNDLE_TEMP_SUFFIX: &str = ".mjs";

/// Age threshold for the stray-bundle sweep below, modelled on
/// `zfb_css::engine`'s `ENTRY_TMP_STALE_AFTER` (zfb#821) but deliberately
/// longer than that constant's 60s: a plugin bundle's staged file is left
/// unlocked while esbuild writes it (see [`bundle_plugin_entry`]), so this
/// window must comfortably exceed [`ESBUILD_BUNDLE_TIMEOUT`] or a concurrent
/// process's sweep could reap an in-flight bundle purely on mtime.
const PLUGIN_BUNDLE_TEMP_STALE_AFTER: Duration = Duration::from_secs(600);

/// Wedge-guard deadline for the esbuild subprocess in [`bundle_plugin_entry`]
/// — a generous backstop, not a performance bound. Mirrors the 5-minute
/// constant `adapter.rs`'s `run_capturing` already uses for the same class
/// of stalled-subprocess risk.
const ESBUILD_BUNDLE_TIMEOUT: Duration = Duration::from_secs(300);

// The staging window (create the temp file → esbuild writes it → lock it) is
// unlocked for at most `ESBUILD_BUNDLE_TIMEOUT`, after which the subprocess is
// killed and the file dropped. Keeping the staleness threshold strictly above
// that bound is what makes "unlocked AND past the stale window" a sound proof
// of abandonment rather than a race against a slow bundle.
const _: () = assert!(
    PLUGIN_BUNDLE_TEMP_STALE_AFTER.as_secs() > ESBUILD_BUNDLE_TIMEOUT.as_secs(),
    "PLUGIN_BUNDLE_TEMP_STALE_AFTER must exceed ESBUILD_BUNDLE_TIMEOUT"
);

/// Bounded ETXTBSY spawn retries for the esbuild subprocess in
/// [`bundle_plugin_entry`] (issue #2378). Same budget and linear backoff
/// (10ms, 20ms, …) as `zfb-config-loader`'s identically-named constants,
/// which fixed this exact race class for its own subprocess spawns
/// (zfb#1008): the race window is fork-to-exec sized, so a handful of
/// short retries is far more than enough without delaying genuine failures.
///
/// Replicated here rather than shared: `zfb-build` does not depend on
/// `zfb-config-loader`, and that crate's helper is shaped around
/// spawn-and-collect-`Output` (it owns a timeout too), while this call site
/// needs the retry around the bare `spawn()` alone.
const ETXTBSY_MAX_RETRIES: u32 = 5;
const ETXTBSY_RETRY_DELAY: Duration = Duration::from_millis(10);

/// `--banner:js` injected into every plugin bundle so a `.cts`/CommonJS
/// entry's `require(...)` calls keep working after bundling (codex review
/// finding 1, #2324).
///
/// esbuild bundles a CommonJS-shaped entry (e.g. `.cts` using
/// `require()`/`export =`) into `--format=esm` output by wrapping it and
/// routing any `require(...)` call through its own `__require` shim. That
/// shim's fallback (`typeof require !== "undefined" ? require : …`) checks
/// for an AMBIENT `require` first — but Node ESM modules have no ambient
/// `require` at all, so without this banner the shim always falls through
/// to its "Dynamic require of … is not supported" throw, breaking every
/// `.cts` plugin that uses ordinary CommonJS syntax. Defining a real
/// top-level `require` via `node:module`'s `createRequire`, anchored to
/// `import.meta.url` (so relative specifiers resolve from the staged
/// bundle's own location), makes the shim defer to it instead. A no-op for
/// pure-ESM sources, which never reference `require` at all.
const REQUIRE_SHIM_BANNER: &str = "import { createRequire as __zfbCreateRequire } from \"node:module\"; const require = __zfbCreateRequire(import.meta.url);";

/// zfb-build's own mirror of `zfb_config_loader::EmbeddedEsbuildGetter`.
///
/// Deliberately NOT shared with the config-loader's type — refactoring the
/// config loader to share it would risk `load_from_ts_file`'s behaviour,
/// which must stay byte-identical (#2307). A packaged `zfb` build supplies
/// a getter that extracts the embedded esbuild binary into a `TempDir`; the
/// caller must keep that `TempDir` alive for as long as the resolved path
/// is used.
pub type EmbeddedEsbuildGetter = Box<dyn FnOnce() -> Option<(TempDir, PathBuf)> + Send>;

/// True iff `module_file_url`'s resolved `file://` URL ends in exactly one
/// of `.ts`, `.tsx`, `.mts`, `.cts` (lowercase match). This is the whole
/// scope gate for plugin bundling — the *call* into [`bundle_plugin_entry`]
/// happens in Wave 4, not here.
pub fn needs_bundling(module_file_url: &str) -> bool {
    let path_part = module_file_url
        .strip_prefix("file://")
        .unwrap_or(module_file_url);
    matches!(
        Path::new(path_part).extension().and_then(|e| e.to_str()),
        Some("ts") | Some("tsx") | Some("mts") | Some("cts")
    )
}

/// Resolve the esbuild binary to bundle a plugin entry with.
///
/// Thin wrapper over the shared [`resolve_esbuild_binary_with_env`] — the
/// shared resolver itself is not modified. `explicit` is always `None`
/// here (plugin bundling has no per-call explicit-path override) and the
/// workspace slot is never overridden, so a tier-4 fallback always means
/// [`DEFAULT_ESBUILD_SLOT`].
///
/// Only the tier-4 "default slot" failure is remapped to a plugin-scoped
/// message below — its `` Evaluating a `zfb.config.ts`… `` hint must never
/// surface on the plugin path. Tier-1/2 failures (an explicit path or
/// `ZFB_ESBUILD_BIN` set but missing) pass through unchanged.
pub fn resolve_esbuild_for_plugins(
    embedded_getter: Option<EmbeddedEsbuildGetter>,
) -> Result<(Option<TempDir>, PathBuf)> {
    resolve_esbuild_binary_with_env(None, |k| std::env::var_os(k), embedded_getter, None)
        .map_err(remap_default_slot_not_found)
}

/// Remap only the shared resolver's tier-4 "default slot" failure to the
/// plugin-scoped esbuild-not-found text; every other failure mode passes
/// through unchanged. Split out from [`resolve_esbuild_for_plugins`] so it
/// can be exercised directly against resolver errors produced with an
/// injected env/slot (the locked `resolve_esbuild_for_plugins` signature
/// accepts neither) without duplicating the mapping logic in test code.
fn remap_default_slot_not_found(err: anyhow::Error) -> anyhow::Error {
    // The tier-4 "default slot" message is the only one carrying this
    // config-loader-specific hint; every other failure mode passes through
    // untouched.
    if err.to_string().contains("Evaluating a `zfb.config.ts`") {
        anyhow!(
            "plugin bundling: esbuild binary not found — bundling a \
             `.ts`/`.tsx`/`.mts`/`.cts` plugin entry needs the pinned \
             esbuild. Set ZFB_ESBUILD_BIN, or stage the workspace binary \
             at {DEFAULT_ESBUILD_SLOT}, or reinstall zfb (packaged builds \
             embed esbuild)."
        )
    } else {
        err
    }
}

/// A bundled plugin artifact staged next to its entry file.
///
/// Owns the staged temp-file guard: dropping this value deletes the staged
/// `.mjs` file. [`Self::file_url`] exposes the staged file's `file://` URL
/// for the caller to hand to a module loader.
#[derive(Debug)]
pub struct StagedPluginBundle {
    staged: NamedTempFile,
    file_url: String,
}

impl StagedPluginBundle {
    /// The staged bundle's `file://` URL.
    pub fn file_url(&self) -> &str {
        &self.file_url
    }

    /// The staged bundle's on-disk path.
    pub fn path(&self) -> &Path {
        self.staged.path()
    }
}

/// Drive `attempt` until it returns anything other than a retryable
/// `ExecutableFileBusy` ("Text file busy", raw OS error 26), with at most
/// [`ETXTBSY_MAX_RETRIES`] linearly backed-off retries.
///
/// The esbuild binary being spawned may have JUST been written — a packaged
/// `zfb` extracts the embedded binary into a tempdir before spawning it. If
/// an unrelated thread forks while that write's fd is briefly open, the
/// forked child holds the fd until its own `execve`, and our `execve` of the
/// freshly-written file fails with ETXTBSY (issue #2378; `zfb-config-loader`
/// hit and fixed the identical race in zfb#1008).
///
/// Retrying is side-effect free because the discriminator guarantees it:
/// ETXTBSY can only originate from the `execve` inside `spawn()`, so a
/// matching error means the child never ran. Every other error — including
/// the `NotFound` of a missing binary — returns on the first attempt, so
/// callers' error wording and latency are unchanged.
async fn spawn_with_etxtbsy_retry<T, F>(mut attempt: F) -> std::io::Result<T>
where
    F: FnMut() -> std::io::Result<T>,
{
    let mut etxtbsy_attempts = 0u32;
    loop {
        match attempt() {
            Err(err)
                if err.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && etxtbsy_attempts < ETXTBSY_MAX_RETRIES =>
            {
                etxtbsy_attempts += 1;
                time::sleep(ETXTBSY_RETRY_DELAY * etxtbsy_attempts).await;
            }
            other => return other,
        }
    }
}

/// Bundle a plugin entry (a `.ts`/`.tsx`/`.mts`/`.cts` file) into a staged
/// `.mjs` artifact using `esbuild_bin`.
///
/// Staging happens in the entry's OWN parent directory — not a shared
/// tempdir — because `--packages=external` bare specifiers resolve at
/// runtime via Node's ancestor `node_modules` walk from the staged file.
/// Anchoring next to the entry makes that resolution identical to the
/// original entry's, for both project-local plugins (hoisted deps) and
/// package plugins (their own nested `node_modules`). There is no fallback
/// location: an unwritable entry directory is a hard error.
///
/// No `--jsx*` / `--tsconfig` flags are passed — esbuild auto-discovers the
/// plugin project's own tsconfig by walking up from the entry's real
/// on-disk location, so the plugin author's `jsx`/`paths` settings govern.
pub async fn bundle_plugin_entry(
    entry: &Path,
    esbuild_bin: &Path,
    plugin_name: &str,
) -> Result<StagedPluginBundle> {
    let stage_dir = entry.parent().ok_or_else(|| {
        anyhow!(
            "plugin bundling: failed to stage bundle for plugin `{plugin_name}` in \
             {}: entry path has no parent directory",
            entry.display()
        )
    })?;

    // Best-effort: reap any stale `.zfb-plugin-bundle-*.mjs` files a
    // previous run stranded in this same directory (issue #2371) before
    // staging a new one. See `sweep_stale_plugin_bundle_files` below.
    sweep_stale_plugin_bundle_files(stage_dir);

    let staged = tempfile::Builder::new()
        .prefix(PLUGIN_BUNDLE_TEMP_PREFIX)
        .suffix(PLUGIN_BUNDLE_TEMP_SUFFIX)
        .tempfile_in(stage_dir)
        .map_err(|err| {
            anyhow!(
                "plugin bundling: failed to stage bundle for plugin `{plugin_name}` in \
                 {}: {err}",
                stage_dir.display()
            )
        })?;
    let staged_path = staged.path().to_path_buf();

    // Claim ownership before esbuild runs where the platform allows it. A
    // Unix `flock` is advisory and invisible to esbuild — which never locks
    // its own `--outfile` — so the lock costs nothing and covers the one case
    // the age gate below cannot: a process frozen (laptop sleep, SIGSTOP)
    // past the staleness window with esbuild still to finish, where the
    // wedge-guard timeout cannot fire either because the whole task is
    // suspended. Windows is excluded deliberately: there `lock_shared` maps
    // to `LockFileEx`, whose shared range lock blocks writes through OTHER
    // handles, so locking here would make esbuild's own `--outfile` write
    // fail (see the post-write lock below).
    #[cfg(unix)]
    let _ = staged.as_file().lock_shared();

    // `kill_on_drop` + the `tokio::time::timeout` below is the async
    // equivalent of `adapter.rs`'s `run_capturing` 5-minute wedge guard: a
    // stalled esbuild binary (or a `ZFB_ESBUILD_BIN` wrapper whose
    // descendant inherits and holds the stdout/stderr pipes open) must not
    // block plugin startup indefinitely. Dropping the timed-out future
    // drops the `Child`, which kills the direct process.
    let mut command = Command::new(esbuild_bin);
    command
        .arg(entry)
        .arg("--bundle")
        .arg("--format=esm")
        .arg("--platform=node")
        .arg("--target=node22")
        .arg("--packages=external")
        .arg("--sourcemap=inline")
        .arg("--log-level=warning")
        .arg("--color=false")
        .arg(format!("--banner:js={REQUIRE_SHIM_BANNER}"))
        .arg(format!("--outfile={}", staged_path.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    // Only the spawn is retried, and only on ETXTBSY — see
    // `spawn_with_etxtbsy_retry` for why that is side-effect free.
    let child = spawn_with_etxtbsy_retry(|| command.spawn())
        .await
        .with_context(|| {
            format!("plugin bundling: failed to spawn esbuild for plugin `{plugin_name}`")
        })?;

    let output = match time::timeout(ESBUILD_BUNDLE_TIMEOUT, child.wait_with_output()).await {
        Ok(result) => result.with_context(|| {
            format!("plugin bundling: failed to run esbuild for plugin `{plugin_name}`")
        })?,
        Err(_) => {
            bail!(
                "plugin bundling: esbuild did not exit within {}s while bundling plugin \
                 `{plugin_name}` ({}) — killed",
                ESBUILD_BUNDLE_TIMEOUT.as_secs(),
                entry.display()
            );
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "plugin bundling: esbuild failed for plugin `{plugin_name}` ({}):\n{stderr}",
            entry.display()
        );
    }

    if !output.stderr.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(
            plugin = plugin_name,
            "esbuild reported warnings while bundling plugin entry: {stderr}"
        );
    }

    // Lock now that esbuild has finished writing `--outfile` — the #1976
    // write-then-lock convention (`zfb-plugin-resolver`'s
    // `allocate_locked_entry_tmp`). This is the ONLY point Windows can lock
    // at: `lock_shared` maps to `LockFileEx` there, whose shared range lock
    // blocks WRITES THROUGH OTHER HANDLES, so locking before the spawn would
    // make esbuild's own `--outfile` write fail on every
    // `.ts`/`.tsx`/`.mts`/`.cts` plugin — and `@takazudo/zfb-win32-x64-msvc`
    // is a shipped platform. Windows' unlocked staging window is covered
    // instead by `PLUGIN_BUNDLE_TEMP_STALE_AFTER` exceeding
    // `ESBUILD_BUNDLE_TIMEOUT` (see the const assertion above), so a
    // concurrent process's sweep can never age out a bundle esbuild is still
    // writing. On Unix the lock is already held from before the spawn and
    // this call is a harmless re-assert. Shared (not exclusive), since
    // nothing but a reading module loader ever opens this file again.
    let _ = staged.as_file().lock_shared();

    let file_url = url::Url::from_file_path(&staged_path)
        .map_err(|()| {
            anyhow!(
                "plugin bundling: failed to convert staged bundle path {} to a file:// URL",
                staged_path.display()
            )
        })?
        .into();

    Ok(StagedPluginBundle { staged, file_url })
}

/// Delete `.zfb-plugin-bundle-*.mjs` files left in `stage_dir` by a
/// previous run that died before its [`tempfile::NamedTempFile`] `Drop`
/// could clean up (SIGKILL / crash / Ctrl-C) — see zfb#2371. Mirrors
/// `zfb_css::engine`'s `sweep_stale_entry_files` (zfb#821): only files
/// matching the exact [`PLUGIN_BUNDLE_TEMP_PREFIX`]/[`PLUGIN_BUNDLE_TEMP_SUFFIX`]
/// shape and older than [`PLUGIN_BUNDLE_TEMP_STALE_AFTER`] are candidates,
/// so a sibling build's freshly-staged bundle is never touched.
///
/// On top of the age gate, a stale candidate is spared if a live process
/// still holds it: `bundle_plugin_entry` takes a *shared* `flock` on its
/// staged file, so an EXCLUSIVE `try_lock` succeeding here proves no such
/// process is still alive, regardless of age — mirroring
/// `zfb_islands::esbuild`'s `sweep_orphaned_in_project_temp_files`
/// lock-spare check (#1976/#2109). Two zfb processes (e.g. `zfb dev` +
/// `zfb build`) can stage bundles in the same plugin directory
/// concurrently; a live bundle must never be reaped. On Windows that lock
/// can only be taken once esbuild has finished writing (#1976's
/// write-then-lock ordering — see the call sites), so a bundle still
/// mid-`esbuild` rests on the age gate alone there; that is why
/// [`PLUGIN_BUNDLE_TEMP_STALE_AFTER`] is pinned above
/// [`ESBUILD_BUNDLE_TIMEOUT`]. Best-effort throughout: any individual error
/// (unreadable dir, racing unlink, permission denied) is ignored — a failed
/// sweep must never fail a build.
fn sweep_stale_plugin_bundle_files(stage_dir: &Path) {
    sweep_stale_plugin_bundle_files_at(stage_dir, SystemTime::now());
}

/// Core of [`sweep_stale_plugin_bundle_files`], parameterised on the
/// reference time so tests can drive staleness deterministically without an
/// mtime setter.
fn sweep_stale_plugin_bundle_files_at(stage_dir: &Path, now: SystemTime) {
    let entries = match std::fs::read_dir(stage_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with(PLUGIN_BUNDLE_TEMP_PREFIX)
            && name.ends_with(PLUGIN_BUNDLE_TEMP_SUFFIX))
        {
            continue;
        }
        // Skip directories and anything we can't stat.
        let metadata = match entry.metadata() {
            Ok(m) if m.is_file() => m,
            _ => continue,
        };
        let old_enough = metadata
            .modified()
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok())
            .map(|age| age >= PLUGIN_BUNDLE_TEMP_STALE_AFTER)
            .unwrap_or(false);
        if !old_enough {
            continue;
        }
        let path = entry.path();
        // Reopen and attempt an EXCLUSIVE lock. `OpenOptions` shares delete
        // access, so holding this handle does not stop the `remove_file`
        // call below on Windows.
        let Ok(candidate) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
        else {
            continue;
        };
        if candidate.try_lock().is_ok() {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use zfb_test_utils::locate_esbuild as locate_real_esbuild;

    // -------------------------------------------------------------------
    // sweep_stale_plugin_bundle_files — zfb#2371 stray-bundle sweep
    // -------------------------------------------------------------------

    /// A reference "now" far enough in the future that any just-written
    /// file reads as stale to the sweep — exercises the stale branch
    /// without an mtime setter.
    fn future_now() -> SystemTime {
        SystemTime::now() + PLUGIN_BUNDLE_TEMP_STALE_AFTER + Duration::from_secs(3600)
    }

    /// A stale `.zfb-plugin-bundle-*.mjs` left in the stage dir by an
    /// aborted run (SIGKILL skipped its RAII `Drop`) must be removed by the
    /// sweep so it never lands in the consumer's tracked tree.
    #[test]
    fn sweep_removes_a_stale_stranded_bundle_file() {
        let dir = tempfile::tempdir().unwrap();
        let stranded = dir.path().join(format!(
            "{PLUGIN_BUNDLE_TEMP_PREFIX}UErIaE{PLUGIN_BUNDLE_TEMP_SUFFIX}"
        ));
        fs::write(&stranded, b"// leaked plugin bundle\n").unwrap();

        sweep_stale_plugin_bundle_files_at(dir.path(), future_now());

        assert!(
            !stranded.exists(),
            "a stale stranded bundle file should have been swept; still present at {}",
            stranded.display()
        );
    }

    /// The sweep must NOT delete a freshly-staged bundle file — that would
    /// be a concurrent `zfb dev`/`zfb build` process's live bundle.
    #[test]
    fn sweep_keeps_a_fresh_bundle_file() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join(format!(
            "{PLUGIN_BUNDLE_TEMP_PREFIX}rN8CIp{PLUGIN_BUNDLE_TEMP_SUFFIX}"
        ));
        fs::write(&live, b"// live plugin bundle\n").unwrap();

        sweep_stale_plugin_bundle_files_at(dir.path(), SystemTime::now());

        assert!(
            live.exists(),
            "a fresh bundle file (a concurrent live bundle) must be kept"
        );
    }

    /// The age gate alone cannot tell "abandoned" from "owned by a process
    /// suspended past the stale window". The shared lock `bundle_plugin_entry`
    /// takes on its own staged file must win over the age gate.
    #[test]
    fn sweep_keeps_a_stale_bundle_file_still_holding_the_shared_lock() {
        let dir = tempfile::tempdir().unwrap();
        let owned = dir.path().join(format!(
            "{PLUGIN_BUNDLE_TEMP_PREFIX}owned{PLUGIN_BUNDLE_TEMP_SUFFIX}"
        ));
        fs::write(&owned, b"// owned plugin bundle\n").unwrap();

        let owner = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&owned)
            .unwrap();
        owner.lock_shared().expect("take the owner's shared lock");

        sweep_stale_plugin_bundle_files_at(dir.path(), future_now());
        assert!(
            owned.exists(),
            "a bundle file still locked by its owner must be spared however old it is"
        );

        // Once the owner releases it — as the OS does for a process that
        // dies without unwinding — the next sweep reaps it.
        drop(owner);
        sweep_stale_plugin_bundle_files_at(dir.path(), future_now());
        assert!(
            !owned.exists(),
            "an unlocked, past-stale-window bundle file must be swept"
        );
    }

    /// The sweep must touch ONLY files matching the exact
    /// `PLUGIN_BUNDLE_TEMP_PREFIX`/`PLUGIN_BUNDLE_TEMP_SUFFIX` shape — even
    /// ones old enough to be eligible by age must survive otherwise.
    #[test]
    fn sweep_leaves_non_matching_files_alone() {
        let dir = tempfile::tempdir().unwrap();
        let user_entry = dir.path().join("index.ts");
        let wrong_suffix = dir
            .path()
            .join(format!("{PLUGIN_BUNDLE_TEMP_PREFIX}abc.txt"));
        let wrong_prefix = dir
            .path()
            .join(format!("some-other-prefix-abc{PLUGIN_BUNDLE_TEMP_SUFFIX}"));
        fs::write(&user_entry, b"export default {};\n").unwrap();
        fs::write(&wrong_suffix, b"x").unwrap();
        fs::write(&wrong_prefix, b"x").unwrap();

        sweep_stale_plugin_bundle_files_at(dir.path(), future_now());

        assert!(
            user_entry.exists(),
            "the plugin's own TS entry must never be swept"
        );
        assert!(
            wrong_suffix.exists(),
            "file with the right prefix but wrong suffix must not be swept"
        );
        assert!(
            wrong_prefix.exists(),
            "file with the right suffix but wrong prefix must not be swept"
        );
    }

    /// A missing stage dir must be a silent no-op (best effort) — it must
    /// never panic or surface an error into the build.
    #[test]
    fn sweep_on_missing_stage_dir_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        sweep_stale_plugin_bundle_files_at(&missing, future_now()); // must not panic
    }

    // -------------------------------------------------------------------
    // needs_bundling
    // -------------------------------------------------------------------

    #[test]
    fn needs_bundling_matches_ts_family_extensions() {
        for ext in ["ts", "tsx", "mts", "cts"] {
            assert!(
                needs_bundling(&format!("file:///project/plugins/index.{ext}")),
                "expected .{ext} to need bundling"
            );
        }
    }

    #[test]
    fn needs_bundling_rejects_non_ts_extensions_and_bare_paths() {
        for ext in ["js", "mjs", "cjs", "json"] {
            assert!(
                !needs_bundling(&format!("file:///project/plugins/index.{ext}")),
                ".{ext} must not need bundling"
            );
        }
        assert!(!needs_bundling("file:///project/plugins/index"));
    }

    #[test]
    fn needs_bundling_is_case_sensitive() {
        // Only the exact lowercase extensions count — an uppercase
        // spelling must not match (decision (b) in #2307: "lowercase
        // match").
        assert!(!needs_bundling("file:///project/plugins/index.TS"));
        assert!(!needs_bundling("file:///project/plugins/index.Tsx"));
    }

    #[test]
    fn needs_bundling_works_without_a_file_scheme_prefix() {
        // Callers pass a resolved `file://` URL, but the gate is defined
        // purely on the extension, so a bare path works the same way.
        assert!(needs_bundling("/project/plugins/index.ts"));
        assert!(!needs_bundling("/project/plugins/index.js"));
    }

    // -------------------------------------------------------------------
    // resolve_esbuild_for_plugins — tier-4 remap
    // -------------------------------------------------------------------

    #[test]
    fn tier4_default_slot_failure_is_remapped_to_plugin_scoped_text() {
        // `cargo test`/`cargo nextest run` sets the test binary's CWD to
        // the crate root (`crates/zfb-build`), so DEFAULT_ESBUILD_SLOT
        // ("crates/zfb/binaries/esbuild/esbuild", resolved relative to
        // CWD) never exists from here regardless of workspace state —
        // verified via a CWD probe during authoring. That makes a genuine
        // tier-4 failure reliably reachable through the REAL production
        // function (no getter, no slot override needed), rather than
        // re-deriving the remap against a hand-built resolver error.
        if std::env::var_os("ZFB_ESBUILD_BIN").is_some() {
            eprintln!(
                "[tier4_default_slot_failure_is_remapped_to_plugin_scoped_text] ZFB_ESBUILD_BIN \
                 is set in the ambient environment; skipping (would mask the tier-4 path)"
            );
            return;
        }

        let err = resolve_esbuild_for_plugins(None).expect_err(
            "with no env override, no getter, and a CWD-relative default slot that cannot \
             exist here, resolution must fail",
        );
        let msg = err.to_string();
        assert!(
            msg.starts_with(
                "plugin bundling: esbuild binary not found — bundling a \
                 `.ts`/`.tsx`/`.mts`/`.cts` plugin entry needs the pinned esbuild."
            ),
            "msg: {msg}"
        );
        assert!(msg.contains("ZFB_ESBUILD_BIN"), "msg: {msg}");
        assert!(msg.contains(DEFAULT_ESBUILD_SLOT), "msg: {msg}");
        assert!(
            !msg.contains("Evaluating a `zfb.config.ts`"),
            "the config-loader hint must never leak onto the plugin path: {msg}"
        );
    }

    #[test]
    fn tier2_env_failure_passes_through_the_shared_remap_function_unchanged() {
        // Tier 2 (ZFB_ESBUILD_BIN set but missing) can't be driven through
        // resolve_esbuild_for_plugins itself without mutating real process
        // env, which this codebase's own tests avoid (see
        // `resolve_esbuild_binary_with_env`'s doc comment and
        // `preview.rs`'s `env_truthy` tests). Instead, produce a genuine
        // tier-2 error via the shared resolver's own injection pattern and
        // feed it through the SAME private remap function
        // `resolve_esbuild_for_plugins` calls, proving the pass-through
        // branch on real (not hand-authored) resolver output.
        let tmp = tempfile::tempdir().unwrap();
        let missing_env_bin = tmp.path().join("not-there");
        let err = resolve_esbuild_binary_with_env(
            None,
            {
                let missing_env_bin = missing_env_bin.clone();
                move |k: &str| {
                    if k == "ZFB_ESBUILD_BIN" {
                        Some(missing_env_bin.clone().into_os_string())
                    } else {
                        None
                    }
                }
            },
            None::<fn() -> Option<(TempDir, PathBuf)>>,
            None,
        )
        .unwrap_err();
        let msg = remap_default_slot_not_found(err).to_string();
        assert!(
            msg.contains("ZFB_ESBUILD_BIN="),
            "tier-2 error must pass through unchanged: {msg}"
        );
        assert!(
            !msg.starts_with("plugin bundling: esbuild binary not found —"),
            "tier-2 error must NOT be remapped: {msg}"
        );
    }

    // -------------------------------------------------------------------
    // resolve_esbuild_for_plugins — embedded tier (mandatory per #2307)
    // -------------------------------------------------------------------

    #[test]
    fn embedded_getter_wins_when_explicit_and_env_are_absent() {
        let Some(real) = locate_real_esbuild() else {
            eprintln!(
                "[embedded_getter_wins_when_explicit_and_env_are_absent] no esbuild binary; skipping"
            );
            return;
        };
        if std::env::var_os("ZFB_ESBUILD_BIN").is_some() {
            eprintln!(
                "[embedded_getter_wins_when_explicit_and_env_are_absent] ZFB_ESBUILD_BIN is set \
                 in the ambient environment; skipping (would confound the getter-wins assertion)"
            );
            return;
        }

        let getter = make_stub_embedded_getter(real.clone());
        let (handle, resolved_path) = resolve_esbuild_for_plugins(Some(getter))
            .expect("embedded getter should resolve a usable esbuild path");
        assert!(
            handle.is_some(),
            "embedded tier must return the TempDir handle, proving that tier (not the workspace slot) was taken"
        );
        assert_ne!(
            resolved_path, real,
            "resolved path should be the STAGED copy the getter produced, not the original binary"
        );
        assert!(resolved_path.exists());
    }

    #[test]
    fn zfb_esbuild_bin_env_still_wins_over_embedded_getter() {
        // Driven via the injected env-getter pattern the existing resolver
        // tests use (bundler.rs's `missing_esbuild_binary_returns_actionable_error`
        // and friends) rather than through resolve_esbuild_for_plugins,
        // whose env tier reads real process env and cannot be injected.
        // A panicking getter proves tier 2 short-circuits before tier 3
        // is ever reached.
        let tmp = tempfile::tempdir().unwrap();
        let env_bin = tmp.path().join("esbuild-from-env");
        fs::write(&env_bin, b"stub").unwrap();
        let env_bin_for_closure = env_bin.clone();

        let getter: EmbeddedEsbuildGetter =
            Box::new(|| panic!("embedded getter must not be invoked when ZFB_ESBUILD_BIN is set"));

        let (handle, resolved) = resolve_esbuild_binary_with_env(
            None,
            move |k: &str| {
                if k == "ZFB_ESBUILD_BIN" {
                    Some(env_bin_for_closure.clone().into_os_string())
                } else {
                    None
                }
            },
            Some(getter),
            None,
        )
        .expect("ZFB_ESBUILD_BIN should resolve without touching the embedded getter");

        assert!(
            handle.is_none(),
            "the env tier never returns a TempDir handle"
        );
        assert_eq!(resolved, env_bin);
    }

    #[tokio::test]
    async fn embedded_tempdir_keeps_the_staged_path_alive_across_a_bundle_call() {
        let Some(real) = locate_real_esbuild() else {
            eprintln!(
                "[embedded_tempdir_keeps_the_staged_path_alive_across_a_bundle_call] no esbuild binary; skipping"
            );
            return;
        };

        let getter = make_stub_embedded_getter(real);
        let (handle, resolved_path) = resolve_esbuild_for_plugins(Some(getter))
            .expect("embedded getter should resolve a usable esbuild path");
        let handle = handle.expect("embedded tier must return the TempDir handle");

        let project = tempfile::tempdir().unwrap();
        let entry = project.path().join("plugin.ts");
        fs::write(
            &entry,
            "export default { name: \"embedded-tier-plugin\" };\n",
        )
        .unwrap();

        // The TempDir handle is still alive here — the whole point of this
        // assertion. Dropping it before this call would delete the staged
        // esbuild binary the bundle call is about to spawn.
        let staged = bundle_plugin_entry(&entry, &resolved_path, "embedded-tier-plugin")
            .await
            .expect("bundling using the embedded-tier resolved path should succeed");
        assert!(staged.file_url().starts_with("file://"));
        assert!(staged.file_url().ends_with(".mjs"));

        drop(handle);
    }

    /// Build a fake [`EmbeddedEsbuildGetter`] that stages a real, runnable
    /// esbuild copy into a fresh `TempDir` — the shape a packaged `zfb`
    /// build's real getter produces (extracting the embedded binary),
    /// without depending on the embedded-vendor snapshot that only the
    /// `zfb` crate has access to.
    fn make_stub_embedded_getter(real_esbuild: PathBuf) -> EmbeddedEsbuildGetter {
        Box::new(move || {
            let dir = tempfile::Builder::new()
                .prefix("zfb-embedded-esbuild-stub-")
                .tempdir()
                .ok()?;
            let dest = dir.path().join(real_esbuild.file_name()?);
            fs::copy(&real_esbuild, &dest).ok()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&dest).ok()?.permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&dest, perms).ok()?;
            }
            Some((dir, dest))
        })
    }

    // -------------------------------------------------------------------
    // bundle_plugin_entry — real esbuild coverage (self-skipping when
    // esbuild is unavailable, matching bundler.rs's own convention).
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn ts_entry_with_extensionless_js_import_resolves_to_sibling_ts() {
        let Some(bin) = locate_real_esbuild() else {
            eprintln!(
                "[ts_entry_with_extensionless_js_import_resolves_to_sibling_ts] no esbuild binary; skipping"
            );
            return;
        };
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        fs::write(
            root.join("x.ts"),
            "export const marker = \"sibling-ts-marker\";\n",
        )
        .unwrap();
        let entry = root.join("index.ts");
        fs::write(
            &entry,
            "import { marker } from \"./x.js\";\nexport default { marker };\n",
        )
        .unwrap();

        let staged = bundle_plugin_entry(&entry, &bin, "dot-js-import-plugin")
            .await
            .expect("esbuild should resolve ./x.js to the sibling x.ts source");
        let body = fs::read_to_string(staged.path()).unwrap();
        assert!(
            body.contains("sibling-ts-marker"),
            "bundle should inline the sibling .ts module's export: {body}"
        );
    }

    #[tokio::test]
    async fn ts_entry_using_an_enum_bundles_and_transforms_it() {
        let Some(bin) = locate_real_esbuild() else {
            eprintln!(
                "[ts_entry_using_an_enum_bundles_and_transforms_it] no esbuild binary; skipping"
            );
            return;
        };
        let project = tempfile::tempdir().unwrap();
        let entry = project.path().join("index.ts");
        fs::write(
            &entry,
            "export enum Color { Red, Green, Blue }\n\
             export default { label: Color[Color.Green] };\n",
        )
        .unwrap();

        let staged = bundle_plugin_entry(&entry, &bin, "enum-plugin")
            .await
            .expect("esbuild should transform a TS enum construct");
        let body = fs::read_to_string(staged.path()).unwrap();
        assert!(
            !body.contains("enum "),
            "the TS `enum` keyword must not survive the transform: {body}"
        );
        assert!(
            body.contains("Green"),
            "the enum's members should still be present post-transform: {body}"
        );
    }

    #[tokio::test]
    async fn ts_entry_using_a_namespace_bundles_and_transforms_it() {
        let Some(bin) = locate_real_esbuild() else {
            eprintln!(
                "[ts_entry_using_a_namespace_bundles_and_transforms_it] no esbuild binary; skipping"
            );
            return;
        };
        let project = tempfile::tempdir().unwrap();
        let entry = project.path().join("index.ts");
        fs::write(
            &entry,
            "export namespace Utils { export const answer = 42; }\n\
             export default { answer: Utils.answer };\n",
        )
        .unwrap();

        let staged = bundle_plugin_entry(&entry, &bin, "namespace-plugin")
            .await
            .expect("esbuild should transform a TS namespace construct");
        let body = fs::read_to_string(staged.path()).unwrap();
        assert!(
            !body.contains("namespace "),
            "the TS `namespace` keyword must not survive the transform: {body}"
        );
        assert!(
            body.contains("42"),
            "the namespace member's value should still be present post-transform: {body}"
        );
    }

    #[tokio::test]
    async fn ts_entry_using_parameter_properties_bundles_and_transforms_them() {
        let Some(bin) = locate_real_esbuild() else {
            eprintln!(
                "[ts_entry_using_parameter_properties_bundles_and_transforms_them] no esbuild binary; skipping"
            );
            return;
        };
        let project = tempfile::tempdir().unwrap();
        let entry = project.path().join("index.ts");
        fs::write(
            &entry,
            "class Plugin {\n  constructor(public label: string) {}\n}\n\
             export default new Plugin(\"param-prop-marker\");\n",
        )
        .unwrap();

        let staged = bundle_plugin_entry(&entry, &bin, "param-props-plugin")
            .await
            .expect("esbuild should transform TS constructor parameter properties");
        let body = fs::read_to_string(staged.path()).unwrap();
        assert!(
            !body.contains("public "),
            "the TS `public` parameter-property modifier must not survive the transform: {body}"
        );
        assert!(
            body.contains("this.label = label") || body.contains("this.label=label"),
            "the parameter property should compile to an explicit constructor assignment: {body}"
        );
    }

    #[tokio::test]
    async fn tsx_import_is_resolved_and_jsx_is_transformed() {
        let Some(bin) = locate_real_esbuild() else {
            eprintln!(
                "[tsx_import_is_resolved_and_jsx_is_transformed] no esbuild binary; skipping"
            );
            return;
        };
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        // esbuild auto-discovers this tsconfig by walking up from the
        // entry's real on-disk location — no --jsx*/--tsconfig flag needed.
        fs::write(
            root.join("tsconfig.json"),
            r#"{"compilerOptions":{"jsx":"react-jsx","jsxImportSource":"react"}}"#,
        )
        .unwrap();
        fs::write(
            root.join("widget.tsx"),
            "export const Widget = () => <div>plugin-widget-marker</div>;\n",
        )
        .unwrap();
        let entry = root.join("index.ts");
        fs::write(
            &entry,
            "import { Widget } from \"./widget.tsx\";\nexport default { Widget };\n",
        )
        .unwrap();

        let staged = bundle_plugin_entry(&entry, &bin, "tsx-import-plugin")
            .await
            .expect("esbuild should resolve the .tsx import and transform its JSX");
        let body = fs::read_to_string(staged.path()).unwrap();
        assert!(
            body.contains("plugin-widget-marker"),
            "the .tsx sibling's content should be inlined: {body}"
        );
        assert!(
            !body.contains("<div>"),
            "raw JSX syntax must not survive the transform: {body}"
        );
        // react/jsx-runtime is external (--packages=external), so the
        // transformed JSX calls into an unresolved external import rather
        // than inlining a runtime we never installed.
        assert!(
            body.contains("react/jsx-runtime") || body.contains("react/jsx-dev-runtime"),
            "JSX should compile down to an external jsx-runtime call: {body}"
        );
    }

    #[tokio::test]
    async fn paths_alias_is_resolved_via_esbuild_auto_discovered_tsconfig() {
        let Some(bin) = locate_real_esbuild() else {
            eprintln!(
                "[paths_alias_is_resolved_via_esbuild_auto_discovered_tsconfig] no esbuild binary; skipping"
            );
            return;
        };
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        fs::write(
            root.join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@lib/*":["./lib/*"]}}}"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("lib")).unwrap();
        fs::write(
            root.join("lib/helper.ts"),
            "export const helperMarker = \"paths-alias-marker\";\n",
        )
        .unwrap();
        let entry = root.join("index.ts");
        fs::write(
            &entry,
            "import { helperMarker } from \"@lib/helper\";\nexport default { helperMarker };\n",
        )
        .unwrap();

        let staged = bundle_plugin_entry(&entry, &bin, "paths-alias-plugin")
            .await
            .expect(
                "esbuild should resolve the @lib/* paths alias via the auto-discovered tsconfig",
            );
        let body = fs::read_to_string(staged.path()).unwrap();
        assert!(
            body.contains("paths-alias-marker"),
            "the paths-aliased module's export should be inlined: {body}"
        );
    }

    #[tokio::test]
    async fn cts_entry_using_require_and_export_equals_bundles_and_executes() {
        // Codex review finding 1 (P1, #2324): a string-equality check on
        // the bundled body cannot prove `require(...)` actually works at
        // runtime — esbuild's `__require` shim text looks identical
        // whether or not it falls through to its "Dynamic require of …
        // is not supported" throw. This test EXECUTES the staged bundle
        // via a real `node` subprocess (skips like the other real-esbuild
        // tests when `node` isn't on PATH) rather than only inspecting
        // its source text. The higher-level through-the-host regression
        // (proving the same shape survives `PluginHost::spawn_with_timeout`
        // end to end) lives in `plugin_runner.rs`'s
        // `cts_plugin_using_require_and_export_equals_executes_through_real_host`.
        let Some(bin) = locate_real_esbuild() else {
            eprintln!(
                "[cts_entry_using_require_and_export_equals_bundles_and_executes] no esbuild binary; skipping"
            );
            return;
        };
        let node_available = Command::new("node")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);
        if !node_available {
            eprintln!(
                "[cts_entry_using_require_and_export_equals_bundles_and_executes] no node on PATH; skipping"
            );
            return;
        }

        let project = tempfile::tempdir().unwrap();
        let entry = project.path().join("plugin.cts");
        // Deliberately ordinary CommonJS-in-.cts constructs: a top-level
        // `require()` call (so a broken shim fails at import time) and
        // `export =` instead of `export default`.
        fs::write(
            &entry,
            "const { platform } = require(\"node:os\");\n\
             const plugin = { marker: \"cts-executes-marker\", platform: platform() };\n\
             export = plugin;\n",
        )
        .unwrap();

        let staged = bundle_plugin_entry(&entry, &bin, "cts-require-plugin")
            .await
            .expect("esbuild should bundle a .cts entry using require()/export=");

        // Deliberately a real `.mjs` FILE run as `node runner.mjs`, not
        // `node -e "<script>"`: `-e` runs the eval string itself as a
        // CommonJS module, and Node makes THAT module's `require` visible
        // as an ambient global even inside modules it dynamically
        // `import()`s — which would silently mask exactly the bug this
        // test exists to catch (verified directly against Node 24: `node
        // -e "import('./mod.mjs')"` does NOT reproduce the "Dynamic
        // require" failure that a genuine `.mjs` entrypoint does). A real
        // `.mjs` file matches how `plugin-host.mjs` itself is invoked in
        // production (`node --enable-source-maps plugin-host.mjs`).
        let runner = project.path().join("runner.mjs");
        fs::write(
            &runner,
            format!(
                "import({:?}).then(m => {{\n\
                 \x20 if (m.default.marker !== \"cts-executes-marker\") throw new Error(\"marker mismatch: \" + JSON.stringify(m.default));\n\
                 \x20 if (typeof m.default.platform !== \"string\") throw new Error(\"require('node:os').platform() did not return a usable value: \" + JSON.stringify(m.default));\n\
                 }}).catch(e => {{ console.error(e.message); process.exit(1); }});\n",
                staged.file_url(),
            ),
        )
        .unwrap();
        let output = Command::new("node")
            .arg(&runner)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .expect("failed to spawn node to execute the staged .cts bundle");
        assert!(
            output.status.success(),
            "importing the staged .cts bundle must succeed and its top-level require() must \
             resolve — got: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn broken_entry_produces_the_locked_error_shape() {
        let Some(bin) = locate_real_esbuild() else {
            eprintln!("[broken_entry_produces_the_locked_error_shape] no esbuild binary; skipping");
            return;
        };
        let project = tempfile::tempdir().unwrap();
        let entry = project.path().join("index.ts");
        // Deliberately unresolvable import — esbuild exits non-zero.
        fs::write(
            &entry,
            "import { nope } from \"./does-not-exist.ts\";\nexport default { nope };\n",
        )
        .unwrap();

        let err = bundle_plugin_entry(&entry, &bin, "broken-plugin")
            .await
            .expect_err("a deliberately broken entry must fail to bundle");
        let msg = err.to_string();
        let expected_prefix = format!(
            "plugin bundling: esbuild failed for plugin `broken-plugin` ({}):",
            entry.display()
        );
        assert!(
            msg.starts_with(&expected_prefix),
            "error must match the locked prefix exactly.\nexpected prefix: {expected_prefix}\ngot: {msg}"
        );
    }

    // -------------------------------------------------------------------
    // ETXTBSY spawn retry (#2378). The retry loop's contract is attempt
    // counting and error classification, driven through the injectable
    // `attempt` seam because a real ETXTBSY is a fork-to-exec-sized race
    // no test can induce on demand. Virtual time (`start_paused`) keeps
    // the linear-backoff sleeps instant. Mirrors `zfb-config-loader`'s
    // `output_bounded_with` tests (zfb#1008).
    // -------------------------------------------------------------------

    fn etxtbsy() -> std::io::Error {
        std::io::Error::from(std::io::ErrorKind::ExecutableFileBusy)
    }

    #[tokio::test(start_paused = true)]
    async fn spawn_retry_absorbs_transient_etxtbsy_and_then_succeeds() {
        let mut attempts = 0u32;
        let result = spawn_with_etxtbsy_retry(|| {
            attempts += 1;
            if attempts < 3 {
                Err(etxtbsy())
            } else {
                Ok("spawned")
            }
        })
        .await;

        assert_eq!(result.expect("should succeed after retries"), "spawned");
        assert_eq!(attempts, 3, "expected 2 ETXTBSY attempts plus 1 success");
    }

    #[tokio::test(start_paused = true)]
    async fn spawn_retry_exhausts_its_budget_and_surfaces_the_original_error() {
        let mut attempts = 0u32;
        let result = spawn_with_etxtbsy_retry(|| {
            attempts += 1;
            Err::<(), _>(etxtbsy())
        })
        .await;

        let err = result.expect_err("a permanently busy binary must still fail");
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::ExecutableFileBusy,
            "the original error must surface, not a retry-specific one"
        );
        assert_eq!(
            attempts,
            ETXTBSY_MAX_RETRIES + 1,
            "expected 1 initial attempt plus {ETXTBSY_MAX_RETRIES} retries"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn spawn_retry_never_retries_a_non_etxtbsy_failure() {
        let mut attempts = 0u32;
        let result = spawn_with_etxtbsy_retry(|| {
            attempts += 1;
            Err::<(), _>(std::io::Error::from(std::io::ErrorKind::NotFound))
        })
        .await;

        assert_eq!(
            result.expect_err("a missing binary must fail").kind(),
            std::io::ErrorKind::NotFound
        );
        assert_eq!(attempts, 1, "a non-ETXTBSY failure must not be retried");
    }

    /// The retry loop must not disturb the locked spawn-failure wording (or
    /// delay it): a missing binary is `NotFound`, so it returns on the first
    /// attempt with the same message it always carried. No real esbuild
    /// needed — staging succeeds and the spawn is what fails.
    #[tokio::test]
    async fn missing_esbuild_binary_reports_the_locked_spawn_error_shape() {
        let project = tempfile::tempdir().unwrap();
        let entry = project.path().join("index.ts");
        fs::write(&entry, "export default {};\n").unwrap();

        let err = bundle_plugin_entry(
            &entry,
            Path::new("/nonexistent/zfb-plugin-bundler-esbuild"),
            "missing-esbuild-plugin",
        )
        .await
        .expect_err("spawning a nonexistent esbuild must fail");

        let msg = err.to_string();
        assert_eq!(
            msg, "plugin bundling: failed to spawn esbuild for plugin `missing-esbuild-plugin`",
            "error must match the locked spawn-failure shape exactly"
        );
    }

    // No real esbuild needed — the failure happens before any subprocess
    // is spawned, when the entry's parent directory is unwritable. Root
    // (and some sandboxes) bypasses directory permission checks entirely,
    // so this probes enforcement first and skips rather than false-passing
    // — same guard used by `preflight_raw_tree_propagates_non_dangling_walk_errors`
    // above. Perms are restored BEFORE the assertion so a failing assertion
    // can't leave a chmod-000-ish directory behind for the tempdir's `Drop`.
    #[cfg(unix)]
    #[tokio::test]
    async fn staging_write_failure_reports_the_locked_error_shape() {
        use std::os::unix::fs::PermissionsExt;

        let project = tempfile::tempdir().unwrap();
        let stage_dir = project.path().join("readonly-plugin-dir");
        fs::create_dir_all(&stage_dir).unwrap();
        let entry = stage_dir.join("index.ts");
        fs::write(&entry, "export default {};\n").unwrap();

        fs::set_permissions(&stage_dir, fs::Permissions::from_mode(0o555)).unwrap();
        let enforced = fs::write(stage_dir.join(".probe"), b"x").is_err();

        let result = if enforced {
            Some(
                bundle_plugin_entry(
                    &entry,
                    Path::new("/nonexistent/esbuild-should-never-be-invoked"),
                    "readonly-dir-plugin",
                )
                .await,
            )
        } else {
            None
        };

        fs::set_permissions(&stage_dir, fs::Permissions::from_mode(0o755)).unwrap();

        match result {
            Some(result) => {
                let err = result.expect_err("staging into an unwritable directory must fail");
                let msg = err.to_string();
                let expected_prefix = format!(
                    "plugin bundling: failed to stage bundle for plugin `readonly-dir-plugin` in {}:",
                    stage_dir.display()
                );
                assert!(
                    msg.starts_with(&expected_prefix),
                    "error must match the locked staging-failure prefix.\nexpected prefix: {expected_prefix}\ngot: {msg}"
                );
            }
            None => eprintln!(
                "[staging_write_failure_reports_the_locked_error_shape] directory permissions \
                 not enforced (running as root?); skipping assertion."
            ),
        }
    }
}
