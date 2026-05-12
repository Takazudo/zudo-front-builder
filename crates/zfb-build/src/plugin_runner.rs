//! Plugin host subprocess + lifecycle hook dispatcher (Sub 3 / #108,
//! extended in Astro-migration epic #253 / #255 with the new `setup`
//! hook).
//!
//! Owns one long-lived `node crates/zfb/js/plugin-host.mjs` process for
//! the lifetime of a build (or dev session) and dispatches the four
//! lifecycle hooks — `setup`, `preBuild`, `postBuild`, `devMiddleware` —
//! over a newline-delimited JSON stdio protocol.
//!
//! `setup` is the newest addition (#255). It runs once per host boot,
//! before `preBuild`, and lets a plugin register virtual modules,
//! aliases, and dev-time injected routes that Wave 2 consumers
//! (#260 V8 host resolver, #261 islands esbuild resolver, dev server
//! injected-route handler) read from the three [`SetupRegistries`]
//! returned by [`PluginHost::run_setup`].
//!
//! ## Why a long-lived subprocess
//!
//! - **Build hooks**: `preBuild` and `postBuild` each fire once per
//!   `zfb build`, but a `zfb dev` session may issue many builds on
//!   watcher ticks. Spawning Node + re-importing every plugin module
//!   per tick would dominate the dev loop's latency budget.
//! - **`devMiddleware`**: handlers are *registered* once at boot and
//!   dispatched per HTTP request. We want one process holding the
//!   handler closures for the lifetime of the session — handlers must
//!   not be re-imported per request.
//!
//! One process serves both purposes; the protocol multiplexes the
//! kinds.
//!
//! ## Protocol summary
//!
//! Each line of stdin is a JSON request with a `kind` discriminator and
//! a numeric `id`; each line of stdout is either a reply keyed by `id`
//! (`{ ok: true | false, ... }`) or a logger passthrough
//! (`{ log: { level, plugin, message } }`). See `plugin-host.mjs` for
//! the exhaustive shape; the Rust side uses [`HostRequest`] and
//! [`HostReply`] to model the round-trip.
//!
//! ## Error model
//!
//! When a plugin throws inside a hook, the JS host responds with
//! `{ ok: false, error: { plugin, hook, message } }`. We surface that
//! as a [`PluginError`] that carries all three fields verbatim. The
//! build orchestrator turns it into a context-bearing `anyhow::Error`
//! that points at the offending plugin.
//!
//! ## Lifecycle
//!
//! - [`PluginHost::spawn`] launches Node and waits for the `init` reply
//!   (which loads every plugin module via dynamic `import()`). Failure
//!   to load any module produces a [`PluginError`] tagged with the
//!   `init` hook.
//! - Build hooks: [`PluginHost::run_pre_build`] and
//!   [`PluginHost::run_post_build`] are sync-friendly wrappers that
//!   block on a single command/response round-trip (this matches the
//!   build orchestrator's call style).
//! - Dev middleware: [`PluginHost::register_dev_middlewares`] returns
//!   the list of `(path, handler_id)` pairs the dev server should
//!   route into the plugin host. [`PluginHost::invoke_dev_handler`]
//!   dispatches one HTTP request to a registered handler.
//! - [`PluginHost::shutdown`] sends a `shutdown` command and joins the
//!   child. Drop also kills the process — the explicit shutdown is the
//!   graceful path.
//!
//! The host writes logger calls out-of-band (no `id`) and the Rust
//! reader forwards them into [`tracing`] at the matching level so the
//! plugin's log lines blend with the rest of the build output.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};
use tracing::{debug, error, info, warn};

use crate::plugin_registries::{
    PluginSetupAccumulator, RawPluginSetupOutput, RawSetupRegistration, SetupCommand,
    SetupRegistries, VirtualLoaderId,
};

/// JS payload that the Rust side stages into a tempfile and runs via
/// `node`. Embedded so we don't have to ship a sidecar at runtime.
const PLUGIN_HOST_MJS: &str = include_str!("../../zfb/js/plugin-host.mjs");

/// One declared plugin in the loaded `Config`. Carries only the data
/// the JS host needs at `init` time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSpec {
    /// Display name (used for diagnostics — the plugin module's own
    /// `name` field wins on the JS side if present).
    pub name: String,
    /// Absolute module specifier (file URL) the host hands to
    /// dynamic `import()`.
    pub module: String,
    /// User-supplied options block from `zfb.config.ts`.
    #[serde(default)]
    pub options: serde_json::Value,
}

/// Build-hook context handed to `preBuild` / `postBuild` plugin
/// callbacks. The same shape works for both hooks; only the lifecycle
/// timing differs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildHookContext {
    pub project_root: PathBuf,
    pub out_dir: PathBuf,
    /// Full loaded config as JSON (the JS side already saw an earlier
    /// view, but we re-send it so plugins can cheaply read it without
    /// having to keep state across hook calls).
    pub config: serde_json::Value,
}

