mod html_normalize;
mod sse;
pub use html_normalize::normalize_html;
pub use sse::{
    decode_utf8_incremental, next_sse_event_name, wait_for_subscribers, wait_for_subscribers_polled,
};

use std::path::{Path, PathBuf};

/// Locate an esbuild binary suitable for integration tests.
///
/// Resolution order (union of all existing 17+ per-test copies):
///
/// 1. `ZFB_ESBUILD_BIN` env var — if set and points to an existing file,
///    return it immediately.
/// 2. Workspace-local slot:
///    `<workspace_root>/crates/zfb/binaries/esbuild/<binary>`, probed for
///    every candidate workspace root (see
///    [`candidate_workspace_roots`] — runtime cwd walk-up first, then
///    `current_exe` ancestry, with the compile-time
///    `CARGO_MANIFEST_DIR`-derived root only as a fallback).
///    Binary name is `esbuild` on Unix, `esbuild.exe` on Windows.
/// 3. pnpm nested store, per candidate root:
///    `node_modules/.pnpm/*/node_modules/@esbuild/<suffix>/bin/<binary>`
///    where `*` is each directory entry under `.pnpm` (pnpm's content-
///    addressed layout). Mirrors the shape in
///    `crates/zfb/tests/css_modules_components_build.rs:166-176`.
///    Note: the pnpm Windows package puts the binary at the package root
///    rather than under `bin/`; this path uses `bin/` uniformly following
///    the spec — a future Windows fix can adjust the suffix path if needed.
/// 4. Flat pnpm store, per candidate root:
///    `node_modules/.pnpm/node_modules/esbuild/bin/<binary>` — kept for
///    parity with `crates/zfb-build/tests/embedded_v8_snapshot_e2e.rs`.
/// 5. Portable PATH walk — iterates `$PATH` entries via
///    `std::env::split_paths` (platform-aware `:` vs `;` split) and
///    checks `<entry>/<binary>`. Does NOT shell out to `which`, which is
///    absent on Windows.
///
/// # Why the workspace root is re-derived at runtime (issue #1007)
///
/// The previous implementation derived the root exclusively from
/// compile-time `env!("CARGO_MANIFEST_DIR")`. Cargo deliberately keeps
/// compiled artifacts relocatable: dep-info source paths are stored
/// package-root-relative and the per-unit env vars cargo itself sets
/// (`CARGO_MANIFEST_DIR` among them) are excluded from the env-dep
/// freshness check (`translate_dep_info` in cargo's fingerprint module).
/// With a shared target dir (e.g. a global `build.target-dir`), an rlib
/// compiled from another checkout of this workspace — a since-deleted
/// `worktrees/<topic>/` path — is reused as "fresh", and its baked-in
/// root points at a directory that no longer has (or never had) the slot
/// binary. Every gated test then silently skipped. Runtime re-derivation
/// makes a stale rlib unable to pin an outdated path.
///
/// # Panics
///
/// Panics **only** on the harness-bug state: the expected platform slot
/// binary (`crates/zfb/binaries/esbuild/<binary>`) exists as a file under
/// a candidate workspace root, yet the lookup above still failed. That
/// state is never a legitimately-missing esbuild, and silently skipping
/// it is exactly the issue #1007 incident. When esbuild is genuinely
/// absent (no slot binary, no pnpm store hit, no PATH hit) the function
/// returns `None` so machines without esbuild skip gracefully.
///
/// Callers should gate with
/// `let Some(esbuild) = locate_esbuild() else { return; }` to skip
/// gracefully, matching the convention in all existing integration tests.
pub fn locate_esbuild() -> Option<PathBuf> {
    // Step 1: ZFB_ESBUILD_BIN env var.
    if let Some(p) = std::env::var_os("ZFB_ESBUILD_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }

    let binary = esbuild_binary_name();
    let roots = candidate_workspace_roots();

    // Step 2: workspace-local binary slot, per candidate root.
    for root in &roots {
        let slot = slot_binary_path(root, binary);
        if slot.is_file() {
            return Some(slot);
        }
    }

    for root in &roots {
        // Step 3: pnpm nested store — node_modules/.pnpm/*/node_modules/@esbuild/<suffix>/bin/<binary>.
        if let Some(suffix) = esbuild_npm_suffix() {
            let pnpm_dir = root.join("node_modules/.pnpm");
            if let Ok(rd) = std::fs::read_dir(&pnpm_dir) {
                for entry in rd.flatten() {
                    let cand = entry
                        .path()
                        .join("node_modules/@esbuild")
                        .join(suffix)
                        .join("bin")
                        .join(binary);
                    if cand.is_file() {
                        return Some(cand);
                    }
                }
            }
        }

        // Step 4: flat pnpm store — node_modules/.pnpm/node_modules/esbuild/bin/<binary>.
        let flat_slot = root
            .join("node_modules/.pnpm/node_modules/esbuild/bin")
            .join(binary);
        if flat_slot.is_file() {
            return Some(flat_slot);
        }
    }

    // Step 5: portable PATH walk — no `which` shell-out (missing on Windows).
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let cand = dir.join(binary);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }

    // Lookup failed. Tripwire (issue #1007): if the expected slot binary
    // exists under any candidate root, this is a harness bug — a silent
    // skip here is exactly the incident class this guards against.
    let checked_slots: Vec<(PathBuf, bool)> = roots
        .iter()
        .map(|root| {
            let slot = slot_binary_path(root, binary);
            let is_file = slot.is_file();
            (slot, is_file)
        })
        .collect();
    match classify_failed_lookup(&checked_slots) {
        SkipKind::GenuinelyAbsent => {
            eprintln!(
                "zfb-test-utils: esbuild not found (ZFB_ESBUILD_BIN, workspace slot, \
                 pnpm stores, PATH all missed) — gated test skipped"
            );
            None
        }
        SkipKind::HarnessBug { present_slots } => panic!(
            "zfb-test-utils harness bug (issue #1007): the esbuild slot binary exists at \
             {present:?} but locate_esbuild() failed to return it. Checked slot paths: \
             {checked:?}. This state must never become a silent skip — fix the lookup.",
            present = present_slots
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
            checked = checked_slots
                .iter()
                .map(|(p, _)| p.display().to_string())
                .collect::<Vec<_>>(),
        ),
    }
}

