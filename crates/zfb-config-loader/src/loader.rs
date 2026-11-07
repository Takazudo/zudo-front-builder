//! The `zfb.config.ts` evaluator: bundle through the supervised native
//! compiler worker, evaluate (in-process V8 on default builds, `node`
//! subprocess on slim builds), and resolve plugin module specifiers.
//!
//! This module is config-shape-agnostic — it returns the user's `default`
//! export as a [`serde_json::Value`] plus the resolved plugin specifiers
//! ([`LoadedTsConfig`]). Deserialising the value into a concrete config
//! struct is the caller's job (see the crate-level docs).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context as _, Result};
use tokio::process::Command;
use zfb_compiler_worker::{BundleRequest, CompileJob, CompilerWorker, WorkerError, WorkerLauncher};
use zfb_rolldown_prototype::{OutputLayout, Platform};

// Only the in-process V8 path resolves plugins in Rust (the slim subprocess
// path resolves them inside config-loader.mjs), so this import is gated to
// the same feature as `resolve_plugins_from_value`.
#[cfg(feature = "direct-v8")]
use crate::node_resolve::resolve_node_bare_specifier;

/// JS payload run by `node` to dynamic-import the bundled config and emit
/// the default export as JSON on stdout. Embedded into the binary so we
/// don't have to ship a sidecar file at runtime.
///
/// Slim-build-only: staged into a tempdir and evaluated by the `node`
/// subprocess in [`load_ts_via_subprocess`]. Default (direct-v8) builds
/// evaluate the bundle in-process and do not need this file.
#[cfg(not(feature = "direct-v8"))]
const CONFIG_LOADER_MJS: &str = include_str!("../js/config-loader.mjs");

/// Stub for the `zfb/config` (and `@takazudo/zfb/config`) import that user
/// TS configs reach for. We alias both the unscoped bare form (`zfb/config`)
/// and the full npm-package form (`@takazudo/zfb/config`) to this stub at
/// bundle time so either spelling works without installing the npm package.
const CONFIG_STUB_MJS: &str = include_str!("../js/zfb-config-stub.mjs");

/// Knobs that tweak loader behaviour. `Default` resolves plugins and has no
/// compiler worker launcher, so a real load needs
/// [`compiler_worker`](Self::compiler_worker) set by the host.
pub struct LoadOptions {
    /// How to start the supervised compiler worker that bundles the config.
    /// Only the host executable knows whether it can serve worker mode, so
    /// the library never derives one: the zfb CLI passes
    /// [`WorkerLauncher::zfb_cli`], an embedder passes a launcher for its
    /// own executable whose `main` calls
    /// `zfb_compiler_worker::run_if_requested()` first. `None` fails a
    /// `.ts` load with an actionable error.
    pub compiler_worker: Option<WorkerLauncher>,
    /// Override the `node` binary. `None` uses `node` from `PATH`. Only the
    /// slim (no `direct-v8`) build evaluates through `node`.
    pub node_binary: Option<OsString>,
    /// Resolve each `config.plugins[]` entry to a `file://` URL before
    /// returning. Defaults to `true` — the CLI / `zfb` bin path needs the
    /// resolved specifiers to load plugins, and must not change.
    ///
    /// Set to `false` for consumers that only read the scalar config fields
    /// and never load plugins (e.g. `zfb-server`'s embed API): a packaged
    /// app shipping a `zfb.config.ts` that lists CLI-only plugins whose
    /// package is absent from the deployment would otherwise FAIL at plugin
    /// resolution, even though the equivalent `.json` embed path ignores
    /// those entries and succeeds. When `false`, resolution is skipped
    /// entirely and `resolved_plugins` comes back empty.
    pub resolve_plugins: bool,
    /// Test-only escape hatch: when `Some`, skip bundling and evaluation
    /// entirely and treat this string as the JSON form of the loader
    /// envelope (or a bare `default` export object) from the user's
    /// `zfb.config.ts`. Production code leaves this `None`.
    #[doc(hidden)]
    pub test_default_export_json: Option<String>,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            compiler_worker: None,
            node_binary: None,
            // Production / CLI default: resolve plugins. Embed callers
            // opt out explicitly with `resolve_plugins: false`.
            resolve_plugins: true,
            test_default_export_json: None,
        }
    }
}

/// The result of evaluating a `zfb.config.ts`: the user's `default` export
/// as a [`serde_json::Value`] plus the resolved plugin module specifiers
/// (one `file://` URL per `config.plugins[]` entry, in order).
///
/// Callers deserialise [`Self::config`] into their own config struct and,
/// if they care about plugins, zip [`Self::resolved_plugins`] onto the
/// deserialised `plugins[]` by index.
#[derive(Debug, Clone)]
pub struct LoadedTsConfig {
    /// The user's `default` export as a JSON value.
    pub config: serde_json::Value,
    /// Resolved plugin module specifiers (`file://` URLs), one per
    /// `config.plugins[]` entry in declaration order. Empty when the
    /// config declares no plugins.
    pub resolved_plugins: Vec<String>,
}

