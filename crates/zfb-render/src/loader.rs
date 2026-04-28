//! Module loader: resolves imports, compiles each via SWC, caches the result.
//!
//! Two kinds of specifiers:
//!
//! 1. **Relative paths** (`./layout`, `../layouts/blog`) — resolved against
//!    the importer's directory using the file system. We probe a small set of
//!    extensions (`.tsx`, `.ts`, `.jsx`, `.js`) before giving up.
//! 2. **Bare specifiers** (`preact`, `react`, `zfb`) — resolved by the
//!    framework adapter (Sub 4) at runtime; this loader treats them as
//!    runtime-provided and does not attempt to read a file from disk.
//!
//! `oxc_resolver` will eventually replace the hand-rolled resolver — this
//! minimal version exists so the orchestrator and tests can wire up without
//! pulling in the full resolver crate while the team is parallel-working.
//!
//! ## MDX specifiers
//!
//! Two specifier shapes route a source string through the MDX→JSX emitter
//! before the SWC TSX pass:
//!
//! 1. Filename ends in `.mdx` (typical on-disk content collection entry).
//! 2. Specifier starts with `mdx://` (the canonical form Sub 4 produces
//!    for compiled collection entries — see
//!    `zfb_content::mdx_jsx_emit::compile_mdx_to_jsx_module`).
//!
//! In both cases the loader calls `zfb_content::mdx_jsx_emit::mdx_to_jsx_module`
//! first, then hands the resulting JSX source through the existing SWC
//! pipeline. Compile errors from `zfb-content` are wrapped as
//! [`RenderError::Compile`] carrying the original specifier as the file
//! location so the rest of the renderer's error UX still applies.
//!
//! ## JS-side bridge contract — `globalThis.__zfb.content`
//!
//! Before evaluating each page module, the renderer must install a
//! namespaced bridge so the JS-side `zfb/content` `CollectionEntry.Content`
//! field can resolve compiled entries:
//!
//! ```text
//! globalThis.__zfb.content.get(specifier)
//!     → ((props: { components?: Record<string, unknown> }) => unknown) | undefined
//! ```
//!
//! The bridge is keyed on `Entry::module_specifier` (see
//! `zfb_content::collection`). The JS stub may pass either the full
//! `mdx://<collection>/<slug>#<hash>` form or the hash-less
//! `mdx://<collection>/<slug>` form (the JS stub has no hash to compute);
//! the resolver must accept both. When `get` returns `None`, JS falls back
//! to a marked `<pre data-zfb-content-fallback>` block.
//!
//! Cross-referenced from `packages/zfb/src/content.ts` (JSDoc on
//! `CollectionEntry.Content`) and `packages/zfb/CONTRIBUTING.md`. Keep
//! the two halves in sync.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use zfb_content::mdx_jsx_emit::{mdx_to_jsx_module, MdxJsxOptions};

use crate::error::{RenderError, Result};

// ---------------------------------------------------------------------------
// ResolverError — typed error for file-read operations
// ---------------------------------------------------------------------------

/// Errors that can occur when the loader tries to read a source file from
/// disk, distinct from compile-time or runtime failures.
///
/// Callers that match on [`RenderError::Io`] lose the distinction between
/// "the file simply wasn't there" (`NotFound`) and "the file existed but
/// couldn't be decoded" (`BadEncoding`). `ResolverError` preserves that
/// distinction so consumers can give actionable diagnostics.
///
/// `ResolverError` is automatically converted to [`RenderError`] by
/// [`ModuleLoader::load_file`]; direct callers only need this type when
/// they want to distinguish the variants before the error bubbles up.
#[derive(Debug, thiserror::Error)]
pub enum ResolverError {
    /// The requested path does not exist on disk.
    #[error("file not found: {path}")]
    NotFound {
        /// The path that was not found.
        path: PathBuf,
    },
    /// An I/O error other than ENOENT occurred while reading the file.
    #[error("i/o error reading {path}: {source}")]
    Io {
        /// The path being read when the error occurred.
        path: PathBuf,
        /// The underlying OS error.
        source: std::io::Error,
    },
    /// The file content is not valid UTF-8.
    #[error("bad encoding in {path}: file is not valid UTF-8")]
    BadEncoding {
        /// The path of the file with the encoding problem.
        path: PathBuf,
    },
}

impl From<ResolverError> for RenderError {
    fn from(e: ResolverError) -> Self {
        match e {
            ResolverError::NotFound { path } => {
                RenderError::Resolve {
                    specifier: path.to_string_lossy().into_owned(),
                    importer: String::new(),
                    line: None,
                    col: None,
                    tried: vec![path.to_string_lossy().into_owned()],
                }
            }
            ResolverError::Io { path, source } => RenderError::Other(format!(
                "i/o error reading {}: {source}",
                path.display()
            )),
            ResolverError::BadEncoding { path } => RenderError::Other(format!(
                "file is not valid UTF-8: {}",
                path.display()
            )),
        }
    }
}

