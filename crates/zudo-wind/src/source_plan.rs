//! Explicit, comparable source ownership and stable relative identities.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

/// A declared root. `path` is resolved against `declaring_dir`, never the process cwd.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct PositiveRoot {
    pub label: String,
    pub declaring_dir: PathBuf,
    pub path: PathBuf,
    pub required: bool,
    /// Paths omitted only while walking this root, resolved against `declaring_dir`.
    pub exclusions: BTreeSet<PathBuf>,
    /// A declared package root also walks its own `node_modules` and `dist`.
    pub package_root: bool,
}

impl PositiveRoot {
    pub fn resolved_path(&self) -> PathBuf {
        self.declaring_dir.join(&self.path)
    }
}

/// Source identity rendered only from a stable label and a lexical relative path.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SourceId {
    label: String,
    relative_path: String,
}

impl SourceId {
    pub fn new(label: impl Into<String>, relative_path: impl AsRef<Path>) -> Result<Self, String> {
        let label = label.into();
        if label.is_empty() || label.starts_with('/') || label.contains('\\') || label.contains(':')
        {
            return Err("source label must be a stable relative identifier".into());
        }
        let raw = relative_path
            .as_ref()
            .to_str()
            .ok_or("source path is not UTF-8")?;
        let normalized = raw.replace('\\', "/");
        if normalized.starts_with('/') || normalized.as_bytes().get(1) == Some(&b':') {
            return Err("source path must be relative".into());
        }
        let mut parts = Vec::new();
        for component in Path::new(&normalized).components() {
            match component {
                Component::Normal(part) => {
                    let part = part.to_str().ok_or("source path is not UTF-8")?;
                    parts.push(part.to_owned());
                }
                Component::CurDir => {}
                Component::ParentDir => {
                    if parts.pop().is_none() {
                        return Err("source path escapes its root".into());
                    }
                }
                _ => return Err("source path must be relative".into()),
            }
        }
        Ok(Self {
            label,
            relative_path: parts.join("/"),
        })
    }

    pub fn label(&self) -> &str {
        &self.label
    }
    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }
    pub fn render(&self) -> String {
        format!("{}:{}", self.label, self.relative_path)
    }

    pub fn is_under(&self, directory: &SourceId) -> bool {
        self.label == directory.label
            && (directory.relative_path.is_empty()
                || self.relative_path == directory.relative_path
                || self
                    .relative_path
                    .starts_with(&format!("{}/", directory.relative_path)))
    }
}

/// An author exclusion glob, matched against paths relative to `declaring_dir`.
/// It wins over every positive root, including explicit sources.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct SourceExclusion {
    /// Who declared it, for example `project` or `preset:@scope/name`.
    pub origin: String,
    pub declaring_dir: PathBuf,
    pub pattern: String,
}

impl SourceExclusion {
    pub fn compile(&self) -> Result<ExclusionMatcher, String> {
        let matcher = compile_exclusion_pattern(&self.pattern)?;
        let mut bases = vec![self.declaring_dir.clone()];
        if let Ok(canonical) = std::fs::canonicalize(&self.declaring_dir) {
            if canonical != self.declaring_dir {
                bases.push(canonical);
            }
        }
        Ok(ExclusionMatcher { bases, matcher })
    }
}

/// A compiled [`SourceExclusion`].
#[derive(Clone, Debug)]
pub struct ExclusionMatcher {
    bases: Vec<PathBuf>,
    matcher: globset::GlobMatcher,
}

impl ExclusionMatcher {
    /// True when `path` or one of its ancestors below the declaring root
    /// matches. Both the walked and canonical identities are checked so a
    /// symlink cannot reintroduce an excluded file.
    pub fn matches_identity(&self, path: &Path, canonical: Option<&Path>) -> bool {
        self.bases.iter().any(|base| {
            std::iter::once(path)
                .chain(canonical)
                .filter_map(|candidate| candidate.strip_prefix(base).ok())
                .any(|relative| {
                    relative
                        .ancestors()
                        .filter(|prefix| !prefix.as_os_str().is_empty())
                        .any(|prefix| self.matcher.is_match(prefix))
                })
        })
    }
}

/// Compile an author exclusion. A pattern excludes a path when it matches
/// the path or any of its ancestor directories, so `src` excludes all of `src`.
pub fn compile_exclusion_pattern(pattern: &str) -> Result<globset::GlobMatcher, String> {
    if pattern.is_empty() {
        return Err("must not be empty".into());
    }
    if pattern.starts_with('!') {
        return Err("negated patterns are not supported; list only paths to exclude".into());
    }
    if pattern.contains('\\') {
        return Err("must use `/` separators".into());
    }
    let relative = pattern.strip_prefix("./").unwrap_or(pattern);
    if relative.starts_with('/') || relative.as_bytes().get(1) == Some(&b':') {
        return Err("must be relative to the declaring root".into());
    }
    // A gitignore-style `dir/` names the directory; ancestor matching then
    // covers everything below it.
    let relative = relative.strip_suffix('/').unwrap_or(relative);
    if relative.is_empty() || relative == "." {
        return Err("must name a path below the declaring root".into());
    }
    if relative.split('/').any(|part| part == "..") {
        return Err("must not contain `..`; it is matched only under the declaring root".into());
    }
    globset::GlobBuilder::new(relative)
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|error| error.kind().to_string())
}

/// Full discovery declaration. Maps have independent owner/producer namespaces.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePlan {
    pub extraction_options: crate::ExtractionOptions,
    pub roots: Vec<PositiveRoot>,
    pub exclusions: BTreeSet<PathBuf>,
    pub author_exclusions: Vec<SourceExclusion>,
    pub package_sources: BTreeMap<String, PositiveRoot>,
    pub generated_sources: BTreeMap<String, BTreeSet<String>>,
    pub manifests: BTreeMap<String, PathBuf>,
    pub safelist: BTreeMap<String, BTreeSet<String>>,
    pub extensions: BTreeSet<String>,
}

impl Default for SourcePlan {
    fn default() -> Self {
        Self {
            extraction_options: crate::ExtractionOptions::default(),
            roots: Vec::new(),
            exclusions: BTreeSet::new(),
            author_exclusions: Vec::new(),
            package_sources: BTreeMap::new(),
            generated_sources: BTreeMap::new(),
            manifests: BTreeMap::new(),
            safelist: BTreeMap::new(),
            extensions: ["tsx", "ts", "jsx", "js", "mdx", "md"]
                .map(str::to_owned)
                .into(),
        }
    }
}