/// Evaluate a single `zfb.config.ts` file: bundle it through the compiler
/// worker, evaluate it (via embedded V8 on default features, via node on
/// slim builds), and resolve each plugin entry's module specifier.
///
/// `ts_path` is the config file; `dir` is the project root (the bundle's
/// working directory and the anchor for path-relative + bare-specifier
/// plugin resolution).
///
/// Returns the user's `default` export as a [`LoadedTsConfig`]. The caller
/// deserialises the value into its own config shape — this crate is
/// config-struct-agnostic so it can serve both `zfb` and `zfb-server`.
pub async fn load_from_ts_file(
    ts_path: &Path,
    dir: &Path,
    opts: &LoadOptions,
) -> Result<LoadedTsConfig> {
    if let Some(canned) = opts.test_default_export_json.as_deref() {
        return parse_loader_envelope(canned, ts_path);
    }

    #[cfg(feature = "direct-v8")]
    {
        load_ts_via_inprocess_v8(ts_path, dir, opts).await
    }
    #[cfg(not(feature = "direct-v8"))]
    {
        let json = load_ts_via_subprocess(ts_path, dir, opts).await?;
        parse_loader_envelope(&json, ts_path)
    }
}

/// A bundled config: the single self-contained ESM file plus the private
/// directory that owns it (dropping it removes the bundle and the stub).
struct BundledConfig {
    dir: tempfile::TempDir,
    bundle_path: PathBuf,
    graph: serde_json::Value,
}

/// Bundle `ts_path` (local imports, helper aliases and all) into one ESM
/// file inside a fresh private temp directory, through the supervised
/// compiler worker. The job deadline is [`CONFIG_SUBPROCESS_TIMEOUT`].
async fn bundle_config(
    ts_path: &Path,
    dir: &Path,
    opts: &LoadOptions,
    platform: Platform,
) -> Result<BundledConfig> {
    let worker = CompilerWorker::require(opts.compiler_worker.clone()).map_err(|error| {
        anyhow!(
            "config loader: cannot bundle {}: {error}. Set \
             `zfb_config_loader::LoadOptions::compiler_worker` to such a launcher",
            ts_path.display()
        )
    })?;

    // The worker resolves relative request paths against its own working
    // directory, so hand it absolute paths in the caller's spelling.
    let cwd = std::path::absolute(dir)
        .with_context(|| format!("config loader: cannot resolve {}", dir.display()))?;
    let entry = std::path::absolute(ts_path)
        .with_context(|| format!("config loader: cannot resolve {}", ts_path.display()))?;

    let tmp = tempfile::Builder::new()
        .prefix("zfb-config-")
        .tempdir()
        .context("config loader: failed to allocate temp directory")?;
    let stub_path = tmp.path().join("zfb-config-stub.mjs");
    let bundle_path = tmp.path().join("zfb-config-bundle.mjs");
    tokio::fs::write(&stub_path, CONFIG_STUB_MJS)
        .await
        .context("config loader: failed to stage zfb-config-stub.mjs")?;

    let stub = stub_path.to_string_lossy().into_owned();
    let mut request = BundleRequest::new(
        cwd,
        entry,
        OutputLayout::file(bundle_path.clone()),
        platform,
    );
    request.target = Some("esnext".to_owned());
    // Both spellings must work: the documented bare `zfb/config` and the
    // published-package form `@takazudo/zfb/config` real projects use.
    request.resolve.aliases = vec![
        ("zfb/config".to_owned(), stub.clone()),
        ("@takazudo/zfb/config".to_owned(), stub),
    ];

    let output = worker
        .compile(CompileJob::new(request, CONFIG_SUBPROCESS_TIMEOUT))
        .await
        .map_err(|error| bundle_error(ts_path, platform, error))?;

    Ok(BundledConfig {
        dir: tmp,
        bundle_path,
        graph: output.graph,
    })
}

fn bundle_error(ts_path: &Path, platform: Platform, error: WorkerError) -> anyhow::Error {
    match error {
        WorkerError::Compile { diagnostic } => {
            if platform == Platform::Neutral {
                let builtins = unresolved_node_builtins(&diagnostic);
                if !builtins.is_empty() {
                    return node_builtin_error(ts_path, &builtins);
                }
            }
            anyhow!(
                "config loader: failed to bundle {}:\n{}",
                ts_path.display(),
                diagnostic.trim_end()
            )
        }
        other => anyhow!(
            "config loader: compiler worker failed to bundle {}: {other}",
            ts_path.display()
        ),
    }
}

/// Node's builtin module names (`module.builtinModules`, unprefixed). A
/// `node:`-prefixed specifier is a builtin whatever follows the prefix.
const NODE_BUILTINS: &[&str] = &[
    "assert",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "domain",
    "events",
    "fs",
    "http",
    "http2",
    "https",
    "inspector",
    "module",
    "net",
    "os",
    "path",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "repl",
    "stream",
    "string_decoder",
    "sys",
    "timers",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];

fn is_node_builtin(specifier: &str) -> bool {
    if specifier.starts_with("node:") {
        return true;
    }
    let root = specifier.split('/').next().unwrap_or(specifier);
    NODE_BUILTINS.contains(&root)
}

