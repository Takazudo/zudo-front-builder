//! Project configuration loader.
//!
//! Loads `zfb.config.ts` or `zfb.config.json` from the project root and
//! parses it into a strongly-typed [`Config`] struct.
//!
//! ## Bootstrap rule
//!
//! The JS runtime used to *parse* this config is fixed by the zfb binary
//! itself — the config CANNOT choose its own runtime. The config CAN choose
//! `framework: "preact" | "react"` (applied after the config is loaded by the
//! framework adapter), `outDir`, `publicDir`, content + Tailwind options, and
//! plugins. There is exactly one runtime; it is not user-overridable in v1.
//!
//! See issue #9 (Wave 2 / Sub 3).
//!
//! ## Resolution order
//!
//! [`load_from_dir`] picks exactly one source per call:
//!
//! 1. `dir/zfb.config.json` — read + parse via `serde_json`. Wins over
//!    `zfb.config.ts` when both are present (back-compat: existing users on
//!    the JSON form keep working unchanged when the TS loader lands).
//! 2. `dir/zfb.config.ts` — bundled to ESM by the pinned `esbuild` binary
//!    (the same one zfb-islands uses), then evaluated by `node` to pull the
//!    default export back as JSON, which is fed into `serde_json::from_str`.
//! 3. Neither present — return [`Config::default`].
//!
//! TS support requires `node` in `PATH` and the staged esbuild binary at
//! `crates/zfb/binaries/esbuild/esbuild` (or the path pointed at by the
//! `ZFB_ESBUILD_BIN` environment variable). `node` was already a hard
//! requirement of `zfb` because the production renderer spawns miniflare;
//! this module surfaces a clean error if it is missing.
//!
//! All produced configs pass [`validate`] before they are returned so
//! callers don't have to think about it.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// Top-level `zfb.config.{ts,json}` schema.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Output directory for built assets. Default: `dist`.
    #[serde(default = "default_out_dir")]
    pub out_dir: PathBuf,

    /// Public/static directory copied verbatim. Default: `public`.
    #[serde(default = "default_public_dir")]
    pub public_dir: PathBuf,

    /// Optional dev/preview server bind host. When absent, the consuming
    /// command (`zfb dev`) falls back to its built-in default (`localhost`).
    /// Made optional so the CLI can layer "flag > config > built-in" cleanly.
    #[serde(default)]
    pub host: Option<String>,

    /// Optional dev/preview server port. When absent, the consuming command
    /// falls back to its built-in default (`3000` for `zfb dev`, `4321`
    /// for `zfb preview`). Optional for the same reason as [`host`].
    #[serde(default)]
    pub port: Option<u16>,

    /// JSX framework runtime. Default: `Preact`.
    #[serde(default)]
    pub framework: Framework,

    /// Content collections.
    #[serde(default)]
    pub collections: Vec<CollectionDef>,

    /// Tailwind-specific config; absent = default behavior.
    #[serde(default)]
    pub tailwind: Option<TailwindConfig>,

    /// User-supplied plugins.
    #[serde(default)]
    pub plugins: Vec<PluginConfig>,

    /// Deploy-target adapter package name. `None` (or omitted) means
    /// pure-static build — any route exporting `prerender = false` is
    /// rejected at build time. A package name like
    /// `"@takazudo/zfb-adapter-cloudflare"` selects the matching
    /// adapter; the build then invokes that package's bin to wrap the
    /// SSR bundle into a deploy-ready entry (e.g. `dist/_worker.js`).
    ///
    /// Accepted shapes: `None`, omitted, the literal string `"none"`,
    /// or any non-empty package name. Empty / whitespace-only strings
    /// are rejected at parse time so a typo doesn't silently fall back
    /// to no-adapter.
    ///
    /// See [`zfb_build::AdapterChoice::from_config`] for the parser
    /// the build orchestrator runs against this field.
    #[serde(default)]
    pub adapter: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            out_dir: default_out_dir(),
            public_dir: default_public_dir(),
            host: None,
            port: None,
            framework: Framework::default(),
            collections: Vec::new(),
            tailwind: None,
            plugins: Vec::new(),
            adapter: None,
        }
    }
}

/// JSX runtime selection. `Preact` is the v1 default.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Framework {
    #[default]
    Preact,
    React,
}

