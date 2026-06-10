//! Read-recorder: the surface through which filesystem-reading feature
//! plugins report the external files a compile depended on (zfb#942).
//!
//! ## Why this exists
//!
//! `transclude`, `imageDimensions`, and `linkValidation` read **other
//! files** while compiling one MDX source, so the MDX compile cache in
//! `zfb-content` cannot key their output on the input string alone — a
//! cached entry would go stale when a referenced file changes. Instead
//! of bypassing the cache wholesale, those plugins report every read
//! through a [`ReadRecorder`]; the compile cache stores the recorded
//! set as a dependency manifest alongside the compiled output and
//! re-validates each dependency (by re-hashing) before honouring a hit.
//!
//! ## Why it lives in `zfb-md-ast`
//!
//! Same reason as `BuildContext` / the visitor traits: this crate is
//! the dependency boundary that lets `zfb-md-extras` plugins name
//! orchestration types without depending on `zfb-content` (which
//! depends on `zfb-md-extras`). The manifest/cache-validation side
//! lives in `zfb-content` (`dep_manifest` + `mdx_jsx_emit`).
//!
//! ## Contract for recording plugins (zfb#944 wires them)
//!
//! - Record EVERY attempted external read, **including failures**: a
//!   missing transclude include that is later created must invalidate
//!   the cached entry, so [`ReadOutcome::Missing`] is recorded too.
//! - Record the **full file contents** hash. A plugin that only
//!   consumes part of a file (e.g. an image-header sniff) must call
//!   [`ReadRecorder::record_file`] so the recorded hash still covers
//!   the whole file; a plugin that already holds the complete bytes
//!   uses [`ReadRecorder::record_bytes`] to avoid a second read.
//! - One recorder instance per pipeline. The recorder is shared with
//!   plugins via `Arc<ReadRecorder>` (interior mutability — every
//!   method takes `&self`), and the compile-cache choke point clears it
//!   before each compile and drains it after, so reads must not be
//!   interleaved from compiles on other pipelines.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use sha2::{Digest, Sha256};

/// Full lowercase-hex SHA-256 digest of `bytes`.
///
/// The one hashing dialect both sides of the cache-validation contract
/// speak: [`ReadRecorder`] hashes at record time, and the manifest
/// validation in `zfb-content::dep_manifest` re-hashes at lookup time.
/// Both MUST call this helper so the comparison can never drift.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Outcome of one attempted external file read, as observed by a
/// feature plugin during compile (or by validation re-probing later).
///
/// Validation semantics (implemented in `zfb-content::dep_manifest`):
/// a cached entry is honoured only when re-probing every recorded path
/// yields an outcome **equal** to the recorded one — except
/// [`ReadOutcome::Error`], which never validates (two error states are
/// not meaningfully comparable), so an entry recording an error is
/// recompiled every time.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReadOutcome {
    /// The read succeeded; `sha256` is the full lowercase-hex SHA-256
    /// of the **complete** file contents (via [`sha256_hex`]).
    Content {
        /// 64 lowercase hex chars.
        sha256: String,
    },
    /// The path did not exist (`io::ErrorKind::NotFound`). Recorded —
    /// not skipped — so creating the file later invalidates the entry.
    Missing,
    /// The read failed for any reason other than NotFound (permission
    /// denied, is-a-directory, I/O error, …). Terminal for caching: an
    /// entry whose manifest carries an `Error` is never stored/served.
    Error,
}

impl ReadOutcome {
    /// Observe the current on-disk state of `path` — the same
    /// classification [`ReadRecorder::record_file`] records, reused by
    /// manifest validation so record-time and lookup-time observations
    /// are always comparable.
    #[must_use]
    pub fn probe(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => Self::of_bytes(&bytes),
            Err(e) => Self::of_io_error(&e),
        }
    }

    /// Outcome for a successful read whose complete contents are `bytes`.
    #[must_use]
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self::Content {
            sha256: sha256_hex(bytes),
        }
    }

    /// Classify an I/O error: `NotFound` → [`ReadOutcome::Missing`]
    /// (re-validatable — the entry stays good while the file stays
    /// absent), anything else → [`ReadOutcome::Error`] (terminal).
    #[must_use]
    pub fn of_io_error(err: &std::io::Error) -> Self {
        if err.kind() == std::io::ErrorKind::NotFound {
            Self::Missing
        } else {
            Self::Error
        }
    }
}

/// Accumulates the `(path, outcome)` pairs one compile's plugins
/// report. See the module docs for the full contract.
///
/// Reads are deduplicated by `PathBuf` equality — the same component
/// semantics the lexical normalisation in
/// `zfb-content::path_norm::normalize_path_lexically` mirrors, so the
/// dedup here agrees exactly with the normalised-string keying of the
/// manifest built from the drained reads. Two records of the same path
/// with **different** outcomes mean the compile observed the file in
/// two states (changed mid-compile): the slot collapses to
/// [`ReadOutcome::Error`] so the torn observation is never cached.
#[derive(Debug, Default)]
pub struct ReadRecorder {
    reads: Mutex<BTreeMap<PathBuf, ReadOutcome>>,
}

