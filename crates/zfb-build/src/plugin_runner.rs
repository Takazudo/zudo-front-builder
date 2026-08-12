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

use crate::plugin_bundler::{self, EmbeddedEsbuildGetter, StagedPluginBundle};
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
    /// Set by [`PluginHost::force_kill_child`] (a deliberate, intentional
    /// teardown fired when a hook times out — see
    /// `request_typed_with_timeout`), *before* the kill so a reader task
    /// observing the resulting pipe EOF can tell it apart from an
    /// unannounced crash. Deliberately a SEPARATE flag from
    /// `shutting_down`: `shutdown()`'s latch has its own idempotency
    /// contract (first caller owns teardown, others no-op), and folding
    /// force-kill into it would let a mid-timeout force-kill silently
    /// satisfy a *different* caller's later `shutdown()` latch check (or
    /// vice versa) — two independent "this termination was intentional"
    /// facts stay independent flags (#2104 codex review, finding 1).
    /// [`HostInner::is_expected_termination`] is the single place both
    /// flags are read together.
    force_killed: AtomicBool,
    /// Latches the FIRST unexpected-death report so the stdout and stderr
    /// readers — which independently observe the same child dying and
    /// would otherwise each emit their own loud message — produce exactly
    /// ONE user-facing message per crash (#2104 codex review, finding 2).
    /// See [`HostInner::claim_death_report`].
    death_reported: AtomicBool,
    /// Reader-task join handle, kept so teardown can be *bounded*: a hung
    /// child only closes stdout (and thus ends the reader loop) when it
    /// actually dies, so shutdown joins this with a deadline instead of
    /// detaching and hoping. `None` once joined.
    reader_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Dropping the [`_tempdir`] removes the staged plugin-host script.
    _tempdir: tempfile::TempDir,
    /// Staged `.mjs` bundle artifacts produced for `.ts`/`.tsx`/`.mts`/
    /// `.cts` plugin entries (#2308). Held for the whole host session —
    /// same Drop-guard lifetime as `_tempdir` above — each staged file is
    /// deleted when the host (all its clones) is dropped. Empty when no
    /// spec needed bundling.
    _staged_plugin_bundles: Vec<StagedPluginBundle>,
    /// Maximum time any single plugin hook reply is awaited before the
    /// host is force-killed and the build fails with a diagnostic error.
    /// Env: ZFB_PLUGIN_HOOK_TIMEOUT (seconds). Default: 120s.
    hook_timeout: std::time::Duration,
}

impl HostInner {
    /// True when a pipe EOF is the expected result of a termination this
    /// process itself initiated — either a normal `shutdown()` or a
    /// hook-timeout `force_kill_child()` — as opposed to the child dying
    /// on its own (crash, OOM-kill, etc.). Both readers consult this
    /// through the single place so the two flags can never drift apart.
    fn is_expected_termination(&self) -> bool {
        self.shutting_down.load(Ordering::Acquire) || self.force_killed.load(Ordering::Acquire)
    }

