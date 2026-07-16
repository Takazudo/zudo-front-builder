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
    // lossy replacement character. (let-else so the diverging `return` stays
    // required on every target — on non-windows non-unix targets like wasm32
    // the cfg(windows) block compiles away, which made a trailing plain
    // `return` trip clippy::needless_return there.)
    let Some(utf8) = value.to_str() else {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            for unit in value.encode_wide() {
                out.push_str(&format!("-u{unit:04x}-"));
            }
        }
        return;
    };
    for &byte in utf8.as_bytes() {
        encode_path_byte(byte, out);
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

/// Workspace-scoped variant of [`module_worker_filename`] (issue #1673, the
/// #1500 flat-naming contract extended for issue #1667 workspace parity).
///
/// `first_party_root` is the widened first-party boundary returned by
/// [`crate::first_party_root_for`] — the workspace root when `project_root`
/// is a claimed pnpm workspace member, or `project_root` itself otherwise.
/// Project-local sources delegate unchanged to [`module_worker_filename`],
/// so every existing project-local name stays byte-identical. A source that
/// falls outside `project_root` but inside `first_party_root` (a workspace
/// sibling reached only through a tsconfig path alias) instead encodes its
/// *workspace*-relative path behind the `worker--ws-` prefix, using the same
/// injective per-byte encoding. A source outside `first_party_root` entirely
/// is rejected.
///
/// The `-ws-` marker is grammar-disjoint from every project-local encoding
/// by construction: the first token after the `worker-` prefix in a
/// project-local name is always a bare alnum/underscore byte or one of the
/// `-d-` / `-h-` / `-xNN-` escape tokens — never `-w`. See
/// `project_local_encoding_can_never_start_with_dash_w` below for the
/// exhaustive proof.
pub fn module_worker_filename_scoped(
    project_root: &Path,
    first_party_root: &Path,
    source_path: &Path,
) -> Result<String, ModuleWorkerPathError> {
    if let Ok(filename) = module_worker_filename(project_root, source_path) {
        return Ok(filename);
    }

    let error = || ModuleWorkerPathError {
        project_root: project_root.to_path_buf(),
        source_path: source_path.to_path_buf(),
    };

    let workspace_root = normalize_path_lexical(first_party_root);
    let source = normalize_path_lexical(source_path);
    let relative = source
        .strip_prefix(&workspace_root)
        .ok()
        .filter(|relative| {
            !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)))
        })
        .ok_or_else(error)?;

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
        return Err(error());
    }
    Ok(format!("{MODULE_WORKER_FILENAME_PREFIX}-ws-{encoded}.js"))
}

