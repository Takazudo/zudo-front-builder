//! Parse esbuild's `--metafile` JSON into per-route transitive module-dep
//! edge sets (#1284/#1287).
//!
//! esbuild already runs once per dev bundle; passing `--metafile=<path>` makes
//! it emit a JSON describing every input file it resolved and that input's own
//! imports. `metafile.inputs[file].imports[*].path` is the canonical
//! *transitive* import graph esbuild itself resolved — no second Rust resolver
//! pass to drift from the real bundle.
//!
//! This module is pure: it takes the metafile bytes plus the
//! shadow-root / project-root / per-route entry mapping and returns, for each
//! route, the set of real on-disk source paths that route transitively imports.
//! The dev path ([`crate`]'s caller in `zfb dev`) upserts those as
//! `DepKind::Module` edges so `dirty_pages(component)` resolves to the
//! consuming route, and registers any out-of-root real paths (symlinked
//! workspace component deps) as extra watch targets.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use serde::Deserialize;

/// Minimal view of esbuild's metafile — only the `inputs` graph is needed.
///
/// Schema (esbuild stable): `{ "inputs": { "<path>": { "imports": [ { "path":
/// "<path>", "kind": "..." }, ... ] }, ... }, "outputs": { ... } }`. Paths in
/// `inputs` are relative to esbuild's working directory (the shadow root, since
/// `run_esbuild` sets `current_dir(shadow)`), except node_modules deps which
/// may appear canonicalised to their real location when esbuild does not
/// preserve symlinks.
#[derive(Debug, Deserialize)]
struct Metafile {
    #[serde(default)]
    inputs: HashMap<String, MetaInput>,
}

#[derive(Debug, Deserialize)]
struct MetaInput {
    #[serde(default)]
    imports: Vec<MetaImport>,
}

#[derive(Debug, Deserialize)]
struct MetaImport {
    path: String,
}

/// A route's entry file as esbuild sees it, paired with the route's
/// project-relative source path the dependency graph keys pages on.
#[derive(Debug, Clone)]
pub struct RouteEntryRef {
    /// The page's source path **relative to `project_root`** (e.g.
    /// `pages/index.tsx`). This becomes the `PageId` the graph upserts.
    pub source_path: PathBuf,
    /// The same file as it appears in the metafile's `inputs` keys — i.e.
    /// relative to the shadow root (esbuild's cwd). For an in-repo page this
    /// equals `source_path`; kept separate so a future shadow layout that
    /// differs from the project layout does not silently misresolve.
    pub metafile_key: String,
}

/// For each route: its project-relative source path and the set of real
/// on-disk module paths it transitively imports (canonicalised).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteModuleDeps {
    pub source_path: PathBuf,
    pub module_deps: BTreeSet<PathBuf>,
}

/// Parse `metafile_bytes` and, for each route in `routes`, walk the transitive
/// `inputs` import graph from that route's entry to collect every reachable
/// source input, mapping each back to a real project path.
///
/// `shadow_root` is esbuild's working directory; `project_root` is the real
/// project tree the watcher reports paths against. A metafile input key is
/// mapped to a real path by:
/// 1. joining it onto `shadow_root` and, if that exists, mapping the
///    shadow-relative tail onto `project_root` (the shadow mirrors the project
///    tree by relative path), then canonicalising;
/// 2. otherwise treating the key as already real (the node_modules /
///    out-of-root symlink-canonicalised case) and canonicalising it.
///
/// The route's own entry file is excluded from its dep set (the graph adds a
/// page self-edge on `upsert`); virtual / generated shadow entries
/// (`entry.mjs`, `.zfb-*`) and anything that does not resolve to a real file
/// are skipped.
pub fn route_module_deps(
    metafile_bytes: &[u8],
    routes: &[RouteEntryRef],
    shadow_root: &Path,
    project_root: &Path,
) -> Vec<RouteModuleDeps> {
    let meta: Metafile = match serde_json::from_slice(metafile_bytes) {
        Ok(m) => m,
        // A malformed / empty metafile must never break the bundle — the dev
        // path falls back to the prior (imprecise) selection on empty deps.
        Err(_) => return Vec::new(),
    };

    routes
        .iter()
        .map(|route| {
            let reachable = transitive_inputs(&meta, &route.metafile_key);
            let mut module_deps: BTreeSet<PathBuf> = BTreeSet::new();
            for key in reachable {
                // Exclude the route's own entry (graph adds a self-edge).
                if key == route.metafile_key {
                    continue;
                }
                if is_synthetic_input(&key) {
                    continue;
                }
                if let Some(real) = map_to_real(&key, shadow_root, project_root) {
                    module_deps.insert(real);
                }
            }
            RouteModuleDeps {
                source_path: route.source_path.clone(),
                module_deps,
            }
        })
        .collect()
}

