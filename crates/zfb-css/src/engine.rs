//! The "CSS engine" abstraction.
//!
//! Tailwind v4 is delivered as a Go-built standalone CLI. To keep our build
//! pipeline portable we shell out to that binary today — but the long-term
//! plan is to swap in a Rust-native implementation (e.g. a future port of
//! Tailwind's class-engine, or our own equivalent) without touching the rest
//! of the pipeline.
//!
//! That swap is exactly what [`CssEngine`] models: take a set of source
//! files to scan for utility classes, return a string of generated CSS.
//! Everything stage-2 and beyond (CSS Modules, hashing, asset emission)
//! is engine-agnostic.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};

/// Abstraction over "produce a single CSS string of utility classes for the
/// given project sources".
///
/// ## Why a trait?
///
/// We currently shell out to the official `tailwindcss` v4 CLI binary
/// ([`TailwindSubprocessEngine`]). When a Rust-native option becomes viable
/// (see [`crate::native_engine::NativeRustEngine`]), we want to swap it in
/// at one site — the [`crate::CssPipeline`] constructor — without rewriting
/// CSS Modules, hashing, or asset emission.
///
/// ## Contract
///
/// - The engine MUST return CSS text suitable for direct concatenation into
///   the global stylesheet. No HTML, no `<style>` tags, no source maps
///   embedded in the string itself.
/// - The engine MAY consult its own configuration (binary path, content
///   globs, theme tokens, etc.). The `sources` parameter is a *hint* of
///   files known to the caller; engines are free to widen the set if their
///   own config requires it (Tailwind v4 has its own `@source` directive).
/// - The engine MUST be deterministic for a given (sources, config) pair —
///   the hashing stage assumes byte-stable output.
pub trait CssEngine {
    /// Produce utility-class CSS for the given source files.
    fn produce_utility_css(&self, sources: &[PathBuf]) -> Result<String>;
}

/// Configuration for [`TailwindSubprocessEngine`].
#[derive(Debug, Clone)]
pub struct TailwindSubprocessConfig {
    /// Path to the `tailwindcss` v4 CLI binary.
    ///
    /// Default: `crates/zfb/binaries/tailwindcss-v4` (relative to the
    /// workspace root). Topic B of Epic 4 reserves this slot in the release
    /// tarball layout. Override via [`Self::with_binary_path`] or via the
    /// `ZFB_TAILWIND_BIN` environment variable (checked at engine
    /// construction time, not at every invocation).
    pub binary_path: PathBuf,

    /// Working directory for the subprocess. The Tailwind v4 CLI resolves
    /// `@source "..."` directives relative to this directory.
    ///
    /// Default: the current working directory at engine construction time.
    pub working_dir: PathBuf,

    /// Optional explicit input CSS file (`-i` flag). When `None`, the
    /// engine will synthesise a minimal entry CSS (see
    /// [`build_synthesised_entry_css`]) covering Tailwind import +
    /// content `@source` directives + optional `@theme` block. Most
    /// projects will set this to point at the user's `styles/global.css`
    /// — the engine will then *prepend* the synthesised `@source` lines
    /// to that file's contents inside a temp file before passing it on.
    pub input_css: Option<PathBuf>,

    /// Extra CLI args appended to the subprocess invocation, for escape
    /// hatches like `--minify`. These are passed verbatim.
    pub extra_args: Vec<OsString>,

    /// Content globs (Tailwind v4 `@source` targets) for the **user
    /// project**. Each entry becomes one `@source "<glob>";` directive
    /// in the synthesised entry CSS.
    ///
    /// Globs are written verbatim, so callers can pass either glob
    /// patterns (`"pages/**/*.tsx"`) or directory shorthands
    /// (`"./components"`) — Tailwind v4 accepts both.
    ///
    /// Defaults to [`DEFAULT_CONTENT_ROOTS`] resolved against
    /// [`Self::working_dir`].
    pub content_globs: Vec<String>,

    /// Content globs for **framework packages** that must NOT be
    /// tree-shaken even though they live outside the user project (e.g.
    /// `packages/zudo-doc-v2/**` after the Phase B split-out). Each
    /// entry becomes a separate `@source` directive — listed after the
    /// user-project globs so per-project overrides win in cascade
    /// order.
    pub framework_package_globs: Vec<String>,

    /// Optional inline `@theme { ... }` block to append to the
    /// synthesised entry CSS. Used by callers that ship a
    /// programmatically-built design-token block (e.g. derived from
    /// `zfb.config.ts`).
    ///
    /// If both `input_css` (which may itself contain `@theme`) and
    /// `theme_block` are set, both end up in the final entry CSS —
    /// later wins per CSS cascade.
    pub theme_block: Option<String>,

