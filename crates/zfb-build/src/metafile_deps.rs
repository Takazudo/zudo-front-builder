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
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use serde::Deserialize;
use zfb_types::normalize_path_lexical;

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

/// Case-1 fallback for [`audit_metafile_stage_escape`]'s discriminator: does
/// `logical_path` name a directory ENTRY that physically sits inside one of
/// `canonical_stage_roots`? Canonicalise the PARENT directory and re-append
/// the file name — deliberately not the full path, which would follow the
/// staged symlink itself out of the stage (that resolved location is exactly
/// the `canonical_path` the caller already knows escaped). The pure-lexical
/// check this backs up is symlink-blind: when `metafile_cwd` is spelled
/// through a symlink alias whose resolved location sits at a different DEPTH
/// (macOS `/var` -> `/private/var`), esbuild's `..` count is computed against
/// the RESOLVED cwd, so the lexical collapse pops one component too many and
/// never re-matches the stage root spelling even though the staged entry is
/// real.
///
/// Fail-closed: only a SUCCESSFUL `std::fs::canonicalize` of the parent may
/// accept — [`canonical_or_self`]'s as-given fallback must not be used here,
/// since a failed canonicalisation would echo back an unproven spelling as if
/// the filesystem had vouched for it.
fn logical_path_names_staged_entry(logical_path: &Path, canonical_stage_roots: &[PathBuf]) -> bool {
    let (Some(parent), Some(name)) = (logical_path.parent(), logical_path.file_name()) else {
        return false;
    };
    let Ok(canonical_parent) = std::fs::canonicalize(parent) else {
        return false;
    };
    let reconstructed = canonical_parent.join(name);
    canonical_stage_roots
        .iter()
        .any(|root| reconstructed.starts_with(root))
}

/// True when any path component is literally `node_modules` — i.e. the path
/// was reached by resolving a bare package specifier through some install
/// root, rather than a relative/project-rooted import. Used by
/// [`audit_metafile_stage_escape`] to tell a package-name resolution (guard
/// (a)'s scope, cases 2/3 of the epic's predicate) apart from an ordinary
/// source path (cases 1/4).
fn has_node_modules_segment(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c, Component::Normal(name) if name == "node_modules"))
}

/// One metafile input, resolved under the two spellings every post-esbuild
/// audit in this module needs:
///
/// - `logical_path`: `key` joined onto `logical_root`, with NO
///   canonicalisation and no disk lookup (esbuild's own recorded spelling,
///   taken at face value).
/// - `canonical_path`: `key`'s real on-disk path with symlinks followed,
///   tried against `canonical_roots` in order (first existing hit wins), or
///   `None` when it resolves under none of them.
///
/// `PathBuf::join` already treats an absolute `key` as replacing the base
/// entirely, so both spellings degrade correctly for the `node_modules`
/// canonicalised-absolute-key case without a separate branch here.
///
/// This is the shared dual-spelling machinery [`audit_metafile_exclusions`]
/// used to inline directly; factored out (issue #1704) so
/// [`audit_metafile_stage_escape`] can reuse it with different roots.
/// `audit_metafile_exclusions` keeps its existing project-rooted logical
/// spelling and project-root-then-shadow-root canonicalisation fallback
/// (`logical_root = project_root`, `canonical_roots = [project_root,
/// shadow_root]`) — behaviorally unchanged. `audit_metafile_stage_escape` is
/// cwd-rooted on both counts: a widened workspace stage runs esbuild from a
/// directory nested BELOW the stage root, so its keys must never be assumed
/// project-rooted.
struct MetafileInputResolution<'a> {
    key: &'a str,
    logical_path: PathBuf,
    canonical_path: Option<PathBuf>,
}

