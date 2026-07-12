//! Plugin host subprocess + lifecycle hook dispatcher (Sub 3 / #108,
//! extended in Astro-migration epic #253 / #255 with the new `setup`
//! hook, and in Preview Parity epic #1541 / sub-issue #1542 with the
//! `previewMiddleware` hook + `"preview"` setup command).
//!
//! Owns one long-lived `node crates/zfb/js/plugin-host.mjs` process for
//! the lifetime of a build (or dev / preview session) and dispatches
//! the lifecycle hooks — `setup`, `preBuild`, `postBuild`,
//! `devMiddleware`, `previewMiddleware` — over a newline-delimited JSON
//! stdio protocol.
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
//! - Preview middleware (#1542): [`PluginHost::register_preview_middlewares`]
//!   / [`PluginHost::invoke_preview_handler`] mirror the dev-middleware
//!   pair above byte-for-byte in wire shape ([`DevRegisterContext`],
//!   [`DevRegistration`], [`DevRequest`], [`DevResponse`] are all
//!   shared) — only the plugin-module hook (`previewMiddleware` vs
//!   `devMiddleware`) and JS-host message `kind` differ.
//! - [`PluginHost::shutdown`] sends a `shutdown` command, waits the
//!   child, and joins the reader task (bounded). It is idempotent — the
//!   host is `Clone` and only the first caller across all clones tears
//!   down; later/overlapping calls are no-ops. Drop also kills the
//!   process — the explicit shutdown is the graceful path.
//!
//! The host writes logger calls out-of-band (no `id`) and the Rust
//! reader forwards them into [`tracing`] at the matching level so the
//! plugin's log lines blend with the rest of the build output.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// One emitted route in the `postBuild` route manifest (#262). Exposed
/// on `ctx.routes.routes` so plugins can iterate every URL the build
/// produced (e.g. to write a `sitemap.xml`).
///
/// For static routes `params` is absent. For dynamic / catchall routes
/// `params` contains the bound parameter values: scalar strings for
/// `[slug]` segments, string arrays for `[...rest]` segments.
///
/// `prerender` mirrors the page module's `export const prerender = ...`:
/// `true` for SSG routes that produced an on-disk HTML/asset under
/// `outDir`, `false` for SSR routes whose response is computed by the
/// runtime adapter and has no on-disk artifact. Plugins that emit
/// indexes of "URLs the build wrote to disk" (sitemap.xml,
/// search-index.json, etc.) should filter `r.prerender !== false`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PostBuildRouteEntry {
    /// Emitted URL path, e.g. `/blog/hello/`.
    pub url: String,
    /// Path under `outDir`, e.g. `blog/hello/index.html`.
    pub output: String,
    /// File extension: `html`, `xml`, `rss`, `txt`, `json`, …
    pub extension: String,
    /// Source page module, relative to the project root,
    /// e.g. `pages/blog/[slug].tsx`.
    pub source: String,
    /// True when the page is prerendered to disk (default / SSG);
    /// false when the page exports `prerender = false` and is served
    /// by the runtime adapter (SSR). Populated for both static and
    /// dynamic routes.
    pub prerender: bool,
    /// Bound route params. `None` for static routes. Dynamic params are
    /// `String` scalars; catchall params are `Vec<String>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<std::collections::BTreeMap<String, PostBuildParamValue>>,
}

/// A single bound route parameter value in the postBuild manifest.
/// Dynamic (`[slug]`) params are scalars; catchall (`[...rest]`) params
/// are arrays, matching the shape the plugin's TypeScript types expose.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum PostBuildParamValue {
    Scalar(String),
    Array(Vec<String>),
}

/// The route manifest passed to `postBuild` plugins on `ctx.routes`.
/// Sorted by `url` for byte-stable output across runs.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PostBuildRouteManifest {
    pub routes: Vec<PostBuildRouteEntry>,
}

impl PostBuildRouteManifest {
    /// Construct an empty manifest.
    pub fn empty() -> Self {
        Self { routes: Vec::new() }
    }
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
    /// Route manifest — present only on `postBuild` calls; absent
    /// (`undefined` in JS) on `preBuild` calls (#262). Sorted by `url`
    /// for byte-stable output across runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routes: Option<PostBuildRouteManifest>,
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
    /// `"build"`, `"dev"`, or `"preview"` (#1542) — string form matches
    /// the `command` field the JS-side `SetupContext` exposes.
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
    /// Latch that makes [`PluginHost::shutdown`] idempotent. The first
    /// caller to flip this false→true owns the teardown (send `shutdown`,
    /// take + wait the child, join the reader). Every other call — on any
    /// of the [`Clone`]d handles the dev server holds — returns `Ok(())`
    /// immediately without touching the child or sending a second
    /// `shutdown` command. This is what stops an in-flight
    /// `invoke_dev_handler` on one clone from being pre-empted by another
    /// clone's `shutdown` `take()`ing the child out from under it.
    shutting_down: AtomicBool,
    /// Reader-task join handle, kept so teardown can be *bounded*: a hung
    /// child only closes stdout (and thus ends the reader loop) when it
    /// actually dies, so shutdown joins this with a deadline instead of
    /// detaching and hoping. `None` once joined.
    reader_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Dropping the [`_tempdir`] removes the staged plugin-host script.
    _tempdir: tempfile::TempDir,
    /// Maximum time any single plugin hook reply is awaited before the
    /// host is force-killed and the build fails with a diagnostic error.
    /// Env: ZFB_PLUGIN_HOOK_TIMEOUT (seconds). Default: 120s.
    hook_timeout: std::time::Duration,
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

/// Default hook timeout in seconds — generous because postBuild may do
/// real work (sitemap generation, asset upload, etc.).
/// Override via ZFB_PLUGIN_HOOK_TIMEOUT env var or config `pluginHookTimeoutSecs`.
const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 120;

/// Resolve the hook timeout: config field > env var > 120s default.
///
/// Precedence (highest to lowest):
/// 1. Explicit `config_secs` value (from `Config.pluginHookTimeoutSecs`)
/// 2. `ZFB_PLUGIN_HOOK_TIMEOUT` env var (seconds)
/// 3. 120s built-in default
pub fn resolve_hook_timeout(config_secs: Option<u64>) -> std::time::Duration {
    // A zero (or unparseable env) value is ignored and falls through to the
    // next source: `0` would make every hook time out instantly, which is
    // never an intended configuration.
    if let Some(s) = config_secs {
        if s > 0 {
            return std::time::Duration::from_secs(s);
        }
    }
    if let Ok(val) = std::env::var("ZFB_PLUGIN_HOOK_TIMEOUT") {
        if let Ok(s) = val.trim().parse::<u64>() {
            if s > 0 {
                return std::time::Duration::from_secs(s);
            }
        }
    }
    std::time::Duration::from_secs(DEFAULT_HOOK_TIMEOUT_SECS)
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
    ///
    /// `hook_timeout` is the maximum time any single hook reply is awaited.
    /// Pass `None` to auto-resolve from `ZFB_PLUGIN_HOOK_TIMEOUT` / 120s default.
    pub async fn spawn(plugins: Vec<PluginSpec>, node_binary: Option<OsString>) -> Result<Self> {
        Self::spawn_with_timeout(plugins, node_binary, None).await
    }

    /// Like [`spawn`] but with an explicit hook timeout (used by the build
    /// orchestrator to thread `Config.pluginHookTimeoutSecs`).
    pub async fn spawn_with_timeout(
        plugins: Vec<PluginSpec>,
        node_binary: Option<OsString>,
        hook_timeout_secs: Option<u64>,
    ) -> Result<Self> {
        let hook_timeout = resolve_hook_timeout(hook_timeout_secs);
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
            shutting_down: AtomicBool::new(false),
            reader_handle: Mutex::new(None),
            _tempdir: tmp,
            hook_timeout,
        });