/// Every external `node:*` or bare Node builtin import recorded in the
/// bundle graph, in first-seen order.
fn external_node_builtins(graph: &serde_json::Value) -> Vec<String> {
    let mut found = Vec::new();
    let sections = ["inputs", "outputs"];
    for section in sections {
        let Some(entries) = graph.get(section).and_then(|v| v.as_object()) else {
            continue;
        };
        for entry in entries.values() {
            let Some(imports) = entry.get("imports").and_then(|v| v.as_array()) else {
                continue;
            };
            for import in imports {
                if import.get("external").and_then(|v| v.as_bool()) != Some(true) {
                    continue;
                }
                let Some(path) = import.get("path").and_then(|v| v.as_str()) else {
                    continue;
                };
                if is_node_builtin(path) && !found.iter().any(|seen| seen == path) {
                    found.push(path.to_owned());
                }
            }
        }
    }
    found
}

/// Node builtins named by `Could not resolve "<specifier>"` lines of a
/// native diagnostic, in first-seen order.
fn unresolved_node_builtins(diagnostic: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in diagnostic.lines() {
        let Some((_, rest)) = line.split_once("Could not resolve ") else {
            continue;
        };
        let rest = rest.trim();
        let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            continue;
        };
        let Some(specifier) = rest[1..].split(quote).next() else {
            continue;
        };
        if is_node_builtin(specifier) && !found.iter().any(|seen| seen == specifier) {
            found.push(specifier.to_owned());
        }
    }
    found
}

fn node_builtin_error(ts_path: &Path, builtins: &[String]) -> anyhow::Error {
    anyhow!(
        "config loader: {} must be self-contained data config; it imports {}. Node \
         builtins are not available while zfb evaluates its config: compute the value \
         without them, or move the work into a plugin or a build step",
        ts_path.display(),
        builtins.join(", ")
    )
}

/// Boxed future returned by an attempt closure passed to [`output_bounded_with`].
///
/// The erased `Pin<Box<dyn Future<...>>>` seam avoids the borrow-checker
/// friction that arises when `FnMut() -> Fut` (with a named `Fut` type
/// parameter) re-borrows `&mut Command` across loop iterations.
pub type AttemptFut = std::pin::Pin<
    Box<
        dyn std::future::Future<
            Output = Result<std::io::Result<std::process::Output>, tokio::time::error::Elapsed>,
        >,
    >,
>;

/// Spawn `cmd` and wait for output, bounded by `timeout`.
///
/// `Command::output()` reads stdout/stderr pipes to EOF, so any grandchild
/// that inherits the pipe write-end (e.g. a detached node process) holds the
/// call forever at low CPU — the same unbounded-output() hang class fixed for
/// the post-dist vectors in #648 / #651. This helper bounds the wait and kills
/// the child (via `.kill_on_drop(true)`) when the deadline is exceeded.
///
/// The inner `io::Result<Output>` is returned unwrapped so callers can
/// inspect `io::ErrorKind` (e.g. `NotFound`) on their own code-path; only the
/// timeout itself becomes the outer `Err`.
pub async fn output_bounded(
    cmd: &mut Command,
    timeout: std::time::Duration,
    name: &str,
) -> Result<std::io::Result<std::process::Output>> {
    cmd.kill_on_drop(true);
    output_bounded_with(
        || Box::pin(tokio::time::timeout(timeout, cmd.output())) as AttemptFut,
        timeout,
        name,
    )
    .await
}

/// Generic helper that owns the ETXTBSY-classify/sleep/retry/exhaustion
/// logic for [`output_bounded`]. The `attempt` closure produces one
/// timeout-wrapped spawn attempt; this helper drives the retry loop.
pub async fn output_bounded_with<F>(
    mut attempt: F,
    timeout: std::time::Duration,
    name: &str,
) -> Result<std::io::Result<std::process::Output>>
where
    F: FnMut() -> AttemptFut,
{
    let mut etxtbsy_attempts = 0u32;
    loop {
        match attempt().await {
            // ETXTBSY ("Text file busy") spawn retry (zfb#1008): the binary
            // being spawned may have JUST been extracted to a tempfile
            // (render_pipeline::embedded_binary). If an unrelated thread
            // forks while the extraction's write fd is briefly open, the
            // forked child holds that fd until its own execve, and our
            // execve of the freshly-written file fails with ETXTBSY. The
            // window is fork-to-exec sized (microseconds) — a short bounded
            // retry absorbs it. Re-running is side-effect free precisely
            // because the discriminator guarantees it: `output()` bundles
            // spawn+wait, but ExecutableFileBusy can only originate from the
            // execve at spawn time — never from wait_with_output — so a
            // matching error means the child never ran.
            Ok(Err(e))
                if e.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && etxtbsy_attempts < ETXTBSY_MAX_RETRIES =>
            {
                etxtbsy_attempts += 1;
                tokio::time::sleep(ETXTBSY_RETRY_DELAY * etxtbsy_attempts).await;
            }
            Ok(res) => return Ok(res),
            Err(_) => bail!(
                "config loader: {} did not exit within {}s — killed",
                name,
                timeout.as_secs()
            ),
        }
    }
}

