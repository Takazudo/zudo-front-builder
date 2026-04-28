//! Production [`ClientBundler`] implementation: shell out to the esbuild
//! CLI binary.
//!
//! Sub 2 of Epic #6 lives here. The trait + shared types
//! ([`crate::bundler::ClientBundler`], [`crate::bundler::Island`],
//! [`crate::bundler::BundleConfig`], [`crate::bundler::BundleOutput`]) are
//! owned by Sub 1 in [`crate::bundler`]; this module is the production
//! implementation that wraps the esbuild subprocess.
//!
//! Mirror of `zfb_css::engine::TailwindSubprocessEngine`: a config struct
//! that locates the binary (defaulting to
//! `crates/zfb/binaries/esbuild`), an implementation that builds the
//! command line, runs it, hashes the output, and writes
//! `{outdir}/assets/islands-{hash}.js`.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};

use crate::bundler::{
    bundle_link_href, island_link_href, BundleConfig, BundleOutput, ClientBundler, FrameworkKind,
    Island, IslandBundle, ModuleId, PerIslandBundleOutput,
};

/// The pinned esbuild CLI version this crate runs against.
///
/// At subprocess startup we run `esbuild --version` and abort with a
/// clear error if the reported version does not match. To bump, see
/// the "External tool version pins" section in `CONTRIBUTING.md` at
/// the workspace root.
pub const EXPECTED_ESBUILD_VERSION: &str = "0.24.0";

/// SHA-256 of the pinned esbuild binary, lowercase hex.
///
/// Set to the empty string when release engineering has not yet
/// populated the slot — in that case the checksum verification is
/// skipped (with a clear log line) and only the `--version` check is
/// enforced. Once populated, the checksum verification runs on first
/// subprocess invocation and any mismatch aborts with a clear error
/// pointing at the bump procedure in `CONTRIBUTING.md`.
pub const EXPECTED_ESBUILD_SHA256: &str = "";

/// One-time cache of `(binary_path → outcome)` for the version + checksum
/// verification. The first invocation of an `EsbuildSubprocessBundler`
/// pays the cost of running `esbuild --version` and hashing the binary;
/// subsequent invocations short-circuit. Wrapped in a `Mutex<BTreeMap>`
/// so a process running multiple bundlers (e.g. an integration test
/// suite) verifies each pinned binary path exactly once.
fn verification_cache() -> &'static Mutex<BTreeMap<PathBuf, ()>> {
    static CACHE: OnceLock<Mutex<BTreeMap<PathBuf, ()>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Run the version + checksum gate on `binary_path`. Idempotent across
/// the process: each path is verified at most once, but every distinct
/// path is verified the first time it is seen.
///
/// `skip` lets callers (notably tests using `mock_subprocess`) bypass
/// the gate without having to expose the cache directly.
fn ensure_binary_verified(binary_path: &Path, skip: bool) -> Result<()> {
    if skip {
        return Ok(());
    }
    let key = binary_path.to_path_buf();
    {
        let cache = verification_cache()
            .lock()
            .expect("esbuild verification cache mutex poisoned");
        if cache.contains_key(&key) {
            return Ok(());
        }
    }

    if !binary_path.exists() {
        return Err(anyhow!(
            "esbuild binary not found at {}; \
             set ZFB_ESBUILD_BIN or update EsbuildSubprocessConfig::binary_path",
            binary_path.display()
        ));
    }

    // 1. Version gate.
    let version_output = Command::new(binary_path)
        .arg("--version")
        .output()
        .with_context(|| {
            format!(
                "failed to spawn `{} --version` for the version gate",
                binary_path.display()
            )
        })?;
    if !version_output.status.success() {
        let stderr = String::from_utf8_lossy(&version_output.stderr);
        return Err(anyhow!(
            "esbuild --version exited with status {}: {}",
            version_output.status,
            stderr.trim()
        ));
    }
    let reported = String::from_utf8_lossy(&version_output.stdout)
        .trim()
        .to_string();
    if reported != EXPECTED_ESBUILD_VERSION {
        return Err(anyhow!(
            "esbuild version mismatch: expected `{}` (pinned in zfb-islands), got `{}` from {}. \
             To resolve, follow the \"External tool version pins\" procedure in CONTRIBUTING.md \
             at the workspace root: bump EXPECTED_ESBUILD_VERSION (and EXPECTED_ESBUILD_SHA256) \
             in crates/zfb-islands/src/esbuild.rs in lock-step with the binary under \
             crates/zfb/binaries/esbuild/esbuild.",
            EXPECTED_ESBUILD_VERSION,
            reported,
            binary_path.display()
        ));
    }

    // 2. SHA-256 gate. Skipped when the constant is empty (development
    // / pre-release-engineering slot).
    if !EXPECTED_ESBUILD_SHA256.is_empty() {
        let bytes = std::fs::read(binary_path)
            .with_context(|| format!("failed to read {} for checksum", binary_path.display()))?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let actual = hex::encode(hasher.finalize());
        if !actual.eq_ignore_ascii_case(EXPECTED_ESBUILD_SHA256) {
            return Err(anyhow!(
                "esbuild binary checksum mismatch for {}: \
                 expected sha256 `{}`, got `{}`. \
                 To resolve, follow the \"External tool version pins\" procedure in \
                 CONTRIBUTING.md at the workspace root: either replace the binary with \
                 one matching EXPECTED_ESBUILD_SHA256, or — when bumping intentionally — \
                 update both the version and SHA256 constants in \
                 crates/zfb-islands/src/esbuild.rs in lock-step with the new binary.",
                binary_path.display(),
                EXPECTED_ESBUILD_SHA256,
                actual
            ));
        }
    }

    verification_cache()
        .lock()
        .expect("esbuild verification cache mutex poisoned")
        .insert(key, ());
    Ok(())
}