/// Candidate workspace roots for the slot/pnpm probes, highest priority
/// first, deduplicated:
///
/// 1. Ancestors of `std::env::current_dir()` that look like the workspace
///    root (`Cargo.toml` file + `crates/` dir). Cargo runs test binaries
///    with the crate's manifest dir as cwd, so this finds the *real*
///    checkout the tests run from — including a `worktrees/<topic>/` root
///    first and the enclosing main repo root after it.
/// 2. Ancestors of `std::env::current_exe()` — covers runners that invoke
///    the test binary from an unrelated cwd, as long as the target dir
///    lives inside the workspace (a shared/global target dir won't match;
///    that's fine, this is an extra probe).
/// 3. The compile-time `CARGO_MANIFEST_DIR`-derived root, kept as a
///    fallback only. This value can be stale when a shared target dir
///    reuses an rlib compiled from another checkout path (issue #1007),
///    which is why it is probed last.
pub fn candidate_workspace_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    if let Ok(cwd) = std::env::current_dir() {
        for dir in cwd.ancestors() {
            if is_workspace_root(dir) {
                push_unique(&mut roots, dir.to_path_buf());
            }
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        for dir in exe.ancestors() {
            if is_workspace_root(dir) {
                push_unique(&mut roots, dir.to_path_buf());
            }
        }
    }

    // CARGO_MANIFEST_DIR = <repo>/crates/zfb-test-utils → parent = <repo>/crates → parent = <repo>.
    // Pushed unconditionally (no marker check): if stale it simply fails
    // the file probes, preserving the pre-#1007 fallback behavior.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(root) = manifest_dir.parent().and_then(Path::parent) {
        push_unique(&mut roots, root.to_path_buf());
    }

    roots
}