/// Collect every input transitively reachable from `entry` via the metafile's
/// per-input `imports` lists (including `entry` itself).
fn transitive_inputs(meta: &Metafile, entry: &str) -> HashSet<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut stack: Vec<String> = vec![entry.to_string()];
    while let Some(cur) = stack.pop() {
        if !seen.insert(cur.clone()) {
            continue;
        }
        if let Some(input) = meta.inputs.get(&cur) {
            for imp in &input.imports {
                if !seen.contains(&imp.path) {
                    stack.push(imp.path.clone());
                }
            }
        }
    }
    seen
}

/// esbuild synthesises some shadow inputs that are not real project source —
/// the generated entry, hydrate shim, plugin virtual-module temp files. These
/// must never become watch targets or graph edges.
fn is_synthetic_input(key: &str) -> bool {
    let name = Path::new(key)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(key);
    name == "entry.mjs"
        || name.starts_with(".zfb-virtual-")
        || name.starts_with(".zfb-")
        || name.contains("zfb-hydrate")
}

/// Map a metafile input key to a real on-disk path (see [`route_module_deps`]).
fn map_to_real(key: &str, shadow_root: &Path, project_root: &Path) -> Option<PathBuf> {
    let key_path = Path::new(key);

    // Absolute keys (node_modules canonicalised by esbuild) are already real.
    if key_path.is_absolute() {
        return canonical_or_self(key_path);
    }

    // Shadow-relative key. The shadow mirrors the project tree by relative
    // path, so the same relative tail under `project_root` is the real source.
    let in_project = project_root.join(key_path);
    if in_project.exists() {
        return canonical_or_self(&in_project);
    }

    // Fall back to the shadow location itself (e.g. a materialised copy that
    // has no project-tree twin). Canonicalising follows any symlink to the
    // real workspace file — exactly the watch target a symlinked dep needs.
    let in_shadow = shadow_root.join(key_path);
    if in_shadow.exists() {
        return canonical_or_self(&in_shadow);
    }

    None
}

/// Canonicalise, falling back to the path as-given when canonicalisation fails
/// (e.g. the file was removed between the bundle and this read) — a best-effort
/// real path is still a usable graph key.
fn canonical_or_self(p: &Path) -> Option<PathBuf> {
    Some(std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()))
}

/// Fail-closed audit: prove that no `bundle.exclude`-matched source leaked
/// into the bundle by cross-checking esbuild's `--metafile` `inputs` record
/// (its authoritative resolution log) against the exclude predicate.
///
/// This deliberately diverges from [`route_module_deps`]'s posture above: that
/// parser is best-effort (a malformed metafile there just degrades dev-loop
/// invalidation precision), but this audit is the last line of defense
/// against a shadow-staging escape hatch — see
/// `.claude/skills/l-lessons-client-bundling/SKILL.md`'s "not-yet-adopted
/// fixed point" note. When `bundle.exclude` is active, esbuild's own
/// resolution record is the only oracle trusted for "did anything excluded
/// get bundled anyway"; failing to read that oracle must not silently pass,
/// it must hard-fail the build. Callers are expected to only invoke this when
/// `bundle.exclude` is non-empty — gating that call is wave-3 wiring, not
/// this primitive.
///
/// Every `inputs` key is checked under BOTH spellings:
/// 1. the logical key as esbuild wrote it, joined onto `project_root` (the
///    pre-canonicalisation spelling); and
/// 2. `map_to_real`'s canonicalised real path.
///
/// A project-relative symlink can make these two spellings disagree about
/// whether a path falls under an excluded prefix, in either direction —
/// checking only one risks a false negative, so both are audited.
///
/// `is_excluded` is expected to be the build's live `bundle.exclude`
/// predicate (e.g. a closure over the compiled glob matcher), taking an
/// absolute path and answering whether it matches an exclude pattern
/// relative to `project_root`.
pub fn audit_metafile_exclusions(
    metafile_bytes: &[u8],
    is_excluded: &dyn Fn(&Path) -> bool,
    shadow_root: &Path,
    project_root: &Path,
) -> Result<()> {
    let meta: Metafile = serde_json::from_slice(metafile_bytes)
        .map_err(|e| anyhow!("zfb bundler: bundle.exclude audit failed to parse metafile: {e}"))?;

    let mut offenders: BTreeSet<String> = BTreeSet::new();
    for key in meta.inputs.keys() {
        if is_synthetic_input(key) {
            continue;
        }

        // Spelling 1: the logical key, as esbuild recorded it, resolved
        // against the project root — no canonicalisation, no disk lookup.
        let logical = project_root.join(Path::new(key));
        if is_excluded(&logical) {
            offenders.insert(key.clone());
            continue;
        }

        // Spelling 2: the same key, mapped to its real on-disk path (may
        // resolve through a symlink, landing somewhere the logical spelling
        // above does not point at).
        if let Some(real) = map_to_real(key, shadow_root, project_root) {
            if is_excluded(&real) {
                offenders.insert(key.clone());
            }
        }
    }

    if offenders.is_empty() {
        return Ok(());
    }

    bail!(
        "zfb bundler: bundle.exclude leak — the following metafile input(s) matched an exclude pattern but were still present in the bundle: {}",
        offenders.into_iter().collect::<Vec<_>>().join(", ")
    );
}