/// Dev-middleware registration context. Sent at boot so each plugin
/// can call `ctx.register(path, handler)` once.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevRegisterContext {
    pub project_root: PathBuf,
    pub config: serde_json::Value,
}

/// Context handed to the new `setup` hook (#255). Carries the active
/// command so a plugin can `if (ctx.command === "dev")` to gate
/// dev-only registrations. The JS side decorates this with three
/// methods — `injectRoute`, `addVirtualModule`, `addAlias` — that
/// accumulate raw registrations the Rust accumulator then folds into
/// the three [`SetupRegistries`].
///
/// Mirrors the existing `BuildHookContext` shape (per-plugin
/// `options` + `logger` are added on the JS side from each plugin's
/// own metadata).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupHookContext {
    pub project_root: PathBuf,
    /// `"build"` or `"dev"` — string form matches the `command` field
    /// the JS-side `SetupContext` exposes.
    pub command: String,
    pub config: serde_json::Value,
}

/// One HTTP request shipped over the wire to a registered plugin
/// dev-middleware handler.
#[derive(Debug, Clone, Serialize)]
pub struct DevRequest {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// One HTTP response sent back from a plugin dev-middleware handler.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevResponse {
    /// `true` when the handler returned `undefined` — the dev server
    /// should fall through to its built-in routes.
    #[serde(default)]
    pub passthrough: bool,
    #[serde(default)]
    pub status: u16,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub body: String,
    /// `"utf8"` (default) or `"base64"`.
    #[serde(default = "default_body_encoding")]
    pub body_encoding: String,
}

fn default_body_encoding() -> String {
    "utf8".to_string()
}

/// One `(path, handlerId)` pair returned by `devRegister`.
#[derive(Debug, Clone, Deserialize)]
pub struct DevRegistration {
    pub path: String,
    #[serde(rename = "handlerId")]
    pub handler_id: String,
    pub plugin: String,
}

/// Error surfaced when a plugin hook throws on the JS side.
///
/// Carries the plugin display name, the hook that threw (`preBuild`,
/// `postBuild`, `devMiddleware`, or `init`), and the raw message
/// (`error.stack` when available, else `error.message`). Build
/// orchestration converts this into a context-bearing `anyhow::Error`
/// so the user sees both the plugin and the hook in the failure
/// banner.
#[derive(Debug, thiserror::Error)]
#[error("plugin `{plugin}` failed in hook `{hook}`: {message}")]
pub struct PluginError {
    pub plugin: String,
    pub hook: String,
    pub message: String,
}

/// Long-lived plugin host. Cheap to clone (the heavy state lives
/// behind an `Arc<Mutex<...>>`) so the dev server's request handler
/// can hold a copy without contending with the build loop.
#[derive(Clone)]
pub struct PluginHost {
    inner: Arc<HostInner>,
}

