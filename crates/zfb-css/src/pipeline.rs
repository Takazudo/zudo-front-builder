//! Hashing, asset emission, and the top-level [`CssPipeline`].
//!
//! Stage 3 of the CSS pipeline:
//!
//! 1. Engine output (Tailwind utilities) and CSS Modules output are
//!    concatenated, in that order, separated by `\n`.
//! 2. The combined bytes are hashed with SHA-256, and the first 8 hex
//!    characters become the asset filename suffix (`styles-{hash}.css`).
//! 3. The file is written under
//!    `{output_root}/assets/styles-{hash}.css`. Default `output_root` is
//!    `dist/`.
//!
//! [`link_href`] derives the URL for the renderer to inject as
//! `<link href="...">`. We do **not** inject HTML here — that's the
//! renderer's responsibility (Epic 3 / Epic 7 wiring).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::engine::CssEngine;
use crate::modules::{CssModulesConfig, CssModulesProcessor};
use crate::scanner::{scan_css_module_imports, ModuleImportScan, SourceModuleUsage};

/// Configuration for [`CssPipeline::new`].
#[derive(Debug, Clone)]
pub struct CssPipelineConfig {
    /// Source files to scan for utility classes (passed to the engine).
    pub sources: Vec<PathBuf>,

    /// CSS Modules files to compile. Order is significant: it determines
    /// the order of concatenation in the global asset and therefore the
    /// hash.
    pub css_modules: Vec<PathBuf>,

    /// Where to write the global asset. The pipeline emits
    /// `{output_root}/assets/styles-{hash}.css` under this directory.
    /// Default: `dist/`.
    pub output_root: PathBuf,

    /// Public base URL prefix for the asset, e.g. `"/"` (default) or
    /// `"https://cdn.example.com/"`. Used by [`link_href`].
    pub base_url: String,

    /// CSS Modules processing config.
    pub modules_config: CssModulesConfig,

    /// When true, the pipeline scans every entry in
    /// [`Self::sources`] for `import * from "*.module.css"` statements
    /// and adds the discovered files to the CSS Modules input set.
    /// Discovered files are appended *after* the explicit
    /// [`Self::css_modules`] list so callers can pin a leading order
    /// that wins the hash.
    ///
    /// Default: `true`. Set to `false` when the caller has its own
    /// (e.g. SWC-based) module graph and wants
    /// [`Self::css_modules`] treated as the complete set.
    pub auto_discover_modules: bool,

    /// Where to write the per-module JSON class-name maps (the JS-side
    /// rewrite contract — see [`crate::lib`] docs). Each `.module.css`
    /// file ends up at
    /// `{class_map_dir}/<rel-from-output_root>.classes.json` if the
    /// module path is descendant of `output_root`'s parent, or at
    /// `{class_map_dir}/<sha8(path)>.classes.json` otherwise. Default:
    /// `{output_root}/css-modules`.
    ///
    /// When `None`, no JSON is written; the in-memory `class_maps` is
    /// still returned in [`CssPipelineOutput`].
    pub class_map_dir: Option<PathBuf>,
}

impl Default for CssPipelineConfig {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            css_modules: Vec::new(),
            output_root: PathBuf::from("dist"),
            base_url: "/".to_string(),
            modules_config: CssModulesConfig::default(),
            auto_discover_modules: true,
            class_map_dir: None,
        }
    }
}

/// Result of running [`CssPipeline::build`].
#[derive(Debug, Clone)]
pub struct CssPipelineOutput {
    /// 8-character hex hash (first 8 chars of SHA-256 of the combined CSS).
    pub hash: String,

    /// The full combined CSS string.
    pub css: String,

    /// Absolute or output-root-relative path the asset was written to.
    /// Convention: `{output_root}/assets/styles-{hash}.css`.
    pub asset_path: PathBuf,

    /// Per-file CSS Modules class-name maps. See
    /// [`crate::modules::CssModulesOutput::class_maps`].
    pub class_maps: HashMap<PathBuf, HashMap<String, String>>,

    /// Per-source-file CSS Modules usage. Empty when
    /// [`CssPipelineConfig::auto_discover_modules`] is false. The
    /// dev-asset-graph crate (`zfb-graph`'s SSE topic) consumes this
    /// to answer "which pages depend on which CSS modules" without
    /// needing to re-scan the sources itself.
    pub per_source_modules: Vec<SourceModuleUsage>,

    /// JSON class-map files emitted to disk, in the order they were
    /// written. Empty when
    /// [`CssPipelineConfig::class_map_dir`] is `None`. Each entry maps
    /// the original `.module.css` path to the `.classes.json` file
    /// containing `{ "originalClass": "scopedClass", ... }`.
    pub class_map_files: HashMap<PathBuf, PathBuf>,
}

