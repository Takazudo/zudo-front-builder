//! Islands manifest — `MarkerName → resolved-tsx-path` JSON map.
//!
//! ## Stable contract
//!
//! The manifest is the data structure the **islands-bundling-shim** topic
//! consumes when generating the per-island esbuild entry points and the
//! browser-side hydration shim. Its on-disk format is a JSON object:
//!
//! ```json
//! {
//!   "Counter": "components/counter.tsx",
//!   "ThemeToggle": "components/theme-toggle.tsx"
//! }
//! ```
//!
//! - **Keys** are marker-name identities exactly as the scanner emits
//!   them: the value of [`crate::Island::marker_name`]. This is the same
//!   identity the runtime/bundling path keys on — the per-island marker
//!   the SSR side writes into `data-zfb-island`. A default-export island
//!   carries the identifier name as its `marker_name` (e.g. `"Foo"` for
//!   `export default function Foo`), so two distinct default exports do
//!   not collide on the literal `"default"` (their `component_name`).
//! - **Values** are the resolved source path. They are written either as
//!   the absolute path the resolver returned, or — when the caller passes a
//!   `root` to [`Manifest::relative_to`] / [`write_manifest`] — as the
//!   forward-slash-normalised relative path from `root` to the source.
//!   Relative paths are the form expected by downstream bundler entry
//!   points; absolute paths are kept available for debugging and tools that
//!   stand outside the project root.
//!
//! ## Stability guarantees
//!
//! - **Deterministic key order**: keys are sorted lexicographically. The
//!   scanner already returns its [`crate::scanner::IslandsSet`] sorted by
//!   `(source_path, component_name)`; the manifest reorders to a single
//!   `BTreeMap<String, _>` keyed by marker name so JSON serialization is
//!   stable regardless of input order.
//! - **Forward-slash paths**: relative paths use `/` even on Windows hosts
//!   so the manifest is portable across the JS/TS bundler subprocess that
//!   reads it. Absolute paths preserve the host separator.
//! - **One value per key**: when two distinct source files produce the same
//!   marker name, the entry whose `source_path` sorts first wins. This
//!   matches the scanner's pre-existing `(source_path, name)` tie-break and
//!   keeps the manifest a flat `marker_name → path` map. A diagnostic
//!   helper, [`Manifest::collisions`], surfaces dropped entries so callers
//!   that want to fail loudly can do so; [`is_same_package_duplicate`]
//!   classifies each one so a package's own source/compiled duplicate can be
//!   dropped without a warning the author cannot act on.
//!
//! ## Downstream contract
//!
//! The bundling-shim topic relies on:
//!
//! - JSON object shape (no nesting, string values).
//! - The hydration shim emitted alongside the bundle does
//!   `querySelectorAll('[data-zfb-island]')` and
//!   `querySelectorAll('[data-zfb-island-skip-ssr]')` and looks up each
//!   element's marker attribute value as a key in this manifest, so the
//!   keys must match the values the SDK's `Island` JSX wrapper writes into
//!   the `data-zfb-island` / `data-zfb-island-skip-ssr` attributes.
//!
//! Do **not** change this format without coordinating with the
//! islands-bundling-shim topic.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use crate::scanner::IslandsSet;

/// A marker-name → resolved-source-path map.
///
/// See the module-level docs for the stable on-disk format and stability
/// guarantees.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    /// `MarkerName → path` entries.
    ///
    /// `BTreeMap` is the storage type because we need deterministic
    /// iteration order in JSON output and we never produce manifests with
    /// enough entries that hashing would matter.
    entries: BTreeMap<String, PathBuf>,
    /// Names dropped due to marker-name collisions across distinct
    /// source files. `(name, dropped_path, kept_path)`.
    collisions: Vec<Collision>,
}

/// A name-collision entry. Two `(source_path, marker_name)` pairs share
/// `marker_name` but came from different source paths; the manifest kept
/// `kept_path` (the first by sort order) and dropped `dropped_path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    /// The colliding marker name.
    pub name: String,
    /// The source path that did **not** make it into the manifest.
    pub dropped_path: PathBuf,
    /// The source path that **did** make it into the manifest.
    pub kept_path: PathBuf,
}