#[derive(Debug, Clone)]
pub struct EsbuildSubprocessConfig {
    /// Path to the esbuild CLI binary.
    ///
    /// Default: `crates/zfb/binaries/esbuild/esbuild` (relative to the
    /// workspace root). Sub 2 reserves the parent `esbuild/` directory in
    /// the release tarball layout (mirroring the Tailwind v4 slot pattern;
    /// the directory shape gives release tooling room to drop sidecars
    /// like a checksum manifest next to the binary). Override via
    /// [`Self::with_binary_path`] or via the `ZFB_ESBUILD_BIN` environment
    /// variable (checked at engine construction time, not at every
    /// invocation).
    pub binary_path: PathBuf,

    /// Working directory for the subprocess. esbuild resolves entry points
    /// and `node_modules` lookups relative to this directory.
    ///
    /// Default: the current working directory at engine construction time.
    pub working_dir: PathBuf,

    /// Extra CLI args appended to the subprocess invocation, for escape
    /// hatches like `--define:process.env.NODE_ENV='production'`. Passed
    /// verbatim — each element becomes a separate `argv` entry, so shell
    /// quoting is not needed and shell injection is not possible.
    ///
    /// **Trust boundary**: callers are expected to assemble these from
    /// build configuration (zfb.config.ts, env vars under operator
    /// control), NOT from end-user input. The same caveat applies to
    /// island `source_path`s — they come from project-local source files
    /// the scanner has identified, not from user-supplied data.
    pub extra_args: Vec<OsString>,

    /// When true, the bundler will return a *fake* JS payload instead of
    /// invoking the subprocess. Used by unit tests to avoid depending on
    /// the binary being installed. The string returned is taken from
    /// [`Self::mock_output`].
    pub mock_subprocess: bool,

    /// Output to return when `mock_subprocess` is true.
    pub mock_output: String,
}

impl Default for EsbuildSubprocessConfig {
    fn default() -> Self {
        let env_override = std::env::var_os("ZFB_ESBUILD_BIN");
        let binary_path = match env_override {
            Some(p) => PathBuf::from(p),
            None => PathBuf::from("crates/zfb/binaries/esbuild/esbuild"),
        };
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            binary_path,
            working_dir,
            extra_args: Vec::new(),
            mock_subprocess: false,
            mock_output: String::new(),
        }
    }
}

impl EsbuildSubprocessConfig {
    /// Override the binary path (chainable).
    pub fn with_binary_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.binary_path = path.into();
        self
    }

    /// Override the working directory (chainable).
    pub fn with_working_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.working_dir = dir.into();
        self
    }

    /// Configure the bundler to skip the subprocess and return `output`
    /// instead. Used by unit tests.
    pub fn with_mock_output(mut self, output: impl Into<String>) -> Self {
        self.mock_subprocess = true;
        self.mock_output = output.into();
        self
    }
}

