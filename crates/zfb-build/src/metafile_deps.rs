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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use serde::Deserialize;
use zfb_types::{has_node_modules_segment, normalize_path_lexical};

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

/// Case-2 acceptance rule (issue #2040, epic #1982): is this package-shaped
/// metafile input a location the target first-party workspace package
/// **itself declares** as an entry root?
///
/// # Why case 2 needed redefining
///
/// The audit's original theory was that a bare package-name import landing on
/// live workspace source is always an escape — which silently assumes every
/// workspace package ships a built `dist/`. It does not: consuming a sibling
/// **from source** (`package.json` `exports` pointing straight at `./src/*`,
/// no build step) is a deliberate, increasingly common monorepo architecture.
/// The resolved target there is still first-party workspace source; it is
/// merely reached by package name instead of by relative path, so it is not
/// something staging was meant to isolate. #1730's real-world repro hard-failed
/// on exactly that shape with 70+ offender inputs.
///
/// # What is read — and what is deliberately NOT
///
/// Only **declared** data, in the same category as `pnpm-workspace.yaml`
/// membership: the target package's own `package.json` `name` and its
/// `exports` / `main` / `module` entry declarations. This is emphatically not
/// a resolver — it never probes extensions, never falls back to an index
/// file, never replicates conditional-`exports` resolution order, and never
/// walks `node_modules`. esbuild has already resolved everything; this only
/// classifies the input esbuild recorded. (`browser` is not read: it is a
/// substitution map, not an entry declaration.)
///
/// # The rule
///
/// All of the following must hold, else the input stays an offender:
///
/// 1. the key is package-shaped — `.../node_modules/<pkg>/<subpath>` — and
///    the package-relative `<subpath>` is non-empty
///    ([`split_package_name_and_subpath`]);
/// 2. the canonical path's trailing components are exactly that `<subpath>`,
///    so the package root is plain arithmetic on two known strings rather
///    than a lookup ([`package_root_for_input`]);
/// 3. `<package root>/package.json` exists and its `name` is the package name
///    the key was reached under — the link and the package agree;
/// 4. the package root is **claimed** by the governing
///    `pnpm-workspace.yaml`'s `packages:` globs
///    ([`zfb_types::first_party::workspace_root_claims_path`], the declared-
///    membership half of #1986's eligibility predicate) — an unclaimed
///    directory inside the workspace tree is not a workspace package;
/// 5. `<subpath>` is covered by one of the package's **declared entries**
///    ([`declared_entries`]).
///
/// Condition 5 is what keeps this from becoming a blanket "workspace siblings
/// are always fine" exemption. A package whose only declaration is
/// `"main": "./dist/index.js"` declares the entry root `dist/`; a deep import
/// reaching `src/internal.ts` past that built entry is undeclared, escapes the
/// stage, and stays rejected. A package declaring `"exports": {"./*":
/// "./src/*"}` declares `src/`, so its whole source tree is accepted. A
/// package declaring a root-level FILE (`"main": "./index.ts"`) declares the
/// package root only when it declares no directory entry alongside it — see
/// [`declared_entries_cover`] for why both halves of that rule are needed.
/// `{"main": "./index.js", "exports": {"./*": "./dist/*"}}` is an ordinary
/// dist-shipping package, and reading its root `main` as a blanket
/// package-wide grant would authorise every `src/` file behind the built
/// entry.
///
/// On success this returns the accepted package's declared name, on-disk root
/// and declared entry roots rather than a bare `bool`, so the stage-escape
/// audit's own case-2 acceptance and epic #2078 Sub 10a's enrolment-selection
/// contract ([`accepted_enrolment_set`]) share exactly one implementation of
/// the rule and can never independently drift on which inputs are accepted.
/// (Through issue #2127 a `package_input_is_declared_first_party_entry`
/// predicate wrapper stood in front of this for the audit's own call site;
/// the audit now classifies through [`classify_package_shaped_input`] and
/// needs the identity itself, so the wrapper is gone and its documentation
/// lives here.)
///
/// This is the **symlinked** staging shape. Its real-copy counterpart —
/// where condition 4 is structurally unsatisfiable and is replaced by a
/// declared-NAME claim — is [`staged_copy_declared_first_party_identity`].
fn declared_first_party_package_identity_from_key(
    key: &str,
    canonical: &Path,
    canonical_first_party_root: &Path,
) -> Option<AcceptedPackage> {
    let (package_name, subpath) = split_package_name_and_subpath(key)?;
    let package_root = package_root_for_input(canonical, &subpath)?;
    let manifest = std::fs::read_to_string(package_root.join("package.json")).ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest).ok()?;
    if manifest.get("name").and_then(|n| n.as_str()) != Some(package_name.as_str()) {
        return None;
    }
    if !zfb_types::first_party::workspace_root_claims_path(
        canonical_first_party_root,
        &package_root,
    ) {
        return None;
    }

    let subpath = subpath.join("/");
    let entries = declared_entries(&manifest);
    if !declared_entries_cover(&entries, &subpath) {
        return None;
    }
    Some(AcceptedPackage {
        name: package_name,
        package_root,
        declared_entry_roots: declared_entry_roots_from(&entries),
    })
}

/// Split a `node_modules`-shaped metafile key into `(package name,
/// package-relative subpath segments)`, keyed on the LAST `node_modules`
/// segment (the install root the specifier actually resolved through).
/// `@scope/pkg` consumes two segments, the only nesting pnpm's public layout
/// produces. Returns `None` when no package-relative subpath remains — the
/// bare package directory itself is never a bundled input.
fn split_package_name_and_subpath(key: &str) -> Option<(String, Vec<String>)> {
    let segments: Vec<&str> = key
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    let last_node_modules = segments.iter().rposition(|s| *s == "node_modules")?;
    let rest = &segments[last_node_modules + 1..];
    let (name, tail) = if rest.first()?.starts_with('@') {
        if rest.len() < 2 {
            return None;
        }
        (format!("{}/{}", rest[0], rest[1]), &rest[2..])
    } else {
        (rest[0].to_string(), &rest[1..])
    };
    if tail.is_empty() {
        return None;
    }
    Some((name, tail.iter().map(|s| (*s).to_string()).collect()))
}

/// The package root `canonical` sits in, derived by stripping the key's
/// package-relative `subpath` off its tail.
///
/// Fail-closed: every stripped component must match the corresponding subpath
/// segment. When the canonical spelling and the key's spelling disagree (the
/// key was rewritten, the link points somewhere structurally different), no
/// package root is claimed and the caller falls back to rejecting the input —
/// this is not a search for a plausible root.
fn package_root_for_input(canonical: &Path, subpath: &[String]) -> Option<PathBuf> {
    let mut root = canonical.to_path_buf();
    for expected in subpath.iter().rev() {
        if root.file_name().and_then(|s| s.to_str()) != Some(expected.as_str()) {
            return None;
        }
        if !root.pop() {
            return None;
        }
    }
    Some(root)
}

