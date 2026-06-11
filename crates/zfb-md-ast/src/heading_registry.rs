//! Cross-file heading-ID registry for wave-6 link validation.
//!
//! `HeadingRegistry` accumulates the heading entries from every markdown
//! file processed during a build. `HeadingLinksPlugin` inserts entries
//! when a registry is attached to the `BuildContext`; `LinkValidationPlugin`
//! (wave 6.2) queries the registry to verify `#fragment` links.
//!
//! Zero-cost for non-validating callers: when no registry is passed in the
//! `BuildContext`, `HeadingLinksPlugin` makes no registry calls at all.

use std::collections::HashMap;
use std::path::PathBuf;

/// A single heading entry recorded by `HeadingLinksPlugin`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingEntry {
    /// The `id` attribute value placed on the heading element (i.e. the
    /// deduplicated slug produced by `heading_links::next_slug`).
    pub id: String,
    /// The plain-text content of the heading (extracted before slugification).
    pub text: String,
    /// Heading depth (1–6).
    pub depth: u8,
}

/// Build-scoped registry mapping each source file path to the list of
/// headings encountered in that file.
///
/// Thread-safety: `HeadingRegistry` is intentionally NOT `Send + Sync` in
/// this wave — a single registry is attached to the pipeline for one build
/// thread. If parallel builds are introduced later, wrap in a `Mutex` or
/// switch to a concurrent map.
#[derive(Debug, Default)]
pub struct HeadingRegistry {
    entries: HashMap<PathBuf, Vec<HeadingEntry>>,
}

impl HeadingRegistry {
    /// Create a new, empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a heading entry for the given source file.
    ///
    /// Called by `HeadingLinksPlugin` for every heading it processes, when a
    /// registry is available in the `BuildContext`. Entries are appended in
    /// document order.
    pub fn insert(&mut self, source_path: PathBuf, entry: HeadingEntry) {
        self.entries.entry(source_path).or_default().push(entry);
    }

    /// Look up all headings recorded for the given source file.
    ///
    /// Returns `None` if no headings have been registered for that path yet.
    #[must_use]
    pub fn get(&self, source_path: &std::path::Path) -> Option<&[HeadingEntry]> {
        self.entries.get(source_path).map(Vec::as_slice)
    }

    /// Return all entries across all files (path → entries slice iterator).
    pub fn iter(&self) -> impl Iterator<Item = (&PathBuf, &Vec<HeadingEntry>)> {
        self.entries.iter()
    }

    /// Total number of heading entries across all files.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.values().map(Vec::len).sum()
    }

    /// True when no heading entries have been recorded yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.values().all(Vec::is_empty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_query() {
        let mut reg = HeadingRegistry::new();
        let path = PathBuf::from("/docs/guide.md");
        reg.insert(
            path.clone(),
            HeadingEntry {
                id: "introduction".to_string(),
                text: "Introduction".to_string(),
                depth: 2,
            },
        );
        reg.insert(
            path.clone(),
            HeadingEntry {
                id: "setup".to_string(),
                text: "Setup".to_string(),
                depth: 2,
            },
        );
        let entries = reg.get(&path).expect("entries must be present");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "introduction");
        assert_eq!(entries[1].id, "setup");
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn is_empty_on_new_registry() {
        let reg = HeadingRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn query_unknown_path_returns_none() {
        let reg = HeadingRegistry::new();
        assert!(reg
            .get(std::path::Path::new("/does/not/exist.md"))
            .is_none());
    }
}