/// One content collection (e.g. blog posts under `content/blog/`).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CollectionDef {
    /// Identifier used at the call site (e.g. `"blog"`).
    pub name: String,
    /// Directory (relative to the project root) holding the entries.
    pub path: PathBuf,
    /// Optional schema. Reserved for v1.1 — accepted today but not enforced.
    #[serde(default)]
    pub schema: Option<serde_json::Value>,
}

/// Tailwind options. Empty by default (Tailwind enabled); users can flip
/// `enabled: false` to opt out.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TailwindConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for TailwindConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// One user plugin entry.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PluginConfig {
    pub name: String,
    #[serde(default)]
    pub options: serde_json::Value,
}

// --- defaults --------------------------------------------------------------

fn default_out_dir() -> PathBuf {
    PathBuf::from("dist")
}

fn default_public_dir() -> PathBuf {
    PathBuf::from("public")
}

fn default_true() -> bool {
    true
}

// --- loader ----------------------------------------------------------------

/// JS payload run by `node` to dynamic-import the bundled config and emit
/// the default export as JSON on stdout. Embedded into the binary so we
/// don't have to ship a sidecar file at runtime.
const CONFIG_LOADER_MJS: &str = include_str!("../js/config-loader.mjs");

/// Stub for the `zfb/config` import that user TS configs reach for. We
/// alias `zfb/config` → this stub at esbuild time so the user's project
/// does not need the `zfb` npm package installed locally just to be
/// parsed.
const CONFIG_STUB_MJS: &str = include_str!("../js/zfb-config-stub.mjs");

/// Default location of the staged esbuild CLI, mirroring
/// `zfb_build::bundler::DEFAULT_ESBUILD_SLOT`. Resolved relative to the
/// process working directory; release packaging stages the real binary
/// here.
const DEFAULT_ESBUILD_SLOT: &str = "crates/zfb/binaries/esbuild/esbuild";

/// Knobs that tweak loader behaviour. Public so build/dev/preview can
/// thread an explicit esbuild override through if they ever need to;
/// `Default` is the production path.
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Override the esbuild binary path. `None` falls back to
    /// `ZFB_ESBUILD_BIN`, then [`DEFAULT_ESBUILD_SLOT`].
    pub esbuild_binary: Option<PathBuf>,
    /// Override the `node` binary. `None` uses `node` from `PATH`.
    pub node_binary: Option<OsString>,
    /// Test-only escape hatch: when `Some`, skip the esbuild + node
    /// subprocesses entirely and treat this string as the JSON form
    /// of `default` from the user's `zfb.config.ts`. Production code
    /// leaves this `None`.
    #[doc(hidden)]
    pub test_default_export_json: Option<String>,
}

/// Load and validate a config from `dir`.
///
/// See the module docs for the resolution order.
pub async fn load_from_dir(dir: &Path) -> Result<Config> {
    load_from_dir_with_options(dir, &LoadOptions::default()).await
}

/// Variant of [`load_from_dir`] with explicit knobs. Most callers want
/// [`load_from_dir`].
pub async fn load_from_dir_with_options(
    dir: &Path,
    opts: &LoadOptions,
) -> Result<Config> {
    let ts_path = dir.join("zfb.config.ts");
    let json_path = dir.join("zfb.config.json");

    // JSON wins over TS for back-compat (see module docs).
    if json_path.exists() {
        let text = tokio::fs::read_to_string(&json_path)
            .await
            .with_context(|| format!("reading {}", json_path.display()))?;
        let cfg: Config = serde_json::from_str(&text).map_err(|e| {
            anyhow!(
                "{}: invalid config JSON at line {}, column {}: {}",
                json_path.display(),
                e.line(),
                e.column(),
                e
            )
        })?;
        validate(&cfg, dir).with_context(|| format!("validating {}", json_path.display()))?;
        return Ok(cfg);
    }

    if ts_path.exists() {
        let cfg = load_from_ts_file(&ts_path, dir, opts)
            .await
            .with_context(|| format!("loading {}", ts_path.display()))?;
        validate(&cfg, dir).with_context(|| format!("validating {}", ts_path.display()))?;
        return Ok(cfg);
    }

    // No file present → defaults.
    let cfg = Config::default();
    // Defaults are always valid, but we still run the check so future
    // additions can't accidentally break this invariant.
    validate(&cfg, dir).expect("Config::default() must validate cleanly");
    Ok(cfg)
}