impl ReadRecorder {
    /// Construct an empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read + hash the full on-disk contents of `path` and record the
    /// observed outcome. Returns the outcome so callers can branch on
    /// it without probing twice.
    ///
    /// Use this when the plugin did not (or cannot) retain the complete
    /// file bytes itself — the recorded hash must always cover the
    /// whole file, not just the part the plugin consumed.
    pub fn record_file(&self, path: &Path) -> ReadOutcome {
        let outcome = ReadOutcome::probe(path);
        self.record_outcome(path, outcome.clone());
        outcome
    }

    /// Record a successful read whose **complete** contents the caller
    /// already holds, avoiding a second filesystem read.
    pub fn record_bytes(&self, path: &Path, bytes: &[u8]) {
        self.record_outcome(path, ReadOutcome::of_bytes(bytes));
    }

    /// Record an explicit outcome the caller observed (typically via
    /// [`ReadOutcome::of_io_error`] after its own `fs::read` failed).
    ///
    /// Conflict rule: recording a different outcome for an
    /// already-recorded path collapses the slot to
    /// [`ReadOutcome::Error`] — see the type-level docs.
    pub fn record_outcome(&self, path: &Path, outcome: ReadOutcome) {
        let mut reads = self.lock();
        match reads.get(path) {
            Some(existing) if *existing != outcome => {
                reads.insert(path.to_path_buf(), ReadOutcome::Error);
            }
            Some(_) => {}
            None => {
                reads.insert(path.to_path_buf(), outcome);
            }
        }
    }

    /// Drain every recorded read, leaving the recorder empty.
    #[must_use]
    pub fn take_reads(&self) -> BTreeMap<PathBuf, ReadOutcome> {
        std::mem::take(&mut *self.lock())
    }

    /// Discard every recorded read. The compile-cache choke point calls
    /// this before each compile so reads left by an earlier compile
    /// (e.g. one that aborted on a parse error) cannot leak into the
    /// next entry's manifest.
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// Number of distinct recorded paths.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// True when nothing has been recorded since the last drain/clear.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// Lock-or-recover: a poisoned mutex still yields a valid guard, so
    /// recording never panics on a prior writer crash (mirrors
    /// `MdxModuleCache::lock` in zfb-content).
    fn lock(&self) -> MutexGuard<'_, BTreeMap<PathBuf, ReadOutcome>> {
        match self.reads.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pinned NIST test vector: sha256("abc"). Guards the hashing
    // dialect both record and validation sides depend on.
    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn probe_classifies_content_missing_and_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("dep.md");
        std::fs::write(&file, b"hello").expect("write");

        assert_eq!(
            ReadOutcome::probe(&file),
            ReadOutcome::Content {
                sha256: sha256_hex(b"hello")
            }
        );
        assert_eq!(
            ReadOutcome::probe(&dir.path().join("absent.md")),
            ReadOutcome::Missing
        );
        // NotFound classification goes through of_io_error; non-NotFound
        // errors map to Error (probing a directory is platform-dependent,
        // so pin the classifier directly instead).
        let not_found = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert_eq!(ReadOutcome::of_io_error(&not_found), ReadOutcome::Missing);
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert_eq!(ReadOutcome::of_io_error(&denied), ReadOutcome::Error);
    }

    #[test]
    fn record_file_and_record_bytes_agree() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("dep.md");
        std::fs::write(&file, b"same bytes").expect("write");

        let via_file = ReadRecorder::new();
        let outcome = via_file.record_file(&file);
        let via_bytes = ReadRecorder::new();
        via_bytes.record_bytes(&file, b"same bytes");

        assert_eq!(via_file.take_reads(), via_bytes.take_reads());
        assert_eq!(
            outcome,
            ReadOutcome::Content {
                sha256: sha256_hex(b"same bytes")
            }
        );
    }

    #[test]
    fn same_path_same_outcome_dedupes() {
        let rec = ReadRecorder::new();
        rec.record_outcome(Path::new("/a/b"), ReadOutcome::Missing);
        rec.record_outcome(Path::new("/a/b"), ReadOutcome::Missing);
        // Spellings Path equality merges (interior `.`) share one slot.
        rec.record_outcome(Path::new("/a/./b"), ReadOutcome::Missing);
        assert_eq!(rec.len(), 1);
    }

    #[test]
    fn conflicting_outcomes_collapse_to_error() {
        let rec = ReadRecorder::new();
        rec.record_outcome(Path::new("/a/b"), ReadOutcome::of_bytes(b"v1"));
        rec.record_outcome(Path::new("/a/b"), ReadOutcome::Missing);
        let reads = rec.take_reads();
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[Path::new("/a/b")], ReadOutcome::Error);
    }

    #[test]
    fn take_reads_drains() {
        let rec = ReadRecorder::new();
        rec.record_outcome(Path::new("/a"), ReadOutcome::Missing);
        assert!(!rec.is_empty());
        let drained = rec.take_reads();
        assert_eq!(drained.len(), 1);
        assert!(rec.is_empty());
    }

    #[test]
    fn clear_discards_pending_reads() {
        let rec = ReadRecorder::new();
        rec.record_outcome(Path::new("/a"), ReadOutcome::Missing);
        rec.clear();
        assert!(rec.take_reads().is_empty());
    }
}
