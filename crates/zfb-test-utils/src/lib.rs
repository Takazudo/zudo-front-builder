mod test_tasks;
pub use test_tasks::{with_test_tasks, TestTasks};
mod http_probe;
pub use http_probe::{http_get_before, poll_before, HttpBodyReadFailure};
mod cross_binary_lock;
mod html_normalize;
mod module_entry_probe;
mod sse;
mod sse_client;
mod test_phase;
mod watcher_handshake;
pub use cross_binary_lock::CrossBinaryE2eLock;
pub use html_normalize::normalize_html;
pub use module_entry_probe::{module_entry_urls, probe_module_entries, ModuleEntryProbe};
pub use sse::{
    assert_frame_has_data, decode_utf8_incremental, next_sse_event_name, next_sse_frame,
    wait_for_subscribers, wait_for_subscribers_polled, SseFrame,
};
pub use sse_client::{open_sse, sse_client};
pub use test_phase::TestPhase;
pub use watcher_handshake::{watcher_live_handshake, HandshakeOpts, HandshakeResult};

use std::path::{Path, PathBuf};

/// Candidate workspace roots, highest priority first, deduplicated:
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
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(root) = manifest_dir.parent().and_then(Path::parent) {
        push_unique(&mut roots, root.to_path_buf());
    }

    roots
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