/// Convenience wrapper over [`audit_metafile_exclusions`]: read the metafile
/// from `metafile_path` first. Fail-closed extends to the read itself — a
/// missing or unreadable metafile is a build error, exactly like malformed
/// JSON, whenever the caller is auditing active exclusions.
pub fn audit_metafile_exclusions_at_path(
    metafile_path: &Path,
    is_excluded: &dyn Fn(&Path) -> bool,
    shadow_root: &Path,
    project_root: &Path,
) -> Result<()> {
    let bytes = std::fs::read(metafile_path).map_err(|e| {
        anyhow!(
            "zfb bundler: bundle.exclude audit failed to read metafile {}: {e}",
            metafile_path.display()
        )
    })?;
    audit_metafile_exclusions(&bytes, is_excluded, shadow_root, project_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, body: &str) -> PathBuf {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn direct_import_becomes_route_dep() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "pages/index.tsx", "import './x'");
        write(root, "components/Header.tsx", "x");

        let metafile = br#"{
            "inputs": {
                "pages/index.tsx": { "imports": [ { "path": "components/Header.tsx" } ] },
                "components/Header.tsx": { "imports": [] }
            }
        }"#;
        let routes = vec![RouteEntryRef {
            source_path: PathBuf::from("pages/index.tsx"),
            metafile_key: "pages/index.tsx".to_string(),
        }];

        let deps = route_module_deps(metafile, &routes, root, root);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].source_path, PathBuf::from("pages/index.tsx"));
        let real_header = std::fs::canonicalize(root.join("components/Header.tsx")).unwrap();
        assert!(
            deps[0].module_deps.contains(&real_header),
            "direct component import must become a real-path module dep; got {:?}",
            deps[0].module_deps
        );
    }

    #[test]
    fn transitive_import_is_flattened_into_route_dep() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "pages/index.tsx", "import './h'");
        write(root, "components/Header.tsx", "import './l'");
        write(root, "components/Logo.tsx", "x");

        // index -> Header -> Logo
        let metafile = br#"{
            "inputs": {
                "pages/index.tsx": { "imports": [ { "path": "components/Header.tsx" } ] },
                "components/Header.tsx": { "imports": [ { "path": "components/Logo.tsx" } ] },
                "components/Logo.tsx": { "imports": [] }
            }
        }"#;
        let routes = vec![RouteEntryRef {
            source_path: PathBuf::from("pages/index.tsx"),
            metafile_key: "pages/index.tsx".to_string(),
        }];

        let deps = route_module_deps(metafile, &routes, root, root);
        let real_logo = std::fs::canonicalize(root.join("components/Logo.tsx")).unwrap();
        assert!(
            deps[0].module_deps.contains(&real_logo),
            "a transitively-imported component must be flattened into the route's deps"
        );
    }

    #[test]
    fn route_own_entry_and_synthetic_inputs_are_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "pages/index.tsx", "x");

        let metafile = br#"{
            "inputs": {
                "entry.mjs": { "imports": [ { "path": "pages/index.tsx" } ] },
                "pages/index.tsx": { "imports": [ { "path": "entry.mjs" } ] }
            }
        }"#;
        let routes = vec![RouteEntryRef {
            source_path: PathBuf::from("pages/index.tsx"),
            metafile_key: "pages/index.tsx".to_string(),
        }];

        let deps = route_module_deps(metafile, &routes, root, root);
        assert!(
            deps[0].module_deps.is_empty(),
            "the route's own entry and the synthetic entry.mjs must not be deps; got {:?}",
            deps[0].module_deps
        );
    }

    #[test]
    fn nonexistent_input_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "pages/index.tsx", "x");

        let metafile = br#"{
            "inputs": {
                "pages/index.tsx": { "imports": [ { "path": "components/Ghost.tsx" } ] }
            }
        }"#;
        let routes = vec![RouteEntryRef {
            source_path: PathBuf::from("pages/index.tsx"),
            metafile_key: "pages/index.tsx".to_string(),
        }];

        let deps = route_module_deps(metafile, &routes, root, root);
        assert!(
            deps[0].module_deps.is_empty(),
            "an input with no real file on disk is skipped, not invented"
        );
    }

    #[test]
    fn malformed_metafile_yields_empty_not_panic() {
        let routes = vec![RouteEntryRef {
            source_path: PathBuf::from("pages/index.tsx"),
            metafile_key: "pages/index.tsx".to_string(),
        }];
        let deps = route_module_deps(b"not json", &routes, Path::new("/p"), Path::new("/p"));
        assert!(deps.is_empty());
    }

    #[test]
    fn symlinked_workspace_dep_canonicalises_to_real_path() {
        // shadow tree differs from project tree; a metafile key resolves only
        // under the shadow root, where it is a symlink to the real workspace
        // file. The dep must canonicalise to that real file (the watch target).
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let shadow = tmp.path().join("shadow");
        let real_pkg = tmp.path().join("workspace/design-system/Button.tsx");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(shadow.join("node_modules/@ds")).unwrap();
        write(&project, "pages/index.tsx", "x");
        std::fs::create_dir_all(real_pkg.parent().unwrap()).unwrap();
        std::fs::write(&real_pkg, "btn").unwrap();

        let link = shadow.join("node_modules/@ds/Button.tsx");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_pkg, &link).unwrap();
        #[cfg(not(unix))]
        std::fs::write(&link, "btn").unwrap();

        write(&project, "pages/index.tsx", "x");

        let metafile = br#"{
            "inputs": {
                "pages/index.tsx": { "imports": [ { "path": "node_modules/@ds/Button.tsx" } ] },
                "node_modules/@ds/Button.tsx": { "imports": [] }
            }
        }"#;
        let routes = vec![RouteEntryRef {
            source_path: PathBuf::from("pages/index.tsx"),
            metafile_key: "pages/index.tsx".to_string(),
        }];

        let deps = route_module_deps(metafile, &routes, &shadow, &project);
        #[cfg(unix)]
        {
            let real = std::fs::canonicalize(&real_pkg).unwrap();
            assert!(
                deps[0].module_deps.contains(&real),
                "symlinked workspace dep must canonicalise to the real file; got {:?}",
                deps[0].module_deps
            );
        }
    }

    // --- audit_metafile_exclusions ------------------------------------

    /// A crude stand-in for `BundleExcludeMatcher::is_excluded` (private to
    /// `bundler.rs`): true when `abs`'s path relative to `project_root`
    /// starts with `prefix`. Good enough to exercise the audit's dual-spelling
    /// logic without pulling in real glob matching.
    fn make_is_excluded(project_root: PathBuf, prefix: &'static str) -> impl Fn(&Path) -> bool {
        move |abs: &Path| {
            abs.strip_prefix(&project_root)
                .map(|rel| rel.to_string_lossy().replace('\\', "/").starts_with(prefix))
                .unwrap_or(false)
        }
    }

    #[test]
    fn audit_passes_when_no_input_is_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let metafile = br#"{
            "inputs": {
                "pages/index.tsx": { "imports": [ { "path": "components/Header.tsx" } ] },
                "components/Header.tsx": { "imports": [] }
            }
        }"#;
        let is_excluded = make_is_excluded(root.clone(), "vendor/legacy/");

        let result = audit_metafile_exclusions(metafile, &is_excluded, &root, &root);
        assert!(
            result.is_ok(),
            "no input matches the exclude pattern, audit must pass; got {result:?}"
        );
    }

    #[test]
    fn audit_fails_when_logical_spelling_is_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        // The excluded file need not exist on disk — the audit checks the
        // metafile's recorded string key, not the filesystem.
        let metafile = br#"{
            "inputs": {
                "pages/index.tsx": { "imports": [ { "path": "vendor/legacy/Old.tsx" } ] },
                "vendor/legacy/Old.tsx": { "imports": [] }
            }
        }"#;
        let is_excluded = make_is_excluded(root.clone(), "vendor/legacy/");

        let err = audit_metafile_exclusions(metafile, &is_excluded, &root, &root)
            .expect_err("an excluded logical key present in inputs must fail the audit");
        let msg = err.to_string();
        assert!(
            msg.contains("bundle.exclude"),
            "error must name bundle.exclude; got {msg:?}"
        );
        assert!(
            msg.contains("vendor/legacy/Old.tsx"),
            "error must name the offending path; got {msg:?}"
        );
    }

    #[test]
    fn audit_fails_when_only_the_real_mapped_spelling_is_excluded() {
        // The metafile's logical key (a shadow-relative node_modules alias)
        // does not itself look excluded, but `map_to_real` resolves it
        // through a symlink to a real file that lives under an excluded
        // project directory. Checking only the logical spelling would miss
        // this leak — exactly the dual-spelling rationale this audit exists
        // for (see the fn doc comment and l-lessons-client-bundling).
        let tmp = tempfile::tempdir().unwrap();
        // Canonicalise the tempdir base up front (macOS maps `/var` ->
        // `/private/var`): `map_to_real` canonicalises the resolved real
        // path, so `is_excluded`'s `strip_prefix(project_root)` must compare
        // against an equally-canonical `project_root` or it spuriously fails
        // to match on platforms where the two spellings differ.
        let base = tmp.path().canonicalize().unwrap();
        let project = base.join("project");
        let shadow = base.join("shadow");
        let real_excluded = project.join("vendor/legacy/Button.tsx");
        std::fs::create_dir_all(real_excluded.parent().unwrap()).unwrap();
        std::fs::write(&real_excluded, "btn").unwrap();
        std::fs::create_dir_all(shadow.join("node_modules/@ds")).unwrap();

        let link = shadow.join("node_modules/@ds/Button.tsx");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_excluded, &link).unwrap();
        #[cfg(not(unix))]
        std::fs::write(&link, "btn").unwrap();

        let metafile = br#"{
            "inputs": {
                "pages/index.tsx": { "imports": [ { "path": "node_modules/@ds/Button.tsx" } ] },
                "node_modules/@ds/Button.tsx": { "imports": [] }
            }
        }"#;
        let is_excluded = make_is_excluded(project.clone(), "vendor/legacy/");

        let result = audit_metafile_exclusions(metafile, &is_excluded, &shadow, &project);

        #[cfg(unix)]
        {
            let err = result.expect_err(
                "the real-mapped spelling resolves into an excluded directory, audit must fail",
            );
            let msg = err.to_string();
            assert!(
                msg.contains("bundle.exclude"),
                "error must name bundle.exclude; got {msg:?}"
            );
            assert!(
                msg.contains("node_modules/@ds/Button.tsx"),
                "error must name the offending metafile key; got {msg:?}"
            );
        }
        #[cfg(not(unix))]
        {
            // No symlink semantics off unix in this test harness — the write()
            // fallback above makes the alias its own file, not a leak.
            let _ = result;
        }
    }

    #[test]
    fn audit_fails_closed_on_malformed_metafile() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let is_excluded = make_is_excluded(root.clone(), "vendor/legacy/");

        let err = audit_metafile_exclusions(b"not json", &is_excluded, &root, &root)
            .expect_err("a malformed metafile must be a build error, not an empty pass");
        assert!(
            err.to_string().contains("bundle.exclude"),
            "fail-closed error must name bundle.exclude; got {err}"
        );
    }

    #[test]
    fn audit_fails_closed_on_missing_metafile_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let missing = root.join("does-not-exist/metafile.json");
        let is_excluded = make_is_excluded(root.clone(), "vendor/legacy/");

        let err = audit_metafile_exclusions_at_path(&missing, &is_excluded, &root, &root)
            .expect_err("an unreadable metafile path must be a build error, not an empty pass");
        assert!(
            err.to_string().contains("bundle.exclude"),
            "fail-closed error must name bundle.exclude; got {err}"
        );
    }
}
