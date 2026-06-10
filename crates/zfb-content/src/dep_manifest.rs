//! Dependency manifest for cached MDX compiles (zfb#942).
//!
//! One [`DependencyManifest`] is stored in every `MdxModuleCache` entry
//! (see `mdx_jsx_emit::CachedMdxModule`): the set of external files the
//! compile's feature plugins reported reading through the
//! [`ReadRecorder`](zfb_md_ast::ReadRecorder), keyed by the shared
//! lexical path normalisation (`crate::path_norm`) and carrying the
//! [`ReadOutcome`] observed at compile time.
//!
//! ## Validation contract
//!
//! Before honouring a cache hit, `compile_mdx_to_jsx_module_cached`
//! calls [`DependencyManifest::still_valid`], which re-probes every
//! recorded path against the current filesystem:
//!
//! - `Content { sha256 }` — the file is re-read and re-hashed; the dep
//!   holds only when the digest matches. **Deletion, permission
//!   errors, and any other read failure are misses** (the conservative
//!   direction: a spurious miss only costs a recompile).
//! - `Missing` — the dep holds only while the path still cleanly
//!   probes as NotFound. A file created at the path — or a probe that
//!   cannot confirm absence — is a miss, so a missing transclude
//!   include that is later created invalidates the entry.
//! - `Error` — never holds. Two error states are not meaningfully
//!   comparable, so such entries are also never *stored*
//!   ([`DependencyManifest::is_storable`]); the check here is
//!   defence-in-depth.
//!
//! Validation is **per-entry rehash on lookup** — there is
//! deliberately no global reverse dependency graph (dep → entries)
//! to maintain; the cost is one `fs::read` + hash per recorded dep per
//! lookup, paid only by entries that recorded reads.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use zfb_md_ast::ReadOutcome;

use crate::path_norm::normalize_path_lexically;

/// The external-read set of one cached MDX compile. See module docs.
///
/// Keys are the lexically-normalised path strings produced by
/// `path_norm::normalize_path_lexically` — string equality of two keys
/// is equivalent to `Path` equality of the recorded paths, so this map
/// dedups exactly the spellings the recorder's `PathBuf` keying dedups,
/// and re-probing `Path::new(key)` observes the same file the plugin
/// read (the normalised rendering is itself a valid path).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyManifest {
    entries: BTreeMap<String, ReadOutcome>,
}

impl DependencyManifest {
    /// Build a manifest from the reads drained out of a
    /// [`ReadRecorder`](zfb_md_ast::ReadRecorder).
    ///
    /// Normalisation cannot collide two recorder slots: the recorder
    /// keys by `PathBuf` equality and the normalised rendering is
    /// injective over it (the pinned property in `path_norm`).
    #[must_use]
    pub fn from_recorded_reads(reads: BTreeMap<PathBuf, ReadOutcome>) -> Self {
        Self {
            entries: reads
                .into_iter()
                .map(|(path, outcome)| (normalize_path_lexically(&path), outcome))
                .collect(),
        }
    }

    /// Number of recorded dependencies.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when the compile reported no external reads — the manifest
    /// of every plain (non-filesystem-reading) pipeline, for which
    /// [`DependencyManifest::still_valid`] trivially holds.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate `(normalised path, outcome)` entries in sorted order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ReadOutcome)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// True when every recorded dependency re-validates against the
    /// current filesystem state — the gate a cache hit must pass. See
    /// the module docs for the per-outcome rules.
    #[must_use]
    pub(crate) fn still_valid(&self) -> bool {
        self.entries.iter().all(|(path, recorded)| {
            // `Error` never validates, even if a fresh probe also errors
            // — error states are not comparable observations.
            !matches!(recorded, ReadOutcome::Error)
                && ReadOutcome::probe(Path::new(path)) == *recorded
        })
    }

    /// True when the manifest may be stored in a cache entry: no
    /// recorded read ended in [`ReadOutcome::Error`]. An entry carrying
    /// an `Error` could never pass [`DependencyManifest::still_valid`],
    /// so storing it would only burn a cache slot.
    #[must_use]
    pub(crate) fn is_storable(&self) -> bool {
        self.entries
            .values()
            .all(|o| !matches!(o, ReadOutcome::Error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zfb_md_ast::ReadRecorder;

    fn manifest_of(rec: &ReadRecorder) -> DependencyManifest {
        DependencyManifest::from_recorded_reads(rec.take_reads())
    }

    #[test]
    fn empty_manifest_is_always_valid_and_storable() {
        let m = DependencyManifest::default();
        assert!(m.is_empty());
        assert!(m.still_valid());
        assert!(m.is_storable());
    }

    #[test]
    fn keys_are_normalised_spellings() {
        let rec = ReadRecorder::new();
        let dir = tempfile::tempdir().expect("tempdir");
        // A redundant interior `.` + separator run normalises away.
        let spelled = dir.path().join(".").join("dep.md");
        std::fs::write(dir.path().join("dep.md"), b"x").expect("write");
        rec.record_file(&spelled);
        let m = manifest_of(&rec);
        let key = m.iter().next().expect("one entry").0.to_string();
        assert_eq!(key, normalize_path_lexically(&dir.path().join("dep.md")));
        assert!(!key.contains("/./"));
    }

    #[test]
    fn unchanged_content_dep_still_validates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dep = dir.path().join("dep.md");
        std::fs::write(&dep, b"v1").expect("write");
        let rec = ReadRecorder::new();
        rec.record_file(&dep);
        let m = manifest_of(&rec);
        assert!(m.still_valid());
    }

    #[test]
    fn edited_content_dep_invalidates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dep = dir.path().join("dep.md");
        std::fs::write(&dep, b"v1").expect("write");
        let rec = ReadRecorder::new();
        rec.record_file(&dep);
        let m = manifest_of(&rec);
        std::fs::write(&dep, b"v2").expect("rewrite");
        assert!(!m.still_valid());
    }

    #[test]
    fn deleted_content_dep_invalidates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dep = dir.path().join("dep.md");
        std::fs::write(&dep, b"v1").expect("write");
        let rec = ReadRecorder::new();
        rec.record_file(&dep);
        let m = manifest_of(&rec);
        std::fs::remove_file(&dep).expect("delete");
        assert!(!m.still_valid());
    }

    #[test]
    fn missing_dep_validates_while_absent_and_invalidates_once_created() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dep = dir.path().join("not-yet.md");
        let rec = ReadRecorder::new();
        assert_eq!(rec.record_file(&dep), ReadOutcome::Missing);
        let m = manifest_of(&rec);
        assert!(m.still_valid(), "still-missing dep must keep the entry valid");
        std::fs::write(&dep, b"now it exists").expect("create");
        assert!(!m.still_valid(), "created dep must invalidate");
    }

    #[test]
    fn error_outcome_is_never_storable_nor_valid() {
        let rec = ReadRecorder::new();
        rec.record_outcome(Path::new("/somewhere/dep.md"), ReadOutcome::Error);
        let m = manifest_of(&rec);
        assert!(!m.is_storable());
        assert!(!m.still_valid());
    }
}
