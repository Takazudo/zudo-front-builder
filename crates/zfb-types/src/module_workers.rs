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
/// The name is a pure, injective function of the complete project-relative
/// source path. Portable ASCII letters, digits, and underscores are retained;
/// separators, dots, and literal hyphens use distinct dash-delimited escape
/// tokens, and every other raw path byte is hex-escaped. Unlike a truncated
/// digest, this encoding is reversible and therefore collision-free by
/// construction, including for non-UTF-8 Unix filenames. For example,
/// `src/search/index.worker.ts` becomes
/// `worker-src-s-search-s-index-d-worker-d-ts.js`.
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

    let mut encoded = String::new();
    for (component_index, component) in relative.components().enumerate() {
        if component_index > 0 {
            encoded.push_str("-s-");
        }
        let Component::Normal(value) = component else {
            unreachable!("relative path was validated above")
        };
        encode_os_component(value, &mut encoded);
    }

    if encoded.is_empty() {
        return Err(ModuleWorkerPathError {
            project_root: project_root.to_path_buf(),
            source_path: source_path.to_path_buf(),
        });
    }
    Ok(format!("{MODULE_WORKER_FILENAME_PREFIX}{encoded}.js"))
}

#[cfg(unix)]
fn encode_os_component(value: &std::ffi::OsStr, out: &mut String) {
    use std::os::unix::ffi::OsStrExt;

    for &byte in value.as_bytes() {
        encode_path_byte(byte, out);
    }
}

#[cfg(not(unix))]
fn encode_os_component(value: &std::ffi::OsStr, out: &mut String) {
    // Valid Unicode paths use UTF-8 on every host, keeping ordinary project
    // filenames byte-stable across platforms. Windows' rare ill-formed
    // UTF-16 spellings are encoded by code unit instead of going through a
    // lossy replacement character.
    if let Some(value) = value.to_str() {
        for &byte in value.as_bytes() {
            encode_path_byte(byte, out);
        }
        return;
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        for unit in value.encode_wide() {
            out.push_str(&format!("-u{unit:04x}-"));
        }
    }
}

fn encode_path_byte(byte: u8, out: &mut String) {
    match byte {
        b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' => out.push(byte as char),
        b'.' => out.push_str("-d-"),
        b'-' => out.push_str("-h-"),
        _ => out.push_str(&format!("-x{byte:02x}-")),
    }
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
    fn filename_is_pure_path_derived_and_injectively_encoded() {
        let root = Path::new("/app");
        let source = root.join("src/search/index.worker.ts");
        let filename = module_worker_filename(root, &source).unwrap();
        assert_eq!(filename, "worker-src-s-search-s-index-d-worker-d-ts.js");
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
        assert!(left.starts_with("worker-src-s-a-s-worker-d-ts"));
        assert!(right.starts_with("worker-src-s-b-s-worker-d-ts"));
        assert_ne!(left, right);
    }

    #[test]
    fn escape_tokens_disambiguate_previously_sanitized_collision() {
        let root = Path::new("/app");
        let nested = module_worker_filename(root, &root.join("a/b.ts")).unwrap();
        let dashed = module_worker_filename(root, &root.join("a-b.ts")).unwrap();
        assert_ne!(nested, dashed);
        assert_eq!(nested, "worker-a-s-b-d-ts.js");
        assert_eq!(dashed, "worker-a-h-b-d-ts.js");
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
        assert!(first.starts_with("./worker-src-s-worker-d-ts.js"));
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

    #[cfg(unix)]
    #[test]
    fn non_utf8_unix_paths_are_encoded_without_lossy_collisions() {
        use std::os::unix::ffi::OsStringExt;

        let root = Path::new("/app");
        let left = root.join(std::ffi::OsString::from_vec(vec![
            b'w', 0x80, b'.', b't', b's',
        ]));
        let right = root.join(std::ffi::OsString::from_vec(vec![
            b'w', 0x81, b'.', b't', b's',
        ]));
        let left_name = module_worker_filename(root, &left).unwrap();
        let right_name = module_worker_filename(root, &right).unwrap();
        assert_eq!(left_name, "worker-w-x80--d-ts.js");
        assert_eq!(right_name, "worker-w-x81--d-ts.js");
        assert_ne!(left_name, right_name);
    }
}
