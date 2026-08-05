//! The "CSS engine" abstraction.
//!
//! Tailwind v4 is delivered as a Bun-compiled standalone CLI. To keep our build
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
use sha2::{Digest, Sha256};

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

    /// Companion assets emitted for package-attributed `url()` references
    /// resolved during the most recent [`Self::produce_utility_css`] call
    /// (issue #2316, decision c: hash-upstream, emit-as-CSS-companions).
    ///
    /// Default: none. Only an engine that resolves package-attributed
    /// `url()`s against a real filesystem (currently
    /// [`TailwindSubprocessEngine`], via the Tailwind compiler's own
    /// sourcemap) overrides this. [`crate::CssPipeline::build_emitter`]
    /// calls it once, immediately after `produce_utility_css`, and folds
    /// the result into [`crate::emitter::CssEmitterOutput::companions`].
    /// "Take" semantics (draining, not peeking) so a stale companion list
    /// from an earlier call can never leak into a later one.
    fn take_package_url_companions(&self) -> Vec<crate::url_attribution::PackageUrlAsset> {
        Vec::new()
    }
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
    /// hatches. These are passed verbatim, EXCEPT that
    /// [`TailwindSubprocessEngine::produce_utility_css`] hard-errors before
    /// spawning the CLI if this contains a minification/optimization flag
    /// (`-m`/`--minify`/`--optimize`) — see
    /// `reject_minify_flags_incompatible_with_attribution`'s doc comment
    /// (codex review finding, #2327): those flags are incompatible with the
    /// package `url()` sourcemap attribution the engine always runs.
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

    /// Negative content globs (Tailwind v4 `@source not "<glob>";`
    /// directives) that exclude subtrees from the explicit `@source` scan
    /// established by `content_globs` / `framework_package_globs` — e.g.
    /// an ungitignored generated subtree inside a workspace-sibling
    /// mirror root (`node_modules`/`dist`/`.git`/`target`/`.turbo`/
    /// `.next`/`.vercel`) that would otherwise leak stale class strings
    /// into the emitted stylesheet.
    ///
    /// `@source not` is a Tailwind v4.1+ addition (the pinned v4.2.0
    /// binary supports it); the engine already parses and rebases this
    /// form when it appears in user-authored CSS — see
    /// [`rebase_source_line`]'s `negated` handling. Each entry becomes
    /// one `@source not "<glob>";` directive, emitted after the positive
    /// `content_globs` / `framework_package_globs` / `inline_sources`
    /// directives so an exclusion always has something to exclude *from*
    /// by the time Tailwind evaluates it.
    ///
    /// Like `content_globs` / `framework_package_globs`, entries are
    /// written verbatim — the engine does NOT rebase them onto
    /// `working_dir`. Only `@source`/`@source not` lines parsed out of
    /// user-authored `input_css` get that treatment (zfb#1327). Tailwind
    /// resolves a relative directive against the synthesised entry's own
    /// directory (see [`entry_dir`], which can differ from `working_dir`
    /// when `input_css` is set), so callers populating this field must
    /// supply already-absolute globs — exactly as `crates/zfb/src/commands/build.rs`
    /// already does for `content_globs`.
    ///
    /// Defaults to empty — no behavior change for callers that don't set
    /// it.
    pub negative_source_globs: Vec<String>,

    /// Content globs for **framework packages** that must NOT be
    /// tree-shaken even though they live outside the user project (e.g.
    /// `packages/zudo-doc-v2/**` after the Phase B split-out). Each
    /// entry becomes a separate `@source` directive — listed after the
    /// user-project globs so per-project overrides win in cascade
    /// order.
    pub framework_package_globs: Vec<String>,

    /// Class-name literals safelisted via `@source inline("<value>");`
    /// (Tailwind v4's "inline source" form — carries a class name, not a
    /// path/glob). zfb#1534: `codeHighlight.roleClasses` values live in
    /// `zfb.config.ts` (not a scanned content root) and are emitted only
    /// into rendered `dist/*.html` (never scanned), so without this the
    /// mapped utilities are silently never generated. Populated by
    /// `build_default_css_payload` from every `roleClasses` value, split
    /// on whitespace, deduped, and sorted (determinism — this feeds the
    /// synthesised entry CSS, which in turn feeds the CSS `hash_8`
    /// input). Listed after the content/framework globs so it reads as
    /// an explicit safelist appendix.
    pub inline_sources: Vec<String>,

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
        // An empty value (set but blank) is treated the same as unset,
        // mirroring the build-time override contract in
        // `crates/zfb/build.rs` / `BUILDING.md`.
        let env_override = std::env::var_os("ZFB_TAILWIND_BIN").filter(|v| !v.is_empty());
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
            negative_source_globs: Vec::new(),
            framework_package_globs: Vec::new(),
            inline_sources: Vec::new(),
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

    /// Replace the negative content globs (`@source not "..."`,
    /// chainable). See [`Self::negative_source_globs`].
    pub fn with_negative_source_globs<I, S>(mut self, globs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.negative_source_globs = globs.into_iter().map(Into::into).collect();
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

    /// Replace the `@source inline("...")` class-name safelist
    /// (chainable). See [`Self::inline_sources`].
    pub fn with_inline_sources<I, S>(mut self, sources: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.inline_sources = sources.into_iter().map(Into::into).collect();
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
        // Empty value = unset, matching `Self::default`'s check above.
        if std::env::var_os("ZFB_TAILWIND_BIN").is_some_and(|v| !v.is_empty()) {
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
    push_escaped_css_string_value(out, value);
    out.push_str("\";\n");
}

/// Append a single `@source not "<escaped_value>";\n` directive to `out`.
/// Mirrors [`push_escaped_source`]'s escaping rules — only `"` and `\`
/// are escaped; glob metacharacters pass through untouched so Tailwind's
/// glob expansion still applies to the excluded pattern.
fn push_escaped_negative_source(out: &mut String, value: &str) {
    out.push_str("@source not \"");
    push_escaped_css_string_value(out, value);
    out.push_str("\";\n");
}

/// Append a single `@source inline("<escaped_value>");\n` directive to
/// `out`. Unlike [`push_escaped_source`], this form carries a literal
/// class name (not a path/glob) — see [`rebase_relative_source_globs`]'s
/// doc comment, which is why the rebase pass leaves `inline(...)`
/// untouched. Only `"` and `\` need escaping, same as the path form.
fn push_escaped_inline_source(out: &mut String, value: &str) {
    out.push_str("@source inline(\"");
    push_escaped_css_string_value(out, value);
    out.push_str("\");\n");
}

/// Append `value` to `out` escaped for use inside a double-quoted CSS
/// string literal (`"` and `\` are the only bytes that need escaping;
/// glob metacharacters must pass through — see [`push_escaped_source`]).
fn push_escaped_css_string_value(out: &mut String, value: &str) {
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
}

/// Rewrite relative `@source` globs in the user's inlined CSS to absolute
/// paths anchored at `base` (the engine's `working_dir`, i.e. the project
/// root).
///
/// Tailwind v4 resolves a relative `@source` path against the directory of
/// the stylesheet *containing* the directive. The user authored their
/// `@source` lines in `input_css`, but zfb inlines that text into a
/// synthesised entry whose on-disk location is an implementation detail —
/// `working_dir` before zfb#1300, `input_css`'s parent after. That
/// relocation silently rebased every relative glob and broke consumers who
/// authored project-root-relative `@source` lines: all their component /
/// package classes vanished from the emitted stylesheet (zfb#1327,
/// zudolab/zudo-doc#2511). Rewriting the globs to absolute paths pins the
/// contract the pre-#1300 releases established — "a relative `@source` in
/// `input_css` is project-root-relative" — no matter where the entry temp
/// file is created.
///
/// Scope: only single-line `@source "<path>";` / `@source not "<path>";`
/// forms (either quote style) are rewritten. `@source inline(...)`,
/// absolute paths, empty values, and anything unparsable pass through
/// byte-for-byte. Scanning is line-based like [`is_tailwind_import_line`];
/// a commented-out `@source` line may still be rewritten, which is
/// harmless — the rewritten bytes stay inside the comment.
fn rebase_relative_source_globs(text: &str, base: &Path) -> String {
    let mut out = String::with_capacity(text.len());
    for (idx, line) in text.split('\n').enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        match rebase_source_line(line, base) {
            Some(rewritten) => out.push_str(&rewritten),
            None => out.push_str(line),
        }
    }
    out
}

/// Rewrite a single line when it is a relative `@source` directive.
/// Returns `None` (caller emits the original line unchanged) for anything
/// else. See [`rebase_relative_source_globs`] for the contract.
fn rebase_source_line(line: &str, base: &Path) -> Option<String> {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let after_keyword = rest.strip_prefix("@source")?;
    // Require whitespace after the keyword so e.g. `@source-map` (a
    // hypothetical future at-rule) is never touched.
    if !after_keyword.starts_with([' ', '\t']) {
        return None;
    }
    let mut body = after_keyword.trim_start_matches([' ', '\t']);
    let mut negated = false;
    if let Some(after_not) = body.strip_prefix("not") {
        if after_not.starts_with([' ', '\t']) {
            negated = true;
            body = after_not.trim_start_matches([' ', '\t']);
        }
    }
    let quote = body.chars().next()?;
    if quote != '"' && quote != '\'' {
        // `inline(...)` sources (and malformed directives) — not a path.
        return None;
    }
    let inner = &body[quote.len_utf8()..];
    // Find the closing quote with CSS backslash-escape awareness,
    // collecting the unescaped value as we go.
    let mut value = String::new();
    let mut close = None;
    let mut chars = inner.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            let (_, escaped) = chars.next()?; // dangling escape → unparsable
            value.push(escaped);
        } else if c == quote {
            close = Some(i);
            break;
        } else {
            value.push(c);
        }
    }
    let close = close?; // no closing quote on this line → leave unchanged
    let suffix = &inner[close + quote.len_utf8()..];
    // An empty value is meaningless as authored; rebasing it would turn it
    // into "scan the whole project root" — leave it alone.
    if value.is_empty() || Path::new(&value).is_absolute() {
        return None;
    }
    let joined = base.join(&value);
    let mut rewritten = String::with_capacity(line.len() + 32);
    rewritten.push_str(indent);
    rewritten.push_str("@source ");
    if negated {
        rewritten.push_str("not ");
    }
    rewritten.push('"');
    push_escaped_css_string_value(&mut rewritten, &joined.display().to_string());
    rewritten.push('"');
    rewritten.push_str(suffix);
    Some(rewritten)
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
/// 4. `@source inline("<value>");` directives for every entry in
///    `inline_sources` — the `codeHighlight.roleClasses` safelist
///    (zfb#1534). Emitted after the path-based `@source` directives so
///    it reads as an explicit appendix to the scanned-content list.
/// 5. `@source not "<glob>";` directives for every entry in
///    `negative_source_globs`, excluding subtrees from the scan
///    established by steps 2–4. Emitted last among the directive block
///    so an exclusion always has a preceding positive `@source` to apply
///    against.
/// 6. The contents of `input_css` if provided (the user's
///    `styles/global.css`, typically including their own `@theme {…}`
///    and authored CSS rules). When the file already starts with
///    `@import "tailwindcss";`, the synthesiser drops the duplicate
///    import we added in step 1 — Tailwind v4 errors on a doubled
///    import. Relative `@source` globs in this text are rebased to
///    absolute paths anchored at `working_dir` so the entry temp file's
///    location cannot change what they match (zfb#1327) — see
///    [`rebase_relative_source_globs`].
/// 7. The inline `theme_block`, if any.
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
    // Then the `codeHighlight.roleClasses` inline safelist (zfb#1534).
    for s in &cfg.inline_sources {
        push_escaped_inline_source(&mut out, s);
    }
    // Then exclusions — `@source not` applies against the positive
    // directives above, so it stays last in the directive block.
    for g in &cfg.negative_source_globs {
        push_escaped_negative_source(&mut out, g);
    }

    if emitted_import
        || !cfg.content_globs.is_empty()
        || !cfg.framework_package_globs.is_empty()
        || !cfg.inline_sources.is_empty()
        || !cfg.negative_source_globs.is_empty()
    {
        out.push('\n');
    }

    if let Some(text) = input_css_text {
        // Inline the user's CSS with relative `@source` globs rebased onto
        // `working_dir` — the entry temp file's location must not decide
        // what those globs match (zfb#1327). See
        // [`rebase_relative_source_globs`].
        let text = rebase_relative_source_globs(text, &cfg.working_dir);
        out.push_str(&text);
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
    /// Companion assets emitted for package-attributed `url()` references
    /// resolved on the most recent real (non-mock) `produce_utility_css`
    /// call. Drained by [`CssEngine::take_package_url_companions`]; the
    /// mock path clears this rather than populating it, since it never
    /// runs attribution (see the call site's doc comment).
    package_url_companions: std::sync::Mutex<Vec<crate::url_attribution::PackageUrlAsset>>,
}

