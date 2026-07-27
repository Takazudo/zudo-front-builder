//! Stage-escape audit **eligibility** predicate (issue #1986, epic #1982).
//!
//! # What this decides — and what it deliberately does not
//!
//! This module answers exactly one question: **is a first-party stage escape
//! structurally possible for this build?** — i.e. should the caller arm the
//! metafile stage-escape audit at all.
//!
//! It does **not**, and must never, decide whether an escape actually
//! occurred. That remains the sole job of esbuild's `--metafile` `inputs`
//! record, audited by `zfb_build::metafile_deps::audit_metafile_stage_escape`.
//! Keeping those two jobs apart is a hard rule of epic #1982: the #1497 loop
//! diverged precisely because a predicate grew into a resolution predictor
//! (see the `l-lessons-client-bundling` postmortem). Accordingly this module
//! reads only **authoritative** data — declared `pnpm-workspace.yaml`
//! membership and the actual symlink targets under a real `node_modules`
//! directory. It never probes extensions, walks `exports`/`main`, or
//! replicates resolution order in any form.
//!
//! # Why the old proxies were wrong (issue #1730)
//!
//! Two call sites stood in for "is this a workspace" with a proxy for the
//! staging root having *widened*:
//!
//! - islands/client: `first_party_root == normalize(project_root)` → skip;
//! - SSR: `workspace_rel.is_none()` → skip.
//!
//! Both are the same test spelled twice, and both read a workspace whose
//! `pnpm-workspace.yaml` claims its own root (`packages: ['.', 'packages/*']`)
//! as "not a workspace": building *from* the workspace root makes
//! `first_party_root_for` return `project_root` itself, so the stage never
//! widens — yet `node_modules/<pkg>` symlinks to live sibling source are
//! every bit as reachable as they are from a nested member. The audit was
//! disabled exactly where the repo owner's real-world repro escapes.
//!
//! # Inputs
//!
//! - `project_root` — the project being built (any spelling; normalised here).
//! - `first_party_root` — the result of [`crate::first_party_root_for`] for
//!   that project: the workspace root when the project is a claimed member,
//!   the normalised project root otherwise.
//! - `node_modules_dir` — the `node_modules` directory whose entries the
//!   build's bare-specifier imports actually resolve through. Callers pass
//!   `<shadow>/node_modules` (itself a symlink to the live tree); the live
//!   `<project_root>/node_modules` names the same tree and is equally valid.
//!   The directory is canonicalised before scanning, so either spelling
//!   yields the same answer.
//!
//! # Output
//!
//! [`AuditEligibility`], a four-variant enum whose [`AuditEligibility::is_eligible`]
//! is the caller-facing bit. The variants exist so a caller (and a test) can
//! see *why*, and so the two ineligible cases stay distinguishable — "not a
//! workspace at all" and "a workspace with nothing first-party linked" are
//! different facts.
//!
//! # Decision table
//!
//! Evaluated top to bottom; the first matching row wins.
//!
//! | # | stage widened (`first_party_root != normalize(project_root)`) | `first_party_root/pnpm-workspace.yaml` is a file | ≥1 first-party-reachable link or declared real copy under `node_modules_dir` | Result | Variant |
//! |---|---|---|---|---|---|
//! | 1 | yes | — (not read) | — (not scanned) | **eligible** | [`AuditEligibility::WidenedStage`] |
//! | 2 | no | no | — (not scanned) | not eligible | [`AuditEligibility::NoWorkspace`] |
//! | 3 | no | yes | yes | **eligible** | [`AuditEligibility::FirstPartyPackageReachable`] |
//! | 4 | no | yes | no | not eligible | [`AuditEligibility::NoReachableFirstPartyPackage`] |
//!
//! Row 1 is unconditional on purpose. A widened stage moves the stage
//! boundary itself, and escapes across it include forms that need no symlink
//! at all (the audit's "case 4": a `..`-climbing or absolute metafile key for
//! which no staged spelling was ever produced). Requiring a reachable link
//! there would *disarm* audits that fire today. Rows 2–4 are the new work:
//! they let a non-widened build arm the audit on evidence rather than on the
//! widening proxy. The predicate is therefore a strict **superset** of the
//! proxies it replaces — nothing eligible under the old rule becomes
//! ineligible under this one.
//!
//! # What counts as first-party-reachable evidence
//!
//! An entry directly under `node_modules_dir` — or one level below a
//! `@scope/` directory, the only nesting pnpm's public layout produces — can
//! establish row 3 in either of two ways: as a **symlink** (resolution-based)
//! or, since issue #2087, as a **declared real copy** (identity-based, no
//! resolution at all). Either way, the entry itself is "no evidence" and
//! skipped on any I/O failure — mirroring the metafile audit's own "skip,
//! don't invent" posture — and entries whose name starts with `.` (`.pnpm`,
//! `.bin`, `.modules.yaml`) are skipped outright.
//!
//! ## As a symlink
//!
//! Satisfies **all three** of:
//!
//! 1. it is a **symlink** (checked with `symlink_metadata`, not followed);
//! 2. its **canonical target** lies inside the canonical `first_party_root`
//!    and contains **no `node_modules` segment** — this is what separates a
//!    workspace link (`node_modules/@scope/ui -> ../packages/ui`) from an
//!    ordinary registry dep, whose pnpm target keeps a `node_modules` segment
//!    (`node_modules/.pnpm/react@19/node_modules/react`);
//! 3. that target is **claimed as a package** by the governing
//!    `pnpm-workspace.yaml`'s `packages:` globs
//!    ([`crate::first_party::workspace_root_claims_path`]).
//!
//! Condition 2 alone already excludes an external `npm link` / `file:` dep
//! whose target sits outside `first_party_root` (issue #1731's reported
//! false-positive topology) — this predicate cannot make that worse, because
//! such a link is never counted. Condition 3 is the declared-membership half
//! the epic requires: a link into some *unclaimed* directory inside the
//! workspace tree is not a workspace package and does not arm the audit.
//!
//! ## As a declared real copy (issue #2087)
//!
//! Satisfies **both** of:
//!
//! 1. it is a **real directory**, not a symlink (a `bundle.exclude`-active
//!    build stages every non-excluded dependency this way instead of leaving
//!    the wholesale `<node_modules_dir> -> <live tree>` symlink in place —
//!    see "Declared-identity recognition" below);
//! 2. its own `package.json` declares a `name` that appears in
//!    [`crate::first_party::claimed_workspace_member_names`]`(first_party_root)`
//!    — the roster of every package `pnpm-workspace.yaml`'s `packages:` globs
//!    claim, read from each claimed member's *own* manifest.
//!
//! No path resolution happens here at all: a real copy's own physical
//! location under `node_modules_dir` is irrelevant, and is never compared
//! against any live source path. Only the declared name is consulted, and
//! only against a roster built the same declared-data-only way this
//! predicate already reads `pnpm-workspace.yaml` for the symlink case. A
//! `package.json` that is missing, unreadable, or carries no string `name`
//! yields no evidence, same as a symlink that fails to canonicalise. An
//! ordinary external registry dependency staged as a real copy (e.g. `react`)
//! is unaffected: its declared name simply matches nothing in the claimed
//! roster, so it is never counted (see fixture 8 below, the negative
//! control).
//!
//! # Consume-from-source (issue #1730's second comment)
//!
//! A workspace sibling whose `package.json` `exports` points straight at
//! `./src/*` with no `dist/` is classified **first-party-reachable** here,
//! exactly like any other workspace link: its symlink target is claimed
//! workspace source with no `node_modules` segment. Nothing about the absence
//! of a build step is visible to — or consulted by — this predicate. Whether
//! that shape should be *accepted* by the audit (today it lands in the
//! audit's "case 2: OFFENDER") is a separate policy question owned by #2040.
//! This predicate's job is only to make sure #2040 has the case in scope.
//!
//! # Declared-identity recognition of real (non-symlink) copies (issue #2087, closed)
//!
//! Historical context: through issue #2081, row 3
//! ([`AuditEligibility::FirstPartyPackageReachable`]) required a **symlink**
//! under `node_modules_dir` — it had no way to recognise a first-party
//! package that was staged as a **real, non-symlink copy** instead. A
//! caller's `node_modules_dir` holds real copies rather than symlinks
//! whenever `bundle.exclude` is active (the live `<node_modules_dir> -> <live
//! tree>` symlink is deliberately never created once exclusions are in play,
//! so an excluded dependency cannot be resurrected by climbing through it —
//! see `crates/zfb-build/src/bundler.rs`). In that configuration the
//! predicate used to fall through to
//! [`AuditEligibility::NoReachableFirstPartyPackage`] even though a
//! first-party package was genuinely staged and reachable — **silently
//! disarming the caller's stage-escape audit** for that build, shipping an
//! undeclared workspace sibling with no error at all (issue #2081's
//! `crates/zfb-build/tests/bundler_root_workspace_stage_escape_audit_disarm_pin.rs`
//! pinned this exact gap with a regression test — its positive pin has since
//! been updated in place to document the fix below plus a residual gap it
//! surfaced, issue #2127).
//!
//! Issue #2087 closed the gap by extending row 3's evidence: a real directory
//! under `node_modules_dir` is now ALSO first-party-reachable when its own
//! `package.json` declares a `name` claimed by `pnpm-workspace.yaml` (see
//! "As a declared real copy" above and
//! [`crate::first_party::claimed_workspace_member_names`]) — the same
//! declared-data-only posture this predicate already used for symlinks,
//! extended to a case where there is no link to resolve at all. Row 3's
//! symlink requirement is no longer the sole path to eligibility; its absence
//! now only means "not reachable *via a symlink*", not "not reachable at
//! all". #1731's external `npm link` / `file:` false-positive topology stays
//! correctly out of scope: an external dependency's declared name simply
//! does not appear in the claimed-member roster, so real-copy staging of one
//! is not itself evidence (see fixture 8, the negative control below).

