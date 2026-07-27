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
//! | # | stage widened (`first_party_root != normalize(project_root)`) | `first_party_root/pnpm-workspace.yaml` is a file | ≥1 first-party-reachable link under `node_modules_dir` | Result | Variant |
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
//! # What counts as a "first-party-reachable link"
//!
//! An entry directly under `node_modules_dir` — or one level below a
//! `@scope/` directory, the only nesting pnpm's public layout produces — that
//! satisfies **all three** of:
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
//! Failures to read the directory, to stat an entry, or to canonicalise a
//! target are treated as "no evidence" and skipped — mirroring the metafile
//! audit's own "skip, don't invent" posture. Entries whose name starts with
//! `.` (`.pnpm`, `.bin`, `.modules.yaml`) are skipped outright.
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
//! # Known limitation — real (non-symlink) staged copies disarm eligibility (issue #2050)
//!
//! **This is NOT closed as of this writing.** Row 3
//! ([`AuditEligibility::FirstPartyPackageReachable`]) requires a **symlink**
//! under `node_modules_dir` (see "What counts as a first-party-reachable
//! link" above) — it has no way to recognise a first-party package that was
//! staged as a **real, non-symlink copy** instead. A caller's
//! `node_modules_dir` holds real copies rather than symlinks whenever
//! `bundle.exclude` is active (the live `<node_modules_dir> -> <live tree>`
//! symlink is deliberately never created once exclusions are in play, so an
//! excluded dependency cannot be resurrected by climbing through it — see
//! `crates/zfb-build/src/bundler.rs`, confirmed by issue #2081's env-gated
//! regression test). In that configuration this predicate falls through to
//! [`AuditEligibility::NoReachableFirstPartyPackage`] even though a
//! first-party package genuinely is staged and reachable — **silently
//! disarming the caller's stage-escape audit** for that build. An undeclared
//! workspace sibling then ships into the bundle with no error at all, the
//! exact "completely silent" P1 shape issue #1730 was meant to close.
//!
//! Issue #2050 (superseded by epic #2078) tracks this gap; #2081 pins the
//! current disarmed behavior with a regression test, and #2087 is the sub
//! that closes it by recognising a staged first-party package via its
//! **declared identity** (`package.json` name, claimed by
//! `pnpm-workspace.yaml`) instead of requiring link-ness — the same
//! declared-data-only posture this predicate already uses for symlinks. Until
//! #2087 lands, treat row 3's symlink requirement as a **necessary but not
//! sufficient** signal: its absence does not prove no first-party package is
//! reachable, only that none was reachable *via a symlink*.

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
    /// entry is a symlink to a claimed workspace package inside
    /// `first_party_root`. This is the #1730 root-claimed-workspace case the
    /// old proxies read as "not a workspace".
    FirstPartyPackageReachable {
        /// The `node_modules` entry (the symlink itself, canonical parent).
        link: PathBuf,
        /// Its canonical target — claimed workspace source.
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

/// The lexicographically first `node_modules` entry that is a symlink to a
/// claimed workspace package inside `first_party_root`, as
/// `(link, canonical target)`.
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

    let mut found: Vec<(PathBuf, PathBuf)> = Vec::new();
    for candidate in package_dir_candidates(&node_modules) {
        let Ok(metadata) = std::fs::symlink_metadata(&candidate) else {
            continue;
        };
        if !metadata.file_type().is_symlink() {
            continue;
        }
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