/// Load a single `zfb.config.ts` file: bundle it with esbuild, evaluate
/// it with node, parse the JSON of the default export.
async fn load_from_ts_file(
    ts_path: &Path,
    dir: &Path,
    opts: &LoadOptions,
) -> Result<Config> {
    let json = if let Some(canned) = opts.test_default_export_json.as_deref() {
        canned.to_string()
    } else {
        load_ts_via_subprocess(ts_path, dir, opts).await?
    };
    let cfg: Config = serde_json::from_str(&json).map_err(|e| {
        anyhow!(
            "{}: failed to parse the default export as zfb config JSON \
             (line {}, column {}): {}\n--- received ---\n{}",
            ts_path.display(),
            e.line(),
            e.column(),
            e,
            json
        )
    })?;
    Ok(cfg)
}

/// Resolve the esbuild binary path using the same precedence the
/// build-time bundler uses: explicit override > `ZFB_ESBUILD_BIN` env >
/// the staged slot under `crates/zfb/binaries/esbuild/`.
fn resolve_esbuild_binary(opts: &LoadOptions) -> Result<PathBuf> {
    if let Some(p) = opts.esbuild_binary.as_deref() {
        if !p.exists() {
            bail!(
                "config loader: esbuild binary not found at explicit path {}",
                p.display()
            );
        }
        return Ok(p.to_path_buf());
    }
    if let Some(env) = std::env::var_os("ZFB_ESBUILD_BIN") {
        let p = PathBuf::from(env);
        if !p.exists() {
            bail!(
                "config loader: esbuild binary not found at ZFB_ESBUILD_BIN={}",
                p.display()
            );
        }
        return Ok(p);
    }
    let slot = PathBuf::from(DEFAULT_ESBUILD_SLOT);
    if !slot.exists() {
        return Err(anyhow!(
            "config loader: esbuild binary not found at default slot {}. \
             Either set ZFB_ESBUILD_BIN to a usable esbuild CLI, or stage \
             the binary at the slot path. (See crates/zfb/binaries/esbuild/README.md.)",
            slot.display()
        ));
    }
    Ok(slot)
}

