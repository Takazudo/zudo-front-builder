//! Plugin bundling: compile a `.ts`/`.tsx`/`.mts`/`.cts` plugin entry into a
//! staged `.mjs` artifact next to the entry, through the supervised native
//! compiler worker (#4633).
//!
//! This module does no TypeScript or `paths` resolution of its own. The
//! typed request leaves tsconfig discovery to the bundler, which resolves
//! each file's nearest tsconfig the way the entry's own toolchain would, so
//! the plugin author's `jsx`/`paths` settings govern.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Result};
use tempfile::NamedTempFile;
use tracing::warn;
use zfb_compiler_worker::request::{OutputLayout, Packages, Platform, SourceMap};
use zfb_compiler_worker::{BundleRequest, CompileJob, CompilerWorker, WorkerError};

/// Filename prefix for staged plugin bundle artifacts
/// (`.zfb-plugin-bundle-*.mjs`), following the #1976 temp-file naming
/// convention (see `zfb_plugin_resolver::VIRTUAL_MODULE_TEMP_PREFIX`).
pub const PLUGIN_BUNDLE_TEMP_PREFIX: &str = ".zfb-plugin-bundle-";

/// Filename suffix for staged plugin bundle artifacts.
pub const PLUGIN_BUNDLE_TEMP_SUFFIX: &str = ".mjs";

/// Age threshold for the stray-bundle sweep below. A staged bundle is
/// unlocked between the worker publishing it and [`bundle_plugin_entry`]
/// re-locking it (and, on Windows, for the whole compile), so this window
/// must comfortably exceed [`PLUGIN_COMPILE_DEADLINE`] or a concurrent
/// process's sweep could reap an in-flight bundle based only on its
/// modification time.
const PLUGIN_BUNDLE_TEMP_STALE_AFTER: Duration = Duration::from_secs(600);

/// Deadline for one plugin compile — a wedge guard, not a performance bound.
/// The worker is terminated and nothing is published when it passes.
const PLUGIN_COMPILE_DEADLINE: Duration = Duration::from_secs(300);

// Keeping the staleness threshold strictly above the compile deadline is
// what makes "unlocked AND past the stale window" a sound proof of
// abandonment rather than a race against a slow compile.
const _: () = assert!(
    PLUGIN_BUNDLE_TEMP_STALE_AFTER.as_secs() > PLUGIN_COMPILE_DEADLINE.as_secs(),
    "PLUGIN_BUNDLE_TEMP_STALE_AFTER must exceed PLUGIN_COMPILE_DEADLINE"
);

/// Banner prepended to every plugin bundle so a `.cts`/CommonJS entry's
/// `require(...)` calls keep working after bundling to ESM (#2324).
///
/// A CommonJS-shaped entry bundled into ESM routes `require(...)` through the
/// bundler's runtime shim, which defers to an ambient `require` — and Node ESM
/// modules have none. Defining a real top-level `require` via `node:module`'s
/// `createRequire`, anchored to `import.meta.url` (so relative specifiers
/// resolve from the staged bundle's own location), makes the shim use it. A
/// no-op for pure-ESM sources, which never reference `require`.
const REQUIRE_SHIM_BANNER: &str = "import { createRequire as __zfbCreateRequire } from \"node:module\"; const require = __zfbCreateRequire(import.meta.url);";

/// True iff `module_file_url`'s resolved `file://` URL ends in exactly one
/// of `.ts`, `.tsx`, `.mts`, `.cts` (lowercase match). This is the whole
/// scope gate for plugin bundling.
pub fn needs_bundling(module_file_url: &str) -> bool {
    let path_part = module_file_url
        .strip_prefix("file://")
        .unwrap_or(module_file_url);
    matches!(
        Path::new(path_part).extension().and_then(|e| e.to_str()),
        Some("ts") | Some("tsx") | Some("mts") | Some("cts")
    )
}