/// The canonical-path-keyed sibling of [`package_root_for_input`] above
/// (issue #2047/#2086): used when the metafile KEY itself is not
/// package-shaped, so there is no `node_modules`-relative subpath to strip
/// from it in the first place. Under copy-mode staging (`node_modules_dir`
/// set, no `--preserve-symlinks` — `esbuild_will_preserve_symlinks`,
/// bundler.rs :10153/:2019), esbuild canonicalises a staged
/// `node_modules/<pkg>` symlink back to a `..`-climbing relative path
/// instead of a `node_modules/...`-shaped one, so [`split_package_name_and_subpath`]
/// finds no `node_modules` segment in the key at all.
///
/// Walks upward from `canonical`'s parent directory to the nearest ancestor
/// carrying a `package.json`, bounded so the walk never climbs above
/// `first_party_root` — this is a search for the nearest package boundary,
/// never a probe for a plausible root outside first-party territory.
///
/// Fail-closed: no `package.json` found within that bound yields `None`, and
/// the caller falls back to the existing case-4 rejection.
fn nearest_package_root_for_canonical(
    canonical: &Path,
    first_party_root: &Path,
) -> Option<PathBuf> {
    let mut dir = canonical.parent()?.to_path_buf();
    loop {
        if !dir.starts_with(first_party_root) {
            return None;
        }
        if dir.join("package.json").is_file() {
            return Some(dir);
        }
        if dir == first_party_root {
            return None;
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// The canonical-key sibling of [`declared_first_party_package_identity_from_key`]
/// above (issue #2047, sub #2086): consulted only when the metafile key is
/// NOT package-shaped but its canonical path still lands inside
/// `canonical_first_party_root` — the copy-mode canonicalisation shape
/// [`nearest_package_root_for_canonical`] documents. Resolves package
/// identity from the CANONICAL PATH via the same declared-data-only rule,
/// minus the "the key names this exact package" cross-check
/// [`declared_first_party_package_identity_from_key`] performs (there is no
/// key-carried package name here to cross-check against):
///
/// 1. the nearest ancestor of `canonical` carrying a `package.json`, bounded
///    to `canonical_first_party_root`
///    ([`nearest_package_root_for_canonical`]);
/// 2. that root's `package.json` must parse and declare a `name` (only a
///    genuine package, not an arbitrary directory that happens to hold a
///    `package.json`, may be exempted);
/// 3. the package root must be **claimed** by the governing
///    `pnpm-workspace.yaml`
///    ([`zfb_types::first_party::workspace_root_claims_path`]);
/// 4. the subpath (`canonical` stripped of that package root) must be
///    covered by the package's own declared entries
///    ([`declared_entries_cover`]) — exactly condition 5 of the
///    node_modules-keyed rule above, no relaxation.
///
/// Fail-closed throughout: any missing `package.json`, unclaimed root, or
/// uncovered subpath returns `false` and the caller's case-4 rejection
/// stands. This is additive only — it never widens what the node_modules-
/// keyed rule already accepts, and it does not replace the case-4 rejection
/// for anything it does not itself cover (e.g. an undeclared deep import
/// reached via a canonicalized key stays rejected, same as before).
fn canonical_input_is_declared_first_party_entry(
    canonical: &Path,
    canonical_first_party_root: &Path,
) -> bool {
    declared_first_party_package_identity_from_canonical(canonical, canonical_first_party_root)
        .is_some()
}

/// Identity-bearing twin of [`canonical_input_is_declared_first_party_entry`]
/// above: the same four conditions, returning the accepted package's identity
/// instead of a bare `bool`. Added for epic #2078 Sub 10a's
/// [`accepted_enrolment_set`], the same reason
/// [`declared_first_party_package_identity_from_key`] carries the identity
/// shape for the node_modules-keyed rule.
fn declared_first_party_package_identity_from_canonical(
    canonical: &Path,
    canonical_first_party_root: &Path,
) -> Option<AcceptedPackage> {
    let package_root = nearest_package_root_for_canonical(canonical, canonical_first_party_root)?;
    let manifest = std::fs::read_to_string(package_root.join("package.json")).ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest).ok()?;
    let name = manifest.get("name").and_then(|n| n.as_str())?.to_string();
    if !zfb_types::first_party::workspace_root_claims_path(
        canonical_first_party_root,
        &package_root,
    ) {
        return None;
    }

    let subpath_rel = canonical.strip_prefix(&package_root).ok()?;
    let subpath: String = subpath_rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");

    let entries = declared_entries(&manifest);
    if !declared_entries_cover(&entries, &subpath) {
        return None;
    }
    Some(AcceptedPackage {
        name,
        package_root,
        declared_entry_roots: declared_entry_roots_from(&entries),
    })
}

/// The declared-name roster of every package the governing
/// `pnpm-workspace.yaml` claims, computed at most once per audit/query pass.
///
/// [`zfb_types::first_party::claimed_workspace_member_names`] walks the whole
/// workspace tree — measured on this repo at ~6600 directories / ~0.48s warm,
/// since it prunes only `node_modules` and `.git` and so descends `target/`,
/// `dist/` and `worktrees/`. That is far too expensive to pay per input, or
/// per ordinary build, so it is built lazily and at most once per pass, the
/// same shape `zfb_types::audit_eligibility`'s own declared-identity branch
/// uses for the identical reason.
///
/// Laziness alone would not have been enough: every ordinary third-party
/// input reaches [`classify_package_shaped_input`] with a `node_modules`
/// segment in its canonical path, so a build that merely imports `preact`
/// would have paid the walk. What actually keeps it off the common path is
/// **gate 1 (locality)**: under an empty `bundle.exclude` — the ordinary
/// configuration — `<shadow>/node_modules` is a wholesale symlink to the live
/// tree, so third-party inputs canonicalise OUTSIDE every stage root and exit
/// before the roster is ever consulted. Measured directly with a temporary
/// probe: **zero** roster builds for the empty-`bundle.exclude` fixture,
/// exactly **one** for the real-copy fixture that needs it. A project with no
/// governing `pnpm-workspace.yaml` costs one failed read and no walk at all —
/// `claimed_workspace_member_names` returns empty before walking anything.
///
/// The remaining cost — one walk per audit pass and one per enrolment query,
/// i.e. twice per SSR bundle, on builds that genuinely real-copy-stage — is
/// accepted rather than shared through a threaded roster, which would mean
/// widening two public function signatures for a cost only paid by the
/// configuration that needs the check.
struct ClaimedMemberRoster<'a> {
    workspace_root: &'a Path,
    members: Option<BTreeMap<String, PathBuf>>,
}

impl<'a> ClaimedMemberRoster<'a> {
    fn new(workspace_root: &'a Path) -> Self {
        Self {
            workspace_root,
            members: None,
        }
    }

    /// The claimed member directory declaring `name`, if the workspace claims
    /// a package by that name at all.
    fn claimed_member_dir(&mut self, name: &str) -> Option<&Path> {
        self.members
            .get_or_insert_with(|| {
                zfb_types::first_party::claimed_workspace_member_names(self.workspace_root)
            })
            .get(name)
            .map(PathBuf::as_path)
    }
}

/// Where a metafile input's canonical path sits, relative to the three
/// boundaries [`classify_package_shaped_input`] discriminates on. Both call
/// sites compute these identically; grouping them keeps the classifier's
/// signature readable and makes it impossible for one site to pass them in a
/// different order than the other.
#[derive(Debug, Clone, Copy)]
struct InputLocality {
    /// The canonical (symlink-resolved) path still contains a `node_modules`
    /// segment.
    canonical_in_node_modules: bool,
    /// The canonical path is inside one of this build's stage roots.
    in_stage: bool,
    /// The canonical path is inside `first_party_root`.
    in_first_party: bool,
}

/// How a package-shaped metafile input classifies under the stage-escape
/// audit's case-2/case-3 boundary.
///
/// Hoisted out of [`audit_metafile_stage_escape`] (issue #2127) so
/// [`accepted_enrolment_set`] decides the boundary through the SAME code
/// rather than through a second copy of the gate. Sub #2088 already shared the
/// identity *leaf* predicates between the two, but each still carried its own
/// `canonical_in_node_modules` / `in_first_party` gate — so widening what the
/// audit accepts here would otherwise have produced a package the audit
/// accepts but the enrolment query skips (accepted-but-not-enrolled, the
/// #2048 defect class epic #2078 exists to eliminate).
enum PackageShapedInput {
    /// Case 3 — an ordinary third-party dependency. Allowed by the audit,
    /// never enrolled.
    ThirdPartyDependency,
    /// Case 2 — a first-party workspace package reached by package name, at a
    /// location the package itself declares as an entry. Allowed by the
    /// audit, and the enrolment set's only member kind. For the real-copy
    /// shape this genuinely is the workspace package and not merely a
    /// name match: gate 3
    /// ([`staged_copy_is_a_copy_of_claimed_member`]) has already proven the
    /// staged manifest agrees with the claimed member's.
    DeclaredFirstPartyEntry(AcceptedPackage),
    /// Case 2 — a first-party workspace package reached by package name at a
    /// location it does NOT declare. An offender; never enrolled.
    UndeclaredFirstPartyEscape { detail: String },
    /// Outside this predicate's four-case scope entirely (neither
    /// `node_modules`-nested nor inside first-party territory). Left
    /// unflagged by the audit; never enrolled.
    OutOfScope,
}

/// The shared case-2/case-3 discriminator for a package-shaped metafile input
/// — the single gate [`audit_metafile_stage_escape`] and
/// [`accepted_enrolment_set`] both classify through.
///
/// # The two staging shapes, and why the canonical path alone cannot tell them apart
///
/// A workspace sibling reached by bare package name arrives in one of two
/// physical shapes, depending on whether `bundle.exclude` is active:
///
/// - **Symlinked** (the historical shape): `<stage>/node_modules/<pkg>` is a
///   symlink to live workspace source, so canonicalising it leaves
///   `node_modules` behind entirely. `canonical_in_node_modules` is false and
///   the package root can be claimed **by path** through
///   `pnpm-workspace.yaml` — condition 4 of
///   [`declared_first_party_package_identity_from_key`]'s rule.
/// - **Real-copy staged** (issue #2127, what active `bundle.exclude`
///   produces): no symlink is created at all; the package is materialised as
///   a genuine copy at `<stage>/node_modules/<pkg>/…`. Canonicalising
///   resolves only ANCESTOR symlinks, so the `node_modules` segment always
///   survives and `canonical_in_node_modules` is trivially true — exactly as
///   it is for an ordinary registry dependency. The staged copy's own path is
///   inside the stage, which no `pnpm-workspace.yaml` claims, so condition 4
///   can never hold for it either.
///
/// Keying case 3 on `canonical_in_node_modules` alone therefore admitted
/// EVERY real-copy-staged package as an "ordinary third-party dependency,
/// allowed" before declared identity was ever consulted — the residual
/// #2050/#2081 escape issue #2127 tracks.
///
/// # The declared-data-only discriminator
///
/// For a real copy, identity is established the same way issue #2087
/// established audit *eligibility* for the same shape: by **declared name**,
/// against [`zfb_types::first_party::claimed_workspace_member_names`]'s
/// roster of every package `pnpm-workspace.yaml` claims. A key's package name
/// that the workspace does not claim is an ordinary dependency and stays case
/// 3 — decided by one string lookup, with no manifest read and no filesystem
/// work, so the blast radius on ordinary `node_modules` traffic is nil. A
/// name the workspace DOES claim gets the identical declared-entry rule case
/// 2 already applies to the symlink shape, with only condition 4 swapped for
/// the declared-name claim just established.
///
/// The roster answers IDENTITY only ("does the workspace claim a package by
/// this name?"). The declared-entry rule then reads the manifest at the
/// input's own package root, exactly as the symlink shape does — see
/// [`staged_copy_declared_first_party_identity`] for why that, and not the
/// claimed member's manifest, is the right source. Only
/// [`AcceptedPackage::package_root`] is taken from the roster, so it names
/// the LIVE workspace source directory for both shapes, which is what an
/// enrolment consumer needs.
///
/// esbuild remains the only resolver throughout: this reclassifies inputs
/// esbuild already recorded and never predicts what it would resolve.
///
/// # Known limit: the roster is the boundary of what can be recognised
///
/// The one fail-OPEN direction here is a claimed member the roster does not
/// list — [`zfb_types::first_party::claimed_workspace_member_names`] skips a
/// member whose `package.json` is missing, unparseable, or nameless, and
/// returns nothing at all when `pnpm-workspace.yaml` cannot be read. Such a
/// package's real-copy staging stays case 3, as it did before this fix. That
/// is deliberate and bounded: it is the SAME roster
/// `zfb_types::stage_escape_audit_eligibility` already trusts to decide
/// whether to arm this audit at all, so the audit is never weaker than the
/// arming decision that precedes it. Widening it would mean inferring
/// membership from something other than declared data.
fn classify_package_shaped_input(
    key: &str,
    canonical: &Path,
    canonical_first_party_root: &Path,
    locality: InputLocality,
    claimed_members: &mut ClaimedMemberRoster<'_>,
) -> PackageShapedInput {
    let InputLocality {
        canonical_in_node_modules,
        in_stage,
        in_first_party,
    } = locality;
    if canonical_in_node_modules {
        // Real-copy shape: the input physically lives under a `node_modules`
        // directory even after canonicalisation. THREE gates stand between
        // that observation and the declared-entry rule, and every one of them
        // exits to case 3 — see this function's own docs for why each is
        // load-bearing.
        //
        // Gate 1, LOCALITY. Only an input inside a root THIS build staged
        // into can be an artifact of our own staging. Anything in a live,
        // vendored, or content-addressable-store `node_modules` outside every
        // stage root is an ordinary dependency by construction, and case 2 is
        // defined as being about staged/first-party locations in the first
        // place. Verified against the real fixture: the #2127 escape resolves
        // to `<shadow>/node_modules/@scope/child/index.ts`, which IS inside
        // the passed stage root (and is NOT under `first_party_root`, which
        // is why locality is checked against the stage, not first-party).
        if !in_stage {
            return PackageShapedInput::ThirdPartyDependency;
        }
        let Some((package_name, subpath)) = split_package_name_and_subpath(key) else {
            // No package-relative subpath in the key at all, so no package
            // can be named — the bare package directory is never a bundled
            // input. Nothing to classify; today's case-3 pass stands.
            return PackageShapedInput::ThirdPartyDependency;
        };
        // Gate 2, DECLARED IDENTITY.
        let Some(member_dir) = claimed_members.claimed_member_dir(&package_name) else {
            return PackageShapedInput::ThirdPartyDependency; // case 3.
        };
        let member_dir = member_dir.to_path_buf();
        // Gate 3, PROVENANCE — see `staged_copy_is_a_copy_of_claimed_member`.
        match staged_copy_declared_first_party_identity(
            &package_name,
            &subpath,
            canonical,
            &member_dir,
        ) {
            Some(StagedCopyClass::DeclaredEntry(package)) => {
                PackageShapedInput::DeclaredFirstPartyEntry(package)
            }
            Some(StagedCopyClass::NotTheClaimedMember) => PackageShapedInput::ThirdPartyDependency,
            None => PackageShapedInput::UndeclaredFirstPartyEscape {
                detail: format!(
                    "{key} (package import resolved to a staged copy of workspace package \
                     {package_name} at a location it does not declare)"
                ),
            },
        }
    } else if in_first_party {
        // Symlinked shape: the link at node_modules/<pkg> canonicalises
        // straight to workspace source, no node_modules layer left.
        match declared_first_party_package_identity_from_key(
            key,
            canonical,
            canonical_first_party_root,
        ) {
            Some(package) => PackageShapedInput::DeclaredFirstPartyEntry(package),
            None => PackageShapedInput::UndeclaredFirstPartyEscape {
                detail: format!(
                    "{} (package import resolved outside node_modules to workspace sibling {})",
                    key,
                    canonical.display()
                ),
            },
        }
    } else {
        PackageShapedInput::OutOfScope
    }
}

/// The real-copy sibling of [`declared_first_party_package_identity_from_key`]
/// (issue #2127): the same declared-entry rule, for a claimed workspace
/// package that was staged as a genuine copy under `<stage>/node_modules`
/// instead of symlinked.
///
/// Conditions 1, 2, 3 and 5 of the case-2 rule are **unchanged and read from
/// exactly the same place**: a package-shaped key with a non-empty subpath
/// ([`split_package_name_and_subpath`]); the canonical path's tail matching
/// that subpath exactly, which yields the package root by arithmetic
/// ([`package_root_for_input`]); that root's `package.json` declaring the
/// name the key reached it under; and the subpath covered by that manifest's
/// own declared entries ([`declared_entries_cover`]). Only condition 4 —
/// "`pnpm-workspace.yaml` claims this package ROOT by path" — differs, because
/// no workspace can ever claim a path inside the stage; the caller has already
/// replaced it with the equivalent declared-NAME claim before calling here
/// (`member_dir` IS the claimed member the roster matched by name).
///
/// # Why the declarations come from the STAGED copy, not from `member_dir`
///
/// The staged copy's manifest is the one that governs what actually shipped,
/// and reading it is also what keeps a name collision from becoming a false
/// build failure. pnpm's store can legitimately hold a PUBLISHED copy of a
/// package the workspace also builds — `node_modules/.pnpm/@acme+ui@1.0.0/
/// node_modules/@acme/ui/dist/index.js`, pulled in transitively by some other
/// dependency. Its name matches the claimed roster, so it reaches this
/// function; but it is an ordinary registry dependency, and judging it
/// against the workspace member's declarations (say `./src/*`, a
/// consume-from-source sibling) would reject its perfectly ordinary
/// `dist/index.js`. Read against its own manifest (`./dist/*`) it is
/// correctly accepted. A genuine staged copy is byte-identical to the live
/// member, so the two readings agree wherever it matters.
///
/// [`AcceptedPackage::package_root`] still reports `member_dir`, the LIVE
/// claimed member directory: that field exists for enrolment consumers, which
/// mirror-copy from workspace source and would be actively misled by a path
/// inside the stage.
///
/// Fail-closed throughout: a canonical path whose tail disagrees with the
/// key's subpath, a staged copy with no readable/parseable `package.json`, a
/// manifest naming a different package than the key reached it under, or a
/// subpath the manifest does not declare all yield `None`, and the caller
/// flags the input. Every one of those paths is reachable only for a package
/// name the workspace itself claims — an ordinary dependency short-circuits
/// to case 3 before ever getting here.
fn staged_copy_declared_first_party_identity(
    package_name: &str,
    subpath: &[String],
    canonical: &Path,
    member_dir: &Path,
) -> Option<StagedCopyClass> {
    let staged_package_root = package_root_for_input(canonical, subpath)?;
    let manifest = std::fs::read_to_string(staged_package_root.join("package.json")).ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest).ok()?;
    if manifest.get("name").and_then(|n| n.as_str()) != Some(package_name) {
        return None;
    }
    let entries = declared_entries(&manifest);
    if !staged_copy_is_a_copy_of_claimed_member(&manifest, &entries, member_dir) {
        // Same name, different package — an ordinary dependency that merely
        // collides with a claimed member's name. Case 3, exactly as before
        // this rule existed.
        return Some(StagedCopyClass::NotTheClaimedMember);
    }
    if !declared_entries_cover(&entries, &subpath.join("/")) {
        return None;
    }
    Some(StagedCopyClass::DeclaredEntry(AcceptedPackage {
        name: package_name.to_string(),
        package_root: member_dir.to_path_buf(),
        declared_entry_roots: declared_entry_roots_from(&entries),
    }))
}