use std::path::{Path, PathBuf};

use crate::first_party::workspace_root_claims_path;
use crate::{has_node_modules_segment, normalize_path_lexical};

/// Why the stage-escape audit is (or is not) eligible to run.
///
/// See the [module docs](self) for the full decision table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditEligibility {
    /// Row 1 — the staging root widened past `project_root` (a claimed,
    /// non-root pnpm-workspace member). Eligible unconditionally: the stage
    /// boundary moved, and escapes across it include non-symlink forms.
    WidenedStage,
    /// Row 3 — the stage did not widen, but at least one `node_modules`
    /// entry is first-party-reachable: either a symlink to a claimed
    /// workspace package inside `first_party_root`, or (issue #2087) a real,
    /// non-symlink directory whose own `package.json` declares a `name`
    /// claimed by `pnpm-workspace.yaml`. This is the #1730
    /// root-claimed-workspace case the old proxies read as "not a
    /// workspace".
    FirstPartyPackageReachable {
        /// The `node_modules` entry itself: the symlink's own path in the
        /// symlink case, or the real staged copy's own path in the
        /// declared-identity case.
        link: PathBuf,
        /// Claimed workspace source for this package: the symlink's
        /// canonical target in the symlink case, or the claimed member
        /// directory that declares the matching name in the declared-identity
        /// case (issue #2087) — the latter is looked up by name, never by
        /// resolving `link` itself.
        target: PathBuf,
    },
    /// Row 2 — no `pnpm-workspace.yaml` governs this build, so there is no
    /// first-party sibling to escape to.
    NoWorkspace,
    /// Row 4 — a workspace governs this build, but nothing under
    /// `node_modules_dir` links to a claimed workspace package. An external
    /// `npm link` / `file:` dep pointing outside `first_party_root` lands
    /// here, not in [`Self::FirstPartyPackageReachable`].
    NoReachableFirstPartyPackage,
}