impl Clone for TailwindSubprocessEngine {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            last_entry_css: std::sync::Mutex::new(
                self.last_entry_css.lock().ok().and_then(|g| g.clone()),
            ),
            package_url_companions: std::sync::Mutex::new(
                self.package_url_companions
                    .lock()
                    .map(|g| g.clone())
                    .unwrap_or_default(),
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
            package_url_companions: std::sync::Mutex::new(Vec::new()),
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

/// Directory the synthesised Tailwind entry temp file is created in — and
/// the directory the stale-entry sweep must sweep. The two MUST agree, so
/// both the create site and the sweep site call this single helper.
///
/// Resolution: `input_css`'s contents are inlined verbatim into the
/// synthesised entry (see [`build_synthesised_entry_css`]), so any relative
/// `@import "./x.css";` inside it resolves against the entry file's own
/// directory. That directory must therefore be `input_css`'s parent — not
/// `working_dir` — for such a sibling import to find `./x.css` (zfb#1300).
/// When there is no `input_css` (fully synthesised entry, no user file to
/// inline), there is no relative-import scope to preserve, so the entry
/// falls back to `working_dir`.
///
/// `@source` directives are unaffected either way — zfb's own globs are
/// always emitted as absolute paths (see `default_source_directives` / the
/// globs built in `crates/zfb/src/commands/build.rs`), and relative globs
/// authored inside `input_css` are rebased to absolute paths at inline time
/// (zfb#1327 — see [`rebase_relative_source_globs`]).
///
/// `input_css` is expected to be absolute in production (the sole caller,
/// `resolve_input_global_css` in `crates/zfb/src/commands/build.rs`, builds
/// it by joining onto an absolute `project_root`). Defensively, a *relative*
/// parent is still resolved against `working_dir` rather than the process's
/// own cwd, so the temp file always lands next to `input_css` regardless of
/// how the caller constructed the path.
fn entry_dir(cfg: &TailwindSubprocessConfig) -> PathBuf {
    match &cfg.input_css {
        Some(input_css) => match input_css.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => {
                if parent.is_absolute() {
                    parent.to_path_buf()
                } else {
                    cfg.working_dir.join(parent)
                }
            }
            _ => cfg.working_dir.clone(),
        },
        None => cfg.working_dir.clone(),
    }
}

/// Tailwind v4 CLI flags (pinned binary v4.2.0) that turn on Lightning CSS
/// minification/optimization. `-m` is `--minify`'s short form (not a
/// distinct behavior); `--optimize` runs Lightning CSS's optimizer without
/// minifying. Both are checked against
/// [`TailwindSubprocessConfig::extra_args`] by
/// [`reject_minify_flags_incompatible_with_attribution`].
const FORBIDDEN_MINIFY_FLAGS: &[&str] = &["--minify", "-m", "--optimize"];

/// Hard-error if `extra_args` requests Tailwind minification/optimization
/// (codex review finding, #2327).
///
/// [`TailwindSubprocessEngine::produce_utility_css`] always runs package
/// `url()` sourcemap attribution on the real (non-mock) subprocess path
/// (`--map` + `crate::url_attribution::attribute_and_emit_package_urls`) —
/// there is no separate opt-out. Attribution's documented trust boundary
/// (`url_attribution.rs`'s module doc, the "Valid only while zfb never
/// passes `-m`/`--optimize`" sentence) is that Lightning CSS's rule merging
/// under either flag destroys the one-declaration-per-line mapping shape
/// position-based sourcemap lookup depends on — silently risking wrong
/// attribution, a spurious hard-error, or the wrong asset being emitted,
/// none of which are safe to let through. Called before anything is spawned
/// (checked ahead of even the binary-exists check), so a misconfigured
/// `extra_args` fails loudly and immediately rather than after paying for a
/// subprocess invocation. The fix is to minify the FINAL emitted CSS
/// downstream instead of asking Tailwind to do it — not to make the
/// attribution lookup robust to optimized output (the unbounded parity-chase
/// #2312 already ruled out for this pipeline).
fn reject_minify_flags_incompatible_with_attribution(extra_args: &[OsString]) -> Result<()> {
    for arg in extra_args {
        // A non-UTF-8 arg can never spell one of these ASCII flags.
        let Some(arg_str) = arg.to_str() else {
            continue;
        };
        // Split off a `=value` suffix defensively — the pinned CLI takes
        // both flags as bare booleans today, but a future CLI version
        // growing an `=` form must not silently bypass this guard.
        let bare = arg_str.split('=').next().unwrap_or(arg_str);
        if FORBIDDEN_MINIFY_FLAGS.contains(&bare) {
            return Err(anyhow!(
                "TailwindSubprocessConfig::extra_args contains `{arg_str}`, which is \
                 incompatible with zfb's package `url()` attribution: Lightning CSS's rule \
                 merging under `-m`/`--minify`/`--optimize` breaks the sourcemap position \
                 lookup attribution depends on. Remove this flag and minify the final CSS \
                 downstream instead."
            ));
        }
    }
    Ok(())
}

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
            // The mock path never runs the real subprocess, so it never
            // parses a sourcemap or resolves package `url()`s. Clear any
            // companions left over from a prior real call so
            // `take_package_url_companions` cannot return a stale set
            // paired with this mock output.
            if let Ok(mut slot) = self.package_url_companions.lock() {
                slot.clear();
            }
            return Ok(self.config.mock_output.clone());
        }

        // #2311/#2315 attribution is unconditional beyond this point (see
        // the `--map` + `attribute_and_emit_package_urls` call below), so
        // `extra_args` must be checked for minification/optimization flags
        // BEFORE anything is spawned — see `reject_minify_flags_incompatible_with_attribution`'s
        // doc comment for why those flags corrupt the attribution.
        reject_minify_flags_incompatible_with_attribution(&self.config.extra_args)?;

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

        // Both the sweep and the temp-file create below must agree on the
        // entry's directory — compute it once via the shared helper.
        let entry_dir = entry_dir(&self.config);

        // Self-heal: delete entry temp files stranded by a past abnormal
        // termination (SIGKILL / crash / Ctrl-C skips the RAII `Drop` that
        // normally removes them). Done before we create the new file so the
        // entry dir is clean even if `git add -A` ran since the leak. See
        // zfb#821.
        sweep_stale_entry_files(&entry_dir);

        // Materialise the synthesised entry CSS into a temp file. There are
        // two distinct resolution scopes at play here:
        //
        // - `@source` directives are always absolute by the time they land
        //   in the entry: zfb's own globs are built absolute
        //   (`crates/zfb/src/commands/build.rs` / `default_source_directives`)
        //   and relative globs authored in `input_css` are rebased onto
        //   `working_dir` at inline time (zfb#1327,
        //   `rebase_relative_source_globs`). So they resolve correctly
        //   regardless of where the entry file lives.
        // - The user's `input_css` (e.g. `styles/global.css`) is inlined
        //   into this entry (see `build_synthesised_entry_css`), so
        //   any relative `@import "./x.css";` it contains resolves against
        //   the entry file's OWN directory. The entry must therefore live
        //   next to `input_css` (its parent dir) — not `working_dir` — for
        //   such sibling imports to find `./x.css` (zfb#1300). When there is
        //   no `input_css`, there is no relative-import scope to preserve,
        //   so we fall back to `working_dir`. See [`entry_dir`].
        let mut entry_tmp = tempfile::Builder::new()
            .prefix(ENTRY_TMP_PREFIX)
            .suffix(ENTRY_TMP_SUFFIX)
            .tempfile_in(&entry_dir)
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
        // #2311/#2315: record per-declaration provenance so relative `url()`s
        // inlined from `@import`ed package stylesheets can be attributed to
        // their origin. The map arrives as a trailing inline comment in the
        // output file and is parsed + stripped back out at the read below —
        // the returned CSS bytes stay identical to a `--map`-less run.
        cmd.arg("--map");
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

        let raw = std::fs::read_to_string(out_tmp.path())
            .context("failed to read tailwind output file")?;
        // Strip the `--map` comment, attribute every relative `url()`, and
        // emit+rewrite every package-attributed reference that resolves
        // (#2315 attribution + #2316 emission): a reference that cannot be
        // resolved still fails the build here — authored/project CSS
        // passes through byte-for-byte either way.
        let (css, companions) = crate::url_attribution::attribute_and_emit_package_urls(
            &raw,
            &self.config.working_dir,
        )?;
        if let Ok(mut slot) = self.package_url_companions.lock() {
            *slot = companions;
        }
        Ok(css)
    }

    fn take_package_url_companions(&self) -> Vec<crate::url_attribution::PackageUrlAsset> {
        self.package_url_companions
            .lock()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default()
    }
}

