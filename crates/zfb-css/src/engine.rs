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

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

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
    pub fn with_embedded_binary(mut self, handle: tempfile::TempDir, path: PathBuf) -> Self {
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

/// Append a single `@source "<escaped_value>";\n` directive to `out`.
///
/// Only `"` and `\` need escaping in a CSS string literal; glob
/// metacharacters (`*`, `{`, `}`, etc.) are left untouched because
/// Tailwind v4 interprets them as glob syntax inside the `@source`
/// value — escaping them would break pattern expansion.
fn push_escaped_source(out: &mut String, value: &str) {
    out.push_str("@source \"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out.push_str("\";\n");
}

/// Strip CSS block comments (`/* … */`) and line comments (`//…`) from
/// `text` so that import-detection is not fooled by commented-out directives.
///
/// The algorithm is a byte state machine matching the one in
/// `scanner.rs::extract_module_css_specifiers`: string literals (delimited
/// by `"`, `'`, or `` ` ``) take precedence over comment scanning so that
/// `/*` inside a string is treated as literal text, not a comment start.
/// Only the "active" (uncommented) characters are returned.
///
/// Caveat: bytes are pushed via `b as char`, so multi-byte UTF-8 sequences
/// are mangled in the returned text. Harmless here — the result is a
/// detection-only scratch copy (the original bytes are what get emitted),
/// and the `@import "tailwindcss"` needle is pure ASCII.
fn strip_css_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        // String literals: copy verbatim so `/*` inside a string isn't
        // mistaken for a comment open.
        if b == b'"' || b == b'\'' || b == b'`' {
            let quote = b;
            out.push(b as char);
            i += 1;
            while i < bytes.len() {
                let c = bytes[i];
                out.push(c as char);
                if c == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                    out.push(bytes[i] as char);
                    i += 1;
                    continue;
                }
                if c == b'\n' || c == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
        } else if b == b'/' && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            if next == b'/' {
                // Line comment: skip until newline (keep the newline so
                // line numbers are preserved for any downstream tool).
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            } else if next == b'*' {
                // Block comment: skip until `*/`.
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    // Preserve newlines so multiline comments don't collapse
                    // adjacent lines together (prevents false `@import`
                    // detection across line boundaries).
                    if bytes[i] == b'\n' {
                        out.push('\n');
                    }
                    i += 1;
                }
                i = i.saturating_add(2); // consume `*/`
            } else {
                out.push(b as char);
                i += 1;
            }
        } else {
            out.push(b as char);
            i += 1;
        }
    }
    out
}