/// Read `path` to a `String`, returning a typed [`ResolverError`] that
/// distinguishes `NotFound`, generic I/O failures, and encoding problems.
///
/// This is the single place all file reads go through so the rest of the
/// loader can pattern-match on the error kind when it matters.
pub fn read_to_string(path: &Path) -> std::result::Result<String, ResolverError> {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| ResolverError::BadEncoding {
            path: path.to_owned(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(ResolverError::NotFound { path: path.to_owned() })
        }
        Err(e) => Err(ResolverError::Io {
            path: path.to_owned(),
            source: e,
        }),
    }
}

use crate::swc_pipeline::{CompileOptions, CompiledModule, JsxRuntime, SwcPipeline};

/// True when the loader should treat `specifier` as MDX source.
///
/// Recognised shapes:
/// - ends in `.mdx` (case-sensitive — matches Astro / mdx-js conventions).
/// - starts with `mdx://` (the URL form produced by
///   `zfb_content::mdx_jsx_emit::compile_mdx_to_jsx_module`).
fn is_mdx_specifier(specifier: &str) -> bool {
    specifier.ends_with(".mdx") || specifier.starts_with("mdx://")
}

/// Cache key for compiled modules.
type Specifier = String;

/// In-memory module loader + compile cache.
pub struct ModuleLoader {
    pipeline: SwcPipeline,
    /// Default JSX runtime (driven by the configured framework adapter).
    jsx_runtime: JsxRuntime,
    /// Compile cache keyed by absolute / canonical path string.
    cache: HashMap<Specifier, CompiledModule>,
    /// File extensions probed for relative imports without an extension.
    probe_exts: Vec<String>,
}

impl ModuleLoader {
    /// Build a fresh loader with empty cache and the given default runtime.
    pub fn new(jsx_runtime: JsxRuntime) -> Self {
        Self {
            pipeline: SwcPipeline::new(),
            jsx_runtime,
            cache: HashMap::new(),
            probe_exts: vec![
                ".tsx".to_string(),
                ".ts".to_string(),
                ".jsx".to_string(),
                ".js".to_string(),
            ],
        }
    }

    /// Whether the given specifier is a bare specifier (no `./` / `../` / `/`
    /// prefix). Bare specifiers are runtime-provided and not resolved here.
    pub fn is_bare(specifier: &str) -> bool {
        !(specifier.starts_with("./") || specifier.starts_with("../") || specifier.starts_with('/'))
    }

    /// Resolve a relative `specifier` against `importer_dir`, probing
    /// extensions and returning the resolved absolute path on success.
    ///
    /// On failure the returned [`RenderError::Resolve`] points at
    /// `importer_dir` (no source line/column). Prefer
    /// [`Self::resolve_relative_from_file`] when the importer's file path is
    /// known so the error message points editors at the actual TSX file.
    pub fn resolve_relative(&self, importer_dir: &Path, specifier: &str) -> Result<PathBuf> {
        let (result, tried) = self.try_resolve(importer_dir, specifier);
        result.ok_or_else(|| {
            RenderError::resolve_with(
                specifier,
                importer_dir.display().to_string(),
                None,
                None,
                tried,
            )
        })
    }

    /// Resolve a relative `specifier` imported from `importer_file`. On
    /// failure the error includes the importer's file path (and optional
    /// line/col of the `import` statement) plus every candidate path probed.
    pub fn resolve_relative_from_file(
        &self,
        importer_file: &Path,
        specifier: &str,
        import_line: Option<u32>,
        import_col: Option<u32>,
    ) -> Result<PathBuf> {
        let importer_dir = importer_file.parent().unwrap_or(Path::new(""));
        let (result, tried) = self.try_resolve(importer_dir, specifier);
        result.ok_or_else(|| {
            RenderError::resolve_with(
                specifier,
                importer_file.display().to_string(),
                import_line,
                import_col,
                tried,
            )
        })
    }

    /// Internal probe loop shared by [`resolve_relative`] and
    /// [`resolve_relative_from_file`]. Returns the resolved path (if any) and
    /// the full list of candidates attempted, in probe order.
    fn try_resolve(&self, importer_dir: &Path, specifier: &str) -> (Option<PathBuf>, Vec<String>) {
        let candidate = importer_dir.join(specifier);
        let mut tried: Vec<String> =
            Vec::with_capacity(self.probe_exts.len() + 1 + self.probe_exts.len());

        // 1) Exact match.
        tried.push(candidate.display().to_string());
        if candidate.is_file() {
            return (Some(candidate), tried);
        }

        // 2) Probe extensions (`./layout` → `./layout.tsx` etc.).
        for ext in &self.probe_exts {
            let mut probe = candidate.clone();
            let new_name = format!(
                "{}{}",
                probe.file_name().and_then(|s| s.to_str()).unwrap_or(""),
                ext
            );
            probe.set_file_name(new_name);
            tried.push(probe.display().to_string());
            if probe.is_file() {
                return (Some(probe), tried);
            }
        }

        // 3) `index.<ext>` inside a directory.
        if candidate.is_dir() {
            for ext in &self.probe_exts {
                let probe = candidate.join(format!("index{ext}"));
                tried.push(probe.display().to_string());
                if probe.is_file() {
                    return (Some(probe), tried);
                }
            }
        }

        (None, tried)
    }

