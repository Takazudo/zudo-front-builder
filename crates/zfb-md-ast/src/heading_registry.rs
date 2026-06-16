//! Cross-file heading-ID registry for wave-6 link validation.
//!
//! `HeadingRegistry` accumulates the heading entries from every markdown
//! file processed during a build. `HeadingLinksPlugin` inserts entries
//! when a registry is attached to the `BuildContext`; `LinkValidationPlugin`
//! (wave 6.2) queries the registry to verify `#fragment` links.
//!
//! Zero-cost for non-validating callers: when no registry is passed in the
//! `BuildContext`, `HeadingLinksPlugin` makes no registry calls at all.

use std::collections::{HashMap, HashSet};
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
    /// Explicit (non-heading) anchor ids recorded per source file.
    ///
    /// Populated by `HeadingLinksPlugin` for every `Element` carrying an `id`
    /// attribute (e.g. `<div id="foo">`) and every `<a name="…">` anchor. This
    /// is a separate store from `entries` because it must be queryable even for
    /// files that have zero headings — the `mark_tracked` / `get` contract on
    /// `entries` only tells the validator whether the file was processed, while
    /// `anchor_ids` independently tells it whether a specific non-heading id
    /// exists. See issue #1095.
    anchor_ids: HashMap<PathBuf, HashSet<String>>,
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

    /// Mark a source file as **tracked** without recording any heading.
    ///
    /// Ensures `get(source_path)` returns `Some(&[])` (an empty slice) rather
    /// than `None` for a file that `HeadingLinksPlugin` processed but which
    /// contained zero `h2`–`h6` headings. This is what lets
    /// `LinkValidationPlugin` distinguish "file processed, has no headings"
    /// (a bare `#anchor` is then definitively broken) from "file never
    /// tracked" (registry unaware — skip to avoid false positives, e.g. the
    /// JSX-emit cache-hit path where `HeadingLinksPlugin` never runs).
    /// Idempotent and never clears existing entries.
    pub fn mark_tracked(&mut self, source_path: PathBuf) {
        self.entries.entry(source_path).or_default();
    }

    /// Record a non-heading explicit anchor `id` for `source_path`.
    ///
    /// Called by `HeadingLinksPlugin` for every hast `Element` whose `id`
    /// attribute was NOT set by heading-slug logic (all non-`h2`–`h6` elements
    /// with an explicit `id`), and for every `<a name="…">` element. This lets
    /// `LinkValidationPlugin` accept `#foo` in a headingless file when a
    /// `<div id="foo">` or `<a name="foo">` is present, without reporting a
    /// false-positive `BrokenLink`.
    ///
    /// The store is path-keyed and independent from `entries` so a file with
    /// zero headings can still have anchor ids recorded. Idempotent for
    /// duplicate ids within the same file.
    ///
    /// NOTE: Raw-HTML elements (`<div id="x">` written as literal HTML inside
    /// markdown) and GFM footnote back-reference anchors (`id="user-content-fn-1"`)
    /// arrive as `HastNode::Raw` (opaque strings) at the point `HeadingLinksPlugin`
    /// walks — they are NOT structured `Element` nodes and therefore cannot be
    /// recorded here. See the known limitation comment in `heading_links.rs`.
    pub fn insert_anchor_id(&mut self, source_path: PathBuf, id: String) {
        self.anchor_ids.entry(source_path).or_default().insert(id);
    }

    /// Return `true` if `id` was recorded as an explicit anchor in `source_path`.
    ///
    /// Returns `false` for unknown paths and unknown ids alike, so callers can
    /// use a simple boolean gate rather than an `Option` chain.
    #[must_use]
    pub fn has_anchor(&self, source_path: &std::path::Path, id: &str) -> bool {
        self.anchor_ids
            .get(source_path)
            .is_some_and(|ids| ids.contains(id))
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

    #[test]
    fn mark_tracked_makes_get_return_empty_slice_not_none() {
        // A file processed with zero headings must be distinguishable from a
        // never-tracked file: `get` returns `Some(&[])`, not `None` (zfb#1093).
        let mut reg = HeadingRegistry::new();
        let path = PathBuf::from("/docs/headingless.md");
        assert!(reg.get(&path).is_none());
        reg.mark_tracked(path.clone());
        assert_eq!(reg.get(&path), Some(&[][..]));
        // No headings were recorded, so the registry is still empty by count.
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }

    // ── anchor_ids store ──────────────────────────────────────────────────────

    #[test]
    fn insert_anchor_id_and_has_anchor() {
        let mut reg = HeadingRegistry::new();
        let path = PathBuf::from("/docs/headingless.md");
        // Unknown path + unknown id → false before any insert.
        assert!(!reg.has_anchor(&path, "foo"));
        reg.insert_anchor_id(path.clone(), "foo".to_string());
        assert!(reg.has_anchor(&path, "foo"));
        // Different id on the same path → still false.
        assert!(!reg.has_anchor(&path, "bar"));
        // Multiple ids on the same path.
        reg.insert_anchor_id(path.clone(), "bar".to_string());
        assert!(reg.has_anchor(&path, "bar"));
    }

    #[test]
    fn has_anchor_unknown_path_returns_false() {
        let reg = HeadingRegistry::new();
        assert!(!reg.has_anchor(std::path::Path::new("/does/not/exist.md"), "any"));
    }

    #[test]
    fn insert_anchor_id_is_idempotent() {
        let mut reg = HeadingRegistry::new();
        let path = PathBuf::from("/docs/page.md");
        reg.insert_anchor_id(path.clone(), "foo".to_string());
        reg.insert_anchor_id(path.clone(), "foo".to_string()); // duplicate — no panic
        assert!(reg.has_anchor(&path, "foo"));
    }

    #[test]
    fn anchor_ids_independent_from_heading_entries() {
        // A file can have anchor ids without any heading entries (and vice versa).
        let mut reg = HeadingRegistry::new();
        let path = PathBuf::from("/docs/headingless.md");
        reg.mark_tracked(path.clone());
        reg.insert_anchor_id(path.clone(), "section".to_string());
        // get() still returns Some(&[]) — no headings.
        assert_eq!(reg.get(&path), Some(&[][..]));
        // has_anchor correctly returns true for the registered id.
        assert!(reg.has_anchor(&path, "section"));
        // len() / is_empty() count only heading entries, unaffected.
        assert_eq!(reg.len(), 0);
        assert!(reg.is_empty());
    }

    #[test]
    fn mark_tracked_is_idempotent_and_preserves_entries() {
        let mut reg = HeadingRegistry::new();
        let path = PathBuf::from("/docs/guide.md");
        reg.insert(
            path.clone(),
            HeadingEntry {
                id: "intro".to_string(),
                text: "Intro".to_string(),
                depth: 2,
            },
        );
        // Marking an already-populated file must not clear its entries.
        reg.mark_tracked(path.clone());
        let entries = reg.get(&path).expect("entries must survive mark_tracked");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "intro");
    }
}
