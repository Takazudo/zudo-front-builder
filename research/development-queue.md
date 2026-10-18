# Development checkpoint and qualification queue

This ledger applies the [canonical policy](../CLAUDE.md#moving-development-and-frozen-qualification).
It is not release approval, a scheduler, or a replacement for source-bound receipts.
Snapshot: 2026-10-18; integration/CI triage owner: the development-checkpoint manager.

## Development checkpoint

- Branch: `base/4294-development-checkpoint`; parent: `21d6807e442bbed61e578c165a91d64c4a7dc141`.
- Scope: reviewed graph-path fix from PR [4308](https://github.com/Takazudo/zudo-front-builder/pull/4308),
  bounded watcher/dev tests from PR [4309](https://github.com/Takazudo/zudo-front-builder/pull/4309),
  and this workflow policy. Source PRs remain open; native acceptance is not claimed.
- Development head: resolve `git rev-parse origin/base/4294-development-checkpoint` after fetching.
  The aggregation PR records each immutable checkpoint SHA and exact-head CI receipt. A later
  tip does not inherit the earlier receipt. This document cannot contain its own commit hash.
- At intake all remote branch heads and all seven open PRs were inspected. Existing `base/**`
  heads concern 4187, 4199, design-system playground, and post-4.5.0 maintenance; none is adopted
  as an implicit parent. No active source branch is rewritten or closed.

| Source | Selected immutable head | Evidence at that head | Development decision |
| --- | --- | --- | --- |
| PR 4308 | `92e25ad4da392b0e566320f5e477e62bf134adc7` | [Linux health 39143914080](https://github.com/Takazudo/zudo-front-builder/actions/runs/39143914080) and other automatic workflows succeeded; preview upload skipped for draft | Integrate exact head; exact-source native follow-up passed (owner receipt below) |
| PR 4309 | `a76e4775fd6ccf6f1d929f8ce5bd8d25a2298b3a` | [Linux health 39146914767](https://github.com/Takazudo/zudo-front-builder/actions/runs/39146914767), 6,868 workspace tests passed; 27 checks succeeded, 6 conditional skips | Integrate exact head; native Mac follow-up pending |

The source changes are disjoint except `docs/public/wind-examples/manifest.json`.
Each source PR generated its own compiler digest. The combined checkpoint must regenerate
with the official `docs/scripts/wind-preview-assets.mjs` and the matching built workspace
compiler, then run `--check`; neither parent's digest proves the combined sources.

## Local checkpoint evidence

The combined source tree was checked on Linux x86_64 with Rust/Cargo 1.99.1, Node 24.19.0,
repository-pinned pnpm 12.8.2, and SHA-verified esbuild 0.25.12. The policy Markdown and
`cargo fmt --all --check` passed. The 19 preview-generator contract tests passed. Focused Rust checks passed: 13 graph unit
plus 7 graph integration, 65 helper unit plus 11 helper integration, 23 recursive-directory
watcher and 3 server publication tests. One existing helper doc-test is explicitly ignored;
none of these executed tests failed or retried.
The official generator rebuilt all 173 previews with the combined-source CLI and `--check`
confirmed freshness; only the compiler digest changed, no sample record or generated CSS.
Compiler source digest: `dbdcdb240a9c3f22e8d3bc5062d515343259104f9d6c1829d8f4e8158738bf4b`.

The scoped `cargo build --locked -p zfb --no-default-features --bin zfb` passed under the
machine-wide heavy guard. It is a generation tool, not default-V8 acceptance. First attempts
remain distinct: an inaccessible guard directory caused fail-open behavior, so that process
was stopped during dependency setup (exit 143); the guarded build then failed on missing
locked `hono`, and after dependency installation on an esbuild downloader send error.
Installing frozen workspace dependencies and staging the exact hash-verified esbuild binary
resolved those prerequisites. No assertion failure was retried. The first commit-hook attempt
also rejected the host's pnpm 11.19.0; a command-scoped shim to the pinned Corepack pnpm fixed
it, with both hooks then passing. No global toolchain setting or hook bypass was used.

The automatic integrated Linux batch and its immutable head/PR merge SHA are recorded in the
aggregation PR. No local full-workspace build/test duplicates that batch. Default-V8 CLI
regressions and the shared-helper consumer closure remain owed to that integrated CI result;
source-PR results above do not substitute for it.

## Trigger audit and external effects

Inspected workflow bodies at parent `21d6807e`; selected topic heads change no workflow.
Recheck this table at each checkpoint that changes workflows. Keep the aggregation PR **draft**.

| Route | Actual trigger/condition | Result for this checkpoint |
| --- | --- | --- |
| `node-free-smoke.yml` production showcase | Push to `main`; production job also requires `github.ref == 'refs/heads/main'` | No production job from the chosen branch/PR. Comments mentioning base pushes are stale; actual push list contains only main |
| `docs-deploy.yml` | Main push touching `docs/**` or its workflow, or explicit manual dispatch | No production dispatch; never push/merge this checkpoint to main |
| `docs-pr-preview.yml` | PR to main/base, docs-path filter; upload job requires non-draft | Docs verification/artifacts may run; preview upload skipped by existing draft guard |
| `node-free-smoke.yml` showcase preview | Same-repository, non-Dependabot, non-draft PR | Draft suppresses preview publication; builds/smokes/artifacts still run |
| `health.yml`, `docs-checks.yml`, `pr-checks.yml` | PR checks, including base targets where declared | One automatic Linux checkpoint batch; monitor exact head and PR merge SHA |
| `actionlint.yml`, `wind-computed-style.yml` | Path-filtered pushes/PRs including base branches | Existing validation may run; no deployment authority |
| `release.yml` and release-following smoke | Explicit release/manual event; `dry_run` defaults false; release smoke follows Release workflow | Not invoked. No release/tag/contract application in this lane |
| `rolldown-plugin-publication.yml` | Push to `prototype/318-rolldown-native` | Not the chosen branch; not invoked |

“Non-publishing” does not mean no external effects: GitHub CI consumes compute and uploads
logs/artifacts, and some checks may report issues. Converting this PR to ready can enable
preview publication on subsequent eligible events. No credentials, deployment conditions,
branch protection, skip markers or runner budgets are changed to suppress checks.

## Native qualification stays on selected source

Native verification is separately owned by the existing Mac qualification session under
[4294](https://github.com/Takazudo/zudo-front-builder/issues/4294). This manager does not operate
that machine, start a new full Mac run, or dispatch Mac Actions. The source-specific follow-ups
remain bound to the PR 4308/4309 heads above, unless that owner explicitly records a replacement.
The owner reports native PR 4308 verification complete at `92e25ad4da392b0e566320f5e477e62bf134adc7`:
13 graph unit tests, 7 integration tests, and both targeted CLI regressions passed; the corrected
virtual-module command actually executed one test (4.70s), without a skip. Independent review
found no material issues. The earlier compile timeout, host restart interruption and invalid-PATH
skip remain separate receipts, not clean passes. The [published receipt](https://github.com/Takazudo/zudo-front-builder/pull/4308#issuecomment-6254981484)
records Apple A18 Pro / ARM64, 8 GiB, macOS 27.0.1, Rust/Cargo 1.99.1, Node 24.14.0 and
hash-verified Darwin ARM64 esbuild 0.25.12. It retains full commands, binary hashes and log
checksums in the owner's local evidence archive; this lane has read the published receipt, not
those archive bytes. PR 4309 remains separate. None of this qualifies the combined development SHA.
Do not silently retarget native work to this development branch.

| Qualification record | State and next action |
| --- | --- |
| Historical native first run | [Original report](https://github.com/Takazudo/zudo-front-builder/issues/4294#issuecomment-6251614904): 6,838 passed, 7 failed, 9 timed out, 86 excluded. Preserve raw receipt and first failures; not current-head acceptance |
| Graph follow-up | Owner reports PASS at frozen `92e25ad4…`: 13 unit + 7 integration + both targeted CLI cases. [Published receipt](https://github.com/Takazudo/zudo-front-builder/pull/4308#issuecomment-6254981484), with toolchain and local archive provenance; no integrated-SHA acceptance |
| Watcher/dev follow-up | [timeout-test-design.md](../crates/zfb-test-utils/timeout-test-design.md): recursive_dirs, watch_add_confirm and two named dev cases, serially, with first phase/cleanup receipts |
| Mac build controls | Command-scoped `CARGO_BUILD_JOBS=1`, existing heavy guard, reusable caches; no global setting change or cargo clean. Separate compilation logs from test logs |
| Required receipt | Immutable SHA/tree; OS/CPU; exact rustc/cargo/Node/pnpm/esbuild/engine/ICU; target/profile/features/flags; command and cache identity; lock/input/binary hashes; artifact paths/digests; first exit/result and retries |
| Classification | Build timeout: incomplete build, tests not run. Behavioral failures/timeouts retain their distinct diagnostics. Skip, cancellation, pending or old-SHA success is never current acceptance |
| Release candidate | None selected or authorized by this checkpoint. Later approval freezes a SHA; only relevant reviewed fixes enter it, each with a new recorded SHA and affected requalification |

## Holds, fixups and next topics

| Work | Owner / status | Next permitted action |
| --- | --- | --- |
| Combined 4308/4309 Linux acceptance | Checkpoint manager | Focused checks/review, genuine manifest regeneration, then one automatic integrated Linux batch; preserve failures and repair attributable defects |
| Native failures from 4294 | Native qualification owner | Complete bounded source-specific follow-ups, preserving idle-host report and unresolved native stalls; Linux does not certify FSEvents |
| PR 4307 (`e467b9b49283e8177e8fb88053f10d22be4dc252` at intake) | Runtime integration owner; real Cargo admission unresolved | Excluded. Resolve admission and evidence before this affected chain proceeds; no live contract application |
| PR 4310 (`7ca64ee35e663ee47b59b39796c3b2dcb90b289b` at intake) | Separate admission/local-provenance task | Do not duplicate or integrate its follow-up here; implementation approval is not application approval |
| PR 4306 | Error identity lane, already part of separate 4307 work | Do not separately import into this checkpoint or confuse its native evidence with these topics |
| PR 4305 | Native Mac sharding proposal | Separate review/adoption decision; not imported. Four cold shards may compile four times; no assumed fourfold speedup |
| PR 4293 | HOLD, bounded-drain proposal | Unchanged, unimported; no implicit adoption |
| Next independent topic | Development manager | Select reviewed dependency-safe work against this base, with focused checks; no need to wait for unrelated Mac qualification |

Unrelated understood failures need a named fixup and retained evidence, not a project-wide
freeze. Security, data-loss, unknown foundation and contract-admission failures block their
affected dependency chain. No source PR is closed by this ledger and no runtime default changes.
