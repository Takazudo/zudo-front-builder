//! Shared lexical path normalisation for cache keys and fingerprint
//! digests (zfb#939).
//!
//! Every place a filesystem path enters the MDX compile-cache keying
//! surface — the `ResolveLinksPlugin` source-map digest in
//! [`Pipeline::add_resolve_links`] and the per-file `source_dir`
//! key-context in [`Pipeline::cache_key_context`] — must spell paths
//! identically, or two logically-equal paths split into two cache
//! entries (spurious miss) while sloppier schemes could let two
//! different paths collide (stale hit). This module is the ONE
//! normalisation helper both sites share.
//!
//! **Lexical, not filesystem-backed.** `std::fs::canonicalize` requires
//! the path to exist and follows symlinks — it can fail mid-tick for a
//! just-removed file and makes hashing fallible. The correctness
//! requirement here is *consistency* (same spelling rules everywhere),
//! not perfect symlink resolution: if two spellings of one dir slip
//! past lexical normalisation, the result is a spurious cache miss
//! (safe), never a wrong hit.
//!
//! [`Pipeline::add_resolve_links`]: crate::pipeline::Pipeline::add_resolve_links
//! [`Pipeline::cache_key_context`]: crate::pipeline::Pipeline::cache_key_context

use std::path::{Component, Path};

/// Normalise a path lexically into a `/`-separated string.
///
/// - separators are normalised to `/` (Windows `\` included, via
///   [`Path::components`]);
/// - repeated separators and interior `.` components are dropped;
/// - `..` pops the previous normal component; a `..` that would climb
///   above a relative path's start is kept (`../a` stays `../a`), and a
///   `..` at an absolute path's root is dropped (root's parent is
///   root);
/// - trailing separators are dropped (`/a/b/` == `/a/b`);
/// - a Windows drive/UNC prefix is preserved verbatim ahead of the
///   normalised remainder.
///
/// Non-UTF-8 components are converted lossily — content paths in zfb
/// projects are UTF-8 in practice, and the surrounding code (`display`,
/// `to_str`) already assumes as much.
pub(crate) fn normalize_path_lexically(path: &Path) -> String {
    let mut prefix: Option<String> = None;
    let mut absolute = false;
    let mut parts: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(p) => {
                prefix = Some(p.as_os_str().to_string_lossy().into_owned());
            }
            Component::RootDir => absolute = true,
            Component::CurDir => {}
            Component::ParentDir => match parts.last().map(String::as_str) {
                // A retained leading `..` cannot be popped — climbing
                // continues above the path's start.
                Some("..") => parts.push("..".to_string()),
                Some(_) => {
                    parts.pop();
                }
                // `/..` is `/`; a relative path keeps its leading `..`.
                None if absolute => {}
                None => parts.push("..".to_string()),
            },
            Component::Normal(c) => parts.push(c.to_string_lossy().into_owned()),
        }
    }
    let mut out = prefix.unwrap_or_default();
    if absolute {
        out.push('/');
    }
    out.push_str(&parts.join("/"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn norm(s: &str) -> String {
        normalize_path_lexically(&PathBuf::from(s))
    }

    #[test]
    fn identical_spellings_of_one_path_normalise_identically() {
        assert_eq!(norm("/a/b/c"), "/a/b/c");
        assert_eq!(norm("/a/./b/c"), "/a/b/c");
        assert_eq!(norm("/a//b/c/"), "/a/b/c");
        assert_eq!(norm("/a/x/../b/c"), "/a/b/c");
        assert_eq!(norm("/a/b/c/."), "/a/b/c");
    }

    #[test]
    fn different_paths_stay_different() {
        assert_ne!(norm("/a/b"), norm("/a/c"));
        assert_ne!(norm("/a/b"), norm("a/b"));
        assert_ne!(norm("/a"), norm("/a/b"));
    }

    #[test]
    fn relative_paths_keep_leading_parent_components() {
        assert_eq!(norm("../a"), "../a");
        assert_eq!(norm("../../a/b"), "../../a/b");
        assert_eq!(norm("a/../../b"), "../b");
        assert_eq!(norm("./a/b"), "a/b");
    }

    #[test]
    fn parent_of_root_is_root() {
        assert_eq!(norm("/../a"), "/a");
        assert_eq!(norm("/.."), "/");
    }

    #[test]
    fn empty_and_dot_normalise_to_empty() {
        assert_eq!(norm(""), "");
        assert_eq!(norm("."), "");
    }
}