/// Module extensions an island can be authored or shipped in. Used when
/// probing a package for the counterpart of a staged copy — see
/// [`is_same_package_duplicate`].
const ISLAND_MODULE_EXTS: &[&str] = &["tsx", "ts", "jsx", "js", "mjs", "cjs", "mts", "cts"];

/// Upper bound on directory entries visited while probing one package for a
/// byte-identical copy. Every bail-out in the probe fails toward "actionable"
/// (the collision keeps warning), so exceeding the budget costs a warning that
/// could have been suppressed — never a suppressed warning that mattered.
const PACKAGE_PROBE_ENTRY_BUDGET: usize = 20_000;

/// Upper bound on the size of a file read for byte comparison during the
/// probe. An island module larger than this is pathological.
const PACKAGE_PROBE_MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// True when the two participants of `collision` are two spellings of the
/// **same npm package's** module rather than two genuinely different
/// components that happen to share a marker name (issue #2441).
///
/// A package that ships both its compiled `dist/` output and its sources can
/// have the same component reach the scanner twice, through two entry graphs
/// — zudo-doc's `packageOwnedRoutes` does exactly this: a page re-exporting
/// `@takazudo/zudo-doc/routes/index` pulls in the compiled
/// `dist/routes/_design-token-panel-bootstrap.js` while the routes plugin
/// separately injects a staged copy of the package's own
/// `routes-src/_design-token-panel-bootstrap.tsx`. Both are the same
/// component, so hydration is correct whichever the manifest keeps, and the
/// warning's remediation ("rename one component") is not actionable: both
/// participants live inside a dependency.
///
/// Membership is established without guessing from names or path shapes:
///
/// - both participants resolve inside the same `node_modules/<pkg>` root, or
/// - one resolves inside package root `P` and the other is **byte-identical**
///   to a file `P` ships under the same file stem — the branch that catches a
///   staged or mirrored copy made outside `node_modules`.
///
/// The byte comparison is what keeps this sound. The issue #999 motivating
/// case — a user-authored `components/theme-toggle.tsx` colliding with a
/// package's `dist/theme-toggle.js` — is not byte-identical to anything the
/// package ships, so it stays loud, which is right: there the advice to rename
/// is real advice.
///
/// Accepted trade-off: a package that genuinely ships two DIFFERENT components
/// under one marker name is suppressed too. That is deliberate — it is an
/// upstream bug, and the consumer whose build log this is cannot rename either
/// one. Suppression is about who can act, not about how confident we are that
/// the two components match.
///
/// This is deliberately **not** part of [`Manifest::from_islands`], which
/// stays a pure data transform with no filesystem access.
pub fn is_same_package_duplicate(collision: &Collision) -> bool {
    match (
        package_root_of(&collision.kept_path),
        package_root_of(&collision.dropped_path),
    ) {
        // Two modules of the same installed package.
        (Some(kept), Some(dropped)) => canonical_or_self(&kept) == canonical_or_self(&dropped),
        // One module of a package, and a copy of that package's own source
        // staged outside `node_modules`.
        (Some(pkg), None) => is_copy_of_package_file(&collision.dropped_path, &pkg),
        (None, Some(pkg)) => is_copy_of_package_file(&collision.kept_path, &pkg),
        // Two first-party modules — always actionable.
        (None, None) => false,
    }
}

/// The `node_modules/<pkg>` (or `node_modules/@scope/<pkg>`) root `path` lives
/// under, or `None` when it is not inside an installed package.
///
/// The lexical spelling is tried first so a pnpm `node_modules/<pkg>` symlink
/// is attributed to the name the project imports; canonicalising is the
/// fallback for a path the scanner already resolved through the link.
fn package_root_of(path: &Path) -> Option<PathBuf> {
    lexical_package_root(path).or_else(|| {
        let canonical = std::fs::canonicalize(path).ok()?;
        lexical_package_root(&canonical)
    })
}

fn lexical_package_root(path: &Path) -> Option<PathBuf> {
    let components: Vec<Component> = path.components().collect();
    // The DEEPEST `node_modules` owns the package: a nested install
    // (`node_modules/a/node_modules/b`) attributes `b`, not `a`.
    let nm = components
        .iter()
        .rposition(|c| c.as_os_str() == "node_modules")?;
    let first = components.get(nm + 1)?;
    let mut root = PathBuf::new();
    for component in &components[..=nm] {
        root.push(component);
    }
    root.push(first);
    if first.as_os_str().to_string_lossy().starts_with('@') {
        root.push(components.get(nm + 2)?);
    }
    Some(root)
}