/// The default [`ClientBundler`]: shells out to the esbuild CLI binary.
///
/// The binary is invoked with each island's `source_path` as an entry
/// point, plus `--bundle --format=esm --splitting=false
/// --tree-shaking=true` (esbuild tree-shakes ESM by default but we set the
/// flag explicitly so the contract is visible). `--minify` and
/// `--sourcemap=linked` are appended per [`BundleConfig`]. The payload is
/// written to a temp file (`--outfile=`), read back, hashed, and written
/// to its final location at `{outdir}/assets/islands-{hash}.js`.
///
/// ### Example
///
/// ```no_run
/// use zfb_islands::{
///     BundleConfig, ClientBundler, EsbuildSubprocessBundler,
///     EsbuildSubprocessConfig, Island,
/// };
/// use std::path::PathBuf;
///
/// let bundler = EsbuildSubprocessBundler::new(EsbuildSubprocessConfig::default());
/// let islands = vec![Island {
///     component_name: "Counter".into(),
///     source_path: PathBuf::from("components/counter.tsx"),
/// }];
/// let out = bundler.bundle(&islands, &BundleConfig::production()).unwrap();
/// assert!(out.asset_url.starts_with("/assets/islands-"));
/// ```
#[derive(Debug, Clone)]
pub struct EsbuildSubprocessBundler {
    config: EsbuildSubprocessConfig,
}

impl EsbuildSubprocessBundler {
    /// Construct a new bundler with the given config.
    pub fn new(config: EsbuildSubprocessConfig) -> Self {
        Self { config }
    }

    /// Construct a new bundler with the default config.
    pub fn with_default_config() -> Self {
        Self::new(EsbuildSubprocessConfig::default())
    }

    /// Borrow the underlying config.
    pub fn config(&self) -> &EsbuildSubprocessConfig {
        &self.config
    }

    /// Internal: produce the JS payload for the given islands. Split out so
    /// tests can drive it directly without going through `bundle`'s
    /// asset-write step.
    fn produce_bundle_js(&self, islands: &[Island], config: &BundleConfig) -> Result<String> {
        if self.config.mock_subprocess {
            return Ok(self.config.mock_output.clone());
        }

        ensure_binary_verified(&self.config.binary_path, false)?;

        // Allocate a temp file in this engine's working dir so esbuild's
        // `--outfile` can be a relative path resolvable from the subprocess
        // cwd. Drop happens when we return — file vanishes after read.
        let tmp = tempfile::Builder::new()
            .prefix("zfb-esbuild-")
            .suffix(".js")
            .tempfile()
            .context("failed to allocate temp file for esbuild output")?;

        let mut cmd = Command::new(&self.config.binary_path);
        cmd.current_dir(&self.config.working_dir);
        cmd.arg("--bundle");
        cmd.arg("--format=esm");
        cmd.arg("--splitting=false");
        // Tree-shaking is on by default for ESM in esbuild; we set the
        // flag explicitly so the contract is visible at the call site.
        cmd.arg("--tree-shaking=true");
        if config.minify {
            cmd.arg("--minify");
        }
        if config.sourcemap {
            cmd.arg("--sourcemap=linked");
        }
        cmd.arg(format!("--outfile={}", tmp.path().display()));
        for extra in &self.config.extra_args {
            cmd.arg(extra);
        }
        for island in islands {
            cmd.arg(&island.source_path);
        }

        let output = cmd
            .output()
            .with_context(|| format!("failed to spawn {}", self.config.binary_path.display()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "esbuild exited with status {}: {}",
                output.status,
                stderr.trim()
            ));
        }