struct HostInner {
    /// Pending in-flight requests keyed by id. The reader task pops
    /// the matching sender when a reply arrives.
    pending: Mutex<HashMap<u64, oneshot::Sender<HostReply>>>,
    /// stdin handle protected by a mutex so concurrent senders
    /// serialise their writes (line-delimited JSON cannot interleave).
    stdin: Mutex<ChildStdin>,
    /// Monotonic id counter.
    next_id: AtomicU64,
    /// Child process handle. Held for the lifetime of the host so
    /// dropping the [`PluginHost`] kills the subprocess.
    child: Mutex<Option<Child>>,
    /// Dropping the [`_tempdir`] removes the staged plugin-host script.
    _tempdir: tempfile::TempDir,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum HostLine {
    /// Reply to a previous request — keyed by `id`.
    Reply(HostReply),
    /// Log passthrough — no `id`, no reply needed.
    Log(LogLine),
}

#[derive(Debug, Deserialize)]
struct HostReply {
    id: u64,
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    result: serde_json::Value,
    #[serde(default)]
    error: Option<HostErrorPayload>,
}

#[derive(Debug, Deserialize)]
struct HostErrorPayload {
    plugin: String,
    hook: String,
    message: String,
}

#[derive(Debug, Deserialize)]
struct LogLine {
    log: LogPayload,
}

#[derive(Debug, Deserialize)]
struct LogPayload {
    level: String,
    plugin: String,
    message: String,
}

impl PluginHost {
    /// Spawn the plugin-host subprocess and load every plugin module
    /// via dynamic `import()`. Returns a handle that can dispatch
    /// hooks and dev-middleware requests.
    ///
    /// `node_binary` defaults to `"node"` from `PATH` when `None`.
    /// `plugins` is the resolved set from `Config.plugins[]` —
    /// entries with `resolved_module = None` are skipped (they cannot
    /// be loaded; the JSON config path produces `None` and we treat
    /// that as a no-plugin build).
    pub async fn spawn(
        plugins: Vec<PluginSpec>,
        node_binary: Option<OsString>,
    ) -> Result<Self> {
        let tmp = tempfile::Builder::new()
            .prefix("zfb-plugin-host-")
            .tempdir()
            .context("plugin host: failed to allocate tempdir")?;
        let host_path = tmp.path().join("plugin-host.mjs");
        tokio::fs::write(&host_path, PLUGIN_HOST_MJS)
            .await
            .context("plugin host: failed to stage plugin-host.mjs")?;

        let node_bin = node_binary.unwrap_or_else(|| OsString::from("node"));
        let mut child = Command::new(&node_bin)
            .arg(&host_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| {
                format!(
                    "plugin host: failed to spawn `{}` plugin-host.mjs",
                    node_bin.to_string_lossy()
                )
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("plugin host: child stdin missing after spawn"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("plugin host: child stdout missing after spawn"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("plugin host: child stderr missing after spawn"))?;

        let inner = Arc::new(HostInner {
            pending: Mutex::new(HashMap::new()),
            stdin: Mutex::new(stdin),
            next_id: AtomicU64::new(1),
            child: Mutex::new(Some(child)),
            _tempdir: tmp,
        });

        // Reader task — drains stdout, dispatches replies to the
        // matching pending sender, and forwards log lines into tracing.
        let inner_for_reader = Arc::clone(&inner);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        Self::handle_line(&inner_for_reader, &line).await;
                    }
                    Ok(None) => {
                        debug!("plugin host: stdout closed");
                        break;
                    }
                    Err(e) => {
                        warn!(error = %e, "plugin host: stdout read error");
                        break;
                    }
                }
            }
            // Wake every still-pending caller with a synthetic close
            // error so we don't deadlock on a child that died early.
            let mut pend = inner_for_reader.pending.lock().await;
            for (id, tx) in pend.drain() {
                let _ = tx.send(HostReply {
                    id,
                    ok: false,
                    result: serde_json::Value::Null,
                    error: Some(HostErrorPayload {
                        plugin: "(host)".into(),
                        hook: "(none)".into(),
                        message: "plugin host stdout closed before reply".into(),
                    }),
                });
            }
        });