/// Canonicalise, falling back to the path itself when the filesystem call
/// fails (a synthetic path in an in-memory harness has no entry on disk).
fn canonical_or_self(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// True when `candidate` is a byte-for-byte copy of a module `package_root`
/// ships under the same file stem.
///
/// Only stem-matched files are read, so the walk normally reads nothing; the
/// whole probe runs at most once per collision, and a collision-free build
/// never reaches it. A project that DOES carry a standing collision pays one
/// bounded walk of one package per rebuild in `zfb dev` — small next to the
/// esbuild pass it sits beside, and the entry budget caps the worst case.
fn is_copy_of_package_file(candidate: &Path, package_root: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(candidate) else {
        return false;
    };
    if !meta.is_file() || meta.len() > PACKAGE_PROBE_MAX_FILE_BYTES {
        return false;
    }
    let Some(stem) = candidate.file_stem() else {
        return false;
    };
    let Ok(candidate_bytes) = std::fs::read(candidate) else {
        return false;
    };

    let mut budget = PACKAGE_PROBE_ENTRY_BUDGET;
    let walk = walkdir::WalkDir::new(canonical_or_self(package_root))
        .follow_links(false)
        .into_iter()
        // A package's own nested installs belong to a different package.
        .filter_entry(|entry| {
            entry.depth() == 0 || !entry.file_type().is_dir() || entry.file_name() != "node_modules"
        });
    for entry in walk {
        if budget == 0 {
            return false;
        }
        budget -= 1;
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.file_stem() != Some(stem) {
            continue;
        }
        let is_module = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ISLAND_MODULE_EXTS.contains(&ext));
        if !is_module {
            continue;
        }
        match entry.metadata() {
            Ok(entry_meta) if entry_meta.len() == meta.len() => {}
            _ => continue,
        }
        if std::fs::read(path).is_ok_and(|bytes| bytes == candidate_bytes) {
            return true;
        }
    }
    false
}

impl Manifest {
    /// Construct an empty manifest.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a manifest from a scanner [`IslandsSet`].
    ///
    /// The scanner's output is already deterministically ordered by
    /// `(source_path, component_name)`. We reorder by **marker name**
    /// ([`crate::Island::marker_name`]) and drop collisions
    /// deterministically (first by `source_path` sort order wins).
    /// Dropped entries are recorded in [`Manifest::collisions`].
    ///
    /// Keying on `marker_name` rather than `component_name` keeps the
    /// scanner-side manifest consistent with the marker-name identity
    /// model the runtime/bundling path uses: two valid default-export
    /// islands (both `component_name == "default"`) with distinct
    /// `marker_name`s are preserved as two entries, not collapsed.
    pub fn from_islands(islands: &IslandsSet) -> Self {
        let mut entries: BTreeMap<String, PathBuf> = BTreeMap::new();
        let mut collisions: Vec<Collision> = Vec::new();
        for island in islands {
            match entries.get(&island.marker_name) {
                None => {
                    entries.insert(island.marker_name.clone(), island.source_path.clone());
                }
                Some(kept) => {
                    if kept != &island.source_path {
                        collisions.push(Collision {
                            name: island.marker_name.clone(),
                            dropped_path: island.source_path.clone(),
                            kept_path: kept.clone(),
                        });
                    }
                }
            }
        }
        Self {
            entries,
            collisions,
        }
    }