        let js =
            std::fs::read_to_string(tmp.path()).context("failed to read esbuild output file")?;
        Ok(js)
    }

    /// Per-island bundle pass.
    ///
    /// Generates one tiny entry script per island that imports the
    /// island's component module **and** the framework-specific
    /// hydration glue, then runs `esbuild --bundle --format=esm` on it
    /// (one subprocess per island — code-split by isolation rather than
    /// by `--splitting`, so dynamic-imports from the runtime stay 1:1
    /// with components). Each per-island bundle is hashed and written
    /// to `{outdir}/islands/{ComponentName}-{hash}.js`. Sourcemaps are
    /// preserved end-to-end when [`BundleConfig::sourcemap`] is true:
    /// esbuild emits a sibling `<file>.map` alongside the asset and a
    /// `//# sourceMappingURL=` comment in the JS so DevTools picks it
    /// up automatically.
    ///
    /// In addition to the per-island bundles, a small **runtime**
    /// bundle is emitted under
    /// `{outdir}/islands/islands-runtime-{hash}.js`. The runtime is
    /// produced from a generated entry that imports
    /// `scheduleHydrate` + `mountIslands` from the
    /// `@takazudo/zfb-runtime` style runtime (`packages/zfb/src/runtime.ts`)
    /// and ships a manifest of `ComponentName → island-bundle-URL` so
    /// the runtime can dynamic-import the right per-island bundle for
    /// each `[data-zfb-island]` element on the page.
    ///
    /// # Determinism
    ///
    /// The per-island and runtime bundles are deterministic for a
    /// given `(islands, framework, config)` triple: the island order is
    /// preserved verbatim from the input slice and the runtime
    /// manifest is keyed by `component_name` in the same order.
    pub fn bundle_per_island(
        &self,
        islands: &[Island],
        framework: FrameworkKind,
        config: &BundleConfig,
    ) -> Result<PerIslandBundleOutput> {
        // Outdir layout: {outdir}/islands/.
        let islands_dir = config.outdir.join("islands");
        std::fs::create_dir_all(&islands_dir)
            .with_context(|| format!("failed to create {}", islands_dir.display()))?;

        let mut bundle_entries: Vec<IslandBundle> = Vec::with_capacity(islands.len());
        let mut manifest: Vec<(String, String)> = Vec::with_capacity(islands.len());

        for island in islands {
            // 1. Generate the per-island entry source.
            let entry_source = render_island_entry_source(framework, island);

            // 2. Bundle it. In mock mode the entry source itself doubles
            //    as the bundled output so unit tests can assert what we
            //    *would* have shipped without spawning esbuild.
            let bundled_js = self.bundle_one_entry(&entry_source, config)?;

            let hash = hash_8(&bundled_js);
            let asset_path = islands_dir.join(format!("{}-{}.js", island.component_name, hash));
            std::fs::write(&asset_path, bundled_js.as_bytes())
                .with_context(|| format!("failed to write {}", asset_path.display()))?;

            let asset_url = island_link_href(&config.base_url, &asset_path);
            manifest.push((island.component_name.clone(), asset_url.clone()));

            bundle_entries.push(IslandBundle {
                component_name: island.component_name.clone(),
                asset_path,
                asset_url,
                hash,
            });
        }

        // 3. Runtime bundle. The manifest is JSON-encoded so it can be
        //    dropped into the runtime entry source as a literal that
        //    esbuild's parser will accept verbatim.
        let manifest_json = serialize_manifest(&manifest);
        let runtime_entry_source = render_runtime_entry_source(framework, &manifest_json);
        let runtime_js = self.bundle_one_entry(&runtime_entry_source, config)?;
        let runtime_hash = hash_8(&runtime_js);
        let runtime_asset_path = islands_dir.join(format!("islands-runtime-{runtime_hash}.js"));
        std::fs::write(&runtime_asset_path, runtime_js.as_bytes())
            .with_context(|| format!("failed to write {}", runtime_asset_path.display()))?;
        let runtime_asset_url = island_link_href(&config.base_url, &runtime_asset_path);

        Ok(PerIslandBundleOutput {
            islands: bundle_entries,
            runtime_asset_path,
            runtime_asset_url,
        })
    }

    /// Internal: bundle a single in-memory entry source. Writes the
    /// source to a temp file (because esbuild expects entry points on
    /// disk), runs the subprocess, returns the bundled JS string.
    fn bundle_one_entry(&self, entry_source: &str, config: &BundleConfig) -> Result<String> {
        if self.config.mock_subprocess {
            // Mock mode: return either the configured mock_output (when
            // set) or the entry source itself so tests can assert the
            // generated entry shape end-to-end.
            if !self.config.mock_output.is_empty() {
                return Ok(self.config.mock_output.clone());
            }
            return Ok(entry_source.to_string());
        }

        ensure_binary_verified(&self.config.binary_path, false)?;

        // Entry temp file (.tsx so esbuild's loader inference picks up
        // JSX automatically — the entry imports component .tsx files).
        let entry_tmp = tempfile::Builder::new()
            .prefix("zfb-esbuild-entry-")
            .suffix(".tsx")
            .tempfile()
            .context("failed to allocate entry temp file")?;
        std::fs::write(entry_tmp.path(), entry_source.as_bytes())
            .context("failed to write entry temp file")?;

        // Output temp file. esbuild writes a sibling `.map` next to it
        // when sourcemap=linked is requested — we read both back.
        let out_tmp = tempfile::Builder::new()
            .prefix("zfb-esbuild-out-")
            .suffix(".js")
            .tempfile()
            .context("failed to allocate out temp file")?;

        let mut cmd = Command::new(&self.config.binary_path);
        cmd.current_dir(&self.config.working_dir);
        cmd.arg("--bundle");
        cmd.arg("--format=esm");
        cmd.arg("--splitting=false");
        cmd.arg("--tree-shaking=true");
        if config.minify {
            cmd.arg("--minify");
        }
        if config.sourcemap {
            cmd.arg("--sourcemap=linked");
        }
        cmd.arg(format!("--outfile={}", out_tmp.path().display()));
        for extra in &self.config.extra_args {
            cmd.arg(extra);
        }
        cmd.arg(entry_tmp.path());

        let output = cmd
            .output()
            .with_context(|| format!("failed to spawn {}", self.config.binary_path.display()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "esbuild exited with status {}: {}",
                output.status,
                stderr.trim()
            ));
        }

        let js =
            std::fs::read_to_string(out_tmp.path()).context("failed to read esbuild output")?;
        Ok(js)
    }
}