        // Stderr drain — the host should never write to stderr in
        // practice, but a programmer error there shouldn't deadlock
        // on a full pipe buffer.
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    warn!(target: "zfb_plugin", "{line}");
                }
            }
        });

        let host = PluginHost { inner };

        // Send `init` and wait for the loaded-count reply. Surface a
        // module-load failure as `PluginError`.
        let result: serde_json::Value = host
            .request_typed(
                "init",
                serde_json::json!({
                    "plugins": plugins,
                }),
            )
            .await?;
        debug!(loaded = ?result, "plugin host: init complete");

        Ok(host)
    }

    /// Run every plugin's `preBuild` hook (in declaration order).
    pub async fn run_pre_build(&self, ctx: &BuildHookContext) -> Result<()> {
        let _ = self
            .request_typed::<serde_json::Value>(
                "preBuild",
                serde_json::json!({ "ctx": ctx }),
            )
            .await?;
        Ok(())
    }

    /// Run every plugin's `postBuild` hook (in declaration order).
    pub async fn run_post_build(&self, ctx: &BuildHookContext) -> Result<()> {
        let _ = self
            .request_typed::<serde_json::Value>(
                "postBuild",
                serde_json::json!({ "ctx": ctx }),
            )
            .await?;
        Ok(())
    }

    /// Run every plugin's new `setup` hook (in declaration order) and
    /// collect the three accumulated registries (#255).
    ///
    /// The JS host collects every `ctx.addAlias` / `ctx.addVirtualModule` /
    /// `ctx.injectRoute` call per plugin and returns the batched
    /// registrations in the reply. Conflict detection runs in Rust
    /// against the canonical types so any violation surfaces with the
    /// offending pair named.
    ///
    /// `command` controls the `ctx.command` string visible to plugins
    /// AND the `injectRoute` guard — calling `injectRoute` from a
    /// `Build` invocation raises `InjectRouteInBuildMode`.
    pub async fn run_setup(
        &self,
        project_root: &std::path::Path,
        command: SetupCommand,
        config: &serde_json::Value,
    ) -> Result<SetupRegistries> {
        // Wire shape returned by the host: one entry per plugin, in
        // declaration order, each carrying that plugin's batched raw
        // registrations.
        #[derive(Deserialize)]
        struct WireOutput {
            plugin: String,
            registrations: Vec<WireRegistration>,
        }

        #[derive(Deserialize)]
        #[serde(tag = "kind")]
        enum WireRegistration {
            #[serde(rename = "alias")]
            Alias { from: String, to: String },
            #[serde(rename = "virtualModule")]
            VirtualModule {
                specifier: String,
                #[serde(rename = "loaderId")]
                loader_id: String,
            },
            #[serde(rename = "injectRoute")]
            InjectRoute {
                pattern: String,
                entrypoint: String,
            },
        }

        #[derive(Deserialize)]
        struct WireReply {
            outputs: Vec<WireOutput>,
        }

        let ctx = SetupHookContext {
            project_root: project_root.to_path_buf(),
            command: command.as_str().to_string(),
            config: config.clone(),
        };
        let reply: WireReply = self
            .request_typed("setup", serde_json::json!({ "ctx": ctx }))
            .await?;

        let mut acc = PluginSetupAccumulator::new(project_root, command);
        for output in reply.outputs {
            let raws: Vec<RawSetupRegistration> = output
                .registrations
                .into_iter()
                .map(|r| match r {
                    WireRegistration::Alias { from, to } => {
                        RawSetupRegistration::Alias { from, to }
                    }
                    WireRegistration::VirtualModule {
                        specifier,
                        loader_id,
                    } => RawSetupRegistration::VirtualModule {
                        specifier,
                        loader_id: VirtualLoaderId(loader_id),
                    },
                    WireRegistration::InjectRoute {
                        pattern,
                        entrypoint,
                    } => RawSetupRegistration::InjectRoute {
                        pattern,
                        entrypoint,
                    },
                })
                .collect();
            acc.ingest(RawPluginSetupOutput {
                plugin: output.plugin,
                registrations: raws,
            })
            .map_err(anyhow::Error::from)?;
        }
        let (aliases, virtual_modules, injected_routes) = acc.finish();
        Ok(SetupRegistries {
            aliases,
            virtual_modules,
            injected_routes,
        })
    }

    /// Invoke a previously-registered virtual-module loader by its
    /// opaque [`VirtualLoaderId`]. The JS host runs the loader (once,
    /// cached) and returns the produced ESM module source as a string.
    ///
    /// Wave 2 consumers (#260 V8 host resolver, #261 islands esbuild
    /// resolver) call this from their respective module-resolution
    /// paths when an import specifier matches a registered
    /// virtual-module entry.
    pub async fn invoke_virtual_loader(
        &self,
        loader_id: &VirtualLoaderId,
    ) -> Result<String> {
        #[derive(Deserialize)]
        struct Reply {
            source: String,
        }
        let reply: Reply = self
            .request_typed(
                "virtualLoad",
                serde_json::json!({ "loaderId": loader_id.as_str() }),
            )
            .await?;
        Ok(reply.source)
    }

    /// Call `devMiddleware(ctx)` on every plugin and collect the
    /// `(path, handlerId)` registrations the dev server should route.
    pub async fn register_dev_middlewares(
        &self,
        ctx: &DevRegisterContext,
    ) -> Result<Vec<DevRegistration>> {
        #[derive(Deserialize)]
        struct Reply {
            registrations: Vec<DevRegistration>,
        }
        let reply: Reply = self
            .request_typed(
                "devRegister",
                serde_json::json!({ "ctx": ctx }),
            )
            .await?;
        Ok(reply.registrations)
    }

    /// Invoke a previously-registered dev-middleware handler.
    pub async fn invoke_dev_handler(
        &self,
        handler_id: &str,
        request: DevRequest,
    ) -> Result<DevResponse> {
        let resp: DevResponse = self
            .request_typed(
                "devInvoke",
                serde_json::json!({
                    "handlerId": handler_id,
                    "request": request,
                }),
            )
            .await?;
        Ok(resp)
    }

    /// Send a `shutdown` command and wait for the child to exit.
    /// Best-effort: if the child has already died, this returns Ok.
    pub async fn shutdown(self) -> Result<()> {
        // Send the shutdown — ignore any error (the child may have
        // already exited). The reply is the loaded "bye" object.
        let _ = self
            .request_typed::<serde_json::Value>("shutdown", serde_json::json!({}))
            .await;
        let mut guard = self.inner.child.lock().await;
        if let Some(mut child) = guard.take() {
            // Give the child a moment to exit cleanly; otherwise force-kill.
            match tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await {
                Ok(Ok(status)) => {
                    debug!(?status, "plugin host: child exited");
                }
                Ok(Err(e)) => warn!(error = %e, "plugin host: wait failed"),
                Err(_) => {
                    let _ = child.kill().await;
                }
            }
        }
        Ok(())
    }

    async fn request_typed<T: serde::de::DeserializeOwned>(
        &self,
        kind: &str,
        body: serde_json::Value,
    ) -> Result<T> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut pend = self.inner.pending.lock().await;
            pend.insert(id, tx);
        }

        let mut envelope = serde_json::json!({
            "id": id,
            "kind": kind,
        });
        if let serde_json::Value::Object(extras) = body {
            if let serde_json::Value::Object(env) = &mut envelope {
                for (k, v) in extras {
                    env.insert(k, v);
                }
            }
        }
        let mut line = serde_json::to_string(&envelope)
            .context("plugin host: serialise request")?;
        line.push('\n');
        let write_outcome: Result<()> = {
            let mut stdin = self.inner.stdin.lock().await;
            let r = stdin
                .write_all(line.as_bytes())
                .await
                .context("plugin host: write request");
            stdin.flush().await.ok();
            r
        };
        // If the stdin write fails the host will never produce a reply
        // for `id`, so we must evict the `pending` entry inserted
        // above. Without this cleanup the map grows unboundedly across
        // transient hiccups (broken pipe, full buffer, etc.).
        if let Err(e) = write_outcome {
            let mut pend = self.inner.pending.lock().await;
            pend.remove(&id);
            return Err(e);
        }

        let reply = rx
            .await
            .map_err(|_| anyhow!("plugin host: reply channel dropped"))?;
        if !reply.ok {
            let payload = reply.error.unwrap_or(HostErrorPayload {
                plugin: "(host)".into(),
                hook: "(none)".into(),
                message: "plugin host returned ok=false with no error payload".into(),
            });
            let err = PluginError {
                plugin: payload.plugin,
                hook: payload.hook,
                message: payload.message,
            };
            return Err(anyhow::Error::new(err));
        }
        let typed: T = serde_json::from_value(reply.result)
            .context("plugin host: deserialise reply.result")?;
        Ok(typed)
    }

    async fn handle_line(inner: &Arc<HostInner>, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        let parsed: HostLine = match serde_json::from_str(line) {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, raw = %line, "plugin host: failed to parse stdout line");
                return;
            }
        };
        match parsed {
            HostLine::Log(LogLine { log }) => {
                match log.level.as_str() {
                    "warn" => {
                        warn!(target: "zfb_plugin", plugin = %log.plugin, "{}", log.message);
                    }
                    "error" => {
                        error!(target: "zfb_plugin", plugin = %log.plugin, "{}", log.message);
                    }
                    _ => {
                        info!(target: "zfb_plugin", plugin = %log.plugin, "{}", log.message);
                    }
                }
            }
            HostLine::Reply(reply) => {
                let id = reply.id;
                let mut pend = inner.pending.lock().await;
                if let Some(tx) = pend.remove(&id) {
                    let _ = tx.send(reply);
                } else {
                    warn!(id, "plugin host: received reply for unknown id");
                }
            }
        }
    }
}