impl AuditEligibility {
    /// The caller-facing bit: arm the metafile stage-escape audit?
    pub fn is_eligible(&self) -> bool {
        matches!(
            self,
            Self::WidenedStage | Self::FirstPartyPackageReachable { .. }
        )
    }
}

/// Decide whether the metafile stage-escape audit should be armed for this
/// build. See the [module docs](self) for inputs, output and the full
/// decision table.
///
/// This decides *whether to audit*, and nothing else. Whether an escape
/// occurred is decided solely by esbuild's metafile inputs.
pub fn stage_escape_audit_eligibility(
    project_root: &Path,
    first_party_root: &Path,
    node_modules_dir: &Path,
) -> AuditEligibility {
    if normalize_path_lexical(first_party_root) != normalize_path_lexical(project_root) {
        return AuditEligibility::WidenedStage;
    }
    if !first_party_root.join("pnpm-workspace.yaml").is_file() {
        return AuditEligibility::NoWorkspace;
    }
    match first_party_reachable_package(first_party_root, node_modules_dir) {
        Some((link, target)) => AuditEligibility::FirstPartyPackageReachable { link, target },
        None => AuditEligibility::NoReachableFirstPartyPackage,
    }
}

/// The lexicographically first `node_modules` entry that is
/// first-party-reachable — either a symlink to a claimed workspace package
/// inside `first_party_root`, or (issue #2087) a real, non-symlink directory
/// whose own `package.json` declares a `name` claimed by
/// `pnpm-workspace.yaml` — as `(link, target)`.
///
/// Deterministic by sorting: `read_dir` order is filesystem-defined, and the
/// returned pair is surfaced in [`AuditEligibility::FirstPartyPackageReachable`].
fn first_party_reachable_package(
    first_party_root: &Path,
    node_modules_dir: &Path,
) -> Option<(PathBuf, PathBuf)> {
    let workspace_root = std::fs::canonicalize(first_party_root)
        .unwrap_or_else(|_| normalize_path_lexical(first_party_root));
    // Canonicalised first so a `<shadow>/node_modules` symlink and the live
    // `<project_root>/node_modules` it points at scan identically.
    let node_modules = std::fs::canonicalize(node_modules_dir).ok()?;

    // Lazily computed: the declared-member roster (issue #2087) needs a full
    // walk of `workspace_root`, which is unnecessary overhead for the common
    // case (every `node_modules` entry is a symlink, the roster is never
    // consulted). Computed at most once per call, the first time a
    // non-symlink directory candidate is actually encountered.
    let mut claimed_members: Option<std::collections::BTreeMap<String, PathBuf>> = None;

    let mut found: Vec<(PathBuf, PathBuf)> = Vec::new();
    for candidate in package_dir_candidates(&node_modules) {
        let Ok(metadata) = std::fs::symlink_metadata(&candidate) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            let Ok(target) = std::fs::canonicalize(&candidate) else {
                continue;
            };
            if !target.starts_with(&workspace_root) || has_node_modules_segment(&target) {
                continue;
            }
            if !workspace_root_claims_path(&workspace_root, &target) {
                continue;
            }
            found.push((candidate, target));
            continue;
        }
        if !metadata.file_type().is_dir() {
            continue;
        }
        // Declared-identity evidence (issue #2087): no symlink, no
        // resolution — only the staged copy's OWN declared `package.json`
        // `name`, checked against the claimed-member roster.
        let Ok(manifest) = std::fs::read_to_string(candidate.join("package.json")) else {
            continue;
        };
        let Some(name) = crate::first_party::package_json_name(&manifest) else {
            continue;
        };
        let claimed = claimed_members.get_or_insert_with(|| {
            crate::first_party::claimed_workspace_member_names(&workspace_root)
        });
        let Some(target) = claimed.get(&name) else {
            continue;
        };
        found.push((candidate, target.clone()));
    }
    found.sort();
    found.into_iter().next()
}