/// Run esbuild + node to compile `ts_path` to ESM and pull the default
/// export back as JSON.
async fn load_ts_via_subprocess(
    ts_path: &Path,
    dir: &Path,
    opts: &LoadOptions,
) -> Result<String> {
    let esbuild = resolve_esbuild_binary(opts)?;

    // Stage the embedded helper scripts and esbuild output into a
    // tempdir that vanishes at the end of this function.
    let tmp = tempfile::Builder::new()
        .prefix("zfb-config-")
        .tempdir()
        .context("config loader: failed to allocate temp directory")?;
    let stub_path = tmp.path().join("zfb-config-stub.mjs");
    let loader_path = tmp.path().join("config-loader.mjs");
    let bundle_path = tmp.path().join("zfb-config-bundle.mjs");
    tokio::fs::write(&stub_path, CONFIG_STUB_MJS)
        .await
        .context("config loader: failed to stage zfb-config-stub.mjs")?;
    tokio::fs::write(&loader_path, CONFIG_LOADER_MJS)
        .await
        .context("config loader: failed to stage config-loader.mjs")?;

    // 1. Bundle the user's TS file. Run with `dir` as cwd so any
    //    relative imports the user wrote (e.g. `./constants`) resolve
    //    against the project root.
    let alias_arg = format!("--alias:zfb/config={}", stub_path.display());
    let outfile_arg = format!("--outfile={}", bundle_path.display());
    let mut cmd = Command::new(&esbuild);
    cmd.current_dir(dir);
    cmd.arg("--bundle");
    cmd.arg("--format=esm");
    cmd.arg("--platform=node");
    cmd.arg("--target=esnext");
    cmd.arg("--log-level=warning");
    cmd.arg(&alias_arg);
    cmd.arg(&outfile_arg);
    cmd.arg(ts_path);

    let esbuild_out = cmd.output().await.with_context(|| {
        format!(
            "config loader: failed to spawn esbuild at {}",
            esbuild.display()
        )
    })?;
    if !esbuild_out.status.success() {
        let stderr = String::from_utf8_lossy(&esbuild_out.stderr);
        bail!(
            "config loader: esbuild failed to bundle {} ({}): {}",
            ts_path.display(),
            esbuild_out.status,
            stderr.trim()
        );
    }

    // 2. Run node against the loader script to print JSON of the default
    //    export. Same cwd so any runtime resolution that escapes the
    //    bundle (it shouldn't) still anchors at the project root.
    let node_bin: OsString = opts
        .node_binary
        .clone()
        .unwrap_or_else(|| OsString::from("node"));
    let mut node_cmd = Command::new(&node_bin);
    node_cmd.current_dir(dir);
    node_cmd.arg(&loader_path);
    node_cmd.arg(&bundle_path);

    let node_out = match node_cmd.output().await {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!(
                "config loader: `{}` was not found in PATH. zfb requires \
                 Node.js to load `zfb.config.ts` (and to run the production \
                 renderer via miniflare). Install Node.js — https://nodejs.org/ \
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

    let stdout = String::from_utf8(node_out.stdout)
        .context("config loader: node stdout was not valid UTF-8")?;
    Ok(stdout)
}

/// Validate a loaded [`Config`] against the project root `dir`.
///
/// Errors:
/// - duplicate `collections[].name`.
/// - `collections[].path` that is absolute or escapes `dir` via `..`.
fn validate(cfg: &Config, dir: &Path) -> Result<()> {
    let mut seen: HashSet<&str> = HashSet::new();
    for c in &cfg.collections {
        if !seen.insert(c.name.as_str()) {
            bail!("duplicate collection name {:?}", c.name);
        }
        ensure_path_in_root(&c.path, dir)
            .with_context(|| format!("collection {:?}", c.name))?;
    }
    Ok(())
}

/// Ensure `path` is relative and stays under `dir` (no `..` escapes, no
/// absolute paths). The path is checked **syntactically** — we do not require
/// it to exist, since collections may be created after the config.
///
/// Limitation: symlinks are not resolved here. If the project allows
/// symlinks in content dirs and that's a concern, callers should
/// `canonicalize()` the resolved path at use-site before reading. zfb's v1
/// trust model treats project files as user-owned, so this matches the
/// surrounding crates.
fn ensure_path_in_root(path: &Path, dir: &Path) -> Result<()> {
    if path.is_absolute() {
        bail!(
            "path {:?} must be relative to the project root ({})",
            path,
            dir.display()
        );
    }
    let mut depth: i32 = 0;
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    bail!(
                        "path {:?} escapes the project root via `..`",
                        path
                    );
                }
            }
            Component::Normal(_) | Component::CurDir => {
                if matches!(comp, Component::Normal(_)) {
                    depth += 1;
                }
            }
            // Prefix / RootDir would have been caught by `is_absolute` above,
            // but be defensive.
            Component::Prefix(_) | Component::RootDir => {
                bail!(
                    "path {:?} must be relative to the project root ({})",
                    path,
                    dir.display()
                );
            }
        }
    }
    Ok(())
}