    /// Number of entries in the manifest.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the manifest is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate `(name, path)` pairs in sorted order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Path)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_path()))
    }

    /// Look up the source path for a marker name.
    pub fn get(&self, name: &str) -> Option<&Path> {
        self.entries.get(name).map(PathBuf::as_path)
    }

    /// Read-only access to the collision diagnostic list.
    ///
    /// Not every entry is actionable — run each through
    /// [`is_same_package_duplicate`] before warning on it.
    pub fn collisions(&self) -> &[Collision] {
        &self.collisions
    }

    /// Return a new manifest whose paths are rendered relative to `root`,
    /// using forward slashes regardless of host OS. Paths that cannot be
    /// expressed relative to `root` (e.g. on a different drive) are kept
    /// in their original form.
    pub fn relative_to(&self, root: &Path) -> Self {
        let mut out: BTreeMap<String, PathBuf> = BTreeMap::new();
        for (name, path) in &self.entries {
            out.insert(name.clone(), to_forward_slash(&relative_path(path, root)));
        }
        Self {
            entries: out,
            collisions: self.collisions.clone(),
        }
    }

    /// Serialize to a stable, pretty-printed JSON string. The output is
    /// the on-disk wire format documented at the module level.
    pub fn to_json_pretty(&self) -> String {
        // Hand-roll a tiny serializer to avoid pulling in a `Serialize`
        // impl on `Manifest` while keeping the path-as-string conversion
        // explicit. We sort by key (BTreeMap iteration is already sorted)
        // and JSON-escape names + paths.
        if self.entries.is_empty() {
            return "{}\n".to_string();
        }
        let mut out = String::from("{\n");
        let mut first = true;
        for (name, path) in &self.entries {
            if !first {
                out.push_str(",\n");
            }
            first = false;
            out.push_str("  ");
            push_json_string(&mut out, name);
            out.push_str(": ");
            push_json_string(&mut out, &path.to_string_lossy());
        }
        out.push('\n');
        out.push('}');
        out.push('\n');
        out
    }
}

/// Convenience: build a manifest from an islands set, rebase paths to
/// `root`, and return the JSON string.
pub fn manifest_json(islands: &IslandsSet, root: Option<&Path>) -> String {
    let manifest = Manifest::from_islands(islands);
    let manifest = match root {
        Some(r) => manifest.relative_to(r),
        None => manifest,
    };
    manifest.to_json_pretty()
}

/// Convenience: build a manifest, rebase paths if `root` is given, and
/// write the JSON to `out_path`. Returns the [`Manifest`] for callers
/// that also want in-process access.
pub fn write_manifest(
    islands: &IslandsSet,
    root: Option<&Path>,
    out_path: &Path,
) -> std::io::Result<Manifest> {
    let manifest = Manifest::from_islands(islands);
    let to_write = match root {
        Some(r) => manifest.relative_to(r),
        None => manifest.clone(),
    };
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(out_path, to_write.to_json_pretty())?;
    Ok(manifest)
}

/// Compute `path` relative to `root` lexically — without consulting the
/// filesystem and without `..` escapes.
///
/// If `path` is not under `root` (different prefix), the original `path`
/// is returned unchanged. If `root` is empty, `path` is returned
/// unchanged. The returned path is host-separated; callers then funnel it
/// through [`to_forward_slash`] for the wire format.
fn relative_path(path: &Path, root: &Path) -> PathBuf {
    if root.as_os_str().is_empty() {
        return path.to_path_buf();
    }
    // Walk both component lists in lock-step. As long as they match, drop
    // the prefix from `path`. The first divergence ends the prefix walk;
    // everything else of `path` becomes the result.
    let mut p_iter = path.components().peekable();
    let mut r_iter = root.components().peekable();
    while let (Some(p), Some(r)) = (p_iter.peek(), r_iter.peek()) {
        if p == r {
            p_iter.next();
            r_iter.next();
        } else {
            break;
        }
    }
    if r_iter.peek().is_some() {
        // root has unmatched components — path is not under root. Return
        // the original path so the caller still gets something useful.
        return path.to_path_buf();
    }
    let rest: PathBuf = p_iter.collect();
    if rest.as_os_str().is_empty() {
        // path == root, return "."
        return PathBuf::from(".");
    }
    rest
}

/// Render a path with forward slashes, regardless of the host separator.
fn to_forward_slash(path: &Path) -> PathBuf {
    let mut out = String::new();
    let mut first = true;
    for comp in path.components() {
        match comp {
            Component::RootDir => {
                out.push('/');
                first = true;
                continue;
            }
            Component::Prefix(p) => {
                out.push_str(&p.as_os_str().to_string_lossy());
                first = true;
                continue;
            }
            Component::CurDir => {
                if !first {
                    out.push('/');
                }
                out.push('.');
            }
            Component::ParentDir => {
                if !first {
                    out.push('/');
                }
                out.push_str("..");
            }
            Component::Normal(name) => {
                if !first {
                    out.push('/');
                }
                out.push_str(&name.to_string_lossy());
            }
        }
        first = false;
    }
    PathBuf::from(out)
}

