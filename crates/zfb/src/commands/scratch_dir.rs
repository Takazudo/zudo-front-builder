//! Per-invocation scratch-dir resolution: `--scratch-dir` / `ZFB_SCRATCH_DIR`,
//! validation (R1-R6), creation, and the ownership lock.
//!
//! See `research/3318-scratch-dir-design.md` (Decisions 1-3). This module
//! never deletes anything: no reaping and no `remove_dir_all` of, or inside,
//! the scratch root.

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use zfb_types::helpers::{canonicalize_existing_prefix, normalize_path_lexical};
use zfb_types::scratch_layout::{
    ScratchLayout, DEFAULT_SCRATCH_DIR_NAME, RESERVED_CHILD_NAMES, SCRATCH_LOCK_FILE_NAME,
};

use crate::commands::dev::authored_watch_roots;
use crate::commands::resolve::resolve_under_root;
use crate::config::Config;

pub(crate) const SCRATCH_DIR_ENV: &str = "ZFB_SCRATCH_DIR";

// Prefixes/names the shadow-session reaper and the watcher-liveness-probe
// sweep delete (zfb-build bundler.rs, watcher_liveness_probe.rs).
const SHADOW_SESSION_PREFIX: &str = "zfb-shadow-session-";
const SWEPT_NAMES: &[&str] = &["zfb-dev-watcher-liveness-probe", "watcher-liveness-probe"];

/// The resolved scratch layout plus, for an explicit scratch dir, the held
/// ownership lock. Keep it alive until the command returns.
#[derive(Debug)]
pub(crate) struct ScratchLease {
    layout: ScratchLayout,
    _lock: Option<File>,
}

impl ScratchLease {
    pub(crate) fn layout(&self) -> &ScratchLayout {
        &self.layout
    }
}

/// flag > env > none. An empty or whitespace-only env value counts as unset.
pub(crate) fn select_scratch_dir(flag: Option<&Path>, env: Option<&OsStr>) -> Option<PathBuf> {
    if let Some(flag) = flag {
        return Some(flag.to_path_buf());
    }
    let env = env?;
    if env.to_string_lossy().trim().is_empty() {
        return None;
    }
    Some(PathBuf::from(env))
}

/// Convenience for command entrypoints: reads `ZFB_SCRATCH_DIR` and resolves.
pub(crate) fn resolve_from_env(
    project_root: &Path,
    cfg: &Config,
    effective_outdir: &Path,
    flag: Option<&Path>,
) -> Result<ScratchLease> {
    let selected = select_scratch_dir(flag, std::env::var_os(SCRATCH_DIR_ENV).as_deref());
    resolve_scratch(project_root, cfg, effective_outdir, selected.as_deref())
}

pub(crate) fn resolve_scratch(
    project_root: &Path,
    cfg: &Config,
    effective_outdir: &Path,
    selected: Option<&Path>,
) -> Result<ScratchLease> {
    let Some(selected) = selected else {
        return Ok(ScratchLease {
            layout: ScratchLayout::default_for(project_root),
            _lock: None,
        });
    };

    let abs = normalize_path_lexical(&resolve_under_root(project_root, selected));
    let prefix = canonicalize_existing_prefix(&abs)
        .with_context(|| format!("cannot resolve scratch dir {}", abs.display()))?;
    let guard = Guard::new(project_root, cfg, effective_outdir);
    guard.validate(&prefix)?;

    fs::create_dir_all(&prefix)
        .with_context(|| format!("failed to create scratch dir {}", prefix.display()))?;
    let canonical = fs::canonicalize(&prefix)
        .with_context(|| format!("failed to canonicalize scratch dir {}", prefix.display()))?;
    guard.validate(&canonical)?;

    check_ownership(&canonical)?;
    let lock = take_lock(&canonical)?;

    Ok(ScratchLease {
        layout: ScratchLayout::for_scratch_dir(project_root, canonical),
        _lock: Some(lock),
    })
}

struct Guard {
    root: PathBuf,
    out_dir: PathBuf,
    public_dir: PathBuf,
    watch_roots: Vec<PathBuf>,
}

fn canon(p: &Path) -> PathBuf {
    canonicalize_existing_prefix(p).unwrap_or_else(|| normalize_path_lexical(p))
}

fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

impl Guard {
    fn new(project_root: &Path, cfg: &Config, effective_outdir: &Path) -> Self {
        Self {
            root: canon(project_root),
            out_dir: canon(&resolve_under_root(project_root, effective_outdir)),
            public_dir: canon(&resolve_under_root(project_root, &cfg.public_dir)),
            watch_roots: authored_watch_roots(project_root, cfg)
                .iter()
                .map(|p| canon(p))
                .collect(),
        }
    }

