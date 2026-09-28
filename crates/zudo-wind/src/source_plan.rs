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

/// Full discovery declaration. Maps have independent owner/producer namespaces.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePlan {
    pub roots: Vec<PositiveRoot>,
    pub exclusions: BTreeSet<PathBuf>,
    pub package_sources: BTreeMap<String, PositiveRoot>,
    pub generated_sources: BTreeMap<String, BTreeSet<String>>,
    pub manifests: BTreeMap<String, PathBuf>,
    pub safelist: BTreeMap<String, BTreeSet<String>>,
    pub extensions: BTreeSet<String>,
}

impl Default for SourcePlan {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            exclusions: BTreeSet::new(),
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