/// Bounded ETXTBSY spawn retries for [`output_bounded`] (zfb#1008). The race
/// window is fork-to-exec sized, so a handful of linearly backed-off retries
/// (10ms, 20ms, …) is far more than enough without delaying genuine failures.
pub const ETXTBSY_MAX_RETRIES: u32 = 5;
pub const ETXTBSY_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(10);

/// Generous backstop deadline for config-loader child processes: the compiler
/// worker job and the slim build's `node` evaluator. Mirrors the 300 s used by
/// `run_capturing` in `zfb-build` (see #648/#651) — a wedge guard, not a
/// performance bound.
pub const CONFIG_SUBPROCESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Bundle `ts_path` for the neutral platform and evaluate it in-process using
/// the embedded V8 isolate. Returns the resolved [`LoadedTsConfig`].
///
/// This is the data-config contract: `zfb.config.ts` must be self-contained.
/// Any `node:*` or bare Node builtin import is rejected from the bundle graph
/// (or from the native unresolved-import failure) before V8 evaluates
/// anything.
#[cfg(feature = "direct-v8")]
async fn load_ts_via_inprocess_v8(
    ts_path: &Path,
    dir: &Path,
    opts: &LoadOptions,
) -> Result<LoadedTsConfig> {
    use zfb_render::ThreadedConfigEvaluator;

    let bundled = bundle_config(ts_path, dir, opts, Platform::Neutral).await?;
    let builtins = external_node_builtins(&bundled.graph);
    if !builtins.is_empty() {
        return Err(node_builtin_error(ts_path, &builtins));
    }

    let bundle_src = tokio::fs::read_to_string(&bundled.bundle_path)
        .await
        .with_context(|| {
            format!(
                "config loader: failed to read the config bundle at {}",
                bundled.bundle_path.display()
            )
        })?;
    drop(bundled.dir);

    // Evaluate in a dedicated OS thread so V8 startup doesn't pin a
    // tokio worker (V8 boot can take hundreds of ms).
    let raw_value =
        tokio::task::spawn_blocking(move || ThreadedConfigEvaluator::eval_bundle(&bundle_src))
            .await
            .map_err(|e| anyhow!("config eval join error: {e}"))?
            .map_err(|e| anyhow!("embedded V8 evaluator failed: {e}"))?;

    // raw_value is the user's `default` export as a serde_json::Value.
    // Walk the plugins[] array and resolve each entry's `name` to a
    // file:// URL — unless the caller opted out (embed API: it only reads
    // the scalar fields and a missing plugin package must not fail the load).
    let resolved_plugins = if opts.resolve_plugins {
        resolve_plugins_from_value(&raw_value, dir)?
    } else {
        Vec::new()
    };

    Ok(LoadedTsConfig {
        config: raw_value,
        resolved_plugins,
    })
}

/// Walk the `plugins[]` array of an evaluated config value and resolve each
/// entry's `name` to a `file://` URL — relative/absolute paths convert
/// directly, bare specifiers resolve via `oxc_resolver`. Used by the
/// in-process V8 path, which gets the config as a value (the slim subprocess
/// path resolves plugins in `config-loader.mjs` and emits the envelope).
#[cfg(feature = "direct-v8")]
fn resolve_plugins_from_value(raw_value: &serde_json::Value, dir: &Path) -> Result<Vec<String>> {
    let plugins_arr = match raw_value.get("plugins") {
        None => vec![],
        Some(v) => v
            .as_array()
            .cloned()
            .ok_or_else(|| anyhow!("config loader: 'plugins' must be an array, got {}", v))?,
    };

    let mut resolved_plugins: Vec<String> = Vec::with_capacity(plugins_arr.len());
    for plugin_val in &plugins_arr {
        let name = plugin_val
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("config loader: plugin entry missing string 'name' field"))?;

        // relative/absolute path → file:// URL; bare specifier → node_resolve.
        let url = if let Some(file_url) = resolve_plugin_path_to_file_url(name, dir)? {
            file_url
        } else {
            // Bare specifier — resolve via oxc_resolver (in-process).
            resolve_node_bare_specifier(name, dir)
                .with_context(|| format!("config loader: resolving plugin {:?}", name))?
        };
        resolved_plugins.push(url);
    }
    Ok(resolved_plugins)
}

/// Resolve a single plugin `name` to a `file://` URL string for the
/// relative/absolute-path case.
///
/// Returns `Ok(Some(url))` for relative (`./`, `../`) or absolute (`/`)
/// paths, checking that the file actually exists. Returns `Ok(None)` for
/// bare specifiers — the caller resolves them via
/// [`crate::node_resolve::resolve_node_bare_specifier`] (both JSON and TS
/// paths share the same resolver since #418). Returns `Err` when a
/// relative/absolute path does not exist on disk.
pub fn resolve_plugin_path_to_file_url(name: &str, dir: &Path) -> Result<Option<String>> {
    let is_relative = name.starts_with("./") || name.starts_with("../");
    let is_absolute = name.starts_with('/');
    if is_relative || is_absolute {
        let raw_path = if is_relative {
            dir.join(name)
        } else {
            PathBuf::from(name)
        };
        let canonical = raw_path.canonicalize().with_context(|| {
            format!(
                "plugin {:?}: cannot resolve plugin file at {} \
                 (does the file exist?)",
                name,
                raw_path.display()
            )
        })?;
        let url = url::Url::from_file_path(&canonical).map_err(|()| {
            anyhow!(
                "plugin {:?}: failed to convert {} to a file:// URL",
                name,
                canonical.display()
            )
        })?;
        Ok(Some(url.into()))
    } else {
        // Bare specifier — caller handles.
        Ok(None)
    }
}

