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
//! **The canonical form mirrors `std::path::Path` equality, no more,
//! no less.** The plugin's runtime lookups go through a
//! `HashMap<PathBuf, String>`, whose Eq/Hash semantics are
//! component-wise: separator runs, interior `.` components, and
//! trailing separators collapse, while `..` and a leading `./` are
//! preserved. The digest/key canonicalisation must merge exactly the
//! spellings the runtime merges — collapsing MORE (e.g. popping `..`)
//! would let two maps that resolve links differently share a
//! fingerprint and serve a stale hit; collapsing LESS only costs a
//! spurious miss (safe).
//!
//! **Lexical, not filesystem-backed.** `std::fs::canonicalize` requires
//! the path to exist and follows symlinks — it can fail mid-tick for a
//! just-removed file and makes hashing fallible. The correctness
//! requirement here is *consistency* with the runtime lookup semantics,
//! not perfect symlink resolution.
//!
//! [`Pipeline::add_resolve_links`]: crate::pipeline::Pipeline::add_resolve_links
//! [`Pipeline::cache_key_context`]: crate::pipeline::Pipeline::cache_key_context

use std::path::{Component, Path};

/// Render a path into its canonical `/`-separated string — string
/// equality of two outputs is equivalent to `Path` equality of the two
/// inputs (for UTF-8 paths).
///
/// - separators are normalised to `/` (Windows `\` included, via
///   [`Path::components`]);
/// - repeated separators, interior `.` components, and trailing
///   separators are dropped (`Path` equality drops them too);
/// - `..` components and a leading `./` are RENDERED VERBATIM —
///   `Path::new("/a/x/../b") != Path::new("/a/b")`, and the runtime
///   source-map `HashMap<PathBuf, _>` distinguishes them the same way,
///   so the cache keying must as well (see module docs);
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
            // `Path::components()` already drops interior `.` — only a
            // leading one survives, and `Path` equality keeps it
            // (`./a != a`), so render it.
            Component::CurDir => parts.push(".".to_string()),
            Component::ParentDir => parts.push("..".to_string()),
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

    /// The load-bearing property: output equality ⟺ `Path` equality.
    /// Pinned directly so the merge rules can never drift from the
    /// runtime `HashMap<PathBuf, _>` lookup semantics.
    #[test]
    fn output_equality_matches_path_equality() {
        let spellings = [
            "/a/b/c",
            "/a/./b/c",
            "/a//b/c/",
            "/a/b/c/.",
            "/a/x/../b/c",
            "/../a",
            "/a",
            "a/b",
            "./a/b",
            "a/./b",
            "../a",
            "a/../b",
            "",
            ".",
        ];
        for x in &spellings {
            for y in &spellings {
                assert_eq!(
                    norm(x) == norm(y),
                    PathBuf::from(x) == PathBuf::from(y),
                    "normalisation must merge `{x}` and `{y}` exactly when Path equality does"
                );
            }
        }
    }

    #[test]
    fn spellings_path_treats_as_equal_normalise_identically() {
        assert_eq!(norm("/a/b/c"), "/a/b/c");
        assert_eq!(norm("/a/./b/c"), "/a/b/c");
        assert_eq!(norm("/a//b/c/"), "/a/b/c");
        assert_eq!(norm("/a/b/c/."), "/a/b/c");
        assert_eq!(norm("a/./b"), "a/b");
    }

    #[test]
    fn spellings_path_distinguishes_stay_distinct() {
        // `..` is preserved: the runtime map lookup distinguishes it, so
        // collapsing it here could merge two cache keys whose compiles
        // resolve links differently (stale hit — the unsafe direction).
        assert_ne!(norm("/a/x/../b"), norm("/a/b"));
        assert_eq!(norm("/a/x/../b"), "/a/x/../b");
        // Leading `./` is preserved for the same reason.
        assert_ne!(norm("./a"), norm("a"));
        assert_eq!(norm("./a"), "./a");
        assert_ne!(norm("/a/b"), norm("/a/c"));
        assert_ne!(norm("/a/b"), norm("a/b"));
        assert_ne!(norm("/a"), norm("/a/b"));
        assert_ne!(norm(""), norm("."));
    }

    #[test]
    fn relative_parent_components_render_verbatim() {
        assert_eq!(norm("../a"), "../a");
        assert_eq!(norm("../../a/b"), "../../a/b");
        assert_eq!(norm("a/../../b"), "a/../../b");
    }
}
