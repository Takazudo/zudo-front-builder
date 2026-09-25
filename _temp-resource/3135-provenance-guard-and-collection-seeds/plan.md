# Big Plan: Provenance Guard and Collection Seeds

**Plan mode:** goal-clear (two bugfixes with unambiguous success criteria)
**Parent branch:** `main` (base-like)
**Base branch:** `base/provenance-guard-and-collection-seeds`
**Epic title:** `[Provenance Guard and Collection Seeds][Epic] Block silent provenance downgrades and bound out-of-root collection staging`

## Source

Recent 3 open issues:

- #3134 — 2.20.3: `@takazudo/zfb-darwin-x64` lost provenance → `ERR_PNPM_TRUST_DOWNGRADE` for `trust-policy=no-downgrade` consumers. (OWNER)
- #3133 — Out-of-root content collection seeds whole source dir → full node_modules staging walk (dev never ready, build OOM). (OWNER)
- #3127 — [plugin init] Capture next timeout trace and resolve demonstrated cause. (OWNER) — **evidence-gated tracker, NOT planned** (see below).

### #3127 triage: left open, untouched

#3127 is explicitly a recurrence-triggered tracker: "remains open until a recurrence supplies causal evidence". Its parent epic #3119 is closed; the diagnostics (`[zfb-plugin-init]` tracing, 7f1f4aa6) already shipped. There is no executable work until a failure trace exists — planning a speculative fix would violate its own rule 5 (deterministic failing-before regression for a *demonstrated* mechanism). Not superseded, not closed.

## Overview

Two independent bugfix tracks in one epic.