/// Force the Tailwind v4 standalone binary to extract its embedded oxide
/// native addon (`.node`) **once, serialized across processes**, before any
/// concurrent build invocation can race on it.
///
/// ## Why this exists (external-tool quirk — not recoverable from our code)
///
/// The `tailwindcss` v4 standalone CLI is a Bun-compiled single-file
/// executable. Its Rust scanner (`@tailwindcss/oxide`) ships as a native
/// `.node` addon embedded in that binary. Bun extracts the addon **lazily** —
/// on the first real scan, NOT on `--help`/`--version` — to a single
/// content-addressed path under `$TMPDIR` (`$TMPDIR/.<exe-hash>-*.node`) that is
/// **shared by every invocation of the same binary**. When several `tailwindcss`
/// processes are spawned concurrently against a cold cache, two can race that
/// first extraction and a reader `dlopen`s a half-written addon, surfacing as:
///
/// ```text
/// TypeError: undefined is not a constructor (evaluating 'new import_oxide.Scanner(...)')
/// ```
///
/// (Observed intermittently — ~1 in 8 — on the `health` CI gate; zfb#1237.)
///
/// ## Why the serialization must be cross-process
///
/// The contention is NOT just between threads of one process: the integration
/// tests spawn many short-lived `zfb build` child **processes** in parallel
/// (libtest threads each shell out via `Command`), all sharing the one extracted
/// addon under `$TMPDIR`. A within-process lock cannot serialize separate child
/// processes, so we coordinate through an advisory **file lock** keyed by the
/// binary path. The first holder runs one throwaway minimal build (which forces
/// the lazy extraction) and drops a `.done` marker; everyone else blocks on the
/// lock, then finds the marker and the extracted addon already in place. Lock +
/// marker live under `$TMPDIR` next to the addon, so they share its lifetime
/// (clear `$TMPDIR` → both vanish → the next cold process re-warms).
///
/// An in-process [`OnceLock`] set is the fast path: once a process has run the
/// protocol for a binary, later calls return without touching the filesystem,
/// and it collapses this process's own threads to a single protocol run.
///
/// Best-effort: any IO/warm-up failure is swallowed — the real invocation that
/// follows is the source of truth for genuine errors.
///
/// Returns `true` if this call ran the protocol, `false` if this process had
/// already warmed the binary. The production call site ignores the return;
/// tests use it to assert the once-per-process / serialized contract.
fn ensure_oxide_extracted(binary_path: &Path) -> bool {
    static WARMED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    let warmed = WARMED.get_or_init(|| Mutex::new(HashSet::new()));

    // In-process fast path. Holding the lock across the protocol also serializes
    // this process's threads so only one runs it; later calls hit `contains`.
    let mut guard = warmed
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.contains(binary_path) {
        return false;
    }
    warm_oxide_cross_process(binary_path);
    guard.insert(binary_path.to_path_buf());
    true
}