/// Top-level CSS pipeline.
///
/// Generic over the engine type so callers can swap [`crate::TailwindSubprocessEngine`]
/// for [`crate::NativeRustEngine`] (or a test double) without touching the
/// pipeline code.
pub struct CssPipeline<E: CssEngine> {
    engine: E,
    modules: CssModulesProcessor,
    config: CssPipelineConfig,
}

impl<E: CssEngine> CssPipeline<E> {
    /// Construct a new pipeline.
    pub fn new(engine: E, config: CssPipelineConfig) -> Self {
        let modules = CssModulesProcessor::new(config.modules_config.clone());
        Self {
            engine,
            modules,
            config,
        }
    }

    /// Run all stages: engine → CSS Modules → hash → write.
    pub fn build(&self) -> Result<CssPipelineOutput> {
        // Stage 0: optionally discover CSS Modules from the source
        // files. Discovered files are appended after the explicit list,
        // de-duplicated.
        let (module_files, per_source_modules) = self.collect_modules()?;

        let tailwind = self
            .engine
            .produce_utility_css(&self.config.sources)
            .context("CSS engine stage failed")?;

        let modules = self
            .modules
            .process(&module_files)
            .context("CSS Modules stage failed")?;

        let combined = combine(&tailwind, &modules.css);
        let hash = hash_8(&tailwind, &modules.css);
        let asset_path = self
            .config
            .output_root
            .join("assets")
            .join(format!("styles-{hash}.css"));

        atomic_write(&asset_path, combined.as_bytes())
            .with_context(|| format!("failed to write {}", asset_path.display()))?;

        // Emit JSON class-name maps if requested.
        let class_map_files = if let Some(dir) = &self.config.class_map_dir {
            write_class_map_files(dir, &modules.class_maps)?
        } else {
            HashMap::new()
        };

        Ok(CssPipelineOutput {
            hash,
            css: combined,
            asset_path,
            class_maps: modules.class_maps,
            per_source_modules,
            class_map_files,
        })
    }

    fn collect_modules(&self) -> Result<(Vec<PathBuf>, Vec<SourceModuleUsage>)> {
        let mut files: Vec<PathBuf> = self.config.css_modules.clone();
        let mut seen: std::collections::HashSet<PathBuf> = files.iter().cloned().collect();

        let per_source = if self.config.auto_discover_modules
            && !self.config.sources.is_empty()
        {
            let scan: ModuleImportScan = scan_css_module_imports(&self.config.sources)
                .context("CSS Modules import scan failed")?;
            for m in &scan.modules {
                // Auto-discover only resolved paths that exist on
                // disk; bare specifiers (e.g. `@org/pkg/...`) are
                // recorded in per_source but not fed to the
                // processor, since lightningcss needs a real file.
                if seen.insert(m.clone()) && m.exists() {
                    files.push(m.clone());
                }
            }
            scan.per_source
        } else {
            Vec::new()
        };

        Ok((files, per_source))
    }

    /// Borrow the underlying config.
    pub fn config(&self) -> &CssPipelineConfig {
        &self.config
    }

    /// Borrow the underlying engine. Useful for inspection in tests
    /// (e.g. fetching [`crate::TailwindSubprocessEngine::last_entry_css`]).
    pub fn engine_ref(&self) -> &E {
        &self.engine
    }
}

/// Emit the JSON class-name map file for each `.module.css` file under
/// `dir`. Returns the on-disk path for each map.
///
/// File layout:
/// `{dir}/<sha8(module_path)>__<basename>.classes.json`. The hash prefix
/// guarantees uniqueness across the whole module set even when two
/// modules share a basename, while keeping the basename visible for
/// debugging.
fn write_class_map_files(
    dir: &Path,
    class_maps: &HashMap<PathBuf, HashMap<String, String>>,
) -> Result<HashMap<PathBuf, PathBuf>> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("failed to create class-map dir {}", dir.display()))?;
    let mut out: HashMap<PathBuf, PathBuf> = HashMap::new();
    for (module_path, names) in class_maps {
        let mut hasher = Sha256::new();
        hasher.update(module_path.to_string_lossy().as_bytes());
        let h = hex::encode(hasher.finalize());
        let prefix: &str = &h[..8];
        let basename = module_path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "anon".to_string());
        let json_path = dir.join(format!("{prefix}__{basename}.classes.json"));

        let mut sorted: Vec<(&String, &String)> = names.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        let json = serde_json::to_string_pretty(
            &sorted
                .into_iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<std::collections::BTreeMap<_, _>>(),
        )
        .context("failed to serialise CSS Modules class-name map")?;
        atomic_write(&json_path, json.as_bytes())
            .with_context(|| format!("failed to write {}", json_path.display()))?;
        out.insert(module_path.clone(), json_path);
    }
    Ok(out)
}