    /// Claim the right to report an unexpected child death. Returns `true`
    /// for exactly the first caller (across both readers); every
    /// subsequent call — including from the *other* reader observing the
    /// same crash — returns `false` and must stay silent. This is what
    /// turns "stdout AND stderr both close on a crash" into one message,
    /// not two (#2104 codex review, finding 2).
    fn claim_death_report(&self) -> bool {
        !self.death_reported.swap(true, Ordering::AcqRel)
    }
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
        Self::spawn_with_timeout(plugins, node_binary, None, None).await
    }

    /// Like [`spawn`] but with an explicit hook timeout (used by the build
    /// orchestrator to thread `Config.pluginHookTimeoutSecs`) and an
    /// optional embedded-esbuild getter for bundling `.ts`/`.tsx`/`.mts`/
    /// `.cts` plugin entries (#2308).
    ///
    /// Before the host process boots, every spec whose resolved module is a
    /// TypeScript-family file (per [`plugin_bundler::needs_bundling`]) is
    /// bundled into a staged `.mjs` artifact and its `module` field is
    /// rewritten to the staged file's `file://` URL. Every other spec
    /// (`.js`/`.mjs`/`.cjs`, and bare packages that resolved to one of
    /// those) is left byte-identical — same plain-`import()` path the host
    /// always used.
    ///
    /// `embedded_esbuild_getter` is consulted to resolve the esbuild binary
    /// **only when at least one spec needs bundling**: a pure-JS/`.mjs`
    /// plugin set never touches esbuild resolution, so it gains no new
    /// failure mode from this change. Pass `None` when no packaged-build
    /// embedded getter is available — the env var / workspace binary slot
    /// tiers still apply (see [`plugin_bundler::resolve_esbuild_for_plugins`]).
    pub async fn spawn_with_timeout(
        mut plugins: Vec<PluginSpec>,
        node_binary: Option<OsString>,
        hook_timeout_secs: Option<u64>,
        embedded_esbuild_getter: Option<EmbeddedEsbuildGetter>,
    ) -> Result<Self> {
        let hook_timeout = resolve_hook_timeout(hook_timeout_secs);

        // Bundle every `.ts`/`.tsx`/`.mts`/`.cts` plugin entry into a staged
        // `.mjs` artifact before the host ever imports anything (#2308).
        // Laziness is load-bearing: esbuild resolution only runs when at
        // least one spec passes `needs_bundling`, so a pure-JS plugin set
        // never touches it.
        let mut staged_bundles: Vec<StagedPluginBundle> = Vec::new();
        if plugins
            .iter()
            .any(|spec| plugin_bundler::needs_bundling(&spec.module))
        {
            // Keep the resolved esbuild binary — and its extraction
            // TempDir, if the embedded tier supplied one — alive across
            // EVERY bundling call in the loop below. Dropping it early
            // would delete a staged embedded-tier esbuild binary mid-loop.
            let (_esbuild_tempdir, esbuild_bin) =
                plugin_bundler::resolve_esbuild_for_plugins(embedded_esbuild_getter)?;

            // Cache staged bundles by the plugin's ORIGINAL resolved
            // module URL (codex review finding 2, #2324): before this
            // cache, two `plugins` entries naming the same TS-family
            // module each got their OWN separately-named staged `.mjs`
            // file, so the host imported two distinct URLs and evaluated
            // the module's top-level side effects twice — diverging from
            // the pre-existing `.js`/`.mjs`/`.cjs` behaviour, where
            // repeated `import()` of the identical resolved URL string
            // hits Node's module cache and evaluates it exactly once (see
            // `duplicate_ts_plugin_specs_share_one_staged_bundle_and_evaluate_once_through_real_host`
            // for the measured pre-existing `.mjs` semantics this cache
            // matches). Handing duplicate specs the SAME staged URL
            // restores that parity for TS-family entries too.
            let mut staged_by_original_module: HashMap<String, String> = HashMap::new();
            for spec in plugins.iter_mut() {
                if !plugin_bundler::needs_bundling(&spec.module) {
                    continue;
                }
                if let Some(staged_url) = staged_by_original_module.get(&spec.module) {
                    spec.module = staged_url.clone();
                    continue;
                }
                let original_module = spec.module.clone();
                let entry = url::Url::parse(&spec.module)
                    .ok()
                    .and_then(|u| u.to_file_path().ok())
                    .ok_or_else(|| {
                        anyhow!(
                            "plugin bundling: could not resolve a filesystem path from module \
                             specifier `{}` for plugin `{}`",
                            spec.module,
                            spec.name
                        )
                    })?;
                let staged =
                    plugin_bundler::bundle_plugin_entry(&entry, &esbuild_bin, &spec.name).await?;
                let staged_url = staged.file_url().to_string();
                spec.module = staged_url.clone();
                staged_by_original_module.insert(original_module, staged_url);
                staged_bundles.push(staged);
            }
        }

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
            // Bundled entries carry `--sourcemap=inline` (#2308) so a
            // bundled plugin's *runtime* throw maps back to its original
            // `.ts` source in stack traces. Harmless for plain `.mjs`
            // plugins — the flag only changes how Node renders a thrown
            // error's stack.
            .arg("--enable-source-maps")
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
            force_killed: AtomicBool::new(false),
            death_reported: AtomicBool::new(false),
            reader_handle: Mutex::new(None),
            _tempdir: tmp,
            _staged_plugin_bundles: staged_bundles,
            hook_timeout,
        });

        // Reader task — drains stdout, dispatches replies to the
        // matching pending sender, and forwards log lines into tracing.
        //
        // Disposition (issue #2104, epic #2099): today, once this loop
        // ends (EOF or a read error), pending/future hook calls just fail
        // one request at a time via the synthetic "stdout closed before
        // reply" error below — nothing announces the process actually
        // died. `shutdown()` flips `shutting_down` to `true` *before* it
        // kills the child, and a hook-timeout `force_kill_child()` flips
        // the separate `force_killed` flag before ITS kill, so an EOF
        // observed while either flag is set (`HostInner::
        // is_expected_termination()`) is the expected close of an
        // intentional termination and stays silent; an EOF observed with
        // both still `false` means the child died unannounced (crash,
        // OOM-kill, etc.). Of those unexpected closes, only whichever
        // reader wins `HostInner::claim_death_report()` — this one or the
        // sibling stderr reader below, both of which see the SAME crash —
        // emits ONE loud `tracing::error!` (codex review, #2104: a
        // force-kill must not pile a misleading death message on top of
        // the accurate timeout diagnostic already being returned, and a
        // crash closing both pipes must not double-report). (Not
        // `zfb::output::error` — this crate sits below `zfb` in the
        // dependency graph and can't reach its CLI-output helpers;
        // `tracing::error!` is this crate's own convention for a loud
        // signal, matching the plugin `"error"`-level log passthrough in
        // `handle_line` above.) The loop body lives in
        // [`Self::run_stdout_reader`] so the EOF-suppression logic is
        // unit-testable without a real `node` child.
        let inner_for_reader = Arc::clone(&inner);
        let reader_handle = tokio::spawn(Self::run_stdout_reader(inner_for_reader, stdout));
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
        //
        // Disposition (issue #2104, epic #2099): this task is separately
        // detached from the stdout reader above (no `JoinHandle` is
        // stashed anywhere for it, and `shutdown()` never joins it) — but
        // it gets the SAME EOF disposition as the stdout reader rather
        // than a silently different one just because it's a separate
        // task: silence when this pipe closes as the expected result of a
        // `shutdown()` or hook-timeout `force_kill_child()` already in
        // flight, and — of an unexpected close — a loud `tracing::error!`
        // ONLY if this reader wins the `claim_death_report()` race against
        // the stdout reader observing the same crash (codex review,
        // #2104). It needs its own `Arc<HostInner>` clone purely to read
        // that shared state. The loop body lives in
        // [`Self::run_stderr_reader`] alongside its stdout counterpart for
        // the same testability reason.
        let inner_for_stderr = Arc::clone(&inner);
        tokio::spawn(Self::run_stderr_reader(inner_for_stderr, stderr));

        let host = PluginHost { inner };

        // Send `init` and wait for the loaded-count reply. Surface a
        // module-load failure as `PluginError`.
        let result: serde_json::Value = match host
            .request_typed(
                "init",
                serde_json::json!({
                    "plugins": plugins,
                }),
            )
            .await
        {
            Ok(v) => v,
            Err(e) => {
                // `init` returning `ok: false` doesn't crash the child — it
                // stays alive, so the stdout/stderr reader tasks (each
                // holding their own `Arc<HostInner>` clone) never see EOF
                // and never exit. Letting `host` merely fall out of scope
                // here would leak the child process AND everything
                // `HostInner`'s Drop would otherwise clean up — the staged
                // plugin-host tempdir and, since #2308, every staged `.ts`
                // bundle artifact. Force-kill so the readers observe EOF,
                // drop their clones, and the Arc's refcount can reach zero.
                host.force_kill_child().await;
                return Err(e);
            }
        };
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
    /// build: a preset that owns the site index calls `injectRoute("/")` in
    /// either mode. Downstream route selection gives a matching user
    /// `pages/index` precedence; otherwise the injected root is materialized
    /// in build and staged for dev rendering.
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
                /// `addVirtualModule`'s optional `{ watchFiles }` option
                /// (#2167). `#[serde(default)]` keeps this backward
                /// compatible with a hypothetical older host build that
                /// never emits the field.
                #[serde(rename = "watchFiles", default)]
                watch_files: Vec<String>,
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
                        watch_files,
                    } => RawSetupRegistration::VirtualModule {
                        specifier,
                        loader_id: VirtualLoaderId(loader_id),
                        watch_files,
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
        self.invoke_virtual_loader_via(loader_id, false).await
    }

    /// Like [`PluginHost::invoke_virtual_loader`] but sends `force: true`
    /// (#2167), telling the JS host to bypass `virtualCache` and
    /// re-invoke the loader, refreshing the memoised entry with the
    /// fresh result.
    ///
    /// Intended for a loader whose `addVirtualModule` registration also
    /// declared `{ watchFiles }` — [`crate::plugin_refresh::PluginRefreshState::refresh`]
    /// (issue #2168) calls this when one of those files changes, so the
    /// next import of the virtual module sees the fresh content instead of
    /// the first-call-wins cached value `invoke_virtual_loader` returns.
    /// `refresh` itself is not yet wired to a live watcher tick — see that
    /// module's doc comment.
    pub async fn invoke_virtual_loader_forced(
        &self,
        loader_id: &VirtualLoaderId,
    ) -> Result<String> {
        self.invoke_virtual_loader_via(loader_id, true).await
    }

    /// Shared implementation behind [`PluginHost::invoke_virtual_loader`] /
    /// [`PluginHost::invoke_virtual_loader_forced`] (#2167).
    async fn invoke_virtual_loader_via(
        &self,
        loader_id: &VirtualLoaderId,
        force: bool,
    ) -> Result<String> {
        #[derive(Deserialize)]
        struct Reply {
            source: String,
        }
        let reply: Reply = self
            .request_typed(
                "virtualLoad",
                serde_json::json!({ "loaderId": loader_id.as_str(), "force": force }),
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
    ///
    /// Deliberately intentional termination: flips `force_killed` *before*
    /// killing (same ordering discipline as `shutdown()` flipping
    /// `shutting_down` before its own teardown) so the reader tasks'
    /// resulting EOF is recognised as expected rather than reported as an
    /// unannounced crash on top of the timeout diagnostic
    /// `request_typed_with_timeout` is about to return (#2104 codex
    /// review, finding 1). Does NOT touch `shutting_down` — a force-kill
    /// is not a `shutdown()` call and must not satisfy that latch.
    async fn force_kill_child(&self) {
        self.inner.force_killed.store(true, Ordering::Release);
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

    /// Drain the child's stdout: dispatch replies/log lines to
    /// [`Self::handle_line`], then on EOF (or a read error) emit the loud
    /// death message — but only when the close is BOTH unexpected
    /// (`HostInner::is_expected_termination()` is false, i.e. neither a
    /// `shutdown()` nor a hook-timeout `force_kill_child()` is already in
    /// flight) AND this reader wins the race to claim it
    /// (`HostInner::claim_death_report()`) against the sibling stderr
    /// reader observing the SAME crash. See the disposition comment at
    /// the spawn site in [`Self::spawn_with_timeout`] for the full
    /// rationale (#2104), and `HostInner`'s field docs for why
    /// force-kill and the single-report claim are separate flags from
    /// `shutting_down` (#2104 codex review, findings 1 and 2).
    ///
    /// Emitted through BOTH `tracing::error!` and `eprintln!`, matching
    /// this crate's existing dual-channel convention for a loud message a
    /// real `zfb` CLI user must see (see
    /// `skip_dangling_symlink_or_fail` in `bundler.rs`): no
    /// `tracing_subscriber` is installed anywhere in the `zfb` binary, so
    /// a bare `tracing::error!` alone is a silent no-op in production —
    /// only tests that install their own subscriber (`#[traced_test]`)
    /// would ever see it. `eprintln!` is the channel production users
    /// actually see (codex review, #2104).
    async fn run_stdout_reader(inner: Arc<HostInner>, stdout: tokio::process::ChildStdout) {
        let mut lines = BufReader::new(stdout).lines();
        let unexpected_eof;
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    Self::handle_line(&inner, &line).await;
                    continue;
                }
                Ok(None) => {
                    debug!("plugin host: stdout closed");
                    unexpected_eof = !inner.is_expected_termination();
                }
                Err(e) => {
                    warn!(error = %e, "plugin host: stdout read error");
                    unexpected_eof = !inner.is_expected_termination();
                }
            }
            break;
        }
        if unexpected_eof && inner.claim_death_report() {
            let msg = "plugin host: stdout reader ended unexpectedly (plugin-host process \
                 likely died) — pending and future plugin hook calls will fail";
            error!("{msg}");
            eprintln!("zfb error: {msg}");
        }
        // Wake every still-pending caller with a synthetic close error so
        // we don't deadlock on a child that died early.
        let mut pend = inner.pending.lock().await;
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
    }

    /// Drain the child's stderr into `tracing::warn!` per line, with the
    /// SAME EOF disposition as [`Self::run_stdout_reader`] — expected
    /// (shutdown or force-kill) closes stay silent, and of the unexpected
    /// closes only whichever reader wins `claim_death_report()` emits the
    /// loud message (same dual `tracing::error!` + `eprintln!` channel,
    /// same rationale) (#2104, codex review findings 1 and 2).
    async fn run_stderr_reader(inner: Arc<HostInner>, stderr: tokio::process::ChildStderr) {
        let mut lines = BufReader::new(stderr).lines();
        let unexpected_eof;
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if !line.trim().is_empty() {
                        warn!(target: "zfb_plugin", "{line}");
                        eprintln!("{}", Self::format_plugin_host_warn_line("stderr", &line));
                    }
                    continue;
                }
                Ok(None) => {
                    unexpected_eof = !inner.is_expected_termination();
                }
                Err(_) => {
                    unexpected_eof = !inner.is_expected_termination();
                }
            }
            break;
        }
        if unexpected_eof && inner.claim_death_report() {
            let msg =
                "plugin host: stderr reader ended unexpectedly (plugin-host process likely died)";
            error!("{msg}");
            eprintln!("zfb error: {msg}");
        }
    }

    /// Format a plugin `{log:{level,plugin,message}}` envelope for the
    /// visible `eprintln!` channel, matching this crate's dual-channel
    /// convention (`tracing` + `eprintln!`, see [`Self::run_stdout_reader`]'s
    /// doc comment for #2104's rationale — no `tracing_subscriber` is
    /// installed anywhere in the `zfb` binary, so `eprintln!` is the
    /// channel production users actually see). `level` is the already-
    /// normalised label (`"warn"`/`"error"`/`"info"`) a caller matched on,
    /// not the raw, unvalidated `log.level` string from the wire.
    fn format_plugin_log_line(level: &str, plugin: &str, message: &str) -> String {
        format!("zfb {level}: [plugin:{plugin}] {message}")
    }

    /// Format a plugin-host line that has no plugin to attribute — either
    /// a raw stderr write from the host subprocess, or a non-JSON stdout
    /// line that failed to parse as a `{log:...}`/reply envelope — for the
    /// same visible `eprintln!` channel. `source` names which pipe it came
    /// from (`"stderr"` / `"stdout"`) so a reader can tell the two apart.
    fn format_plugin_host_warn_line(source: &str, detail: &str) -> String {
        format!("zfb warn: [plugin-host {source}] {detail}")
    }

    async fn handle_line(inner: &Arc<HostInner>, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        let parsed: HostLine = match serde_json::from_str(line) {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, raw = %line, "plugin host: failed to parse stdout line");
                eprintln!("{}", Self::format_plugin_host_warn_line("stdout", line));
                return;
            }
        };
        match parsed {
            HostLine::Log(LogLine { log }) => match log.level.as_str() {
                "warn" => {
                    warn!(target: "zfb_plugin", plugin = %log.plugin, "{}", log.message);
                    eprintln!(
                        "{}",
                        Self::format_plugin_log_line("warn", &log.plugin, &log.message)
                    );
                }
                "error" => {
                    error!(target: "zfb_plugin", plugin = %log.plugin, "{}", log.message);
                    eprintln!(
                        "{}",
                        Self::format_plugin_log_line("error", &log.plugin, &log.message)
                    );
                }
                _ => {
                    info!(target: "zfb_plugin", plugin = %log.plugin, "{}", log.message);
                    eprintln!(
                        "{}",
                        Self::format_plugin_log_line("info", &log.plugin, &log.message)
                    );
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

    // --- Visible-channel line formatting (issue #2369) -----------------
    //
    // Pure helpers, so these assert the exact rendered text directly
    // rather than trying to capture in-process `eprintln!` output (awkward
    // and race-prone with parallel test execution sharing one stderr).

    #[test]
    fn format_plugin_log_line_attributes_plugin_and_level_for_all_three_levels() {
        assert_eq!(
            PluginHost::format_plugin_log_line("info", "my-plugin", "hello"),
            "zfb info: [plugin:my-plugin] hello"
        );
        assert_eq!(
            PluginHost::format_plugin_log_line("warn", "my-plugin", "careful"),
            "zfb warn: [plugin:my-plugin] careful"
        );
        assert_eq!(
            PluginHost::format_plugin_log_line("error", "my-plugin", "boom"),
            "zfb error: [plugin:my-plugin] boom"
        );
    }

    #[test]
    fn format_plugin_host_warn_line_tags_the_source_pipe() {
        assert_eq!(
            PluginHost::format_plugin_host_warn_line("stderr", "uncaught at plugin-host.mjs:12"),
            "zfb warn: [plugin-host stderr] uncaught at plugin-host.mjs:12"
        );
        assert_eq!(
            PluginHost::format_plugin_host_warn_line("stdout", "not json at all"),
            "zfb warn: [plugin-host stdout] not json at all"
        );
    }

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
                // A per-call-incrementing counter so a cache-hit (loader
                // NOT re-invoked) and a forced reload (loader IS
                // re-invoked) are observably distinguishable (#2167).
                let calls = 0;
                addVirtualModule("virtual:data", () => {
                  calls += 1;
                  return `export default ${calls}`;
                });
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
        assert!(
            vm.watch_files.is_empty(),
            "no watchFiles option was supplied, so the registry entry must be empty"
        );
        // #2167 contract amendment: an unforced load still cache-hits
        // (the loader is NOT re-invoked — `calls` stays at 1); a forced
        // load bypasses the cache and DOES re-invoke the loader,
        // refreshing the memoised entry; a subsequent unforced load then
        // cache-hits the freshly-refreshed value rather than the stale
        // pre-force one.
        let first = host.invoke_virtual_loader(&vm.loader_id).await.unwrap();
        assert_eq!(first, "export default 1");
        let second = host.invoke_virtual_loader(&vm.loader_id).await.unwrap();
        assert_eq!(
            second, first,
            "unforced load must still cache-hit and not re-invoke the loader"
        );
        let forced = host
            .invoke_virtual_loader_forced(&vm.loader_id)
            .await
            .unwrap();
        assert_eq!(
            forced, "export default 2",
            "force:true must bypass the cache and re-invoke the loader"
        );
        let after_force = host.invoke_virtual_loader(&vm.loader_id).await.unwrap();
        assert_eq!(
            after_force, forced,
            "an unforced load after a forced reload must cache-hit the refreshed value"
        );
        assert_eq!(regs.injected_routes.len(), 1);
        let r = &regs.injected_routes.as_slice()[0];
        assert_eq!(r.pattern, "/dev/x");
        assert_eq!(r.entrypoint, tmp.path().join("scripts/x.ts"));
        host.shutdown().await.ok();
    }

    #[tokio::test]
    async fn setup_hook_virtual_module_watch_files_round_trip() {
        // #2167: `addVirtualModule`'s optional third arg `{ watchFiles }`
        // must survive the host -> Rust wire hop into
        // `VirtualModuleEntry::watch_files`.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("watcher.mjs");
        let watch_a = tmp.path().join("watch-a.json");
        let watch_b = tmp.path().join("watch-b.json");
        let plugin_src = format!(
            r#"
            export default {{
              name: "watcher",
              setup({{ addVirtualModule }}) {{
                addVirtualModule("virtual:watched", () => 'export default 1', {{
                  watchFiles: [{a:?}, {b:?}],
                }});
              }},
            }};
            "#,
            a = watch_a.to_string_lossy().to_string(),
            b = watch_b.to_string_lossy().to_string(),
        );
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();
        let host = PluginHost::spawn(
            vec![PluginSpec {
                name: "watcher".into(),
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
                crate::plugin_registries::SetupCommand::Dev,
                &serde_json::json!({}),
            )
            .await
            .expect("setup ok");
        let vm = regs
            .virtual_modules
            .get("virtual:watched")
            .expect("vm registered");
        assert_eq!(vm.watch_files, vec![watch_a.clone(), watch_b.clone()]);
        host.shutdown().await.ok();
    }

    #[tokio::test]
    async fn setup_hook_virtual_module_rejects_relative_watch_files() {
        // #2167: absolute-only enforcement lives host-side
        // (`plugin-host.mjs`'s `addVirtualModule`), mirroring
        // `extraWatchPaths`'s absolute-only rule
        // (`crates/zfb/src/config.rs`). A relative entry must be
        // rejected rather than silently accepted and later
        // misinterpreted by a dev-server watcher with no defined base
        // directory to resolve it against.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("relwatcher.mjs");
        tokio::fs::write(
            &plugin_path,
            r#"
            export default {
              name: "relwatcher",
              setup({ addVirtualModule }) {
                addVirtualModule("virtual:rel", () => 'export default 1', {
                  watchFiles: ["./relative/path.json"],
                });
              },
            };
            "#,
        )
        .await
        .unwrap();
        let host = PluginHost::spawn(
            vec![PluginSpec {
                name: "relwatcher".into(),
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
                crate::plugin_registries::SetupCommand::Dev,
                &serde_json::json!({}),
            )
            .await
            .expect_err("a relative watchFiles entry must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("absolute"),
            "expected an absolute-path error, got: {msg}"
        );
        host.shutdown().await.ok();
    }

    #[tokio::test]
    async fn setup_hook_virtual_module_rejects_directory_watch_files() {
        // #2373: a directory watchFiles entry was silently accepted but
        // permanently inert — the Rust side registers a NON-recursive
        // file-parent watch and matches ownership by exact path, so
        // nothing under the directory was ever actually watched. This is
        // a deliberate validation tightening (previously silently inert
        // — issue #2367 confirms nothing could ever have relied on it
        // working), not a behavior change plugins could depend on.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let watched_dir = tmp.path().join("watched-dir");
        tokio::fs::create_dir(&watched_dir).await.unwrap();
        let plugin_path = tmp.path().join("dirwatcher.mjs");
        let plugin_src = format!(
            r#"
            export default {{
              name: "dirwatcher",
              setup({{ addVirtualModule }}) {{
                addVirtualModule("virtual:dir", () => 'export default 1', {{
                  watchFiles: [{dir:?}],
                }});
              }},
            }};
            "#,
            dir = watched_dir.to_string_lossy().to_string(),
        );
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();
        let host = PluginHost::spawn(
            vec![PluginSpec {
                name: "dirwatcher".into(),
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
                crate::plugin_registries::SetupCommand::Dev,
                &serde_json::json!({}),
            )
            .await
            .expect_err("a directory watchFiles entry must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("directories are not watched recursively"),
            "expected a directory-rejection error, got: {msg}"
        );
        host.shutdown().await.ok();
    }

    #[tokio::test]
    async fn console_log_from_plugin_arrives_as_a_log_envelope_attributed_to_the_plugin() {
        // #2373: drives `plugin-host.mjs` directly over its stdio wire
        // protocol rather than through `PluginHost` (whose `{log:...}`
        // dispatch in `handle_line` emits under the `target: "zfb_plugin"`
        // tracing target — invisible to `tracing-test`'s crate-scoped
        // default filter, so `logs_contain` can't observe it here). Going
        // straight to the wire lets this test assert on the EXACT
        // envelope the host emits: proof a redirected `console.log`
        // arrives as a well-formed `{"log":{"level":"info","plugin":...,
        // "message":...}}` line — not a raw, unparseable line that would
        // fall into the Rust side's parse-failure warning path — and that
        // `plugin` correctly names the emitting plugin.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("console-plugin.mjs");
        tokio::fs::write(
            &plugin_path,
            r#"
            export default {
              name: "console-plugin",
              preBuild() {
                console.log("hello from console.log");
              },
            };
            "#,
        )
        .await
        .unwrap();
        let host_path = tmp.path().join("plugin-host.mjs");
        tokio::fs::write(&host_path, PLUGIN_HOST_MJS).await.unwrap();

        let mut child = Command::new("node")
            .arg("--enable-source-maps")
            .arg(&host_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("node spawns");
        let mut stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let mut lines = BufReader::new(stdout).lines();

        let init_msg = serde_json::json!({
            "id": 1,
            "kind": "init",
            "plugins": [{
                "module": file_url_for_test(&plugin_path),
                "name": "console-plugin",
                "options": {},
            }],
        });
        stdin
            .write_all(format!("{init_msg}\n").as_bytes())
            .await
            .unwrap();
        let init_reply = lines.next_line().await.unwrap().expect("init reply line");
        let init_reply: serde_json::Value = serde_json::from_str(&init_reply).unwrap();
        assert_eq!(
            init_reply["ok"],
            serde_json::json!(true),
            "init failed: {init_reply}"
        );

        let pre_build_msg = serde_json::json!({
            "id": 2,
            "kind": "preBuild",
            "ctx": {
                "projectRoot": tmp.path(),
                "outDir": tmp.path().join("dist"),
                "config": {},
            },
        });
        stdin
            .write_all(format!("{pre_build_msg}\n").as_bytes())
            .await
            .unwrap();

        // The `console.log` call happens synchronously before `preBuild`
        // returns, so its log-envelope line is written to stdout before
        // the `preBuild` reply line — the same ordering the Rust reader
        // relies on in production.
        let log_line = lines.next_line().await.unwrap().expect("log line");
        let log_json: serde_json::Value = serde_json::from_str(&log_line).unwrap_or_else(|e| {
            panic!(
                "log line must be well-formed JSON, not a raw parse-failure line: {e}\nline: {log_line}"
            )
        });
        assert_eq!(
            log_json,
            serde_json::json!({
                "log": {
                    "level": "info",
                    "plugin": "console-plugin",
                    "message": "hello from console.log",
                }
            }),
        );

        let reply_line = lines
            .next_line()
            .await
            .unwrap()
            .expect("preBuild reply line");
        let reply_json: serde_json::Value = serde_json::from_str(&reply_line).unwrap();
        assert_eq!(
            reply_json["ok"],
            serde_json::json!(true),
            "preBuild failed: {reply_json}"
        );

        stdin
            .write_all(b"{\"id\":3,\"kind\":\"shutdown\"}\n")
            .await
            .unwrap();
        let _ = lines.next_line().await;
        let _ = child.wait().await;
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
        // A preset's setup runs in both `zfb dev` and `zfb build`, so a
        // package may register `injectRoute("/")` in either mode. This host
        // test pins registration acceptance only; downstream route selection
        // gives a matching user `pages/index` precedence in both modes.
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

        // Dev: accepted by the host. Rendering and user-page precedence are
        // handled later by the route resolver and request dispatch.
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
            force_killed: AtomicBool::new(false),
            death_reported: AtomicBool::new(false),
            reader_handle: Mutex::new(None),
            _tempdir: tmp,
            _staged_plugin_bundles: Vec::new(),
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

    // --- Reader EOF disposition (#2104, no node required) --------------------
    //
    // `run_stdout_reader` / `run_stderr_reader` emit exactly one loud
    // `tracing::error!` when their pipe closes unexpectedly (child died
    // without `shutdown()` having flipped `shutting_down` first), and stay
    // silent when the same EOF happens during a normal `shutdown()` call.
    // These drive the extracted reader loops directly against a real but
    // trivial child (`true`, which exits immediately and closes both
    // pipes) so the EOF path fires deterministically without needing
    // `node` — Level 1, T0.

    /// Build a minimal `Arc<HostInner>` plus a `true`-child's stdout/stderr
    /// pipes. `true` exits immediately, so both pipes hit EOF as soon as
    /// the reader starts reading — no node, no fixed sleep.
    #[cfg(unix)]
    async fn stub_inner_with_true_child() -> Option<(
        Arc<HostInner>,
        tokio::process::ChildStdout,
        tokio::process::ChildStderr,
    )> {
        let mut child = match tokio::process::Command::new("true")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return None, // `true` not available (extremely rare)
        };
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let inner = Arc::new(HostInner {
            pending: Mutex::new(HashMap::new()),
            stdin: Mutex::new(stdin),
            next_id: AtomicU64::new(1),
            child: Mutex::new(Some(child)),
            shutting_down: AtomicBool::new(false),
            force_killed: AtomicBool::new(false),
            death_reported: AtomicBool::new(false),
            reader_handle: Mutex::new(None),
            _tempdir: tmp,
            _staged_plugin_bundles: Vec::new(),
            hook_timeout: std::time::Duration::from_secs(1),
        });
        Some((inner, stdout, stderr))
    }

    /// `shutting_down = false` (never called `shutdown()`) is the
    /// unexpected-death case: the stdout EOF must produce exactly one
    /// loud `tracing::error!` naming the reader as ended unexpectedly.
    #[tokio::test]
    #[cfg(unix)]
    #[tracing_test::traced_test]
    async fn stdout_reader_logs_loud_error_on_eof_when_not_shutting_down() {
        let Some((inner, stdout, _stderr)) = stub_inner_with_true_child().await else {
            eprintln!("skipping: `true` not available");
            return;
        };
        assert!(!inner.shutting_down.load(Ordering::Acquire));
        let watchdog = std::time::Duration::from_secs(5);
        tokio::time::timeout(watchdog, PluginHost::run_stdout_reader(inner, stdout))
            .await
            .expect("stdout reader must return once the pipe closes");
        assert!(
            logs_contain("stdout reader ended unexpectedly"),
            "an unexpected stdout EOF must produce exactly one loud error message"
        );
    }

    /// Same EOF, but `shutting_down = true` (as `shutdown()` sets it
    /// before tearing the child down) — the expected close of a normal,
    /// intentional shutdown must NOT produce the loud error message.
    #[tokio::test]
    #[cfg(unix)]
    #[tracing_test::traced_test]
    async fn stdout_reader_stays_silent_on_eof_during_shutdown() {
        let Some((inner, stdout, _stderr)) = stub_inner_with_true_child().await else {
            eprintln!("skipping: `true` not available");
            return;
        };
        inner.shutting_down.store(true, Ordering::Release);
        let watchdog = std::time::Duration::from_secs(5);
        tokio::time::timeout(watchdog, PluginHost::run_stdout_reader(inner, stdout))
            .await
            .expect("stdout reader must return once the pipe closes");
        assert!(
            !logs_contain("stdout reader ended unexpectedly"),
            "a shutdown-initiated stdout EOF must not turn routine shutdown into noise"
        );
    }

    /// The stderr reader gets the identical disposition treatment as the
    /// stdout reader (#2104's requirement that both readers behave the
    /// same, not silently diverge because they're separate tasks).
    #[tokio::test]
    #[cfg(unix)]
    #[tracing_test::traced_test]
    async fn stderr_reader_logs_loud_error_on_eof_when_not_shutting_down() {
        let Some((inner, _stdout, stderr)) = stub_inner_with_true_child().await else {
            eprintln!("skipping: `true` not available");
            return;
        };
        assert!(!inner.shutting_down.load(Ordering::Acquire));
        let watchdog = std::time::Duration::from_secs(5);
        tokio::time::timeout(watchdog, PluginHost::run_stderr_reader(inner, stderr))
            .await
            .expect("stderr reader must return once the pipe closes");
        assert!(
            logs_contain("stderr reader ended unexpectedly"),
            "an unexpected stderr EOF must produce exactly one loud error message"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    #[tracing_test::traced_test]
    async fn stderr_reader_stays_silent_on_eof_during_shutdown() {
        let Some((inner, _stdout, stderr)) = stub_inner_with_true_child().await else {
            eprintln!("skipping: `true` not available");
            return;
        };
        inner.shutting_down.store(true, Ordering::Release);
        let watchdog = std::time::Duration::from_secs(5);
        tokio::time::timeout(watchdog, PluginHost::run_stderr_reader(inner, stderr))
            .await
            .expect("stderr reader must return once the pipe closes");
        assert!(
            !logs_contain("stderr reader ended unexpectedly"),
            "a shutdown-initiated stderr EOF must not turn routine shutdown into noise"
        );
    }

    // --- Force-kill / single-report EOF suppression (#2104 codex review) -----
    //
    // Finding 1: `force_kill_child()` (fired by a hook timeout) must not let
    // the reader's resulting EOF pile a misleading "process likely died"
    // alarm on top of the accurate timeout diagnostic already being
    // returned. Finding 2: an unexpected crash closes BOTH stdout and
    // stderr, but only ONE of the two readers may report it. Level 1, T0 —
    // no `node` required.

    /// `HostInner::claim_death_report` is the pure shared-state primitive
    /// both readers race on: exactly the first caller gets `true`, every
    /// later caller (including a differently-ordered interleaving) gets
    /// `false`. Reuses the `true`-child stub purely for a valid
    /// `Arc<HostInner>` — no reader task is driven here, this pins the
    /// primitive in isolation from the I/O plumbing around it.
    #[tokio::test]
    #[cfg(unix)]
    async fn claim_death_report_returns_true_exactly_once() {
        let Some((inner, _stdout, _stderr)) = stub_inner_with_true_child().await else {
            eprintln!("skipping: `true` not available");
            return;
        };
        assert!(
            inner.claim_death_report(),
            "the first claim must win the race"
        );
        assert!(
            !inner.claim_death_report(),
            "a second claim (the sibling reader observing the same crash) must lose"
        );
        assert!(
            !inner.claim_death_report(),
            "every subsequent claim must also lose — this is a one-shot latch"
        );
    }

    /// `is_expected_termination` must recognise a force-kill as expected
    /// even though `shutting_down` (the `shutdown()` latch) was never
    /// touched — the two flags are deliberately independent (finding 1).
    #[tokio::test]
    #[cfg(unix)]
    async fn is_expected_termination_recognises_force_kill_independently_of_shutdown_latch() {
        let Some((inner, _stdout, _stderr)) = stub_inner_with_true_child().await else {
            eprintln!("skipping: `true` not available");
            return;
        };
        assert!(!inner.is_expected_termination());
        inner.force_killed.store(true, Ordering::Release);
        assert!(
            inner.is_expected_termination(),
            "force_killed alone must satisfy the expected-termination check"
        );
        assert!(
            !inner.shutting_down.load(Ordering::Acquire),
            "sanity: setting force_killed must not touch the separate shutdown() latch"
        );
    }

    /// A hook timeout's `force_kill_child()` must suppress the stdout
    /// reader's EOF alarm — the exact regression the codex review's
    /// finding 1 named: a force-kill closing the pipe used to still look
    /// like an unannounced crash on top of the timeout error already
    /// returned to the caller.
    #[tokio::test]
    #[cfg(unix)]
    #[tracing_test::traced_test]
    async fn force_kill_child_suppresses_the_stdout_eof_alarm() {
        let Some((inner, stdout, _stderr)) = stub_inner_with_true_child().await else {
            eprintln!("skipping: `true` not available");
            return;
        };
        let host = PluginHost {
            inner: Arc::clone(&inner),
        };
        // Simulate the hook-timeout path: force-kill without ever calling
        // `shutdown()` (`shutting_down` stays false the whole test).
        host.force_kill_child().await;
        assert!(
            inner.force_killed.load(Ordering::Acquire),
            "force_kill_child must flip force_killed before killing"
        );
        assert!(
            !inner.shutting_down.load(Ordering::Acquire),
            "force-kill must NOT satisfy the separate shutdown() latch"
        );
        let watchdog = std::time::Duration::from_secs(5);
        tokio::time::timeout(watchdog, PluginHost::run_stdout_reader(inner, stdout))
            .await
            .expect("stdout reader must return once the pipe closes");
        assert!(
            !logs_contain("stdout reader ended unexpectedly"),
            "a force-kill-initiated EOF must not be reported as an unannounced crash"
        );
    }

    /// Same suppression, stderr side — the stderr reader must independently
    /// honour the same force-kill flag.
    #[tokio::test]
    #[cfg(unix)]
    #[tracing_test::traced_test]
    async fn force_kill_child_suppresses_the_stderr_eof_alarm() {
        let Some((inner, _stdout, stderr)) = stub_inner_with_true_child().await else {
            eprintln!("skipping: `true` not available");
            return;
        };
        let host = PluginHost {
            inner: Arc::clone(&inner),
        };
        host.force_kill_child().await;
        let watchdog = std::time::Duration::from_secs(5);
        tokio::time::timeout(watchdog, PluginHost::run_stderr_reader(inner, stderr))
            .await
            .expect("stderr reader must return once the pipe closes");
        assert!(
            !logs_contain("stderr reader ended unexpectedly"),
            "a force-kill-initiated EOF must not be reported as an unannounced crash"
        );
    }

    /// An unexpected death closes BOTH stdout and stderr — codex review
    /// finding 2 was that each reader independently reported it, producing
    /// two near-identical user-facing errors for one crash. Running both
    /// readers concurrently against the SAME `true`-child's pipes must
    /// produce exactly ONE "process likely died" line, not two.
    #[tokio::test]
    #[cfg(unix)]
    #[tracing_test::traced_test]
    async fn unexpected_death_is_reported_exactly_once_not_twice() {
        let Some((inner, stdout, stderr)) = stub_inner_with_true_child().await else {
            eprintln!("skipping: `true` not available");
            return;
        };
        assert!(!inner.shutting_down.load(Ordering::Acquire));
        assert!(!inner.force_killed.load(Ordering::Acquire));
        let inner_for_stdout = Arc::clone(&inner);
        let watchdog = std::time::Duration::from_secs(5);
        tokio::time::timeout(watchdog, async {
            tokio::join!(
                PluginHost::run_stdout_reader(inner_for_stdout, stdout),
                PluginHost::run_stderr_reader(inner, stderr)
            )
        })
        .await
        .expect("both readers must return once their pipes close");
        logs_assert(|lines| {
            let hits = lines
                .iter()
                .filter(|line| line.contains("process likely died"))
                .count();
            if hits == 1 {
                Ok(())
            } else {
                Err(format!(
                    "expected exactly one \"process likely died\" message, found {hits}: {lines:?}"
                ))
            }
        });
    }

    // --- #2308 plugin TS bundling wiring integration tests -----------------

    /// Build a fake [`EmbeddedEsbuildGetter`] that stages a real, runnable
    /// esbuild copy into a fresh `TempDir` — the shape a packaged `zfb`
    /// build's real getter produces. Mirrors
    /// `plugin_bundler::tests::make_stub_embedded_getter`, which is private
    /// to its own module — a small, deliberate duplication rather than
    /// reaching into another module's test code.
    fn make_stub_embedded_esbuild_getter(
        real_esbuild: std::path::PathBuf,
    ) -> EmbeddedEsbuildGetter {
        Box::new(move || {
            let dir = tempfile::Builder::new()
                .prefix("zfb-plugin-runner-embedded-esbuild-stub-")
                .tempdir()
                .ok()?;
            let dest = dir.path().join(real_esbuild.file_name()?);
            std::fs::copy(&real_esbuild, &dest).ok()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&dest).ok()?.permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&dest, perms).ok()?;
            }
            Some((dir, dest))
        })
    }

    #[tokio::test]
    async fn mjs_plugin_never_touches_esbuild_resolution() {
        // Decisions (a)/(c): a plugin set with no `.ts`-family entry must
        // never call `resolve_esbuild_for_plugins` at all — a panicking
        // getter proves the laziness gate itself, not merely that
        // bundling happened to be skipped. This is the ".mjs plugin that
        // works today still works" acceptance criterion made precise: the
        // plain plain-`import()` path is exercised with zero esbuild
        // involvement, byte-identical to before this wiring.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("plain.mjs");
        tokio::fs::write(
            &plugin_path,
            r#"
            export default {
              name: "plain-mjs",
              preBuild() {},
            };
            "#,
        )
        .await
        .unwrap();
        let getter: EmbeddedEsbuildGetter =
            Box::new(|| panic!("embedded getter must not be invoked for a pure-.mjs plugin set"));
        let host = PluginHost::spawn_with_timeout(
            vec![PluginSpec {
                name: "plain-mjs".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
            None,
            Some(getter),
        )
        .await
        .expect(".mjs plugin should load via the untouched plain-import() path");
        host.shutdown().await.ok();
    }

    #[tokio::test]
    async fn ts_plugin_entry_with_enum_namespace_and_parameter_properties_loads_through_real_host()
    {
        // The reporting issue's documented "failure mode 2" TS constructs
        // (enum, namespace, constructor parameter properties) already
        // proved they BUNDLE cleanly in `plugin_bundler::tests` (Wave 3).
        // This proves the wired-up result actually LOADS AND RUNS through
        // the real host: a preBuild hook computes a value derived from an
        // enum, a namespace, and a parameter property, and writes it to a
        // marker file only reachable if `import()` of the staged bundle
        // succeeded and the hook actually executed.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let Some(esbuild) = zfb_test_utils::locate_esbuild() else {
            eprintln!("skipping: no esbuild binary available");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("ts-marker.txt");
        let plugin_path = tmp.path().join("plugin.ts");
        let plugin_src = format!(
            r#"
            import {{ writeFileSync }} from "node:fs";

            enum Level {{ Low, High }}
            namespace Labels {{ export const high = "high-marker"; }}
            class Prop {{ constructor(public level: Level) {{}} }}

            export default {{
              name: "ts-enum-namespace-paramprop",
              preBuild() {{
                const p = new Prop(Level.High);
                const label = p.level === Level.High ? Labels.high : "low";
                writeFileSync({marker:?}, label);
              }},
            }};
            "#,
            marker = marker.to_string_lossy().to_string(),
        );
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();

        let getter = make_stub_embedded_esbuild_getter(esbuild);
        let host = PluginHost::spawn_with_timeout(
            vec![PluginSpec {
                name: "ts-enum-namespace-paramprop".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
            None,
            Some(getter),
        )
        .await
        .expect("a .ts entry using enum/namespace/parameter-properties should bundle and load");

        let ctx = BuildHookContext {
            project_root: tmp.path().to_path_buf(),
            out_dir: tmp.path().join("dist"),
            config: serde_json::json!({}),
            routes: None,
        };
        host.run_pre_build(&ctx).await.expect("preBuild ok");
        host.shutdown().await.ok();

        let written = tokio::fs::read_to_string(&marker).await.unwrap();
        assert_eq!(written, "high-marker");
    }

    #[tokio::test]
    async fn tsx_plugin_entry_with_paths_alias_loads_through_real_host() {
        // Covers the remaining two documented failure modes together: a
        // `.tsx` entry (JSX syntax) importing a sibling through a
        // `paths`-aliased specifier that esbuild resolves via its
        // auto-discovered tsconfig. The JSX factory is never invoked here
        // (only `String(Widget)` is read) so the test needs no runtime
        // JSX-runtime resolution — it only has to prove the module LOADS
        // (JSX parsed + transformed, `paths` alias resolved) through the
        // real host.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let Some(esbuild) = zfb_test_utils::locate_esbuild() else {
            eprintln!("skipping: no esbuild binary available");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        tokio::fs::write(
            root.join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@lib/*":["./lib/*"]}}}"#,
        )
        .await
        .unwrap();
        tokio::fs::create_dir_all(root.join("lib")).await.unwrap();
        tokio::fs::write(
            root.join("lib/marker.ts"),
            "export const marker = \"paths-alias-marker\";\n",
        )
        .await
        .unwrap();
        let marker_file = root.join("tsx-marker.txt");
        let plugin_path = root.join("plugin.tsx");
        let plugin_src = format!(
            r#"
            import {{ writeFileSync }} from "node:fs";
            import {{ marker }} from "@lib/marker";

            // Never invoked — only present so this .tsx entry actually
            // exercises esbuild's JSX transform (proving raw JSX syntax
            // does not survive as a `import()`-time SyntaxError). Calling
            // it would throw (no JSX runtime is installed in this
            // fixture), so the hook below never does.
            const Widget = () => <div>{{marker}}</div>;
            void Widget;

            export default {{
              name: "tsx-paths-alias",
              preBuild() {{
                // The real proof this test wants: the `@lib/marker`
                // paths-aliased import resolved and its value reached
                // plugin code — only possible if the module actually
                // loaded through the real host.
                writeFileSync({marker_file:?}, marker);
              }},
            }};
            "#,
            marker_file = marker_file.to_string_lossy().to_string(),
        );
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();

        let getter = make_stub_embedded_esbuild_getter(esbuild);
        let host = PluginHost::spawn_with_timeout(
            vec![PluginSpec {
                name: "tsx-paths-alias".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
            None,
            Some(getter),
        )
        .await
        .expect("a .tsx entry with a paths alias should bundle and load");

        let ctx = BuildHookContext {
            project_root: tmp.path().to_path_buf(),
            out_dir: tmp.path().join("dist"),
            config: serde_json::json!({}),
            routes: None,
        };
        host.run_pre_build(&ctx).await.expect("preBuild ok");
        host.shutdown().await.ok();

        let written = tokio::fs::read_to_string(&marker_file).await.unwrap();
        assert_eq!(
            written, "paths-alias-marker",
            "the paths-aliased import's value should reach the plugin's own logic"
        );
    }

    #[tokio::test]
    async fn broken_ts_plugin_entry_fails_with_the_locked_bundle_error_not_a_plugin_error() {
        // Decision (e): a bundle failure is a plain `anyhow` error, never
        // `PluginError { hook: "init" }` — the host process never even
        // boots, so there is no plugin code that "ran and threw". Proven
        // through the real `spawn_with_timeout` call (not just
        // `bundle_plugin_entry` in isolation, which `plugin_bundler::tests`
        // already covers), and asserts the error is neither a raw Node
        // stack nor the config loader's `zfb.config.ts` diagnostic.
        let Some(esbuild) = zfb_test_utils::locate_esbuild() else {
            eprintln!("skipping: no esbuild binary available");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("broken.ts");
        tokio::fs::write(
            &plugin_path,
            "import { nope } from \"./does-not-exist.ts\";\nexport default { name: \"broken\", nope };\n",
        )
        .await
        .unwrap();

        let getter = make_stub_embedded_esbuild_getter(esbuild);
        let result = PluginHost::spawn_with_timeout(
            vec![PluginSpec {
                name: "broken".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
            None,
            Some(getter),
        )
        .await;
        // `PluginHost` doesn't implement `Debug`, so `expect_err` (which
        // requires `T: Debug` to format the unexpected-Ok case) can't be
        // used directly here.
        let err = match result {
            Ok(_) => {
                panic!("a deliberately broken .ts entry must fail to bundle, never boot the host")
            }
            Err(e) => e,
        };

        let msg = err.to_string();
        let expected_prefix = format!(
            "plugin bundling: esbuild failed for plugin `broken` ({}):",
            plugin_path.display()
        );
        assert!(
            msg.starts_with(&expected_prefix),
            "error must match the locked bundle-failure prefix.\nexpected prefix: {expected_prefix}\ngot: {msg}"
        );
        assert!(
            msg.contains("✘ [ERROR]"),
            "the error must carry esbuild's own diagnostic marker, not just our wrapper text: {msg}"
        );
        assert!(
            !msg.contains("Evaluating a `zfb.config.ts`"),
            "the config-loader diagnostic must never leak onto the plugin path: {msg}"
        );
        assert!(
            extract_plugin_error(&err).is_none(),
            "a bundle failure must be a plain anyhow error, never a PluginError (hook=\"init\" \
             means the plugin code ran and threw, which never happened here): {err:?}"
        );
    }

    #[tokio::test]
    async fn init_failure_for_a_ts_plugin_tears_down_the_host_and_removes_the_staged_bundle() {
        // Regression for a light-review (codex) finding on #2308: an
        // `init` failure does NOT crash the child process — `ok: false`
        // is just an ordinary reply, so the process stays alive. Without
        // an explicit teardown on that path, the stdout/stderr reader
        // tasks (each holding their own `Arc<HostInner>` clone) never
        // observe EOF and never exit, so `host` merely falling out of
        // scope leaks the child process AND everything `HostInner`'s
        // Drop would otherwise clean up — the plugin-host tempdir and,
        // since #2308, the staged `.ts` bundle artifact. Proves the fix:
        // after a bundled plugin's `init` fails, the staged bundle file
        // is eventually removed.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let Some(esbuild) = zfb_test_utils::locate_esbuild() else {
            eprintln!("skipping: no esbuild binary available");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("throws-at-import.ts");
        tokio::fs::write(
            &plugin_path,
            "throw new Error(\"boom-at-import\");\nexport default { name: \"throws-at-import\" };\n",
        )
        .await
        .unwrap();

        let getter = make_stub_embedded_esbuild_getter(esbuild);
        let result = PluginHost::spawn_with_timeout(
            vec![PluginSpec {
                name: "throws-at-import".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
            None,
            Some(getter),
        )
        .await;
        let err = match result {
            Ok(_) => panic!("a plugin that throws at import time must fail init"),
            Err(e) => e,
        };
        assert!(
            extract_plugin_error(&err).is_some(),
            "an import-time throw is a genuine `init`-hook failure, not a bundle failure: {err:?}"
        );

        // The staged bundle sits next to the entry (decision d).
        let staged: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.starts_with(plugin_bundler::PLUGIN_BUNDLE_TEMP_PREFIX)
                    && name.ends_with(plugin_bundler::PLUGIN_BUNDLE_TEMP_SUFFIX)
            })
            .collect();
        // The file may already be gone by the time we list the
        // directory (cleanup is async and can win the race) — what
        // matters is that it never survives indefinitely, so only poll
        // for removal when we actually observed it first.
        if let Some(entry) = staged.into_iter().next() {
            let path = entry.path();
            let removed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while path.exists() {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
            })
            .await;
            assert!(
                removed.is_ok(),
                "the staged bundle must be removed once the host tears down after an init failure"
            );
        }
    }

    #[tokio::test]
    async fn staged_ts_bundle_survives_until_host_shutdown() {
        // Decisions (c)/(d): the staged `.mjs` bundle is held on the
        // `PluginHost` handle (`Vec<StagedPluginBundle>`) for the WHOLE
        // host session, not dropped right after boot — confirmed by
        // checking the staged file still exists on disk while the host is
        // alive, then confirming its removal once the host (its one and
        // only clone here) is dropped.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let Some(esbuild) = zfb_test_utils::locate_esbuild() else {
            eprintln!("skipping: no esbuild binary available");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let plugin_path = tmp.path().join("plugin.ts");
        tokio::fs::write(
            &plugin_path,
            "export default { name: \"staged-lifetime-plugin\" };\n",
        )
        .await
        .unwrap();

        let getter = make_stub_embedded_esbuild_getter(esbuild);
        let host = PluginHost::spawn_with_timeout(
            vec![PluginSpec {
                name: "staged-lifetime-plugin".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
            None,
            Some(getter),
        )
        .await
        .expect("host should spawn, bundling the .ts entry first");

        // The staged bundle sits next to the entry (decision d), named
        // with the `.zfb-plugin-bundle-*.mjs` prefix/suffix.
        let mut staged_files: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.starts_with(plugin_bundler::PLUGIN_BUNDLE_TEMP_PREFIX)
                    && name.ends_with(plugin_bundler::PLUGIN_BUNDLE_TEMP_SUFFIX)
            })
            .collect();
        assert_eq!(
            staged_files.len(),
            1,
            "exactly one staged bundle should exist while the host is alive"
        );
        let staged_path = staged_files.remove(0).path();
        assert!(
            staged_path.exists(),
            "the staged bundle must still be on disk while the host session is alive"
        );

        host.shutdown().await.ok();
        drop(host);

        assert!(
            !staged_path.exists(),
            "the staged bundle must be removed once the host (all its clones) is dropped"
        );
    }

    // --- #2309 through-the-host test coverage (complements the wiring
    // tests above, does not duplicate them) --------------------------------

    #[tokio::test]
    async fn ts_plugin_dot_js_import_resolves_to_sibling_ts_through_real_host() {
        // Failure mode 1 (the reporting issue's headline case), promoted
        // from `plugin_bundler::tests::ts_entry_with_extensionless_js_import_resolves_to_sibling_ts`
        // (which only proves the BUNDLE inlines the sibling's export) to
        // the real host: an entry importing `./helper.js` — a path
        // esbuild resolves to the sibling `helper.ts` source — must
        // actually LOAD and RUN through `spawn_with_timeout`, not merely
        // bundle cleanly.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let Some(esbuild) = zfb_test_utils::locate_esbuild() else {
            eprintln!("skipping: no esbuild binary available");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        tokio::fs::write(
            root.join("helper.ts"),
            "export const helperMarker = \"dot-js-import-marker\";\n",
        )
        .await
        .unwrap();
        let marker_file = root.join("marker.txt");
        let plugin_path = root.join("index.ts");
        let plugin_src = format!(
            r#"
            import {{ writeFileSync }} from "node:fs";
            import {{ helperMarker }} from "./helper.js";

            export default {{
              name: "dot-js-import-plugin",
              preBuild() {{
                writeFileSync({marker_file:?}, helperMarker);
              }},
            }};
            "#,
            marker_file = marker_file.to_string_lossy().to_string(),
        );
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();

        let getter = make_stub_embedded_esbuild_getter(esbuild);
        let host = PluginHost::spawn_with_timeout(
            vec![PluginSpec {
                name: "dot-js-import-plugin".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
            None,
            Some(getter),
        )
        .await
        .expect("a `./helper.js` import resolving to sibling helper.ts should bundle and load");

        let ctx = BuildHookContext {
            project_root: root.to_path_buf(),
            out_dir: root.join("dist"),
            config: serde_json::json!({}),
            routes: None,
        };
        host.run_pre_build(&ctx).await.expect("preBuild ok");
        host.shutdown().await.ok();

        let written = tokio::fs::read_to_string(&marker_file).await.unwrap();
        assert_eq!(written, "dot-js-import-marker");
    }

    #[tokio::test]
    async fn tsx_import_actually_executes_through_real_host() {
        // Wave 4's `tsx_plugin_entry_with_paths_alias_loads_through_real_host`
        // deliberately never CALLS its Widget component (its own comment
        // says so) — proving only that JSX syntax survives `import()`.
        // #2309 asks for a `.tsx` import whose module actually EXECUTES:
        // the entry here calls the imported component and writes the
        // values that come back out of the executed JSX call, which is
        // only possible if esbuild's JSX transform produced runnable code
        // (not merely parseable code) and the runtime factory it calls
        // actually ran.
        //
        // Classic JSX (`jsxFactory: "h"`, `h` defined in the SAME file as
        // the JSX usage) is used instead of the automatic runtime so the
        // test needs no jsx-runtime package staged in a node_modules the
        // fixture doesn't have — `--packages=external` would otherwise
        // leave an automatic-runtime import unresolved at Node's import
        // time.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let Some(esbuild) = zfb_test_utils::locate_esbuild() else {
            eprintln!("skipping: no esbuild binary available");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        tokio::fs::write(
            root.join("tsconfig.json"),
            r#"{"compilerOptions":{"jsx":"react","jsxFactory":"h"}}"#,
        )
        .await
        .unwrap();
        tokio::fs::write(
            root.join("widget.tsx"),
            r#"export function h(type: string, props: Record<string, unknown> | null, ...children: unknown[]) {
  return { type, props, children };
}
export const Widget = () => <div data-marker="tsx-widget-marker">{"tsx-widget-body"}</div>;
"#,
        )
        .await
        .unwrap();
        let marker_file = root.join("marker.txt");
        let plugin_path = root.join("index.ts");
        let plugin_src = format!(
            r#"
            import {{ writeFileSync }} from "node:fs";
            import {{ Widget }} from "./widget.tsx";

            export default {{
              name: "tsx-actually-executes",
              preBuild() {{
                // Calling Widget() is the whole point of this test — the
                // marker only reaches disk if the compiled `h(...)` call
                // actually ran and returned a real value.
                const vnode = Widget();
                writeFileSync({marker_file:?}, JSON.stringify(vnode));
              }},
            }};
            "#,
            marker_file = marker_file.to_string_lossy().to_string(),
        );
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();

        let getter = make_stub_embedded_esbuild_getter(esbuild);
        let host = PluginHost::spawn_with_timeout(
            vec![PluginSpec {
                name: "tsx-actually-executes".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
            None,
            Some(getter),
        )
        .await
        .expect("a .tsx import calling its exported component should bundle and load");

        let ctx = BuildHookContext {
            project_root: root.to_path_buf(),
            out_dir: root.join("dist"),
            config: serde_json::json!({}),
            routes: None,
        };
        host.run_pre_build(&ctx).await.expect("preBuild ok");
        host.shutdown().await.ok();

        let written = tokio::fs::read_to_string(&marker_file).await.unwrap();
        let vnode: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(
            vnode["type"].as_str().unwrap(),
            "div",
            "the executed JSX call should have produced a real vnode, not a literal: {written}"
        );
        assert_eq!(
            vnode["props"]["data-marker"].as_str().unwrap(),
            "tsx-widget-marker"
        );
        assert_eq!(
            vnode["children"][0].as_str().unwrap(),
            "tsx-widget-body",
            "the component's child text should be present in the ACTUAL call result: {written}"
        );
    }

    #[tokio::test]
    async fn mjs_plugin_import_meta_url_and_relative_fs_read_are_unchanged_through_real_host() {
        // Regression required by #2309: for an existing (non-TS) plugin,
        // the Wave-2 contract must hold exactly as it did before #2308 —
        // no staging happens at all, `import.meta.url` still names the
        // ORIGINAL on-disk file (not a staged copy — none exists), and a
        // relative filesystem read next to the plugin file keeps
        // working. Reuses the panicking-getter laziness proof from
        // `mjs_plugin_never_touches_esbuild_resolution` so this test also
        // independently re-confirms bundling never runs for a `.mjs`
        // entry.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        tokio::fs::write(root.join("sidecar.txt"), "sidecar-marker")
            .await
            .unwrap();
        let marker_file = root.join("marker.txt");
        let plugin_path = root.join("plugin.mjs");
        let plugin_src = format!(
            r#"
            import {{ writeFileSync, readFileSync }} from "node:fs";
            import {{ fileURLToPath }} from "node:url";
            import {{ dirname, join }} from "node:path";

            export default {{
              name: "mjs-dirname-plugin",
              preBuild() {{
                const dir = dirname(fileURLToPath(import.meta.url));
                const sidecar = readFileSync(join(dir, "sidecar.txt"), "utf8");
                writeFileSync({marker_file:?}, JSON.stringify({{ dir, sidecar, url: import.meta.url }}));
              }},
            }};
            "#,
            marker_file = marker_file.to_string_lossy().to_string(),
        );
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();

        let getter: EmbeddedEsbuildGetter =
            Box::new(|| panic!("embedded getter must not be invoked for a pure-.mjs plugin set"));
        let host = PluginHost::spawn_with_timeout(
            vec![PluginSpec {
                name: "mjs-dirname-plugin".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
            None,
            Some(getter),
        )
        .await
        .expect(".mjs plugin should load via the untouched plain-import() path");

        let ctx = BuildHookContext {
            project_root: root.to_path_buf(),
            out_dir: root.join("dist"),
            config: serde_json::json!({}),
            routes: None,
        };
        host.run_pre_build(&ctx).await.expect("preBuild ok");
        host.shutdown().await.ok();

        let written = tokio::fs::read_to_string(&marker_file).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(parsed["sidecar"].as_str().unwrap(), "sidecar-marker");
        // Compare against the CANONICALIZED root, not the raw tempdir path:
        // on macOS `std::env::temp_dir()` sits under `/var`, a symlink to
        // `/private/var`, and Node's ESM loader canonicalizes module paths
        // (`fs.realpath`) when computing `import.meta.url` — a platform
        // quirk of the test's tempdir, unrelated to bundling.
        let canonical_root = std::fs::canonicalize(root).unwrap();
        assert_eq!(
            parsed["dir"].as_str().unwrap(),
            canonical_root.to_string_lossy(),
            "dirname(fileURLToPath(import.meta.url)) must be the plugin's ORIGINAL directory: {written}"
        );
        assert!(
            parsed["url"].as_str().unwrap().ends_with("plugin.mjs"),
            "import.meta.url must still name the original on-disk file — no staged copy exists for .mjs: {written}"
        );
    }

    #[tokio::test]
    async fn bundled_ts_plugin_dirname_relative_sidecar_read_succeeds_through_real_host() {
        // The Wave-2 `import.meta.url` contract (#2303 decision record):
        // the bundle is staged in the ENTRY's own directory, so
        // `dirname(fileURLToPath(import.meta.url))` still resolves to
        // that directory even though the staged file's NAME differs from
        // the entry's. Assert the dirname-relative read succeeds —
        // deliberately do NOT assert on the staged filename itself (the
        // decision record is explicit that the filename is allowed to
        // differ).
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let Some(esbuild) = zfb_test_utils::locate_esbuild() else {
            eprintln!("skipping: no esbuild binary available");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        tokio::fs::write(root.join("sidecar.txt"), "ts-sidecar-marker")
            .await
            .unwrap();
        let marker_file = root.join("marker.txt");
        let plugin_path = root.join("plugin.ts");
        let plugin_src = format!(
            r#"
            import {{ writeFileSync, readFileSync }} from "node:fs";
            import {{ fileURLToPath }} from "node:url";
            import {{ dirname, join }} from "node:path";

            export default {{
              name: "ts-dirname-plugin",
              preBuild() {{
                const dir = dirname(fileURLToPath(import.meta.url));
                const sidecar = readFileSync(join(dir, "sidecar.txt"), "utf8");
                writeFileSync({marker_file:?}, JSON.stringify({{ dir, sidecar }}));
              }},
            }};
            "#,
            marker_file = marker_file.to_string_lossy().to_string(),
        );
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();

        let getter = make_stub_embedded_esbuild_getter(esbuild);
        let host = PluginHost::spawn_with_timeout(
            vec![PluginSpec {
                name: "ts-dirname-plugin".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
            None,
            Some(getter),
        )
        .await
        .expect("a .ts entry reading a dirname-relative sidecar should bundle and load");

        let ctx = BuildHookContext {
            project_root: root.to_path_buf(),
            out_dir: root.join("dist"),
            config: serde_json::json!({}),
            routes: None,
        };
        host.run_pre_build(&ctx).await.expect("preBuild ok");
        host.shutdown().await.ok();

        let written = tokio::fs::read_to_string(&marker_file).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(
            parsed["sidecar"].as_str().unwrap(),
            "ts-sidecar-marker",
            "a dirname-relative sidecar read must succeed for a bundled .ts plugin: {written}"
        );
        // See the matching comment in
        // `mjs_plugin_import_meta_url_and_relative_fs_read_are_unchanged_through_real_host`:
        // compare against the canonicalized root, not the raw tempdir
        // path, to sidestep macOS's `/var` -> `/private/var` symlink.
        let canonical_root = std::fs::canonicalize(root).unwrap();
        assert_eq!(
            parsed["dir"].as_str().unwrap(),
            canonical_root.to_string_lossy(),
            "the staged bundle's dirname must still be the entry's OWN directory: {written}"
        );
    }

    #[tokio::test]
    async fn bundled_ts_plugin_bare_specifier_resolves_from_project_node_modules_through_real_host()
    {
        // Decision (b): `--packages=external` leaves bare specifiers
        // unresolved at bundle time; the entry is staged in ITS OWN
        // directory (not a shared tempdir), so Node's ordinary ancestor
        // `node_modules` walk from the staged file's location is
        // identical to the original entry's. Proves a bundled .ts plugin
        // importing a bare-specifier dep from the project's
        // `node_modules` resolves at import time.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let Some(esbuild) = zfb_test_utils::locate_esbuild() else {
            eprintln!("skipping: no esbuild binary available");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let dep_dir = root.join("node_modules/fake-dep");
        tokio::fs::create_dir_all(&dep_dir).await.unwrap();
        tokio::fs::write(
            dep_dir.join("package.json"),
            r#"{"name":"fake-dep","version":"1.0.0","type":"module","main":"index.mjs"}"#,
        )
        .await
        .unwrap();
        tokio::fs::write(
            dep_dir.join("index.mjs"),
            "export const depMarker = \"node-modules-dep-marker\";\n",
        )
        .await
        .unwrap();

        let marker_file = root.join("marker.txt");
        let plugin_path = root.join("plugin.ts");
        let plugin_src = format!(
            r#"
            import {{ writeFileSync }} from "node:fs";
            import {{ depMarker }} from "fake-dep";

            export default {{
              name: "ts-bare-specifier-plugin",
              preBuild() {{
                writeFileSync({marker_file:?}, depMarker);
              }},
            }};
            "#,
            marker_file = marker_file.to_string_lossy().to_string(),
        );
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();

        let getter = make_stub_embedded_esbuild_getter(esbuild);
        let host = PluginHost::spawn_with_timeout(
            vec![PluginSpec {
                name: "ts-bare-specifier-plugin".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
            None,
            Some(getter),
        )
        .await
        .expect(
            "a bare-specifier import from the project's node_modules should resolve at runtime",
        );

        let ctx = BuildHookContext {
            project_root: root.to_path_buf(),
            out_dir: root.join("dist"),
            config: serde_json::json!({}),
            routes: None,
        };
        host.run_pre_build(&ctx).await.expect("preBuild ok");
        host.shutdown().await.ok();

        let written = tokio::fs::read_to_string(&marker_file).await.unwrap();
        assert_eq!(written, "node-modules-dep-marker");
    }

    #[tokio::test]
    async fn cts_plugin_using_require_and_export_equals_executes_through_real_host() {
        // Codex review finding 1 (P1, #2324): a `.cts` plugin using
        // ordinary CommonJS constructs (`require("node:fs")`, `export =`)
        // is bundled as ESM with `--packages=external`; esbuild's
        // `__require` shim then defers to an ambient `require`, which
        // Node ESM has none of, so the entry fails at import time with
        // "Dynamic require of ... is not supported" — the documented
        // `.cts` support (`docs/.../plugins.mdx`, "Plugin entry files —
        // .ts, .tsx, .mts, .cts support") is broken for this common
        // shape. Reproduced directly against esbuild's raw output before
        // this test was written (`node --input-type=module -e
        // "import('./plugin.mjs')..."` on the exact same bundling flags
        // this test drives through the real host): the import rejects
        // with `Dynamic require of "node:fs" is not supported`. Fixed by
        // a `--banner:js` in `plugin_bundler::bundle_plugin_entry` that
        // defines a real top-level `require` via `node:module`
        // `createRequire(import.meta.url)` — esbuild's shim checks
        // `typeof require !== "undefined"` first and defers to it.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let Some(esbuild) = zfb_test_utils::locate_esbuild() else {
            eprintln!("skipping: no esbuild binary available");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let marker_file = root.join("marker.txt");
        let plugin_path = root.join("plugin.cts");
        // Deliberately ordinary CommonJS-in-.cts constructs: a top-level
        // `require()` call (so the failure surfaces at import time, not
        // lazily inside a hook) and `export =` instead of `export default`.
        let plugin_src = format!(
            r#"
            const {{ writeFileSync }} = require("node:fs");

            const plugin = {{
              name: "cts-require-plugin",
              preBuild() {{
                writeFileSync({marker_file:?}, "cts-require-marker");
              }},
            }};

            export = plugin;
            "#,
            marker_file = marker_file.to_string_lossy().to_string(),
        );
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();

        let getter = make_stub_embedded_esbuild_getter(esbuild);
        let host = PluginHost::spawn_with_timeout(
            vec![PluginSpec {
                name: "cts-require-plugin".into(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
            None,
            Some(getter),
        )
        .await
        .expect(
            "a .cts entry using top-level require()/export= should bundle and load through the \
             real host — if this panics with \"Dynamic require of ... is not supported\", the \
             banner:js require() shim in bundle_plugin_entry is missing or broken",
        );

        let ctx = BuildHookContext {
            project_root: root.to_path_buf(),
            out_dir: root.join("dist"),
            config: serde_json::json!({}),
            routes: None,
        };
        host.run_pre_build(&ctx).await.expect("preBuild ok");
        host.shutdown().await.ok();

        let written = tokio::fs::read_to_string(&marker_file).await.unwrap();
        assert_eq!(written, "cts-require-marker");
    }

    #[tokio::test]
    async fn duplicate_mjs_plugin_specs_share_one_module_record_and_evaluate_once_through_real_host(
    ) {
        // Baseline measurement for codex review finding 2 (#2324): the
        // PRE-EXISTING semantics for a `.js`/`.mjs`/`.cjs` plugin module
        // named by more than one `PluginSpec`. `.mjs` specs are never
        // rewritten by the bundling loop (`needs_bundling` is false for
        // them), so both entries carry the identical `file://` URL string
        // straight into `plugin-host.mjs`'s `await import(entry.module)`
        // loop — and Node's ESM module cache is keyed by resolved URL, so
        // the SECOND `import()` call returns the already-evaluated module
        // record without re-running its top-level code. This test proves
        // that with a counter file incremented at MODULE TOP LEVEL (not
        // inside a hook, so it measures evaluation count, not hook-call
        // count — both specs' hooks still fire separately, once each, via
        // `plugin-host.mjs`'s per-registration `plugins` array). The `.ts`
        // counterpart below (`duplicate_ts_plugin_specs_share_one_staged_bundle_and_evaluate_once_through_real_host`)
        // asserts the bundling path now matches this exact baseline.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let counter_file = root.join("counter.txt");
        let plugin_path = root.join("dup.mjs");
        let plugin_src = format!(
            r#"
            import {{ readFileSync, writeFileSync, existsSync }} from "node:fs";

            const counterPath = {counter:?};
            const prev = existsSync(counterPath) ? Number(readFileSync(counterPath, "utf8")) : 0;
            writeFileSync(counterPath, String(prev + 1));

            export default {{
              name: "dup-mjs-plugin",
              preBuild() {{}},
            }};
            "#,
            counter = counter_file.to_string_lossy().to_string(),
        );
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();

        let module_url = file_url_for_test(&plugin_path);
        let host = PluginHost::spawn(
            vec![
                PluginSpec {
                    name: "dup-a".into(),
                    module: module_url.clone(),
                    options: serde_json::json!({}),
                },
                PluginSpec {
                    name: "dup-b".into(),
                    module: module_url,
                    options: serde_json::json!({}),
                },
            ],
            None,
        )
        .await
        .expect("host spawns with two specs naming the same .mjs module");
        host.shutdown().await.ok();

        let written = tokio::fs::read_to_string(&counter_file).await.unwrap();
        assert_eq!(
            written, "1",
            "two specs naming the identical .mjs URL must hit Node's module cache and \
             evaluate the module's top level exactly once: {written}"
        );
    }

    #[tokio::test]
    async fn duplicate_ts_plugin_specs_share_one_staged_bundle_and_evaluate_once_through_real_host()
    {
        // Codex review finding 2 (P2, #2324): before the
        // `staged_by_original_module` cache in `spawn_with_timeout`, each
        // `PluginSpec` naming the same `.ts` module got its OWN
        // separately-named staged `.mjs` bundle (a fresh `NamedTempFile`
        // per bundling call), so the host imported two DISTINCT URLs for
        // what should be one module — diverging from the `.mjs` baseline
        // measured above, where Node's module cache collapses repeated
        // imports of the identical URL to a single evaluation. This test
        // pins both halves of the fix: exactly ONE staged bundle file
        // exists on disk while the host is alive (not two), and the
        // module's top-level counter — identical technique to the `.mjs`
        // baseline above — shows exactly one evaluation.
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let Some(esbuild) = zfb_test_utils::locate_esbuild() else {
            eprintln!("skipping: no esbuild binary available");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let counter_file = root.join("counter.txt");
        let plugin_path = root.join("dup.ts");
        let plugin_src = format!(
            r#"
            import {{ readFileSync, writeFileSync, existsSync }} from "node:fs";

            const counterPath: string = {counter:?};
            const prev: number = existsSync(counterPath) ? Number(readFileSync(counterPath, "utf8")) : 0;
            writeFileSync(counterPath, String(prev + 1));

            export default {{
              name: "dup-ts-plugin",
              preBuild() {{}},
            }};
            "#,
            counter = counter_file.to_string_lossy().to_string(),
        );
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();

        let module_url = file_url_for_test(&plugin_path);
        let getter = make_stub_embedded_esbuild_getter(esbuild);
        let host = PluginHost::spawn_with_timeout(
            vec![
                PluginSpec {
                    name: "dup-a".into(),
                    module: module_url.clone(),
                    options: serde_json::json!({}),
                },
                PluginSpec {
                    name: "dup-b".into(),
                    module: module_url,
                    options: serde_json::json!({}),
                },
            ],
            None,
            None,
            Some(getter),
        )
        .await
        .expect("host spawns with two specs naming the same .ts module");

        // Exactly one staged bundle on disk — not one per duplicate spec.
        let staged_files: Vec<_> = std::fs::read_dir(root)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.starts_with(plugin_bundler::PLUGIN_BUNDLE_TEMP_PREFIX)
                    && name.ends_with(plugin_bundler::PLUGIN_BUNDLE_TEMP_SUFFIX)
            })
            .collect();
        assert_eq!(
            staged_files.len(),
            1,
            "two specs naming the identical .ts module must reuse ONE staged bundle, not stage \
             a separate copy per duplicate spec"
        );

        host.shutdown().await.ok();

        let written = tokio::fs::read_to_string(&counter_file).await.unwrap();
        assert_eq!(
            written, "1",
            "two specs sharing one staged bundle URL must hit Node's module cache and evaluate \
             the module's top level exactly once, matching the .mjs baseline: {written}"
        );
    }
}
