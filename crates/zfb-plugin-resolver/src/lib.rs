//! # zfb-plugin-resolver
//!
//! Shared exact-match resolver inputs for the two esbuild call sites in
//! the workspace:
//!
//! - the main page/layout bundler in `zfb-build::bundler`
//! - the islands bundler in `zfb-islands::esbuild`
//!
//! Both crates used to forward plugin-registered aliases as
//! `--alias:<from>=<to>` CLI flags. esbuild's `--alias` is
//! **prefix-with-slash** — registering `@/foo` would silently match
//! `@/foo/bar` as well, which contradicts the documented exact-match
//! contract honored by the embedded V8 host
//! (`zfb-render::BundleModuleLoader::resolve_alias`).
//!
//! This helper produces the inputs needed to express the same aliases
//! as **exact-match** entries in esbuild's `--tsconfig` path-mapping
//! pipeline: a `compilerOptions.paths` entry without the wildcard
//! suffix is a literal exact-match in TypeScript / esbuild.
//!
//! ## What it does
//!
//! Given a list of plugin aliases and a list of plugin virtual modules,
//! plus a working dir for temp-file allocation, the helper:
//!
//! 1. Writes each virtual module's source to a temp `.mjs` file under
//!    the working dir (held alive in `ResolverInputs::_temp_files`).
//! 2. Builds a `Vec<(String, String)>` of `(specifier, absolute path)`
//!    pairs, with paths normalized to POSIX forward-slash form so the
//!    same JSON is emitted on Windows / WSL / Linux.
//! 3. Provides a `merge_into_tsconfig_paths` helper that folds those
//!    pairs into the caller's existing `tsconfig_paths` map without
//!    overwriting user-supplied entries.
//!
//! Callers then write the merged map into their synthetic tsconfig and
//! pass `--tsconfig=<that path>` to esbuild. No `--alias` flags are
//! emitted for plugin entries.
//!
//! ## Why one shared crate
//!
//! `zfb-build` depends on `zfb-render` and `zfb-content`; `zfb-islands`
//! does not depend on `zfb-build` (and `zfb-build` re-uses
//! `zfb-islands` through the orchestrator's bundling fan-out). Placing
//! this helper in either crate would create a cycle or pull in the
//! whole orchestrator graph. A tiny leaf crate sidesteps both.

use std::path::Path;

use anyhow::{Context, Result};
use tempfile::NamedTempFile;
use zfb_types::path_to_posix_string;

/// Output of [`build_resolver_inputs`].
///
/// `paths_entries` is the list of `(specifier, absolute-target-path)`
/// pairs the caller will merge into its synthetic
/// `compilerOptions.paths` map. The target path is already in POSIX
/// forward-slash form.
///
/// `_temp_files` holds the on-disk lifetime of any virtual-module
/// `.mjs` files the helper wrote. The caller MUST keep this value
/// alive (e.g. by binding it to a local) until esbuild has finished
/// reading the files; their `Drop` impl deletes them from disk.
pub struct ResolverInputs {
    /// `(specifier, absolute-path)` pairs to merge into
    /// `compilerOptions.paths`. Paths are POSIX forward-slash.
    pub paths_entries: Vec<(String, String)>,

    /// Held-alive temp-file handles for plugin virtual modules.
    ///
    /// Public so callers can extend the lifetime explicitly; the field
    /// itself is otherwise opaque — the caller does not need to read
    /// it. Underscore prefix discourages incidental use.
    pub _temp_files: Vec<NamedTempFile>,
}

/// Build the per-call resolver inputs from plugin-registered aliases
/// and virtual modules.
///
/// - `aliases`: `(from, to)` pairs where `to` is an absolute filesystem
///   path the alias resolves to.
/// - `virtual_modules`: `(specifier, source)` pairs where `source` is
///   the ESM source text to materialize to disk.
/// - `working_dir`: where to allocate virtual-module temp files. Falls
///   back to the system temp dir when the path is not a directory
///   (matches the islands path's pre-existing fallback for the
///   unit-test environment).
///
/// On success the returned `ResolverInputs::paths_entries` contains one
/// entry per alias plus one entry per virtual module, all keyed on the
/// bare specifier, all targeting a single absolute POSIX path.
pub fn build_resolver_inputs(
    aliases: &[(String, String)],
    virtual_modules: &[(String, String)],
    working_dir: &Path,
) -> Result<ResolverInputs> {
    let mut paths_entries: Vec<(String, String)> = Vec::new();
    let mut temp_files: Vec<NamedTempFile> = Vec::new();

    // Plugin aliases first. The target string already came from the
    // plugin registry as an absolute filesystem path (resolved against
    // the project root by `PluginSetupAccumulator::resolve_against_root`).
    // Normalize to POSIX so the JSON output is platform-stable.
    for (from, to) in aliases {
        paths_entries.push((from.clone(), path_to_posix_string(Path::new(to))));
    }

    // Plugin virtual modules. Each source is written to a `.mjs` temp
    // file *inside* `working_dir` so esbuild's upward node_modules walk
    // resolves to the right tree. If `working_dir` is not a real dir
    // (unit-test environment, missing path), fall back to the system
    // temp dir — the islands path does the same.
    for (specifier, source) in virtual_modules {
        let tmp = if working_dir.is_dir() {
            tempfile::Builder::new()
                .prefix(".zfb-virtual-")
                .suffix(".mjs")
                .tempfile_in(working_dir)
                .with_context(|| {
                    format!(
                        "zfb-plugin-resolver: failed to allocate virtual-module \
                         temp file for `{specifier}` inside `{}`",
                        working_dir.display()
                    )
                })?
        } else {
            tempfile::Builder::new()
                .prefix("zfb-virtual-")
                .suffix(".mjs")
                .tempfile()
                .with_context(|| {
                    format!(
                        "zfb-plugin-resolver: failed to allocate virtual-module \
                         temp file for `{specifier}`"
                    )
                })?
        };
        std::fs::write(tmp.path(), source.as_bytes()).with_context(|| {
            format!(
                "zfb-plugin-resolver: failed to write virtual-module source \
                 for specifier `{specifier}` to `{}`",
                tmp.path().display()
            )
        })?;
        paths_entries.push((specifier.clone(), path_to_posix_string(tmp.path())));
        temp_files.push(tmp);
    }

    Ok(ResolverInputs {
        paths_entries,
        _temp_files: temp_files,
    })
}