/// Cross-process half of [`ensure_oxide_extracted`]: serialize the first oxide
/// extraction via an advisory file lock, so concurrent `zfb build` child
/// processes do not race the shared addon. Best-effort — any IO error is
/// swallowed.
///
/// ## Keyed by binary CONTENT, not path
///
/// Each `zfb build` extracts its embedded tailwind binary to a *fresh* tempdir
/// (`render_pipeline::embedded_binary`), so the children's binary *paths* all
/// differ — yet Bun addresses the extracted addon by executable *content*, so
/// the identical-content children share one addon and race it. (If Bun keyed by
/// path the children would be isolated and there would be no cross-process race
/// to begin with — but CI proves there is one.) So the `.done` marker is keyed
/// by a content hash of the binary; all identical-content invocations agree on
/// one marker and warm exactly once. The lock file is a single global one under
/// `$TMPDIR` — distinct versions merely serialize their (separate) warm-ups,
/// which is harmless. Marker lives next to Bun's addon under `$TMPDIR`, sharing
/// its lifetime (clear `$TMPDIR` → both vanish → the next cold process re-warms).
fn warm_oxide_cross_process(binary_path: &Path) {
    let Some(key) = tailwind_content_key(binary_path) else {
        return;
    };
    let base = std::env::temp_dir();
    let lock_path = base.join("zfb-tailwind-oxide-warmup.lock");
    let done_path = base.join(format!("zfb-tailwind-oxide-warmup-{key}.done"));

    // Fast path for fresh processes: this binary content was already warmed in
    // this `$TMPDIR`; skip the lock entirely.
    if done_path.exists() {
        return;
    }

    let Ok(lock_file) = std::fs::File::create(&lock_path) else {
        return;
    };
    // Blocking advisory exclusive lock (flock / LockFileEx via std). Released on
    // unlock, drop, or process exit — so a crashed holder leaves no stale lock.
    if lock_file.lock().is_err() {
        return;
    }
    // Re-check under the lock: another process may have warmed while we waited.
    if !done_path.exists() && run_oxide_warmup_build(binary_path) {
        // Mark done only on success: a failed warm-up must not let the next
        // process skip warming against a still-unextracted addon.
        let _ = std::fs::File::create(&done_path);
    }
    let _ = lock_file.unlock();
}