/// Append a JSON-encoded string literal (with surrounding quotes) to `out`.
fn push_json_string(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundler::Island;

    fn island(name: &str, path: &str) -> Island {
        Island::new(name, PathBuf::from(path))
    }

    fn island_marker(component: &str, path: &str, marker: &str) -> Island {
        Island::with_marker_name(component, PathBuf::from(path), marker)
    }

    // -- is_same_package_duplicate (#2441) ---------------------------------

    fn write_file(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, contents).expect("write");
    }

    fn collision_between(kept: &Path, dropped: &Path) -> Collision {
        Collision {
            name: "Widget".to_string(),
            kept_path: kept.to_path_buf(),
            dropped_path: dropped.to_path_buf(),
        }
    }

    /// The zudo-doc shape (#2441): the package's compiled `dist/` module and a
    /// verbatim copy of the package's own shipped source, staged OUTSIDE
    /// `node_modules` so a virtual-module import resolves. Same component,
    /// nothing for the author to rename.
    #[test]
    fn staged_verbatim_copy_of_a_package_source_is_a_same_package_duplicate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let source = r#""use client";
export function Widget() { return null; }
Widget.displayName = "Widget";
"#;
        let pkg = root.join("node_modules/@takazudo/zudo-doc");
        write_file(&pkg.join("routes-src/_widget.tsx"), source);
        write_file(
            &pkg.join("dist/routes/_widget.js"),
            "export function Widget() { return null; }\n",
        );
        let staged = root.join(".zudo-doc/routes-src/_widget.tsx");
        write_file(&staged, source);

        assert!(is_same_package_duplicate(&collision_between(
            &staged,
            &pkg.join("dist/routes/_widget.js"),
        )));
        // Orientation must not matter — the manifest keeps whichever path
        // sorts first, which is not under our control.
        assert!(is_same_package_duplicate(&collision_between(
            &pkg.join("dist/routes/_widget.js"),
            &staged,
        )));
    }

    /// A staged copy that DRIFTED from what the package ships is no longer
    /// provably the same component, so it must stay loud.
    #[test]
    fn staged_copy_that_differs_from_the_package_source_still_warns() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let pkg = root.join("node_modules/@takazudo/zudo-doc");
        write_file(
            &pkg.join("routes-src/_widget.tsx"),
            "export function Widget() { return null; }\n",
        );
        write_file(
            &pkg.join("dist/routes/_widget.js"),
            "export function Widget() { return null; }\n",
        );
        let staged = root.join(".zudo-doc/routes-src/_widget.tsx");
        write_file(&staged, "export function Widget() { return <div />; }\n");

        assert!(!is_same_package_duplicate(&collision_between(
            &staged,
            &pkg.join("dist/routes/_widget.js"),
        )));
    }

    /// Issue #999's motivating case: a user-authored component colliding with
    /// a package-provided one. Different components, and "rename one" is real
    /// advice — this must never be suppressed.
    #[test]
    fn local_component_colliding_with_a_package_component_still_warns() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let pkg = root.join("node_modules/@takazudo/zudo-doc");
        write_file(
            &pkg.join("dist/theme-toggle/index.js"),
            "export function ThemeToggle() { return null; }\n",
        );
        let local = root.join("src/components/theme-toggle.tsx");
        write_file(&local, "export function ThemeToggle() { return null; }\n");

        assert!(!is_same_package_duplicate(&collision_between(
            &local,
            &pkg.join("dist/theme-toggle/index.js"),
        )));
    }

    /// Even a first-party file that happens to share the package file's stem
    /// stays loud unless the bytes match — the stem alone proves nothing.
    #[test]
    fn stem_match_without_byte_identity_still_warns() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let pkg = root.join("node_modules/acme");
        write_file(
            &pkg.join("dist/widget.js"),
            "export function Widget() { return null; }\n",
        );
        let local = root.join("src/components/widget.tsx");
        write_file(&local, "export function Widget() { return <span />; }\n");

        assert!(!is_same_package_duplicate(&collision_between(
            &local,
            &pkg.join("dist/widget.js"),
        )));
    }

    /// Two modules of the SAME installed package need no byte comparison.
    #[test]
    fn two_modules_of_one_installed_package_are_a_same_package_duplicate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkg = dir.path().join("node_modules/@takazudo/zudo-doc");
        write_file(&pkg.join("dist/routes/_widget.js"), "// compiled\n");
        write_file(&pkg.join("routes-src/_widget.tsx"), "// source\n");

        assert!(is_same_package_duplicate(&collision_between(
            &pkg.join("dist/routes/_widget.js"),
            &pkg.join("routes-src/_widget.tsx"),
        )));
    }

    /// Two DIFFERENT packages colliding is actionable upstream, so it warns.
    #[test]
    fn two_different_packages_still_warn() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_file(&root.join("node_modules/acme/dist/widget.js"), "// acme\n");
        write_file(
            &root.join("node_modules/other/dist/widget.js"),
            "// other\n",
        );

        assert!(!is_same_package_duplicate(&collision_between(
            &root.join("node_modules/acme/dist/widget.js"),
            &root.join("node_modules/other/dist/widget.js"),
        )));
    }

    /// Two first-party modules never reach a package probe.
    #[test]
    fn two_first_party_modules_still_warn() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_file(&root.join("src/a/widget.tsx"), "// same\n");
        write_file(&root.join("src/b/widget.tsx"), "// same\n");

        assert!(!is_same_package_duplicate(&collision_between(
            &root.join("src/a/widget.tsx"),
            &root.join("src/b/widget.tsx"),
        )));
    }

    /// A nested install belongs to the DEEPEST `node_modules` — the inner
    /// package, not its host.
    #[test]
    fn nested_install_is_attributed_to_the_inner_package() {
        let outer = Path::new("/proj/node_modules/host/dist/widget.js");
        let inner = Path::new("/proj/node_modules/host/node_modules/@scope/dep/dist/widget.js");
        assert_eq!(
            package_root_of(outer).unwrap(),
            PathBuf::from("/proj/node_modules/host")
        );
        assert_eq!(
            package_root_of(inner).unwrap(),
            PathBuf::from("/proj/node_modules/host/node_modules/@scope/dep")
        );
        assert!(package_root_of(Path::new("/proj/src/widget.tsx")).is_none());
    }

    /// A package's own nested `node_modules` is not part of that package, so a
    /// copy of a transitive dependency's file is not a same-package duplicate.
    #[test]
    fn a_copy_of_a_nested_dependency_file_is_not_a_same_package_duplicate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let source = "export function Widget() { return null; }\n";
        write_file(
            &root.join("node_modules/host/node_modules/dep/widget.tsx"),
            source,
        );
        write_file(
            &root.join("node_modules/host/dist/widget.js"),
            "// compiled\n",
        );
        let staged = root.join(".staged/widget.tsx");
        write_file(&staged, source);

        assert!(!is_same_package_duplicate(&collision_between(
            &staged,
            &root.join("node_modules/host/dist/widget.js"),
        )));
    }

    #[test]
    fn from_islands_produces_name_keyed_map_in_sorted_order() {
        let set = vec![
            island("Tabs", "/proj/components/tabs.tsx"),
            island("Counter", "/proj/components/counter.tsx"),
        ];
        let m = Manifest::from_islands(&set);
        let collected: Vec<(&str, &Path)> = m.iter().collect();
        assert_eq!(collected[0].0, "Counter");
        assert_eq!(collected[1].0, "Tabs");
        assert_eq!(
            m.get("Counter").unwrap(),
            Path::new("/proj/components/counter.tsx")
        );
        assert_eq!(
            m.get("Tabs").unwrap(),
            Path::new("/proj/components/tabs.tsx")
        );
    }

    #[test]
    fn relative_to_root_uses_forward_slashes() {
        let set = vec![
            island("Counter", "/proj/components/counter.tsx"),
            island("Tabs", "/proj/components/nested/tabs.tsx"),
        ];
        let m = Manifest::from_islands(&set).relative_to(Path::new("/proj"));
        assert_eq!(
            m.get("Counter").unwrap(),
            Path::new("components/counter.tsx")
        );
        assert_eq!(
            m.get("Tabs").unwrap(),
            Path::new("components/nested/tabs.tsx")
        );
    }

    #[test]
    fn relative_to_unrelated_root_keeps_original_path() {
        let set = vec![island("Counter", "/proj/components/counter.tsx")];
        let m = Manifest::from_islands(&set).relative_to(Path::new("/elsewhere"));
        // Different root prefix → original path is kept.
        assert_eq!(
            m.get("Counter").unwrap(),
            Path::new("/proj/components/counter.tsx")
        );
    }

    #[test]
    fn json_output_is_pretty_and_stable() {
        let set = vec![
            island("Tabs", "/proj/components/tabs.tsx"),
            island("Counter", "/proj/components/counter.tsx"),
        ];
        let m = Manifest::from_islands(&set).relative_to(Path::new("/proj"));
        let json = m.to_json_pretty();
        // Keys must appear in sorted order.
        let counter_pos = json.find("\"Counter\"").expect("has Counter");
        let tabs_pos = json.find("\"Tabs\"").expect("has Tabs");
        assert!(counter_pos < tabs_pos, "Counter must come first: {json}");
        // Path values are forward-slashed.
        assert!(json.contains("\"components/counter.tsx\""), "json={json}");
        assert!(json.contains("\"components/tabs.tsx\""), "json={json}");
        // Trailing newline so the file is POSIX-friendly.
        assert!(json.ends_with("}\n"), "json={json:?}");
    }

    #[test]
    fn empty_manifest_serializes_as_empty_object() {
        let m = Manifest::new();
        assert_eq!(m.to_json_pretty(), "{}\n");
    }

    #[test]
    fn duplicate_names_collide_first_path_wins() {
        // Same component name "Counter" exported by two different files.
        // The scanner-style sort would put `/a/counter.tsx` before
        // `/b/counter.tsx`, and the manifest mirrors that.
        let set = vec![
            island("Counter", "/a/counter.tsx"),
            island("Counter", "/b/counter.tsx"),
        ];
        let m = Manifest::from_islands(&set);
        assert_eq!(m.get("Counter").unwrap(), Path::new("/a/counter.tsx"));
        assert_eq!(m.collisions().len(), 1);
        let c = &m.collisions()[0];
        assert_eq!(c.name, "Counter");
        assert_eq!(c.kept_path, PathBuf::from("/a/counter.tsx"));
        assert_eq!(c.dropped_path, PathBuf::from("/b/counter.tsx"));
    }

    #[test]
    fn distinct_default_exports_keyed_by_marker_name_do_not_collide() {
        // Two default-export islands both have component_name "default"
        // but distinct marker_names from different source files. Keying on
        // marker_name keeps both entries; keying on component_name (the old
        // behaviour) would have collapsed them into one + a false collision.
        let set = vec![
            island_marker("default", "/a/foo.tsx", "Foo"),
            island_marker("default", "/b/bar.tsx", "Bar"),
        ];
        let m = Manifest::from_islands(&set);
        assert_eq!(m.len(), 2);
        assert_eq!(m.get("Foo").unwrap(), Path::new("/a/foo.tsx"));
        assert_eq!(m.get("Bar").unwrap(), Path::new("/b/bar.tsx"));
        assert!(
            m.collisions().is_empty(),
            "no collision: {:?}",
            m.collisions()
        );
    }

    #[test]
    fn json_string_escapes_special_characters() {
        let mut s = String::new();
        push_json_string(&mut s, "with \"quote\" and \\back and \nnewline");
        assert_eq!(s, "\"with \\\"quote\\\" and \\\\back and \\nnewline\"");
    }

    #[test]
    fn manifest_json_helper_round_trips_through_root() {
        let set = vec![island("Counter", "/proj/components/counter.tsx")];
        let json = manifest_json(&set, Some(Path::new("/proj")));
        assert!(json.contains("\"Counter\""));
        assert!(json.contains("\"components/counter.tsx\""));
    }

    #[test]
    fn write_manifest_creates_parent_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join("nested/deep/manifest.json");
        let set = vec![island("Counter", "/proj/components/counter.tsx")];
        write_manifest(&set, Some(Path::new("/proj")), &out).expect("write");
        let on_disk = std::fs::read_to_string(&out).expect("read");
        assert!(on_disk.contains("\"Counter\""));
        assert!(on_disk.contains("\"components/counter.tsx\""));
    }
}