        // Reader task — drains stdout, dispatches replies to the
        // matching pending sender, and forwards log lines into tracing.
        let inner_for_reader = Arc::clone(&inner);
        let reader_handle = tokio::spawn(async move {
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
        // Stash the reader handle so shutdown can join it with a bound
        // rather than leaving the task fully detached.
        *inner
            .reader_handle
            .try_lock()
            .expect("reader_handle uncontended at spawn time — no other handle exists yet") =
            Some(reader_handle);

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
            .request_typed::<serde_json::Value>("preBuild", serde_json::json!({ "ctx": ctx }))
            .await?;
        Ok(())
    }

    /// Run every plugin's `postBuild` hook (in declaration order).
    ///
    /// `ctx` must carry a populated `routes` field — use
    /// [`PostBuildRouteManifest`] to construct it from the build's
    /// emitted route table (#262).
    pub async fn run_post_build(&self, ctx: &BuildHookContext) -> Result<()> {
        let _ = self
            .request_typed::<serde_json::Value>("postBuild", serde_json::json!({ "ctx": ctx }))
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
    /// `command` controls the `ctx.command` string visible to plugins.
    /// `injectRoute` is accepted into `injected_routes` in BOTH dev and
    /// build (#1193, #1262): a preset that owns the site index calls
    /// `injectRoute("/")` in either mode, and the host must not crash. The
    /// dev `"/"` reservation is enforced downstream in the Rust dev route
    /// resolution (`resolve_dev_pages_root` drops an injected `"/"` so it is
    /// never rendered in dev), NOT by rejecting it here.
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
                // Optional `{ prerender }` hint (#1193). Absent in the
                // JS payload when the plugin omits the options arg.
                #[serde(default)]
                prerender: Option<bool>,
            },
            /// Client-side side-effect entry (#1196).
            #[serde(rename = "addClientEntry")]
            AddClientEntry { entrypoint: String },
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
                        prerender,
                    } => RawSetupRegistration::InjectRoute {
                        pattern,
                        entrypoint,
                        prerender,
                    },
                    WireRegistration::AddClientEntry { entrypoint } => {
                        RawSetupRegistration::AddClientEntry { entrypoint }
                    }
                })
                .collect();
            acc.ingest(RawPluginSetupOutput {
                plugin: output.plugin,
                registrations: raws,
            })
            .map_err(anyhow::Error::from)?;
        }
        let (aliases, virtual_modules, injected_routes, client_entries) = acc.finish();
        Ok(SetupRegistries {
            aliases,
            virtual_modules,
            injected_routes,
            client_entries,
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
    pub async fn invoke_virtual_loader(&self, loader_id: &VirtualLoaderId) -> Result<String> {
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
        self.register_middlewares_via("devRegister", ctx).await
    }

    /// Invoke a previously-registered dev-middleware handler.
    pub async fn invoke_dev_handler(
        &self,
        handler_id: &str,
        request: DevRequest,
    ) -> Result<DevResponse> {
        self.invoke_middleware_via("devInvoke", handler_id, request)
            .await
    }

    /// Call `previewMiddleware(ctx)` on every plugin and collect the
    /// `(path, handlerId)` registrations the preview server should
    /// route (#1542). Mirrors [`PluginHost::register_dev_middlewares`]
    /// end-to-end — same wire shapes ([`DevRegisterContext`],
    /// [`DevRegistration`]) — the only difference is which plugin-module
    /// hook fires (`previewMiddleware`, not `devMiddleware`) and which
    /// JS-host message `kind` is sent (`previewRegister`, not
    /// `devRegister`), both threaded through the shared
    /// [`PluginHost::register_middlewares_via`] helper. A plugin
    /// declaring only `devMiddleware` produces NO registration here, and
    /// vice versa — the two hooks are dispatched independently (#1542
    /// baked decision 1: explicit per-mode opt-in, not devMiddleware-reuse).
    pub async fn register_preview_middlewares(
        &self,
        ctx: &DevRegisterContext,
    ) -> Result<Vec<DevRegistration>> {
        self.register_middlewares_via("previewRegister", ctx).await
    }

    /// Invoke a previously-registered preview-middleware handler.
    /// Mirrors [`PluginHost::invoke_dev_handler`] — same [`DevRequest`] /
    /// [`DevResponse`] wire shapes, dispatched via the JS host's
    /// `previewInvoke` message kind (via the shared
    /// [`PluginHost::invoke_middleware_via`] helper) against the handler
    /// map the host populated from `previewRegister` (kept separate from
    /// the dev-middleware handler map on the JS side, so a preview
    /// handler id can never resolve to a dev handler or vice versa).
    pub async fn invoke_preview_handler(
        &self,
        handler_id: &str,
        request: DevRequest,
    ) -> Result<DevResponse> {
        self.invoke_middleware_via("previewInvoke", handler_id, request)
            .await
    }

    /// Shared implementation behind [`PluginHost::register_dev_middlewares`]
    /// / [`PluginHost::register_preview_middlewares`] (#1542). `kind` is
    /// the JS-host message kind (`"devRegister"` or `"previewRegister"`)
    /// — everything else about the round-trip (wire shape, reply
    /// decoding) is identical between the two hooks.
    async fn register_middlewares_via(
        &self,
        kind: &str,
        ctx: &DevRegisterContext,
    ) -> Result<Vec<DevRegistration>> {
        #[derive(Deserialize)]
        struct Reply {
            registrations: Vec<DevRegistration>,
        }
        let reply: Reply = self
            .request_typed(kind, serde_json::json!({ "ctx": ctx }))
            .await?;
        Ok(reply.registrations)
    }

    /// Shared implementation behind [`PluginHost::invoke_dev_handler`] /
    /// [`PluginHost::invoke_preview_handler`] (#1542). `kind` is the
    /// JS-host message kind (`"devInvoke"` or `"previewInvoke"`).
    async fn invoke_middleware_via(
        &self,
        kind: &str,
        handler_id: &str,
        request: DevRequest,
    ) -> Result<DevResponse> {
        let resp: DevResponse = self
            .request_typed(
                kind,
                serde_json::json!({
                    "handlerId": handler_id,
                    "request": request,
                }),
            )
            .await?;
        Ok(resp)
    }

    /// Force-kill the child process. Used when a hook timeout fires.
    async fn force_kill_child(&self) {
        let mut guard = self.inner.child.lock().await;
        if let Some(mut child) = guard.take() {
            let _ = child.kill().await;
        }
    }

    /// Send a `shutdown` command and wait for the child to exit.
    ///
    /// **Idempotent.** [`PluginHost`] is [`Clone`] and the dev server holds
    /// clones; the first call to `shutdown` on any clone wins (latched via
    /// an [`AtomicBool`]) and every subsequent call — including overlapping
    /// ones on sibling clones — is a no-op returning `Ok(())`. Without this
    /// latch a second clone calling `shutdown` would `take()` and kill the
    /// child out from under an in-flight `invoke_dev_handler` on another
    /// clone, which then races the reader task's stdout-close path and gets
    /// the synthetic "stdout closed before reply" error.
    ///
    /// Best-effort: if the child has already died, this returns Ok.
    ///
    /// **Caller contract:** no dispatch (hook or `invoke_dev_handler`) may
    /// overlap `shutdown`. The latch makes *shutdown itself* race-free, but
    /// it does not order a concurrent dispatch against the teardown — the
    /// caller (dev server / build orchestrator) is responsible for quiescing
    /// dispatch before tearing the host down.
    pub async fn shutdown(&self) -> Result<()> {
        // Latch: the first caller to flip false→true owns teardown; all
        // others (overlapping calls on sibling clones) return immediately.
        if self.inner.shutting_down.swap(true, Ordering::AcqRel) {
            return Ok(());
        }

        // Send the shutdown with the 2s shutdown budget, not the hook timeout.
        let shutdown_budget = std::time::Duration::from_secs(2);
        let _ = self
            .request_typed_with_timeout::<serde_json::Value>(
                "shutdown",
                serde_json::json!({}),
                shutdown_budget,
            )
            .await;
        {
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
        }

        // Join the reader task with a bound. Once the child is gone its
        // stdout closes and the reader loop returns on its own; the deadline
        // is a guard against a wedged pipe so teardown can't hang here.
        if let Some(handle) = self.inner.reader_handle.lock().await.take() {
            let abort = handle.abort_handle();
            match tokio::time::timeout(std::time::Duration::from_secs(2), handle).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!(error = %e, "plugin host: reader task join failed"),
                Err(_) => {
                    warn!("plugin host: reader task did not finish within shutdown budget");
                    // Bounded teardown: abort the wedged reader rather than
                    // dropping its JoinHandle (which would only detach it).
                    abort.abort();
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
        let timeout = self.inner.hook_timeout;
        self.request_typed_with_timeout(kind, body, timeout).await
    }

    async fn request_typed_with_timeout<T: serde::de::DeserializeOwned>(
        &self,
        kind: &str,
        body: serde_json::Value,
        timeout: std::time::Duration,
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
        let mut line =
            serde_json::to_string(&envelope).context("plugin host: serialise request")?;
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

        let reply = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => {
                return Err(anyhow!("plugin host: reply channel dropped"));
            }
            Err(_elapsed) => {
                // Hook did not complete within the deadline. Evict the pending
                // entry (the channel is gone), force-kill the child so no
                // future hooks can hang, then return a diagnostic error.
                {
                    let mut pend = self.inner.pending.lock().await;
                    pend.remove(&id);
                }
                self.force_kill_child().await;
                let secs = timeout.as_secs();
                return Err(anyhow!(
                    "`{}` hook did not complete within {}s — \
                     check for an unresolved promise / open handle / setInterval \
                     in a plugin's `{}` implementation",
                    kind,
                    secs,
                    kind,
                ));
            }
        };
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
            HostLine::Log(LogLine { log }) => match log.level.as_str() {
                "warn" => {
                    warn!(target: "zfb_plugin", plugin = %log.plugin, "{}", log.message);
                }
                "error" => {
                    error!(target: "zfb_plugin", plugin = %log.plugin, "{}", log.message);
                }
                _ => {
                    info!(target: "zfb_plugin", plugin = %log.plugin, "{}", log.message);
                }
            },
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
            routes: None,
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
            routes: None,
        };
        let err = host
            .run_pre_build(&ctx)
            .await
            .expect_err("preBuild should propagate the throw");
        let pe = extract_plugin_error(&err).expect("PluginError carried in chain");
        assert_eq!(pe.plugin, "thrower");
        assert_eq!(pe.hook, "preBuild");
        assert!(
            pe.message.contains("boom from preBuild"),
            "msg: {}",
            pe.message
        );
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
            routes: None,
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
        assert_eq!(
            resp.headers.get("x-method").map(|s| s.as_str()),
            Some("GET")
        );
        assert_eq!(resp.body, "hello /echo?x=1");

        host.shutdown().await.ok();
    }

    // --- #1542 previewMiddleware integration tests ---------------------------

    #[tokio::test]
    async fn host_preview_middleware_register_and_invoke_round_trip() {
        // Mirrors `host_dev_middleware_register_and_invoke_round_trip`
        // above byte-for-byte, but through the `previewMiddleware` hook
        // and the `previewRegister`/`previewInvoke` message kinds
        // (#1542) — proves the preview path is a genuine end-to-end
        // mirror of the dev path, not a stub.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("preview-middleware.mjs");
        tokio::fs::write(
            &plugin_path,
            r#"
            export default {
              name: "echo-preview-mw",
              previewMiddleware({ register }) {
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
                name: "echo-preview-mw".into(),
                module: module_url,
                options: serde_json::json!({}),
            }],
            None,
        )
        .await
        .expect("host spawns");

        let regs = host
            .register_preview_middlewares(&DevRegisterContext {
                project_root: tmp.path().to_path_buf(),
                config: serde_json::json!({}),
            })
            .await
            .expect("register ok");
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].path, "/echo");

        let resp = host
            .invoke_preview_handler(
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
        assert_eq!(
            resp.headers.get("x-method").map(|s| s.as_str()),
            Some("GET")
        );
        assert_eq!(resp.body, "hello /echo?x=1");

        host.shutdown().await.ok();
    }

    #[tokio::test]
    async fn preview_and_dev_middleware_hooks_are_dispatched_independently() {
        // #1542 baked decision 1: a NEW `previewMiddleware` hook, NOT
        // devMiddleware-reuse. A plugin declaring only `devMiddleware`
        // must produce zero registrations under `previewRegister`, and
        // a plugin declaring only `previewMiddleware` must produce zero
        // registrations under `devRegister` — the two hooks are
        // dispatched independently, never conflated. Also locks in
        // invoke-time isolation: a handler_id minted by one hook must
        // fail (not cross-dispatch) when passed to the other hook's
        // invoke method.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("split-middleware.mjs");
        tokio::fs::write(
            &plugin_path,
            r#"
            export default {
              name: "split-mw",
              devMiddleware({ register }) {
                register("/dev-only", () => ({ status: 200, body: "dev" }));
              },
              previewMiddleware({ register }) {
                register("/preview-only", () => ({ status: 200, body: "preview" }));
              },
            };
            "#,
        )
        .await
        .unwrap();
        let host = PluginHost::spawn(
            vec![PluginSpec {
                name: "split-mw".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
        )
        .await
        .expect("host spawns");

        let ctx = DevRegisterContext {
            project_root: tmp.path().to_path_buf(),
            config: serde_json::json!({}),
        };
        let dev_regs = host
            .register_dev_middlewares(&ctx)
            .await
            .expect("devRegister ok");
        assert_eq!(dev_regs.len(), 1);
        assert_eq!(dev_regs[0].path, "/dev-only");

        let preview_regs = host
            .register_preview_middlewares(&ctx)
            .await
            .expect("previewRegister ok");
        assert_eq!(preview_regs.len(), 1);
        assert_eq!(preview_regs[0].path, "/preview-only");

        // Isolation must hold at INVOKE time too, not just registration
        // time — a dev handler_id must never resolve through
        // `invoke_preview_handler` (and vice versa), because the JS host
        // keeps the two hooks' handler maps entirely separate.
        let dev_via_preview = host
            .invoke_preview_handler(
                &dev_regs[0].handler_id,
                DevRequest {
                    method: "GET".into(),
                    url: "/dev-only".into(),
                    headers: HashMap::new(),
                    body: None,
                },
            )
            .await;
        assert!(
            dev_via_preview.is_err(),
            "a dev handler_id must NOT resolve via invoke_preview_handler"
        );

        let preview_via_dev = host
            .invoke_dev_handler(
                &preview_regs[0].handler_id,
                DevRequest {
                    method: "GET".into(),
                    url: "/preview-only".into(),
                    headers: HashMap::new(),
                    body: None,
                },
            )
            .await;
        assert!(
            preview_via_dev.is_err(),
            "a preview handler_id must NOT resolve via invoke_dev_handler"
        );

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
    async fn setup_hook_accepts_inject_route_in_build_mode() {
        // #1193: `injectRoute` during a build is now ACCEPTED end-to-end
        // through the JS host (it contributes a package-owned build
        // route), replacing the pre-#1193 dev-only hard error. The
        // optional `{ prerender }` arg threads through to the registry.
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
                injectRoute("/preset-page", "./scripts/x.ts");
                injectRoute("/ssr-page", "./scripts/ssr.ts", { prerender: false });
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
        let regs = host
            .run_setup(
                tmp.path(),
                crate::plugin_registries::SetupCommand::Build,
                &serde_json::json!({}),
            )
            .await
            .expect("injectRoute during build must be accepted (#1193)");
        assert_eq!(regs.injected_routes.len(), 2);
        let by_pattern = |p: &str| {
            regs.injected_routes
                .as_slice()
                .iter()
                .find(|r| r.pattern == p)
                .unwrap_or_else(|| panic!("missing route {p}"))
        };
        // No options → prerender hint absent (build defaults to SSG).
        assert_eq!(by_pattern("/preset-page").prerender, None);
        // `{ prerender: false }` threads through to the registry.
        assert_eq!(by_pattern("/ssr-page").prerender, Some(false));
        host.shutdown().await.ok();
    }

    #[tokio::test]
    async fn setup_hook_accepts_root_inject_route_in_dev_and_build() {
        // #1262: a preset's `setup()` runs in BOTH `zfb dev` and `zfb build`,
        // so a package owning the site index calls `injectRoute("/")` in both.
        // The host must ACCEPT the `"/"` registration in dev (the pre-#1262
        // dev-only throw crashed any such preset's dev server — the bug) AND in
        // build. The dev `"/"` reservation is upheld DOWNSTREAM, in the Rust
        // dev route resolution (`resolve_dev_pages_root` drops the injected
        // `"/"` so it is never rendered in dev) — NOT by rejecting it here.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("rooter.mjs");
        tokio::fs::write(
            &plugin_path,
            r#"
            export default {
              name: "rooter",
              setup({ injectRoute }) {
                injectRoute("/", "./scripts/home.ts");
              },
            };
            "#,
        )
        .await
        .unwrap();

        // Dev: ACCEPTED (no throw) — the dev server must boot. The "/" drop is
        // enforced later in `resolve_dev_pages_root`, not by the host.
        let dev_host = PluginHost::spawn(
            vec![PluginSpec {
                name: "rooter".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
        )
        .await
        .expect("host spawns");
        let dev_regs = dev_host
            .run_setup(
                tmp.path(),
                crate::plugin_registries::SetupCommand::Dev,
                &serde_json::json!({}),
            )
            .await
            .expect("dev injectRoute(\"/\") must be ACCEPTED (#1262)");
        assert_eq!(dev_regs.injected_routes.len(), 1);
        assert_eq!(dev_regs.injected_routes.as_slice()[0].pattern, "/");
        dev_host.shutdown().await.ok();

        // Build: accepted.
        let build_host = PluginHost::spawn(
            vec![PluginSpec {
                name: "rooter".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
        )
        .await
        .expect("host spawns");
        let regs = build_host
            .run_setup(
                tmp.path(),
                crate::plugin_registries::SetupCommand::Build,
                &serde_json::json!({}),
            )
            .await
            .expect("build injectRoute(\"/\") must be accepted (#1193)");
        assert_eq!(regs.injected_routes.len(), 1);
        assert_eq!(regs.injected_routes.as_slice()[0].pattern, "/");
        build_host.shutdown().await.ok();
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
        assert!(
            msg.contains("alias")
                && msg.contains("@/x")
                && msg.contains("`a`")
                && msg.contains("`b`"),
            "got: {msg}"
        );
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

    // --- #1542 preview setup-lifecycle integration tests ----------------------

    #[tokio::test]
    async fn preview_setup_drives_setup_hook_with_preview_command() {
        // #1542 decision 2: `run_preview_setup` (crate::plugin_registries)
        // fires the shared JS `setup` handler with `ctx.command ===
        // "preview"` — same wire kind as build/dev, distinguished only
        // by the new `SetupCommand::Preview` variant — so plugin-side
        // state init runs under preview too.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("preview-setupper.mjs");
        tokio::fs::write(
            &plugin_path,
            r#"
            export default {
              name: "preview-setupper",
              setup({ command }) {
                if (command !== "preview") {
                  throw new Error("expected preview, got " + command);
                }
              },
            };
            "#,
        )
        .await
        .unwrap();
        let host = PluginHost::spawn(
            vec![PluginSpec {
                name: "preview-setupper".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
        )
        .await
        .expect("host spawns");

        crate::plugin_registries::run_preview_setup(
            Some(&host),
            tmp.path(),
            &serde_json::json!({}),
        )
        .await
        .expect("preview setup must see command == \"preview\"");

        host.shutdown().await.ok();
    }

    #[tokio::test]
    async fn preview_setup_with_no_host_is_a_no_op() {
        // `run_preview_setup(None, ...)` mirrors `run_plugin_setup`'s
        // "no host, no-op" handling for plugin-less projects — Level 1,
        // no node/subprocess required.
        let root = std::path::PathBuf::from("/proj");
        let regs = crate::plugin_registries::run_preview_setup(None, &root, &serde_json::json!({}))
            .await
            .expect("no-host preview setup must succeed");
        assert!(regs.aliases.is_empty());
        assert!(regs.virtual_modules.is_empty());
        assert!(regs.injected_routes.is_empty());
        assert!(regs.client_entries.is_empty());
    }

    #[tokio::test]
    async fn preview_lifecycle_never_fires_pre_build() {
        // #1542 decision 3: `preBuild` does NOT fire under preview. There
        // is no Rust-side call path from the preview lifecycle
        // (`run_preview_setup` + `register_preview_middlewares`) into
        // `run_pre_build` — this test locks that in behaviourally: a
        // plugin's `preBuild` hook writes a marker file, and driving
        // ONLY the preview-side lifecycle must never touch it.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("preview-no-prebuild.mjs");
        let marker = tmp.path().join("prebuild-marker.txt");
        let plugin_src = format!(
            r#"
            import {{ writeFileSync }} from "node:fs";
            export default {{
              name: "preview-no-prebuild",
              setup() {{}},
              preBuild() {{
                writeFileSync({marker:?}, "fired");
              }},
              previewMiddleware({{ register }}) {{
                register("/x", () => ({{ status: 200 }}));
              }},
            }};
            "#,
            marker = marker.to_string_lossy().to_string(),
        );
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();
        let host = PluginHost::spawn(
            vec![PluginSpec {
                name: "preview-no-prebuild".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
        )
        .await
        .expect("host spawns");

        crate::plugin_registries::run_preview_setup(
            Some(&host),
            tmp.path(),
            &serde_json::json!({}),
        )
        .await
        .expect("preview setup ok");
        let regs = host
            .register_preview_middlewares(&DevRegisterContext {
                project_root: tmp.path().to_path_buf(),
                config: serde_json::json!({}),
            })
            .await
            .expect("previewRegister ok");
        assert_eq!(regs.len(), 1);

        host.shutdown().await.ok();

        assert!(
            !marker.exists(),
            "preBuild must NOT fire anywhere in the preview lifecycle"
        );
    }

    // --- #262 routes-manifest tests ------------------------------------------

    /// `postBuild` receives `ctx.routes` populated; `preBuild` sees
    /// `ctx.routes === undefined`. Covers the core acceptance criterion.
    #[tokio::test]
    async fn post_build_ctx_carries_routes_manifest_pre_build_does_not() {
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("manifest-probe.mjs");
        let pre_marker = tmp.path().join("pre-routes-type.txt");
        let post_marker = tmp.path().join("post-routes.json");
        let plugin_src = format!(
            r#"
            import {{ writeFileSync }} from "node:fs";
            export default {{
              name: "manifest-probe",
              preBuild(ctx) {{
                // routes must be undefined on preBuild
                writeFileSync({pre:?}, typeof ctx.routes);
              }},
              postBuild(ctx) {{
                // routes must be present on postBuild
                writeFileSync({post:?}, JSON.stringify(ctx.routes));
              }},
            }};
            "#,
            pre = pre_marker.to_string_lossy().to_string(),
            post = post_marker.to_string_lossy().to_string(),
        );
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();

        let module_url = file_url_for_test(&plugin_path);
        let host = PluginHost::spawn(
            vec![PluginSpec {
                name: "manifest-probe".into(),
                module: module_url,
                options: serde_json::json!({}),
            }],
            None,
        )
        .await
        .expect("host spawns");

        let pre_ctx = BuildHookContext {
            project_root: tmp.path().to_path_buf(),
            out_dir: tmp.path().join("dist"),
            config: serde_json::json!({}),
            routes: None,
        };
        host.run_pre_build(&pre_ctx).await.expect("preBuild ok");

        // Build a manifest with one static route.
        let manifest = PostBuildRouteManifest {
            routes: vec![PostBuildRouteEntry {
                url: "/".to_string(),
                output: "index.html".to_string(),
                extension: "html".to_string(),
                source: "pages/index.tsx".to_string(),
                prerender: true,
                params: None,
            }],
        };
        let post_ctx = BuildHookContext {
            project_root: tmp.path().to_path_buf(),
            out_dir: tmp.path().join("dist"),
            config: serde_json::json!({}),
            routes: Some(manifest),
        };
        host.run_post_build(&post_ctx).await.expect("postBuild ok");
        host.shutdown().await.ok();

        // preBuild must have seen `typeof ctx.routes === "undefined"`.
        let pre_type = tokio::fs::read_to_string(&pre_marker).await.unwrap();
        assert_eq!(
            pre_type.trim(),
            "undefined",
            "preBuild must NOT expose ctx.routes, got type: {pre_type}",
        );

        // postBuild must have received the manifest.
        let post_json = tokio::fs::read_to_string(&post_marker).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&post_json).unwrap();
        let routes = parsed["routes"].as_array().expect("routes array");
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0]["url"], "/");
        assert_eq!(routes[0]["output"], "index.html");
        assert_eq!(routes[0]["extension"], "html");
        assert_eq!(routes[0]["source"], "pages/index.tsx");
        // params must be absent for a static route.
        assert!(
            routes[0].get("params").is_none() || routes[0]["params"].is_null(),
            "static route must not carry params, got: {}",
            routes[0]
        );
    }

    /// Dynamic route params surface as string scalars; catchall params
    /// surface as string arrays (#262 AC).
    #[tokio::test]
    async fn post_build_route_manifest_dynamic_and_catchall_params() {
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("params-probe.mjs");
        let marker = tmp.path().join("routes.json");
        let plugin_src = format!(
            r#"
            import {{ writeFileSync }} from "node:fs";
            export default {{
              name: "params-probe",
              postBuild(ctx) {{
                writeFileSync({marker:?}, JSON.stringify(ctx.routes));
              }},
            }};
            "#,
            marker = marker.to_string_lossy().to_string(),
        );
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();
        let module_url = file_url_for_test(&plugin_path);
        let host = PluginHost::spawn(
            vec![PluginSpec {
                name: "params-probe".into(),
                module: module_url,
                options: serde_json::json!({}),
            }],
            None,
        )
        .await
        .expect("host spawns");

        let mut dyn_params = std::collections::BTreeMap::new();
        dyn_params.insert(
            "slug".to_string(),
            PostBuildParamValue::Scalar("hello-world".to_string()),
        );

        let mut catchall_params = std::collections::BTreeMap::new();
        catchall_params.insert(
            "rest".to_string(),
            PostBuildParamValue::Array(vec!["a".to_string(), "b".to_string()]),
        );

        let manifest = PostBuildRouteManifest {
            routes: vec![
                PostBuildRouteEntry {
                    url: "/blog/hello-world".to_string(),
                    output: "blog/hello-world/index.html".to_string(),
                    extension: "html".to_string(),
                    source: "pages/blog/[slug].tsx".to_string(),
                    prerender: true,
                    params: Some(dyn_params),
                },
                PostBuildRouteEntry {
                    url: "/docs/a/b".to_string(),
                    output: "docs/a/b/index.html".to_string(),
                    extension: "html".to_string(),
                    source: "pages/docs/[...rest].tsx".to_string(),
                    prerender: true,
                    params: Some(catchall_params),
                },
                PostBuildRouteEntry {
                    url: "/sitemap.xml".to_string(),
                    output: "sitemap.xml".to_string(),
                    extension: "xml".to_string(),
                    source: "pages/sitemap.xml.tsx".to_string(),
                    prerender: true,
                    params: None,
                },
            ],
        };
        let ctx = BuildHookContext {
            project_root: tmp.path().to_path_buf(),
            out_dir: tmp.path().join("dist"),
            config: serde_json::json!({}),
            routes: Some(manifest),
        };
        host.run_post_build(&ctx).await.expect("postBuild ok");
        host.shutdown().await.ok();

        let json: serde_json::Value =
            serde_json::from_str(&tokio::fs::read_to_string(&marker).await.unwrap()).unwrap();
        let routes = json["routes"].as_array().unwrap();
        assert_eq!(routes.len(), 3);

        // Dynamic route: slug is a string scalar.
        let dyn_r = routes
            .iter()
            .find(|r| r["url"] == "/blog/hello-world")
            .unwrap();
        assert_eq!(dyn_r["params"]["slug"], "hello-world");

        // Catchall route: rest is a string array.
        let ca_r = routes.iter().find(|r| r["url"] == "/docs/a/b").unwrap();
        assert!(
            ca_r["params"]["rest"].is_array(),
            "catchall param must be array"
        );
        let rest = ca_r["params"]["rest"].as_array().unwrap();
        assert_eq!(rest, &[serde_json::json!("a"), serde_json::json!("b")]);

        // Non-HTML route: extension is "xml", no params key.
        let xml_r = routes.iter().find(|r| r["url"] == "/sitemap.xml").unwrap();
        assert_eq!(xml_r["extension"], "xml");
        assert_eq!(xml_r["output"], "sitemap.xml");
        assert!(
            xml_r.get("params").is_none() || xml_r["params"].is_null(),
            "non-HTML static route must not carry params, got: {}",
            xml_r
        );

        // prerender field is present on all routes and serialises as a
        // boolean (not omitted, not null).
        for r in routes {
            assert!(
                r["prerender"].is_boolean(),
                "prerender must be a boolean, got: {}",
                r
            );
        }
    }

    /// Mixed SSG + SSR routes round-trip through the JS plugin boundary
    /// with the `prerender` field preserved on each entry. Locks the new
    /// contract: SSG (`prerender = true`) and SSR (`prerender = false`)
    /// stay distinguishable end-to-end so consumer plugins can filter
    /// `r.prerender !== false` to enumerate only on-disk URLs.
    #[tokio::test]
    async fn post_build_route_manifest_preserves_prerender_field() {
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("prerender-probe.mjs");
        let marker = tmp.path().join("routes.json");
        let plugin_src = format!(
            r#"
            import {{ writeFileSync }} from "node:fs";
            export default {{
              name: "prerender-probe",
              postBuild(ctx) {{
                writeFileSync({marker:?}, JSON.stringify(ctx.routes));
              }},
            }};
            "#,
            marker = marker.to_string_lossy().to_string(),
        );
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();
        let module_url = file_url_for_test(&plugin_path);
        let host = PluginHost::spawn(
            vec![PluginSpec {
                name: "prerender-probe".into(),
                module: module_url,
                options: serde_json::json!({}),
            }],
            None,
        )
        .await
        .expect("host spawns");

        let manifest = PostBuildRouteManifest {
            routes: vec![
                PostBuildRouteEntry {
                    url: "/".to_string(),
                    output: "index.html".to_string(),
                    extension: "html".to_string(),
                    source: "pages/index.tsx".to_string(),
                    prerender: true,
                    params: None,
                },
                PostBuildRouteEntry {
                    url: "/api/search".to_string(),
                    output: "api/search/index.html".to_string(),
                    extension: "html".to_string(),
                    source: "pages/api/search.tsx".to_string(),
                    prerender: false,
                    params: None,
                },
            ],
        };
        let ctx = BuildHookContext {
            project_root: tmp.path().to_path_buf(),
            out_dir: tmp.path().join("dist"),
            config: serde_json::json!({}),
            routes: Some(manifest),
        };
        host.run_post_build(&ctx).await.expect("postBuild ok");
        host.shutdown().await.ok();

        let json: serde_json::Value =
            serde_json::from_str(&tokio::fs::read_to_string(&marker).await.unwrap()).unwrap();
        let routes = json["routes"].as_array().unwrap();
        assert_eq!(routes.len(), 2);

        let ssg = routes.iter().find(|r| r["url"] == "/").unwrap();
        assert_eq!(
            ssg["prerender"],
            serde_json::json!(true),
            "SSG route must have prerender=true, got: {}",
            ssg
        );

        let ssr = routes.iter().find(|r| r["url"] == "/api/search").unwrap();
        assert_eq!(
            ssr["prerender"],
            serde_json::json!(false),
            "SSR route must have prerender=false, got: {}",
            ssr
        );
    }

    // --- Timeout tests (no node required) -----------------------------------

    /// Build a minimal PluginHost whose child is a `sleep`-style process
    /// that never writes to stdout, so every hook reply awaits forever.
    /// We don't need the reader task to be running — the oneshot rx will
    /// never resolve because no reply ever arrives on stdout.
    #[cfg(unix)]
    async fn stub_host_with_timeout(timeout: std::time::Duration) -> Option<PluginHost> {
        // `sleep 300` never writes anything to stdout, so rx.await blocks forever
        // unless bounded — exactly the condition under test.
        let mut child = match tokio::process::Command::new("sleep")
            .arg("300")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return None, // `sleep` not available (extremely rare)
        };
        let stdin = child.stdin.take().expect("stdin");
        // We need a valid tempdir even though the host script is never run.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let inner = Arc::new(HostInner {
            pending: Mutex::new(HashMap::new()),
            stdin: Mutex::new(stdin),
            next_id: AtomicU64::new(1),
            child: Mutex::new(Some(child)),
            shutting_down: AtomicBool::new(false),
            reader_handle: Mutex::new(None),
            _tempdir: tmp,
            hook_timeout: timeout,
        });
        Some(PluginHost { inner })
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn post_build_hook_times_out_and_returns_error() {
        // Short deadline so the test stays fast; watchdog is 3× to ensure
        // a regression is RED (hung), not a false-pass from the watchdog.
        let short = std::time::Duration::from_millis(150);
        let host = match stub_host_with_timeout(short).await {
            Some(h) => h,
            None => {
                eprintln!("skipping: `sleep` not available");
                return;
            }
        };
        let ctx = BuildHookContext {
            project_root: std::path::PathBuf::from("/tmp"),
            out_dir: std::path::PathBuf::from("/tmp/dist"),
            config: serde_json::json!({}),
            routes: None,
        };
        // Outer watchdog: if the inner timeout fires correctly the call
        // returns within ~150ms; allow 5s so CI is never flaky.
        let watchdog = std::time::Duration::from_secs(5);
        let result = tokio::time::timeout(watchdog, host.run_post_build(&ctx)).await;
        match result {
            Err(_watchdog_fired) => {
                panic!("run_post_build did NOT time out — the timeout fix is missing");
            }
            Ok(inner_result) => {
                let err = inner_result.expect_err("run_post_build must return Err on timeout");
                let msg = format!("{err:#}");
                assert!(
                    msg.contains("postBuild"),
                    "error message must name the hook; got: {msg}"
                );
                assert!(
                    msg.contains("did not complete within"),
                    "error message must state the timeout; got: {msg}"
                );
            }
        }
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn pre_build_hook_times_out_and_returns_error() {
        let short = std::time::Duration::from_millis(150);
        let host = match stub_host_with_timeout(short).await {
            Some(h) => h,
            None => {
                eprintln!("skipping: `sleep` not available");
                return;
            }
        };
        let ctx = BuildHookContext {
            project_root: std::path::PathBuf::from("/tmp"),
            out_dir: std::path::PathBuf::from("/tmp/dist"),
            config: serde_json::json!({}),
            routes: None,
        };
        let watchdog = std::time::Duration::from_secs(5);
        let result = tokio::time::timeout(watchdog, host.run_pre_build(&ctx)).await;
        match result {
            Err(_watchdog_fired) => {
                panic!("run_pre_build did NOT time out — the timeout fix is missing");
            }
            Ok(inner_result) => {
                let err = inner_result.expect_err("run_pre_build must return Err on timeout");
                let msg = format!("{err:#}");
                assert!(
                    msg.contains("preBuild"),
                    "error message must name the hook; got: {msg}"
                );
            }
        }
    }

    #[test]
    fn resolve_hook_timeout_uses_default_when_nothing_set() {
        // Can't safely mutate env in a multi-threaded test process, but
        // we can test the no-override path with an explicit None.
        let d = resolve_hook_timeout(None);
        // Env may or may not be set in the test runner; only verify the
        // default path when the var is absent.
        if std::env::var("ZFB_PLUGIN_HOOK_TIMEOUT").is_err() {
            assert_eq!(
                d,
                std::time::Duration::from_secs(DEFAULT_HOOK_TIMEOUT_SECS),
                "default must be 120s"
            );
        }
    }

    #[test]
    fn resolve_hook_timeout_config_field_wins_over_default() {
        let d = resolve_hook_timeout(Some(42));
        assert_eq!(d, std::time::Duration::from_secs(42));
    }

    // --- Shutdown idempotency / concurrency (no node required) ---------------
    //
    // #1140: `shutdown` takes `&self` and is latched so overlapping calls on
    // sibling clones (the dev server holds clones) don't race the child-take.
    // These tests use the `sleep`-stub host (no reader task, no real
    // protocol) so they exercise pure Rust shared-state — Level 1, T0.

    /// A second `shutdown` after the child is already gone must short-circuit
    /// on the latch and return `Ok(())` essentially instantly — it must NOT
    /// re-enter the 2s shutdown-request budget. This is the regression guard:
    /// before the fix `shutdown(self)` consumed the host so a "second" call
    /// wasn't even expressible; now it must be a cheap no-op.
    #[tokio::test]
    #[cfg(unix)]
    async fn shutdown_is_idempotent_second_call_is_a_fast_noop() {
        let host = match stub_host_with_timeout(std::time::Duration::from_secs(120)).await {
            Some(h) => h,
            None => {
                eprintln!("skipping: `sleep` not available");
                return;
            }
        };
        // First shutdown owns teardown. No reader task replies, so the
        // shutdown request drains its 2s budget; bound it with a watchdog so
        // a regression (hang) is RED, not a silent slow pass.
        let watchdog = std::time::Duration::from_secs(8);
        tokio::time::timeout(watchdog, host.shutdown())
            .await
            .expect("first shutdown must finish within the watchdog")
            .expect("first shutdown returns Ok");
        // Child must be gone after the first teardown.
        assert!(
            host.inner.child.lock().await.is_none(),
            "child must be taken by the first shutdown"
        );
        // Second call: the latch is already set, so this must return without
        // awaiting the budget at all. 200ms is far below the 2s budget but
        // generous enough to never flake on a loaded CI box.
        let started = std::time::Instant::now();
        host.shutdown()
            .await
            .expect("second shutdown is a no-op Ok");
        assert!(
            started.elapsed() < std::time::Duration::from_millis(200),
            "second shutdown must short-circuit on the latch, took {:?}",
            started.elapsed()
        );
    }

    /// Overlapping `shutdown` calls on two clones must both return `Ok` and
    /// exactly one of them must perform teardown (the other no-ops on the
    /// latch). This is the concurrency shape the dev server hits.
    #[tokio::test]
    #[cfg(unix)]
    async fn concurrent_shutdown_on_clones_both_ok_single_teardown() {
        let host = match stub_host_with_timeout(std::time::Duration::from_secs(120)).await {
            Some(h) => h,
            None => {
                eprintln!("skipping: `sleep` not available");
                return;
            }
        };
        let clone = host.clone();
        let watchdog = std::time::Duration::from_secs(8);
        let (a, b) = tokio::time::timeout(watchdog, async move {
            tokio::join!(host.shutdown(), clone.shutdown())
        })
        .await
        .expect("concurrent shutdown must finish within the watchdog");
        a.expect("clone A shutdown Ok");
        b.expect("clone B shutdown Ok");
        // No panic, no double-kill: reaching here with both Ok is the
        // assertion. (The child was already taken by whichever clone won.)
    }
}
