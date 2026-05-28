mod html_normalize;
pub use html_normalize::normalize_html;

use std::path::PathBuf;

/// Locate an esbuild binary suitable for integration tests.
///
/// Resolution order (union of all existing 17+ per-test copies):
///
/// 1. `ZFB_ESBUILD_BIN` env var — if set and points to an existing file,
///    return it immediately.
/// 2. Workspace-local slot:
///    `<workspace_root>/crates/zfb/binaries/esbuild/<binary>`.
///    Binary name is `esbuild` on Unix, `esbuild.exe` on Windows.
///    Workspace root is two parents up from `CARGO_MANIFEST_DIR`
///    (i.e. `crates/zfb-test-utils` → `crates` → repo root).
/// 3. Workspace-root pnpm nested store:
///    `node_modules/.pnpm/*/node_modules/@esbuild/<suffix>/bin/<binary>`
///    where `*` is each directory entry under `.pnpm` (pnpm's content-
///    addressed layout). Mirrors the shape in
///    `crates/zfb/tests/css_modules_components_build.rs:166-176`.
///    Note: the pnpm Windows package puts the binary at the package root
///    rather than under `bin/`; this path uses `bin/` uniformly following
///    the spec — a future Windows fix can adjust the suffix path if needed.
/// 4. Workspace-root flat pnpm store:
///    `node_modules/.pnpm/node_modules/esbuild/bin/<binary>` — kept for
///    parity with `crates/zfb-build/tests/embedded_v8_snapshot_e2e.rs`.
/// 5. Portable PATH walk — iterates `$PATH` entries via
///    `std::env::split_paths` (platform-aware `:` vs `;` split) and
///    checks `<entry>/<binary>`. Does NOT shell out to `which`, which is
///    absent on Windows.
///
/// Returns `None` if no candidate exists. Callers should gate with
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
    // Workspace root: two parents up from this crate's manifest directory.
    // CARGO_MANIFEST_DIR = <repo>/crates/zfb-test-utils → parent = <repo>/crates → parent = <repo>
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf());

    if let Some(ref workspace) = workspace {
        // Step 2: workspace-local binary slot.
        let slot = workspace
            .join("crates/zfb/binaries/esbuild")
            .join(binary);
        if slot.is_file() {
            return Some(slot);
        }

        // Step 3: pnpm nested store — node_modules/.pnpm/*/node_modules/@esbuild/<suffix>/bin/<binary>.
        if let Some(suffix) = esbuild_npm_suffix() {
            let pnpm_dir = workspace.join("node_modules/.pnpm");
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
        let flat_slot = workspace
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

    None
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