/// Every path under `node_modules` that may name a package directory: its
/// direct entries, plus one level below each `@scope/` directory. Dot-entries
/// (`.pnpm`, `.bin`) are skipped; no deeper nesting is walked, because pnpm's
/// public layout produces none.
fn package_dir_candidates(node_modules: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let Ok(entries) = std::fs::read_dir(node_modules) else {
        return candidates;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if !name.starts_with('@') {
            candidates.push(path);
            continue;
        }
        // A scope directory is a plain directory holding the real entries.
        let Ok(scoped) = std::fs::read_dir(&path) else {
            continue;
        };
        for scoped_entry in scoped.flatten() {
            if scoped_entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            candidates.push(scoped_entry.path());
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workspace fixture: writes `pnpm-workspace.yaml` with `globs` and
    /// creates an empty `node_modules` under `project`. Returns
    /// `(workspace_root, node_modules_dir)`.
    fn workspace(root: &Path, globs: &str) -> (PathBuf, PathBuf) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join("pnpm-workspace.yaml"), globs).unwrap();
        let node_modules = root.join("node_modules");
        std::fs::create_dir_all(&node_modules).unwrap();
        (root.to_path_buf(), node_modules)
    }

    #[cfg(unix)]
    fn link(from: &Path, to: &Path) {
        if let Some(parent) = from.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::os::unix::fs::symlink(to, from).unwrap();
    }

    /// Fixture 1 — the #1730 topology: `packages: ['.', 'packages/*']` built
    /// FROM the workspace root, so the stage never widens. The old proxies
    /// read this as "not a workspace" and disabled the audit; the reachable
    /// `node_modules/@scope/ui -> packages/ui` link proves otherwise.
    #[cfg(unix)]
    #[test]
    fn fixture_1_root_claimed_workspace_is_eligible() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        let (_, node_modules) = workspace(&root, "packages: ['.', 'packages/*']\n");
        let ui = root.join("packages/ui/src");
        std::fs::create_dir_all(&ui).unwrap();
        link(&node_modules.join("@scope/ui"), &root.join("packages/ui"));

        let eligibility = stage_escape_audit_eligibility(&root, &root, &node_modules);
        assert!(eligibility.is_eligible(), "{eligibility:?}");
        let AuditEligibility::FirstPartyPackageReachable { target, .. } = eligibility else {
            panic!("expected a reachable first-party package");
        };
        assert_eq!(
            target,
            std::fs::canonicalize(root.join("packages/ui")).unwrap()
        );
    }

    /// Fixture 2 — an ordinary nested workspace member. Row 1 of the table:
    /// eligible via the widened stage, WITHOUT scanning `node_modules` at
    /// all (the directory here does not even exist). This is the "must keep
    /// working as today" case for every currently-armed workspace build.
    #[test]
    fn fixture_2_ordinary_nested_member_is_eligible_via_widened_stage() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        workspace(&root, "packages: ['packages/*']\n");
        let project = root.join("packages/app");
        std::fs::create_dir_all(&project).unwrap();

        assert_eq!(
            stage_escape_audit_eligibility(&project, &root, &project.join("node_modules")),
            AuditEligibility::WidenedStage
        );
    }

    /// Fixture 3 — a plain non-workspace project. No governing
    /// `pnpm-workspace.yaml`, so no first-party sibling exists to escape to
    /// and the audit stays off, exactly as today.
    #[test]
    fn fixture_3_non_workspace_project_is_not_eligible() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("site");
        let node_modules = project.join("node_modules");
        std::fs::create_dir_all(&node_modules).unwrap();

        let eligibility = stage_escape_audit_eligibility(&project, &project, &node_modules);
        assert_eq!(eligibility, AuditEligibility::NoWorkspace);
        assert!(!eligibility.is_eligible());
    }

    /// Fixture 4 — issue #1731's reported false-positive topology: an
    /// external `npm link` / `file:` dep whose target lies OUTSIDE
    /// `first_party_root`. It is not a first-party link, so it never arms
    /// the audit. This predicate must not make #1731 worse.
    #[cfg(unix)]
    #[test]
    fn fixture_4_external_link_outside_first_party_root_is_not_eligible() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        let (_, node_modules) = workspace(&root, "packages: ['.', 'packages/*']\n");
        let external = dir.path().join("elsewhere/vendor-widget");
        std::fs::create_dir_all(&external).unwrap();
        link(&node_modules.join("vendor-widget"), &external);

        let eligibility = stage_escape_audit_eligibility(&root, &root, &node_modules);
        assert_eq!(eligibility, AuditEligibility::NoReachableFirstPartyPackage);
        assert!(!eligibility.is_eligible());
    }

    /// Fixture 5 — the real-world shape from #1730's second comment: a
    /// first-party workspace sibling CONSUMED FROM SOURCE (`exports` ->
    /// `./src/*`, no `dist/`), reached by bare package name through the
    /// workspace symlink. It must classify as first-party-reachable so
    /// #2040 can decide the acceptance policy for it. The absence of a build
    /// step is invisible to this predicate by design.
    #[cfg(unix)]
    #[test]
    fn fixture_5_sibling_consumed_from_source_is_first_party_reachable() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        let (_, node_modules) = workspace(&root, "packages: ['.', 'packages/*', 'apps/*']\n");
        let ui = root.join("packages/ui");
        std::fs::create_dir_all(ui.join("src/shared/cta-button")).unwrap();
        std::fs::write(
            ui.join("package.json"),
            r#"{"name":"@zudo-sg/ui","exports":{"./*":"./src/*"}}"#,
        )
        .unwrap();
        std::fs::write(
            ui.join("src/shared/cta-button/cta-button.tsx"),
            "export const CtaButton = () => null;\n",
        )
        .unwrap();
        // Deliberately no `dist/` anywhere in the fixture.
        assert!(!ui.join("dist").exists());
        link(&node_modules.join("@zudo-sg/ui"), &ui);

        let eligibility = stage_escape_audit_eligibility(&root, &root, &node_modules);
        assert!(eligibility.is_eligible(), "{eligibility:?}");
        let AuditEligibility::FirstPartyPackageReachable { link, target } = eligibility else {
            panic!("consume-from-source must be first-party-reachable");
        };
        assert_eq!(link.file_name().unwrap(), "ui");
        assert_eq!(target, std::fs::canonicalize(&ui).unwrap());
    }

    /// Fixture 6 (issue #2087) — the declared-identity counterpart to
    /// fixture 1: the SAME undeclared-vs-claimed topology, but the
    /// `node_modules` entry is a REAL (non-symlink) DIRECTORY copy of the
    /// claimed package instead of a symlink — the exact shape a
    /// `bundle.exclude`-active build stages (issue #2081's regression pin).
    /// No symlink anywhere in this fixture. Recognised purely by its own
    /// `package.json` `name` matching a claimed workspace member — no path
    /// resolution at all.
    #[test]
    fn fixture_6_real_copy_staged_package_is_first_party_reachable_via_declared_identity() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        let (_, node_modules) = workspace(&root, "packages: ['.', 'packages/*']\n");
        let ui = root.join("packages/ui");
        std::fs::create_dir_all(&ui).unwrap();
        std::fs::write(ui.join("package.json"), r#"{"name":"@scope/ui"}"#).unwrap();

        // A REAL COPY, not a symlink.
        let staged = node_modules.join("@scope/ui");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("package.json"), r#"{"name":"@scope/ui"}"#).unwrap();
        std::fs::write(staged.join("index.js"), "export default 1;\n").unwrap();
        assert!(
            !std::fs::symlink_metadata(&staged)
                .unwrap()
                .file_type()
                .is_symlink(),
            "fixture must stage a real directory, not a symlink"
        );

        let eligibility = stage_escape_audit_eligibility(&root, &root, &node_modules);
        assert!(eligibility.is_eligible(), "{eligibility:?}");
        let AuditEligibility::FirstPartyPackageReachable { link, target } = eligibility else {
            panic!("a declared-identity-matched real copy must be first-party-reachable");
        };
        // Compare against the canonicalized path, matching every other
        // fixture in this suite — macOS resolves the tempdir's `/var` prefix
        // to `/private/var` via canonicalize, which `node_modules_dir`
        // scanning already goes through internally.
        assert_eq!(link, std::fs::canonicalize(&staged).unwrap());
        assert_eq!(target, std::fs::canonicalize(&ui).unwrap());
    }

    /// Fixture 7 (issue #2087, negative control) — a real-copy `node_modules`
    /// entry whose `package.json` is missing or malformed must not arm the
    /// audit, and must not panic; it is "no evidence", exactly like a
    /// symlink that fails to canonicalise.
    #[test]
    fn fixture_7_real_copy_with_malformed_or_missing_manifest_is_not_eligible() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        let (_, node_modules) = workspace(&root, "packages: ['.', 'packages/*']\n");

        // Malformed JSON.
        let broken = node_modules.join("broken-pkg");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join("package.json"), "{ not json").unwrap();

        // No package.json at all.
        let bare = node_modules.join("bare-pkg");
        std::fs::create_dir_all(&bare).unwrap();

        let eligibility = stage_escape_audit_eligibility(&root, &root, &node_modules);
        assert_eq!(eligibility, AuditEligibility::NoReachableFirstPartyPackage);
        assert!(!eligibility.is_eligible());
    }

    /// Fixture 8 (issue #2087, negative control) — an ordinary EXTERNAL
    /// dependency staged as a real copy (the common `bundle.exclude` shape
    /// for every non-excluded, non-workspace dependency) must NOT arm the
    /// audit just because it happens to be a real directory: its declared
    /// name matches no claimed workspace member. Widening the predicate to
    /// real copies must never widen it to "any real copy at all".
    #[test]
    fn fixture_8_real_copy_of_unclaimed_external_dependency_is_not_eligible() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        let (_, node_modules) = workspace(&root, "packages: ['.', 'packages/*']\n");
        // Claimed workspace member exists, but is irrelevant here — no entry
        // under `node_modules` declares its name.
        std::fs::create_dir_all(root.join("packages/ui")).unwrap();
        std::fs::write(
            root.join("packages/ui/package.json"),
            r#"{"name":"@scope/ui"}"#,
        )
        .unwrap();

        // A real copy of an ORDINARY external dependency, unclaimed by the
        // workspace.
        let react = node_modules.join("react");
        std::fs::create_dir_all(&react).unwrap();
        std::fs::write(react.join("package.json"), r#"{"name":"react"}"#).unwrap();

        let eligibility = stage_escape_audit_eligibility(&root, &root, &node_modules);
        assert_eq!(eligibility, AuditEligibility::NoReachableFirstPartyPackage);
        assert!(!eligibility.is_eligible());
    }

    /// An ordinary registry dependency in pnpm's real layout keeps a
    /// `node_modules` segment in its canonical target, so it is never
    /// mistaken for a workspace link — the audit's "case 3" shape.
    #[cfg(unix)]
    #[test]
    fn registry_dependency_through_dot_pnpm_is_not_first_party() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        let (_, node_modules) = workspace(&root, "packages: ['.', 'packages/*']\n");
        let store = node_modules.join(".pnpm/react@19.0.0/node_modules/react");
        std::fs::create_dir_all(&store).unwrap();
        link(&node_modules.join("react"), &store);

        assert_eq!(
            stage_escape_audit_eligibility(&root, &root, &node_modules),
            AuditEligibility::NoReachableFirstPartyPackage
        );
    }

    /// A link into a directory that IS inside the workspace tree but is not
    /// claimed by any `packages:` glob is not a workspace package. This is
    /// the declared-membership half of the predicate doing work that the
    /// boundary check alone cannot.
    #[cfg(unix)]
    #[test]
    fn unclaimed_directory_inside_the_workspace_is_not_first_party() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        let (_, node_modules) = workspace(&root, "packages: ['.', 'packages/*']\n");
        let scratch = root.join("scratch/tool");
        std::fs::create_dir_all(&scratch).unwrap();
        link(&node_modules.join("tool"), &scratch);

        assert_eq!(
            stage_escape_audit_eligibility(&root, &root, &node_modules),
            AuditEligibility::NoReachableFirstPartyPackage
        );
    }

    /// Callers pass `<shadow>/node_modules`, which is itself a symlink to the
    /// live tree. Canonicalising the directory before scanning makes both
    /// spellings yield the same answer.
    #[cfg(unix)]
    #[test]
    fn a_shadow_node_modules_symlink_scans_the_live_tree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        let (_, node_modules) = workspace(&root, "packages: ['.', 'packages/*']\n");
        std::fs::create_dir_all(root.join("packages/ui")).unwrap();
        link(&node_modules.join("ui"), &root.join("packages/ui"));

        let shadow = dir.path().join("stage/ws");
        std::fs::create_dir_all(&shadow).unwrap();
        link(&shadow.join("node_modules"), &node_modules);

        let via_shadow = stage_escape_audit_eligibility(&root, &root, &shadow.join("node_modules"));
        let via_live = stage_escape_audit_eligibility(&root, &root, &node_modules);
        assert!(via_shadow.is_eligible(), "{via_shadow:?}");
        assert_eq!(via_shadow, via_live);
    }

    /// A missing / unreadable `node_modules` is "no evidence", not an error:
    /// the workspace row falls through to not-eligible rather than panicking
    /// or arming on a guess.
    #[test]
    fn a_missing_node_modules_dir_is_not_eligible() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("pnpm-workspace.yaml"), "packages: ['.']\n").unwrap();

        assert_eq!(
            stage_escape_audit_eligibility(&root, &root, &root.join("node_modules")),
            AuditEligibility::NoReachableFirstPartyPackage
        );
    }

    /// An unnormalised `project_root` spelling must not read as widened —
    /// the same trap `stage_escape_audit_policy`'s own unit test guards at
    /// the islands call site.
    #[test]
    fn an_unnormalised_project_root_does_not_read_as_widened() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("site");
        std::fs::create_dir_all(&project).unwrap();
        let unnormalised = project.join("./sub/..");

        assert_eq!(
            stage_escape_audit_eligibility(&unnormalised, &project, &project.join("node_modules")),
            AuditEligibility::NoWorkspace
        );
    }
}