/// Internal envelope shape produced by the TS config evaluator.
///
/// On the default (direct-v8) path the in-process evaluator builds the
/// equivalent value in Rust. On the slim-build path `js/config-loader.mjs`
/// writes `{"config": <user-default-export>, "plugins": [<resolved-module-
/// specifier>, …]}` to stdout. Callers that supply `test_default_export_json`
/// can pass either the envelope shape or a bare config object — the
/// bare-config branch is kept for backwards-test-compat.
#[derive(Debug, serde::Deserialize)]
struct LoaderEnvelope {
    config: serde_json::Value,
    #[serde(default)]
    plugins: Vec<String>,
}

/// Parse the loader output (envelope or bare config object) into a
/// [`LoadedTsConfig`]. The envelope shape is tried first; a bare config
/// object falls back to an empty `resolved_plugins`.
fn parse_loader_envelope(json: &str, ts_path: &Path) -> Result<LoadedTsConfig> {
    // Try the envelope shape first.
    if let Ok(envelope) = serde_json::from_str::<LoaderEnvelope>(json) {
        let LoaderEnvelope {
            config,
            plugins: resolved,
        } = envelope;
        return Ok(LoadedTsConfig {
            config,
            resolved_plugins: resolved,
        });
    }
    // Backwards-compat: tests that pre-date the envelope supply the bare
    // config JSON directly via `test_default_export_json`. Accept that shape.
    let config: serde_json::Value = serde_json::from_str(json).map_err(|e| {
        anyhow!(
            "{}: failed to parse the default export as JSON \
             (line {}, column {}): {}\n--- received line {} ---\n{}",
            ts_path.display(),
            e.line(),
            e.column(),
            e,
            e.line(),
            json.lines()
                .nth(e.line().saturating_sub(1))
                .unwrap_or_default()
                .chars()
                .take(240)
                .collect::<String>()
        )
    })?;
    Ok(LoadedTsConfig {
        config,
        resolved_plugins: Vec::new(),
    })
}

