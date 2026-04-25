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

    /// Optional explicit input CSS file (`-i` flag). When `None`, Tailwind
    /// v4's default scan-and-emit mode is used. Most projects will provide
    /// a tiny entrypoint like `@import "tailwindcss";`.
    pub input_css: Option<PathBuf>,

    /// Extra CLI args appended to the subprocess invocation, for escape
    /// hatches like `--minify`. These are passed verbatim.
    pub extra_args: Vec<OsString>,

    /// When true, the engine will return a *fake* CSS string instead of
    /// invoking the subprocess. Used by unit tests to avoid depending on
    /// the binary being installed.
    ///
    /// The string returned is taken from [`Self::mock_output`].
    pub mock_subprocess: bool,

    /// Output to return when `mock_subprocess` is true.
    pub mock_output: String,
}

impl Default for TailwindSubprocessConfig {
    fn default() -> Self {
        // Resolve binary path: env override → default workspace-relative path.
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
            mock_subprocess: false,
            mock_output: String::new(),
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
#[derive(Debug, Clone)]
pub struct TailwindSubprocessEngine {
    config: TailwindSubprocessConfig,
}

impl TailwindSubprocessEngine {
    /// Construct a new engine with the given config.
    pub fn new(config: TailwindSubprocessConfig) -> Self {
        Self { config }
    }

    /// Construct a new engine with the default config.
    pub fn with_default_config() -> Self {
        Self::new(TailwindSubprocessConfig::default())
    }

    /// Borrow the underlying config.
    pub fn config(&self) -> &TailwindSubprocessConfig {
        &self.config
    }
}

impl CssEngine for TailwindSubprocessEngine {
    fn produce_utility_css(&self, sources: &[PathBuf]) -> Result<String> {
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

        let tmp = tempfile::Builder::new()
            .prefix("zfb-tailwind-")
            .suffix(".css")
            .tempfile()
            .context("failed to allocate temp file for tailwind output")?;

        let mut cmd = Command::new(&self.config.binary_path);
        cmd.current_dir(&self.config.working_dir);
        if let Some(input) = &self.config.input_css {
            cmd.arg("-i").arg(input);
        }
        cmd.arg("-o").arg(tmp.path());
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

        let output = cmd
            .output()
            .with_context(|| format!("failed to spawn {}", self.config.binary_path.display()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "tailwindcss exited with status {}: {}",
                output.status,
                stderr.trim()
            ));
        }

        let css = std::fs::read_to_string(tmp.path())
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
