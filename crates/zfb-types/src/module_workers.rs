//! Stable module-worker asset naming and cache-key helpers.
//!
//! These helpers live in `zfb-types` so the browser bundle pipelines and the
//! SSR shadow pass use exactly the same contract without introducing a
//! dependency between `zfb-build` and `zfb-islands`.

use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::normalize_path_lexical;

/// Prefix reserved for emitted module-worker companions.
pub const MODULE_WORKER_FILENAME_PREFIX: &str = "worker-";

/// CSP `_headers` glob matching every module-worker companion.
pub const MODULE_WORKER_CSP_GLOB: &str = "/assets/worker-*.js";

/// A worker source cannot be expressed as a project-relative asset name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleWorkerPathError {
    project_root: PathBuf,
    source_path: PathBuf,
}

impl std::fmt::Display for ModuleWorkerPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "module-worker source {} is not a non-empty path beneath project root {}",
            self.source_path.display(),
            self.project_root.display()
        )
    }
}

impl std::error::Error for ModuleWorkerPathError {}

/// Return the stable flat filename for a project-local module-worker source.
///
/// The name is a pure function of the complete project-relative source path:
/// path separators and dots become dashes, while the remaining portable
/// filename characters are retained. A short hash of the unsanitized relative
/// path is appended so otherwise-ambiguous spellings (`a/b.ts` and `a-b.ts`)
/// remain collision-free without any discovery-order suffix. For example,
/// `src/search/index.worker.ts` becomes
/// `worker-src-search-index-worker-ts-<path-hash>.js`.
///
/// This helper deliberately does not inspect the emitted bundle. The SSR pass
/// runs before client asset discovery, so an emission-order-based dedupe name
/// would make the server and browser rewrites disagree.
pub fn module_worker_filename(
    project_root: &Path,
    source_path: &Path,
) -> Result<String, ModuleWorkerPathError> {
    let root = normalize_path_lexical(project_root);
    let source = normalize_path_lexical(source_path);
    let relative = source
        .strip_prefix(&root)
        .ok()
        .filter(|relative| {
            !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)))
        })
        .ok_or_else(|| ModuleWorkerPathError {
            project_root: project_root.to_path_buf(),
            source_path: source_path.to_path_buf(),
        })?;

    let mut sanitized = String::new();
    let mut logical_relative = String::new();
    for (component_index, component) in relative.components().enumerate() {
        if component_index > 0 {
            sanitized.push('-');
            logical_relative.push('/');
        }
        let Component::Normal(value) = component else {
            unreachable!("relative path was validated above")
        };
        let value = value.to_string_lossy();
        logical_relative.push_str(&value);
        for ch in value.chars() {
            match ch {
                '.' => sanitized.push('-'),
                ch if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') => sanitized.push(ch),
                // Keep the filename portable and deterministic for unusual
                // source names. Repeated punctuation is intentionally not
                // collapsed: doing so would create avoidable collisions.
                _ => sanitized.push('-'),
            }
        }
    }

    if sanitized.is_empty() {
        return Err(ModuleWorkerPathError {
            project_root: project_root.to_path_buf(),
            source_path: source_path.to_path_buf(),
        });
    }
    let path_digest = hex::encode(Sha256::digest(logical_relative.as_bytes()));
    Ok(format!(
        "{MODULE_WORKER_FILENAME_PREFIX}{sanitized}-{}.js",
        &path_digest[..8]
    ))
}

/// Return the deterministic eight-hex cache key for worker content bytes.
///
/// The emitted filename remains stable for CSP matching; callers append this
/// content key as `?v=<hash>` to the rewritten `new URL(...)` argument. The
/// caller decides the content envelope: zfb-build supplies a deterministic,
/// length-prefixed serialization of the complete first-party worker graph so
/// transitive edits invalidate the URL too. Eight lowercase hex characters
/// match zfb's existing content-address convention.
pub fn module_worker_content_hash(content: &[u8]) -> String {
    let digest = hex::encode(Sha256::digest(content));
    digest[..8].to_string()
}

/// Build the relative URL written into a matched module-worker constructor.
///
/// The worker companion is flat beside its owning browser entry, so the URL
/// always starts with `./`. The path-derived filename is stable while the
/// caller-supplied content query changes whenever the worker graph changes.
pub fn module_worker_url_specifier(
    project_root: &Path,
    source_path: &Path,
    content_hash: &str,
) -> Result<String, ModuleWorkerPathError> {
    Ok(format!(
        "./{}?v={}",
        module_worker_filename(project_root, source_path)?,
        content_hash
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_is_pure_path_derived_and_sanitized() {
        let root = Path::new("/app");
        let source = root.join("src/search/index.worker.ts");
        let filename = module_worker_filename(root, &source).unwrap();
        assert!(filename.starts_with("worker-src-search-index-worker-ts-"));
        assert!(filename.ends_with(".js"));
        assert_eq!(
            filename,
            module_worker_filename(root, &source).unwrap(),
            "the same source must always produce the same filename"
        );
    }

    #[test]
    fn complete_nested_path_avoids_same_basename_collision() {
        let root = Path::new("/app");
        let left = module_worker_filename(root, &root.join("src/a/worker.ts")).unwrap();
        let right = module_worker_filename(root, &root.join("src/b/worker.ts")).unwrap();
        assert!(left.starts_with("worker-src-a-worker-ts-"));
        assert!(right.starts_with("worker-src-b-worker-ts-"));
        assert_ne!(left, right);
    }

    #[test]
    fn path_fingerprint_disambiguates_sanitized_collision() {
        let root = Path::new("/app");
        let nested = module_worker_filename(root, &root.join("a/b.ts")).unwrap();
        let dashed = module_worker_filename(root, &root.join("a-b.ts")).unwrap();
        assert_ne!(nested, dashed);
        assert!(nested.starts_with("worker-a-b-ts-"));
        assert!(dashed.starts_with("worker-a-b-ts-"));
    }

    #[test]
    fn source_outside_project_is_rejected() {
        let error =
            module_worker_filename(Path::new("/app"), Path::new("/other/worker.ts")).unwrap_err();
        assert!(error
            .to_string()
            .contains("not a non-empty path beneath project root"));
    }

    #[test]
    fn content_hash_and_url_are_deterministic() {
        let root = Path::new("/app");
        let source_path = root.join("src/worker.ts");
        let first_hash = module_worker_content_hash(b"postMessage('a')");
        let changed_hash = module_worker_content_hash(b"postMessage('b')");
        let first = module_worker_url_specifier(root, &source_path, &first_hash).unwrap();
        let second = module_worker_url_specifier(root, &source_path, &first_hash).unwrap();
        let changed = module_worker_url_specifier(root, &source_path, &changed_hash).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("./worker-src-worker-ts-"));
        assert!(first.contains(".js?v="));
        assert_ne!(first, changed);
        assert_eq!(module_worker_content_hash(b"x").len(), 8);
    }

    #[test]
    fn csp_glob_tracks_reserved_prefix() {
        assert_eq!(MODULE_WORKER_CSP_GLOB, "/assets/worker-*.js");
        assert!(
            module_worker_filename(Path::new("/app"), Path::new("/app/a.ts"))
                .unwrap()
                .starts_with(MODULE_WORKER_FILENAME_PREFIX)
        );
    }
}