/// Whether a single CSS line is an `@import "tailwindcss"` directive.
///
/// Matches the v4 umbrella import (`@import "tailwindcss"`) and the split
/// sub-imports (`tailwindcss/preflight`, `tailwindcss/utilities`, …) in
/// either quote style, after trimming surrounding whitespace. This is the
/// single predicate shared by:
///
/// - [`build_synthesised_entry_css`]'s `user_has_import` detection (the
///   Tailwind-enabled path), and
/// - the `zfb` build command's authored-CSS import stripper (the
///   `tailwind.enabled = false` path, issue #824),
///
/// so both paths agree byte-for-byte on what counts as "the Tailwind
/// import". The caller passes a raw line; trimming happens here, so callers
/// feeding lines with or without a trailing `\n` get identical results.
pub fn is_tailwind_import_line(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("@import \"tailwindcss\"")
        || t.starts_with("@import 'tailwindcss'")
        || t.starts_with("@import \"tailwindcss/")
        || t.starts_with("@import 'tailwindcss/")
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

    // Strip block/line comments before scanning for the tailwind import so
    // that a commented-out `@import "tailwindcss";` inside a `/* … */`
    // block does not suppress the real synthesised import. The stripped text
    // is used only for detection; the original bytes are written to the
    // output unchanged.
    let user_has_import = input_css_text
        .map(|t| {
            let stripped = strip_css_comments(t);
            stripped.lines().any(is_tailwind_import_line)
        })
        .unwrap_or(false);

    if !user_has_import {
        out.push_str("@import \"tailwindcss\";\n");
        emitted_import = true;
    }

    // User-project content globs first.
    for g in &cfg.content_globs {
        push_escaped_source(&mut out, g);
    }
    // Then framework packages.
    for g in &cfg.framework_package_globs {
        push_escaped_source(&mut out, g);
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
                self.last_entry_css.lock().ok().and_then(|g| g.clone()),
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

/// Filename prefix for the synthesised Tailwind entry temp file. Shared by
/// the create site and the self-healing sweep so the two never drift.
const ENTRY_TMP_PREFIX: &str = "zfb-tailwind-entry-";
/// Filename suffix (extension) for the synthesised Tailwind entry temp file.
const ENTRY_TMP_SUFFIX: &str = ".css";
/// Minimum age before the sweep will delete a stranded entry temp file.
///
/// The sweep removes only files older than this so two concurrent builds in
/// the same project dir cannot delete each other's *live* entry file: a
/// Tailwind pass opens its `-i` entry at spawn (milliseconds), ~1000x under
/// this window, so a file this old is necessarily orphaned by an earlier
/// aborted run.
///
/// Out of scope (deliberately): the residual cross-process race where two
/// `zfb` builds run in the *same* `working_dir` and one build's Tailwind has
/// not opened its entry within this window (e.g. a multi-minute-latency
/// wrapper via `ZFB_TAILWIND_BIN`). std offers no portable file-liveness
/// check, a lockfile would add a dependency for a non-supported workflow, and
/// the worst case is a single build failing loudly with "file not found" —
/// never corruption or a silent bad artifact. On Unix, unlinking a file the
/// reader has already opened is harmless (the inode survives until fd close),
/// so only the create→open gap is at risk; on Windows the open file cannot be
/// unlinked at all.
const ENTRY_TMP_STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(60);

/// Delete `zfb-tailwind-entry-*.css` files left in `dir` by a previous run
/// that died before its [`tempfile::NamedTempFile`] `Drop` could clean up
/// (SIGKILL / crash / Ctrl-C). See zfb#821.
///
/// Only files older than [`ENTRY_TMP_STALE_AFTER`] are removed, so a sibling
/// build running concurrently in the same project keeps its freshly-created
/// live entry. Best-effort: any individual error (unreadable dir, racing
/// unlink, permission denied) is ignored — a failed sweep must never break a
/// build.
fn sweep_stale_entry_files(dir: &Path) {
    sweep_stale_entry_files_at(dir, std::time::SystemTime::now());
}

/// Core of [`sweep_stale_entry_files`], parameterised on the reference time
/// so tests can drive staleness without manipulating file mtimes (which has
/// no `std`-only setter). A file counts as stale when
/// `now - mtime >= ENTRY_TMP_STALE_AFTER`.
fn sweep_stale_entry_files_at(dir: &Path, now: std::time::SystemTime) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with(ENTRY_TMP_PREFIX) && name.ends_with(ENTRY_TMP_SUFFIX)) {
            continue;
        }
        // Skip directories and anything we can't stat.
        let metadata = match entry.metadata() {
            Ok(m) if m.is_file() => m,
            _ => continue,
        };
        // Age out via mtime: only sweep files that have been sitting long
        // enough to be certainly orphaned, never a sibling's live entry.
        let old_enough = metadata
            .modified()
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok())
            .map(|age| age >= ENTRY_TMP_STALE_AFTER)
            .unwrap_or(false);
        if old_enough {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

impl CssEngine for TailwindSubprocessEngine {
    fn produce_utility_css(&self, sources: &[PathBuf]) -> Result<String> {
        // Build the synthesised entry CSS. Read user input_css if set —
        // failure to read is fatal because the user explicitly asked for
        // it.
        let user_text = match &self.config.input_css {
            Some(p) => Some(
                std::fs::read_to_string(p)
                    .with_context(|| format!("failed to read user input CSS at {}", p.display()))?,
            ),
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

        // External-tool quirk: make the Bun-embedded oxide `.node` extract
        // exactly once, serialized, before any parallel invocation can race on
        // the shared addon file. Without this, concurrent cold spawns (e.g. the
        // parallel integration tests all driving this one binary) intermittently
        // load a half-written addon and die with
        // `undefined is not a constructor (new import_oxide.Scanner(...))`.
        // See `ensure_oxide_extracted` and zfb#1237 for the full rationale.
        ensure_oxide_extracted(&self.config.binary_path);

        // Self-heal: delete entry temp files stranded by a past abnormal
        // termination (SIGKILL / crash / Ctrl-C skips the RAII `Drop` that
        // normally removes them). Done before we create the new file so the
        // project root is clean even if `git add -A` ran since the leak.
        // See zfb#821.
        sweep_stale_entry_files(&self.config.working_dir);

        // Materialise the synthesised entry CSS into a temp file so
        // Tailwind's `@source` resolution uses our wrapper. The file lives
        // in `working_dir` (not a subdir like `.zfb/`) on purpose: the
        // user's `input_css` is inlined into this entry, so any relative
        // `@import "./x.css";` in it resolves against the entry file's
        // directory — which must be `working_dir` for those imports to find
        // the user's siblings. `@source` paths are already absolute, so they
        // are unaffected by the location.
        let mut entry_tmp = tempfile::Builder::new()
            .prefix(ENTRY_TMP_PREFIX)
            .suffix(ENTRY_TMP_SUFFIX)
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

/// Force the Tailwind v4 standalone binary to extract its embedded oxide
/// native addon (`.node`) **once, serialized**, before any concurrent build
/// invocation can race on it.
///
/// ## Why this exists (external-tool quirk — not recoverable from our code)
///
/// The `tailwindcss` v4 standalone CLI is a Bun-compiled single-file
/// executable. Its Rust scanner (`@tailwindcss/oxide`) ships as a native
/// `.node` addon embedded in that binary. Bun extracts the addon **lazily** —
/// on the first real scan, NOT on `--help`/`--version` — to a single
/// content-addressed path under `$TMPDIR` (`$TMPDIR/.<exe-hash>-*.node`) that is
/// **shared by every invocation of the same binary**. When several `tailwindcss`
/// processes are spawned concurrently against a cold cache (e.g. the parallel
/// integration tests under `cargo test`, all driving this one shared binary),
/// two can race that first extraction and a reader `dlopen`s a half-written
/// addon, surfacing as:
///
/// ```text
/// TypeError: undefined is not a constructor (evaluating 'new import_oxide.Scanner(...)')
/// ```
///
/// (Observed intermittently — ~1 in 8 — on the `health` CI gate; zfb#1237.)
///
/// The fix: run one throwaway minimal build while holding a process-global
/// lock, so the addon is fully extracted before any parallel invocation
/// proceeds. Later invocations find the extracted addon on disk and reuse it
/// (it persists for the life of `$TMPDIR`). Keyed by binary path so an
/// env/temp-dir override is warmed independently.
///
/// Best-effort: a warm-up failure is swallowed — the real invocation that
/// follows is the source of truth for genuine errors.
///
/// Caveat: serialization is per-process. Plain `cargo test` runs test binaries
/// sequentially, so the first process warms the shared addon and later ones
/// reuse it; a cross-process runner (`cargo nextest`) sharing one `$TMPDIR`
/// could still race two cold processes. zfb uses plain `cargo test`.
///
/// Returns `true` if this call performed the warm-up, `false` if the binary was
/// already warmed by an earlier caller. The production call site ignores the
/// return; tests use it to assert the once-only / serialized contract.
fn ensure_oxide_extracted(binary_path: &Path) -> bool {
    static WARMED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    let warmed = WARMED.get_or_init(|| Mutex::new(HashSet::new()));

    // Hold the lock ACROSS the warm-up spawn: concurrent callers block on
    // `.lock()` until the first has fully extracted the addon — that blocking
    // is the entire point, so the check and the spawn must share one critical
    // section (do not collapse this into `insert()`'s bool, which would let the
    // second caller proceed while the first is still extracting).
    let mut guard = warmed.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.contains(binary_path) {
        return false;
    }

    // A minimal `@import "tailwindcss";` build makes the CLI construct its
    // scanner, which triggers the lazy oxide `.node` extraction. `--help` does
    // not, so the warm-up must be a real build.
    if let Ok(dir) = tempfile::tempdir() {
        let in_css = dir.path().join("warmup.css");
        let out_css = dir.path().join("warmup.out.css");
        if std::fs::write(&in_css, b"@import \"tailwindcss\";\n").is_ok() {
            let _ = Command::new(binary_path)
                .arg("-i")
                .arg(&in_css)
                .arg("-o")
                .arg(&out_css)
                .output();
        }
        // `dir` is cleaned on drop.
    }

    // Mark warmed regardless of warm-up outcome: a persistent failure is a real
    // binary problem the next real invocation will report — we must not spin on
    // the warm-up every call.
    guard.insert(binary_path.to_path_buf());
    true
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
        // push_escaped_source escapes `"` and `\` so a project root
        // containing those bytes (legal on Linux/macOS) still emits a
        // well-formed `@source "..."` directive that Tailwind can parse.
        push_escaped_source(&mut out, &full.display().to_string());
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
            std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let cfg = TailwindSubprocessConfig::default().with_embedded_binary(dir, bin_path.clone());
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

    // -----------------------------------------------------------------------
    // Sub #706 — escaped @source globs
    // -----------------------------------------------------------------------

    /// A glob value containing a double-quote must be escaped so the emitted
    /// `@source "..."` directive is well-formed CSS.
    #[test]
    fn push_escaped_source_escapes_double_quote() {
        let mut out = String::new();
        push_escaped_source(&mut out, r#"pages/"odd"/**"#);
        assert_eq!(out, r#"@source "pages/\"odd\"/**";"#.to_string() + "\n");
    }

    /// A glob value containing a backslash (common on Windows paths) must be
    /// double-escaped so the CSS parser sees a single `\`.
    #[test]
    fn push_escaped_source_escapes_backslash() {
        let mut out = String::new();
        push_escaped_source(&mut out, r"C:\project\pages");
        assert_eq!(out, "@source \"C:\\\\project\\\\pages\";\n");
    }

    /// Glob metacharacters (`*`, `{`, `}`, `?`) must pass through untouched
    /// — Tailwind v4 needs them as-is for pattern expansion.
    #[test]
    fn push_escaped_source_leaves_glob_chars_untouched() {
        let mut out = String::new();
        push_escaped_source(&mut out, "pages/**/*.{tsx,jsx}");
        assert_eq!(out, "@source \"pages/**/*.{tsx,jsx}\";\n");
    }

    /// `build_synthesised_entry_css` must escape `"` in content_globs and
    /// framework_package_globs (the pre-existing `default_source_directives`
    /// path was already correct; this test covers the newly-fixed paths).
    #[test]
    fn synthesised_css_escapes_quotes_in_globs() {
        let cfg = TailwindSubprocessConfig::default()
            .with_mock_output(String::new())
            .with_content_globs([r#"pages/"special"/**"#])
            .with_framework_package_globs([r#"packages/"fw"/**"#]);
        let css = build_synthesised_entry_css(&cfg, None);
        assert!(
            css.contains(r#"@source "pages/\"special\"/**";"#),
            "content_globs quote not escaped; got:\n{css}"
        );
        assert!(
            css.contains(r#"@source "packages/\"fw\"/**";"#),
            "framework_package_globs quote not escaped; got:\n{css}"
        );
    }

    /// `default_source_directives` must escape a `"` in the project root.
    #[test]
    fn default_source_directives_escapes_quotes_in_root() {
        let root = std::path::PathBuf::from(r#"/pro"ject"#);
        let out = default_source_directives(&root);
        // Every content root entry should have the quote escaped.
        assert!(
            out.contains(r#"@source "/pro\"ject/"#),
            "quote in project root not escaped; got:\n{out}"
        );
    }

    // -----------------------------------------------------------------------
    // Sub #739 — comment-aware tailwind import detection
    // -----------------------------------------------------------------------

    /// A `@import "tailwindcss";` that appears only inside a `/* … */` block
    /// comment must NOT suppress the synthesised prepended import — it is not
    /// an active directive, so the real `@import "tailwindcss";\n` should
    /// appear at the top of the output.
    #[test]
    fn block_comment_import_does_not_suppress_synthesised_import() {
        let input_css = r#"
/*
 * Old import — kept for reference but commented out.
 * @import "tailwindcss";
 */

@layer base {
  body { margin: 0; }
}
"#;
        let cfg = TailwindSubprocessConfig::default().with_mock_output(String::new());
        let css = build_synthesised_entry_css(&cfg, Some(input_css));
        assert!(
            css.starts_with("@import \"tailwindcss\";\n"),
            "synthesised import must be prepended when the real import is inside a block comment;\
             \ngot:\n{css}"
        );
    }

    /// An active (uncommented) `@import "tailwindcss";` must still suppress
    /// the synthesised prepended import (regression guard).
    #[test]
    fn real_import_suppresses_synthesised_import() {
        let input_css = "@import \"tailwindcss\";\n\n@layer base { body { margin: 0; } }\n";
        let cfg = TailwindSubprocessConfig::default().with_mock_output(String::new());
        let css = build_synthesised_entry_css(&cfg, Some(input_css));
        // The synthesised "@import ..." must NOT appear; the user's own line
        // is already in the output as part of input_css_text.
        let import_count = css.matches("@import \"tailwindcss\"").count();
        assert_eq!(
            import_count, 1,
            "only the user's own import should appear (count=1); got:\n{css}"
        );
    }

    /// A `@import "tailwindcss";` inside a line comment (`//`) must also not
    /// suppress the synthesised import.
    #[test]
    fn line_comment_import_does_not_suppress_synthesised_import() {
        let input_css = "// @import \"tailwindcss\";\n@layer base {}\n";
        let cfg = TailwindSubprocessConfig::default().with_mock_output(String::new());
        let css = build_synthesised_entry_css(&cfg, Some(input_css));
        assert!(
            css.starts_with("@import \"tailwindcss\";\n"),
            "synthesised import must be prepended when the real import is inside a line comment;\
             \ngot:\n{css}"
        );
    }

    /// `strip_css_comments` must not mangle an active import on a line
    /// adjacent to a multi-line block comment.
    #[test]
    fn strip_css_comments_preserves_active_imports_adjacent_to_block_comment() {
        let text = "/* comment */\n@import \"tailwindcss\";\n";
        let stripped = strip_css_comments(text);
        assert!(
            stripped.contains("@import \"tailwindcss\";"),
            "active import after block comment must survive stripping; got:\n{stripped}"
        );
    }

    // -----------------------------------------------------------------------
    // zfb#821 — self-healing sweep of stranded entry temp files
    // -----------------------------------------------------------------------

    use std::time::{Duration, SystemTime};

    /// A reference "now" far enough in the future that any just-written file
    /// reads as stale to the sweep — exercises the stale branch without an
    /// mtime setter (which `std` lacks).
    fn future_now() -> SystemTime {
        SystemTime::now() + ENTRY_TMP_STALE_AFTER + Duration::from_secs(3600)
    }

    /// A stale `zfb-tailwind-entry-*.css` left in `working_dir` by an aborted
    /// run (SIGKILL skipped its RAII `Drop`) must be removed by the sweep so a
    /// later `git add -A` cannot sweep it into the user's repo.
    #[test]
    fn sweep_removes_stale_entry_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stranded = dir.path().join("zfb-tailwind-entry-UErIaE.css");
        std::fs::write(&stranded, b"/* leaked */").unwrap();

        // `now` in the future → the just-written file is past the stale window.
        sweep_stale_entry_files_at(dir.path(), future_now());

        assert!(
            !stranded.exists(),
            "stale entry temp file should have been swept; still present at {}",
            stranded.display()
        );
    }

    /// The sweep must NOT delete a freshly-created entry file — that would be
    /// a *live* sibling's file in a concurrent build in the same project dir.
    #[test]
    fn sweep_keeps_fresh_entry_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let live = dir.path().join("zfb-tailwind-entry-rN8CIp.css");
        std::fs::write(&live, b"/* live */").unwrap();

        // Real `now`: the file's mtime is within the stale window, so it stays.
        sweep_stale_entry_files_at(dir.path(), SystemTime::now());

        assert!(
            live.exists(),
            "a fresh entry temp file (live concurrent build) must be kept"
        );
    }

    /// The sweep must touch ONLY `zfb-tailwind-entry-*.css` files. Unrelated
    /// files in the project root — even ones old enough to be eligible by age
    /// — must survive.
    #[test]
    fn sweep_leaves_unrelated_files_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let user_file = dir.path().join("global.css");
        let other_tmp = dir.path().join("zfb-tailwind-out-abc.css"); // output temp, different prefix
        let no_ext = dir.path().join("zfb-tailwind-entry-noext"); // right prefix, wrong suffix
        std::fs::write(&user_file, b"body{}").unwrap();
        std::fs::write(&other_tmp, b"/* out */").unwrap();
        std::fs::write(&no_ext, b"x").unwrap();

        // Future `now` makes every file age-eligible; only the name filter
        // protects the unrelated ones.
        sweep_stale_entry_files_at(dir.path(), future_now());

        assert!(user_file.exists(), "user CSS must never be swept");
        assert!(
            other_tmp.exists(),
            "output temp (different prefix) must not be swept by the entry sweep"
        );
        assert!(
            no_ext.exists(),
            "file with the entry prefix but wrong suffix must not be swept"
        );
    }

    /// The sweep on a non-existent directory must be a silent no-op (best
    /// effort) — it must never panic or surface an error into the build.
    #[test]
    fn sweep_on_missing_dir_is_noop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("does-not-exist");
        sweep_stale_entry_files_at(&missing, future_now()); // must not panic
    }

    /// The oxide warm-up must run exactly once per binary path even when many
    /// callers hit it concurrently — that single serialized extraction is what
    /// prevents the Bun `.node` race (zfb#1237). Exercised deterministically
    /// with a non-existent binary: the warm-up spawn fails (best-effort,
    /// swallowed), but the once-only / serialized bookkeeping is exactly what we
    /// assert, with no dependency on the real tailwind binary.
    #[test]
    fn ensure_oxide_extracted_warms_exactly_once_under_concurrency() {
        use std::thread;

        // Unique, non-existent path so it is guaranteed absent from the
        // process-global warmed set at the start of this test.
        let dir = tempfile::tempdir().expect("tempdir");
        let fake_bin = Arc::new(dir.path().join("not-a-real-tailwind"));

        const N: usize = 16;
        let handles: Vec<_> = (0..N)
            .map(|_| {
                let p = Arc::clone(&fake_bin);
                thread::spawn(move || ensure_oxide_extracted(&p))
            })
            .collect();
        let warmed_count = handles
            .into_iter()
            .map(|h| h.join().expect("warm-up thread panicked"))
            .filter(|&did_warm| did_warm)
            .count();

        assert_eq!(
            warmed_count, 1,
            "exactly one of {N} concurrent callers must perform the warm-up; \
             the rest must see it already warmed (serialized first-extraction)"
        );

        // A later call for the same path is a no-op (stays warmed).
        assert!(
            !ensure_oxide_extracted(&fake_bin),
            "already-warmed path must not warm again"
        );
    }
}
