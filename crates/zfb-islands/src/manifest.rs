//! Islands manifest — `ComponentName → resolved-tsx-path` JSON map.
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
//! - **Keys** are component-name identities exactly as the scanner emits
//!   them: the value of [`crate::Island::component_name`]. Default exports
//!   surface as the literal string `"default"` (per the scanner's
//!   "Component identity" rules).
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
//!   `BTreeMap<String, _>` keyed by name so JSON serialization is stable
//!   regardless of input order.
//! - **Forward-slash paths**: relative paths use `/` even on Windows hosts
//!   so the manifest is portable across the JS/TS bundler subprocess that
//!   reads it. Absolute paths preserve the host separator.
//! - **One value per key**: when two distinct source files export the same
//!   component name, the entry whose `source_path` sorts first wins. This
//!   matches the scanner's pre-existing `(source_path, name)` tie-break and
//!   keeps the manifest a flat `name → path` map. A diagnostic helper,
//!   [`Manifest::collisions`], surfaces dropped entries so callers that
//!   want to fail loudly can do so.
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

/// A component-name → resolved-source-path map.
///
/// See the module-level docs for the stable on-disk format and stability
/// guarantees.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    /// `ComponentName → path` entries.
    ///
    /// `BTreeMap` is the storage type because we need deterministic
    /// iteration order in JSON output and we never produce manifests with
    /// enough entries that hashing would matter.
    entries: BTreeMap<String, PathBuf>,
    /// Names dropped due to component-name collisions across distinct
    /// source files. `(name, dropped_path, kept_path)`.
    collisions: Vec<Collision>,
}

/// A name-collision entry. Two `(source_path, name)` pairs share `name`
/// but came from different source paths; the manifest kept `kept_path`
/// (the first by sort order) and dropped `dropped_path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    /// The colliding component name.
    pub name: String,
    /// The source path that did **not** make it into the manifest.
    pub dropped_path: PathBuf,
    /// The source path that **did** make it into the manifest.
    pub kept_path: PathBuf,
}

impl Manifest {
    /// Construct an empty manifest.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a manifest from a scanner [`IslandsSet`].
    ///
    /// The scanner's output is already deterministically ordered by
    /// `(source_path, component_name)`. We reorder by component name and
    /// drop collisions deterministically (first by `source_path` sort
    /// order wins). Dropped entries are recorded in
    /// [`Manifest::collisions`].
    pub fn from_islands(islands: &IslandsSet) -> Self {
        let mut entries: BTreeMap<String, PathBuf> = BTreeMap::new();
        let mut collisions: Vec<Collision> = Vec::new();
        for island in islands {
            match entries.get(&island.component_name) {
                None => {
                    entries.insert(island.component_name.clone(), island.source_path.clone());
                }
                Some(kept) => {
                    if kept != &island.source_path {
                        collisions.push(Collision {
                            name: island.component_name.clone(),
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

    /// Look up the source path for a component name.
    pub fn get(&self, name: &str) -> Option<&Path> {
        self.entries.get(name).map(PathBuf::as_path)
    }

    /// Read-only access to the collision diagnostic list.
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
        assert_eq!(m.get("Counter").unwrap(), Path::new("/proj/components/counter.tsx"));
        assert_eq!(m.get("Tabs").unwrap(), Path::new("/proj/components/tabs.tsx"));
    }

    #[test]
    fn relative_to_root_uses_forward_slashes() {
        let set = vec![
            island("Counter", "/proj/components/counter.tsx"),
            island("Tabs", "/proj/components/nested/tabs.tsx"),
        ];
        let m = Manifest::from_islands(&set).relative_to(Path::new("/proj"));
        assert_eq!(m.get("Counter").unwrap(), Path::new("components/counter.tsx"));
        assert_eq!(m.get("Tabs").unwrap(), Path::new("components/nested/tabs.tsx"));
    }

    #[test]
    fn relative_to_unrelated_root_keeps_original_path() {
        let set = vec![island("Counter", "/proj/components/counter.tsx")];
        let m = Manifest::from_islands(&set).relative_to(Path::new("/elsewhere"));
        // Different root prefix → original path is kept.
        assert_eq!(m.get("Counter").unwrap(), Path::new("/proj/components/counter.tsx"));
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