    /// When true, the engine will return a *fake* CSS string instead of
    /// invoking the subprocess. Used by unit tests to avoid depending on
    /// the binary being installed.
    ///
    /// The string returned is taken from [`Self::mock_output`]. When the
    /// mock path is taken, the synthesised entry CSS is still computed
    /// and recorded on [`TailwindSubprocessEngine::last_entry_css`] so
    /// tests can assert on it.
    pub mock_subprocess: bool,

    /// Output to return when `mock_subprocess` is true.
    pub mock_output: String,

    /// Sub #212 — keep the [`tempfile::TempDir`] backing an
    /// embedded-binary extraction alive for the lifetime of the engine.
    ///
    /// Populated by [`Self::with_embedded_binary`]. Wrapped in [`Arc`] so
    /// that the surrounding `Clone` impl on `TailwindSubprocessConfig`
    /// keeps working — `tempfile::TempDir` is itself not `Clone`. The
    /// field is non-public; callers don't see it, they just receive a
    /// `binary_path` that points inside the live tempdir.
    _embedded_handle: Option<Arc<tempfile::TempDir>>,
}

impl Default for TailwindSubprocessConfig {
    fn default() -> Self {
        // Resolve binary path with the precedence:
        //   1. `ZFB_TAILWIND_BIN` env override (full bypass for ops).
        //   2. embedded extraction (sub #212) — wired in by the caller
        //      via [`Self::with_embedded_binary`]. We keep `default()`
        //      infallible by not touching the embedded snapshot here;
        //      the binary lives inside the `zfb` crate's
        //      `EMBEDDED_VENDOR` and `zfb-css` would otherwise need a
        //      reverse dependency to reach it. The `zfb` crate is the
        //      sole caller that constructs this config and is the
        //      natural place to do the (potentially fallible) extract.
        //   3. workspace-relative fallback for in-workspace dev.
        let env_override = std::env::var_os("ZFB_TAILWIND_BIN");
        let binary_path = match env_override {
            Some(p) => PathBuf::from(p),
            None => PathBuf::from("crates/zfb/binaries/tailwindcss-v4"),
        };
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            binary_path,
            working_dir,
            input_css: None,
            extra_args: Vec::new(),
            content_globs: Vec::new(),
            framework_package_globs: Vec::new(),
            theme_block: None,
            mock_subprocess: false,
            mock_output: String::new(),
            _embedded_handle: None,
        }
    }
}

impl TailwindSubprocessConfig {
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

    /// Override the input CSS entrypoint (chainable).
    pub fn with_input_css(mut self, path: impl Into<PathBuf>) -> Self {
        self.input_css = Some(path.into());
        self
    }

    /// Configure the engine to skip the subprocess and return `output`
    /// instead. Used by unit tests.
    pub fn with_mock_output(mut self, output: impl Into<String>) -> Self {
        self.mock_subprocess = true;
        self.mock_output = output.into();
        self
    }

    /// Replace the user-project content globs (chainable).
    pub fn with_content_globs<I, S>(mut self, globs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.content_globs = globs.into_iter().map(Into::into).collect();
        self
    }

    /// Replace the framework-package content globs (chainable).
    pub fn with_framework_package_globs<I, S>(mut self, globs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.framework_package_globs = globs.into_iter().map(Into::into).collect();
        self
    }

    /// Set an inline `@theme { ... }` block (chainable).
    pub fn with_theme_block(mut self, block: impl Into<String>) -> Self {
        self.theme_block = Some(block.into());
        self
    }

    /// Sub #212 — install an embedded-binary extraction as the resolved
    /// [`Self::binary_path`] and keep the backing [`tempfile::TempDir`]
    /// alive for the lifetime of `self`.
    ///
    /// This is the wiring point for the precedence step "embedded
    /// extraction" between the env override and the workspace-relative
    /// fallback. The caller is the zfb crate (which has access to the
    /// `EMBEDDED_VENDOR` snapshot via
    /// `crates/zfb/src/render_pipeline.rs::embedded_binary`); zfb-css
    /// itself stays free of any reverse dependency on the embedded
    /// vendor tree.
    ///
    /// Precedence is preserved: if `ZFB_TAILWIND_BIN` is set in the
    /// process environment, this method is a no-op so the env override
    /// keeps winning. Otherwise the embedded `path` replaces the
    /// workspace-relative fallback that [`Self::default`] selected.
    ///
    /// `handle` is wrapped in [`Arc`] so the surrounding
    /// `#[derive(Clone)]` keeps working — `tempfile::TempDir` is not
    /// itself `Clone`.
    pub fn with_embedded_binary(
        mut self,
        handle: tempfile::TempDir,
        path: PathBuf,
    ) -> Self {
        if std::env::var_os("ZFB_TAILWIND_BIN").is_some() {
            // Env tier already won — drop the handle on the floor and
            // leave `binary_path` pointing at the env value.
            drop(handle);
            return self;
        }
        self.binary_path = path;
        self._embedded_handle = Some(Arc::new(handle));
        self
    }
}

