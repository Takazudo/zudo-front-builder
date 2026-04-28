//! Persist [`DependencyGraph`] snapshots to disk so cold starts can
//! reuse a previously-computed graph instead of rebuilding from
//! scratch.
//!
//! ## Wire format
//!
//! `MAGIC (4) || VERSION (2 LE) || DIGEST (32) || bincode(DependencyGraph)`
//!
//! Magic + version let us reject foreign files and stale formats
//! without misreading bytes as a graph. The digest is checked against
//! the caller's freshly-computed [`ManifestDigest`] before any
//! deserialisation work happens; if they disagree, the file is
//! discarded. Bincode (1.x) is used because it's already tiny, fast,
//! and serde-native — no extra ceremony needed for the graph types
//! that already derive `Serialize`/`Deserialize`.
//!
//! ## Manifest digest — what invalidates the graph
//!
//! [`ManifestDigest::compute`] hashes the inputs that determine graph
//! identity:
//!
//! 1. The full byte content of every "config-like" file the caller
//!    passes in (e.g. `zfb.config.json`, `zfb.config.ts`). Those files
//!    are tiny and definitive; a single byte change must invalidate.
//! 2. A canonical listing of every file under each watched root, fed
//!    in as `(relative_path, mtime_secs, len_bytes)`. We deliberately
//!    use mtime+len rather than full content hashes — file metadata
//!    is cheap to read and matches the heuristic Make/Ninja-style
//!    incremental tools have used reliably for decades. The trade-off
//!    is documented: an unchanged file whose mtime jitters (e.g. a
//!    `touch` with no edits) will trigger a fresh build on the next
//!    `zfb dev` start. That is the safe direction — false-invalidate,
//!    never false-reuse — so we accept it.
//!
//! Entries are sorted by path before hashing so the digest is
//! deterministic across runs and platforms (HashMap ordering is not).
//!
//! Future work: when the resolver lands a content-hash side-channel,
//! callers can plug it in here without changing the wire format.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::error::GraphError;
use crate::DependencyGraph;

/// 4-byte magic identifying a zfb-graph snapshot file.
const MAGIC: &[u8; 4] = b"ZFBG";

/// On-disk format version. Bump on any backwards-incompatible
/// change to the wire format or to fields that bincode round-trips.
const VERSION: u16 = 1;

/// Stable hash over the inputs that determine graph identity.
///
/// Two graphs built from the same set of inputs produce the same
/// digest; any tracked change flips it. Use [`Self::compute`] to
/// build one from a project layout, or [`Self::from_bytes`] when
/// reading a digest off disk.
///
/// The digest is opaque — the only operations callers should rely on
/// are equality and the [`Self::as_hex`] debugging helper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestDigest([u8; 32]);

