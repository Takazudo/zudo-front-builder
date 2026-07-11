# Salvaged reference material for epic #1554 (source issue #1527)

Provenance: the interrupted fix attempt for the `bundle.exclude` parity escapes —
worktree `worktrees/final-cross-feature-gaps`, branch `fix/final-cross-feature-gaps`,
**uncommitted** diff on top of commit `c350dc4550ed943bfd57fe60380cc36e8a27e2d3`
(machine-local; baked here on 2026-07-12 so implementation sessions can read it).

## Contents

- `bundler_exact_match_resolution.worktree.rs` — full copy of the worktree's modified
  `crates/zfb-build/tests/bundler_exact_match_resolution.rs` (68 tests vs main's 43).
- `tests-only.patch` — `git diff` of that one file against c350dc45.

## ⚠️ How to use this — scenario reference ONLY

The FIX code that accompanied these tests in the worktree is the **non-converging
resolver-parity approach** (60/63 passing, abandoned — see
`.claude/skills/l-lessons-client-bundling/SKILL.md`). It is deliberately NOT baked here.
Do not port its implementation ideas. Write **fresh implementations** per the decided
architecture in epic #1554; extract only the *test scenarios* below.

## Test-name map

In scope for epic #1554:

- **Finding 1** (blocked bare-shaped alias keys → live bare-package fallback):
  - `excluded_exact_bare_alias_cannot_fall_back_to_same_named_package` (~line 411)
  - `overlapping_wildcard_cannot_fall_back_to_same_named_package_subpath` (~452)
- **Finding 2** (staged first-party package climbs to live node_modules):
  - `first_party_root_exact_target_cannot_climb_to_excluded_bare_dependency` (~1373)
  - `first_party_package_imports_external_cannot_climb_to_excluded_dependency` (~1568)
- **Allowed-closure positives** (guard against over-suppression):
  - `exact_target_stages_dependency_from_configured_external_node_modules` (~1135)
  - `node_modules_symlinked_package_stages_non_hoisted_canonical_dependency_only_without_preserve` (~1720, modified vs main)
  - `pnpm_canonical_dependency_exclusion_blocks_synthetic_logical_alias_copy` (~1808)

The other ~19 new tests in the worktree file are round-8 parity churn — **out of scope**
for epic #1554. One of them (`exact_isolation_blocks_project_live_ancestor_when_tmpdir_is_project_local`,
the project-local `$TMPDIR` ancestor-walk escape) is explicitly filed-not-fixed per the
epic's out-of-scope contract.

## Cleanup

The confirm sub of epic #1554 deletes this directory from the base branch before the
root PR merges (see `_temp-resource/README.md` lifecycle).