/// What [`staged_copy_declared_first_party_identity`] found once it could read
/// the input's own manifest.
enum StagedCopyClass {
    /// The input IS a staged copy of the claimed workspace member, at a
    /// location that member declares.
    DeclaredEntry(AcceptedPackage),
    /// The input merely SHARES a claimed member's name — a different package
    /// entirely. Case 3, allowed, never enrolled.
    NotTheClaimedMember,
}

/// Gate 3 of the real-copy discriminator (issue #2127 review): is this staged
/// package actually a COPY of the claimed workspace member, or a different
/// package that merely shares its declared name?
///
/// # Why a name match is not enough
///
/// **pnpm 10 defaults `link-workspace-packages` to `false`.** A dependency
/// declared `"@acme/ui": "^1.0.0"` — no `workspace:` protocol — therefore
/// installs the PUBLISHED registry copy even though `pnpm-workspace.yaml`
/// claims a member by that same name, and an active `bundle.exclude` stages
/// that registry copy into `<shadow>/node_modules/@acme/ui/` like any other
/// dependency. Gates 1 and 2 cannot tell it apart from a staged copy of the
/// workspace member. Without this gate it would be judged by the case-2
/// declared-entry rule, and an ordinary dual-format publish
/// (`{"main": "dist/cjs/index.js", "module": "dist/esm/index.js"}`, declaring
/// the entry roots `dist/cjs/` and `dist/esm/`) whose bundle also pulls
/// `dist/shared/chunk.js` — the standard rollup/tsup layout — would HARD-FAIL
/// an ordinary build. A workspace claiming `.` amplifies this: the root
/// package's own name joins the roster, and generic names (`docs`, `app`,
/// `site`) collide with real registry packages.
///
/// # The check, and why these two fields
///
/// Declared data only, and only fields staging copies verbatim: the `version`
/// string and the DECLARED ENTRY SET. Staging materialises a workspace
/// member's `package.json` unchanged, so a genuine staged copy agrees with
/// the live member on both — the invariant
/// [`staged_copy_declared_first_party_identity`]'s own docs already rely on.
/// A registry package disagrees on at least one in any realistic
/// configuration: you depend on a published `^1.0.0` while your member is at
/// some other version, or the two ship different entries.
///
/// Compared semantically (parsed `version`, sorted/deduped entry roots) rather
/// than byte-for-byte: byte equality would break the moment anything
/// reformats a manifest, which is the FAIL-OPEN direction — it would silently
/// re-admit #2127's escape.
///
/// # Residual limit, stated plainly
///
/// A registry package whose name, version AND declared entry set all match the
/// workspace member's is indistinguishable from a staged copy of it by
/// declared data alone, and is judged by the case-2 rule. That needs a project
/// to depend on a published copy of its own member at the exact same version
/// with an identical entry set — a degenerate configuration, and the only
/// alternative would be to inspect file contents or resolve, both of which
/// this audit is forbidden from doing.
fn staged_copy_is_a_copy_of_claimed_member(
    staged_manifest: &serde_json::Value,
    staged_entries: &[DeclaredEntry],
    member_dir: &Path,
) -> bool {
    let Ok(member_manifest) = std::fs::read_to_string(member_dir.join("package.json")) else {
        return false;
    };
    let Ok(member_manifest) = serde_json::from_str::<serde_json::Value>(&member_manifest) else {
        return false;
    };
    let version = |manifest: &serde_json::Value| {
        manifest
            .get("version")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    if version(staged_manifest) != version(&member_manifest) {
        return false;
    }
    declared_entry_roots_from(staged_entries)
        == declared_entry_roots_from(&declared_entries(&member_manifest))
}

/// One entry a `package.json` declares, in the two shapes a target can take.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum DeclaredEntry {
    /// A package-relative directory prefix authorising every subpath under it
    /// (`./src/*` -> `src/`, `./dist/index.js` -> `dist/`). The empty prefix
    /// authorises the whole package and is only ever produced by a
    /// root-level WILDCARD (`./*`).
    Prefix(String),
    /// A concrete file at the package root (`"main": "./index.js"`), which
    /// authorises exactly that one file. Deliberately NOT a prefix: an
    /// ordinary dist-shipping package commonly carries a root `main`, and
    /// reading it as the empty prefix would grant its whole source tree.
    ExactFile(String),
}

impl DeclaredEntry {
    fn covers(&self, subpath: &str) -> bool {
        match self {
            Self::Prefix(prefix) => prefix.is_empty() || subpath.starts_with(prefix),
            Self::ExactFile(file) => subpath == file,
        }
    }
}

/// Whether the package's declared entries authorise `subpath`.
///
/// Almost always this is just "any entry covers it". The one aggregate rule:
/// a root-level FILE entry (`"main": "./index.ts"`) authorises the whole
/// package **only when the package declares no directory entry at all**.
///
/// Both halves are load-bearing:
///
/// - A package whose only declarations are root-level files has no
///   build-artifact directory to keep separate from its source, and its entry
///   necessarily imports its siblings (`./index.ts` -> `./helper.ts`); esbuild
///   records those as inputs too, so treating the root file as *literally*
///   the only authorised input would reject the ordinary consume-from-source
///   shape.
/// - A package that ALSO declares a directory entry does have that
///   separation, and there the root file must authorise only itself:
///   `{"main": "./index.js", "exports": {"./*": "./dist/*"}}` is an ordinary
///   dist-shipping package, and reading its root `main` as a package-wide
///   grant would authorise every `src/` file behind the built entry — the
///   blanket exemption condition 5 exists to prevent.
///
/// This is decided by plain arithmetic over the DECLARED set. It deliberately
/// does not walk the entry's imports: which files the entry actually reaches
/// is esbuild's answer to give, and predicting it in Rust is the failure mode
/// this whole audit is built to avoid.
fn declared_entries_cover(entries: &[DeclaredEntry], subpath: &str) -> bool {
    let declares_directory = entries
        .iter()
        .any(|entry| matches!(entry, DeclaredEntry::Prefix(prefix) if !prefix.is_empty()));
    entries.iter().any(|entry| match entry {
        DeclaredEntry::ExactFile(_) if !declares_directory => true,
        other => other.covers(subpath),
    })
}

/// The entries a `package.json` declares, from `exports` (walked recursively
/// — conditions, subpath maps and arrays are all just nesting around the
/// target strings), `main` and `module`.
///
/// A target carrying a directory component contributes the directory portion
/// of its path up to the first `*`: `./dist/index.js` -> `dist/`, `./src/*`
/// -> `src/`. A target with no directory component contributes an exact file
/// (`./index.ts`) unless it is a wildcard (`./*`), which is the only spelling
/// that declares the package root itself. Targets that are absolute or climb
/// out with `..` are ignored.
///
/// The `./` prefix is **required** for `exports` targets — the spec mandates
/// it, and a bare string there is a package name, not a location inside this
/// package — but **optional** for `main` and `module`, where the bare form
/// (`"main": "dist/index.js"`) is both valid and common.
fn declared_entries(manifest: &serde_json::Value) -> Vec<DeclaredEntry> {
    fn entry(target: &str, require_dot_slash: bool) -> Option<DeclaredEntry> {
        let target = match target.strip_prefix("./") {
            Some(rest) => rest,
            None if require_dot_slash => return None,
            None => target,
        };
        if target.starts_with('/') || target.split('/').any(|segment| segment == "..") {
            return None;
        }
        let (up_to_wildcard, has_wildcard) = match target.find('*') {
            Some(at) => (&target[..at], true),
            None => (target, false),
        };
        Some(match up_to_wildcard.rfind('/') {
            Some(at) => DeclaredEntry::Prefix(up_to_wildcard[..=at].to_string()),
            None if has_wildcard => DeclaredEntry::Prefix(String::new()),
            None => DeclaredEntry::ExactFile(up_to_wildcard.to_string()),
        })
    }

    fn collect(value: &serde_json::Value, require_dot_slash: bool, out: &mut Vec<DeclaredEntry>) {
        match value {
            serde_json::Value::String(target) => {
                out.extend(entry(target, require_dot_slash));
            }
            serde_json::Value::Array(items) => items
                .iter()
                .for_each(|v| collect(v, require_dot_slash, out)),
            serde_json::Value::Object(map) => map
                .values()
                .for_each(|v| collect(v, require_dot_slash, out)),
            _ => {}
        }
    }

    let mut entries = Vec::new();
    for (field, require_dot_slash) in [("exports", true), ("main", false), ("module", false)] {
        if let Some(value) = manifest.get(field) {
            collect(value, require_dot_slash, &mut entries);
        }
    }
    entries
}