impl ManifestDigest {
    /// Wrap a precomputed 32-byte digest. Mostly useful in tests.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Borrow the raw 32 bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Hex-encoded digest, lowercase. For debug logs only.
    pub fn as_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in &self.0 {
            s.push_str(&format!("{:02x}", b));
        }
        s
    }

    /// Compute a digest over a project layout.
    ///
    /// `project_root` is the prefix used to resolve relative
    /// `watch_roots` and to emit project-relative paths inside the
    /// hash (so the digest does not depend on the absolute install
    /// directory).
    ///
    /// `watch_roots` is the list of directory or file roots whose
    /// listing affects graph identity. Missing roots are silently
    /// skipped — they contribute the empty listing.
    ///
    /// `config_files` is the list of config-like files whose full
    /// byte content matters (e.g. `zfb.config.json`,
    /// `zfb.config.ts`). Missing files are silently skipped — they
    /// contribute no bytes. Paths may be absolute or relative to
    /// `project_root`.
    pub fn compute(
        project_root: &Path,
        watch_roots: &[PathBuf],
        config_files: &[PathBuf],
    ) -> Result<Self, GraphError> {
        let mut hasher = Sha256::new();

        // (1) Config files — full content hash. Sort by absolute path
        // so the order in `config_files` does not affect the digest.
        let mut cfg_paths: Vec<PathBuf> = config_files
            .iter()
            .map(|p| resolve(project_root, p))
            .collect();
        cfg_paths.sort();
        cfg_paths.dedup();
        for p in &cfg_paths {
            if let Ok(bytes) = fs::read(p) {
                hasher.update(b"cfg:");
                let rel = relativise(project_root, p);
                hasher.update(rel.as_bytes());
                hasher.update(b":");
                hasher.update(&(bytes.len() as u64).to_le_bytes());
                hasher.update(b":");
                hasher.update(&bytes);
                hasher.update(b"\n");
            }
        }

        // (2) Watched roots — listing of (rel_path, mtime, len).
        // Collect into a BTreeMap so the iteration order is
        // deterministic regardless of filesystem walk order.
        let mut entries: BTreeMap<String, FileMeta> = BTreeMap::new();
        for root in watch_roots {
            let abs_root = resolve(project_root, root);
            if !abs_root.exists() {
                continue;
            }
            if abs_root.is_file() {
                if let Ok(meta) = abs_root.metadata() {
                    let rel = relativise(project_root, &abs_root);
                    entries.insert(rel, FileMeta::from_metadata(&meta));
                }
                continue;
            }
            for entry in WalkDir::new(&abs_root)
                .follow_links(false)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let Ok(meta) = entry.metadata() else { continue };
                let rel = relativise(project_root, entry.path());
                entries.insert(rel, FileMeta::from_metadata(&meta));
            }
        }
        for (rel, meta) in &entries {
            hasher.update(b"f:");
            hasher.update(rel.as_bytes());
            hasher.update(b":");
            hasher.update(&meta.mtime.to_le_bytes());
            hasher.update(b":");
            hasher.update(&meta.len.to_le_bytes());
            hasher.update(b"\n");
        }

        let out = hasher.finalize();
        let mut buf = [0u8; 32];
        buf.copy_from_slice(&out);
        Ok(Self(buf))
    }
}

#[derive(Debug, Clone, Copy)]
struct FileMeta {
    mtime: i64,
    len: u64,
}

impl FileMeta {
    fn from_metadata(m: &fs::Metadata) -> Self {
        let mtime = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Self {
            mtime,
            len: m.len(),
        }
    }
}

fn resolve(project_root: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        project_root.join(p)
    }
}

fn relativise(project_root: &Path, p: &Path) -> String {
    p.strip_prefix(project_root)
        .unwrap_or(p)
        .to_string_lossy()
        .into_owned()
}

/// Serialise `graph` and `digest` to `path`. Creates parent
/// directories as needed and writes via a temp-file rename so a
/// crash partway through does not leave a half-written graph file.
pub fn save_to_disk(
    graph: &DependencyGraph,
    digest: &ManifestDigest,
    path: &Path,
) -> Result<(), GraphError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let body = bincode::serialize(graph)?;
    let mut buf: Vec<u8> = Vec::with_capacity(4 + 2 + 32 + body.len());
    buf.extend_from_slice(MAGIC);
    buf.extend_from_slice(&VERSION.to_le_bytes());
    buf.extend_from_slice(digest.as_bytes());
    buf.extend_from_slice(&body);

    // Atomic-ish write: write to `path.tmp`, then rename. On the
    // same filesystem rename is atomic on POSIX/Windows.
    let tmp = path.with_extension("bin.tmp");
    {
        let mut f = File::create(&tmp)?;
        f.write_all(&buf)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Try to read a graph from `path` and return it iff the on-disk
/// digest matches `expected`.
///
/// Returns `Ok(None)` for the common "no usable cache" cases —
/// missing file, mismatched digest, or a header from a different
/// version of zfb. Returns `Err` only for genuine IO / decode
/// failures the caller may want to surface (e.g. permission
/// denied), so callers can simply pattern-match `Ok(Some(g))` for
/// the fast path and fall back to a fresh graph otherwise.
pub fn load_from_disk(
    path: &Path,
    expected: &ManifestDigest,
) -> Result<Option<DependencyGraph>, GraphError> {
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };

    let mut header = [0u8; 4 + 2 + 32];
    if let Err(e) = f.read_exact(&mut header) {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            // Truncated / empty file — discard.
            return Ok(None);
        }
        return Err(e.into());
    }

    if &header[0..4] != MAGIC {
        return Ok(None);
    }
    let version = u16::from_le_bytes([header[4], header[5]]);
    if version != VERSION {
        return Ok(None);
    }
    let mut on_disk_digest = [0u8; 32];
    on_disk_digest.copy_from_slice(&header[6..6 + 32]);
    if on_disk_digest != *expected.as_bytes() {
        return Ok(None);
    }

    let mut body = Vec::new();
    f.read_to_end(&mut body)?;
    let graph: DependencyGraph = bincode::deserialize(&body)?;
    Ok(Some(graph))
}

