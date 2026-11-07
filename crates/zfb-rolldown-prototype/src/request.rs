//! Typed, engine-neutral bundle requests and results (#4629).
//!
//! These are zfb's own types. No upstream Rolldown type appears here, so the
//! engine can move without changing callers. Every option a caller used to
//! spell as esbuild argv is an explicit field.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One bundle job: a single entry, its output layout and the policies the
/// caller owns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleRequest {
    /// Working directory in the caller's spelling. Relative paths in this
    /// request resolve against it, and graph keys are spelled relative to it.
    pub cwd: PathBuf,
    /// The entry module, relative to `cwd` or absolute.
    pub entry: PathBuf,
    pub output: OutputLayout,
    pub format: Format,
    pub platform: Platform,
    /// Syntax target, e.g. `es2020`, `esnext`, `node22`.
    pub target: Option<String>,
    pub jsx: Jsx,
    /// Explicit tsconfig, relative to `cwd` or absolute. Its JSX fields apply
    /// to every module, `node_modules` included, unless `jsx` already sets them.
    pub tsconfig: Option<PathBuf>,
    pub resolve: Resolve,
    /// Global identifier replacements, in declaration order. Values are JS
    /// expressions (a JSON string literal for string values).
    pub defines: Vec<(String, String)>,
    /// Loader overrides keyed by file extension including the dot (`.svg`).
    pub loaders: Vec<(String, Loader)>,
    pub externals: Vec<External>,
    /// Code splitting for dynamic imports. Off produces exactly one chunk.
    pub splitting: bool,
    pub minify: bool,
    pub keep_names: bool,
    pub source_map: SourceMap,
    /// Where to write the normalized module graph (metafile-shaped JSON),
    /// relative to `cwd` or absolute. `None` still validates the graph.
    pub graph: Option<PathBuf>,
}

impl BundleRequest {
    /// A request with the defaults every zfb caller starts from: ESM, no
    /// splitting, no minify, no source map, no graph file.
    pub fn new(
        cwd: impl Into<PathBuf>,
        entry: impl Into<PathBuf>,
        output: OutputLayout,
        platform: Platform,
    ) -> Self {
        Self {
            cwd: cwd.into(),
            entry: entry.into(),
            output,
            format: Format::Esm,
            platform,
            target: None,
            jsx: Jsx::default(),
            tsconfig: None,
            resolve: Resolve::default(),
            defines: Vec::new(),
            loaders: Vec::new(),
            externals: Vec::new(),
            splitting: false,
            minify: false,
            keep_names: false,
            source_map: SourceMap::None,
            graph: None,
        }
    }
}

/// Output layout. A single file never splits; a directory may.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputLayout {
    /// One output file, relative to `cwd` or absolute.
    File(PathBuf),
    Dir {
        /// Output directory, relative to `cwd` or absolute.
        dir: PathBuf,
        /// Entry file pattern without extension, e.g. `[name]`.
        entry_names: Option<String>,
        /// Chunk file pattern without extension, e.g. `[name]-[hash]`.
        chunk_names: Option<String>,
        /// Asset file pattern without extension, e.g. `[name]-[hash]`.
        asset_names: Option<String>,
    },
}

impl OutputLayout {
    pub fn dir(dir: impl Into<PathBuf>) -> Self {
        Self::Dir {
            dir: dir.into(),
            entry_names: None,
            chunk_names: None,
            asset_names: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    Esm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform {
    Browser,
    /// No platform defaults: no implicit `main` fields and no Node builtins.
    Neutral,
    Node,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Jsx {
    /// `Some(Automatic)` selects the automatic runtime; `None` leaves it to
    /// the tsconfig or the engine default.
    pub runtime: Option<JsxRuntime>,
    pub import_source: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JsxRuntime {
    Automatic,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolve {
    /// Ordered `(key, target)` aliases. A key matches the exact specifier and
    /// its `key/` subpaths; a trailing `$` matches the exact specifier only and
    /// a `key/*` key substitutes the subpath into a `target/*` target.
    pub aliases: Vec<(String, String)>,
    /// Extra export conditions, added to the platform defaults.
    pub conditions: Option<Vec<String>>,
    /// Package entry fields in priority order, replacing the platform defaults.
    pub main_fields: Option<Vec<String>>,
    /// Resolve a symlinked module from its link location, not its target.
    pub preserve_symlinks: bool,
    /// Additional module directories searched after `node_modules`, in order
    /// (the role `NODE_PATH` plays for Node).
    pub extra_module_dirs: Vec<PathBuf>,
}

/// How files with an extension are loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Loader {
    Js,
    Jsx,
    Ts,
    Tsx,
    Json,
    Text,
    Base64,
    DataUrl,
    Binary,
    Empty,
    /// Copy the file into the output and import its relative URL string.
    File,
    /// Copy the file into the output and keep the import, rewritten to the
    /// copied file's relative path.
    Copy,
    /// Raw CSS. Rejected at build time: zfb's CSS is owned by Wind/Lightning CSS.
    Css,
}

impl Loader {
    /// Parses a loader name as configured by users and plugins.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "js" => Self::Js,
            "jsx" => Self::Jsx,
            "ts" => Self::Ts,
            "tsx" => Self::Tsx,
            "json" => Self::Json,
            "text" => Self::Text,
            "base64" => Self::Base64,
            "dataurl" => Self::DataUrl,
            "binary" => Self::Binary,
            "empty" => Self::Empty,
            "file" => Self::File,
            "copy" => Self::Copy,
            "css" => Self::Css,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Js => "js",
            Self::Jsx => "jsx",
            Self::Ts => "ts",
            Self::Tsx => "tsx",
            Self::Json => "json",
            Self::Text => "text",
            Self::Base64 => "base64",
            Self::DataUrl => "dataurl",
            Self::Binary => "binary",
            Self::Empty => "empty",
            Self::File => "file",
            Self::Copy => "copy",
            Self::Css => "css",
        }
    }
}

/// A specifier left unbundled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum External {
    /// The specifier itself and its `spec/` subpaths.
    Package(String),
    /// Every specifier starting with this prefix (`node:`).
    Prefix(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceMap {
    None,
    /// A sibling `.map` file referenced by a `sourceMappingURL` comment.
    Linked,
    /// The map embedded in the output as a data URL.
    Inline,
}

/// A generated, validated bundle that is not yet visible to anyone. Nothing
/// is written until [`Generated::publish`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Generated {
    /// Output files (bundles, chunks, copied resources, maps) and, when
    /// requested, the graph file, in the caller's path spelling.
    pub files: Vec<(PathBuf, Vec<u8>)>,
    /// The normalized module graph: `{inputs, outputs, zfbBackend, warnings}`.
    pub graph: serde_json::Value,
    /// Engine warnings that did not fail the build, rendered.
    pub warnings: Vec<String>,
}

impl Generated {
    /// Writes every file. Callers publish only after any validation they own.
    pub fn publish(&self) -> std::io::Result<()> {
        for (path, bytes) in &self.files {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, bytes).map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!("write native output {}: {error}", path.display()),
                )
            })?;
        }
        Ok(())
    }

    /// The bytes generated for `path`, if any.
    pub fn file(&self, path: &Path) -> Option<&[u8]> {
        self.files
            .iter()
            .find(|(candidate, _)| candidate == path)
            .map(|(_, bytes)| bytes.as_slice())
    }
}

/// Engine identity recorded in the graph's `zfbBackend` and in caches.
pub const BACKEND_IDENTITY: &str = "rolldown-24bc2d0";