// --- tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_config_has_sensible_values() {
        let cfg = Config::default();
        assert_eq!(cfg.out_dir, PathBuf::from("dist"));
        assert_eq!(cfg.public_dir, PathBuf::from("public"));
        assert_eq!(cfg.host, None);
        assert_eq!(cfg.port, None);
        assert_eq!(cfg.framework, Framework::Preact);
        assert!(cfg.collections.is_empty());
        assert!(cfg.tailwind.is_none());
        assert!(cfg.plugins.is_empty());
    }

    #[tokio::test]
    async fn empty_dir_returns_default_config() {
        let tmp = TempDir::new().unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(cfg, Config::default());
    }

    #[tokio::test]
    async fn loads_from_json_fixture() {
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "outDir": "build",
            "publicDir": "static",
            "host": "0.0.0.0",
            "port": 4000,
            "framework": "react",
            "collections": [
                { "name": "blog", "path": "content/blog" },
                { "name": "docs", "path": "content/docs" }
            ],
            "tailwind": { "enabled": false },
            "plugins": [
                { "name": "my-plugin", "options": { "level": 2 } }
            ]
        }"#;
        tokio::fs::write(tmp.path().join("zfb.config.json"), json)
            .await
            .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(cfg.out_dir, PathBuf::from("build"));
        assert_eq!(cfg.public_dir, PathBuf::from("static"));
        assert_eq!(cfg.host.as_deref(), Some("0.0.0.0"));
        assert_eq!(cfg.port, Some(4000));
        assert_eq!(cfg.framework, Framework::React);
        assert_eq!(cfg.collections.len(), 2);
        assert_eq!(cfg.collections[0].name, "blog");
        assert_eq!(cfg.collections[1].path, PathBuf::from("content/docs"));
        assert_eq!(
            cfg.tailwind,
            Some(TailwindConfig { enabled: false })
        );
        assert_eq!(cfg.plugins.len(), 1);
        assert_eq!(cfg.plugins[0].name, "my-plugin");
    }

    #[tokio::test]
    async fn loads_minimal_json_with_defaults() {
        // Empty object should round-trip to all defaults.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.json"), "{}")
            .await
            .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("load ok");
        assert_eq!(cfg, Config::default());
    }

    #[tokio::test]
    async fn invalid_json_reports_position() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            "{ \"port\": \"not-a-number\" }",
        )
        .await
        .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("should reject");
        let msg = format!("{err:#}");
        assert!(msg.contains("zfb.config.json"), "msg: {msg}");
        assert!(msg.contains("line"), "msg: {msg}");
    }

    #[tokio::test]
    async fn rejects_duplicate_collection_names() {
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "collections": [
                { "name": "blog", "path": "content/blog" },
                { "name": "blog", "path": "content/blog2" }
            ]
        }"#;
        tokio::fs::write(tmp.path().join("zfb.config.json"), json)
            .await
            .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("should reject duplicates");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("duplicate collection name"),
            "msg: {msg}"
        );
    }

    #[tokio::test]
    async fn rejects_absolute_collection_path() {
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "collections": [
                { "name": "blog", "path": "/etc/passwd" }
            ]
        }"#;
        tokio::fs::write(tmp.path().join("zfb.config.json"), json)
            .await
            .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("should reject absolute path");
        let msg = format!("{err:#}");
        assert!(msg.contains("relative"), "msg: {msg}");
    }

    #[tokio::test]
    async fn rejects_parent_dir_escape() {
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "collections": [
                { "name": "blog", "path": "../outside" }
            ]
        }"#;
        tokio::fs::write(tmp.path().join("zfb.config.json"), json)
            .await
            .unwrap();
        let err = load_from_dir(tmp.path())
            .await
            .expect_err("should reject .. escape");
        let msg = format!("{err:#}");
        assert!(msg.contains("escapes"), "msg: {msg}");
    }

    #[tokio::test]
    async fn allows_internal_dotdot_that_does_not_escape() {
        // `a/../b` resolves to `b` — within the project root, so it should
        // be accepted. (Same shape as `./content/blog`.)
        let tmp = TempDir::new().unwrap();
        let json = r#"{
            "collections": [
                { "name": "blog", "path": "a/../b" },
                { "name": "docs", "path": "." }
            ]
        }"#;
        tokio::fs::write(tmp.path().join("zfb.config.json"), json)
            .await
            .unwrap();
        let cfg = load_from_dir(tmp.path()).await.expect("should accept");
        assert_eq!(cfg.collections.len(), 2);
    }

    #[tokio::test]
    async fn ts_config_loads_via_test_override() {
        // Drives the TS-loading code path WITHOUT running esbuild + node:
        // `LoadOptions::test_default_export_json` short-circuits the
        // subprocess and feeds the canned JSON into the same parse +
        // validate pipeline the production path uses.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.ts"),
            "// pretend TS — content irrelevant for this test\n\
             import { defineConfig } from \"zfb/config\";\n\
             export default defineConfig({ port: 4000 });\n",
        )
        .await
        .unwrap();
        let opts = LoadOptions {
            test_default_export_json: Some(
                r#"{"port": 4000, "framework": "react", "collections": [{"name":"blog","path":"content/blog"}]}"#
                    .to_string(),
            ),
            ..LoadOptions::default()
        };
        let cfg = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect("ts loader (mocked) should succeed");
        assert_eq!(cfg.port, Some(4000));
        assert_eq!(cfg.framework, Framework::React);
        assert_eq!(cfg.collections.len(), 1);
        assert_eq!(cfg.collections[0].name, "blog");
    }

    #[tokio::test]
    async fn ts_config_validates_through_same_rules_as_json() {
        // Validation runs after the TS load too — duplicate collection
        // names should still be rejected.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.ts"), "export default {};\n")
            .await
            .unwrap();
        let opts = LoadOptions {
            test_default_export_json: Some(
                r#"{"collections":[{"name":"blog","path":"content/blog"},{"name":"blog","path":"content/blog2"}]}"#
                    .to_string(),
            ),
            ..LoadOptions::default()
        };
        let err = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect_err("should reject duplicates from TS too");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("duplicate collection name"),
            "msg: {msg}"
        );
    }

    #[tokio::test]
    async fn json_wins_over_ts_for_back_compat() {
        // Both files present → JSON wins. The TS loader is not invoked,
        // so the test override (which would fail validation if used) is
        // not consulted.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.json"),
            r#"{"port": 5500}"#,
        )
        .await
        .unwrap();
        tokio::fs::write(
            tmp.path().join("zfb.config.ts"),
            "export default { port: 9999 };\n",
        )
        .await
        .unwrap();
        let opts = LoadOptions {
            // Deliberately bogus: if the TS path WERE taken we would
            // fail JSON parsing here.
            test_default_export_json: Some("not-json".into()),
            ..LoadOptions::default()
        };
        let cfg = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect("json wins, ts override is ignored");
        assert_eq!(cfg.port, Some(5500));
    }

    #[tokio::test]
    async fn ts_config_missing_node_emits_actionable_error() {
        // Force the node binary to a path that doesn't exist so the
        // spawn fails with NotFound, mirroring what the user sees when
        // node is not on PATH. We also point esbuild at a nonexistent
        // path so we never get past the binary-resolution step on
        // machines where esbuild happens to be installed — the test is
        // checking the shape of the error, not the full pipeline.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.ts"), "export default {};\n")
            .await
            .unwrap();
        // We need esbuild to succeed first; supply a fake one only if
        // the real default slot exists. Otherwise the esbuild lookup
        // bails first and this assertion would not exercise the node
        // path. Skip in that case — the message we want to test is
        // covered by the resolve_esbuild_binary check anyway.
        if !PathBuf::from(DEFAULT_ESBUILD_SLOT).exists()
            && std::env::var_os("ZFB_ESBUILD_BIN").is_none()
        {
            return;
        }
        let opts = LoadOptions {
            node_binary: Some(OsString::from("zfb-no-such-node-binary-xyz")),
            ..LoadOptions::default()
        };
        let err = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect_err("missing node should error cleanly");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("not found in PATH"),
            "msg should call out PATH: {msg}"
        );
        assert!(
            msg.contains("Node.js"),
            "msg should mention Node.js: {msg}"
        );
    }

    #[tokio::test]
    async fn ts_config_invalid_json_from_subprocess_includes_payload() {
        // When the subprocess emits something that doesn't deserialize,
        // the error includes the received payload to aid debugging.
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("zfb.config.ts"), "export default {};\n")
            .await
            .unwrap();
        let opts = LoadOptions {
            test_default_export_json: Some("not-json-at-all".into()),
            ..LoadOptions::default()
        };
        let err = load_from_dir_with_options(tmp.path(), &opts)
            .await
            .expect_err("garbage stdout must not be silently accepted");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("zfb.config.ts"),
            "msg should name the file: {msg}"
        );
        assert!(msg.contains("received"), "msg should echo the payload: {msg}");
    }

    /// Real subprocess flow: esbuild + node against the canonical
    /// `examples/basic-blog/zfb.config.future.ts` example. Gated behind
    /// `--include-ignored` because the staged esbuild slot is empty in
    /// CI today (see crates/zfb/binaries/esbuild/README.md) and the
    /// test will fail to find the binary. Run locally with
    /// `ZFB_ESBUILD_BIN=$(which esbuild) cargo test ts_real_subprocess
    /// --include-ignored -p zfb`.
    #[tokio::test]
    #[ignore = "requires real esbuild + node; opt in via --include-ignored"]
    async fn ts_real_subprocess_loads_basic_blog_future_ts() {
        // Locate the example file via CARGO_MANIFEST_DIR to be cwd-
        // independent.
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let example_ts = manifest_dir
            .join("../../examples/basic-blog/zfb.config.future.ts")
            .canonicalize()
            .expect("example future.ts must exist");

        // Copy the file into a fresh tmp project root so the loader
        // takes the TS branch (no sibling JSON).
        let tmp = TempDir::new().unwrap();
        let dst = tmp.path().join("zfb.config.ts");
        tokio::fs::copy(&example_ts, &dst).await.unwrap();

        let cfg = load_from_dir(tmp.path())
            .await
            .expect("real subprocess load should succeed");
        // Spot-check fields the example pins.
        assert_eq!(cfg.framework, Framework::Preact);
        assert_eq!(
            cfg.tailwind,
            Some(TailwindConfig { enabled: true })
        );
        assert_eq!(cfg.collections.len(), 1);
        assert_eq!(cfg.collections[0].name, "blog");
        assert_eq!(
            cfg.collections[0].path,
            PathBuf::from("content/blog")
        );
    }
}
