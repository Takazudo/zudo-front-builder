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

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{RenderError, Result};
use crate::swc_pipeline::{CompileOptions, CompiledModule, JsxRuntime, SwcPipeline};

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
    pub fn resolve_relative(&self, importer_dir: &Path, specifier: &str) -> Result<PathBuf> {
        let candidate = importer_dir.join(specifier);

        // 1) Exact match.
        if candidate.is_file() {
            return Ok(candidate);
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
            if probe.is_file() {
                return Ok(probe);
            }
        }

        // 3) `index.<ext>` inside a directory.
        if candidate.is_dir() {
            for ext in &self.probe_exts {
                let probe = candidate.join(format!("index{ext}"));
                if probe.is_file() {
                    return Ok(probe);
                }
            }
        }

        Err(RenderError::resolve(
            specifier,
            importer_dir.display().to_string(),
        ))
    }

    /// Compile a source string through SWC and cache it under `specifier`.
    /// Returns the cached entry on subsequent calls.
    pub fn load_source(&mut self, specifier: &str, source: &str) -> Result<&CompiledModule> {
        if !self.cache.contains_key(specifier) {
            let opts = CompileOptions::default()
                .with_filename(specifier.to_string())
                .with_jsx_runtime(self.jsx_runtime);
            let compiled = self.pipeline.compile(source, &opts)?;
            self.cache.insert(specifier.to_string(), compiled);
        }
        Ok(self.cache.get(specifier).expect("just inserted above"))
    }

    /// Read `path` and load+compile it. Caches by absolute-path key.
    pub fn load_file(&mut self, path: &Path) -> Result<&CompiledModule> {
        let key = path.to_string_lossy().to_string();
        if !self.cache.contains_key(&key) {
            let source = std::fs::read_to_string(path)?;
            let opts = CompileOptions::default()
                .with_filename(key.clone())
                .with_jsx_runtime(self.jsx_runtime);
            let compiled = self.pipeline.compile(&source, &opts)?;
            self.cache.insert(key.clone(), compiled);
        }
        Ok(self.cache.get(&key).expect("just inserted above"))
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