// -----------------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DepKind, PageDeps, PageId};
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }
    fn pid(s: &str) -> PageId {
        PageId::new(p(s))
    }

    fn small_graph() -> DependencyGraph {
        let mut g = DependencyGraph::new();
        g.upsert(PageDeps::new(
            pid("/pages/a.tsx"),
            vec![(p("/layouts/main.tsx"), DepKind::Module)],
        ));
        g.upsert(PageDeps::new(
            pid("/pages/b.tsx"),
            vec![
                (p("/layouts/main.tsx"), DepKind::Module),
                (p("/styles/site.css"), DepKind::Style),
            ],
        ));
        g.mark_global("/zfb.config.ts");
        g
    }

    #[test]
    fn roundtrip_write_then_read_matches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".zfb").join("graph.bin");
        let digest = ManifestDigest::from_bytes([7u8; 32]);

        let g = small_graph();
        save_to_disk(&g, &digest, &path).expect("save");

        let loaded = load_from_disk(&path, &digest)
            .expect("load")
            .expect("present + digest match");
        assert_eq!(loaded, g);
    }

    #[test]
    fn digest_mismatch_invalidates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("graph.bin");
        let written_digest = ManifestDigest::from_bytes([1u8; 32]);
        let other_digest = ManifestDigest::from_bytes([2u8; 32]);

        save_to_disk(&small_graph(), &written_digest, &path).unwrap();

        // Right digest → present.
        assert!(load_from_disk(&path, &written_digest).unwrap().is_some());
        // Wrong digest → discarded; caller will rebuild.
        assert!(load_from_disk(&path, &other_digest).unwrap().is_none());
    }

    #[test]
    fn missing_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.bin");
        let digest = ManifestDigest::from_bytes([0u8; 32]);
        assert!(load_from_disk(&path, &digest).unwrap().is_none());
    }

    #[test]
    fn bad_magic_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("garbage.bin");
        std::fs::write(&path, b"not a zfb-graph file at all, just bytes").unwrap();
        let digest = ManifestDigest::from_bytes([0u8; 32]);
        assert!(load_from_disk(&path, &digest).unwrap().is_none());
    }

    #[test]
    fn manifest_digest_changes_when_file_content_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("pages")).unwrap();
        let cfg = root.join("zfb.config.json");
        std::fs::write(&cfg, b"{\"x\":1}").unwrap();

        let watch = vec![PathBuf::from("pages")];
        let cfgs = vec![PathBuf::from("zfb.config.json")];

        let d1 = ManifestDigest::compute(root, &watch, &cfgs).unwrap();

        // Change config content → digest must flip.
        std::fs::write(&cfg, b"{\"x\":2}").unwrap();
        let d2 = ManifestDigest::compute(root, &watch, &cfgs).unwrap();
        assert_ne!(d1, d2);
    }

    #[test]
    fn manifest_digest_changes_when_watched_file_added() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("pages")).unwrap();

        let watch = vec![PathBuf::from("pages")];
        let cfgs: Vec<PathBuf> = vec![];

        let d1 = ManifestDigest::compute(root, &watch, &cfgs).unwrap();
        std::fs::write(root.join("pages").join("new.tsx"), b"export {}").unwrap();
        let d2 = ManifestDigest::compute(root, &watch, &cfgs).unwrap();
        assert_ne!(d1, d2);
    }

    #[test]
    fn manifest_digest_is_stable_across_calls() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::write(root.join("pages").join("a.tsx"), b"a").unwrap();
        std::fs::write(root.join("pages").join("b.tsx"), b"bb").unwrap();

        let watch = vec![PathBuf::from("pages")];
        let cfgs: Vec<PathBuf> = vec![];

        let d1 = ManifestDigest::compute(root, &watch, &cfgs).unwrap();
        let d2 = ManifestDigest::compute(root, &watch, &cfgs).unwrap();
        assert_eq!(d1, d2);
    }

    #[test]
    fn missing_watch_root_is_skipped_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let watch = vec![PathBuf::from("nope-not-here")];
        let cfgs: Vec<PathBuf> = vec![];
        // Should not error and should be reproducible.
        let d = ManifestDigest::compute(root, &watch, &cfgs).unwrap();
        assert_eq!(d, ManifestDigest::compute(root, &watch, &cfgs).unwrap());
    }

    /// Acceptance evidence (sub-task 5 of issue #55): a non-trivial
    /// previously-persisted graph reloads from disk in well under
    /// 50ms when nothing has changed. Modelled on a basic-blog-sized
    /// site (~50 pages, modest dep counts each) — comfortably
    /// larger than the example we ship while still fast to set up.
    #[test]
    fn cold_load_of_realistic_graph_is_under_50ms() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("graph.bin");
        let digest = ManifestDigest::from_bytes([42u8; 32]);

        let mut g = DependencyGraph::new();
        for i in 0..50 {
            let page = pid(&format!("/pages/p{i}.tsx"));
            let mut deps = Vec::with_capacity(8);
            // A handful of shared layout/component/css deps per
            // page — exercises both forward and reverse indexes.
            deps.push((p("/layouts/main.tsx"), DepKind::Module));
            deps.push((p("/components/header.tsx"), DepKind::Module));
            deps.push((p("/components/footer.tsx"), DepKind::Module));
            deps.push((p("/styles/site.css"), DepKind::Style));
            for j in 0..4 {
                deps.push((
                    PathBuf::from(format!("/components/widget-{i}-{j}.tsx")),
                    DepKind::Module,
                ));
            }
            g.upsert(PageDeps::new(page, deps));
        }
        save_to_disk(&g, &digest, &path).unwrap();

        let started = std::time::Instant::now();
        let loaded = load_from_disk(&path, &digest)
            .expect("load")
            .expect("present");
        let elapsed = started.elapsed();
        assert_eq!(loaded, g);
        // Generous ceiling for noisy CI; the typical local run is
        // sub-millisecond.
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "graph reload took {elapsed:?}, expected < 50ms",
        );
    }

    #[test]
    fn manifest_digest_handles_single_file_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cfg = root.join("zfb.config.ts");
        std::fs::write(&cfg, b"export default {}").unwrap();

        // Pass the config file path as a *watch root* (not as a
        // config_file): the digest builder should treat single-file
        // roots correctly without crashing or skipping.
        let watch = vec![PathBuf::from("zfb.config.ts")];
        let cfgs: Vec<PathBuf> = vec![];

        let d1 = ManifestDigest::compute(root, &watch, &cfgs).unwrap();
        std::fs::write(&cfg, b"export default { changed: true }").unwrap();
        let d2 = ManifestDigest::compute(root, &watch, &cfgs).unwrap();
        assert_ne!(d1, d2);
    }
}