/// Build the synthesised entry CSS that the engine hands to Tailwind v4.
///
/// The output, in order, is:
///
/// 1. `@import "tailwindcss";` — required for v4 utility generation.
/// 2. `@source "<glob>";` directives for every entry in
///    `content_globs` (the user project). Globs are emitted **before**
///    framework globs so user-project overrides win in cascade order.
/// 3. `@source "<glob>";` directives for every entry in
///    `framework_package_globs` (e.g. `packages/zudo-doc-v2/**`).
/// 4. The contents of `input_css` if provided (the user's
///    `styles/global.css`, typically including their own `@theme {…}`
///    and authored CSS rules). When the file already starts with
///    `@import "tailwindcss";`, the synthesiser drops the duplicate
///    import we added in step 1 — Tailwind v4 errors on a doubled
///    import.
/// 5. The inline `theme_block`, if any.
///
/// The returned `String` is what the engine writes to a temp file and
/// passes to `tailwindcss -i <tmp>`. It is also stashed on
/// [`TailwindSubprocessEngine::last_entry_css`] so tests can inspect it
/// without spawning the binary.
pub fn build_synthesised_entry_css(
    cfg: &TailwindSubprocessConfig,
    input_css_text: Option<&str>,
) -> String {
    let mut out = String::new();
    let mut emitted_import = false;

    // Detect a leading `@import "tailwindcss";` (full bundle) *or* any
    // split-import sub-path (`@import "tailwindcss/preflight"`, etc.) in
    // the user CSS so we don't prepend the full bundle on top and leak the
    // default palette tokens. The split-import pattern is the deliberate
    // way users opt out of the full default theme.
    let user_has_import = input_css_text
        .map(|t| t.lines().any(|l| {
            let t = l.trim();
            t.starts_with("@import \"tailwindcss\"") || t.starts_with("@import 'tailwindcss'")
                || t.starts_with("@import \"tailwindcss/") || t.starts_with("@import 'tailwindcss/")
        }))
        .unwrap_or(false);

    if !user_has_import {
        out.push_str("@import \"tailwindcss\";\n");
        emitted_import = true;
    }

    // User-project content globs first.
    for g in &cfg.content_globs {
        out.push_str(&format!("@source \"{g}\";\n"));
    }
    // Then framework packages.
    for g in &cfg.framework_package_globs {
        out.push_str(&format!("@source \"{g}\";\n"));
    }

    if emitted_import || !cfg.content_globs.is_empty() || !cfg.framework_package_globs.is_empty() {
        out.push('\n');
    }

    if let Some(text) = input_css_text {
        out.push_str(text);
        if !text.ends_with('\n') {
            out.push('\n');
        }
    }

    if let Some(theme) = &cfg.theme_block {
        out.push('\n');
        out.push_str(theme);
        if !theme.ends_with('\n') {
            out.push('\n');
        }
    }

    out
}

/// The default [`CssEngine`]: shells out to the `tailwindcss` v4 CLI binary.
///
/// The binary is invoked with an output flag (`-o`) pointing at a temp file;
/// the file is then read back and returned as a `String`. The temp file is
/// cleaned up when its [`tempfile::NamedTempFile`] handle drops (i.e. when
/// the call returns).
///
/// ### Example
///
/// ```no_run
/// use zfb_css::{CssEngine, TailwindSubprocessConfig, TailwindSubprocessEngine};
/// use std::path::PathBuf;
///
/// let engine = TailwindSubprocessEngine::new(TailwindSubprocessConfig::default());
/// let css = engine.produce_utility_css(&[PathBuf::from("pages/index.tsx")]).unwrap();
/// assert!(!css.is_empty());
/// ```
#[derive(Debug)]
pub struct TailwindSubprocessEngine {
    config: TailwindSubprocessConfig,
    /// Last synthesised entry CSS — populated on every call to
    /// [`Self::produce_utility_css`] (including when the mock path is
    /// taken). Tests assert on this without needing the binary.
    last_entry_css: std::sync::Mutex<Option<String>>,
}