    fn validate(&self, scratch: &Path) -> Result<()> {
        let show = scratch.display();

        if scratch.starts_with(&self.root) {
            let default_root = self.root.join(DEFAULT_SCRATCH_DIR_NAME);
            let rel = scratch.strip_prefix(&default_root).ok();
            match rel {
                Some(rel) if !rel.as_os_str().is_empty() => {
                    for comp in rel.components() {
                        let name = comp.as_os_str().to_string_lossy();
                        if RESERVED_CHILD_NAMES.contains(&name.as_ref()) {
                            bail!(
                                "scratch dir {show} rejected (R2): path component `{name}` under \
                                 {} is reserved for zfb's own subdirectories",
                                default_root.display()
                            );
                        }
                    }
                }
                _ => {
                    if scratch == self.root {
                        bail!(
                            "scratch dir {show} rejected (R1): it equals the project root {}",
                            self.root.display()
                        );
                    }
                    bail!(
                        "scratch dir {show} rejected (R2): inside the project it must be a \
                         strict descendant of {}; use `{}/<name>` or a path outside the project",
                        default_root.display(),
                        default_root.display()
                    );
                }
            }
        } else if self.root.starts_with(scratch) {
            bail!(
                "scratch dir {show} rejected (R1): it contains the project root {}",
                self.root.display()
            );
        }

        if overlaps(scratch, &self.out_dir) {
            bail!(
                "scratch dir {show} rejected (R3): overlaps the output directory {}",
                self.out_dir.display()
            );
        }
        if overlaps(scratch, &self.public_dir) {
            bail!(
                "scratch dir {show} rejected (R4): overlaps the public directory {}",
                self.public_dir.display()
            );
        }
        if let Some(w) = self.watch_roots.iter().find(|w| overlaps(scratch, w)) {
            bail!(
                "scratch dir {show} rejected (R5): overlaps the watch root {}",
                w.display()
            );
        }

        for ancestor in scratch.ancestors().skip(1) {
            if ancestor.join(SCRATCH_LOCK_FILE_NAME).exists() {
                bail!(
                    "scratch dir {show} rejected (R6): nested inside another scratch root {}",
                    ancestor.display()
                );
            }
        }
        for comp in scratch.components() {
            let name = comp.as_os_str().to_string_lossy();
            if name.starts_with(SHADOW_SESSION_PREFIX) || SWEPT_NAMES.contains(&name.as_ref()) {
                bail!(
                    "scratch dir {show} rejected (R6): path component `{name}` is swept by zfb's \
                     shadow-session reaper / watcher-probe cleanup"
                );
            }
        }
        Ok(())
    }
}

fn check_ownership(dir: &Path) -> Result<()> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("failed to read scratch dir {}", dir.display()))?;
    let mut any = false;
    for entry in &mut entries {
        let entry = entry?;
        if entry.file_name() == SCRATCH_LOCK_FILE_NAME {
            return Ok(());
        }
        any = true;
    }
    if any {
        bail!(
            "refusing to use non-empty directory {} that zfb did not create as a scratch dir \
             (no {SCRATCH_LOCK_FILE_NAME} marker)",
            dir.display()
        );
    }
    Ok(())
}

