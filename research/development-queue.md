# Development checkpoint and qualification queue

This ledger applies the [canonical policy](../CLAUDE.md#moving-development-and-frozen-qualification).
It is not release approval, a scheduler, or a replacement for source-bound receipts.
Snapshot: 2026-10-19; integration/CI triage owner: the development-checkpoint manager.

## Development checkpoint

- Branch: `base/4294-development-checkpoint`; parent: `21d6807e442bbed61e578c165a91d64c4a7dc141`.
- Scope: reviewed graph-path fix from PR [4308](https://github.com/Takazudo/zudo-front-builder/pull/4308),
  bounded watcher/dev tests from PR [4309](https://github.com/Takazudo/zudo-front-builder/pull/4309),
  ICU publication fix from PR [4313](https://github.com/Takazudo/zudo-front-builder/pull/4313),
  qualification-evidence docs from PR [4316](https://github.com/Takazudo/zudo-front-builder/pull/4316),
  the per-command nextest evidence helper from PR [4318](https://github.com/Takazudo/zudo-front-builder/pull/4318),
  and this workflow policy. Native acceptance of the combined checkpoint is not claimed.
- Development head: resolve `git rev-parse origin/base/4294-development-checkpoint` after fetching.
  The aggregation PR records each immutable checkpoint SHA and exact-head CI receipt. A later
  tip does not inherit the earlier receipt. This document cannot contain its own commit hash.
- At intake all remote branch heads and all seven open PRs were inspected. Existing `base/**`
  heads concern 4187, 4199, design-system playground, and post-4.5.0 maintenance; none is adopted
  as an implicit parent. No active source branch is rewritten or closed.

| Source | Selected immutable head | Evidence at that head | Development decision |
| --- | --- | --- | --- |
| PR 4308 | `92e25ad4da392b0e566320f5e477e62bf134adc7` | [Linux health 39143914080](https://github.com/Takazudo/zudo-front-builder/actions/runs/39143914080) and other automatic workflows succeeded; preview upload skipped for draft | Integrate exact head; exact-source native follow-up passed (owner receipt below) |
| PR 4309 | `a76e4775fd6ccf6f1d929f8ce5bd8d25a2298b3a` | [Linux health 39146914767](https://github.com/Takazudo/zudo-front-builder/actions/runs/39146914767), 6,868 workspace tests passed; 27 checks succeeded, 6 conditional skips | Included; 28 selected native checks passed, with original watcher XML missing (receipt below) |

The source changes are disjoint except `docs/public/wind-examples/manifest.json`.
Each source PR generated its own compiler digest. The combined checkpoint must regenerate
with the official `docs/scripts/wind-preview-assets.mjs` and the matching built workspace
compiler, then run `--check`; neither parent's digest proves the combined sources.

## Historical local checkpoint evidence

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

The subsequent integrated Linux batch is recorded in [PR 4312](https://github.com/Takazudo/zudo-front-builder/pull/4312).
At checkpoint `b605a4deb17c387f65cc2d8c81893473c84253b0`, tree
`6cd7a6da497ece0c09ffbc2d9c895ea24acfc2ce` matches the tested PR 4313 merge tree.
[Health 39168306313](https://github.com/Takazudo/zudo-front-builder/actions/runs/39168306313)
reported 6,874 workspace passes (86 excluded), 52 environment-gate passes (1,674 excluded),
and 457 md-extras passes: 7,383 executed JUnit cases. Doctests are separate.
This is recorded tree-equivalent evidence, not an all-green exact-head rollup: redundant
aggregation health/docs/binary runs were cancelled after the tree comparison. Cancelled jobs
remain cancelled. No local full-workspace run duplicates that batch.

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
those archive bytes. PR 4309 has its own [later native receipt](https://github.com/Takazudo/zudo-front-builder/pull/4309#issuecomment-6258920017)
at `a76e4775fd6ccf6f1d929f8ce5bd8d25a2298b3a`: 23 recursive_dirs, 3 watch_add_confirm and
2 named dev cases passed on their first execution, without retries. Compilation of three
binaries is compile-only evidence. One guard refusal preceded execution. The original watcher
JUnit XML was overwritten and is unrecoverable; its full log, 23 identities and exit remain,
while original server/dev XML survives. No reconstructed XML or replacement run closes that gap.
This supersedes earlier “native unrun” summaries only for those 28 selected checks; it neither
explains the original nine stalls nor qualifies the full suite or combined development SHA.
Do not silently retarget native work to this development branch.

| Qualification record | State and next action |
| --- | --- |
| Historical native first run | [Original report](https://github.com/Takazudo/zudo-front-builder/issues/4294#issuecomment-6251614904): 6,838 passed, 7 failed, 9 timed out, 86 excluded. Preserve raw receipt and first failures; not current-head acceptance |
| Graph follow-up | Owner reports PASS at frozen `92e25ad4…`: 13 unit + 7 integration + both targeted CLI cases. [Published receipt](https://github.com/Takazudo/zudo-front-builder/pull/4308#issuecomment-6254981484), with toolchain and local archive provenance; no integrated-SHA acceptance |
| Watcher/dev follow-up | 28 selected native passes at frozen `a76e4775…`; [receipt](https://github.com/Takazudo/zudo-front-builder/pull/4309#issuecomment-6258920017). Original watcher XML missing; full-suite qualification remains open. [Design](../crates/zfb-test-utils/timeout-test-design.md) |
| Mac build controls | Command-scoped `CARGO_BUILD_JOBS=1`, existing heavy guard, reusable caches; no global setting change or cargo clean. Separate compilation logs from test logs |
| Required receipt | Immutable SHA/tree; OS/CPU; exact rustc/cargo/Node/pnpm/esbuild/engine/ICU; target/profile/features/flags; command and cache identity; lock/input/binary hashes; artifact paths/digests; first exit/result and retries |
| Classification | Build timeout: incomplete build, tests not run. Behavioral failures/timeouts retain their distinct diagnostics. Skip, cancellation, pending or old-SHA success is never current acceptance |
| Release candidate | None selected or authorized by this checkpoint. Later approval freezes a SHA; only relevant reviewed fixes enter it, each with a new recorded SHA and affected requalification |

## Development and qualification matrix

These are development states at the snapshot, not release approval. None of the changes below
is claimed released. “Included” refers to the development checkpoint branch (heads `b605a4d` and
later), not to `main` before the checkpoint merge (`21d6807e`).
Exact source heads remain available through each linked PR; newer heads need new evidence.

| Work / immutable head | Implemented / checkpoint inclusion | Qualification and remaining boundary |
| --- | --- | --- |
| [4305](https://github.com/Takazudo/zudo-front-builder/pull/4305), `bd02b8afaddf158cee76f857526d1fa1409c258c` | Native sharding harness; excluded | Linux harness checks; native sharded full run unrun; adoption separate |
| [4306](https://github.com/Takazudo/zudo-front-builder/pull/4306), `5162d753b1b771749a75c227ad91e653ac3045cd` | Error constructor fix; in 4307, excluded here | Isolated-head native gap is not filled by 4309; integration evidence belongs to 4307 |
| [4307](https://github.com/Takazudo/zudo-front-builder/pull/4307), `e467b9b49283e8177e8fb88053f10d22be4dc252` | Opt-in direct V8 integration; excluded | Linux native CLI/graph/hydration evidence in PR; real lock-admission failure blocks this chain; default Deno unchanged |
| [4308](https://github.com/Takazudo/zudo-front-builder/pull/4308), `92e25ad4da392b0e566320f5e477e62bf134adc7` | Graph-path fix; included | Source-specific native 13 unit + 7 integration + 2 CLI passes; receipt above; not combined-head acceptance |
| [4309](https://github.com/Takazudo/zudo-front-builder/pull/4309), `a76e4775fd6ccf6f1d929f8ce5bd8d25a2298b3a` | Bounded watcher/dev tests; included | Native subset 28 passes; missing original watcher XML and full-suite gap retained |
| [4310](https://github.com/Takazudo/zudo-front-builder/pull/4310), `7ca64ee35e663ee47b59b39796c3b2dcb90b289b` | External-edge admission capability; excluded | Linux capability checks; no live application approval or application |
| [4311](https://github.com/Takazudo/zudo-front-builder/pull/4311), `b596c4d009afe7b0a5d1445680d2ee57438e6261` | Owner-attested evidence capability, stacked on 4310; excluded | Linux checks only; integrity checks do not authenticate native execution; no native collection/application |
| [4312](https://github.com/Takazudo/zudo-front-builder/pull/4312), `b605a4deb17c387f65cc2d8c81893473c84253b0` | Development checkpoint | Tree-equivalent Linux evidence above; cancelled duplicate runs not passes; combined native qualification open |
| [4313](https://github.com/Takazudo/zudo-front-builder/pull/4313), `b605a4deb17c387f65cc2d8c81893473c84253b0` | Atomic ICU file publication; merged into checkpoint only | Offline publication checks and Linux batch; no native ICU execution claim |
| [4314](https://github.com/Takazudo/zudo-front-builder/issues/4314) / [4315](https://github.com/Takazudo/zudo-front-builder/pull/4315), `bda633b3589e201737b02cf2060f7b24d41a45a7` | Security issue / rustls fix candidate; excluded | Admission-blocked: live contracts retain old lock pin. Independent Linux checks passed; Cloud local compilation exhausted disk before tests; hosted Linux exact-source verification later passed (below). Native evidence/application remain separate |
| [4316](https://github.com/Takazudo/zudo-front-builder/pull/4316), `2b2d12d897c81791fa46d26032a667bde1d5e220` | Qualification/Cloud capacity docs; included by normal merge | Docs only; no execution claim |
| [4317](https://github.com/Takazudo/zudo-front-builder/pull/4317), `9e8fbd1487adc6cc922dfd5d4272fbf9e30ec82a` | Hosted Linux exact-source Rust lane, stacked on 4315; excluded | Executed once for source `bda633b` (below); security-branch evidence only |
| [4318](https://github.com/Takazudo/zudo-front-builder/pull/4318), `048a8f18d1b4d31a24c1760401994c3d875dac45` | Per-command nextest evidence helper; included by normal merge | 15 offline helper tests; owner-reported WSL real capture (below); no native Mac claim |
| [4319](https://github.com/Takazudo/zudo-front-builder/pull/4319), `02e25b55f4de76152af82483a79c8a6b95cdc599` | Local Darwin security-evidence adapter, stacked on 4315; excluded | Final-head CI passed; no real Darwin size collection or live application yet |

For 4315, the [capacity and check receipt](https://github.com/Takazudo/zudo-front-builder/pull/4315#issuecomment-6266482957)
records Clippy, 457 md-extras tests, 10 jsx-pragma tests, no-V8 compile and saved Linux artifact
replay passing. Doctests report 3 passed/7 ignored; compile-only examples are not execution.
In that Cloud environment, full nextest and scoped environment-gate builds failed before
execution: **zero tests executed** there; that first result is retained. Saved-byte replay is not
full original-input replay or Darwin measurement.

The Linux-only option was later implemented in draft (unmerged) PR 4317 and executed once on a standard hosted
Ubuntu 24.04 runner: [run 39248247275](https://github.com/Takazudo/zudo-front-builder/actions/runs/39248247275),
workflow/control head `9e8fbd1487adc6cc922dfd5d4272fbf9e30ec82a`, checked-out source
`bda633b3589e201737b02cf2060f7b24d41a45a7`, Rust 1.99.1, no retries. Every recorded check exited 0:
build, inventory, Clippy, workspace nextest **6,874 passed / 86 skipped**, doctests 3 passed /
7 ignored, md-extras 457, md-extras Clippy, no-V8, jsx-pragma no-V8 10 (986 filtered), and the
environment-gate filter 52 passed / 1,674 skipped. These are separate selections over overlapping
test sets; do not add them into a unique total. This is evidence for the security branch source
only, not for this checkpoint or `main`, and it neither evaluates nor applies live admission.

PR 4319 (head `02e25b55f4de76152af82483a79c8a6b95cdc599`, stacked on 4315) passed its final-head
CI: [local Darwin evidence 39254654825](https://github.com/Takazudo/zudo-front-builder/actions/runs/39254654825)
(synthetic evidence) and [binary/scaffold 39254654886](https://github.com/Takazudo/zudo-front-builder/actions/runs/39254654886).
Real Darwin size collection and live application have not happened. The security owner owns
4315/4317/4319 and admission; none of them is imported into this checkpoint.

## Per-command nextest evidence capture (PR 4318)

PR 4318's helper (`scripts/capture-nextest-evidence.py`) passed its 15 offline tests. The
[owner-reported WSL capture](https://github.com/Takazudo/zudo-front-builder/pull/4318#issuecomment-6269269513)
used source `b605a4deb17c387f65cc2d8c81893473c84253b0` and tool head
`048a8f18d1b4d31a24c1760401994c3d875dac45` with nextest 0.9.140, Python 3.12.3, Rust 1.95.0 on
WSL2 ext4/inotify: `recursive_dirs` 23 passed and `watch_add_confirm` 3 passed; child and wrapper
exits were 0 with `status=captured`. The first capture's evidence was byte-unchanged after the
second; the second `preexisting.xml` matched the first original XML, and the second report had a
new run UUID. This rests on the owner's report: the raw files stay on the owner's WSL host and
were not independently retrieved or re-executed from Cloud. It does not recover #4309's lost
native XML, establish FSEvents/Windows behavior, or qualify a Mac run.

## Holds, fixups and next topics

| Work | Owner / status | Next permitted action |
| --- | --- | --- |
| Combined checkpoint Linux evidence | Checkpoint manager | Integrated batch recorded above; keep tree-equivalent evidence distinct from cancelled duplicate jobs and later-head acceptance. Heads after `b605a4d` (4316/4318 merges) need their own exact-head CI, recorded in PR 4312; until then it is pending |
| PRs 4315/4317/4319 (rustls security chain) | Security owner | Excluded from this checkpoint. Real Darwin size collection, admission resolution and any live application remain open |
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
