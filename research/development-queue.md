# Development checkpoint and qualification queue

This ledger applies the [canonical policy](../CLAUDE.md#moving-development-and-frozen-qualification).
It is not release approval, a scheduler, or a replacement for source-bound receipts.
Current snapshot: 2026-10-19 20:45 UTC, `main` `013b0ab` after PRs 4323, 4328, 4329 and 4324;
integration/CI triage owner: the project integration manager (one fixed owner). The "Round record
at main `21ef7c4`" section and everything from "Historical checkpoint 4294" onward are earlier
records, kept as evidence; where they say "draft", "candidate" or "pending", the current roadmap
below wins.

## Current roadmap (main `013b0ab`, 2026-10-19 20:45 UTC)

### Main integration of 2026-10-19 (owner-approved)

The owner approved this order at 19:26 UTC. Each PR was merged with a normal merge commit, with no
squash, rebase or force-push, after its latest head showed every required check green. The ruleset
does not require an up-to-date head, and none of the four PRs touched another one's files.

| Order | PR | Tested head (required CI green) | Merge commit on `main` |
| --- | --- | --- | --- |
| 1 | [4323](https://github.com/Takazudo/zudo-front-builder/pull/4323) roadmap ledger | `ccf7363` | `7f58a01607db44c418ec68b08746d806bdc553da` |
| 2 | [4328](https://github.com/Takazudo/zudo-front-builder/pull/4328) packed-build byte parity (4231) | `732a03de` | `7426913076cf6ef76b948281b2f78a8903e3c668` |
| 3 | [4329](https://github.com/Takazudo/zudo-front-builder/pull/4329) docs cleanup and v4.5.4 drafts | `ed55aaa` | `695ca660c3b166e5a43bc0370b35c1eb63e7dcd5` |
| 4 | [4324](https://github.com/Takazudo/zudo-front-builder/pull/4324) nextest 0.9.140 pin | `7aeb31f` | `013b0ab8fe5059e64ac0f7592a6eb938ac476e29` |

Automatic `main` CI, first attempt, no retries or dispatches:

| `main` SHA | health | Binary smoke, scaffold E2E and showcase | Other |
| --- | --- | --- | --- |
| `7f58a01` | [39315505385](https://github.com/Takazudo/zudo-front-builder/actions/runs/39315505385) cancelled (superseded) | [39315505689](https://github.com/Takazudo/zudo-front-builder/actions/runs/39315505689) success | — |
| `7426913` | [39315529982](https://github.com/Takazudo/zudo-front-builder/actions/runs/39315529982) cancelled (superseded) | [39315529777](https://github.com/Takazudo/zudo-front-builder/actions/runs/39315529777) cancelled (superseded) | — |
| `695ca66` | [39316341673](https://github.com/Takazudo/zudo-front-builder/actions/runs/39316341673) cancelled (superseded) | [39316341725](https://github.com/Takazudo/zudo-front-builder/actions/runs/39316341725) cancelled (superseded) | docs-deploy [39316341746](https://github.com/Takazudo/zudo-front-builder/actions/runs/39316341746) success |
| `013b0ab` | [39316381257](https://github.com/Takazudo/zudo-front-builder/actions/runs/39316381257) success (completed 20:47 UTC; `build (no-v8)`, `health` with workspace nextest on the 0.9.140 pin, and all seven `wasm-md` legs) | [39316381226](https://github.com/Takazudo/zudo-front-builder/actions/runs/39316381226) success | actionlint [39316381210](https://github.com/Takazudo/zudo-front-builder/actions/runs/39316381210) success |

- Cancelled runs are not passes. `main` runs are superseded by the next push, so the intermediate
  heads have no complete health result. `013b0ab` contains all four merges, and its run is the
  integrated evidence.
- At `013b0ab` the smoke run passed both binary builds, the four local-mode smokes, the Amazon
  Linux glibc floor, `Scaffold E2E` (now including the 4328 Stage 7 parity check) and the built-site
  browser smoke. `Showcase deploy (create-zfb, production)` deployed and passed its post-deploy
  health check (job 118285903814).
- The docs deploy at `695ca66` published the merged README/installation changes. The v4.5.4
  drafts live under `research/`, so no version page was published.
- Still not run at `013b0ab`: `exam.yml` with the 0.9.140 pin (next scheduled Sat 2026-10-24),
  any native Mac or Windows run, and Security Audit (last red at `21d6807e`, see 4314).

### Owner policy from 20:38 UTC: Claude-led, implementation first

Claude leads and keeps implementing. Release preparation must not stop implementation. When the
Mac is free, verified changes are gathered into a release checkpoint that downstream projects can
dogfood. This is release planning, not blanket publish approval. Before any actual release, report
the exact version, the included changes and the remaining qualification. The owner is away for
about six hours from 20:33 UTC. Decisions that need new approval are collected below and are not
inferred. The message did not ask for additional Mac tests.

Two separate tracks, which must not be mixed:

- **4.5.4 patch checkpoint (now; L5).** The version is approved as 4.5.4; the release is not. The
  candidate is `main` at its latest CI-green head, currently `013b0ab`. The drafts and the bump table are in `research/release-prep-v4.5.4/README.md`. Package-facing
  content is the two islands fixes (4298, 4308). The rustls 0.23.46 transition is not on `main` and
  is not in the drafts. L5 is preparing the checkpoint report and a draft bump PR. md-wasm sizes and
  island-size ceilings come from CI measurement; a red ceiling is recorded, never relaxed.
- **Deno exit roadmap (future; 4303, not assigned).** Nothing in the current plan removes Deno or
  switches the default. The remaining blockers are listed in the
  [4303 comment](https://github.com/Takazudo/zudo-front-builder/issues/4303#issuecomment-6276732610):
  the Gate A default-switch approval, native qualification on four platforms, lock-admission apply,
  the deno_core crates still on the default path, a missing direct-V8 CI lane (L2 is drafting an
  opt-in, non-required one), and a separate Gate B approval for removal. The work that does
  proceed is opt-in only: 4307 and the 4302 Linux x64 row and harness.

### Awaiting owner approval (held, not inferred)

| Decision | State and evidence |
| --- | --- |
| Darwin apply for the rustls lock transition (4314) | Stage attempt 2 matched the recipe's expected values, as checked from Moya's reported values ([L1 verdict](https://github.com/Takazudo/zudo-front-builder/issues/4314#issuecomment-6276677595)). Apply waits for an independent review of `staged.diff` (SHA-256 `942fd70b…`), which needs that file to be shared first. `--apply` and live admission are not approved. Apply must also update the two hand-pinned history tests (`repinnedFrom` and the rewire route) and get a fresh-context review. Recipe defect found in L1's stage review: `--source-root` pointed at SOURCE, so the staged README live hash is SOURCE's, and `--apply` on the current base is refused as "moved since staging". Routes: apply on a `bda633b` checkout then a normal merge into the base, or re-stage. Both stages write the same two files (the README and the test file), so after Darwin apply the Linux stage must be re-run from the same artifact (expires 2026-10-26) |
| Linux x64 lock transition apply (4314) | `decision-linux-x64.json` still pins the old lock, so Darwin alone does not turn the base green (same comment). The Linux stage is done in Cloud by integration-lead decision, within the approved stage-only scope: exit 0 ([4314 comment](https://github.com/Takazudo/zudo-front-builder/issues/4314#issuecomment-6276966739); evidence in draft [4332](https://github.com/Takazudo/zudo-front-builder/pull/4332) under `stage-evidence/linux-4314/`, manifest `9b2746b9…`, diff `fddf15d5…`). Apply stays held. Apply must also update the two hand-pinned history tests (`repinnedFrom` and the rewire route) and get a fresh-context review |
| Publish the 4314 Darwin receipt's local paths and operator string | (a) accept as published or (b) reissue the receipt with neutral paths. A decision card is open in the L1 thread; L1 recommends (a) |
| Merge the security base into `main` | `base/4314-security-darwin-evidence` (4315, 4319, 4326) reaches `main` only after both applies, together with the other candidates; not approved |
| Merge 4317 (hosted Linux exact-source lane) | [PR 4317](https://github.com/Takazudo/zudo-front-builder/pull/4317), draft on `base/4314-security-darwin-evidence`; run 39248247275 passed for source `bda633b` only |
| Adopt 4305 native sharding (with 4325) | On hold until after the next exam (owner, 19:26 UTC). [4305](https://github.com/Takazudo/zudo-front-builder/pull/4305) and [4325](https://github.com/Takazudo/zudo-front-builder/pull/4325) stay draft; 4325 needs `main` merged in after 4324 because of the `exam.yml` install-line conflict |
| Whether rustls 0.23.46 must be on `main` before the 4.5.4 cut | Prepared security entry and the reasoning: `research/release-prep-v4.5.4/README.md`, section "Security lane: not on `main`, not in these notes". 4314 stays red until the transition lands |
| 4231 item (2): Darwin, Windows and Mac WebKit checks of the 4298 token change | NOT RUN. Item (1) is met by 4328 (`7426913`). Options: run it on the Mac, waive it, or move it to a separate D-class issue. 4231 stays open until then, and 4252, 4243 and 4241 wait on 4231 in that order (L4 comments on each issue; [4231](https://github.com/Takazudo/zudo-front-builder/issues/4231)) |
| Run the 4.5.4 release | Not approved. Before asking, report the exact version, the included changes and the remaining qualification (native Mac at the candidate SHA, the Darwin and Linux applies, 4305 on hold) |

The 4252 S7 ledger is that issue's own deliverable, so the missing per-PR merge SHAs and CI for
S0, S1 and S2 and the post-merge `main` CI for `15a76210` (noted by L4 on 4252) are recorded there
when 4252 is worked, not copied into this roadmap.

Also still held from earlier rounds: 4268 (noyalib 0.0.58 `render` is over its ceiling), 4289 and
4293 (drain contract), 4311 (provenance policy), and the 4303 gates above.

### Threads, owners and open draft PRs

| Lane | Owner | Current work | Open PRs |
| --- | --- | --- | --- |
| L0 integration, CI triage | roadmap thread | This ledger; `main` CI at `013b0ab` | this ledger PR (draft) |
| L1 security, Darwin evidence | L1 thread | Review `staged.diff` once shared; Linux stage done in Cloud by integration-lead decision (evidence in draft [4332](https://github.com/Takazudo/zudo-front-builder/pull/4332)); no apply | [4317](https://github.com/Takazudo/zudo-front-builder/pull/4317) (draft, base `base/4314-security-darwin-evidence`) |
| L2 opt-in direct V8 | L2 thread | Opt-in direct-V8 CI lane (manual or label trigger, not required); 4302 Linux x64 row and harness | [4307](https://github.com/Takazudo/zudo-front-builder/pull/4307) (draft; lock gate red) |
| L3 native qualification | L3 thread (idle) | Nothing running; 4305 waits for the next exam | [4305](https://github.com/Takazudo/zudo-front-builder/pull/4305), [4325](https://github.com/Takazudo/zudo-front-builder/pull/4325) (drafts, on hold) |
| L4 issue sweep | L4 thread | 4019 closed with evidence; 4231, 4252, 4243 and 4241 held on 4231 item (2) | — |
| L5 docs, release prep | L5 thread | 4.5.4 checkpoint report and draft bump PR | (new drafts pending) |

Other open drafts not owned by a running thread: [4306](https://github.com/Takazudo/zudo-front-builder/pull/4306)
(superseded; contained in 4307) and [4293](https://github.com/Takazudo/zudo-front-builder/pull/4293)
(HOLD, 4289 drain contract).

## Round record at main `21ef7c4` (before the 2026-10-19 main integration)

### Cloud round summary (2026-10-19)

All Cloud-doable work from this round is done. States below use the ledger vocabulary: implemented,
reviewed, integrated (into which branch) and verified (which evidence, which SHA). Nothing here is
released, applied to live admission, or merged to `main` beyond PR 4322.

**1. Outcomes by lane**

| Lane | Implemented + reviewed | Integrated | Verified | Not run |
| --- | --- | --- | --- | --- |
| L0 roadmap | This ledger (PR 4323), 4235 body refresh | Draft PR to `main` | Docs format check; PR CI green at `b1268ec` | — |
| L1 security | 4315 (rustls 0.23.46), 4319 + 4311 unified as 4326 | `base/4314-security-darwin-evidence` `7e79458` | Hosted Linux run 39248247275 (source `bda633b`, log and artifact checked separately) | Darwin collection, stage+apply; base health keeps 2 known reds until apply |
| L2 direct V8 | 4307 refreshed onto main, Wind manifest regenerated, 4302 harness (9 review fixes) | `base/4301-direct-v8` `bea94d1` | Linux x64 native row PASS at `14d984a`; CI red only on unadmitted lock | Other native rows, Windows observer, packaging, Rolldown coverage, frozen acceptance, Chromium, default local build |
| L3 native | 4324 (nextest 0.9.140 pin, install-action v2.82.10), 4325 (4305 fixtures at 0.9.140) | Drafts only | 4324 CI green, 6,874 passed; health now installs nextest from the tarball with SHA-256 check; Mac helper receipt at `21ef7c4` (graph lane 43/43) | `exam.yml` with the new pin not yet run (next scheduled Sat 2026-10-24); native full workspace at `21ef7c4` |
| L4 sweep | Classification, closures, 4231 test (4328) | 4328 ready for review | 4328 CI 15 success / 0 failure at `732a03de` | — |
| L5 docs/release prep | 4329: current-state README, toolchain/Python 3.11 docs, five-lane v4.5.4 drafts, bump proposal | Draft | Docs build passed; health still running at this update | No release, tag or publish |

**2. Issues**

- Closed as completed: 4251, 4219, 4218, 4224, 4223, 4098 (evidence comments on each); 4311 closed as
  integrated into 4326; probe PR 4327 closed after recording its size result on 4268.
- Open with next action: 4231/4252/4243/4241 (merge 4328, then S7 ledger, then close in that order);
  4314 (close after rustls reaches `main` and a Security Audit run is green); 4294 (native full
  workspace at a selected SHA); 4133 (Mac reproduction); 4301/4302 (lock admission, then native
  rows); 4268 (owner decision; 0.0.58 over ceiling); 4304 (watch 2026-10-25); 4019 (real supervisor
  suite on a reaping init); 4289/4293, 4303, 318, 3386, 3372, 3879, 3357, 1373 (owner or upstream
  triggers as classified above); 4123/4124/4276 (Mac or owner manual); 1492 sticky.

**3. Main candidates and recommended order**

1. **4323** (this ledger; docs-only; conflict-free with the others).
2. **4328** (packed-build byte comparison for 4231; CI green; ready). Unblocks the 4231 chain.
3. **4329** (docs and v4.5.4 drafts; drafts live outside the changelog path so a `main` push
   publishes no version page). Merge after its health finishes green.
4. **4324** (nextest 0.9.140 pin). Merge before any 4305 work; first `exam.yml` run with it is the
   2026-10-24 schedule unless the owner dispatches one.
5. **4305 + 4325** only if the owner adopts sharding: rebase 4325 onto `main` after 4324, because
   both edit the `exam.yml` nextest install lines (merge-tree conflict confirmed); keep 4324's
   install-action v2.82.10 pin.

Not candidates now: `base/4314-security-darwin-evidence` (red until stage+apply is approved and
done; 4317 rides with it) and `base/4301-direct-v8` (waits on lock admission, which depends on L1).

**4. Minimal Mac prompts**

| Prompt | Fixed SHAs | Status |
| --- | --- | --- |
| L1 `/mnt/project-files/l1-security-evidence/mac-prompt-4314.md` (stage only, `LOCAL-SECURITY-4314.md`) | SOURCE `bda633b`, TOOL `7e79458` | Waits for the owner to lift the Mac preflight hold |
| L3 [4294 comment](https://github.com/Takazudo/zudo-front-builder/issues/4294#issuecomment-6274028064) (new capture helper) | `21ef7c4` | Executed by Moya: [receipt](https://github.com/Takazudo/zudo-front-builder/pull/4322#issuecomment-6274889047) |
| L2 [4302 native-row handoff](https://github.com/Takazudo/zudo-front-builder/issues/4302#issuecomment-6275155771) (`PLATFORMS.md` "Native row run prompts") | CANDIDATE `14d984a` | Not authorized or run |

**Owner decisions recorded 2026-10-19 19:26 UTC:** merge 4323 → 4328 → 4329 → 4324 to `main` after
each PR's required CI is green; 4305/4325 sharding adoption is on hold until after the next exam;
Mac security is approved for recipe/resource preflight then stage collection only (no `--apply`, no
live admission); prepare version 4.5.4 (patch) notes and bump, with the release itself not approved.

**5. Remaining conditions for release and owner decisions to take together**

- Lock admission apply (rustls transition on L1; direct-V8 edge on L2): Darwin stage collection is approved, `--apply` is not.
- Whether to adopt 4305 sharding (with 4325): on hold until after the next exam.
- Version: 4.5.4 patch chosen by the owner; the release itself is not approved.
- Whether rustls 0.23.46 must be on `main` before the cut (4314 stays red until it is).
- Native verification before or after the cut (full workspace at the candidate SHA is not run).
- Whether to merge 4317 (hosted Linux exact-source lane).

### Main and its CI

- `main` = `21ef7c4678adba115a3607801ededeeaf6e5814e` (merge of PR [4322](https://github.com/Takazudo/zudo-front-builder/pull/4322)).
  It contains PR 4312 (checkpoint: 4308, 4309, 4313, 4316, 4318; merge `502d9e6e`), PR 4310
  (external-edge admission capability, head `28be51b`), PR 4321 (capture failure handling, `ac5eedd`)
  and PR 4320 (capture TMPDIR fixture, `35f7b86`). Earlier merged work referenced below: 4298
  (`15a76210`), 4234 (`d28cdd57`), 4259, 4260, 4120 (`8b1fe8b0`).
- Exact-head automatic CI on `21ef7c4`, first attempt, no retries:
  [health 39294618774](https://github.com/Takazudo/zudo-front-builder/actions/runs/39294618774) success
  (`build (no-v8)`, `health`; `wasm-md` skipped by its path gate) and
  [binary/scaffold/showcase 39294618739](https://github.com/Takazudo/zudo-front-builder/actions/runs/39294618739) success.
  No docs-deploy or wind-computed-style run was triggered for this head. This is Linux evidence only.
- Not yet run at `21ef7c4`: any native Mac/Windows execution, the scheduled `exam.yml` (last run
  39002778546 at `d1c8b288`; next scheduled Sat 2026-10-24), Security Audit (last run 39217854663
  red at `21d6807e`, see 4314), Supervisor watch (next Sun 2026-10-25), YAML candidate watch (next Thu 2026-10-22).
- Cloud host at snapshot: 4 vCPU, 15 GiB RAM, about 30 GiB writable disk allowance (`df` shows
  9.1G used / 30G available). That is the same class of limit that stopped a full Rust build in
  PR 4315's receipt; do not repeat a cold full workspace build here. Use hosted Linux Actions.

### Lanes, owners and next actions

One owner per lane; files listed are the lane's edit scope. A lane must not edit another lane's
files without a recorded hand-off. `research/development-queue.md` itself is edited only by the
integration manager.

| Lane | Items | Branch / head | Edit scope | Depends on | Next action |
| --- | --- | --- | --- | --- | --- |
| L0 Integration + CI triage | main, this ledger | `main` `21ef7c4` | this file; `base/**` aggregation branches | — | Create a fresh `base/**` from `main` for the next batch; record exact-head CI per integrated head |
| L1 Security + Darwin evidence | [4314](https://github.com/Takazudo/zudo-front-builder/issues/4314); 4315, 4319 and unified-module 4326 integrated, 4311 closed as integrated; [4317](https://github.com/Takazudo/zudo-front-builder/pull/4317) open on the new base | `base/4314-security-darwin-evidence` `7e79458` (source `bda633b` unchanged) | `Cargo.lock`; `research/v3-island-size/**`; `scripts/{rustls-security-transition,island-size-local-evidence,rebaseline-island-size-darwin,rebaseline-island-size-linux}.mjs` + tests; `.github/workflows/{island-size-darwin,local-darwin-security-evidence}.yml` | Owner Darwin collection and stage+apply approval | Base health has two known reds (old lock-pin assertions; Linux lock provenance) that stay until stage+apply. Run 39248247275 log and artifact checked separately and recorded on 4317 (zip retained). Mac prompt: `/mnt/project-files/l1-security-evidence/mac-prompt-4314.md` (SOURCE `bda633b`, TOOL `7e79458`) |
| L2 Opt-in direct V8 | [4301](https://github.com/Takazudo/zudo-front-builder/issues/4301), [4307](https://github.com/Takazudo/zudo-front-builder/pull/4307) (contains 4306 as `322ad117`), [4302](https://github.com/Takazudo/zudo-front-builder/issues/4302) harness | 4307 head `14d984a` (main merged at `bd4feaf`); integrated by normal merge into `base/4301-direct-v8` `bea94d1` (not a main candidate) | `crates/zfb*/**` touched by 4307; `prototypes/direct-v8/**`; `docs/public/wind-examples/manifest.json` (regenerate only) | Lock admission: declaration, receipt and dry proof PASS on both targets; apply waits on Darwin evidence, L1 and owner approval. 4310 capability alone does not admit it | CI on 4307: 25 success / 1 failure / 7 skipped; the failure is only health's unadmitted-lock assertion (main is green); cause, fix owner (L1) and next check in [4307 comment](https://github.com/Takazudo/zudo-front-builder/pull/4307#issuecomment-6275153813). Wind manifest regenerated with the official generator (173 fresh). 4302 harness (`PLATFORMS.md`, `qualify-target.py`; 9 review findings fixed); Linux x64 native row PASS; fixed native-row procedure and minimal Rolldown failing case in [4302 comment](https://github.com/Takazudo/zudo-front-builder/issues/4302#issuecomment-6275155771). NOT RUN: Linux arm64, Darwin arm64/x64 and Windows native rows, Windows observer, packaging harness, Rolldown coverage, frozen producer acceptance at this head, Chromium, default (`embed_v8`) local build |
| L3 Native qualification | [4294](https://github.com/Takazudo/zudo-front-builder/issues/4294), [4305](https://github.com/Takazudo/zudo-front-builder/pull/4305), [4133](https://github.com/Takazudo/zudo-front-builder/issues/4133); drafts [4324](https://github.com/Takazudo/zudo-front-builder/pull/4324), [4325](https://github.com/Takazudo/zudo-front-builder/pull/4325) | 4324 `7aeb31f` (base main); 4325 `9e14d59` (base 4305 branch); 4305 `bd02b8a` | `.github/workflows/{health,exam}.yml` nextest pins; `.github/actions/native-exam-setup/**`, `scripts/native-exam*`, `tests/unit/native-exam.*`, `tests/fixtures/native-exam/**` | Mac owner; 4305 adoption by owner | Decision: unify on nextest 0.9.140 (owner, 2026-10-19). Before it, four versions coexisted (4305 0.9.148; `health.yml` 0.9.138; `exam.yml` 0.9.136 via the `taiki-e/install-action` v2.79.5 pin, seen in run 39002778546; Mac runs and capture helper 0.9.140). 4324 pins health/exam to 0.9.140 and bumps `taiki-e/install-action` to v2.82.10 (health now installs nextest from the release tarball with a SHA-256 check; CI all green, 6,874 passed; `exam.yml` not yet run with it); 4325 re-takes 4305's fixtures and conflicts with 4324 on the `exam.yml` install lines. Minimal Mac prompt: [4294 comment](https://github.com/Takazudo/zudo-front-builder/issues/4294#issuecomment-6274028064) |
| L4 Issue sweep | items listed below | new topic branch per change | issue comments; new test for 4231 | — | Closures done (see sweep); 4231 test in 4328 (main candidate); S7 ledger 4252 after it merges |
| L5 Docs and release prep | README, CLAUDE.md, changelog drafts | new topic branch | `README.md`, `docs/src/content/docs/**` (not `docs/public/wind-examples/`), `CLAUDE.md` | Lands after L1/L2 decisions for content | Separate current / opt-in / planned features; draft five-lane notes and a version-bump proposal from actual changes; no publication |

Shared-file rules: `Cargo.lock` is L1-only (a 4268 size probe may use a throwaway draft PR that
is never merged). `docs/public/wind-examples/manifest.json` is regenerated once by whichever lane
integrates Rust changes, never hand-edited. `crates/zfb/src/commands/dev.rs` belongs to L2 while
4307 is open, so 4133 work waits or coordinates.

### Native qualification status (4294)

The 2,094 "unrun" figure is superseded. The native first run at `21d6807e` executed 6,854 tests:
6,838 passed, 7 failed, 9 timed out, 0 unrun; doctests exit 0. Original JUnit is lost and is not
treated as recovered.

| Group | Tests | Later fixed-SHA evidence | Open |
| --- | --- | --- | --- |
| Graph identity (2 failed) | `build_package_routes::virtual_module_only_import_ships_as_island`, `scratch_dir_concurrent_e2e::concurrent_dev_build_check_are_isolated` | Product fix 4308; native PASS at `92e25ad4` (13 + 7 + 1 + 1) | No full-suite run at `21ef7c4` |
| Watcher (9 timed out at 300 s) | `zfb-watcher::recursive_dirs` (9 cases) | Test redesign 4309; native 23/23 at `a76e4775` | Root cause of the original stalls unexplained; original XML lost |
| Server watch (3 failed) | `zfb-server::watch_add_confirm` (3 `real_watcher_*`) | 4309; native 3/3 at `a76e4775` | Same |
| Dev bind (2 failed) | `dev_bind_before_walk_e2e` (2) | 4309; native 2/2 at `a76e4775` | Same; not run natively after `b605a4de` |

None of the 16 failures from `21d6807e` reproduced on current main; native Mac passes for them
were recorded at `92e25ad4`, `a76e4775` and `b605a4de`. The two pre-4321 Mac captures (26 tests:
23 + 3) used the old helper and are not evidence for the helper now on main. The new helper made a
real capture on Linux at `21ef7c4` (`recursive_dirs` 23/23); that is not Mac evidence. A separate,
limited native Mac receipt now exists at `21ef7c4`
([receipt](https://github.com/Takazudo/zudo-front-builder/pull/4322#issuecomment-6274889047)):
the new helper's 25 unique offline tests passed once each under logical and physical TMPDIR, and a
real capture of `zfb-graph --lib --locked --retries 0` passed 43/43 (cargo-nextest 0.9.140, `ci`
profile, isolated clean source/target), with JUnit byte identity, SHA-256 and receipt/source
fingerprints checked. Its scope is this SHA's helper and that graph lane only: it claims nothing
about the 4315 Darwin producer, V8, the watcher, the full workspace, b4push, live admission or release. 4305 merges
cleanly into main and its 20 offline harness tests pass, but the native sharded run has never been
dispatched; adoption waits on the owner (nextest is being unified on 0.9.140). 4133 (eight
`cargo test -p zfb --lib` Mac hangs) did not reproduce under nextest and remains unverified; 4309
does not cover it. Evidence links are in the latest comments on 4294, 4305 and 4133.

### Issue sweep classification (executed by L4; closures carry evidence comments)

- **Closed as completed (2026-10-19):** 4251 (via 4298 `15a76210`); 4219, 4218, 4224 and 4223
  (via 4234 `d28cdd57`); 4098 (4120 merged; remaining work tracked in 318, 4123, 4124, 4133).
- **Open, waiting on 4231:** 4231 stays open until 4328 merges (it adds the missing comparison of
  workspace-build and packed-tarball-build bytes); 4252 (S7 ledger), then 4243 and 4241, close after it.
- **B, superseded:** PR 4306 (contained in 4307).
- **C, Cloud work:** packed-build byte-comparison test for 4231: [4328](https://github.com/Takazudo/zudo-front-builder/pull/4328) head `732a03de`, CI 15 success / 0 failure, ready for review and a main candidate; 4231's acceptance criteria were replaced to match it; 4252 S7
  ledger; 4235 body refresh; 4019 (the `tests/process-reaping-diagnostic/diagnose.py` probe passed
  on the current Cloud init, but that is not the real supervisor suite, so it stays open); 4268
  size probe done: draft 4327 measured `render` at 1,114,879 B, over the 1,100,000 B ceiling;
  recorded on 4268 and the probe closed (tarball, npm test and browser legs NOT RUN; pin stays `=0.0.55`).
- **D, waits on Mac/env:** 4294, 4133, 4123, 4124, 4276, 4315/4319 Darwin collection.
- **E, owner decision / HOLD / trigger:** 4289 and PR 4293 (HOLD: drain contract); 4303 (default
  cutover and Deno removal gates); 4302 (after 4301); 4305 (adoption; nextest unified on 0.9.140); 4311
  (provenance policy); 4317 (merge decision); 4268 (0.0.58: 2.3–2.9x slower and `render` over ceiling);
  4304 (red only from intentional zudoDoc identity drift; await the 2026-10-25 watch); 1373
  (upstream triggers; the `deny.toml` bincode reason is current: the lock still shows `deno_core`
  0.399.0 and `syntect` 5.3.0 on bincode 1.x); 318, 3386, 3372, 3879, 3357; 1492 stays sticky.

## Historical checkpoint 4294 (integrated to main by PR 4312)

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

### Historical local checkpoint evidence

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

### Trigger audit and external effects

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

### Native qualification stays on selected source

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

### Development and qualification matrix

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

### Per-command nextest evidence capture (PR 4318)

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

### Holds, fixups and next topics

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