fn take_lock(dir: &Path) -> Result<File> {
    let lock_path = dir.join(SCRATCH_LOCK_FILE_NAME);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("failed to open scratch lock {}", lock_path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => bail!(
            "scratch dir {} is in use by another zfb process (lock {}); give each concurrent \
             command its own `--scratch-dir`",
            dir.display(),
            lock_path.display()
        ),
        Err(TryLockError::Error(e)) => bail!(
            "failed to lock scratch dir {} (lock {}): {e}; use a scratch dir on a local \
             filesystem",
            dir.display(),
            lock_path.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, PathBuf, Config) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap().join("proj");
        fs::create_dir_all(&root).unwrap();
        (tmp, root, Config::default())
    }

    fn resolve(root: &Path, cfg: &Config, p: &Path) -> Result<ScratchLease> {
        resolve_scratch(root, cfg, Path::new("dist"), Some(p))
    }

    fn err(root: &Path, cfg: &Config, p: &Path) -> String {
        format!("{:#}", resolve(root, cfg, p).unwrap_err())
    }

    #[test]
    fn selection_prefers_flag_and_ignores_blank_env() {
        let f = Path::new("/f");
        assert_eq!(
            select_scratch_dir(Some(f), Some(OsStr::new("/e"))),
            Some(PathBuf::from("/f"))
        );
        assert_eq!(
            select_scratch_dir(None, Some(OsStr::new("/e"))),
            Some(PathBuf::from("/e"))
        );
        assert_eq!(select_scratch_dir(None, Some(OsStr::new(""))), None);
        assert_eq!(select_scratch_dir(None, Some(OsStr::new("  \t"))), None);
        assert_eq!(select_scratch_dir(None, None), None);
    }

    #[test]
    fn default_path_has_no_lock_and_creates_nothing() {
        let (_t, root, cfg) = setup();
        let lease = resolve_scratch(&root, &cfg, Path::new("dist"), None).unwrap();
        assert!(!lease.layout().is_session());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    }

    #[test]
    fn relative_path_resolves_against_project_root() {
        let (_t, root, cfg) = setup();
        let lease = resolve(&root, &cfg, Path::new("../out/./s")).unwrap();
        let expected = root.parent().unwrap().join("out/s");
        assert_eq!(lease.layout().root(), expected.canonicalize().unwrap());
        assert!(lease.layout().is_session());
        assert!(expected.join(SCRATCH_LOCK_FILE_NAME).is_file());
    }

    #[test]
    fn accepts_zfb_build_descendant_and_outside_dir() {
        let (t, root, cfg) = setup();
        resolve(&root, &cfg, &root.join(".zfb-build/session-a")).unwrap();
        resolve(&root, &cfg, &t.path().join("elsewhere")).unwrap();
    }

    #[test]
    fn r1_rejects_root_parent_and_symlink_alias() {
        let (t, root, cfg) = setup();
        assert!(err(&root, &cfg, &root).contains("R1"));
        assert!(err(&root, &cfg, root.parent().unwrap()).contains("R1"));
        let alias = t.path().join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        assert!(err(&root, &cfg, &alias).contains("R1"));
    }

    #[test]
    fn r2_rejects_in_project_paths_outside_zfb_build_and_reserved_children() {
        let (_t, root, cfg) = setup();
        assert!(err(&root, &cfg, &root.join("tmp-scratch")).contains("R2"));
        assert!(err(&root, &cfg, &root.join(".zfb-build")).contains("R2"));
        assert!(err(&root, &cfg, &root.join(".zfb-build/dev-assets/x")).contains("R2"));
    }

    #[test]
    fn r3_rejects_outdir_overlap_including_symlinked_alias() {
        let (t, root, cfg) = setup();
        let inside = resolve_scratch(
            &root,
            &cfg,
            Path::new(".zfb-build/out"),
            Some(&root.join(".zfb-build/out/x")),
        );
        assert!(format!("{:#}", inside.unwrap_err()).contains("R3"));
        let real = t.path().join("real");
        fs::create_dir_all(&real).unwrap();
        let alias = t.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let out = real.join("dist");
        let msg = format!(
            "{:#}",
            resolve_scratch(&root, &cfg, &out, Some(&alias.join("dist/x"))).unwrap_err()
        );
        assert!(msg.contains("R3"), "{msg}");
        assert!(!out.exists() || fs::read_dir(&out).unwrap().count() == 0);
    }

    #[test]
    fn r4_rejects_public_dir_overlap() {
        let (t, root, mut cfg) = setup();
        cfg.public_dir = t.path().join("pub");
        assert!(err(&root, &cfg, &t.path().join("pub/s")).contains("R4"));
    }

    #[test]
    fn r5_rejects_watch_root_overlap() {
        let (t, root, mut cfg) = setup();
        let extra = t.path().join("watched");
        fs::create_dir_all(&extra).unwrap();
        cfg.extra_watch_paths = vec![extra.clone()];
        assert!(err(&root, &cfg, &extra.join("s")).contains("R5"));
        assert!(err(
            &root,
            &cfg,
            extra.parent().unwrap().join("watched").as_path()
        )
        .contains("R5"));
    }

    #[test]
    fn r6_rejects_nested_scratch_and_swept_names() {
        let (t, root, cfg) = setup();
        let outer = t.path().join("outer");
        resolve(&root, &cfg, &outer).unwrap();
        assert!(err(&root, &cfg, &outer.join("inner")).contains("R6"));
        assert!(err(&root, &cfg, &t.path().join("zfb-shadow-session-x")).contains("R6"));
        assert!(err(&root, &cfg, &t.path().join("watcher-liveness-probe/a")).contains("R6"));
    }

    #[test]
    fn lock_contention_is_a_hard_error() {
        let (t, root, cfg) = setup();
        let dir = t.path().join("s");
        let lease = resolve(&root, &cfg, &dir).unwrap();
        let probe = File::open(dir.join(SCRATCH_LOCK_FILE_NAME)).unwrap();
        assert!(probe.try_lock().is_err());
        assert!(err(&root, &cfg, &dir).contains("in use by another zfb process"));
        drop(lease);
        resolve(&root, &cfg, &dir).unwrap();
    }

    #[test]
    fn rejects_non_empty_unmarked_dir_and_preserves_reused_contents() {
        let (t, root, cfg) = setup();
        let dir = t.path().join("s");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("keep.txt"), b"data").unwrap();
        assert!(err(&root, &cfg, &dir).contains("did not create"));
        assert_eq!(fs::read(dir.join("keep.txt")).unwrap(), b"data");

        fs::write(dir.join(SCRATCH_LOCK_FILE_NAME), b"").unwrap();
        let lease = resolve(&root, &cfg, &dir).unwrap();
        assert_eq!(fs::read(dir.join("keep.txt")).unwrap(), b"data");
        drop(lease);
    }
}