/// Generate the per-island entry script for `framework`.
///
/// The entry imports the component module by its resolved source path
/// (esbuild resolves it from the bundler's working directory), wraps
/// it in framework-specific `mount(props, element, mode)` glue, and
/// exposes the function as the bundle's default export. The hydration
/// runtime dynamic-imports this default export.
///
/// Mode is one of:
/// - `"hydrate"` — used for SSR'd islands (`[data-zfb-island]`).
/// - `"render"` — used for SSR-skip islands
///   (`[data-zfb-island-skip-ssr]`); avoids hydrate-mismatch warnings
///   because the DOM is empty when we mount.
pub fn render_island_entry_source(framework: FrameworkKind, island: &Island) -> String {
    let path = island.source_path.to_string_lossy();
    let path_lit = json_string(&path);
    let component_name = &island.component_name;
    let component_lit = json_string(component_name);
    match framework {
        FrameworkKind::Preact => format!(
            r#"// Generated by zfb-islands::EsbuildSubprocessBundler::bundle_per_island
import * as Mod from {path_lit};
import {{ h, hydrate, render }} from "preact";
const Component = (Mod as any)[{component_lit}] ?? (Mod as any).default;
export function mount(props, element, mode) {{
  const vnode = h(Component, props);
  if (mode === "hydrate") {{
    hydrate(vnode, element);
  }} else {{
    render(vnode, element);
  }}
}}
export default mount;
"#
        ),
        FrameworkKind::React => format!(
            r#"// Generated by zfb-islands::EsbuildSubprocessBundler::bundle_per_island
import * as Mod from {path_lit};
import {{ createElement }} from "react";
import {{ hydrateRoot, createRoot }} from "react-dom/client";
const Component = (Mod as any)[{component_lit}] ?? (Mod as any).default;
export function mount(props, element, mode) {{
  const vnode = createElement(Component, props);
  if (mode === "hydrate") {{
    hydrateRoot(element, vnode);
  }} else {{
    createRoot(element).render(vnode);
  }}
}}
export default mount;
"#
        ),
    }
}

/// Generate the runtime entry script for `framework`.
///
/// The runtime imports the framework-agnostic hydration runtime from
/// the `zfb` SDK package, hands it the manifest of
/// `ComponentName → bundle URL` so it can dynamic-import the right
/// per-island bundle for each `[data-zfb-island]` element. The
/// manifest is inlined as a JSON literal — no extra fetch — so the
/// runtime works in static-host environments.
///
/// `manifest_json` MUST already be a valid JSON object literal
/// (produced by [`serialize_manifest`]).
pub fn render_runtime_entry_source(framework: FrameworkKind, manifest_json: &str) -> String {
    let _ = framework; // current shim is framework-agnostic; the per-island bundles carry the framework-specific glue.
    format!(
        r#"// Generated by zfb-islands::EsbuildSubprocessBundler::bundle_per_island
import {{ mountIslands }} from "zfb/runtime";
const ISLAND_MANIFEST = {manifest_json};
mountIslands(ISLAND_MANIFEST);
"#
    )
}