/// A stable key for the tailwind binary's *content* — `sha256` of the file
/// bytes, truncated to 16 hex chars. Read in chunks to avoid buffering the
/// whole ~80 MiB binary; the just-extracted file is usually still warm in the
/// page cache, so this is cheap in practice. Returns `None` if the file cannot
/// be read (the real invocation will report that failure).
fn tailwind_content_key(binary_path: &Path) -> Option<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(binary_path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hex::encode(&hasher.finalize()[..8]))
}

/// Run one throwaway minimal Tailwind build to force Bun's lazy oxide `.node`
/// extraction. Returns whether it succeeded. Spawned with its CWD set to the
/// scratch dir so Tailwind v4's working-directory source auto-detection (active
/// for a bare `@import "tailwindcss";` with no `source(...)`) scans only the
/// empty temp dir — never the user's project/monorepo.
fn run_oxide_warmup_build(binary_path: &Path) -> bool {
    // The binary path must be made absolute BEFORE we override the child's cwd:
    // a relative program path (the default `crates/zfb/binaries/tailwindcss-v4`,
    // or a relative `ZFB_TAILWIND_BIN`) would otherwise be resolved against the
    // scratch `current_dir` below — not the caller's cwd — so the spawn would
    // fail and silently skip the warm-up.
    let Ok(abs_bin) = binary_path.canonicalize() else {
        return false;
    };
    let Ok(dir) = tempfile::tempdir() else {
        return false;
    };
    let in_css = dir.path().join("warmup.css");
    let out_css = dir.path().join("warmup.out.css");
    if std::fs::write(&in_css, b"@import \"tailwindcss\";\n").is_err() {
        return false;
    }
    Command::new(&abs_bin)
        .current_dir(dir.path())
        .arg("-i")
        .arg(&in_css)
        .arg("-o")
        .arg(&out_css)
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
    // `dir` is cleaned on drop.
}