/// Classification of a failed esbuild lookup, decided from explicit probe
/// inputs so the panic contract is hermetically testable (issue #1007).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipKind {
    /// No slot binary exists anywhere we know to look — esbuild is
    /// genuinely absent and the caller should skip gracefully.
    GenuinelyAbsent,
    /// At least one expected slot binary path IS a file, yet the lookup
    /// failed to return it — a harness bug that must panic loudly.
    HarnessBug {
        /// The slot paths that exist as files despite the failed lookup.
        present_slots: Vec<PathBuf>,
    },
}

/// Decide how a failed lookup must behave.
///
/// `checked_slots` pairs each expected platform slot path
/// (`<root>/crates/zfb/binaries/esbuild/<binary>`) with whether that exact
/// path is a file. Only a present slot *binary* triggers
/// [`SkipKind::HarnessBug`] — the slot *directory* being non-empty never
/// does (it permanently holds `.gitkeep` and `README.md`).
pub fn classify_failed_lookup(checked_slots: &[(PathBuf, bool)]) -> SkipKind {
    let present_slots: Vec<PathBuf> = checked_slots
        .iter()
        .filter(|(_, is_file)| *is_file)
        .map(|(path, _)| path.clone())
        .collect();
    if present_slots.is_empty() {
        SkipKind::GenuinelyAbsent
    } else {
        SkipKind::HarnessBug { present_slots }
    }
}

/// Expected platform slot binary path under `root`.
fn slot_binary_path(root: &Path, binary: &str) -> PathBuf {
    root.join("crates/zfb/binaries/esbuild").join(binary)
}

/// A directory is treated as a workspace root when it has a `Cargo.toml`
/// file and a `crates/` directory — the layout of this workspace.
fn is_workspace_root(dir: &Path) -> bool {
    dir.join("Cargo.toml").is_file() && dir.join("crates").is_dir()
}

fn push_unique(roots: &mut Vec<PathBuf>, root: PathBuf) {
    if !roots.contains(&root) {
        roots.push(root);
    }
}

/// Returns the esbuild binary file name for the current platform.
///
/// `esbuild.exe` on Windows, `esbuild` everywhere else.
fn esbuild_binary_name() -> &'static str {
    if cfg!(windows) {
        "esbuild.exe"
    } else {
        "esbuild"
    }
}

/// Returns the esbuild npm package suffix for the current platform/arch,
/// or `None` for unsupported combinations (causing step 3 to be skipped).
///
/// These suffixes match the `esbuild_platform_meta` table in
/// `crates/zfb/build.rs:238` — keep both in sync if either changes.
fn esbuild_npm_suffix() -> Option<&'static str> {
    if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        Some("linux-x64")
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") {
        Some("linux-arm64")
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
        Some("darwin-x64")
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        Some("darwin-arm64")
    } else if cfg!(target_os = "windows") && cfg!(target_arch = "x86_64") {
        Some("win32-x64")
    } else {
        None
    }
}

/// Expands to `PathBuf::from(env!("CARGO_BIN_EXE_zfb"))` at the call site.
///
/// `CARGO_BIN_EXE_zfb` is set by Cargo only when compiling integration
/// tests for the crate that owns the `zfb` binary (`crates/zfb/`). The
/// macro form is required so that `env!()` expands at the *caller's*
/// compile unit — a plain function call cannot access a test-binary env
/// var from another crate.
///
/// Usage: place `use zfb_test_utils::zfb_binary;` in your test file, then
/// call `zfb_binary!()` to obtain the path to the compiled `zfb` binary.
#[macro_export]
macro_rules! zfb_binary {
    () => {{
        ::std::path::PathBuf::from(env!("CARGO_BIN_EXE_zfb"))
    }};
}