/// Merge `entries` (typically `ResolverInputs::paths_entries`) into a
/// caller-owned `paths` map shaped like `BTreeMap<String, Vec<String>>`
/// — the shape TypeScript / esbuild expect under
/// `compilerOptions.paths`.
///
/// **Collision policy: user wins.** If the map already contains an
/// entry for a given specifier (because the caller pre-populated it
/// from `BundlerInput::tsconfig_paths`), the existing value is left
/// untouched. The explicit `tsconfig_paths` field is the user's
/// hand-written contract; plugin-registered aliases are additive
/// suggestions on top. Plugin-vs-plugin conflicts are already
/// detected upstream by `PluginSetupAccumulator` (`AliasConflict` /
/// `VirtualModuleConflict`), so by the time entries reach this helper
/// they are guaranteed to be self-consistent.
pub fn merge_into_tsconfig_paths(
    paths: &mut std::collections::BTreeMap<String, Vec<String>>,
    entries: &[(String, String)],
) {
    for (specifier, target) in entries {
        paths
            .entry(specifier.clone())
            .or_insert_with(|| vec![target.clone()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    /// `build_resolver_inputs` with zero aliases + zero virtual modules
    /// returns an empty `paths_entries` and no temp files.
    #[test]
    fn empty_inputs_produce_empty_outputs() {
        let dir = TempDir::new().unwrap();
        let r = build_resolver_inputs(&[], &[], dir.path()).unwrap();
        assert!(r.paths_entries.is_empty());
        assert!(r._temp_files.is_empty());
    }

    /// An alias `(from, to)` produces exactly one `paths` entry keyed
    /// on the bare specifier and pointing to the target path. No
    /// wildcard suffix — this is the exact-match contract.
    #[test]
    fn alias_produces_exact_match_paths_entry() {
        let dir = TempDir::new().unwrap();
        let aliases = vec![("@/foo".to_string(), "/abs/src/foo.tsx".to_string())];
        let r = build_resolver_inputs(&aliases, &[], dir.path()).unwrap();
        assert_eq!(r.paths_entries.len(), 1);
        assert_eq!(r.paths_entries[0].0, "@/foo");
        assert_eq!(r.paths_entries[0].1, "/abs/src/foo.tsx");
        // No wildcard — bare specifier only.
        assert!(!r.paths_entries[0].0.contains('*'));
        assert!(!r.paths_entries[0].1.contains('*'));
    }

    /// Virtual modules materialize to a `.mjs` temp file under
    /// `working_dir` and the entry's target points at that file. The
    /// `NamedTempFile` lives in `_temp_files` so the disk handle is
    /// held alive until the caller drops `ResolverInputs`.
    #[test]
    fn virtual_module_materializes_to_temp_mjs_under_working_dir() {
        let dir = TempDir::new().unwrap();
        let vms = vec![(
            "virtual:meta".to_string(),
            "export default { ok: true };".to_string(),
        )];
        let r = build_resolver_inputs(&[], &vms, dir.path()).unwrap();
        assert_eq!(r.paths_entries.len(), 1);
        assert_eq!(r.paths_entries[0].0, "virtual:meta");
        assert_eq!(r._temp_files.len(), 1);
        let tmp_path = r._temp_files[0].path().to_path_buf();
        // The temp file is under working_dir.
        assert!(
            tmp_path.starts_with(dir.path()),
            "temp file `{}` must be under working_dir `{}`",
            tmp_path.display(),
            dir.path().display()
        );
        // Suffix is `.mjs`.
        assert_eq!(tmp_path.extension().and_then(|s| s.to_str()), Some("mjs"));
        // The path entry's target matches the temp file path (POSIX).
        assert_eq!(r.paths_entries[0].1, path_to_posix_string(&tmp_path));
        // The source was written to disk.
        let contents = std::fs::read_to_string(&tmp_path).unwrap();
        assert_eq!(contents, "export default { ok: true };");
    }

    /// When `working_dir` does not exist, the helper still succeeds by
    /// falling back to the system temp dir. The path is no longer
    /// guaranteed to be under `working_dir`, but the file is still
    /// written and the entry references it.
    #[test]
    fn virtual_module_falls_back_to_system_tmpdir_when_working_dir_missing() {
        let bogus = std::path::PathBuf::from("/this/path/should/not/exist/zfb-test");
        let vms = vec![("virtual:x".to_string(), "export const x = 1;".to_string())];
        let r = build_resolver_inputs(&[], &vms, &bogus).unwrap();
        assert_eq!(r._temp_files.len(), 1);
        let tmp_path = r._temp_files[0].path();
        assert!(tmp_path.exists(), "temp file must be on disk");
        let contents = std::fs::read_to_string(tmp_path).unwrap();
        assert_eq!(contents, "export const x = 1;");
    }

    /// `merge_into_tsconfig_paths` adds new entries when the key is
    /// absent. Plugin entries are wrapped as `vec![target]` — the
    /// `Vec<String>` shape esbuild expects for `compilerOptions.paths`
    /// values.
    #[test]
    fn merge_adds_new_keys() {
        let mut paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let entries = vec![
            ("@/foo".to_string(), "/abs/foo.tsx".to_string()),
            ("virtual:meta".to_string(), "/tmp/v.mjs".to_string()),
        ];
        merge_into_tsconfig_paths(&mut paths, &entries);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths.get("@/foo").unwrap(), &vec!["/abs/foo.tsx".to_string()]);
        assert_eq!(
            paths.get("virtual:meta").unwrap(),
            &vec!["/tmp/v.mjs".to_string()]
        );
    }

    /// **Collision policy test.** Pre-existing user entries (from
    /// `BundlerInput::tsconfig_paths`) MUST be preserved when a plugin
    /// later registers the same specifier — the explicit
    /// `tsconfig_paths` field is the user's hand-written contract.
    #[test]
    fn merge_preserves_user_entries_on_collision() {
        let mut paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
        paths.insert(
            "@/foo".to_string(),
            vec!["src/user-version.tsx".to_string()],
        );
        let entries = vec![("@/foo".to_string(), "/abs/plugin-version.tsx".to_string())];
        merge_into_tsconfig_paths(&mut paths, &entries);
        // User entry is untouched.
        assert_eq!(
            paths.get("@/foo").unwrap(),
            &vec!["src/user-version.tsx".to_string()],
            "user-supplied tsconfig_paths entries must win over plugin entries"
        );
    }

    /// Pre-existing user entries with DIFFERENT keys coexist with
    /// plugin entries. Both are honored.
    #[test]
    fn merge_honors_both_user_and_plugin_entries() {
        let mut paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
        paths.insert(
            "@/components/*".to_string(),
            vec!["src/components/*".to_string()],
        );
        let entries = vec![("@/data".to_string(), "/abs/data.ts".to_string())];
        merge_into_tsconfig_paths(&mut paths, &entries);
        assert_eq!(paths.len(), 2);
        assert!(paths.contains_key("@/components/*"));
        assert!(paths.contains_key("@/data"));
    }

    /// `to_posix` is a no-op on Unix-style paths and converts
    /// backslashes when present. The output must contain only forward
    /// slashes.
    #[test]
    fn to_posix_replaces_backslashes() {
        assert_eq!(path_to_posix_string(Path::new("/abs/foo.tsx")), "/abs/foo.tsx");
        // Simulate a Windows-style path string. On Unix `Path` does not
        // recognize `\` as a separator, but `to_string_lossy()` still
        // returns the literal characters, which our replace then maps
        // to forward slashes.
        assert_eq!(
            path_to_posix_string(Path::new(r"C:\abs\foo.tsx")),
            "C:/abs/foo.tsx",
            "to_posix must replace backslashes with forward slashes"
        );
    }

    /// Combined: aliases + virtual modules together produce one entry
    /// per registration in deterministic order (aliases first, then
    /// virtual modules), and the temp-file count matches the
    /// virtual-module count.
    #[test]
    fn combined_aliases_and_virtual_modules_round_trip() {
        let dir = TempDir::new().unwrap();
        let aliases = vec![
            ("@/foo".to_string(), "/abs/foo.tsx".to_string()),
            ("@/bar".to_string(), "/abs/bar.tsx".to_string()),
        ];
        let vms = vec![(
            "virtual:meta".to_string(),
            "export const meta = 1;".to_string(),
        )];
        let r = build_resolver_inputs(&aliases, &vms, dir.path()).unwrap();
        assert_eq!(r.paths_entries.len(), 3);
        assert_eq!(r.paths_entries[0].0, "@/foo");
        assert_eq!(r.paths_entries[1].0, "@/bar");
        assert_eq!(r.paths_entries[2].0, "virtual:meta");
        assert_eq!(r._temp_files.len(), 1);
    }
}