/// Default content roots that the project scans for utility classes.
///
/// `zfb-css` does not enforce a specific scanning strategy — Tailwind v4 has
/// its own — but exposes this constant so callers (and the tailwind config
/// generator) agree on a single list.
pub const DEFAULT_CONTENT_ROOTS: &[&str] = &["pages", "components", "layouts", "content", "src"];

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

    /// The `@source not` negative form escapes the same way as the
    /// positive form — mirrors [`push_escaped_source_escapes_double_quote`].
    #[test]
    fn push_escaped_negative_source_escapes_double_quote() {
        let mut out = String::new();
        push_escaped_negative_source(&mut out, r#"pages/"odd"/**"#);
        assert_eq!(out, r#"@source not "pages/\"odd\"/**";"#.to_string() + "\n");
    }

    // -----------------------------------------------------------------------
    // zfb#1327 — relative `@source` globs in inlined input CSS must be
    // rebased onto working_dir so the entry temp file's location cannot
    // change what they match (Tailwind v4 resolves relative `@source`
    // against the containing stylesheet's directory)
    // -----------------------------------------------------------------------

    /// Platform-absolute base for the rebase tests — a drive-prefixed path
    /// on Windows so `is_absolute()` holds there too (a bare `/project/root`
    /// is NOT absolute on Windows and would flip the rewrite decision).
    fn rebase_test_base() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from("C:\\project\\root")
        } else {
            PathBuf::from("/project/root")
        }
    }

    /// The escaped CSS-string form of `base.join(rel)` — how a rewritten
    /// directive's value must appear in the output on the host platform
    /// (Windows joins with `\`, which the CSS string then escapes).
    fn rebased_value(base: &Path, rel: &str) -> String {
        let mut out = String::new();
        push_escaped_css_string_value(&mut out, &base.join(rel).display().to_string());
        out
    }

    /// A relative `@source` glob is rewritten to an absolute path anchored
    /// at `base`, preserving the terminating `;`.
    #[test]
    fn rebase_rewrites_relative_source_to_absolute() {
        let base = rebase_test_base();
        let out =
            rebase_relative_source_globs("@source \"src/components/**/*.{tsx,ts}\";\n", &base);
        assert_eq!(
            out,
            format!(
                "@source \"{}\";\n",
                rebased_value(&base, "src/components/**/*.{tsx,ts}")
            )
        );
    }

    /// Single-quoted relative globs are rewritten too (re-emitted in the
    /// canonical double-quote style).
    #[test]
    fn rebase_rewrites_single_quoted_relative_source() {
        let base = rebase_test_base();
        let out = rebase_relative_source_globs("@source 'pages/**/*.tsx';\n", &base);
        assert_eq!(
            out,
            format!("@source \"{}\";\n", rebased_value(&base, "pages/**/*.tsx"))
        );
    }

    /// The `@source not "<path>";` negation form keeps its `not` keyword
    /// across the rewrite.
    #[test]
    fn rebase_preserves_not_keyword() {
        let base = rebase_test_base();
        let out = rebase_relative_source_globs("@source not \"legacy/**\";\n", &base);
        assert_eq!(
            out,
            format!("@source not \"{}\";\n", rebased_value(&base, "legacy/**"))
        );
    }

    /// Absolute globs are already immune to the entry file's location —
    /// they must pass through byte-for-byte.
    #[test]
    fn rebase_leaves_absolute_source_untouched() {
        let line = if cfg!(windows) {
            "@source \"C:/already/absolute/pages/**\";\n"
        } else {
            "@source \"/already/absolute/pages/**\";\n"
        };
        let out = rebase_relative_source_globs(line, &rebase_test_base());
        assert_eq!(out, line);
    }

    /// `@source inline("...")` carries class names, not a path — it must
    /// never be rewritten (nor its `not` variant).
    #[test]
    fn rebase_leaves_inline_source_untouched() {
        let text = "@source inline(\"lg:flex\");\n@source not inline(\"lg:hidden\");\n";
        let out = rebase_relative_source_globs(text, Path::new("/project/root"));
        assert_eq!(out, text);
    }

    /// An empty `@source "";` is meaningless as authored; rebasing it would
    /// turn it into "scan the whole project root" — leave it alone.
    #[test]
    fn rebase_leaves_empty_value_untouched() {
        let line = "@source \"\";\n";
        let out = rebase_relative_source_globs(line, Path::new("/project/root"));
        assert_eq!(out, line);
    }

    /// A directive with no closing quote on the line is unparsable —
    /// pass it through unchanged rather than guessing.
    #[test]
    fn rebase_leaves_unclosed_quote_untouched() {
        let line = "@source \"src/components\n";
        let out = rebase_relative_source_globs(line, Path::new("/project/root"));
        assert_eq!(out, line);
    }

    /// Everything that is not an `@source` directive — imports, rules,
    /// `@theme` blocks — must survive byte-for-byte, including
    /// trailing-newline structure.
    #[test]
    fn rebase_leaves_non_source_lines_untouched() {
        let text = "@import \"tailwindcss\";\n@theme {\n  --color-x: #fff;\n}\nbody { margin: 0; }";
        let out = rebase_relative_source_globs(text, Path::new("/project/root"));
        assert_eq!(out, text);
    }

    /// Indentation before the directive and trailing content after the
    /// closing quote (the `;`, comments) are preserved verbatim.
    #[test]
    fn rebase_preserves_indent_and_suffix() {
        let base = rebase_test_base();
        let out = rebase_relative_source_globs("  @source \"pages/**\"; /* keep me */\n", &base);
        assert_eq!(
            out,
            format!(
                "  @source \"{}\"; /* keep me */\n",
                rebased_value(&base, "pages/**")
            )
        );
    }

    /// A `"` in the base path (legal on Linux/macOS) must be escaped in the
    /// rewritten directive so the emitted CSS string stays well-formed —
    /// same contract as [`push_escaped_source`]. The expected value is
    /// derived via [`rebased_value`] (join + escape), so on Unix this pins
    /// `/odd\"root/pages/**` exactly.
    #[test]
    fn rebase_escapes_quotes_in_base() {
        let base = PathBuf::from("/odd\"root");
        let out = rebase_relative_source_globs("@source \"pages/**\";\n", &base);
        assert_eq!(
            out,
            format!("@source \"{}\";\n", rebased_value(&base, "pages/**"))
        );
        #[cfg(unix)]
        assert_eq!(out, "@source \"/odd\\\"root/pages/**\";\n");
    }

    /// End-to-end through the synthesiser: relative `@source` globs inside
    /// the inlined user CSS come out absolute (anchored at `working_dir`),
    /// while the rest of the user's text is inlined verbatim (zfb#1327).
    #[test]
    fn synthesised_entry_rebases_relative_source_from_input_css() {
        let base = rebase_test_base();
        let cfg = TailwindSubprocessConfig::default().with_working_dir(base.clone());
        let input_css = "@source \"src/components/**/*.tsx\";\nbody { margin: 0; }\n";
        let css = build_synthesised_entry_css(&cfg, Some(input_css));
        let rebased = format!(
            "@source \"{}\";",
            rebased_value(&base, "src/components/**/*.tsx")
        );
        assert!(
            css.contains(&rebased),
            "relative @source from input CSS must be rebased onto working_dir; got:\n{css}"
        );
        assert!(
            !css.contains("@source \"src/components/**/*.tsx\";"),
            "the original relative directive must not survive; got:\n{css}"
        );
        assert!(
            css.contains("body { margin: 0; }"),
            "non-@source user CSS must be inlined verbatim; got:\n{css}"
        );
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

    // -----------------------------------------------------------------------
    // zfb#1534 — `codeHighlight.roleClasses` `@source inline(...)` safelist
    // -----------------------------------------------------------------------

    /// The synthesised entry CSS carries one `@source inline("...")`
    /// directive per configured `inline_sources` entry — this is the
    /// mechanism that safelists `codeHighlight.roleClasses` values
    /// (e.g. `text-violet-600`) that live in `zfb.config.ts` and are
    /// emitted only into rendered `dist/*.html`, neither of which
    /// Tailwind's `@source` glob scan ever sees.
    #[test]
    fn synthesised_entry_contains_inline_source_per_role_class() {
        let cfg = TailwindSubprocessConfig::default()
            .with_mock_output(String::new())
            .with_inline_sources(["text-violet-600", "dark:text-violet-400"]);
        let css = build_synthesised_entry_css(&cfg, None);
        assert!(
            css.contains("@source inline(\"text-violet-600\");"),
            "expected an inline source directive for text-violet-600; got:\n{css}"
        );
        assert!(
            css.contains("@source inline(\"dark:text-violet-400\");"),
            "expected an inline source directive for dark:text-violet-400; got:\n{css}"
        );
    }

    /// `inline_sources` values are escaped the same way path-based
    /// `@source` values are (only `"` and `\` — glob/selector
    /// metacharacters like `:` pass through untouched).
    #[test]
    fn synthesised_css_escapes_quotes_in_inline_sources() {
        let cfg = TailwindSubprocessConfig::default()
            .with_mock_output(String::new())
            .with_inline_sources([r#"text-"weird"-class"#]);
        let css = build_synthesised_entry_css(&cfg, None);
        assert!(
            css.contains(r#"@source inline("text-\"weird\"-class");"#),
            "inline_sources quote not escaped; got:\n{css}"
        );
    }

    /// Determinism: the same config produces byte-identical synthesised
    /// entry CSS across calls — the entry text feeds the CSS `hash_8`
    /// input (via the real Tailwind subprocess output), so unstable
    /// output here would produce a non-reproducible asset hash for an
    /// unchanged config.
    #[test]
    fn synthesised_entry_is_byte_identical_for_same_inline_sources() {
        let cfg = TailwindSubprocessConfig::default()
            .with_mock_output(String::new())
            .with_inline_sources(["dark:text-violet-400", "text-violet-600"]);
        let a = build_synthesised_entry_css(&cfg, None);
        let b = build_synthesised_entry_css(&cfg, None);
        assert_eq!(
            a, b,
            "same config must yield byte-identical synthesised entry CSS"
        );
    }

    /// A different `inline_sources` set changes the synthesised entry
    /// CSS. Real Tailwind is deterministic in its own scan (same input
    /// -> same output), so a changed entry is what drives a changed
    /// `hash_8` once the real subprocess runs (env-gated locally — see
    /// the tailwindcss-v4 env-gate tests in this crate's
    /// `tests/integration.rs`).
    #[test]
    fn synthesised_entry_changes_when_inline_sources_change() {
        let base = TailwindSubprocessConfig::default().with_mock_output(String::new());
        let a = build_synthesised_entry_css(
            &base.clone().with_inline_sources(["text-violet-600"]),
            None,
        );
        let b = build_synthesised_entry_css(&base.with_inline_sources(["text-blue-600"]), None);
        assert_ne!(
            a, b,
            "a different roleClasses-derived inline source set must change the synthesised entry"
        );
    }

    /// No `inline_sources` configured (the common case: no
    /// `codeHighlight.roleClasses`) must not emit a stray `@source
    /// inline(` directive or otherwise perturb the entry.
    #[test]
    fn synthesised_entry_omits_inline_directives_when_empty() {
        let cfg = TailwindSubprocessConfig::default().with_mock_output(String::new());
        let css = build_synthesised_entry_css(&cfg, None);
        assert!(
            !css.contains("@source inline("),
            "no inline source directive expected when inline_sources is empty; got:\n{css}"
        );
    }

    /// `inline_sources` directives coexist with `content_globs` and
    /// `framework_package_globs` directives — configuring a
    /// `codeHighlight.roleClasses` safelist must not crowd out or
    /// mangle the pre-existing path-based `@source` directives.
    #[test]
    fn synthesised_entry_inline_sources_coexist_with_content_globs() {
        let cfg = TailwindSubprocessConfig::default()
            .with_mock_output(String::new())
            .with_content_globs(["pages/**/*.tsx"])
            .with_framework_package_globs(["packages/zudo-doc-v2/**"])
            .with_inline_sources(["text-violet-600"]);
        let css = build_synthesised_entry_css(&cfg, None);
        assert!(
            css.contains("@source \"pages/**/*.tsx\";"),
            "content glob must still be emitted; got:\n{css}"
        );
        assert!(
            css.contains("@source \"packages/zudo-doc-v2/**\";"),
            "framework glob must still be emitted; got:\n{css}"
        );
        assert!(
            css.contains("@source inline(\"text-violet-600\");"),
            "inline source must be present verbatim; got:\n{css}"
        );
    }

    // -----------------------------------------------------------------------
    // Issue #1800 — negative-source (`@source not`) exclusion globs
    // -----------------------------------------------------------------------

    /// The synthesised entry CSS carries one `@source not "..."` directive
    /// per configured `negative_source_globs` entry.
    #[test]
    fn synthesised_entry_emits_negative_source_directive() {
        let cfg = TailwindSubprocessConfig::default()
            .with_mock_output(String::new())
            .with_negative_source_globs(["sibling/generated/**"]);
        let css = build_synthesised_entry_css(&cfg, None);
        assert!(
            css.contains("@source not \"sibling/generated/**\";"),
            "expected an @source not directive for sibling/generated/**; got:\n{css}"
        );
    }

    /// `negative_source_globs` values are escaped the same way path-based
    /// `@source` values are (only `"` and `\` — glob metacharacters pass
    /// through untouched) — mirrors `synthesised_css_escapes_quotes_in_globs`.
    #[test]
    fn synthesised_css_escapes_quotes_in_negative_source_globs() {
        let cfg = TailwindSubprocessConfig::default()
            .with_mock_output(String::new())
            .with_negative_source_globs([r#"sibling/"generated"/**"#]);
        let css = build_synthesised_entry_css(&cfg, None);
        assert!(
            css.contains(r#"@source not "sibling/\"generated\"/**";"#),
            "negative_source_globs quote not escaped; got:\n{css}"
        );
    }

    /// `negative_source_globs` directives coexist with `content_globs`,
    /// `framework_package_globs`, and `inline_sources` — and are emitted
    /// after all of them, per the directive order contract (the exclusion
    /// always has something preceding it to exclude from).
    #[test]
    fn synthesised_entry_negative_source_coexists_with_positive_globs() {
        let cfg = TailwindSubprocessConfig::default()
            .with_mock_output(String::new())
            .with_content_globs(["pages/**/*.tsx"])
            .with_framework_package_globs(["packages/zudo-doc-v2/**"])
            .with_inline_sources(["text-violet-600"])
            .with_negative_source_globs(["sibling/generated/**"]);
        let css = build_synthesised_entry_css(&cfg, None);
        let content_idx = css
            .find("@source \"pages/**/*.tsx\";")
            .expect("content glob must still be emitted");
        let framework_idx = css
            .find("@source \"packages/zudo-doc-v2/**\";")
            .expect("framework glob must still be emitted");
        let inline_idx = css
            .find("@source inline(\"text-violet-600\");")
            .expect("inline source must still be emitted");
        let negative_idx = css
            .find("@source not \"sibling/generated/**\";")
            .expect("negative source glob must be emitted");
        assert!(
            content_idx < framework_idx && framework_idx < inline_idx && inline_idx < negative_idx,
            "directive order (content -> framework -> inline -> negative) must be preserved; got:\n{css}"
        );
    }

    /// `negative_source_globs` (config-driven) and a user-authored
    /// `@source not "...";` line in `input_css` (parsed/rebased by
    /// [`rebase_relative_source_globs`], zfb#1327) must coexist — the new
    /// config-driven emission path must not interfere with the pre-existing
    /// user-CSS parsing path.
    #[test]
    fn synthesised_entry_negative_source_coexists_with_user_authored_source_not() {
        let base = rebase_test_base();
        let cfg = TailwindSubprocessConfig::default()
            .with_working_dir(base.clone())
            .with_mock_output(String::new())
            .with_negative_source_globs(["config-driven/exclude/**"]);
        let input_css = "@source not \"legacy/**\";\nbody { margin: 0; }\n";
        let css = build_synthesised_entry_css(&cfg, Some(input_css));
        assert!(
            css.contains("@source not \"config-driven/exclude/**\";"),
            "config-driven negative source glob must be present; got:\n{css}"
        );
        let rebased = format!("@source not \"{}\";", rebased_value(&base, "legacy/**"));
        assert!(
            css.contains(&rebased),
            "user-authored @source not must be rebased and preserved; got:\n{css}"
        );
        assert!(
            css.contains("body { margin: 0; }"),
            "non-@source user CSS must still be inlined verbatim; got:\n{css}"
        );
    }

    /// No `negative_source_globs` configured (the common case) must not
    /// emit a stray `@source not` directive or otherwise perturb the entry.
    #[test]
    fn synthesised_entry_omits_negative_source_directive_when_empty() {
        let cfg = TailwindSubprocessConfig::default().with_mock_output(String::new());
        let css = build_synthesised_entry_css(&cfg, None);
        assert!(
            !css.contains("@source not "),
            "no negative source directive expected when negative_source_globs is empty; got:\n{css}"
        );
    }

    /// Empty-field parity: this is the test that matters most. The
    /// synthesised entry text feeds the CSS `hash_8` input, so an unset
    /// `negative_source_globs` (every non-workspace project today) must
    /// produce byte-identical output to what the pre-#1800 code emitted —
    /// not merely "no `@source not` substring", but zero stray bytes of
    /// any kind (no extra blank line, no extra whitespace). Asserted via
    /// an exact `assert_eq!` against a hand-built expected string, not a
    /// "looks the same" `contains` check.
    #[test]
    fn synthesised_entry_empty_negative_source_globs_is_byte_identical_to_pre_feature_output() {
        let cfg = TailwindSubprocessConfig::default()
            .with_mock_output(String::new())
            .with_content_globs(["pages/**/*.tsx"])
            .with_framework_package_globs(["packages/zudo-doc-v2/**"])
            .with_inline_sources(["text-violet-600"]);
        let css = build_synthesised_entry_css(&cfg, Some("body { margin: 0; }\n"));
        let expected = concat!(
            "@import \"tailwindcss\";\n",
            "@source \"pages/**/*.tsx\";\n",
            "@source \"packages/zudo-doc-v2/**\";\n",
            "@source inline(\"text-violet-600\");\n",
            "\n",
            "body { margin: 0; }\n",
        );
        assert_eq!(
            css, expected,
            "empty negative_source_globs must not add or remove a single byte from the pre-#1800 output; got:\n{css}"
        );
    }

    /// Same parity guarantee for the all-defaults case (no globs, no
    /// input CSS at all) — the bare synthesised entry must stay exactly
    /// `@import "tailwindcss";` plus the trailing blank-line separator.
    #[test]
    fn synthesised_entry_all_defaults_empty_negative_source_globs_is_byte_identical() {
        let cfg = TailwindSubprocessConfig::default().with_mock_output(String::new());
        let css = build_synthesised_entry_css(&cfg, None);
        assert_eq!(css, "@import \"tailwindcss\";\n\n");
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
    // zfb#1300 — entry dir must follow input_css so relative sibling
    // `@import`s in the user's CSS resolve correctly
    // -----------------------------------------------------------------------

    /// When `input_css` is set, the synthesised entry must be created in
    /// its parent directory — not `working_dir` — so an inlined relative
    /// sibling `@import "./tokens.css";` resolves against the entry's own
    /// location (zfb#1300).
    #[test]
    fn entry_dir_follows_input_css_parent_when_set() {
        let cfg = TailwindSubprocessConfig::default()
            .with_working_dir("/project/root")
            .with_input_css("/project/root/styles/global.css");
        assert_eq!(entry_dir(&cfg), PathBuf::from("/project/root/styles"));
    }

    /// When `input_css` is `None` (fully synthesised entry, nothing to
    /// inline), there is no relative-import scope to preserve, so the entry
    /// falls back to `working_dir`.
    #[test]
    fn entry_dir_falls_back_to_working_dir_when_input_css_is_none() {
        let cfg = TailwindSubprocessConfig::default().with_working_dir("/project/root");
        assert_eq!(entry_dir(&cfg), PathBuf::from("/project/root"));
    }

    /// A bare filename `input_css` (no directory component) has an empty
    /// `parent()` — fall back to `working_dir` rather than resolving
    /// against an empty path.
    #[test]
    fn entry_dir_falls_back_to_working_dir_when_input_css_has_no_parent() {
        let cfg = TailwindSubprocessConfig::default()
            .with_working_dir("/project/root")
            .with_input_css("global.css");
        assert_eq!(entry_dir(&cfg), PathBuf::from("/project/root"));
    }

    /// Defensive case: production always passes an absolute `input_css`
    /// (built by joining onto an absolute `project_root`), but a
    /// *relative* `input_css` with a relative parent must still resolve
    /// against `working_dir` — not the process's own cwd — so the temp
    /// file lands next to `input_css` regardless of how the caller built
    /// the path.
    #[test]
    fn entry_dir_resolves_relative_input_css_parent_against_working_dir() {
        let cfg = TailwindSubprocessConfig::default()
            .with_working_dir("/project/root")
            .with_input_css("styles/global.css");
        assert_eq!(entry_dir(&cfg), PathBuf::from("/project/root/styles"));
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