/// Stringify a package's declared entries for
/// [`AcceptedPackage::declared_entry_roots`] (epic #2078 Sub 10a): a
/// [`DeclaredEntry::Prefix`] becomes its package-relative directory string
/// (`""` for the whole-package wildcard shape), a [`DeclaredEntry::ExactFile`]
/// becomes its package-relative file string. Sorted and deduplicated so the
/// result does not depend on `exports`/`main`/`module` declaration order.
fn declared_entry_roots_from(entries: &[DeclaredEntry]) -> Vec<String> {
    let mut roots: Vec<String> = entries
        .iter()
        .map(|entry| match entry {
            DeclaredEntry::Prefix(prefix) => prefix.clone(),
            DeclaredEntry::ExactFile(file) => file.clone(),
        })
        .collect();
    roots.sort();
    roots.dedup();
    roots
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
/// 2. a staged `node_modules/<pkg>` (or `node_modules/@scope/pkg`) entry
///    resolving to a first-party WORKSPACE SIBLING = **OFFENDER, unless the
///    sibling package itself declares that location as an entry root**. The
///    exception (issue #2040) is the "consume from source" monorepo idiom:
///    reaching first-party workspace source by package name instead of by
///    relative path is not an escape. Anything the package does NOT declare
///    stays the package-name escape Guard (a) rejects at scan time; this is
///    the metafile backstop for whatever slips past it. Two physical shapes
///    land here, and issue #2127 is why the second one had to be named
///    explicitly (see [`classify_package_shaped_input`] for the full
///    discriminator):
///    - **symlinked** — the staged entry is a link, so its canonical path
///      leaves `node_modules` entirely and the package root can be claimed
///      BY PATH through `pnpm-workspace.yaml`
///      ([`declared_first_party_package_identity_from_key`]);
///    - **real-copy staged** — what an active `bundle.exclude` produces: no
///      link is created and the package is materialised as a genuine copy
///      inside the stage, so its canonical path trivially KEEPS a
///      `node_modules` segment and no workspace can claim its path. Identity
///      is established BY DECLARED NAME instead, against
///      [`zfb_types::first_party::claimed_workspace_member_names`], and the
///      same declared-entry rule then applies
///      ([`staged_copy_declared_first_party_identity`]).
/// 3. an ordinary third-party dependency — a `node_modules`-nested install
///    (pnpm's `.pnpm/<pkg>@<ver>/node_modules/<pkg>` content-addressable
///    layout, or any other real install with a `node_modules` segment still
///    present in its canonical path) whose package name the governing
///    `pnpm-workspace.yaml` does NOT claim = **ALLOWED**. The claimed-name
///    check is what separates this from case 2's real-copy shape: before
///    issue #2127 the surviving `node_modules` segment alone decided case 3,
///    which admitted every real-copy-staged workspace sibling here — allowed
///    before declared identity was ever consulted.
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

    let mut claimed_members = ClaimedMemberRoster::new(&canonical_first_party_root);
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
            // node_modules root. Both staging shapes — a symlink that
            // canonicalises out of node_modules, and a real staged copy that
            // stays inside one — are told apart by
            // [`classify_package_shaped_input`], the single gate
            // [`accepted_enrolment_set`] classifies through too.
            if let PackageShapedInput::UndeclaredFirstPartyEscape { detail } =
                classify_package_shaped_input(
                    record.key,
                    &canonical,
                    &canonical_first_party_root,
                    InputLocality {
                        canonical_in_node_modules,
                        in_stage,
                        in_first_party,
                    },
                    &mut claimed_members,
                )
            {
                offenders.push(detail);
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
            // Canonical-key sibling of the case-2 declared-entry exemption
            // (issue #2047/#2086): `package_shaped` above is checked against
            // the metafile KEY string, but under copy-mode staging esbuild
            // can canonicalise a staged `node_modules/<pkg>` symlink back to
            // a `..`-climbing relative key instead of a `node_modules/...`-
            // shaped one, so this input never entered the case-2/3 branch at
            // all even though its canonical target is a declared, claimed,
            // covered first-party package entry. Resolve package identity
            // from the CANONICAL PATH instead, via the same declared-data-
            // only rule, fail-closed — see
            // [`canonical_input_is_declared_first_party_entry`].
            if canonical_input_is_declared_first_party_entry(
                &canonical,
                &canonical_first_party_root,
            ) {
                continue;
            }

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

// ── Enrolment-selection contract (epic #2078 Sub 10a) ──────────────────────
//
// Sub 9 (#2087) added `zfb_types::first_party::claimed_workspace_member_names`
// — every package a workspace's `pnpm-workspace.yaml` CLAIMS. This section
// answers a different, narrower question: of those claims, which packages
// did THIS bundle session's metafile actually show were REACHED and ACCEPTED
// by the stage-escape audit's declared-entry rule? Sub 10b (SSR coupling,
// `bundler.rs`) and Sub 10c (islands/client coupling, `commands/build.rs`)
// both need exactly this narrower answer before they may mirror-enrol,
// expand macros for, or register watches on a sibling package — see the
// "bounded enrolment set" requirement in epic #2078's Wave 4 restructure.

/// One declared consume-from-source package this bundle session's metafile
/// showed was reached AND accepted by the stage-escape audit's declared-entry
/// rule, in any of its three key/staging shapes —
/// [`declared_first_party_package_identity_from_key`]'s case-2
/// node_modules-keyed symlink form,
/// [`staged_copy_declared_first_party_identity`]'s real-copy form (issue
/// #2127), or [`canonical_input_is_declared_first_party_entry`]'s
/// canonical-key sibling for copy-mode staging (issues #2047/#2086). See
/// [`accepted_enrolment_set`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedPackage {
    /// The package's own declared `package.json` `name`.
    pub name: String,
    /// The package's own root directory on disk (canonical — symlinks
    /// resolved), i.e. the directory holding its `package.json`. This is the
    /// boundary an enrolment pass (Sub 10b/10c) would mirror-copy from or
    /// otherwise register — never a subset chosen by this contract.
    ///
    /// **Always LIVE workspace source, never a staged location.** For the
    /// real-copy staging shape (issue #2127) the inputs esbuild actually read
    /// live inside the stage, and this deliberately names the claimed member's
    /// own directory instead: an enrolment consumer mirrors from workspace
    /// source, and pointing it at a build artifact inside the stage would be
    /// actively misleading. So [`AcceptedEnrolmentSet::reached_inputs`] may
    /// name paths OUTSIDE this root for that shape — see its own docs. The
    /// two are still describing one package: #2127's gate 3
    /// ([`staged_copy_is_a_copy_of_claimed_member`]) admits a staged copy only
    /// after proving it agrees with this directory's manifest on `version` and
    /// on the declared entry set, so `declared_entry_roots` below is identical
    /// whichever of the two manifests it is read from.
    pub package_root: PathBuf,
    /// The package-relative entry locations its OWN `package.json` declares
    /// via `exports`/`main`/`module` — a directory prefix (e.g. `"src/"`, or
    /// `""` for the whole package, the `./*` wildcard shape) or a single file
    /// (e.g. `"index.ts"`). Sorted and deduplicated. This is what bounds
    /// enrolment to what the package itself declares reachable, not its
    /// entire directory tree (build output, tests, tooling config, etc.
    /// included) — see [`declared_entries_cover`]'s own docs for why a
    /// package's declarations, not its whole tree, is the accepted surface.
    pub declared_entry_roots: Vec<String>,
}

/// The bounded enrolment set for one bundle session (epic #2078 Sub 10a —
/// the shared selection contract Sub 10b and Sub 10c couple against). See
/// [`accepted_enrolment_set`] for how this is computed.
///
/// Keyed by each package's own declared `name`, so a package reached through
/// more than one metafile input (e.g. two separate subpaths of the same
/// sibling) is still exactly one entry here, not one per input.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AcceptedEnrolmentSet {
    by_name: BTreeMap<String, AcceptedPackage>,
    inputs_by_name: BTreeMap<String, BTreeSet<PathBuf>>,
}

impl AcceptedEnrolmentSet {
    /// Whether nothing was reached and accepted this session.
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// How many distinct packages were reached and accepted this session.
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// Whether `name` — a package's declared `package.json` `name` — was
    /// reached and accepted this session.
    pub fn contains(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// The accepted package record for `name`, if it was reached and
    /// accepted this session.
    pub fn get(&self, name: &str) -> Option<&AcceptedPackage> {
        self.by_name.get(name)
    }

    /// Every accepted package, in declared-name order.
    pub fn iter(&self) -> impl Iterator<Item = &AcceptedPackage> {
        self.by_name.values()
    }

    /// The canonical paths of the metafile inputs that made `name` accepted —
    /// i.e. the files esbuild ACTUALLY read from that package in this bundle
    /// session, each one individually cleared by the declared-entry rule.
    ///
    /// This is the tightest bound a consumer can key off: strictly the bundled
    /// files, never the package's whole tree and never a location its manifest
    /// merely declares reachable. Epic #2078 Sub 10b (issue #2089) needs
    /// exactly that precision — it turns "this package was accepted" into a
    /// per-FILE question ("did THIS bundled file ship an unexpanded macro?"),
    /// so an unreferenced macro-bearing fixture elsewhere in the package can
    /// never be mistaken for one that reached the bundle. An input REJECTED by
    /// the audit (e.g. an undeclared deep import into a dist-shipping sibling)
    /// is never recorded here, even when a sibling input made the same package
    /// accepted — see `accepted_enrolment_set`'s own docs for that boundary.
    ///
    /// These are the paths esbuild RECORDED, which for the real-copy staging
    /// shape (issue #2127) sit inside the stage rather than under
    /// [`AcceptedPackage::package_root`] — deliberately, since the point of
    /// this method is "which bytes actually reached the bundle", and for that
    /// shape the bytes came from the staged copy. A consumer that needs the
    /// live-source counterpart must map it through `package_root` itself
    /// rather than assume containment.
    ///
    /// Empty for a name that was not accepted this session.
    pub fn reached_inputs(&self, name: &str) -> impl Iterator<Item = &Path> {
        self.inputs_by_name
            .get(name)
            .into_iter()
            .flatten()
            .map(PathBuf::as_path)
    }
}

impl<'a> IntoIterator for &'a AcceptedEnrolmentSet {
    type Item = &'a AcceptedPackage;
    type IntoIter = std::collections::btree_map::Values<'a, String, AcceptedPackage>;

    fn into_iter(self) -> Self::IntoIter {
        self.by_name.values()
    }
}

/// Bounded enrolment-selection contract (epic #2078 Sub 10a): which declared
/// consume-from-source packages did THIS bundle session's metafile show were
/// reached and accepted by the stage-escape audit's declared-entry rule, and
/// what are their declared entry roots?
///
/// This is a QUERY over the exact same metafile
/// [`audit_metafile_stage_escape`] classifies for its own pass — not a
/// second resolution pass, and not an enumeration of workspace membership.
/// It shares the audit's own machinery on BOTH levels, so it can never
/// independently drift from what the audit would accept: the identity-bearing
/// leaf rules (`declared_first_party_package_identity_from_key`/
/// `_from_canonical`) since Sub 10a, and — since issue #2127 — the
/// case-2/case-3 GATE itself, [`classify_package_shaped_input`], which each
/// side previously carried its own copy of. That gate now decides both
/// staging shapes in one place, so widening what the audit accepts (as #2127
/// did for real-copy-staged workspace siblings) cannot produce a package the
/// audit accepts but this query skips. It remains the same non-goal the
/// audit-eligibility predicate's own docs describe (see
/// `zfb_types::audit_eligibility`'s module docs: a predicate must never grow
/// into a second resolver). esbuild stays the only resolver; this only
/// reclassifies what esbuild already recorded.
///
/// # The bounded-set guarantee — never "every claimed workspace member"
///
/// The result can only ever name a package esbuild ACTUALLY resolved an
/// input from in THIS metafile. A package `pnpm-workspace.yaml` claims but
/// that nothing in this bundle session imports never appears here, no matter
/// how many workspace members are claimed in total — see
/// [`zfb_types::first_party::claimed_workspace_member_names`] for the
/// (deliberately separate) full claimed-member roster, and this module's own
/// `accepted_enrolment_set_excludes_unrelated_claimed_member` test for a
/// fixture proving the boundary directly. Wholesale-mirroring every claimed
/// member regardless of whether it is reached would be an uncontrolled
/// perf/staging-surface expansion — exactly what this contract exists to
/// prevent Sub 10b/10c from doing.
///
/// # What this does NOT decide
///
/// Whether, and how, to actually mirror-enrol, expand macros for, or
/// register watches on an accepted package is Sub 10b's (SSR coupling,
/// `crates/zfb-build/src/bundler.rs`) and Sub 10c's (islands/client coupling,
/// `crates/zfb/src/commands/build.rs`) job, not this function's. This only
/// answers "which packages, and where" — a query, never a wiring.
///
/// # Arguments
///
/// Identical shape to [`audit_metafile_stage_escape`]'s: `metafile_cwd` is
/// esbuild's own working directory the metafile's keys resolve against;
/// `stage_roots` is the full set of roots a legitimately staged input may
/// live under; `first_party_root` is the outer boundary of "this build's own
/// source" (the project root, or the widened workspace root for a
/// sibling-mirrored build). Callers that already invoke
/// `audit_metafile_stage_escape[_at_path]` for the same bundle session pass
/// it the exact same arguments.
pub fn accepted_enrolment_set(
    metafile_bytes: &[u8],
    metafile_cwd: &Path,
    stage_roots: &[&Path],
    first_party_root: &Path,
) -> Result<AcceptedEnrolmentSet> {
    let meta: Metafile = serde_json::from_slice(metafile_bytes).map_err(|e| {
        anyhow!("zfb bundler: enrolment-selection query failed to parse metafile: {e}")
    })?;

    let canonical_stage_roots: Vec<PathBuf> = stage_roots
        .iter()
        .map(|root| canonical_or_self(root).unwrap_or_else(|| root.to_path_buf()))
        .collect();
    let canonical_first_party_root =
        canonical_or_self(first_party_root).unwrap_or_else(|| first_party_root.to_path_buf());
    let lexical_stage_roots: Vec<PathBuf> = stage_roots
        .iter()
        .map(|root| normalize_path_lexical(root))
        .collect();

    let mut claimed_members = ClaimedMemberRoster::new(&canonical_first_party_root);
    let mut by_name: BTreeMap<String, AcceptedPackage> = BTreeMap::new();
    let mut inputs_by_name: BTreeMap<String, BTreeSet<PathBuf>> = BTreeMap::new();
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
            // Classified through the SAME gate the audit uses
            // ([`classify_package_shaped_input`], issue #2127) rather than a
            // second copy of it, so a package the audit accepts can never be
            // one this query silently skips. Case 3 (ordinary third-party
            // dep) and anything out of the four-case scope carry no package
            // identity this contract cares about; an offender (rejected by
            // the audit) is simply not enrolled — this contract has no
            // separate "rejected" channel to report to, that remains
            // `audit_metafile_stage_escape`'s job.
            if let PackageShapedInput::DeclaredFirstPartyEntry(pkg) = classify_package_shaped_input(
                record.key,
                &canonical,
                &canonical_first_party_root,
                InputLocality {
                    canonical_in_node_modules,
                    in_stage,
                    in_first_party,
                },
                &mut claimed_members,
            ) {
                inputs_by_name
                    .entry(pkg.name.clone())
                    .or_default()
                    .insert(canonical);
                by_name.entry(pkg.name.clone()).or_insert(pkg);
            }
            continue;
        }

        if in_stage {
            continue; // ordinary staged source — not a package boundary at all.
        }

        let logical_in_stage = lexical_stage_roots
            .iter()
            .any(|root| normalize_path_lexical(&record.logical_path).starts_with(root));
        if logical_in_stage
            || logical_path_names_staged_entry(&record.logical_path, &canonical_stage_roots)
        {
            continue; // case 1: ordinary staged source, through an aliased cwd spelling.
        }