fn resolve_metafile_inputs<'a>(
    meta: &'a Metafile,
    logical_root: &Path,
    canonical_roots: &[&Path],
) -> Vec<MetafileInputResolution<'a>> {
    meta.inputs
        .keys()
        .filter(|key| !is_synthetic_input(key))
        .map(|key| {
            let key_path = Path::new(key.as_str());
            let logical_path = logical_root.join(key_path);
            let canonical_path = if key_path.is_absolute() {
                canonical_or_self(key_path)
            } else {
                canonical_roots.iter().find_map(|root| {
                    let candidate = root.join(key_path);
                    if candidate.exists() {
                        canonical_or_self(&candidate)
                    } else {
                        None
                    }
                })
            };
            MetafileInputResolution {
                key,
                logical_path,
                canonical_path,
            }
        })
        .collect()
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
    for record in resolve_metafile_inputs(&meta, project_root, &[project_root, shadow_root]) {
        // Spelling 1: the logical key, as esbuild recorded it, resolved
        // against the project root — no canonicalisation, no disk lookup.
        if is_excluded(&record.logical_path) {
            offenders.insert(record.key.to_string());
            continue;
        }

        // Spelling 2: the same key, mapped to its real on-disk path (may
        // resolve through a symlink, landing somewhere the logical spelling
        // above does not point at).
        if let Some(real) = record.canonical_path {
            if is_excluded(&real) {
                offenders.insert(record.key.to_string());
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

/// Guard (b) primitive (issue #1704, epic #1702): fail-closed stage-escape
/// audit for a REAL esbuild metafile emitted from inside a widened stage.
/// `bundle.exclude`'s dual-spelling audit above proves nothing EXCLUDED
/// leaked in; this proves nothing STAGING was meant to isolate (a
/// workspace-sibling package reached via its bare package name, or live
/// first-party source reached without ever producing a staged spelling)
/// escaped the stage boundary either.
///
/// Every non-synthetic input is classified by the epic's four-case
/// predicate:
///
/// 1. a staged source symlink that canonicalises to live first-party source
///    = **ALLOWED** — the intentional staged-mirror/symlink shape the
///    sibling-mirror epic (#1691) relies on. (A copy-mode staged source
///    canonicalises to a path still inside the stage and is allowed too,
///    trivially — see the `in_stage` short-circuit below.)
/// 2. a staged `node_modules/<pkg>` (or `node_modules/@scope/pkg`) symlink
///    that canonicalises to a live WORKSPACE SIBLING (no further
///    `node_modules` segment in the real path) = **OFFENDER** — the exact
///    package-name reach Guard (a) rejects at scan time; this is the
///    metafile backstop for anything that slips past it.
/// 3. an ordinary `node_modules`-nested third-party dependency (pnpm's
///    `.pnpm/<pkg>@<ver>/node_modules/<pkg>` content-addressable layout, or
///    any other real install with a `node_modules` segment still present in
///    its canonical path) = **ALLOWED**.
/// 4. a first-party input recorded OUTSIDE every stage root = **OFFENDER** —
///    esbuild resolved straight into live source without ever producing a
///    staged spelling for it at all (e.g. climbing a workspace-hoisted
///    `node_modules` symlink past the stage boundary via a `package.json`
///    `main`/`imports` field). Whether esbuild wrote that as an absolute
///    path or, running without `--preserve-symlinks`, as a `..`-laden
///    relative path climbing out of `metafile_cwd` (e.g.
///    `../../workspace/packages/shared/index.ts`) is the same escape under
///    two spellings — case 1 and case 4 are told apart by whether the
///    *logical* (pre-canonicalisation, symlink-unaware) spelling itself
///    still names a location inside a stage root, not by whether the key
///    string happens to be absolute. That lexical answer alone is wrong
///    when `metafile_cwd` is itself spelled through a symlink alias whose
///    resolved location sits at a different DEPTH (macOS `/var` ->
///    `/private/var`): esbuild's `..` count is computed against the
///    RESOLVED cwd, so the lexical collapse escapes the stage spelling even
///    for a genuinely staged entry. So when the lexical check fails, the
///    filesystem gets the last word via
///    [`logical_path_names_staged_entry`]: canonicalise the logical path's
///    parent (fail-closed — only a successful canonicalisation may accept)
///    and allow iff the named entry physically sits inside a canonical
///    stage root. Deliberate consequence: a key that lexically LEAVES a
///    stage root and RE-ENTERS one (`../../../alias/stage/…`) is allowed —
///    what matters is that the entry it names IS a staged directory entry,
///    i.e. a staged spelling exists; do not re-tighten this to "never
///    leaves the stage". A genuine climb to live source still
///    parent-canonicalises OUTSIDE every stage root and stays flagged.
///
/// An input whose canonical path falls under neither `first_party_root` nor
/// any `stage_roots` entry (e.g. a genuinely external symlink) is outside
/// this predicate's four-case scope and is left unflagged, as is any input
/// that does not resolve to a real on-disk path at all (mirrors
/// [`route_module_deps`]'s "skip, don't invent" posture).
///
/// `metafile_cwd` is the metafile's OWN working directory — do NOT assume it
/// equals a stage root: a widened workspace stage runs esbuild from a
/// mirrored directory nested BELOW the stage root, so callers must pass the
/// exact directory esbuild ran in (relative metafile keys resolve against
/// it, not against any stage root). `stage_roots` is the full set of roots a
/// resolved input may legitimately live under (a build can stage into more
/// than one root — e.g. a shadow root plus a separately wholesale-mirrored
/// sibling root). `first_party_root` is the outer boundary of "this build's
/// own source" (the project root, or the widened workspace root for a
/// sibling-mirrored build).
///
/// Callers are expected to invoke this once per REAL esbuild subprocess in a
/// widened stage, immediately after that subprocess succeeds — wiring which
/// call sites invoke it, and when a stage counts as "widened", is wave 2/3
/// (#1705/#1707), not this primitive.
pub fn audit_metafile_stage_escape(
    metafile_bytes: &[u8],
    metafile_cwd: &Path,
    stage_roots: &[&Path],
    first_party_root: &Path,
) -> Result<()> {
    let meta: Metafile = serde_json::from_slice(metafile_bytes)
        .map_err(|e| anyhow!("zfb bundler: stage-escape audit failed to parse metafile: {e}"))?;

    let canonical_stage_roots: Vec<PathBuf> = stage_roots
        .iter()
        .map(|root| canonical_or_self(root).unwrap_or_else(|| root.to_path_buf()))
        .collect();
    let canonical_first_party_root =
        canonical_or_self(first_party_root).unwrap_or_else(|| first_party_root.to_path_buf());
    // Lexically (not canonically — no symlink-following, no disk lookup)
    // normalised stage roots, for the case-1-vs-case-4 "did a staged
    // spelling ever exist" check below. Must NOT be the canonical roots
    // above: the whole point is to compare against the pre-symlink-
    // resolution shape of both sides.
    let lexical_stage_roots: Vec<PathBuf> = stage_roots
        .iter()
        .map(|root| normalize_path_lexical(root))
        .collect();

    let mut offenders: Vec<String> = Vec::new();
    for record in resolve_metafile_inputs(&meta, metafile_cwd, &[metafile_cwd]) {
        let Some(canonical) = record.canonical_path else {
            continue;
        };

        let package_shaped = has_node_modules_segment(Path::new(record.key));
        let canonical_in_node_modules = has_node_modules_segment(&canonical);
        let in_stage = canonical_stage_roots
            .iter()
            .any(|root| canonical.starts_with(root));
        let in_first_party = canonical.starts_with(&canonical_first_party_root);

        if package_shaped {
            // Cases 2/3: resolved via a bare package specifier through some
            // node_modules root.
            if canonical_in_node_modules {
                continue; // case 3: ordinary third-party dep, allowed.
            }
            if in_first_party {
                // case 2: the symlink at node_modules/<pkg> canonicalises
                // straight to workspace source, no node_modules layer left.
                offenders.push(format!(
                    "{} (package import resolved outside node_modules to workspace sibling {})",
                    record.key,
                    canonical.display()
                ));
            }
            continue;
        }

        if in_stage {
            continue; // ordinary staged source, still inside the stage.
        }

        // The canonical (symlink-followed) path escaped the stage. Case 1
        // vs case 4 hinges on whether a genuine staged spelling ever
        // existed: does the LOGICAL path (`metafile_cwd.join(key)`,
        // lexically normalised — `..` collapsed WITHOUT touching the
        // filesystem, so a symlink target is never followed here) still
        // land inside a stage root? A key like "components/Header.tsx"
        // does (the symlink itself is a real filesystem entry inside the
        // stage) — case 1, allowed. A key like
        // "../../workspace/packages/shared/index.ts", OR an equivalent
        // already-absolute key, lexically escapes the stage before any
        // canonicalisation even happens — case 4, no staged spelling was
        // ever produced, flag it regardless of which spelling esbuild used.
        let logical_in_stage = lexical_stage_roots
            .iter()
            .any(|root| normalize_path_lexical(&record.logical_path).starts_with(root));

        if logical_in_stage {
            continue; // case 1: allowed by design.
        }

        // The lexical pass above is symlink-blind in BOTH directions: a
        // symlink-aliased `metafile_cwd` spelling at a different depth than
        // its resolved location (macOS `/var` -> `/private/var`) makes it
        // fail for a genuinely staged entry. Ask the filesystem before
        // flagging: canonicalise the logical path's parent and accept iff
        // the named entry physically sits inside a canonical stage root.
        // Kept AFTER the cheap lexical pass so the syscall only runs for
        // inputs whose canonical path already left the stage.
        if logical_path_names_staged_entry(&record.logical_path, &canonical_stage_roots) {
            continue; // still case 1, through an aliased cwd spelling.
        }

        if in_first_party {
            // case 4: esbuild never produced a staged spelling for this
            // input at all — it recorded the live first-party path
            // directly, whether as an absolute key or a `..`-climbing one.
            offenders.push(format!(
                "{} (first-party input resolved outside every stage root, no staged spelling)",
                record.key
            ));
        }
        // Otherwise: a path outside first-party territory entirely — out of
        // this predicate's four-case scope, left unflagged.
    }

    if offenders.is_empty() {
        return Ok(());
    }

    bail!(
        "zfb bundler: stage-escape audit — the following metafile input(s) escaped their stage: {}",
        offenders.join(", ")
    );
}

/// Convenience wrapper over [`audit_metafile_stage_escape`]: read the
/// metafile from `metafile_path` first. Fail-closed extends to the read
/// itself — a missing or unreadable metafile is a build error, exactly like
/// malformed JSON, mirroring [`audit_metafile_exclusions_at_path`].
pub fn audit_metafile_stage_escape_at_path(
    metafile_path: &Path,
    metafile_cwd: &Path,
    stage_roots: &[&Path],
    first_party_root: &Path,
) -> Result<()> {
    let bytes = std::fs::read(metafile_path).map_err(|e| {
        anyhow!(
            "zfb bundler: stage-escape audit failed to read metafile {}: {e}",
            metafile_path.display()
        )
    })?;
    audit_metafile_stage_escape(&bytes, metafile_cwd, stage_roots, first_party_root)
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

    // --- audit_metafile_stage_escape (issue #1704) --------------------

    #[test]
    fn stage_escape_allows_staged_source_symlink_to_first_party() {
        // Preserve-symlinks staging shape: the staged entry is itself a
        // symlink whose target is real first-party source that lives
        // outside the stage. This is case 1 of the epic's predicate —
        // exactly the sibling-mirror pattern (#1691) — and must be allowed.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let stage = base.join("stage");
        let first_party = base.join("workspace");
        let real_sibling = first_party.join("packages/shared/src/Header.tsx");
        write(&first_party, "packages/shared/src/Header.tsx", "header");

        let link = stage.join("components/Header.tsx");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_sibling, &link).unwrap();
        #[cfg(not(unix))]
        std::fs::write(&link, "header").unwrap();

        let metafile = br#"{"inputs": {"components/Header.tsx": {"imports": []}}}"#;

        let result = audit_metafile_stage_escape(metafile, &stage, &[&stage], &first_party);
        assert!(
            result.is_ok(),
            "a staged source symlink resolving to live first-party source must be allowed; got {result:?}"
        );
    }

    #[test]
    fn stage_escape_allows_staged_source_copy_within_stage() {
        // Copy-mode staging shape: the staged entry is a plain copy, not a
        // symlink, so its canonical path never leaves the stage at all.
        // Trivially allowed — nothing escaped.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let stage = base.join("stage");
        let first_party = base.join("workspace");
        write(&stage, "components/Header.tsx", "header");

        let metafile = br#"{"inputs": {"components/Header.tsx": {"imports": []}}}"#;

        let result = audit_metafile_stage_escape(metafile, &stage, &[&stage], &first_party);
        assert!(
            result.is_ok(),
            "a copy-mode staged source with no escape must be allowed; got {result:?}"
        );
    }

    #[test]
    fn stage_escape_flags_package_name_symlink_to_workspace_sibling() {
        // Case 2: a staged node_modules/@scope/pkg symlink canonicalises
        // straight to live workspace source (no further node_modules
        // segment) — the exact package-name reach Guard (a) blocks at scan
        // time. This is the metafile backstop and must be an offender.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let stage = base.join("stage");
        let first_party = base.join("workspace");
        let real_sibling_pkg = first_party.join("packages/design-system/Button.tsx");
        write(&first_party, "packages/design-system/Button.tsx", "btn");

        let link = stage.join("node_modules/@ds/Button.tsx");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_sibling_pkg, &link).unwrap();
        #[cfg(not(unix))]
        std::fs::write(&link, "btn").unwrap();

        let metafile = br#"{"inputs": {"node_modules/@ds/Button.tsx": {"imports": []}}}"#;

        let result = audit_metafile_stage_escape(metafile, &stage, &[&stage], &first_party);

        #[cfg(unix)]
        {
            let err = result.expect_err(
                "a node_modules package symlink canonicalising to a workspace sibling must be an offender",
            );
            let msg = err.to_string();
            assert!(
                msg.contains("stage-escape audit"),
                "error must name the stage-escape audit; got {msg:?}"
            );
            assert!(
                msg.contains("node_modules/@ds/Button.tsx"),
                "error must name the offending metafile key; got {msg:?}"
            );
        }
        #[cfg(not(unix))]
        {
            // No symlink semantics off unix in this test harness — the
            // write() fallback keeps the staged path a real file under
            // node_modules, which case 3 (ordinary third-party dep) allows.
            let _ = result;
        }
    }

    #[test]
    fn stage_escape_allows_ordinary_pnpm_third_party_dep() {
        // Case 3: an ordinary node_modules-nested third-party dependency —
        // pnpm's `.pnpm/<pkg>@<ver>/node_modules/<pkg>` content-addressable
        // layout — must be allowed. Unlike the offender tests above, this
        // one holds on every platform: whether or not the staged entry is a
        // real symlink, its resolved path always keeps a `node_modules`
        // segment (the store layout puts one there deliberately).
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let stage = base.join("stage");
        let store = base.join("store/.pnpm/preact@10.0.0/node_modules/preact");
        write(&store, "index.js", "preact");

        let link = stage.join("node_modules/preact/index.js");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(store.join("index.js"), &link).unwrap();
        #[cfg(not(unix))]
        std::fs::write(&link, "preact").unwrap();

        let first_party = base.join("workspace");
        let metafile = br#"{"inputs": {"node_modules/preact/index.js": {"imports": []}}}"#;

        let result = audit_metafile_stage_escape(metafile, &stage, &[&stage], &first_party);
        assert!(
            result.is_ok(),
            "an ordinary node_modules-nested third-party dep must be allowed; got {result:?}"
        );
    }

    #[test]
    fn stage_escape_flags_absolute_first_party_input_outside_stage() {
        // Case 4: esbuild recorded an already-absolute, real path directly —
        // no staged spelling was ever produced for this input. Reaching
        // live first-party source outside every stage root this way (e.g.
        // by climbing a workspace-hoisted node_modules symlink past the
        // stage boundary) is exactly the "resurrection" escape the audit
        // exists to catch.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let stage = base.join("stage");
        let first_party = base.join("workspace");
        let real_file = first_party.join("pages/other.tsx");
        write(&first_party, "pages/other.tsx", "other");

        let metafile = format!(
            r#"{{"inputs": {{"{}": {{"imports": []}}}}}}"#,
            real_file.display()
        );

        let result =
            audit_metafile_stage_escape(metafile.as_bytes(), &stage, &[&stage], &first_party);
        let err = result.expect_err(
            "an absolute first-party input outside every stage root must be an offender",
        );
        let msg = err.to_string();
        assert!(
            msg.contains("stage-escape audit"),
            "error must name the stage-escape audit; got {msg:?}"
        );
        assert!(
            msg.contains(&real_file.display().to_string()),
            "error must name the offending absolute path; got {msg:?}"
        );
    }

    #[test]
    fn stage_escape_flags_relative_dotdot_climb_to_first_party_without_a_staged_spelling() {
        // Case 4 via the OTHER spelling esbuild can choose: running without
        // `--preserve-symlinks`, esbuild does not necessarily write an
        // already-canonicalised ABSOLUTE key for a resolution that reaches
        // outside its cwd — it can write a `..`-laden path relative to
        // `metafile_cwd` instead (e.g. `../workspace/pages/other.tsx`). No
        // symlink is even needed to construct this: it is purely a claim,
        // via `..` components, that a location OUTSIDE the stage was
        // resolved directly, with no staged spelling ever produced for it.
        // A predicate keyed on `key.is_absolute()` alone misses this
        // spelling entirely — it must be caught the same way case 4 always
        // is, via the lexically-normalised logical path escaping the stage.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let stage = base.join("stage");
        let first_party = base.join("workspace");
        write(&first_party, "pages/other.tsx", "other");
        std::fs::create_dir_all(&stage).unwrap();

        let metafile = br#"{"inputs": {"../workspace/pages/other.tsx": {"imports": []}}}"#;

        let result = audit_metafile_stage_escape(metafile, &stage, &[&stage], &first_party);
        let err = result.expect_err(
            "a relative `..`-climbing key reaching first-party source with no staged spelling must be an offender",
        );
        assert!(
            err.to_string().contains("../workspace/pages/other.tsx"),
            "error must name the offending relative key; got {err}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn stage_escape_allows_staged_symlink_reached_through_symlink_aliased_cwd() {
        // Repro for issue #1795 (epic #1794): on macOS `$TMPDIR` is spelled
        // `/var/folders/…` while `/var` is a symlink to `/private/var`, so
        // the alias spelling is one component SHALLOWER than the
        // kernel-resolved location. esbuild's Go-side cwd is the RESOLVED
        // spelling, so the `..` count in a relative metafile key matches the
        // resolved depth; joining that key onto the unresolved
        // `metafile_cwd` and collapsing lexically pops one component too
        // many and never re-matches the stage root spelling — a
        // legitimately staged symlink (case 1) was misclassified as a
        // case-4 escape. Portable mirror of that topology: `alias ->
        // physical/real` (depth-changing; a same-depth sibling alias does
        // NOT reproduce the bug).
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();

        let real_stage = base.join("physical/real/stage");
        std::fs::create_dir_all(base.join("physical/real")).unwrap();
        std::os::unix::fs::symlink(base.join("physical/real"), base.join("alias")).unwrap();

        let first_party = base.join("workspace");
        let real_source = first_party.join("packages/shared/src/Header.tsx");
        write(&first_party, "packages/shared/src/Header.tsx", "header");

        let link = real_stage.join("components/Header.tsx");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&real_source, &link).unwrap();

        // The caller passes the ALIAS spelling for both the cwd and the
        // stage root, exactly as the macOS `$TMPDIR`-derived paths arrive.
        let alias_stage = base.join("alias/stage");

        // The `..` count matches the RESOLVED cwd depth
        // (`physical/real/stage` = three components below `base`), and the
        // key re-enters the stage through the alias spelling.
        let key = "../../../alias/stage/components/Header.tsx";

        // Precondition 1: the pure-lexical comparison FAILS — the `..`
        // count exceeds the alias spelling's depth, so the lexical collapse
        // escapes `base` and never re-matches the lexical stage root.
        let lexical_joined = normalize_path_lexical(&alias_stage.join(key));
        assert!(
            !lexical_joined.starts_with(normalize_path_lexical(&alias_stage)),
            "precondition: the pure-lexical comparison must fail; got {lexical_joined:?}"
        );

        // Precondition 2: canonicalising the same logical path's PARENT
        // lands under the CANONICAL stage root — a staged spelling exists.
        let logical = alias_stage.join(key);
        let canonical_parent = std::fs::canonicalize(logical.parent().unwrap())
            .expect("precondition: the logical parent must canonicalise");
        assert!(
            canonical_parent.starts_with(std::fs::canonicalize(&real_stage).unwrap()),
            "precondition: the canonical parent must land inside the canonical stage root; got {canonical_parent:?}"
        );

        let metafile = format!(r#"{{"inputs": {{"{key}": {{"imports": []}}}}}}"#);

        let result = audit_metafile_stage_escape(
            metafile.as_bytes(),
            &alias_stage,
            &[&alias_stage],
            &first_party,
        );
        assert!(
            result.is_ok(),
            "a staged symlink reached through a symlink-aliased cwd spelling is case 1 and must be allowed; got {result:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn stage_escape_flags_genuine_climb_to_live_source_under_symlink_aliased_cwd() {
        // Guard-not-weakened companion to the aliased-cwd acceptance above:
        // a GENUINE case-4 climb — a key that reaches live first-party
        // source directly, never naming a staged entry — must STILL be
        // flagged when the cwd is spelled through the depth-changing alias.
        // Its logical parent canonicalises fine, but lands under the live
        // workspace, not under any canonical stage root.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();

        std::fs::create_dir_all(base.join("physical/real/stage")).unwrap();
        std::os::unix::fs::symlink(base.join("physical/real"), base.join("alias")).unwrap();

        let first_party = base.join("workspace");
        write(&first_party, "pages/other.tsx", "other");

        let alias_stage = base.join("alias/stage");
        // Same resolved-depth `..` count as the acceptance repro, but the
        // climb descends into live workspace source instead of re-entering
        // the stage through the alias.
        let key = "../../../workspace/pages/other.tsx";

        let metafile = format!(r#"{{"inputs": {{"{key}": {{"imports": []}}}}}}"#);

        let result = audit_metafile_stage_escape(
            metafile.as_bytes(),
            &alias_stage,
            &[&alias_stage],
            &first_party,
        );
        let err = result.expect_err(
            "a genuine `..`-climb to live source must stay flagged under an aliased cwd spelling",
        );
        assert!(
            err.to_string().contains(key),
            "error must name the offending key; got {err}"
        );
    }

    #[test]
    fn stage_escape_retains_offender_when_logical_parent_cannot_canonicalize() {
        // Fail-closed: the aliased-cwd acceptance path may only accept when
        // canonicalising the logical parent SUCCEEDS. An absolute
        // first-party key whose path does not exist on disk
        // (`canonical_or_self` falls back to the as-given spelling in
        // `resolve_metafile_inputs`) has an unprovable spelling — the
        // offender must be retained, never accepted on the fallback.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let stage = base.join("stage");
        std::fs::create_dir_all(&stage).unwrap();
        let first_party = base.join("workspace");
        std::fs::create_dir_all(&first_party).unwrap();

        let ghost = first_party.join("ghost/dir/file.tsx"); // never created
        assert!(
            std::fs::canonicalize(ghost.parent().unwrap()).is_err(),
            "precondition: the logical parent must NOT canonicalise"
        );

        let metafile = format!(
            r#"{{"inputs": {{"{}": {{"imports": []}}}}}}"#,
            ghost.display()
        );

        let result =
            audit_metafile_stage_escape(metafile.as_bytes(), &stage, &[&stage], &first_party);
        let err =
            result.expect_err("an unprovable first-party spelling must be retained as an offender");
        assert!(
            err.to_string().contains("ghost/dir/file.tsx"),
            "error must name the offender; got {err}"
        );
    }

    #[test]
    fn stage_escape_resolves_relative_keys_against_nested_metafile_cwd() {
        // Nested-cwd key resolution: a widened workspace stage runs esbuild
        // from a directory nested BELOW the stage root, so a relative key
        // must resolve against that nested cwd — not the (outer) stage
        // root. Prove this by placing the escaping symlink only under the
        // nested cwd: if the audit mistakenly joined the key onto
        // `stage_roots` instead of `metafile_cwd`, this input would not
        // resolve to a real path at all and would be silently skipped
        // instead of flagged.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let stage = base.join("stage");
        let cwd = stage.join("workspace-mirror/inner");
        let first_party = base.join("workspace");
        let real_sibling_pkg = first_party.join("packages/design-system/Icon.tsx");
        write(&first_party, "packages/design-system/Icon.tsx", "icon");

        let link = cwd.join("node_modules/@ds/Icon.tsx");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_sibling_pkg, &link).unwrap();
        #[cfg(not(unix))]
        std::fs::write(&link, "icon").unwrap();

        let metafile = br#"{"inputs": {"node_modules/@ds/Icon.tsx": {"imports": []}}}"#;

        let result = audit_metafile_stage_escape(metafile, &cwd, &[&stage], &first_party);

        #[cfg(unix)]
        {
            let err = result.expect_err(
                "a package-name escape staged under a nested metafile cwd must still be caught",
            );
            assert!(
                err.to_string().contains("node_modules/@ds/Icon.tsx"),
                "error must name the offending metafile key; got {err}"
            );
        }
        #[cfg(not(unix))]
        {
            let _ = result;
        }
    }

    #[test]
    fn stage_escape_honors_the_full_stage_root_set() {
        // `stage_roots` is a SET, not a single root: a build can widen into
        // more than one root (e.g. a shadow root plus a separately
        // wholesale-mirrored sibling root). An absolute input resolving
        // inside a second, non-cwd stage root must be allowed when that
        // root is present in the set, and flagged when it is omitted —
        // proving the plural parameter is actually consulted, not just
        // accepted and ignored.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let stage_a = base.join("stageA");
        let first_party = base.join("workspace");
        // The second stage root is a sibling mirror staged inside the
        // widened first-party tree, as `mirror_sibling_root` does.
        let stage_b = first_party.join(".zfb-sibling-stage");
        let real_file = stage_b.join("pages/mirrored.tsx");
        write(&stage_b, "pages/mirrored.tsx", "mirrored");

        let metafile = format!(
            r#"{{"inputs": {{"{}": {{"imports": []}}}}}}"#,
            real_file.display()
        );

        let allowed = audit_metafile_stage_escape(
            metafile.as_bytes(),
            &stage_a,
            &[&stage_a, &stage_b],
            &first_party,
        );
        assert!(
            allowed.is_ok(),
            "an absolute input inside a recognised second stage root must be allowed; got {allowed:?}"
        );

        let flagged =
            audit_metafile_stage_escape(metafile.as_bytes(), &stage_a, &[&stage_a], &first_party);
        assert!(
            flagged.is_err(),
            "the same input must be flagged once its stage root is omitted from the set"
        );
    }

    #[test]
    fn stage_escape_fails_closed_on_malformed_metafile() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let err = audit_metafile_stage_escape(b"not json", &root, &[&root], &root)
            .expect_err("a malformed metafile must be a build error, not an empty pass");
        assert!(
            err.to_string().contains("stage-escape audit"),
            "fail-closed error must name the stage-escape audit; got {err}"
        );
    }

    #[test]
    fn stage_escape_fails_closed_on_missing_metafile_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let missing = root.join("does-not-exist/metafile.json");

        let err = audit_metafile_stage_escape_at_path(&missing, &root, &[&root], &root)
            .expect_err("an unreadable metafile path must be a build error, not an empty pass");
        assert!(
            err.to_string().contains("stage-escape audit"),
            "fail-closed error must name the stage-escape audit; got {err}"
        );
    }
}