/// The typed compile request for a plugin entry staged at `staged`.
///
/// ESM for Node 22 with no splitting, an inline source map (the host runs
/// Node with `--enable-source-maps`) and the [`REQUIRE_SHIM_BANNER`].
/// Packages stay external — resolved at runtime by Node's ancestor
/// `node_modules` walk from the staged file — after aliases, tsconfig
/// `paths`/`baseUrl` and package imports resolve, so a mapping that targets
/// a local file is still bundled.
pub fn plugin_compile_request(entry: &Path, staged: &Path) -> BundleRequest {
    let cwd = entry.parent().unwrap_or(entry);
    let mut request = BundleRequest::new(cwd, entry, OutputLayout::file(staged), Platform::Node);
    request.target = Some("node22".into());
    request.source_map = SourceMap::Inline;
    request.banner = Some(REQUIRE_SHIM_BANNER.into());
    request.resolve.packages = Packages::External;
    request
}

/// A bundled plugin artifact staged next to its entry file.
///
/// Owns the staged temp-file guard and the shared lock on it: dropping this
/// value deletes the staged `.mjs` file. [`Self::file_url`] exposes the
/// staged file's `file://` URL for the caller to hand to a module loader.
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

/// Compile a plugin entry (a `.ts`/`.tsx`/`.mts`/`.cts` file) into a staged
/// `.mjs` artifact through `worker`.
///
/// Staging happens in the entry's OWN parent directory — not a shared
/// tempdir — because external bare specifiers resolve at runtime via Node's
/// ancestor `node_modules` walk from the staged file, and relative
/// `import.meta.url` usage must see the entry's directory. Anchoring next to
/// the entry makes both identical to the original entry's, for project-local
/// plugins (hoisted deps) and package plugins (their own nested
/// `node_modules`). There is no fallback location: an unwritable entry
/// directory is a hard error.
pub async fn bundle_plugin_entry(
    entry: &Path,
    worker: &CompilerWorker,
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
    // staging a new one.
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

    // The reserved name carries an ownership lock while the worker compiles,
    // where the platform allows it. On Windows `lock_shared` is a
    // `LockFileEx` range lock that blocks writes through other handles, so
    // the reservation stays unlocked there and rests on the age gate.
    #[cfg(unix)]
    let _ = staged.as_file().lock_shared();

    let request = plugin_compile_request(entry, &staged_path);
    let output = worker
        .compile(CompileJob::new(request, PLUGIN_COMPILE_DEADLINE))
        .await
        .map_err(|err| match err {
            WorkerError::Compile { diagnostic } => anyhow!(
                "plugin bundling: failed to compile plugin `{plugin_name}` ({}):\n{diagnostic}",
                entry.display()
            ),
            other => anyhow!(
                "plugin bundling: compiler worker failed for plugin `{plugin_name}` ({}): {other}",
                entry.display()
            ),
        })?;
    for warning in &output.warnings {
        warn!(
            plugin = plugin_name,
            "compiler reported a warning while bundling plugin entry: {warning}"
        );
    }

    // The worker publishes by renaming a new file over the reserved name, so
    // the reservation's handle (and its lock) now names an unlinked inode.
    // Re-open the published file and hold the shared lock on it for the
    // bundle's lifetime; the guard still deletes it by path on drop.
    let temp_path = staged.into_temp_path();
    let published = std::fs::File::open(&temp_path).map_err(|err| {
        anyhow!(
            "plugin bundling: failed to open the compiled bundle for plugin `{plugin_name}` at \
             {}: {err}",
            staged_path.display()
        )
    })?;
    let _ = published.lock_shared();
    let staged = NamedTempFile::from_parts(published, temp_path);

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
/// could clean up (SIGKILL / crash / Ctrl-C) — see zfb#2371. Only files
/// matching the exact [`PLUGIN_BUNDLE_TEMP_PREFIX`]/[`PLUGIN_BUNDLE_TEMP_SUFFIX`]
/// shape and older than [`PLUGIN_BUNDLE_TEMP_STALE_AFTER`] are candidates,
/// so a sibling build's freshly-staged bundle is never touched.
///
/// On top of the age gate, a stale candidate is spared if a live process
/// still holds it: [`bundle_plugin_entry`] holds a *shared* lock on its
/// staged file, so an EXCLUSIVE `try_lock` succeeding here proves no such
/// process is still alive, regardless of age — mirroring
/// `zfb_islands`' `sweep_orphaned_in_project_temp_files` lock-spare check
/// (#1976/#2109). Two zfb processes (e.g. `zfb dev` + `zfb build`) can stage
/// bundles in the same plugin directory concurrently; a live bundle must
/// never be reaped. A bundle still being compiled rests on the age gate
/// alone where it is unlocked, which is why
/// [`PLUGIN_BUNDLE_TEMP_STALE_AFTER`] is pinned above
/// [`PLUGIN_COMPILE_DEADLINE`]. Best-effort throughout: any individual error
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
        let path: PathBuf = entry.path();
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
    use zfb_compiler_worker::WorkerLauncher;

    // Compiles through a real worker live in `tests/plugin_bundling_native.rs`:
    // only integration tests can name the test worker host executable.

    /// A worker whose host program does not exist: any compile attempt fails
    /// at spawn, so reaching it proves the code path started a worker.
    fn unspawnable_worker() -> CompilerWorker {
        CompilerWorker::new(WorkerLauncher::new(
            "/nonexistent/zfb-plugin-compiler-worker",
        ))
    }

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
    // plugin_compile_request
    // -------------------------------------------------------------------

    #[test]
    fn compile_request_carries_the_plugin_bundle_contract() {
        let entry = Path::new("/project/plugins/index.ts");
        let staged = Path::new("/project/plugins/.zfb-plugin-bundle-x.mjs");
        let request = plugin_compile_request(entry, staged);
        assert_eq!(request.cwd, Path::new("/project/plugins"));
        assert_eq!(request.entry, entry);
        assert_eq!(request.output, OutputLayout::file(staged));
        assert_eq!(request.platform, Platform::Node);
        assert_eq!(request.target.as_deref(), Some("node22"));
        assert_eq!(request.source_map, SourceMap::Inline);
        assert_eq!(request.banner.as_deref(), Some(REQUIRE_SHIM_BANNER));
        assert_eq!(request.resolve.packages, Packages::External);
        assert!(!request.splitting);
        assert!(!request.minify);
        assert_eq!(
            request.tsconfig, None,
            "each file's nearest tsconfig must govern, not a pinned one"
        );
        assert!(request.resolve.extra_module_dirs.is_empty());
    }

    // -------------------------------------------------------------------
    // bundle_plugin_entry — failures before or at worker start
    // -------------------------------------------------------------------

    /// A worker host that cannot be started fails the bundle with the
    /// plugin-scoped wrapper and leaves no staged file behind.
    #[tokio::test]
    async fn unstartable_worker_reports_a_plugin_scoped_error_and_stages_nothing() {
        let project = tempfile::tempdir().unwrap();
        let entry = project.path().join("index.ts");
        fs::write(&entry, "export default {};\n").unwrap();

        let err = bundle_plugin_entry(&entry, &unspawnable_worker(), "missing-worker-plugin")
            .await
            .expect_err("a worker host that cannot be spawned must fail the bundle");

        let msg = err.to_string();
        let expected_prefix = format!(
            "plugin bundling: compiler worker failed for plugin `missing-worker-plugin` ({}):",
            entry.display()
        );
        assert!(msg.starts_with(&expected_prefix), "got: {msg}");
        assert!(
            msg.contains("/nonexistent/zfb-plugin-compiler-worker"),
            "the error must name the worker program that failed to start: {msg}"
        );
        let leftovers: Vec<_> = fs::read_dir(project.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(PLUGIN_BUNDLE_TEMP_PREFIX)
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "no staged file may survive a failed bundle"
        );
    }

    // The failure happens before any worker is spawned, when the entry's
    // parent directory is unwritable. Root (and some sandboxes) bypasses
    // directory permission checks entirely, so this probes enforcement first
    // and skips rather than false-passing. Perms are restored BEFORE the
    // assertion so a failing assertion can't leave a chmod-000-ish directory
    // behind for the tempdir's `Drop`.
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
            Some(bundle_plugin_entry(&entry, &unspawnable_worker(), "readonly-dir-plugin").await)
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