/// Workspace-scoped variant of [`module_worker_url_specifier`]; see
/// [`module_worker_filename_scoped`] for the naming contract.
pub fn module_worker_url_specifier_scoped(
    project_root: &Path,
    first_party_root: &Path,
    source_path: &Path,
    content_hash: &str,
) -> Result<String, ModuleWorkerPathError> {
    Ok(format!(
        "./{}?v={}",
        module_worker_filename_scoped(project_root, first_party_root, source_path)?,
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

    // ── scoped (workspace-parity, issue #1673) ──────────────────────────────

    #[test]
    fn scoped_project_local_source_is_byte_identical_to_unscoped() {
        let project_root = Path::new("/ws/apps/site");
        let workspace_root = Path::new("/ws");
        let source = project_root.join("src/search/index.worker.ts");

        let unscoped = module_worker_filename(project_root, &source).unwrap();
        let scoped = module_worker_filename_scoped(project_root, workspace_root, &source).unwrap();
        assert_eq!(scoped, unscoped);
        assert_eq!(scoped, "worker-src-s-search-s-index-d-worker-d-ts.js");
    }

    #[test]
    fn scoped_workspace_sibling_source_uses_ws_prefix_and_workspace_relative_path() {
        let project_root = Path::new("/ws/apps/site");
        let workspace_root = Path::new("/ws");
        // Reached only via a tsconfig path alias (e.g. `@shared/*`), not
        // beneath the project root.
        let source = workspace_root.join("packages/shared/src/worker.ts");

        let filename =
            module_worker_filename_scoped(project_root, workspace_root, &source).unwrap();
        assert_eq!(
            filename,
            "worker--ws-packages-s-shared-s-src-s-worker-d-ts.js"
        );
        assert!(filename.starts_with(MODULE_WORKER_FILENAME_PREFIX));

        // Deterministic: same source always yields the same name.
        assert_eq!(
            filename,
            module_worker_filename_scoped(project_root, workspace_root, &source).unwrap()
        );
    }

    #[test]
    fn scoped_workspace_root_itself_delegates_when_project_root_equals_workspace_root() {
        // No workspace widening (first_party_root == project_root): behavior
        // must be identical to the unscoped function, including rejection.
        let project_root = Path::new("/app");
        let error =
            module_worker_filename_scoped(project_root, project_root, Path::new("/other/w.ts"))
                .unwrap_err();
        assert!(error
            .to_string()
            .contains("not a non-empty path beneath project root"));
    }

    #[test]
    fn scoped_source_outside_first_party_root_is_rejected() {
        let project_root = Path::new("/ws/apps/site");
        let workspace_root = Path::new("/ws");
        let error = module_worker_filename_scoped(
            project_root,
            workspace_root,
            Path::new("/elsewhere/worker.ts"),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("not a non-empty path beneath project root"));
    }

    #[test]
    fn scoped_workspace_root_source_itself_is_rejected() {
        // The workspace root path with nothing beneath it encodes to an
        // empty relative path, same rejection as the unscoped function's
        // empty-relative-path case.
        let project_root = Path::new("/ws/apps/site");
        let workspace_root = Path::new("/ws");
        assert!(
            module_worker_filename_scoped(project_root, workspace_root, workspace_root).is_err()
        );
    }

    #[test]
    fn scoped_url_specifier_matches_scoped_filename_and_carries_hash() {
        let project_root = Path::new("/ws/apps/site");
        let workspace_root = Path::new("/ws");
        let source = workspace_root.join("packages/shared/src/worker.ts");
        let hash = module_worker_content_hash(b"postMessage('a')");

        let url = module_worker_url_specifier_scoped(project_root, workspace_root, &source, &hash)
            .unwrap();
        let filename =
            module_worker_filename_scoped(project_root, workspace_root, &source).unwrap();
        assert_eq!(url, format!("./{filename}?v={hash}"));
    }

    #[test]
    fn scoped_names_still_match_the_reserved_prefix_and_csp_glob() {
        let project_root = Path::new("/ws/apps/site");
        let workspace_root = Path::new("/ws");
        let source = workspace_root.join("packages/shared/worker.ts");
        let filename =
            module_worker_filename_scoped(project_root, workspace_root, &source).unwrap();

        assert!(filename.starts_with(MODULE_WORKER_FILENAME_PREFIX));
        // MODULE_WORKER_CSP_GLOB is "/assets/worker-*.js": `*` matches any
        // byte sequence including the embedded "-ws-" marker and further
        // dashes, so a scoped name under /assets/ still satisfies the glob.
        assert_eq!(MODULE_WORKER_CSP_GLOB, "/assets/worker-*.js");
        let simulated_asset_path = format!("/assets/{filename}");
        let (prefix, suffix) = ("/assets/worker-", ".js");
        assert!(simulated_asset_path.starts_with(prefix));
        assert!(simulated_asset_path.ends_with(suffix));
    }

    /// Grammar-level disjointness proof (issue #1673): exhaustively checks,
    /// for every possible input byte, that [`encode_path_byte`] never
    /// produces a token starting with `-w`. Combined with the fact that the
    /// inter-component separator is the fixed literal `-s-` (also not
    /// `-w`-prefixed) and is never emitted before the first component, this
    /// proves NO project-local encoding produced by [`module_worker_filename`]
    /// can ever start with `-w` immediately after the `worker-` prefix.
    /// [`module_worker_filename_scoped`]'s workspace-sibling branch always
    /// starts its encoded suffix with the literal `-ws-`, so the two name
    /// spaces can never collide.
    #[test]
    fn project_local_encoding_can_never_start_with_dash_w() {
        for byte in 0u8..=255 {
            let mut out = String::new();
            encode_path_byte(byte, &mut out);
            assert!(
                !out.starts_with("-w"),
                "byte {byte:#04x} encoded to {out:?}, which starts with the reserved -w marker"
            );
        }
        // The fixed component separator is likewise never `-w`-prefixed.
        assert!(!"-s-".starts_with("-w"));
    }

    /// Property-style sweep (issue #1673): for a broad, deterministically
    /// generated set of project-local and workspace-sibling source paths
    /// (including paths that already contain literal `-`, `.`, `s`, `w`,
    /// and non-ASCII bytes — the characters most likely to produce an
    /// accidental collision), no project-local name ever equals any
    /// workspace-scoped name.
    #[test]
    fn scoped_and_unscoped_namespaces_never_collide_across_generated_paths() {
        let project_root = Path::new("/ws/apps/site");
        let workspace_root = Path::new("/ws");

        let segments = [
            "a",
            "b",
            "worker",
            "ws",
            "w",
            "s",
            "index.worker.ts",
            "a-b",
            "a.b",
            "a--b",
            "-leading-dash",
            "unicode-\u{1F600}",
            "sub_dir",
            "-ws-literal",
        ];

        let mut project_local_names = std::collections::HashSet::new();
        let mut workspace_names = std::collections::HashSet::new();

        // Deterministic LCG so the sweep is reproducible without pulling in
        // a `rand`/`proptest` dependency for a single grammar test.
        let mut state: u64 = 0x9E3779B97F4A7C15;
        let mut next = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            state
        };

        for _ in 0..500 {
            let depth = 1 + (next() % 3) as usize;
            let mut project_relative = PathBuf::new();
            let mut workspace_relative = PathBuf::from("packages/shared");
            for _ in 0..depth {
                let segment = segments[(next() as usize) % segments.len()];
                project_relative.push(segment);
                workspace_relative.push(segment);
            }

            let project_local_source = project_root.join(&project_relative);
            let workspace_sibling_source = workspace_root.join(&workspace_relative);

            if let Ok(name) = module_worker_filename(project_root, &project_local_source) {
                project_local_names.insert(name);
            }
            if let Ok(name) = module_worker_filename_scoped(
                project_root,
                workspace_root,
                &workspace_sibling_source,
            ) {
                workspace_names.insert(name);
            }
        }

        assert!(!project_local_names.is_empty());
        assert!(!workspace_names.is_empty());
        let collisions: Vec<_> = project_local_names.intersection(&workspace_names).collect();
        assert!(
            collisions.is_empty(),
            "project-local and workspace-scoped names collided: {collisions:?}"
        );
        assert!(workspace_names
            .iter()
            .all(|name| name.starts_with(&format!("{MODULE_WORKER_FILENAME_PREFIX}-ws-"))));
    }
}