impl Clone for TailwindSubprocessEngine {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            last_entry_css: std::sync::Mutex::new(
                self.last_entry_css
                    .lock()
                    .ok()
                    .and_then(|g| g.clone()),
            ),
        }
    }
}

impl TailwindSubprocessEngine {
    /// Construct a new engine with the given config.
    pub fn new(config: TailwindSubprocessConfig) -> Self {
        Self {
            config,
            last_entry_css: std::sync::Mutex::new(None),
        }
    }

    /// Construct a new engine with the default config.
    pub fn with_default_config() -> Self {
        Self::new(TailwindSubprocessConfig::default())
    }

    /// Borrow the underlying config.
    pub fn config(&self) -> &TailwindSubprocessConfig {
        &self.config
    }

    /// Most recent synthesised entry CSS — i.e. the file contents passed
    /// to `tailwindcss -i <tmp>` on the last
    /// [`Self::produce_utility_css`] call. Returns `None` if the engine
    /// has not been invoked yet.
    pub fn last_entry_css(&self) -> Option<String> {
        self.last_entry_css.lock().ok().and_then(|g| g.clone())
    }
}

impl CssEngine for TailwindSubprocessEngine {
    fn produce_utility_css(&self, sources: &[PathBuf]) -> Result<String> {
        // Build the synthesised entry CSS. Read user input_css if set —
        // failure to read is fatal because the user explicitly asked for
        // it.
        let user_text = match &self.config.input_css {
            Some(p) => Some(std::fs::read_to_string(p).with_context(|| {
                format!(
                    "failed to read user input CSS at {}",
                    p.display()
                )
            })?),
            None => None,
        };
        let entry_css = build_synthesised_entry_css(&self.config, user_text.as_deref());

        // Stash for test introspection. Lock-poison is non-fatal — at
        // worst we lose the snapshot.
        if let Ok(mut slot) = self.last_entry_css.lock() {
            *slot = Some(entry_css.clone());
        }

        if self.config.mock_subprocess {
            return Ok(self.config.mock_output.clone());
        }

        // Sanity-check: the binary should exist before we try to spawn it.
        // This gives a much clearer error message than the OS-level "no
        // such file or directory" from `Command::spawn`.
        if !self.config.binary_path.exists() {
            return Err(anyhow!(
                "tailwindcss v4 binary not found at {}; \
                 set ZFB_TAILWIND_BIN or update TailwindSubprocessConfig::binary_path",
                self.config.binary_path.display()
            ));
        }

        // Materialise the synthesised entry CSS into a temp file so
        // Tailwind's `@source` resolution uses our wrapper (and the user
        // file's relative imports still resolve, since the temp file is
        // adjacent to the working_dir, not to the user file).
        let mut entry_tmp = tempfile::Builder::new()
            .prefix("zfb-tailwind-entry-")
            .suffix(".css")
            .tempfile_in(&self.config.working_dir)
            .context("failed to allocate temp file for tailwind entry CSS")?;
        {
            use std::io::Write;
            entry_tmp
                .as_file_mut()
                .write_all(entry_css.as_bytes())
                .context("failed to write tailwind entry CSS")?;
            entry_tmp
                .as_file_mut()
                .flush()
                .context("failed to flush tailwind entry CSS")?;
        }

        let out_tmp = tempfile::Builder::new()
            .prefix("zfb-tailwind-out-")
            .suffix(".css")
            .tempfile()
            .context("failed to allocate temp file for tailwind output")?;

        let mut cmd = Command::new(&self.config.binary_path);
        cmd.current_dir(&self.config.working_dir);
        cmd.arg("-i").arg(entry_tmp.path());
        cmd.arg("-o").arg(out_tmp.path());
        for extra in &self.config.extra_args {
            cmd.arg(extra);
        }

        // Annotate sources purely for diagnostics. Tailwind v4 discovers
        // sources via its own config / `@source` directives — but we record
        // the caller's hint in an env var so a wrapping script can see it.
        let sources_joined: Vec<String> = sources
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        cmd.env("ZFB_TAILWIND_SOURCES", sources_joined.join("\n"));

        // Capture both stdout and stderr; relay stderr through a clean
        // error message rather than letting it scroll into the parent
        // terminal (zfb-build log will pick this up via the returned
        // Err's chain).
        let output = cmd
            .output()
            .with_context(|| format!("failed to spawn {}", self.config.binary_path.display()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(anyhow!(
                "tailwindcss exited with status {}\n--- stderr ---\n{}\n--- stdout ---\n{}",
                output.status,
                stderr.trim(),
                stdout.trim()
            ));
        }

        let css = std::fs::read_to_string(out_tmp.path())
            .context("failed to read tailwind output file")?;
        Ok(css)
    }
}

/// Default content roots that the project scans for utility classes.
///
/// `zfb-css` does not enforce a specific scanning strategy — Tailwind v4 has
/// its own — but exposes this constant so callers (and the tailwind config
/// generator) agree on a single list.
pub const DEFAULT_CONTENT_ROOTS: &[&str] = &["pages", "components", "layouts", "content"];

/// Build a string `@source` declaration list from the project root, suitable
/// for emitting into a Tailwind v4 entrypoint CSS file.
pub fn default_source_directives(project_root: &Path) -> String {
    let mut out = String::new();
    for root in DEFAULT_CONTENT_ROOTS {
        let full = project_root.join(root);
        out.push_str(&format!("@source \"{}\";\n", full.display()));
    }
    out
}

/// Convenience: a small map describing the engine's identity, used when we
/// want to record provenance in pipeline outputs (e.g. log lines).
pub fn engine_provenance(name: &str, version: Option<&str>) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("engine".to_string(), name.to_string());
    if let Some(v) = version {
        m.insert("version".to_string(), v.to_string());
    }
    m
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Sub #212 — `with_embedded_binary` installs the path returned by
    /// the caller's embedded extraction (a tempdir + binary stem) AND
    /// keeps the [`tempfile::TempDir`] alive on the config so the path
    /// stays valid for every subprocess invocation.
    ///
    /// Bundles BOTH behavioural assertions — the embedded-tier install AND
    /// the env-override no-op — into a single test so that `cargo test`'s
    /// parallel runner can never race the two cases on the shared
    /// `ZFB_TAILWIND_BIN` env var. Splitting them caused the env-set test
    /// to leak the var into the embedded-tier test on parallel scheduling.
    ///
    /// We don't shell out to the real binary here — that's gated by the
    /// `#[ignore]` integration tests in zfb-build. This unit test just
    /// proves the wiring: the path is updated, the file is still present
    /// after we drop the original `TempDir` handle (which we surrender to
    /// the config), and the env override always wins.
    #[test]
    fn with_embedded_binary_installs_path_unless_env_override_is_set() {
        // Tiny scope guard so a panic doesn't leak the env var.
        struct EnvGuard {
            key: &'static str,
            prev: Option<std::ffi::OsString>,
        }
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                match &self.prev {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }

        // ----- Phase 1: env unset -> embedded path wins ----------------
        let prev = std::env::var_os("ZFB_TAILWIND_BIN");
        std::env::remove_var("ZFB_TAILWIND_BIN");
        let _guard = EnvGuard {
            key: "ZFB_TAILWIND_BIN",
            prev,
        };

        let dir = tempfile::tempdir().expect("tempdir");
        let bin_path = dir.path().join("tailwindcss-v4");
        std::fs::write(&bin_path, b"#!/bin/sh\necho ok\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }

        let cfg = TailwindSubprocessConfig::default()
            .with_embedded_binary(dir, bin_path.clone());
        assert_eq!(
            cfg.binary_path, bin_path,
            "no env override → embedded path should win over the workspace fallback"
        );
        assert!(
            cfg.binary_path.exists(),
            "embedded binary path should exist via the captured TempDir handle"
        );

        // Cloning the config preserves the embedded path and keeps the
        // handle alive (Arc<TempDir>). Drop the original — the clone's
        // path must still resolve.
        let cloned = cfg.clone();
        drop(cfg);
        assert!(
            cloned.binary_path.exists(),
            "Clone should keep the TempDir alive so binary_path stays valid"
        );
        drop(cloned);

        // ----- Phase 2: env set -> env value wins, embedded is no-op --
        std::env::set_var("ZFB_TAILWIND_BIN", "/tmp/zfb-test-tailwind-env-override");

        let dir2 = tempfile::tempdir().expect("tempdir");
        let bin_path2 = dir2.path().join("tailwindcss-v4");
        std::fs::write(&bin_path2, b"x").unwrap();

        // Default reads the env override.
        let cfg = TailwindSubprocessConfig::default();
        assert_eq!(
            cfg.binary_path,
            PathBuf::from("/tmp/zfb-test-tailwind-env-override"),
            "default() should honour ZFB_TAILWIND_BIN"
        );

        // with_embedded_binary must not displace the env-override value.
        let cfg = cfg.with_embedded_binary(dir2, bin_path2);
        assert_eq!(
            cfg.binary_path,
            PathBuf::from("/tmp/zfb-test-tailwind-env-override"),
            "env-override path must win over with_embedded_binary"
        );

        // _guard drops here, restoring the previous ZFB_TAILWIND_BIN value.
    }
}
