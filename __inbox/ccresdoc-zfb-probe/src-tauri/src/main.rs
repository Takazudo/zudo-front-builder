//! CCResDoc-on-current-zfb probe (issue #349) — Mode B sidecar.
//!
//! What this probe demonstrates:
//!   - Spawn the current zfb dev server as a Tauri sidecar.
//!   - Wait for `127.0.0.1:<port>` to become reachable.
//!   - The Tauri window then loads the zfb-served page (URL configured
//!     in `tauri.conf.json`'s `app.windows[0].url`).
//!
//! What this probe does NOT do (intentional, see findings doc):
//!   - Bundle the zfb binary as a Tauri external bin under
//!     `bundle.externalBin`. A real ship would resolve the platform-
//!     specific binary from `packages/zfb-<target>/` and add a
//!     `binaries/zfb-<target-triple>` entry per Tauri sidecar conventions.
//!   - Watch `~/.claude/CLAUDE.md` for hot-reload. zfb's watcher only
//!     follows the project's `pages/`, `content/`, etc. — see findings
//!     doc for the watcher-include path.
//!   - Implement Mode D (in-process embed) — deferred pending issue #346.

use std::env;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};

/// Port the zfb dev server binds — matches `claude-doc-site/zfb.config.json`.
const ZFB_DEV_PORT: u16 = 4321;

/// Max wait for the dev server's listening socket to come up.
const READY_TIMEOUT: Duration = Duration::from_secs(20);

/// Locate the zfb binary. Order:
///   1. `ZFB_BIN` env var (used by `cargo run` from this directory).
///   2. PATH lookup of `zfb` (devs with the npm CLI on their PATH).
///
/// A shipped CCResDoc-on-current-zfb would replace this with a Tauri
/// sidecar resolved at `app.path().resolve("binaries/zfb", BaseDirectory::Resource)`.
fn resolve_zfb_bin() -> Result<PathBuf> {
    if let Ok(p) = env::var("ZFB_BIN") {
        return Ok(PathBuf::from(p));
    }
    // Fallback: trust PATH. spawn() will surface ENOENT if not found.
    Ok(PathBuf::from("zfb"))
}

/// Resolve the zfb sample project under this probe directory.
fn resolve_project_dir() -> Result<PathBuf> {
    // CARGO_MANIFEST_DIR is `__inbox/ccresdoc-zfb-probe/src-tauri/`.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let project = manifest
        .parent()
        .ok_or_else(|| anyhow!("manifest has no parent"))?
        .join("claude-doc-site");
    Ok(project)
}

/// Spawn `zfb dev` as a child process. The returned `Child` is held by
/// the Tauri managed state so it's killed on app exit (handled via the
/// `run_event` callback below).
fn spawn_zfb_dev() -> Result<Child> {
    let bin = resolve_zfb_bin()?;
    let project = resolve_project_dir()?;
    Command::new(&bin)
        .arg("dev")
        .arg("--port")
        .arg(ZFB_DEV_PORT.to_string())
        .current_dir(&project)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawning zfb dev from {}", bin.display()))
}

/// Block until the zfb dev server accepts a TCP connection on the
/// configured port, or `READY_TIMEOUT` elapses.
fn wait_for_dev_server() -> Result<()> {
    let addr = format!("127.0.0.1:{ZFB_DEV_PORT}");
    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(
            &addr.parse().with_context(|| format!("parsing {addr}"))?,
            Duration::from_millis(200),
        )
        .is_ok()
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(200));
    }
    Err(anyhow!(
        "zfb dev server did not come up on {addr} within {:?}",
        READY_TIMEOUT
    ))
}

/// Tauri-managed handle to the zfb dev child. Drop kills the process so
/// closing the window cleans up the sidecar.
struct ZfbServer(Child);

impl Drop for ZfbServer {
    fn drop(&mut self) {
        // Best-effort. If the child already exited, kill() returns Err
        // we don't surface — this is a probe, not a production daemon.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn main() -> Result<()> {
    let child = spawn_zfb_dev()?;
    let server = ZfbServer(child);

    // Block on readiness BEFORE the Tauri window opens. A real app
    // would do this in `tauri::Builder::setup` async to keep the UI
    // responsive (splash screen, etc.). For the probe, blocking is fine.
    wait_for_dev_server().context("waiting for zfb dev")?;

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(server)
        .run(tauri::generate_context!())
        .map_err(|e| anyhow!("tauri run failed: {e}"))
}