        if !in_first_party {
            continue; // outside first-party territory entirely — out of scope.
        }
        if let Some(pkg) = declared_first_party_package_identity_from_canonical(
            &canonical,
            &canonical_first_party_root,
        ) {
            inputs_by_name
                .entry(pkg.name.clone())
                .or_default()
                .insert(canonical);
            by_name.entry(pkg.name.clone()).or_insert(pkg);
        }
    }

    Ok(AcceptedEnrolmentSet {
        by_name,
        inputs_by_name,
    })
}

/// Declared-data-only package identity for a first-party **source path** the
/// caller already holds, rather than for a metafile key (epic #2078 Sub 10c,
/// issue #2090).
///
/// [`accepted_enrolment_set`] answers "which declared consume-from-source
/// packages did THIS bundle session's metafile show were reached and
/// accepted". The islands/client pipeline's no-stage path
/// (`crates/zfb/src/commands/build.rs`) has no metafile at all — esbuild runs
/// there without `--metafile`, which is exactly why #2048's defect is silent
/// in that pipeline — so its sanctioned loud-failure fallback has to apply the
/// same declared-entry acceptance rule to a path it already holds. Delegating
/// to [`declared_first_party_package_identity_from_canonical`] is what keeps
/// that fallback from drifting away from what the audit itself accepts: the
/// same four conditions (nearest `package.json` inside `first_party_root`, a
/// declared `name`, a `pnpm-workspace.yaml`-claimed root, a subpath covered by
/// the package's own declared entries), the same fail-closed posture.
///
/// This is a QUERY over declared data (`package.json` + `pnpm-workspace.yaml`)
/// and never a resolution pass: it says nothing about whether esbuild did, or
/// would, resolve `source` — only whether the package that owns it declares
/// that location reachable. Callers bring their own evidence of reachability.
pub fn declared_first_party_package_for_source(
    source: &Path,
    first_party_root: &Path,
) -> Option<AcceptedPackage> {
    let canonical = canonical_or_self(source)?;
    let canonical_first_party_root = canonical_or_self(first_party_root)?;
    declared_first_party_package_identity_from_canonical(&canonical, &canonical_first_party_root)
}