/// Combine engine output + CSS Modules output exactly as the hashing stage
/// will see it. Exposed as a free function so tests and the hashing helper
/// agree on the canonical form.
pub(crate) fn combine(tailwind: &str, modules: &str) -> String {
    let mut out = String::with_capacity(tailwind.len() + modules.len() + 1);
    out.push_str(tailwind);
    out.push('\n');
    out.push_str(modules);
    out
}

/// Compute the 8-char hex hash for the given (tailwind, modules) pair.
///
/// The hash is the first 8 characters of `sha256(tailwind + "\n" + modules)`
/// in lowercase hex. Using a fixed separator means appending a class to one
/// half is distinguishable from prepending it to the other.
pub fn hash_8(tailwind: &str, modules: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(tailwind.as_bytes());
    hasher.update(b"\n");
    hasher.update(modules.as_bytes());
    let digest = hasher.finalize();
    let full = hex::encode(digest);
    full[..8].to_string()
}

/// Build the public URL for the global CSS asset.
///
/// `base_url` is concatenated with the asset path's filename portion under
/// an `assets/` segment. `base_url` is normalised to end with `/`.
///
/// Examples:
/// ```
/// use std::path::PathBuf;
/// use zfb_css::link_href;
///
/// let p = PathBuf::from("dist/assets/styles-abc12345.css");
/// assert_eq!(link_href("/", &p), "/assets/styles-abc12345.css");
/// assert_eq!(
///     link_href("https://cdn.example.com", &p),
///     "https://cdn.example.com/assets/styles-abc12345.css",
/// );
/// ```
pub fn link_href(base_url: &str, asset_path: &Path) -> String {
    let filename = asset_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let trimmed = base_url.trim_end_matches('/');
    format!("{trimmed}/assets/{filename}")
}

/// Atomic write helper local to this crate. Mirrors the
/// `write-temp-then-rename` recipe used by `zfb-build::atomic` so the
/// CSS pipeline never leaves a half-written `.css` (or class-map JSON)
/// behind.
fn atomic_write(dest: &Path, bytes: &[u8]) -> Result<()> {
    static SEQ: AtomicU64 = AtomicU64::new(0);

    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
    }

    let pid = std::process::id();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut name = dest
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("zfb-css"));
    name.push(format!(".tmp-{pid}-{seq}"));
    let mut temp_path = dest.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    if temp_path.as_os_str().is_empty() {
        temp_path = PathBuf::from(".");
    }
    temp_path.push(name);

    {
        let mut f = std::fs::File::create(&temp_path)
            .with_context(|| format!("failed to create temp file {}", temp_path.display()))?;
        f.write_all(bytes)
            .with_context(|| format!("failed to write temp file {}", temp_path.display()))?;
        f.sync_all()
            .with_context(|| format!("failed to fsync temp file {}", temp_path.display()))?;
    }

    if let Err(e) = std::fs::rename(&temp_path, dest) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(e).with_context(|| {
            format!(
                "failed to rename {} -> {}",
                temp_path.display(),
                dest.display()
            )
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_for_identical_inputs() {
        let a = hash_8(".a{color:red}", ".b{color:blue}");
        let b = hash_8(".a{color:red}", ".b{color:blue}");
        assert_eq!(a, b);
        assert_eq!(a.len(), 8);
    }

    #[test]
    fn hash_changes_when_a_class_changes() {
        let before = hash_8(".a{color:red}", ".b{color:blue}");
        let after = hash_8(".a{color:green}", ".b{color:blue}");
        assert_ne!(
            before, after,
            "changing a class in the tailwind half must change the hash"
        );

        let after2 = hash_8(".a{color:red}", ".b{color:teal}");
        assert_ne!(
            before, after2,
            "changing a class in the modules half must change the hash"
        );
    }

    #[test]
    fn hash_uses_separator_to_avoid_boundary_collisions() {
        // Without a separator, ("ab", "cd") and ("a", "bcd") would hash the
        // same. With our "\n" separator they must differ.
        let a = hash_8("ab", "cd");
        let b = hash_8("a", "bcd");
        assert_ne!(a, b);
    }

    #[test]
    fn link_href_normalises_trailing_slash() {
        let p = PathBuf::from("dist/assets/styles-deadbeef.css");
        assert_eq!(link_href("/", &p), "/assets/styles-deadbeef.css");
        assert_eq!(link_href("", &p), "/assets/styles-deadbeef.css");
        assert_eq!(
            link_href("https://cdn.example.com/", &p),
            "https://cdn.example.com/assets/styles-deadbeef.css"
        );
        assert_eq!(
            link_href("https://cdn.example.com", &p),
            "https://cdn.example.com/assets/styles-deadbeef.css"
        );
    }
}