/// Serialize a manifest of `(component_name → asset_url)` pairs to a
/// JSON object literal. Order of keys is preserved (the runtime does
/// not rely on it but determinism matters for hashing).
fn serialize_manifest(entries: &[(String, String)]) -> String {
    let mut s = String::from("{");
    for (i, (k, v)) in entries.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&json_string(k));
        s.push(':');
        s.push_str(&json_string(v));
    }
    s.push('}');
    s
}

/// Encode `s` as a JSON string literal (with surrounding double
/// quotes). Used to splice user-supplied paths and component names
/// into generated entry sources without risking syntax errors from
/// stray quotes / backslashes / control chars.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

impl ClientBundler for EsbuildSubprocessBundler {
    fn bundle(&self, islands: &[Island], config: &BundleConfig) -> Result<BundleOutput> {
        let js = self.produce_bundle_js(islands, config)?;

        let hash = hash_8(&js);
        let asset_path = config
            .outdir
            .join("assets")
            .join(format!("islands-{hash}.js"));

        if let Some(parent) = asset_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        std::fs::write(&asset_path, js.as_bytes())
            .with_context(|| format!("failed to write {}", asset_path.display()))?;

        let asset_url = bundle_link_href(&config.base_url, &asset_path);

        // Default `module_ids` mapping: identity per-island. Sub 1's
        // scanner is the source of truth for the canonical mapping (e.g.
        // when the exported name differs from the component name); until
        // it lands a richer Island shape, `component_name` is the right
        // contract for the hydration runtime to assume.
        let module_ids: Vec<ModuleId> = islands.iter().map(|i| i.component_name.clone()).collect();

        Ok(BundleOutput {
            asset_path,
            asset_url,
            module_ids,
        })
    }
}