/// Borrow a [`PluginError`] out of an [`anyhow::Error`] when present.
/// The build orchestrator uses this to wrap a `preBuild` failure with
/// the offending plugin + hook in its diagnostic.
pub fn extract_plugin_error(err: &anyhow::Error) -> Option<&PluginError> {
    err.downcast_ref::<PluginError>()
}

/// Convert an [`anyhow::Error`] into the same error with extra context
/// that names the plugin + hook when it carries a [`PluginError`].
pub fn annotate_with_plugin_error(err: anyhow::Error) -> anyhow::Error {
    if let Some(pe) = extract_plugin_error(&err) {
        let label = format!(
            "plugin lifecycle hook failed: plugin=`{}` hook=`{}`",
            pe.plugin, pe.hook,
        );
        return err.context(label);
    }
    err
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn file_url_for_test(p: &Path) -> String {
        // Minimal `file://` URL for absolute paths we control in
        // tests. Avoids pulling in the `url` crate as a dep.
        let abs = p.to_string_lossy().to_string();
        // Linux/macOS-only test infrastructure — Windows path
        // serialisation would need a different shape.
        format!("file://{abs}")
    }

    fn host_node_available() -> bool {
        // Best-effort detection — we can't run the full host without a
        // real `node` binary on PATH. CI may or may not have one.
        std::process::Command::new("node")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn host_spawns_and_shuts_down_with_zero_plugins() {
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let host = PluginHost::spawn(Vec::new(), None)
            .await
            .expect("host spawns");
        host.shutdown().await.expect("host shuts down cleanly");
    }

    #[tokio::test]
    async fn host_runs_pre_and_post_build_for_a_local_plugin() {
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("plugin.mjs");
        let marker_pre = tmp.path().join("pre-marker.txt");
        let marker_post = tmp.path().join("post-marker.txt");
        // Plugin module that touches a marker file in each hook so
        // the test can confirm the hook actually fired.
        let plugin_src = format!(
            r#"
            import {{ writeFileSync }} from "node:fs";
            export default {{
              name: "marker-plugin",
              preBuild(ctx) {{
                writeFileSync({pre:?}, ctx.outDir);
              }},
              postBuild(ctx) {{
                writeFileSync({post:?}, ctx.outDir);
              }},
            }};
            "#,
            pre = marker_pre.to_string_lossy().to_string(),
            post = marker_post.to_string_lossy().to_string(),
        );
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();

        let module_url = file_url_for_test(&plugin_path);
        let host = PluginHost::spawn(
            vec![PluginSpec {
                name: "marker-plugin".into(),
                module: module_url,
                options: serde_json::json!({}),
            }],
            None,
        )
        .await
        .expect("host spawns");

        let ctx = BuildHookContext {
            project_root: tmp.path().to_path_buf(),
            out_dir: tmp.path().join("dist"),
            config: serde_json::json!({}),
        };
        host.run_pre_build(&ctx).await.expect("preBuild ok");
        host.run_post_build(&ctx).await.expect("postBuild ok");
        host.shutdown().await.expect("shutdown ok");

        let pre = tokio::fs::read_to_string(&marker_pre).await.unwrap();
        let post = tokio::fs::read_to_string(&marker_post).await.unwrap();
        assert!(pre.ends_with("dist"), "pre marker: {pre}");
        assert!(post.ends_with("dist"), "post marker: {post}");
    }

    #[tokio::test]
    async fn host_propagates_plugin_throws_as_plugin_error() {
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("throwing.mjs");
        tokio::fs::write(
            &plugin_path,
            r#"
            export default {
              name: "thrower",
              preBuild() {
                throw new Error("boom from preBuild");
              },
            };
            "#,
        )
        .await
        .unwrap();
        let module_url = file_url_for_test(&plugin_path);
        let host = PluginHost::spawn(
            vec![PluginSpec {
                name: "thrower".into(),
                module: module_url,
                options: serde_json::json!({}),
            }],
            None,
        )
        .await
        .expect("host spawns");
        let ctx = BuildHookContext {
            project_root: tmp.path().to_path_buf(),
            out_dir: tmp.path().join("dist"),
            config: serde_json::json!({}),
        };
        let err = host
            .run_pre_build(&ctx)
            .await
            .expect_err("preBuild should propagate the throw");
        let pe = extract_plugin_error(&err).expect("PluginError carried in chain");
        assert_eq!(pe.plugin, "thrower");
        assert_eq!(pe.hook, "preBuild");
        assert!(pe.message.contains("boom from preBuild"), "msg: {}", pe.message);
        host.shutdown().await.ok();
    }

    #[tokio::test]
    async fn host_runs_all_three_hooks_for_one_plugin() {
        // Spec acceptance criterion (Sub 3 / #108): "integration test
        // that registers a plugin with all 3 hooks against a small
        // fixture project and asserts each fires". A single plugin
        // implementing all three hooks goes through `init`, both
        // build hooks, and one dev-middleware round-trip — confirming
        // the host multiplexes the kinds correctly on one process.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("triple.mjs");
        let log_path = tmp.path().join("triple.log");
        let log_str = log_path.to_string_lossy().to_string();
        // Using `appendFileSync` so the order of hook firings is
        // visible in the log file.
        let plugin_src = format!(
            r#"
            import {{ appendFileSync }} from "node:fs";
            const log = (line) => appendFileSync({log:?}, line + "\n");
            export default {{
              name: "triple",
              preBuild() {{ log("preBuild"); }},
              postBuild() {{ log("postBuild"); }},
              devMiddleware({{ register }}) {{
                register("/triple", () => ({{ status: 200, body: "triple-ok" }}));
              }},
            }};
            "#,
            log = log_str,
        );
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();
        let module_url = file_url_for_test(&plugin_path);
        let host = PluginHost::spawn(
            vec![PluginSpec {
                name: "triple".into(),
                module: module_url,
                options: serde_json::json!({}),
            }],
            None,
        )
        .await
        .expect("host spawns");
        let bctx = BuildHookContext {
            project_root: tmp.path().to_path_buf(),
            out_dir: tmp.path().join("dist"),
            config: serde_json::json!({}),
        };
        host.run_pre_build(&bctx).await.expect("preBuild ok");
        let regs = host
            .register_dev_middlewares(&DevRegisterContext {
                project_root: tmp.path().to_path_buf(),
                config: serde_json::json!({}),
            })
            .await
            .expect("register ok");
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].path, "/triple");
        let resp = host
            .invoke_dev_handler(
                &regs[0].handler_id,
                DevRequest {
                    method: "GET".into(),
                    url: "/triple".into(),
                    headers: HashMap::new(),
                    body: None,
                },
            )
            .await
            .expect("invoke ok");
        assert_eq!(resp.body, "triple-ok");
        host.run_post_build(&bctx).await.expect("postBuild ok");
        host.shutdown().await.ok();
        let log = tokio::fs::read_to_string(&log_path).await.unwrap();
        assert!(log.contains("preBuild"));
        assert!(log.contains("postBuild"));
        // preBuild must have fired before postBuild.
        let pre_ix = log.find("preBuild").unwrap();
        let post_ix = log.find("postBuild").unwrap();
        assert!(pre_ix < post_ix, "preBuild must precede postBuild: {log}");
    }

    #[tokio::test]
    async fn host_dev_middleware_register_and_invoke_round_trip() {
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("middleware.mjs");
        tokio::fs::write(
            &plugin_path,
            r#"
            export default {
              name: "echo-mw",
              devMiddleware({ register }) {
                register("/echo", (req) => ({
                  status: 200,
                  headers: { "x-method": req.method },
                  body: `hello ${req.url}`,
                }));
              },
            };
            "#,
        )
        .await
        .unwrap();
        let module_url = file_url_for_test(&plugin_path);
        let host = PluginHost::spawn(
            vec![PluginSpec {
                name: "echo-mw".into(),
                module: module_url,
                options: serde_json::json!({}),
            }],
            None,
        )
        .await
        .expect("host spawns");

        let regs = host
            .register_dev_middlewares(&DevRegisterContext {
                project_root: tmp.path().to_path_buf(),
                config: serde_json::json!({}),
            })
            .await
            .expect("register ok");
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].path, "/echo");

        let resp = host
            .invoke_dev_handler(
                &regs[0].handler_id,
                DevRequest {
                    method: "GET".into(),
                    url: "/echo?x=1".into(),
                    headers: HashMap::new(),
                    body: None,
                },
            )
            .await
            .expect("invoke ok");
        assert!(!resp.passthrough);
        assert_eq!(resp.status, 200);
        assert_eq!(resp.headers.get("x-method").map(|s| s.as_str()), Some("GET"));
        assert_eq!(resp.body, "hello /echo?x=1");

        host.shutdown().await.ok();
    }

    // --- #255 setup-hook integration tests ----------------------------------

    #[tokio::test]
    async fn setup_hook_collects_alias_virtual_module_and_inject_route_in_dev() {
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("setupper.mjs");
        // One plugin exercises all three setup-ctx methods, plus the
        // dev-only injectRoute path.
        tokio::fs::write(
            &plugin_path,
            r#"
            export default {
              name: "setupper",
              setup({ addAlias, addVirtualModule, injectRoute, command }) {
                if (command !== "dev") {
                  throw new Error("expected dev, got " + command);
                }
                addAlias("@/foo", "./src/foo.tsx");
                addVirtualModule("virtual:data", () => 'export default 42');
                injectRoute("/dev/x", "./scripts/x.ts");
              },
            };
            "#,
        )
        .await
        .unwrap();
        let module_url = file_url_for_test(&plugin_path);
        let host = PluginHost::spawn(
            vec![PluginSpec {
                name: "setupper".into(),
                module: module_url,
                options: serde_json::json!({}),
            }],
            None,
        )
        .await
        .expect("host spawns");
        let regs = host
            .run_setup(
                tmp.path(),
                crate::plugin_registries::SetupCommand::Dev,
                &serde_json::json!({}),
            )
            .await
            .expect("setup ok");
        assert_eq!(regs.aliases.len(), 1);
        let alias = regs.aliases.get("@/foo").expect("alias registered");
        assert_eq!(alias.plugin, "setupper");
        assert_eq!(alias.target, tmp.path().join("src/foo.tsx"));
        assert_eq!(regs.virtual_modules.len(), 1);
        let vm = regs
            .virtual_modules
            .get("virtual:data")
            .expect("vm registered");
        assert_eq!(vm.plugin, "setupper");
        // Invoke the loader and confirm the cached source is returned
        // both times.
        let first = host.invoke_virtual_loader(&vm.loader_id).await.unwrap();
        assert_eq!(first, "export default 42");
        let second = host.invoke_virtual_loader(&vm.loader_id).await.unwrap();
        assert_eq!(second, first);
        assert_eq!(regs.injected_routes.len(), 1);
        let r = &regs.injected_routes.as_slice()[0];
        assert_eq!(r.pattern, "/dev/x");
        assert_eq!(r.entrypoint, tmp.path().join("scripts/x.ts"));
        host.shutdown().await.ok();
    }

    #[tokio::test]
    async fn setup_hook_rejects_inject_route_in_build_mode() {
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("builder.mjs");
        tokio::fs::write(
            &plugin_path,
            r#"
            export default {
              name: "builder",
              setup({ injectRoute }) {
                injectRoute("/dev/x", "./scripts/x.ts");
              },
            };
            "#,
        )
        .await
        .unwrap();
        let host = PluginHost::spawn(
            vec![PluginSpec {
                name: "builder".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
        )
        .await
        .expect("host spawns");
        let err = host
            .run_setup(
                tmp.path(),
                crate::plugin_registries::SetupCommand::Build,
                &serde_json::json!({}),
            )
            .await
            .expect_err("injectRoute during build must error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("injectRoute") && msg.contains("dev-only"),
            "expected InjectRouteInBuildMode-style error, got: {msg}",
        );
        host.shutdown().await.ok();
    }

    #[tokio::test]
    async fn setup_hook_detects_alias_conflicts_across_plugins() {
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_a = tmp.path().join("a.mjs");
        let plugin_b = tmp.path().join("b.mjs");
        tokio::fs::write(
            &plugin_a,
            r#"
            export default {
              name: "a",
              setup({ addAlias }) { addAlias("@/x", "./a.tsx"); },
            };
            "#,
        )
        .await
        .unwrap();
        tokio::fs::write(
            &plugin_b,
            r#"
            export default {
              name: "b",
              setup({ addAlias }) { addAlias("@/x", "./b.tsx"); },
            };
            "#,
        )
        .await
        .unwrap();
        let host = PluginHost::spawn(
            vec![
                PluginSpec {
                    name: "a".into(),
                    module: file_url_for_test(&plugin_a),
                    options: serde_json::json!({}),
                },
                PluginSpec {
                    name: "b".into(),
                    module: file_url_for_test(&plugin_b),
                    options: serde_json::json!({}),
                },
            ],
            None,
        )
        .await
        .expect("host spawns");
        let err = host
            .run_setup(
                tmp.path(),
                crate::plugin_registries::SetupCommand::Dev,
                &serde_json::json!({}),
            )
            .await
            .expect_err("conflicting alias must error");
        let msg = format!("{err:#}");
        assert!(msg.contains("alias") && msg.contains("@/x") && msg.contains("`a`") && msg.contains("`b`"), "got: {msg}");
        host.shutdown().await.ok();
    }

    #[tokio::test]
    async fn setup_hook_silent_when_plugin_has_no_setup() {
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("silent.mjs");
        tokio::fs::write(
            &plugin_path,
            r#"
            export default {
              name: "silent",
              preBuild() {},
            };
            "#,
        )
        .await
        .unwrap();
        let host = PluginHost::spawn(
            vec![PluginSpec {
                name: "silent".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
        )
        .await
        .expect("host spawns");
        let regs = host
            .run_setup(
                tmp.path(),
                crate::plugin_registries::SetupCommand::Build,
                &serde_json::json!({}),
            )
            .await
            .expect("setup no-op ok");
        assert!(regs.aliases.is_empty());
        assert!(regs.virtual_modules.is_empty());
        assert!(regs.injected_routes.is_empty());
        host.shutdown().await.ok();
    }

    #[tokio::test]
    async fn setup_hook_closes_markdown_extension_surface() {
        // Spec AC: SetupContext exposes ONLY injectRoute /
        // addVirtualModule / addAlias. No addRemarkPlugin etc. — those
        // markdown surfaces are deliberately closed in v1. Test by
        // checking the ctx from a plugin's `setup` and asserting the
        // forbidden names are `undefined`.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("closed.mjs");
        let marker = tmp.path().join("closed-surface.txt");
        let marker_str = marker.to_string_lossy().to_string();
        let plugin_src = format!(
            r#"
            import {{ writeFileSync }} from "node:fs";
            export default {{
              name: "closed",
              setup(ctx) {{
                // Probe each forbidden surface. Each must be undefined
                // so the v1 contract stays narrow.
                const missing = [];
                if (typeof ctx.addRemarkPlugin !== "undefined") missing.push("addRemarkPlugin");
                if (typeof ctx.addRehypePlugin !== "undefined") missing.push("addRehypePlugin");
                if (typeof ctx.addMarkdownVisitor !== "undefined") missing.push("addMarkdownVisitor");
                writeFileSync({marker:?}, missing.join(","));
              }},
            }};
            "#,
            marker = marker_str,
        );
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();
        let host = PluginHost::spawn(
            vec![PluginSpec {
                name: "closed".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
        )
        .await
        .expect("host spawns");
        host.run_setup(
            tmp.path(),
            crate::plugin_registries::SetupCommand::Dev,
            &serde_json::json!({}),
        )
        .await
        .expect("setup ok");
        let leaked = tokio::fs::read_to_string(&marker).await.unwrap();
        assert!(
            leaked.is_empty(),
            "SetupContext leaked forbidden surfaces: {leaked}",
        );
        host.shutdown().await.ok();
    }
}