**Track A (#3134) — provenance downgrade guard.** v2.20.3 used `--fast-mac`, publishing `zfb-darwin-x64` without `--provenance` while every earlier version back to the #2627 restore (2.13.0) was attested. pnpm's `no-downgrade` compares by publish date, so any consumer (Linux included — pnpm resolves all optionalDependencies) fails. Today the only signals are a non-fatal `::warning` (`scripts/publish-npm-packages.sh:266-322`) and the weekly after-the-fact `drift-net` check. Fix: a pre-publish preflight that fails a release which would downgrade any package's trust, unless explicitly acknowledged.

**Immediate unblock (out-of-epic, release operation):** v2.20.4 is already bumped on `main` (73056a62) but untagged/unpublished. Run `/l-make-release` on the **default path (no `--fast-mac`)** so `release.yml`'s `macos-15-intel` leg builds and attests darwin-x64. Amend the zfb-lane `v2.20.4.mdx` with a Bug Fixes line "`@takazudo/zfb-darwin-x64` is attested again; 2.20.3 shipped it without provenance, which broke `trust-policy=no-downgrade` installs (#3134)". Verify with `node scripts/check-provenance-drift.mjs` (all 10 `ok`). Recommended follow-ups (outward actions, user decides): `npm deprecate @takazudo/zfb@2.20.3` (+ `zfb-darwin-x64@2.20.3`) pointing to 2.20.4, and a comment on zudolab/zudo-doc#4395 once 2.20.4 ships. This is a release, not a code sub-task — worktree children cannot push/tag — so it lives in the epic body as a prerequisite checklist item for the user/manager, and should happen now rather than wait for the epic.

**Track B (#3133) — bounded collection staging.** Verified at HEAD 741c26db:
1. `bundler.rs:3083-3089` pushes every collection root into `source_graph_roots` regardless of include/exclude.
2. `collect_project_source_module_graph_seed_files` (`bundler.rs:10345`, walk 10361) walks the whole root; seeds only `raw_source_extension` (ts/tsx/js/jsx/mjs/cjs/mts/cts) + `.css` — **`.mdx` is never a seed**. So `include: ["**/*.mdx"]` seeds only the non-included siblings.
3. `extend_node_modules_dependency_staging` (`bundler.rs:10393`) flips `workspace_staging_active` (10769-10780) and drains deferred lists to fixpoint.
4. Per-package import scan (10622-10660) walks whole physical pnpm dirs with a full swc parse per file; `visited` (10459) is keyed on **logical** root, so a pnpm-private dep reachable along N paths is re-scanned N times. No per-physical-dir cache. (Correction vs issue: on unix with empty `bundle.exclude`, ordinary deps are symlinked not copied — cost is the scan, not the copy.)
5. `bundler_input.rs:352-362` feeds both dev and build.

`source_graph_roots` is consumed only by staging (3149); dev watcher / narrowing / HMR use config-driven paths (`dev.rs:202,238,1523,2174`), so narrowing seeds does not affect watching (confirmed against l-lessons-dev-watcher-narrowing).

Fix: (B1) memoize the import scan by canonical physical package dir — behavior-identical, removes the N× multiplier; (B2) scope collection seeding to include/exclude-matched entries and what they import, so non-included sibling components can't flip workspace staging; (B3) real-binary confirm.

## Wave order

- Wave 1 (parallel): A1 preflight script · A3 skill/docs · B0 repro fixture + baseline · B1 import-scan memoization
- Wave 2 (parallel): A2 release.yml enforcement (after A1) · D decision: choose collection-seed fix shape (after B0, B1)
- Wave 3: B2 collection-seed fix (shape locked by D; may be SKIPped by D)
- Wave 4 (confirm): B3 real-binary dev-ready + build confirmation

Rust resource note: at most 2 concurrent cargo children (B0, B1) in Wave 1; free disk 416 GB (each worktree target/ 6-30 GB) — fine.

## Sub-tasks

### A1 — Provenance preflight check (script + vitest)
- Wave 1 · Depends on: none · subagents · sonnet
- New `scripts/check-provenance-preflight.mjs`, reusing `hasAttestation`, prerelease handling and fetch-with-retry from `scripts/check-provenance-drift.mjs` and `PUBLISHED_PACKAGES` from `scripts/retire-next-dist-tag.mjs`.
- Pure `wouldDowngrade(packument, { version })`: true when any published version **other than the target** (prereleases ignored when target is stable — same as `classifyPackage` at check-provenance-drift.mjs:98-101) carries `dist.attestations`.
- Only packages that do **not yet have the target version** are checked (publish skips existing versions, release.yml:~1052 — retries/partial publishes must not false-fail).
- 404 packument (never published) → ok. 5xx / 429 / network after retries → exit 2 (fail closed).
- CLI: `node scripts/check-provenance-preflight.mjs --version <v> --mode mac-local|recovery-no-provenance` (mac-local → `@takazudo/zfb-darwin-x64`; recovery → all 10). Exit 1 with `DOWNGRADE <pkg>@<v> (earlier attested: <ver>)` lines.
- Tests `scripts/__tests__/check-provenance-preflight.test.mjs` with injected packuments: attested-earlier → downgrade; never attested → ok; stable target with only prerelease attested → ok; prerelease target counts prereleases; target already published → skipped; 404 → ok; fetch failure → exit 2; mode mapping.

### A3 — Skill, checklist and troubleshooting docs
- Wave 1 · Depends on: none (contract fixed below) · subagents · sonnet
- Fixed contract (shared with A1/A2): CLI `node scripts/check-provenance-preflight.mjs --version <v> --mode mac-local|recovery-no-provenance`; Release-body ack line `<!-- zfb-release: allow-provenance-downgrade -->`; dispatch input `allow_provenance_downgrade`; skill flag `--accept-provenance-downgrade`.
- `l-make-release/SKILL.md`: Step 1 `--fast-mac` precondition runs the preflight; abort unless `--accept-provenance-downgrade`, which writes the marker into the draft body. Reuse-draft path (~497-507): strip a stale marker when the flag is absent; add it when present. Replace "intended supervision" wording (~:4, :23, :545, :658) with "consumer-breaking for `trust-policy=no-downgrade`".
- `l-make-mac-release-binary/SKILL.md` (~:9, :130): the separate-Mac flow must add the marker via `gh release edit --notes` before publishing, only after an explicit decision.
- `RELEASE_DAY_CHECKLIST.md` (~119-157, 196-266): fast-Mac is consumer-breaking whenever an earlier version is attested (true since 2.13.0); recovery is now **always** consumer-breaking for all 10 packages; document marker/input.
- `docs/src/content/docs/guides/troubleshooting.mdx` (~51-80) + `docs-ja` twin: add the 2.20.3 darwin-x64 case; remedy = 2.20.4+ (or pin 2.20.2).
- No package changelog entry (repo-only tooling).

### A2 — Enforce preflight in release.yml + publish script
- Wave 2 · Depends on: A1 · subagents · sonnet
- **Fast-Mac:** in `detect-mac-local` after `mac_local_present=true`, run the preflight before any build leg. That job checks out `checkout_sha` (release.yml:253) — for recovery it's an old tree without the script, so run the script from a control checkout at `github.sha` (mirror the `.release-control` pattern at release.yml:832-837).
- **Recovery:** run the preflight in the `publish` job from `.release-control/scripts/` before the recovery publish (~1088-1093).
- Ack: `release: published` → read `github.event.release.body` via an `env:` var (never interpolate into `run:`), strip `\r`, `grep -Fxq '<!-- zfb-release: allow-provenance-downgrade -->'`. Payload snapshot, not `gh release view`. `workflow_dispatch` → boolean input `allow_provenance_downgrade` (default false). Acked runs keep the existing `::warning` + summary and say consumers with `no-downgrade` will break.
- `scripts/publish-npm-packages.sh` (`emit_trust_downgrade_advisory` ~266-322, `main` ~350): in `mac-local` / `recovery-no-provenance` modes require `ZFB_ALLOW_PROVENANCE_DOWNGRADE=1` (set only when acked), else exit non-zero before any publish. Extend `tests/unit/publish-npm-packages.sh`.
- AC: unit shell tests green; actionlint clean; default all-provenance path unchanged; fresh-context review of release.yml diff (gate-defining file).

### B0 — Repro fixture + baseline measurement for #3133
- Wave 1 · Depends on: none · subagents · sonnet
- Fixture (under `crates/zfb/tests/fixtures/`) mirroring #3133 realistically: pnpm workspace `apps/site` + `packages/ui/src/components/button/{button.tsx,button.mdx}` where `button.mdx` imports `./button.tsx` and `button.tsx` imports a workspace package + npm deps with a pnpm-private transitive dep reachable along ≥3 logical paths; plus one sibling `.tsx` no MDX imports; collection `include: ["**/*.mdx"]`, `allowOutsideRoot: true`. Plus a control variant without the collection.
- Add a test-visible staging stats hook (physical-scan count, logical visits, whether workspace staging flipped) — minimal, `#[cfg]`/debug-env gated — so B1/B2/B3 can assert on it. Coordinate: B1 may extend the same stats struct.
- Record baseline in the PR/issue comment: `ZFB_DEV_TIMING=1` dev ready time with vs without collection, `zfb build` time/peak RSS, scan counts. Test that runs it is `#[ignore = "heavy: ..."]`-classified per crates/CLAUDE.md if heavy, else a staging-level test asserting current counts.

### B1 — Memoize node_modules import scan per canonical package dir
- Wave 1 · Depends on: none · subagents · opus
- In `extend_node_modules_dependency_staging` (bundler.rs:~10393), cache the **physical scan result only** (relative files, specifiers, `package_external_import_names`) keyed by canonical `physical_root`, across passes and logical aliases. `package_was_symlinked` depends on the logical root (~10618-10619) and must stay per visit. Do not collapse logical aliases (#1646).
- Staging output byte-identical; regression fixture with one dep via ≥3 logical paths asserts exactly 1 physical scan (revert-and-fail proven).
- AC: scoped `cargo test -p zfb-build` (exact-match resolution suite + bundler unit tests + new test); clippy clean.

### D — Decision: choose the collection-seed fix shape
- Wave 2 · Depends on: B0, B1 · subagents · fable (branching choice; the downstream spec depends on it)
- Run B0's fixture on the merged base (with B1). Compare against baseline and control. Decide:
  (a) include-scoped seeding with MDX-import following (spec below), (b) that plus a contingency (e.g. skipping non-runtime subtrees such as `*.map`/tests/docs in package scans, or a persistent physical-scan cache keyed by dir+mtime), or (c) B1 alone suffices → mark B2 `**SKIP:** <reason>`.
- Edit B2's body via `gh issue edit` with the concrete file:symbol spec and target numbers. No production code.

### B2 — Collection-seed fix (shape locked by D)
- Wave 3 · Depends on: D · subagents · opus
- Default spec (D may refine): replace the whole-root push at bundler.rs:3083-3089 for collections. Seeds = files matched by the collection's `CollectionFilter` (zfb-content/src/collection.rs:182/203/260; `include` may match `.tsx` entries — seed them directly) plus imports of included `.md`/`.mdx` entries, obtained by parsing `compile_mdx_to_jsx_module_cached` output (zfb-content/src/mdx_jsx_emit.rs:~3772) with the existing swc `collect_import_specifiers` (module_worker.rs:~2112) — catches remark-plugin and mdx-components imports. Follow relative chains via `discover_module_preprocessing_with_context` (bundler.rs:~2950) after verifying it accepts `allowOutsideRoot` files.
- Gate first: confirm how an MDX entry's `./X.tsx` resolves when the shadow filter (bundler.rs:~8925-8944) drops non-included `.tsx`; keep it working.
- New behaviour to disclose in PR: MDX bare imports now seed (can stage more under non-empty `bundle.exclude`) — test it.
- Keep stage-escape audit tests green (l-lessons-client-bundling). Regressions: non-included sibling `.tsx` importing a workspace package does not flip staging; MDX→tsx→bare dep under `bundle.exclude` still staged. Revert-and-fail both.

### B3 — Confirm: real-binary dev-ready + build on the #3133 fixture
- Wave 4 · Depends on: B2 · subagents · sonnet
- Real-binary test on B0's fixture: `zfb build` succeeds; `zfb dev` becomes ready within a deadline derived from B0's measured control baseline (CLAUDE.md rule 8); scan counts match D's target. Skip when esbuild slot absent. Classify in crates/CLAUDE.md manifest/nextest group if heavy. Post before/after numbers. If B2 was SKIPped, depends on D satisfied → still runs.

## Delegated decisions (-po)

- **Ack channel for fast-Mac/recovery:** Release-body marker + dispatch input (rejected: repo variable — sticky, easy to leave on; sentinel asset — invisible). Review target: A2 PR shows the marker is exact-match and default-deny.
- **Fail closed on registry errors** in preflight (rejected: warn) — a release can be retried; a shipped downgrade cannot be undone. Review target: exit 2 path.
- **B fix shape:** memoization (behavior-identical) + include-scoped seeding with MDX-import following (rejected: walking only package `exports`-reachable files — reimplements esbuild's resolver, violates client-bundling lesson; rejected: collapsing logical aliases — #1646 risk). Review target: B2 does not drop bare deps reached via MDX→component chains.
- **Release unblock stays out of the epic** — release requires pushing to main/tagging, which worktree children cannot do, and consumers should not wait for Track B.

## Original requirements checklist

- [#3134] Consumers with `no-downgrade` can install the next release → 2.20.4 via default CI path (prerequisite, out-of-epic) + changelog note.
- [#3134] 2.20.3 cannot be fixed retroactively → acknowledged; troubleshooting doc says upgrade.
- [#3134] Prevent repeat: fast-Mac fails or requires explicit ack when an earlier version is attested → A1 + A2 + A3.
- [#3134] Or document in RELEASE_DAY_CHECKLIST.md as consumer-breaking → A3.
- [#3134] Per-package attestation guard (#2625) → A1/A2 cover all 10 packages per mode.
- [#3133] Apply include/exclude in seed walk / seed only from what included MDX imports → B2.
- [#3133] Code files beside MDX should not start workspace staging → B2 regression (a).
- [#3133] Bound or cache per-package import walks → B1.
- [#3133] Realistic repro (MDX→tsx→workspace pkg) measured before fixing → B0; fix shape chosen from data → D.
- [#3133] Dev ready time ≈ without the collection; build no OOM → B3.
- [#3133] Build shares the path (bundler_input.rs:352) → covered by fixing the shared bundler; B3 checks both.
- [#3127] Deliberately not planned — evidence-gated tracker stays open.

## Review Notes

Codex was rate-limited; the second opinion came from an Opus reviewer. Applied:
1. B2 alone may not fix the realistic chain (button.mdx imports button.tsx) → added B0 (measure first) and D (fable decision, may SKIP/extend B2). Applied.
2. Recovery runs check out an old tree → preflight runs from a `.release-control` checkout / in publish job. Applied (A2).
3. Ack from `github.event.release.body` via env, strip `\r`, no live `gh release view`. Applied (A2).
4. Preflight: exclude target version, skip already-published packages, 404 = ok, recovery always needs ack. Applied (A1, A3).
5. Reused drafts / separate-Mac flow must manage the marker. Applied (A3).
6. MDX imports via compiled output + existing swc collector; reuse `discover_module_preprocessing_with_context`; disclose new MDX-bare-import seeding. Applied (B2).
7. Cache only the physical scan; `package_was_symlinked` stays per visit. Applied (B1).
8. A3 into Wave 1. Applied. A2 kept after A1 (gate-defining file).
10. `npm deprecate` + consumer comment → added as recommended out-of-epic follow-ups (outward actions left to the user).
11. #3127 unplanned — confirmed.

## Verification Report

Sonnet verifier: all clear. No missing/misinterpreted requirements; scheduling metadata valid on all 8 subs; graph acyclic, greedy steps = [#3136 #3137 #3138 #3139] → [#3140 #3141] → [#3142] → [#3143], max 2 concurrent Rust children; portability clean. One nit fixed: epic used codename "B2" → replaced with #3142.

## Created

Epic #3135; subs #3136 (A1), #3137 (A3), #3138 (B0), #3139 (B1), #3140 (A2), #3141 (D), #3142 (B2), #3143 (B3).