    /// Compile a source string through SWC and cache it under `specifier`.
    /// Returns the cached entry on subsequent calls.
    ///
    /// If `specifier` is an MDX specifier (ends in `.mdx` or starts with
    /// `mdx://`), `source` is first run through
    /// `zfb_content::mdx_jsx_emit::mdx_to_jsx_module` and the resulting JSX
    /// text is what gets fed into SWC.
    pub fn load_source(&mut self, specifier: &str, source: &str) -> Result<&CompiledModule> {
        if !self.cache.contains_key(specifier) {
            let jsx_source = self.maybe_mdx_to_jsx(specifier, source)?;
            let opts = CompileOptions::default()
                .with_filename(specifier.to_string())
                .with_jsx_runtime(self.jsx_runtime);
            let compiled = self.pipeline.compile(&jsx_source, &opts)?;
            self.cache.insert(specifier.to_string(), compiled);
        }
        Ok(self.cache.get(specifier).expect("just inserted above"))
    }

    /// Read `path` and load+compile it. Caches by absolute-path key.
    ///
    /// Returns a typed [`ResolverError`] (converted to [`RenderError`]) that
    /// distinguishes `NotFound`, generic I/O failures, and encoding problems.
    ///
    /// `.mdx` files are routed through the MDX→JSX emitter before SWC; see
    /// [`Self::load_source`].
    pub fn load_file(&mut self, path: &Path) -> Result<&CompiledModule> {
        let key = path.to_string_lossy().to_string();
        if !self.cache.contains_key(&key) {
            let source = read_to_string(path).map_err(RenderError::from)?;
            let jsx_source = self.maybe_mdx_to_jsx(&key, &source)?;
            let opts = CompileOptions::default()
                .with_filename(key.clone())
                .with_jsx_runtime(self.jsx_runtime);
            let compiled = self.pipeline.compile(&jsx_source, &opts)?;
            self.cache.insert(key.clone(), compiled);
        }
        Ok(self.cache.get(&key).expect("just inserted above"))
    }

    /// If `specifier` looks like MDX, run the source through the MDX→JSX
    /// emitter; otherwise return `source` unchanged.
    ///
    /// Errors from `zfb-content` are wrapped as [`RenderError::Compile`]
    /// with `specifier` as the file location so the renderer's existing
    /// error formatting points editors at the original `.mdx` (or `mdx://`)
    /// source.
    fn maybe_mdx_to_jsx(&self, specifier: &str, source: &str) -> Result<String> {
        if !is_mdx_specifier(specifier) {
            return Ok(source.to_string());
        }
        let opts = MdxJsxOptions::default().with_filename(specifier.to_string());
        mdx_to_jsx_module(source, opts)
            .map_err(|e| RenderError::compile(specifier.to_string(), e.to_string()))
    }

    /// Number of cache entries (handy for assertions).
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_bare_specifiers() {
        assert!(ModuleLoader::is_bare("preact"));
        assert!(ModuleLoader::is_bare("react"));
        assert!(ModuleLoader::is_bare("zfb"));
        assert!(!ModuleLoader::is_bare("./layout"));
        assert!(!ModuleLoader::is_bare("../layouts/blog"));
        assert!(!ModuleLoader::is_bare("/abs/path"));
    }

    #[test]
    fn detects_mdx_specifiers() {
        assert!(is_mdx_specifier("post.mdx"));
        assert!(is_mdx_specifier("./posts/hello.mdx"));
        assert!(is_mdx_specifier("mdx://blog/hello#abc12345"));
        assert!(!is_mdx_specifier("page.tsx"));
        assert!(!is_mdx_specifier("./layout"));
        assert!(!is_mdx_specifier("preact"));
        // case-sensitive on the extension, by design
        assert!(!is_mdx_specifier("post.MDX"));
    }

    #[test]
    fn caches_compiled_sources() {
        let mut loader = ModuleLoader::new(JsxRuntime::Preact);
        let _ = loader
            .load_source("page.tsx", "export const x: number = 1;")
            .expect("compile ok");
        let _ = loader
            .load_source("page.tsx", "/* different source — should hit cache */")
            .expect("cache hit");
        assert_eq!(loader.cache_len(), 1);
    }
}