/// Convenience wrapper over [`accepted_enrolment_set`]: read the metafile
/// from `metafile_path` first, mirroring
/// [`audit_metafile_stage_escape_at_path`]'s own read-then-classify shape.
pub fn accepted_enrolment_set_at_path(
    metafile_path: &Path,
    metafile_cwd: &Path,
    stage_roots: &[&Path],
    first_party_root: &Path,
) -> Result<AcceptedEnrolmentSet> {
    let bytes = std::fs::read(metafile_path).map_err(|e| {
        anyhow!(
            "zfb bundler: enrolment-selection query failed to read metafile {}: {e}",
            metafile_path.display()
        )
    })?;
    accepted_enrolment_set(&bytes, metafile_cwd, stage_roots, first_party_root)
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

    /// Build the #2040 topology: a pnpm workspace claiming `.` and
    /// `packages/*`, a first-party sibling package at `packages/<dir>` with
    /// the given `package.json`, and a stage whose `node_modules/<name>` is a
    /// directory symlink to that package (the workspace-hoisted install shape
    /// a bare package-name import resolves through). Returns
    /// `(stage, first_party_root)`.
    #[cfg(unix)]
    fn write_workspace_sibling_stage(
        base: &Path,
        package_dir: &str,
        package_json: &str,
        files: &[(&str, &str)],
    ) -> (PathBuf, PathBuf) {
        let first_party = base.join("workspace");
        write(
            &first_party,
            "pnpm-workspace.yaml",
            "packages:\n  - '.'\n  - 'packages/*'\n",
        );
        let package = first_party.join("packages").join(package_dir);
        write(&package, "package.json", package_json);
        for (rel, body) in files {
            write(&package, rel, body);
        }

        let name = serde_json::from_str::<serde_json::Value>(package_json)
            .ok()
            .and_then(|manifest| {
                manifest
                    .get("name")
                    .and_then(|n| n.as_str())
                    .map(str::to_string)
            })
            .expect("fixture package.json must declare a name");
        let stage = base.join("stage");
        let link = stage.join("node_modules").join(&name);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&package, &link).unwrap();
        (stage, first_party)
    }

    #[test]
    #[cfg(unix)]
    fn stage_escape_allows_consume_from_source_sibling_declared_by_wildcard_exports() {
        // Issue #2040 / the #1730 repro: `@acme/ui` is consumed FROM SOURCE —
        // its exports point straight at `./src/*`, no dist, no build step.
        // Both the declared entry itself and a file it pulls in transitively
        // sit under the declared `src/` entry root, so neither is a stage
        // escape even though both are case-2 shaped.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (stage, first_party) = write_workspace_sibling_stage(
            &base,
            "ui",
            r#"{ "name": "@acme/ui", "exports": { "./*": "./src/*" } }"#,
            &[
                ("src/cta-button.tsx", "import './theme';"),
                ("src/theme.ts", "theme"),
            ],
        );

        let metafile = br#"{"inputs": {
            "node_modules/@acme/ui/src/cta-button.tsx": {"imports": []},
            "node_modules/@acme/ui/src/theme.ts": {"imports": []}
        }}"#;

        let result = audit_metafile_stage_escape(metafile, &stage, &[&stage], &first_party);
        assert!(
            result.is_ok(),
            "a first-party workspace sibling consumed from source must not be a stage escape; got {result:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn stage_escape_flags_dist_shipping_sibling_reached_at_an_undeclared_source_path() {
        // The guard against #2040 becoming a blanket "workspace siblings are
        // always fine" exemption: `@acme/built` DOES ship a built dist and
        // declares only `./dist/index.js`. Its declared dist entry is
        // accepted; a deep import climbing past it into `src/internal.ts`
        // reaches a location the package never declared, escapes the stage,
        // and must stay rejected.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (stage, first_party) = write_workspace_sibling_stage(
            &base,
            "built",
            // Bare (non-`./`) `main`, the form the spec allows and packages
            // commonly use — its declared `dist/` root must still register.
            r#"{ "name": "@acme/built", "main": "dist/index.js" }"#,
            &[
                ("dist/index.js", "built"),
                ("src/internal.ts", "internal source"),
            ],
        );

        let metafile = br#"{"inputs": {
            "node_modules/@acme/built/dist/index.js": {"imports": []},
            "node_modules/@acme/built/src/internal.ts": {"imports": []}
        }}"#;

        let err = audit_metafile_stage_escape(metafile, &stage, &[&stage], &first_party)
            .expect_err("an undeclared deep import into a dist-shipping sibling must stay flagged");
        let msg = err.to_string();
        assert!(
            msg.contains("node_modules/@acme/built/src/internal.ts"),
            "the undeclared source path must be named as an offender; got {msg:?}"
        );
        assert!(
            !msg.contains("dist/index.js"),
            "the package's own declared dist entry must not be flagged; got {msg:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn stage_escape_allows_consume_from_source_sibling_via_canonicalized_key() {
        // Flipped by #2086 (Staging Correctness 2 epic #2078, Wave 3; #2082's
        // Part 2, epic Wave 1, was the RED author). The SAME #2040 topology
        // as `stage_escape_allows_consume_from_source_sibling_declared_by_wildcard_exports`
        // above, but esbuild recorded a CANONICALIZED (non-`node_modules`-
        // shaped) metafile key for the resolved import instead of a
        // `node_modules/...`-shaped one — exactly what happens under
        // copy_mode (`node_modules_dir` set + non-empty `tsconfig_paths`, no
        // `--preserve-symlinks`; see bundler.rs :10153/:2019 and this crate's
        // `tests/bundler_consume_from_source_esbuild_regression.rs` env-gate
        // sibling test).
        //
        // `package_shaped` (this file's `has_node_modules_segment` check on
        // the KEY STRING, not the canonical path) is `false` for this key, so
        // the case-2 declared-entry exemption
        // (`declared_first_party_package_identity_from_key`) never fires — the
        // input used to fall straight through to the case-1/case-4
        // stage-membership check, which rejected it as case 4 even though
        // `@acme/ui` is declared, claimed, and its entry covers the import.
        // `canonical_input_is_declared_first_party_entry` now resolves
        // package identity from the canonical path instead and accepts it.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (stage, first_party) = write_workspace_sibling_stage(
            &base,
            "ui",
            r#"{ "name": "@acme/ui", "exports": { "./*": "./src/*" } }"#,
            &[
                ("src/cta-button.tsx", "import './theme';"),
                ("src/theme.ts", "theme"),
            ],
        );

        // The canonicalized key climbs straight out of the stage to the real
        // workspace path — no `node_modules` segment anywhere in the string,
        // unlike the `node_modules/@acme/ui/...` spelling the sibling test
        // above uses for the identical topology.
        let metafile = br#"{"inputs": {
            "../workspace/packages/ui/src/cta-button.tsx": {"imports": []},
            "../workspace/packages/ui/src/theme.ts": {"imports": []}
        }}"#;

        let result = audit_metafile_stage_escape(metafile, &stage, &[&stage], &first_party);
        assert!(
            result.is_ok(),
            "a first-party workspace sibling consumed from source must not be a stage \
             escape regardless of whether esbuild recorded a node_modules-shaped or a \
             canonicalized metafile key for it; got {result:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn stage_escape_flags_dist_shipping_sibling_canonicalized_key_stays_rejected() {
        // Part 3 (#2082, mandatory fail-closed twin). Mirrors
        // `stage_escape_flags_dist_shipping_sibling_reached_at_an_undeclared_source_path`
        // above, but reached via a CANONICALIZED (non-`node_modules`-shaped)
        // key instead. An UNDECLARED deep import into `src/internal.ts` must
        // stay rejected TODAY (case-4 rejection, same as now) and — this is
        // the point of this test — must STILL be rejected once Sub #2086
        // lands the canonical-key exemption for declared entries. This is
        // the boundary that stops #2086 from becoming a blanket "any
        // canonical path under first_party_root is fine" exemption: only a
        // DECLARED entry may ever be accepted, node_modules-shaped key or
        // not.
        //
        // Deliberately a single-offender fixture (unlike the node_modules-
        // shaped negative above, which also asserts the declared dist entry
        // is spared): today, case 4 doesn't consult declaredness at all, so
        // a canonicalized dist entry alongside this one would ALSO be
        // flagged pre-fix — that is Part 1/Part 2's concern, not this
        // permanent guard's. This fixture only pins what must never flip:
        // an undeclared canonical-key climb stays an offender before AND
        // after #2086.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (stage, first_party) = write_workspace_sibling_stage(
            &base,
            "built",
            r#"{ "name": "@acme/built", "main": "dist/index.js" }"#,
            &[
                ("dist/index.js", "built"),
                ("src/internal.ts", "internal source"),
            ],
        );

        let metafile = br#"{"inputs": {
            "../workspace/packages/built/src/internal.ts": {"imports": []}
        }}"#;

        let err = audit_metafile_stage_escape(metafile, &stage, &[&stage], &first_party)
            .expect_err(
                "an undeclared deep import reached via a canonicalized key must stay \
                 flagged, both today and after the canonical-key exemption lands \
                 (#2047/#2086)",
            );
        let msg = err.to_string();
        assert!(
            msg.contains("../workspace/packages/built/src/internal.ts"),
            "the undeclared source path must be named as an offender; got {msg:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn stage_escape_flags_consume_from_source_target_not_claimed_by_the_workspace() {
        // Declared entry roots alone are not enough: the target must also be
        // a package the governing pnpm-workspace.yaml actually claims. Here
        // the workspace claims only `packages/*`, and the link points at an
        // unclaimed `vendored/ui` directory inside the workspace tree.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let first_party = base.join("workspace");
        write(
            &first_party,
            "pnpm-workspace.yaml",
            "packages:\n  - '.'\n  - 'packages/*'\n",
        );
        let package = first_party.join("vendored/ui");
        write(
            &package,
            "package.json",
            r#"{ "name": "@acme/ui", "exports": { "./*": "./src/*" } }"#,
        );
        write(&package, "src/cta-button.tsx", "cta");

        let stage = base.join("stage");
        let link = stage.join("node_modules/@acme/ui");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&package, &link).unwrap();

        let metafile =
            br#"{"inputs": {"node_modules/@acme/ui/src/cta-button.tsx": {"imports": []}}}"#;

        let err = audit_metafile_stage_escape(metafile, &stage, &[&stage], &first_party)
            .expect_err("an unclaimed directory is not a workspace package and must stay flagged");
        assert!(
            err.to_string()
                .contains("node_modules/@acme/ui/src/cta-button.tsx"),
            "the unclaimed target must be named as an offender; got {err}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn stage_escape_flags_package_whose_manifest_name_disagrees_with_the_link() {
        // The link and the package must agree on identity. A
        // `node_modules/@acme/ui` link whose target declares itself as
        // something else is not the package the specifier named, so its
        // declarations do not vouch for the input.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let first_party = base.join("workspace");
        write(
            &first_party,
            "pnpm-workspace.yaml",
            "packages:\n  - '.'\n  - 'packages/*'\n",
        );
        let package = first_party.join("packages/ui");
        write(
            &package,
            "package.json",
            r#"{ "name": "@acme/something-else", "exports": { "./*": "./src/*" } }"#,
        );
        write(&package, "src/cta-button.tsx", "cta");

        let stage = base.join("stage");
        let link = stage.join("node_modules/@acme/ui");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&package, &link).unwrap();

        let metafile =
            br#"{"inputs": {"node_modules/@acme/ui/src/cta-button.tsx": {"imports": []}}}"#;

        let err = audit_metafile_stage_escape(metafile, &stage, &[&stage], &first_party)
            .expect_err("a name mismatch between link and manifest must stay flagged");
        assert!(
            err.to_string()
                .contains("node_modules/@acme/ui/src/cta-button.tsx"),
            "the mismatched target must be named as an offender; got {err}"
        );
    }

    #[test]
    fn declared_entries_reads_only_declared_targets() {
        // Unit-level proof of the declaration reader: nested conditional
        // exports and arrays are just nesting around target strings; a
        // wildcard contributes its prefix directory; a root-level FILE
        // contributes only itself; `..`-climbing targets contribute nothing,
        // and a bare (non-`./`) string is a package name in `exports` but a
        // valid path in `main`/`module`.
        let manifest: serde_json::Value = serde_json::from_str(
            r#"{
                "main": "dist/index.js",
                "module": "./index.mjs",
                "exports": {
                    "./*": "./src/*",
                    ".": { "import": ["./lib/a.js", "not-relative"], "require": "../escape.js" }
                }
            }"#,
        )
        .unwrap();

        let mut entries = declared_entries(&manifest);
        entries.sort();
        assert_eq!(
            entries,
            vec![
                DeclaredEntry::Prefix("dist/".into()),
                DeclaredEntry::Prefix("lib/".into()),
                DeclaredEntry::Prefix("src/".into()),
                DeclaredEntry::ExactFile("index.mjs".into()),
            ]
        );

        // A root-level WILDCARD is the one spelling that declares the whole
        // package tree — it literally says "every subpath maps to itself".
        let root_wildcard: serde_json::Value =
            serde_json::from_str(r#"{ "exports": { "./*": "./*" } }"#).unwrap();
        assert_eq!(
            declared_entries(&root_wildcard),
            vec![DeclaredEntry::Prefix(String::new())]
        );

        // An absolute `main` names nothing inside the package.
        let absolute: serde_json::Value =
            serde_json::from_str(r#"{ "main": "/etc/passwd" }"#).unwrap();
        assert!(declared_entries(&absolute).is_empty());
    }

    #[test]
    fn root_level_main_authorises_only_itself_not_the_whole_package() {
        // A root-level `main` used to collapse to the empty prefix, which
        // accepted EVERY package subpath — so the extremely common
        // dist-shipping shape below silently granted blanket access to the
        // package's own `src/`, defeating condition 5 of the acceptance rule.
        let manifest: serde_json::Value =
            serde_json::from_str(r#"{ "main": "./index.js", "exports": { "./*": "./dist/*" } }"#)
                .unwrap();
        let entries = declared_entries(&manifest);

        assert!(
            declared_entries_cover(&entries, "index.js"),
            "the declared root entry itself must stay accepted: {entries:?}"
        );
        assert!(
            declared_entries_cover(&entries, "dist/thing.js"),
            "the declared `dist/` wildcard must stay accepted: {entries:?}"
        );
        assert!(
            !declared_entries_cover(&entries, "src/internal.ts"),
            "an undeclared deep subpath must NOT be authorised by a root-level \
             `main` when the package also declares a directory entry: {entries:?}"
        );
        // The bare-form spelling (`"main": "dist/index.js"`, no `./`) is
        // equally common and must be read the same way.
        let bare: serde_json::Value =
            serde_json::from_str(r#"{ "main": "index.js", "module": "dist/index.mjs" }"#).unwrap();
        let bare = declared_entries(&bare);
        assert!(declared_entries_cover(&bare, "index.js"));
        assert!(!declared_entries_cover(&bare, "src/internal.ts"));
    }

    #[test]
    fn root_only_source_package_still_authorises_its_sibling_sources() {
        // The other half of the rule above: when a package declares NOTHING
        // but root-level files it has no build-artifact directory to keep
        // separate, and its entry necessarily imports its siblings — esbuild
        // records `helper.ts` as an input right beside `index.ts`. Reading
        // the root entry as literally the only authorised file would reject
        // the ordinary consume-from-source shape.
        let manifest: serde_json::Value =
            serde_json::from_str(r#"{ "exports": { ".": "./index.ts" } }"#).unwrap();
        let entries = declared_entries(&manifest);

        assert!(declared_entries_cover(&entries, "index.ts"));
        assert!(
            declared_entries_cover(&entries, "helper.ts"),
            "a root-entry source package must authorise the siblings its entry \
             imports: {entries:?}"
        );
        assert!(
            declared_entries_cover(&entries, "internal/deep.ts"),
            "…including nested ones, since nothing declares a directory to keep \
             separate: {entries:?}"
        );

        // Adding a single directory entry flips the package into the
        // "has a build-artifact directory" shape, and the root file narrows
        // to itself again.
        let with_dist: serde_json::Value =
            serde_json::from_str(r#"{ "exports": { ".": "./index.ts", "./x": "./dist/x.js" } }"#)
                .unwrap();
        let with_dist = declared_entries(&with_dist);
        assert!(declared_entries_cover(&with_dist, "index.ts"));
        assert!(declared_entries_cover(&with_dist, "dist/x.js"));
        assert!(!declared_entries_cover(&with_dist, "helper.ts"));
    }

    #[test]
    fn split_package_name_and_subpath_keys_on_the_last_node_modules_segment() {
        assert_eq!(
            split_package_name_and_subpath("node_modules/@acme/ui/src/a.ts"),
            Some((
                "@acme/ui".to_string(),
                vec!["src".to_string(), "a.ts".to_string()]
            ))
        );
        assert_eq!(
            split_package_name_and_subpath("node_modules/.pnpm/x@1/node_modules/x/index.js"),
            Some(("x".to_string(), vec!["index.js".to_string()]))
        );
        // No package-relative subpath left, and no node_modules at all.
        assert_eq!(
            split_package_name_and_subpath("node_modules/@acme/ui"),
            None
        );
        assert_eq!(split_package_name_and_subpath("src/a.ts"), None);
    }

    /// Issue #2127's real-copy staging topology, the counterpart to
    /// [`write_workspace_sibling_stage`] above: the workspace member still
    /// lives at `packages/<package_dir>`, but `<stage>/node_modules/<name>`
    /// is a genuine COPY of it rather than a symlink to it. This is what an
    /// active `bundle.exclude` produces — once exclusions are in play the
    /// live `<shadow>/node_modules -> <live tree>` symlink is deliberately
    /// never created (it would let esbuild climb back to an excluded
    /// dependency), so every non-excluded dependency is materialised into the
    /// stage at its logical path instead; see `crates/zfb-build/src/bundler.rs`.
    ///
    /// Deliberately NOT `#[cfg(unix)]`-gated, unlike its symlink sibling:
    /// real-copy staging needs no symlink at all, and the classification
    /// under test is identical on every platform.
    ///
    /// Returns `(stage, first_party_root)`.
    fn write_workspace_sibling_real_copy_stage(
        base: &Path,
        package_dir: &str,
        package_json: &str,
        files: &[(&str, &str)],
    ) -> (PathBuf, PathBuf) {
        let first_party = base.join("workspace");
        write(
            &first_party,
            "pnpm-workspace.yaml",
            "packages:\n  - '.'\n  - 'packages/*'\n",
        );
        write(
            &first_party,
            "package.json",
            r#"{ "name": "@acme/host", "private": true }"#,
        );
        let package = first_party.join("packages").join(package_dir);
        write(&package, "package.json", package_json);
        for (rel, body) in files {
            write(&package, rel, body);
        }

        let name = serde_json::from_str::<serde_json::Value>(package_json)
            .ok()
            .and_then(|manifest| {
                manifest
                    .get("name")
                    .and_then(|n| n.as_str())
                    .map(str::to_string)
            })
            .expect("fixture package.json must declare a name");
        // The staged REAL COPY — same manifest, same files, physically
        // present inside the stage. No link anywhere.
        let staged = base.join("stage").join("node_modules").join(&name);
        write(&staged, "package.json", package_json);
        for (rel, body) in files {
            write(&staged, rel, body);
        }
        (base.join("stage"), first_party)
    }

    #[test]
    fn stage_escape_flags_undeclared_workspace_sibling_staged_as_a_real_copy() {
        // Issue #2127, the fix: the #2081/#2050 topology at unit level. The
        // workspace claims `@scope/child`, which declares NOTHING (no
        // `exports`, no `main`) — the case-2 offender shape. Staged as a real
        // copy its canonical path trivially keeps a `node_modules` segment
        // (there is no symlink to resolve away from), which used to land it
        // in case 3 "ordinary third-party dependency, allowed" before
        // declared identity was ever consulted, shipping the escape silently.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (stage, first_party) = write_workspace_sibling_real_copy_stage(
            &base,
            "child",
            r#"{ "name": "@scope/child", "private": true }"#,
            &[("index.ts", "export const childMarker = 'CHILD';")],
        );

        let metafile = br#"{"inputs": {"node_modules/@scope/child/index.ts": {"imports": []}}}"#;

        let err = audit_metafile_stage_escape(metafile, &stage, &[&stage], &first_party)
            .expect_err(
            "an undeclared workspace sibling staged as a REAL COPY must be flagged — real-copy \
             staging must not become a blanket case-3 exemption",
        );
        let msg = err.to_string();
        assert!(
            msg.contains("stage-escape audit"),
            "error must name the stage-escape audit; got {msg:?}"
        );
        assert!(
            msg.contains("node_modules/@scope/child/index.ts"),
            "error must name the offending metafile key; got {msg:?}"
        );
        assert!(
            msg.contains("@scope/child"),
            "error must name the workspace package the staged copy belongs to; got {msg:?}"
        );
    }

    #[test]
    fn stage_escape_allows_consume_from_source_sibling_staged_as_a_real_copy() {
        // Blast-radius control for #2127: the #2040 consume-from-source
        // carve-out must survive real-copy staging untouched. `@acme/ui`
        // declares `./*` -> `./src/*`, so every file under `src/` is a
        // declared entry — reaching it by package name is not an escape, and
        // the fact that it arrived as a staged copy rather than a symlink
        // changes nothing about that.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (stage, first_party) = write_workspace_sibling_real_copy_stage(
            &base,
            "ui",
            r#"{ "name": "@acme/ui", "exports": { "./*": "./src/*" } }"#,
            &[
                ("src/cta-button.tsx", "import './theme';"),
                ("src/theme.ts", "theme"),
            ],
        );

        let metafile = br#"{"inputs": {
            "node_modules/@acme/ui/src/cta-button.tsx": {"imports": []},
            "node_modules/@acme/ui/src/theme.ts": {"imports": []}
        }}"#;

        let result = audit_metafile_stage_escape(metafile, &stage, &[&stage], &first_party);
        assert!(
            result.is_ok(),
            "a declared consume-from-source sibling must stay accepted when it is staged as a \
             real copy instead of symlinked; got {result:?}"
        );
    }

    #[test]
    fn stage_escape_real_copy_flags_only_the_undeclared_deep_import_not_the_declared_entry() {
        // The other half of the blast-radius boundary: #2127's real-copy
        // discriminator must be as selective as the symlink shape's rule
        // already is. `@acme/built` ships a built dist and declares only
        // `./dist/index.js`; its declared entry is spared while a deep import
        // climbing past it into `src/internal.ts` stays an offender — the
        // real-copy mirror of
        // `stage_escape_flags_dist_shipping_sibling_reached_at_an_undeclared_source_path`.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (stage, first_party) = write_workspace_sibling_real_copy_stage(
            &base,
            "built",
            r#"{ "name": "@acme/built", "main": "dist/index.js" }"#,
            &[
                ("dist/index.js", "built"),
                ("src/internal.ts", "internal source"),
            ],
        );

        let metafile = br#"{"inputs": {
            "node_modules/@acme/built/dist/index.js": {"imports": []},
            "node_modules/@acme/built/src/internal.ts": {"imports": []}
        }}"#;

        let err = audit_metafile_stage_escape(metafile, &stage, &[&stage], &first_party)
            .expect_err(
            "an undeclared deep import into a real-copy-staged dist-shipping sibling must stay \
             flagged",
        );
        let msg = err.to_string();
        assert!(
            msg.contains("node_modules/@acme/built/src/internal.ts"),
            "the undeclared source path must be named as an offender; got {msg:?}"
        );
        assert!(
            !msg.contains("dist/index.js"),
            "the package's own declared dist entry must not be flagged; got {msg:?}"
        );
    }

    #[test]
    fn stage_escape_allows_third_party_real_copy_whose_name_no_workspace_claims() {
        // The blast-radius control that matters most (#2127): the
        // case-2/case-3 boundary governs ALL third-party dependency
        // classification, not just workspace siblings. Inside a GENUINE
        // workspace — same fixture as the sibling tests above, so the claimed
        // roster is non-empty and really is consulted — an ordinary registry
        // dependency staged as a real copy must stay case 3. Nothing but its
        // declared name separates it from the flagged sibling: `preact` is
        // claimed by no `packages:` glob, so it never reaches the
        // declared-entry rule at all.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (stage, first_party) = write_workspace_sibling_real_copy_stage(
            &base,
            "ui",
            r#"{ "name": "@acme/ui", "exports": { "./*": "./src/*" } }"#,
            &[("src/cta-button.tsx", "cta")],
        );
        // A real-copy-staged registry dep, declaring only a root `main` — the
        // exact declaration shape that would make a CLAIMED sibling's
        // `src/`-side deep import an offender.
        let third_party = stage.join("node_modules/preact");
        write(
            &third_party,
            "package.json",
            r#"{ "name": "preact", "main": "dist/preact.js" }"#,
        );
        write(&third_party, "src/index.js", "preact source");

        let metafile = br#"{"inputs": {
            "node_modules/@acme/ui/src/cta-button.tsx": {"imports": []},
            "node_modules/preact/src/index.js": {"imports": []}
        }}"#;

        let result = audit_metafile_stage_escape(metafile, &stage, &[&stage], &first_party);
        assert!(
            result.is_ok(),
            "an ordinary third-party dependency staged as a real copy must stay allowed (case 3) \
             — its declared name matches nothing the workspace claims; got {result:?}"
        );
    }

    #[test]
    fn stage_escape_allows_registry_dep_sharing_a_claimed_name_whose_chunk_no_entry_covers() {
        // THE regression this rule most has to avoid (found in review of
        // #2127): pnpm 10 defaults `link-workspace-packages` to FALSE, so a
        // dependency declared `"@acme/ui": "^1.0.0"` — no `workspace:`
        // protocol — installs the PUBLISHED registry copy even though
        // `pnpm-workspace.yaml` claims a member by that same name, and an
        // active `bundle.exclude` stages that registry copy into the shadow
        // like any other dependency. Name and locality therefore both match a
        // staged workspace copy.
        //
        // Judged by the case-2 declared-entry rule it would HARD-FAIL: a
        // dual-format publish declares the entry roots `dist/cjs/` and
        // `dist/esm/`, and the standard rollup/tsup layout also emits
        // `dist/shared/chunk.js`, which neither covers. Gate 3
        // (`staged_copy_is_a_copy_of_claimed_member`) is what keeps it in
        // case 3: the registry copy is at 1.0.0 while the workspace member is
        // at 2.0.0-dev, so it is provably NOT a copy of that member.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (stage, first_party) = write_workspace_sibling_real_copy_stage(
            &base,
            "ui",
            r#"{ "name": "@acme/ui", "version": "2.0.0-dev",
                 "main": "dist/cjs/index.js", "module": "dist/esm/index.js" }"#,
            &[("dist/cjs/index.js", "cjs"), ("dist/esm/index.js", "esm")],
        );
        // The registry copy, staged at its natural position exactly like any
        // other non-excluded dependency.
        let registry = stage.join("node_modules/@acme/ui");
        std::fs::remove_dir_all(&registry).unwrap();
        write(
            &registry,
            "package.json",
            r#"{ "name": "@acme/ui", "version": "1.0.0",
                 "main": "dist/cjs/index.js", "module": "dist/esm/index.js" }"#,
        );
        write(
            &registry,
            "dist/cjs/index.js",
            "import '../shared/chunk.js';",
        );
        write(&registry, "dist/shared/chunk.js", "shared chunk");

        let metafile = br#"{"inputs": {
            "node_modules/@acme/ui/dist/cjs/index.js": {"imports": []},
            "node_modules/@acme/ui/dist/shared/chunk.js": {"imports": []}
        }}"#;

        let result = audit_metafile_stage_escape(metafile, &stage, &[&stage], &first_party);
        assert!(
            result.is_ok(),
            "an ordinary registry dependency that merely SHARES a claimed member's name must \
             stay case-3 allowed, including the shared chunk no declared entry covers — \
             otherwise the #2127 rule breaks ordinary pnpm builds; got {result:?}"
        );

        let enrolled = accepted_enrolment_set(metafile, &stage, &[&stage], &first_party).unwrap();
        assert!(
            enrolled.is_empty(),
            "a registry dependency must never be enrolled as a first-party package; got {enrolled:?}"
        );
    }

    #[test]
    fn stage_escape_allows_node_modules_input_outside_every_stage_root() {
        // Gate 1 (locality), added in review of #2127: case 2 is defined as
        // being about STAGED / first-party locations. An input in a live,
        // vendored, or store `node_modules` outside every stage root is an
        // ordinary dependency by construction and must never reach the
        // declared-entry rule on a name collision alone — even when its name,
        // version and declared entries would all match the claimed member.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (stage, first_party) = write_workspace_sibling_real_copy_stage(
            &base,
            "ui",
            r#"{ "name": "@acme/ui", "version": "1.0.0", "main": "dist/index.js" }"#,
            &[("dist/index.js", "built")],
        );
        // A vendored copy that is neither inside the stage nor under
        // first_party_root, carrying an undeclared `src/` file.
        let vendored = base.join("vendor/node_modules/@acme/ui");
        write(
            &vendored,
            "package.json",
            r#"{ "name": "@acme/ui", "version": "1.0.0", "main": "dist/index.js" }"#,
        );
        write(&vendored, "src/internal.ts", "internal");

        let metafile = format!(
            r#"{{"inputs": {{"{}": {{"imports": []}}}}}}"#,
            vendored.join("src/internal.ts").display()
        );

        let result =
            audit_metafile_stage_escape(metafile.as_bytes(), &stage, &[&stage], &first_party);
        assert!(
            result.is_ok(),
            "a node_modules-nested input outside every stage root is out of case 2's scope and \
             must stay allowed; got {result:?}"
        );
    }

    #[test]
    fn stage_escape_allows_published_store_copy_sharing_a_claimed_member_name() {
        // The sharpest blast-radius control for #2127's declared-NAME
        // identity check: a name in the claimed roster is NOT by itself proof
        // that an input is workspace source. pnpm's store legitimately holds
        // a PUBLISHED copy of a package the workspace also builds, pulled in
        // transitively by some other dependency — and its declared entry
        // roots may differ from the live member's (here the workspace
        // consumes `@acme/ui` from source via `./src/*` while the published
        // 1.0.0 tarball ships `./dist/*`).
        //
        // This is why the declared-entry rule reads the manifest at the
        // input's OWN package root rather than the claimed member's: judged
        // against the workspace member's `./src/*` this perfectly ordinary
        // `dist/index.js` would be flagged, failing a valid build.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (stage, first_party) = write_workspace_sibling_real_copy_stage(
            &base,
            "ui",
            r#"{ "name": "@acme/ui", "exports": { "./*": "./src/*" } }"#,
            &[("src/cta-button.tsx", "cta")],
        );
        // The published tarball of the SAME name in pnpm's content-
        // addressable store, declaring a built entry the workspace member
        // does not declare at all.
        let published = stage.join("node_modules/.pnpm/@acme+ui@1.0.0/node_modules/@acme/ui");
        write(
            &published,
            "package.json",
            r#"{ "name": "@acme/ui", "exports": { "./*": "./dist/*" } }"#,
        );
        write(&published, "dist/index.js", "published build output");

        let metafile = br#"{"inputs": {
            "node_modules/.pnpm/@acme+ui@1.0.0/node_modules/@acme/ui/dist/index.js": {"imports": []}
        }}"#;

        let result = audit_metafile_stage_escape(metafile, &stage, &[&stage], &first_party);
        assert!(
            result.is_ok(),
            "a published store copy that merely SHARES a claimed workspace member's name must \
             stay allowed — it is judged against its own declared entries, not the live \
             member's; got {result:?}"
        );
    }

    #[test]
    fn stage_escape_flags_staged_copy_whose_manifest_name_disagrees_with_the_key() {
        // Condition 3 of the case-2 rule, carried into the real-copy shape
        // (#2127) unchanged: the key and the package must agree on identity.
        // A `node_modules/@scope/child` staged directory whose own manifest
        // declares something else is not the package the specifier named, so
        // its declarations cannot vouch for the input — and because the KEY's
        // name is one the workspace claims, failing closed here is the only
        // safe answer. The real-copy mirror of
        // `stage_escape_flags_package_whose_manifest_name_disagrees_with_the_link`.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (stage, first_party) = write_workspace_sibling_real_copy_stage(
            &base,
            "child",
            r#"{ "name": "@scope/child", "exports": { "./*": "./src/*" } }"#,
            &[("src/index.ts", "child")],
        );
        // Overwrite the staged copy's manifest so it no longer claims to be
        // the package the key reached it under.
        write(
            &stage.join("node_modules/@scope/child"),
            "package.json",
            r#"{ "name": "@scope/something-else", "exports": { "./*": "./src/*" } }"#,
        );

        let metafile =
            br#"{"inputs": {"node_modules/@scope/child/src/index.ts": {"imports": []}}}"#;

        let err = audit_metafile_stage_escape(metafile, &stage, &[&stage], &first_party)
            .expect_err("a staged copy whose manifest name disagrees with the key must be flagged");
        assert!(
            err.to_string()
                .contains("node_modules/@scope/child/src/index.ts"),
            "the mismatched staged copy must be named as an offender; got {err}"
        );
    }

    #[test]
    fn accepted_enrolment_set_tracks_real_copy_staged_sibling_in_lockstep_with_the_audit() {
        // Issue #2127's mandatory coupling: `accepted_enrolment_set` used to
        // carry its OWN copy of the case-2/case-3 gate, so widening what the
        // audit accepts here without moving both together would have produced
        // a package the audit accepts but the enrolment query skips —
        // accepted-but-not-enrolled, the #2048 defect class. Both now
        // classify through `classify_package_shaped_input`.
        //
        // One metafile, all three real-copy outcomes at once: a DECLARED
        // sibling (accepted -> enrolled), an UNDECLARED deep import into that
        // same package's undeclared `src/` (rejected -> never recorded as a
        // reached input), and an ordinary third-party dep (case 3 -> never
        // enrolled).
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (stage, first_party) = write_workspace_sibling_real_copy_stage(
            &base,
            "built",
            r#"{ "name": "@acme/built", "main": "dist/index.js" }"#,
            &[
                ("dist/index.js", "built"),
                ("src/internal.ts", "internal source"),
            ],
        );
        let third_party = stage.join("node_modules/preact");
        write(&third_party, "package.json", r#"{ "name": "preact" }"#);
        write(&third_party, "index.js", "preact");

        let metafile = br#"{"inputs": {
            "node_modules/@acme/built/dist/index.js": {"imports": []},
            "node_modules/@acme/built/src/internal.ts": {"imports": []},
            "node_modules/preact/index.js": {"imports": []}
        }}"#;

        let enrolled = accepted_enrolment_set(metafile, &stage, &[&stage], &first_party).unwrap();

        assert_eq!(
            enrolled.len(),
            1,
            "exactly the one declared, claimed package must be enrolled; got {enrolled:?}"
        );
        assert!(
            enrolled.contains("@acme/built"),
            "the real-copy-staged declared sibling must be enrolled, in lockstep with the audit \
             accepting it; got {enrolled:?}"
        );
        assert!(
            !enrolled.contains("preact"),
            "an ordinary third-party dependency must never be enrolled; got {enrolled:?}"
        );

        let package = enrolled.get("@acme/built").unwrap();
        assert_eq!(
            package.package_root,
            first_party.join("packages/built"),
            "the enrolled package root must be the LIVE claimed member directory, not the staged \
             copy — a consumer enrols from workspace source"
        );

        let reached: Vec<&Path> = enrolled.reached_inputs("@acme/built").collect();
        assert_eq!(
            reached,
            vec![stage
                .join("node_modules/@acme/built/dist/index.js")
                .as_path()],
            "only the ACCEPTED input may be recorded as reached — the rejected undeclared deep \
             import must not be, even though a sibling input made the package accepted"
        );
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

    // ── Enrolment-selection contract (epic #2078 Sub 10a) ──────────────────

    #[test]
    #[cfg(unix)]
    fn accepted_enrolment_set_includes_node_modules_keyed_declared_sibling() {
        // Mirrors `stage_escape_allows_consume_from_source_sibling_declared_by_wildcard_exports`
        // above: the SAME topology and metafile the audit accepts must
        // surface `@acme/ui` in the enrolment-selection contract's result,
        // with its own package root and declared `src/` entry.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (stage, first_party) = write_workspace_sibling_stage(
            &base,
            "ui",
            r#"{ "name": "@acme/ui", "exports": { "./*": "./src/*" } }"#,
            &[
                ("src/cta-button.tsx", "import './theme';"),
                ("src/theme.ts", "theme"),
            ],
        );

        let metafile = br#"{"inputs": {
            "node_modules/@acme/ui/src/cta-button.tsx": {"imports": []},
            "node_modules/@acme/ui/src/theme.ts": {"imports": []}
        }}"#;

        let set = accepted_enrolment_set(metafile, &stage, &[&stage], &first_party)
            .expect("a declared consume-from-source sibling must be a valid query, not an error");
        assert_eq!(
            set.len(),
            1,
            "exactly one distinct package must be enrolled: {set:?}"
        );
        let pkg = set
            .get("@acme/ui")
            .expect("@acme/ui must be present in the enrolment set");
        assert_eq!(pkg.package_root, first_party.join("packages/ui"));
        assert_eq!(pkg.declared_entry_roots, vec!["src/".to_string()]);
        assert!(set.contains("@acme/ui"));
        assert!(!set.is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn accepted_enrolment_set_includes_canonicalized_key_declared_sibling() {
        // Mirrors `stage_escape_allows_consume_from_source_sibling_via_canonicalized_key`
        // above (issues #2047/#2086's copy-mode canonicalized-key shape):
        // the enrolment contract must recognise this package too, not just
        // the node_modules-keyed spelling.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (stage, first_party) = write_workspace_sibling_stage(
            &base,
            "ui",
            r#"{ "name": "@acme/ui", "exports": { "./*": "./src/*" } }"#,
            &[
                ("src/cta-button.tsx", "import './theme';"),
                ("src/theme.ts", "theme"),
            ],
        );

        let metafile = br#"{"inputs": {
            "../workspace/packages/ui/src/cta-button.tsx": {"imports": []},
            "../workspace/packages/ui/src/theme.ts": {"imports": []}
        }}"#;

        let set = accepted_enrolment_set(metafile, &stage, &[&stage], &first_party)
            .expect("a canonicalized-key declared sibling must be a valid query, not an error");
        assert_eq!(set.len(), 1);
        assert!(set.contains("@acme/ui"));
    }

    #[test]
    #[cfg(unix)]
    fn accepted_enrolment_set_excludes_undeclared_offender_but_keeps_the_declared_entry() {
        // Mirrors `stage_escape_flags_dist_shipping_sibling_reached_at_an_undeclared_source_path`:
        // `@acme/built` ships a built dist and declares only `./dist/index.js`.
        // The package is still enrolled ONCE (it has at least one genuinely
        // declared, accepted input), but the undeclared deep import into
        // `src/internal.ts` is an offender, not itself a reason to enrol
        // anything, and the resulting `declared_entry_roots` names only what
        // the manifest itself declares (`dist/`), never the undeclared path.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (stage, first_party) = write_workspace_sibling_stage(
            &base,
            "built",
            r#"{ "name": "@acme/built", "main": "dist/index.js" }"#,
            &[
                ("dist/index.js", "built"),
                ("src/internal.ts", "internal source"),
            ],
        );

        let metafile = br#"{"inputs": {
            "node_modules/@acme/built/dist/index.js": {"imports": []},
            "node_modules/@acme/built/src/internal.ts": {"imports": []}
        }}"#;

        let set = accepted_enrolment_set(metafile, &stage, &[&stage], &first_party)
            .expect("offending inputs must not turn this query itself into an error");
        assert_eq!(
            set.len(),
            1,
            "the package enrols once, not per accepted input: {set:?}"
        );
        let pkg = set
            .get("@acme/built")
            .expect("@acme/built must be enrolled");
        assert_eq!(pkg.declared_entry_roots, vec!["dist/".to_string()]);
    }

    #[test]
    #[cfg(unix)]
    fn accepted_enrolment_set_reports_only_the_accepted_inputs_as_reached() {
        // The per-FILE bound epic #2078 Sub 10b (issue #2089) keys off. Same
        // `@acme/built` fixture as the test above: the package IS accepted (its
        // declared `dist/index.js` was reached), but the undeclared deep import
        // into `src/internal.ts` is an OFFENDER, so it must never appear as a
        // reached input — otherwise a consumer would attribute a rejected input
        // to an accepted package.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (stage, first_party) = write_workspace_sibling_stage(
            &base,
            "built",
            r#"{ "name": "@acme/built", "main": "dist/index.js" }"#,
            &[
                ("dist/index.js", "built"),
                ("src/internal.ts", "internal source"),
            ],
        );

        let metafile = br#"{"inputs": {
            "node_modules/@acme/built/dist/index.js": {"imports": []},
            "node_modules/@acme/built/src/internal.ts": {"imports": []}
        }}"#;

        let set = accepted_enrolment_set(metafile, &stage, &[&stage], &first_party).unwrap();
        let reached: Vec<PathBuf> = set
            .reached_inputs("@acme/built")
            .map(Path::to_path_buf)
            .collect();
        assert_eq!(
            reached,
            vec![first_party.join("packages/built/dist/index.js")],
            "only the declared, accepted input may be reported as reached"
        );
        assert_eq!(
            set.reached_inputs("@acme/never-accepted").count(),
            0,
            "a name that was never accepted has no reached inputs"
        );
    }

    #[test]
    #[cfg(unix)]
    fn accepted_enrolment_set_reports_every_accepted_input_of_one_package() {
        // A package reached through more than one input is still ONE entry in
        // the set (asserted by the sibling test above), but every one of its
        // accepted inputs must be individually visible — Sub 10b inspects each
        // bundled file, not just the first one that made the package accepted.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (stage, first_party) = write_workspace_sibling_stage(
            &base,
            "ui",
            r#"{ "name": "@acme/ui", "exports": { "./*": "./src/*" } }"#,
            &[
                ("src/cta-button.tsx", "import './theme';"),
                ("src/theme.ts", "theme"),
            ],
        );

        let metafile = br#"{"inputs": {
            "node_modules/@acme/ui/src/cta-button.tsx": {"imports": []},
            "node_modules/@acme/ui/src/theme.ts": {"imports": []}
        }}"#;

        let set = accepted_enrolment_set(metafile, &stage, &[&stage], &first_party).unwrap();
        assert_eq!(set.len(), 1);
        let reached: Vec<PathBuf> = set
            .reached_inputs("@acme/ui")
            .map(Path::to_path_buf)
            .collect();
        assert_eq!(
            reached,
            vec![
                first_party.join("packages/ui/src/cta-button.tsx"),
                first_party.join("packages/ui/src/theme.ts"),
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn accepted_enrolment_set_excludes_a_package_reached_only_at_an_undeclared_path() {
        // The sharper negative: when EVERY input reaching a package is
        // undeclared (no accepted input exists for it at all), the package
        // must not appear in the enrolment set — "reached" alone is not
        // "accepted".
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (stage, first_party) = write_workspace_sibling_stage(
            &base,
            "built",
            r#"{ "name": "@acme/built", "main": "dist/index.js" }"#,
            &[
                ("dist/index.js", "built"),
                ("src/internal.ts", "internal source"),
            ],
        );

        // Only the UNDECLARED path is in this metafile — the declared
        // `dist/index.js` entry was never actually imported this session.
        let metafile = br#"{"inputs": {
            "node_modules/@acme/built/src/internal.ts": {"imports": []}
        }}"#;

        let set = accepted_enrolment_set(metafile, &stage, &[&stage], &first_party).unwrap();
        assert!(
            set.is_empty(),
            "a package reached only through an undeclared path must not be enrolled: {set:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn accepted_enrolment_set_excludes_ordinary_third_party_dependency() {
        // Case 3 (an ordinary `.pnpm`-nested registry dependency) must never
        // surface as an "accepted package" — it is not first-party at all.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let work = root.join("work");
        let pnpm_pkg = work.join("node_modules/.pnpm/left-pad@1.0.0/node_modules/left-pad");
        std::fs::create_dir_all(&pnpm_pkg).unwrap();
        std::fs::write(pnpm_pkg.join("index.js"), "module.exports = 1;\n").unwrap();
        std::fs::create_dir_all(work.join("node_modules")).unwrap();
        std::os::unix::fs::symlink(&pnpm_pkg, work.join("node_modules/left-pad")).unwrap();

        let metafile = br#"{"inputs": {"node_modules/left-pad/index.js": {"imports": []}}}"#;

        let set = accepted_enrolment_set(metafile, &work, &[&work], &root).unwrap();
        assert!(
            set.is_empty(),
            "an ordinary third-party dep must never be enrolled: {set:?}"
        );
    }

    #[test]
    fn accepted_enrolment_set_fails_closed_on_malformed_metafile() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let err = accepted_enrolment_set(b"not json", &root, &[&root], &root)
            .expect_err("a malformed metafile must be a build error, not an empty result");
        assert!(
            err.to_string().contains("enrolment-selection query"),
            "fail-closed error must name the enrolment-selection query; got {err}"
        );
    }

    #[test]
    fn accepted_enrolment_set_at_path_fails_closed_on_missing_metafile_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let missing = root.join("does-not-exist/metafile.json");

        let err = accepted_enrolment_set_at_path(&missing, &root, &[&root], &root)
            .expect_err("an unreadable metafile path must be a build error, not an empty result");
        assert!(
            err.to_string().contains("enrolment-selection query"),
            "fail-closed error must name the enrolment-selection query; got {err}"
        );
    }

    /// THE central regression guard for epic #2078 Sub 10a's "bounded
    /// enrolment set" requirement: a fixture with TWO claimed workspace
    /// members, where only ONE is actually imported/reached by the project
    /// under bundling this session. The unreached member must NOT appear in
    /// [`accepted_enrolment_set`]'s result even though
    /// `pnpm-workspace.yaml` claims it exactly as much as the reached one —
    /// proving this contract is never "every claimed workspace member."
    #[test]
    #[cfg(unix)]
    fn accepted_enrolment_set_excludes_unrelated_claimed_member() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let first_party = base.join("workspace");
        write(
            &first_party,
            "pnpm-workspace.yaml",
            "packages:\n  - '.'\n  - 'packages/*'\n",
        );

        // Reached member: imported by the project this session.
        let ui = first_party.join("packages/ui");
        write(
            &ui,
            "package.json",
            r#"{ "name": "@acme/ui", "exports": { "./*": "./src/*" } }"#,
        );
        write(&ui, "src/cta-button.tsx", "export const x = 1;\n");

        // Unrelated member: claimed by the SAME `pnpm-workspace.yaml` globs,
        // but nothing in this bundle session imports it — it never appears
        // anywhere in the metafile below.
        let unrelated = first_party.join("packages/unrelated");
        write(
            &unrelated,
            "package.json",
            r#"{ "name": "@acme/unrelated", "exports": { "./*": "./src/*" } }"#,
        );
        write(&unrelated, "src/index.ts", "export const y = 2;\n");

        // Sanity: the workspace claims BOTH members — proves the exclusion
        // below is not an accident of the fixture's own claim globs.
        let claimed = zfb_types::claimed_workspace_member_names(&first_party);
        assert!(claimed.contains_key("@acme/ui"));
        assert!(claimed.contains_key("@acme/unrelated"));
        assert_eq!(claimed.len(), 2);

        let stage = base.join("stage");
        let link = stage.join("node_modules/@acme/ui");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&ui, &link).unwrap();
        // No node_modules entry for `@acme/unrelated` at all — it is
        // reachable in principle (the workspace claims it), but nothing
        // actually imports it, so no metafile input ever names it either.

        let metafile = br#"{"inputs": {
            "node_modules/@acme/ui/src/cta-button.tsx": {"imports": []}
        }}"#;

        let set = accepted_enrolment_set(metafile, &stage, &[&stage], &first_party)
            .expect("a valid, if partial, bundle session must not itself be an error");

        assert_eq!(
            set.len(),
            1,
            "only the reached member may be enrolled, never the whole claimed roster: {set:?}"
        );
        assert!(
            set.contains("@acme/ui"),
            "the reached member must still be enrolled"
        );
        assert!(
            !set.contains("@acme/unrelated"),
            "a claimed-but-unreached member must NOT be enrolled just because it is claimed: {set:?}"
        );
    }

    #[test]
    fn accepted_enrolment_set_default_and_accessors_on_an_empty_result() {
        // The contract's own accessors must behave sanely for the "nothing
        // to enrol" case (a build with no first-party escapes at all) —
        // usable standalone, independent of any Sub 10b/10c wiring.
        let set = AcceptedEnrolmentSet::default();
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
        assert!(!set.contains("anything"));
        assert!(set.get("anything").is_none());
        assert_eq!(set.iter().count(), 0);
        assert_eq!((&set).into_iter().count(), 0);
    }
}