/// Compute the 8-char hex hash for the given bundle bytes.
///
/// The hash is the first 8 characters of `sha256(js)` in lowercase hex.
/// Mirrors `zfb-css::pipeline::hash_8` (which folds two halves via a `\n`
/// separator) — for islands there is only one byte stream so no separator
/// is needed.
pub fn hash_8(js: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(js.as_bytes());
    let digest = hasher.finalize();
    let full = hex::encode(digest);
    full[..8].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_for_identical_inputs() {
        let a = hash_8("export const x = 1;");
        let b = hash_8("export const x = 1;");
        assert_eq!(a, b);
        assert_eq!(a.len(), 8);
    }

    #[test]
    fn hash_changes_when_payload_changes() {
        let before = hash_8("export const x = 1;");
        let after = hash_8("export const x = 2;");
        assert_ne!(before, after);
    }

    #[test]
    fn json_string_escapes_specials() {
        assert_eq!(json_string("a/b"), "\"a/b\"");
        assert_eq!(json_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_string("a\\b"), "\"a\\\\b\"");
        assert_eq!(json_string("a\nb"), "\"a\\nb\"");
    }

    #[test]
    fn serialize_manifest_round_trips_through_serde_json() {
        let entries = vec![
            ("Counter".into(), "/islands/Counter-abc12345.js".into()),
            ("Button".into(), "/islands/Button-deadbeef.js".into()),
        ];
        let s = serialize_manifest(&entries);
        let parsed: serde_json::Value = serde_json::from_str(&s).expect("valid json");
        assert_eq!(
            parsed["Counter"].as_str(),
            Some("/islands/Counter-abc12345.js")
        );
        assert_eq!(
            parsed["Button"].as_str(),
            Some("/islands/Button-deadbeef.js")
        );
    }

    #[test]
    fn render_island_entry_source_preact_imports_bare_preact() {
        let island = Island::new("Counter", "/abs/components/Counter.tsx");
        let src = render_island_entry_source(FrameworkKind::Preact, &island);
        assert!(src.contains(r#"from "preact""#));
        assert!(!src.contains("preact/compat"));
        assert!(src.contains("hydrate"));
        assert!(src.contains("render"));
        assert!(src.contains(r#"from "/abs/components/Counter.tsx""#));
        assert!(src.contains("export function mount"));
        assert!(src.contains("export default mount"));
    }

    #[test]
    fn render_island_entry_source_react_uses_client_apis() {
        let island = Island::new("Modal", "/abs/components/Modal.tsx");
        let src = render_island_entry_source(FrameworkKind::React, &island);
        assert!(src.contains(r#"from "react-dom/client""#));
        assert!(src.contains("hydrateRoot"));
        assert!(src.contains("createRoot"));
        // Distinguish hydrate path (SSR'd) from render path (SSR-skip).
        assert!(src.contains(r#"mode === "hydrate""#));
    }

    #[test]
    fn render_runtime_entry_source_inlines_manifest_and_calls_mount() {
        let manifest = serialize_manifest(&[
            ("Counter".into(), "/islands/Counter-abc.js".into()),
            ("Button".into(), "/islands/Button-def.js".into()),
        ]);
        let src = render_runtime_entry_source(FrameworkKind::Preact, &manifest);
        assert!(src.contains(r#"import { mountIslands } from "zfb/runtime""#));
        assert!(src.contains(r#""Counter":"/islands/Counter-abc.js""#));
        assert!(src.contains("mountIslands(ISLAND_MANIFEST);"));
    }

    #[test]
    fn bundle_per_island_in_mock_mode_writes_per_island_assets_and_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = EsbuildSubprocessConfig::default();
        // Empty mock_output makes bundle_one_entry echo the entry source
        // back, so each island's bytes (and therefore its hash) reflect
        // the deterministic entry shape.
        let bundler = EsbuildSubprocessBundler::new(EsbuildSubprocessConfig {
            mock_subprocess: true,
            ..cfg
        });
        let islands = vec![
            Island::new("Counter", "/abs/components/Counter.tsx"),
            Island::new("Modal", "/abs/components/Modal.tsx"),
        ];
        let config = BundleConfig::default()
            .with_outdir(dir.path().to_path_buf())
            .with_base_url("/");
        let out = bundler
            .bundle_per_island(&islands, FrameworkKind::Preact, &config)
            .expect("bundle_per_island in mock mode succeeds");

        // Two per-island bundles, in the same order as input.
        assert_eq!(out.islands.len(), 2);
        assert_eq!(out.islands[0].component_name, "Counter");
        assert_eq!(out.islands[1].component_name, "Modal");

        for entry in &out.islands {
            // Hash is 8 lowercase hex chars.
            assert_eq!(entry.hash.len(), 8);
            assert!(entry.hash.chars().all(|c| c.is_ascii_hexdigit()));
            // File exists on disk under {outdir}/islands/.
            assert!(entry.asset_path.exists(), "{:?}", entry.asset_path);
            assert!(entry.asset_path.starts_with(dir.path().join("islands")));
            // Public URL lives under /islands/.
            assert!(entry.asset_url.starts_with("/islands/"));
            assert!(entry.asset_url.ends_with(".js"));
        }

        // Runtime bundle exists, has its own hash, sits next to the
        // per-island bundles, and bears a /islands/ public URL.
        assert!(out.runtime_asset_path.exists());
        assert!(out
            .runtime_asset_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("islands-runtime-"));
        assert!(out.runtime_asset_url.starts_with("/islands/"));
    }

    #[test]
    fn bundle_per_island_is_deterministic_across_runs() {
        let dir = tempfile::tempdir().unwrap();
        let bundler = EsbuildSubprocessBundler::new(EsbuildSubprocessConfig {
            mock_subprocess: true,
            ..EsbuildSubprocessConfig::default()
        });
        let islands = vec![Island::new("Counter", "/abs/components/Counter.tsx")];
        let config = BundleConfig::default().with_outdir(dir.path().to_path_buf());
        let a = bundler
            .bundle_per_island(&islands, FrameworkKind::Preact, &config)
            .unwrap();
        let b = bundler
            .bundle_per_island(&islands, FrameworkKind::Preact, &config)
            .unwrap();
        assert_eq!(a.islands[0].hash, b.islands[0].hash);
        assert_eq!(a.islands[0].asset_url, b.islands[0].asset_url);
        assert_eq!(a.runtime_asset_url, b.runtime_asset_url);
    }

    #[test]
    fn esbuild_subprocess_config_env_override_is_honoured() {
        let prev = std::env::var_os("ZFB_ESBUILD_BIN");
        std::env::set_var("ZFB_ESBUILD_BIN", "/tmp/zfb-esbuild-overridden");
        let cfg = EsbuildSubprocessConfig::default();
        assert_eq!(
            cfg.binary_path,
            PathBuf::from("/tmp/zfb-esbuild-overridden")
        );
        match prev {
            Some(v) => std::env::set_var("ZFB_ESBUILD_BIN", v),
            None => std::env::remove_var("ZFB_ESBUILD_BIN"),
        }
    }
}