/// Bundle `ts_path` for the Node platform through the compiler worker, then
/// run `node` to pull the default export back as JSON (envelope shape).
///
/// Slim-build fallback: available only when `direct-v8` is disabled. Node
/// evaluates the bundle, so builtins stay external and remain usable here,
/// exactly as before.
#[cfg(not(feature = "direct-v8"))]
async fn load_ts_via_subprocess(ts_path: &Path, dir: &Path, opts: &LoadOptions) -> Result<String> {
    let bundled = bundle_config(ts_path, dir, opts, Platform::Node).await?;
    let loader_path = bundled.dir.path().join("config-loader.mjs");
    tokio::fs::write(&loader_path, CONFIG_LOADER_MJS)
        .await
        .context("config loader: failed to stage config-loader.mjs")?;

    // Same cwd as the bundle so any runtime resolution that escapes the
    // bundle (it shouldn't) still anchors at the project root.
    let node_bin: OsString = opts
        .node_binary
        .clone()
        .unwrap_or_else(|| OsString::from("node"));
    let mut node_cmd = Command::new(&node_bin);
    node_cmd.current_dir(dir);
    node_cmd.arg(&loader_path);
    node_cmd.arg(&bundled.bundle_path);
    // Project root — the loader uses this for path-relative plugin
    // resolution and for bare-specifier `node_modules` lookup.
    node_cmd.arg(dir);
    // Opt out of plugin resolution for callers that only read the scalar
    // fields (embed API): a CLI-only plugin whose package is absent from
    // the deployment must not fail the load. The loader emits an empty
    // `plugins` array when this flag is present.
    if !opts.resolve_plugins {
        node_cmd.arg("--no-resolve-plugins");
    }

    // Bounded wait — guards against the unbounded-output() hang class (#648/#651).
    let node_out = match output_bounded(&mut node_cmd, CONFIG_SUBPROCESS_TIMEOUT, "node").await? {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!(
                "config loader: `{}` was not found in PATH. This zfb build (without the \
                 embedded V8 evaluator) requires Node.js to load `zfb.config.ts`. \
                 Install Node.js — https://nodejs.org/ \
                 — or point zfb at a node binary by setting the `ZFB_NODE_BIN` \
                 env var on a future zfb release.",
                node_bin.to_string_lossy()
            );
        }
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!(
                "config loader: failed to spawn `{}`",
                node_bin.to_string_lossy()
            )));
        }
    };
    if !node_out.status.success() {
        let stderr = String::from_utf8_lossy(&node_out.stderr);
        bail!(
            "config loader: node failed evaluating {} ({}): {}",
            ts_path.display(),
            node_out.status,
            stderr.trim()
        );
    }

    let stdout = String::from_utf8(node_out.stdout).map_err(|e| {
        // Echo the lossy form so the operator can see what node actually
        // emitted — much more useful than a bare "not valid UTF-8".
        let bytes = e.as_bytes();
        anyhow!(
            "config loader: node stdout for {} was not valid UTF-8 ({} bytes): {}",
            ts_path.display(),
            bytes.len(),
            String::from_utf8_lossy(bytes)
        )
    })?;
    Ok(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn envelope_shape_is_parsed_with_resolved_plugins() {
        // The envelope format: `{ config, plugins: [...resolved-specifiers...] }`.
        // parse_loader_envelope keeps config + plugins separate so the caller
        // can zip the specifiers onto its own deserialised plugins[] by index.
        let ts_path = Path::new("/proj/zfb.config.ts");
        let json = r#"{
            "config": {
                "plugins": [
                    { "name": "@example/zfb-plugin-search", "options": { "level": 2 } },
                    { "name": "./plugins/local.mjs" }
                ]
            },
            "plugins": [
                "file:///abs/node_modules/@example/zfb-plugin-search/index.js",
                "file:///abs/project/plugins/local.mjs"
            ]
        }"#;
        let loaded = parse_loader_envelope(json, ts_path).expect("envelope parses");
        assert_eq!(loaded.resolved_plugins.len(), 2);
        assert_eq!(
            loaded.resolved_plugins[0],
            "file:///abs/node_modules/@example/zfb-plugin-search/index.js"
        );
        let plugins = loaded.config.get("plugins").and_then(|v| v.as_array());
        assert_eq!(plugins.map(|a| a.len()), Some(2));
    }

    #[tokio::test]
    async fn bare_config_object_falls_back_to_empty_plugins() {
        // A bare `default` export object (no envelope wrapper) is accepted
        // for backwards-test-compat, with empty resolved_plugins.
        let ts_path = Path::new("/proj/zfb.config.ts");
        let loaded = parse_loader_envelope(r#"{"port": 4000}"#, ts_path).expect("bare object");
        assert!(loaded.resolved_plugins.is_empty());
        assert_eq!(
            loaded.config.get("port").and_then(|v| v.as_u64()),
            Some(4000)
        );
    }

    #[tokio::test]
    async fn invalid_json_includes_payload_and_file() {
        // Garbage stdout must not be silently accepted — the error names the
        // file and echoes the received payload to aid debugging.
        let ts_path = Path::new("/proj/zfb.config.ts");
        let err = parse_loader_envelope("not-json-at-all", ts_path)
            .expect_err("garbage must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("zfb.config.ts"),
            "msg should name the file: {msg}"
        );
        assert!(
            msg.contains("received"),
            "msg should echo the payload: {msg}"
        );
    }

    #[tokio::test]
    async fn load_from_ts_file_uses_test_override_envelope() {
        // The test override short-circuits bundling/V8/node entirely.
        let tmp = TempDir::new().unwrap();
        let ts_path = tmp.path().join("zfb.config.ts");
        tokio::fs::write(&ts_path, "export default {};\n")
            .await
            .unwrap();
        let opts = LoadOptions {
            test_default_export_json: Some(r#"{"config": {"port": 9999}, "plugins": []}"#.into()),
            ..LoadOptions::default()
        };
        let loaded = load_from_ts_file(&ts_path, tmp.path(), &opts)
            .await
            .expect("override path");
        assert_eq!(
            loaded.config.get("port").and_then(|v| v.as_u64()),
            Some(9999)
        );
    }

    /// `LoadOptions::default()` MUST resolve plugins — the CLI / `zfb` bin
    /// path relies on it and must not change. Guards against accidentally
    /// flipping the default when the embed opt-out was added (issue #1037).
    #[test]
    fn load_options_default_resolves_plugins() {
        assert!(
            LoadOptions::default().resolve_plugins,
            "default must keep resolving plugins for the CLI path"
        );
    }

    /// Slim-build (`--no-default-features`) regression for the embed opt-out
    /// (issue #1037): drive the staged `config-loader.mjs` with node directly
    /// — no bundling, no V8 — against a config that lists a bare plugin that
    /// cannot resolve. Without the flag the loader FAILS at resolution; with
    /// `--no-resolve-plugins` it succeeds and emits an empty `plugins` array,
    /// which is exactly the embed path's behaviour. The 4 scalar config
    /// fields survive in the envelope either way.
    #[cfg(not(feature = "direct-v8"))]
    #[tokio::test]
    async fn no_resolve_plugins_flag_skips_unresolvable_plugin() {
        if Command::new("node")
            .arg("--version")
            .output()
            .await
            .is_err()
        {
            eprintln!("skipping: node not available on PATH");
            return;
        }

        let tmp = TempDir::new().unwrap();
        let loader_path = tmp.path().join("config-loader.mjs");
        tokio::fs::write(&loader_path, CONFIG_LOADER_MJS)
            .await
            .unwrap();
        // A pre-bundled ESM module standing in for the compiler worker output: a config
        // whose `plugins[]` names a bare specifier with no `node_modules`
        // entry anywhere under the (empty) project root — unresolvable.
        let bundle_path = tmp.path().join("bundle.mjs");
        tokio::fs::write(
            &bundle_path,
            "export default { outDir: \"out\", publicDir: \"pub\", base: \"/app\", \
             trailingSlash: true, plugins: [{ name: \"@absent/zfb-plugin-ghost\" }] };\n",
        )
        .await
        .unwrap();

        let run = |extra: Option<&'static str>| {
            let loader_path = loader_path.clone();
            let bundle_path = bundle_path.clone();
            let project_root = tmp.path().to_path_buf();
            async move {
                let mut cmd = Command::new("node");
                cmd.arg(&loader_path).arg(&bundle_path).arg(&project_root);
                if let Some(flag) = extra {
                    cmd.arg(flag);
                }
                cmd.output().await.expect("spawn node")
            }
        };

        // Without the flag: bare-specifier resolution fails the load.
        let without = run(None).await;
        assert!(
            !without.status.success(),
            "an unresolvable plugin must fail when resolution is on; stdout: {}",
            String::from_utf8_lossy(&without.stdout)
        );

        // With the flag: load succeeds, plugins resolved to empty, scalars kept.
        let with = run(Some("--no-resolve-plugins")).await;
        assert!(
            with.status.success(),
            "--no-resolve-plugins must skip resolution and succeed; stderr: {}",
            String::from_utf8_lossy(&with.stderr)
        );
        let json = String::from_utf8(with.stdout).unwrap();
        let loaded = parse_loader_envelope(&json, &bundle_path).expect("envelope parses");
        assert!(
            loaded.resolved_plugins.is_empty(),
            "resolved_plugins must be empty when resolution is skipped"
        );
        let cfg: EmbedTestScalars =
            serde_json::from_value(loaded.config).expect("scalar fields deserialise");
        assert_eq!(cfg.out_dir.as_deref(), Some("out"));
        assert_eq!(cfg.public_dir.as_deref(), Some("pub"));
        assert_eq!(cfg.base.as_deref(), Some("/app"));
        assert_eq!(cfg.trailing_slash, Some(true));
    }

    /// Mirrors `zfb-server`'s `EmbedConfig` scalar subset (camelCase) so the
    /// test can prove the embed-relevant fields survive a plugins-skipped
    /// load without depending on the `zfb-server` crate.
    #[cfg(not(feature = "direct-v8"))]
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct EmbedTestScalars {
        out_dir: Option<String>,
        public_dir: Option<String>,
        base: Option<String>,
        trailing_slash: Option<bool>,
    }

    #[test]
    fn resolve_plugin_path_to_file_url_returns_none_for_bare_specifier() {
        // Bare specifiers are the caller's job (node_resolve) — None signals that.
        let dir = Path::new("/proj");
        assert_eq!(
            resolve_plugin_path_to_file_url("@scope/pkg", dir).unwrap(),
            None
        );
        assert_eq!(
            resolve_plugin_path_to_file_url("pkg-name", dir).unwrap(),
            None
        );
    }

    #[test]
    fn resolve_plugin_path_to_file_url_errors_when_relative_file_missing() {
        let dir = Path::new("/nonexistent-proj-xyz");
        let err = resolve_plugin_path_to_file_url("./plugins/missing.mjs", dir)
            .expect_err("missing relative plugin file should error");
        let msg = format!("{err:#}");
        assert!(msg.contains("does the file exist"), "msg: {msg}");
    }

    // --- resolve_plugins_from_value tests ---------------------------------------

    /// A missing `plugins` key is a clean no-plugins load (not an error).
    #[cfg(feature = "direct-v8")]
    #[test]
    fn resolve_plugins_from_value_absent_plugins_returns_empty() {
        let value = serde_json::json!({ "port": 3000 });
        let result = resolve_plugins_from_value(&value, Path::new("/proj"))
            .expect("absent plugins must not error");
        assert!(result.is_empty(), "expected empty vec, got: {result:?}");
    }

    /// A `plugins` value that is present but not an array (e.g. an accidental
    /// object typo) must return a descriptive error rather than silently
    /// treating the config as plugin-free.
    #[cfg(feature = "direct-v8")]
    #[test]
    fn resolve_plugins_from_value_non_array_plugins_errors() {
        let value = serde_json::json!({ "plugins": { "name": "oops-should-be-array" } });
        let err = resolve_plugins_from_value(&value, Path::new("/proj"))
            .expect_err("non-array plugins must error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("plugins") && msg.contains("array"),
            "error should mention 'plugins' and 'array'; got: {msg}"
        );
    }

    // --- output_bounded tests ---------------------------------------------------

    /// Verify that `output_bounded` returns a named timeout error promptly when
    /// the spawned child keeps stdout open (via a backgrounded grandchild), which
    /// would hang `Command::output()` indefinitely.
    ///
    /// Mirrors the intent of `run_capturing_returns_promptly_when_grandchild_holds_pipe_open`
    /// in `crates/zfb-build/src/adapter.rs` (#648 / #651), adapted to the async path.
    #[cfg(unix)]
    #[tokio::test]
    async fn output_bounded_returns_timeout_error_when_grandchild_holds_pipe_open() {
        let mut cmd = tokio::process::Command::new("sh");
        // `sleep 30 &` backgrounds a grandchild that inherits the pipe
        // write-end; without the timeout this would block for 30 s.
        cmd.arg("-c").arg("sleep 30 & exit 0");

        // Short timeout so the test itself is fast.
        let timeout = std::time::Duration::from_secs(2);

        // Outer watchdog: if output_bounded itself hangs (regression), the
        // test times out in 10 s and fails RED rather than hanging CI.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            output_bounded(&mut cmd, timeout, "test-stub"),
        )
        .await
        .expect("output_bounded did not return within 10 s — pipe-EOF hang regression");

        let err = result.expect_err("output_bounded should have returned a timeout error");
        let msg = err.to_string();
        assert!(
            msg.contains("did not exit within"),
            "error must name the timeout; got: {msg}"
        );
        assert!(
            msg.contains("test-stub"),
            "error must name the subprocess; got: {msg}"
        );
    }

    /// Non-ETXTBSY spawn errors pass straight through the zfb#1008 retry
    /// loop in `output_bounded` — only `ExecutableFileBusy` is retried, so
    /// a missing binary surfaces its `NotFound` immediately as the inner
    /// `io::Result`, preserving the callers' `io::ErrorKind` inspection
    /// contract.
    #[tokio::test]
    async fn output_bounded_passes_through_non_etxtbsy_spawn_errors() {
        let mut cmd = tokio::process::Command::new("/nonexistent/zfb-output-bounded-test-binary");
        let result = output_bounded(&mut cmd, std::time::Duration::from_secs(5), "missing-bin")
            .await
            .expect("spawn failure is the inner io::Result, not the timeout Err");
        let err = result.expect_err("spawning a nonexistent binary must fail");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    // --- output_bounded_with injectable-seam tests --------------------------------

    /// Helper: build the `Pin<Box<dyn Future<...>>>` type alias expected by
    /// `output_bounded_with` from a pre-resolved `io::Result<Output>`.
    ///
    /// `#[cfg(unix)]` because `ExitStatusExt::from_raw` is Unix-only.
    #[cfg(unix)]
    fn make_attempt(result: std::io::Result<std::process::Output>) -> AttemptFut {
        Box::pin(std::future::ready(Ok(result)))
    }

    /// `output_bounded_with` retries `ExecutableFileBusy` errors and
    /// succeeds on the third attempt. Virtual time (`start_paused`) keeps
    /// the linear-backoff sleeps instant.
    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn output_bounded_with_retries_etxtbsy_then_succeeds() {
        use std::os::unix::process::ExitStatusExt as _;

        let counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter_clone = counter.clone();

        // Attempt returns ETXTBSY twice, then succeeds.
        let attempt = move || {
            let n = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let result: std::io::Result<std::process::Output> = if n < 2 {
                Err(std::io::Error::from(std::io::ErrorKind::ExecutableFileBusy))
            } else {
                Ok(std::process::Output {
                    status: std::process::ExitStatus::from_raw(0),
                    stdout: vec![],
                    stderr: vec![],
                })
            };
            make_attempt(result)
        };

        let result = output_bounded_with(
            attempt,
            std::time::Duration::from_secs(300),
            "test-retry-succeed",
        )
        .await
        .expect("should succeed after retries");

        assert!(result.is_ok(), "inner result should be Ok on success");
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "expected exactly 3 attempts (2 ETXTBSY + 1 success)"
        );
    }

    /// `output_bounded_with` exhausts all ETXTBSY retries and surfaces the
    /// `ExecutableFileBusy` error as the inner `io::Result`. Virtual time keeps
    /// the linear-backoff sleeps instant.
    ///
    /// Total attempts = `ETXTBSY_MAX_RETRIES + 1` = 6:
    /// one initial attempt plus up to 5 retries (guard: `etxtbsy_attempts < 5`).
    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn output_bounded_with_exhausts_retries_and_surfaces_etxtbsy() {
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter_clone = counter.clone();

        let attempt = move || {
            counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            make_attempt(Err(std::io::Error::from(
                std::io::ErrorKind::ExecutableFileBusy,
            )))
        };

        let result = output_bounded_with(
            attempt,
            std::time::Duration::from_secs(300),
            "test-exhaustion",
        )
        .await
        .expect("exhausted retries should surface as inner Err, not outer bail");

        let err = result.expect_err("should be Err after exhaustion");
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::ExecutableFileBusy,
            "exhausted error must be ExecutableFileBusy"
        );
        // Verify the actual loop contract: 1 initial + ETXTBSY_MAX_RETRIES retries.
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::SeqCst),
            ETXTBSY_MAX_RETRIES + 1,
            "expected {} total attempts (1 initial + {} retries)",
            ETXTBSY_MAX_RETRIES + 1,
            ETXTBSY_MAX_RETRIES,
        );
    }
}
