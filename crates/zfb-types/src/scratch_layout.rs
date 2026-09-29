//! Single source of truth for every generated-artifact path zfb owns.
//!
//! Pure path arithmetic: no I/O. See `research/3318-scratch-dir-design.md`
//! (Decision 4).

use std::path::{Path, PathBuf};

pub const DEFAULT_SCRATCH_DIR_NAME: &str = ".zfb-build";
pub const DEFAULT_STAGING_DIR_NAME: &str = ".zfb";
pub const SCRATCH_LOCK_FILE_NAME: &str = ".zfb-scratch.lock";
pub const DEV_PAGES_DIR_NAME: &str = "dev-pages";
pub const DEV_ASSETS_DIR_NAME: &str = "dev-assets";
pub const LIVENESS_PROBE_DIR_NAME: &str = "watcher-liveness-probe";
pub const PLUGIN_SCRATCH_DIR_NAME: &str = "plugins";
pub const GRAPH_BIN_FILE_NAME: &str = "graph.bin";
pub const RUNTIME_BUNDLE_BASENAME: &str = "bundle-runtime.mjs";
pub const RESERVED_CHILD_NAMES: &[&str] = &[
    DEV_PAGES_DIR_NAME,
    DEV_ASSETS_DIR_NAME,
    LIVENESS_PROBE_DIR_NAME,
    PLUGIN_SCRATCH_DIR_NAME,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScratchLayout {
    project_root: PathBuf,
    root: PathBuf,
    session: bool,
}

impl ScratchLayout {
    pub fn default_for(project_root: &Path) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
            root: project_root.join(DEFAULT_SCRATCH_DIR_NAME),
            session: false,
        }
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_session(&self) -> bool {
        self.session
    }

    pub fn bundle_outdir(&self) -> PathBuf {
        self.root.clone()
    }

    pub fn dev_pages_root(&self) -> PathBuf {
        self.root.join(DEV_PAGES_DIR_NAME)
    }

    pub fn dev_assets_root(&self) -> PathBuf {
        self.root.join(DEV_ASSETS_DIR_NAME)
    }

    pub fn liveness_probe_dir(&self) -> PathBuf {
        self.root.join(LIVENESS_PROBE_DIR_NAME)
    }

    /// Reserved for plugin scratch; zfb never creates it.
    pub fn plugin_scratch_dir(&self) -> PathBuf {
        self.root.join(PLUGIN_SCRATCH_DIR_NAME)
    }

    pub fn graph_bin(&self) -> PathBuf {
        if self.session {
            self.root.join(GRAPH_BIN_FILE_NAME)
        } else {
            self.project_root
                .join(DEFAULT_STAGING_DIR_NAME)
                .join(GRAPH_BIN_FILE_NAME)
        }
    }

    pub fn written_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![
            self.project_root.join(DEFAULT_STAGING_DIR_NAME),
            self.project_root.join(DEFAULT_SCRATCH_DIR_NAME),
        ];
        if self.session {
            roots.push(self.root.clone());
        }
        roots
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_pins_every_accessor() {
        let p = Path::new("/tmp/proj");
        let l = ScratchLayout::default_for(p);
        assert!(!l.is_session());
        assert_eq!(l.project_root(), p);
        assert_eq!(l.root(), p.join(".zfb-build"));
        assert_eq!(l.bundle_outdir(), p.join(".zfb-build"));
        assert_eq!(l.dev_pages_root(), p.join(".zfb-build/dev-pages"));
        assert_eq!(l.dev_assets_root(), p.join(".zfb-build/dev-assets"));
        assert_eq!(
            l.liveness_probe_dir(),
            p.join(".zfb-build/watcher-liveness-probe")
        );
        assert_eq!(l.plugin_scratch_dir(), p.join(".zfb-build/plugins"));
        assert_eq!(l.graph_bin(), p.join(".zfb/graph.bin"));
        assert_eq!(
            l.written_roots(),
            vec![p.join(".zfb"), p.join(".zfb-build")]
        );
    }

    #[test]
    fn session_layout_relocates_graph_and_adds_written_root() {
        let p = Path::new("/tmp/proj");
        let l = ScratchLayout {
            project_root: p.to_path_buf(),
            root: PathBuf::from("/tmp/scratch"),
            session: true,
        };
        assert_eq!(l.graph_bin(), PathBuf::from("/tmp/scratch/graph.bin"));
        assert_eq!(
            l.written_roots(),
            vec![
                p.join(".zfb"),
                p.join(".zfb-build"),
                PathBuf::from("/tmp/scratch")
            ]
        );
    }
}
