# zfb / zudo-front-builder

Workspace for the `zfb` Rust workspace + `@takazudo/zfb-runtime` and `zfb` npm packages.

## Development strategy: integrate first, use CI to find regressions

This repository is large. Repeating full builds and test suites for every child topic makes
super-epic development unnecessarily slow. The default development structure is
**super-epic → epic → sub-topic**, with one manager owning integration and CI triage.
This repo-specific policy takes precedence over generic workflow-skill instructions that require
heavy child verification or green CI before each development merge.

### Planning invariant and boundary vocabulary

Every implementation plan must inherit this section. Use a compact reference to
`CLAUDE.md#development-strategy-integrate-first-use-ci-to-find-regressions`, plus the
plan's actual integration parent, dependency DAG, verification owner, checks, and exceptions.
Do not copy this entire policy into each issue. The worked example and review cases in
[the planning policy reference](.claude/references/development-planning.md) apply to plans,
epics, sub-topics, and confirmation topics created by any workflow skill.

Branch names are not safety properties. Use a reviewed `base/**` development integration
branch, after inspecting the actual push, PR, preview and workflow-run triggers at its SHA.
`main` is a production boundary here: its pushes deploy the showcase, and docs-path pushes
can deploy docs. Development integration does not authorize pushing or merging to `main`.
Keep aggregation PRs **draft** to use the existing docs/showcase preview-upload guards;
a non-draft PR can publish a preview even when its base is `base/**`. CI logs/artifact uploads
still have external effects. Recheck triggers when workflows change; do not change credentials,
deployment guards or branch protections to make a route convenient.

| Boundary | Green required before proceeding? | Meaning |
| --- | --- | --- |
| Child → epic base | No by default | Reviewed and integrated; CI may be pending |
| Epic → super-epic/development parent | No by default | Integrated; CI alerts remain owned |
| Development base → next independent work | Conditional | Failure understood/localized, work independent, repair tracked |
| Final development completion | Evidence reconciliation required | Required checks satisfied; no unresolved real regression called complete |
| Release/npm publication/external impact | Yes, per the authoritative procedure | Release verification and publication gates apply |

Use these states precisely in plans and evidence ledgers:

- **Implemented:** the change exists.
- **Reviewed:** topic review completed.
- **Integrated:** merged into the intended development parent.
- **CI pending / CI observed:** recorded status for an exact SHA, with run/job evidence.
- **Verified:** required evidence passed for the named boundary and tested state.
- **Release-ready:** the release procedure's required evidence is satisfied.

Pending, cancelled, skipped, deferred, superseded, pass-on-retry, and failed are not passed.
Preserve the original failure and retry evidence; an older SHA's green result does not prove
that a newer integrated head is green.

Plans must name the manager responsible for CI diagnosis/fixing rounds, batch heavy checks at
integration boundaries, and assign practical cheap affected checks to children. Encode explicit
dependency edges for shared-file/generated-artifact collisions as well as logical dependencies.
Dependent work starts from the actual merged base and revalidates the assumptions it relies on.
If further integration would obscure failure attribution, pause that affected chain.
Confirmation topics collect evidence and missing coverage; they never manufacture a green status.

### Branches and progression

- Sub-topic branches merge into their epic base; epic PRs merge into the super-epic base; the
  terminal super-epic PR remains a draft against its recorded parent until that boundary is
  separately authorized. Follow the issue's explicit branch markers and dependencies. A standalone epic may target its recorded parent
  directly; do not reinterpret a bundled epic as an independent main-targeted PR.
- Run the authorized development chain autonomously (`-a`, plus `-m` for requested merges).
  Continue to the next ready topic or sibling epic in the same session. Do not wait for another
  user prompt at each boundary.
- **Merge reviewed development work without waiting for CI to turn green**, at sub-topic,
  epic, and super-epic levels **within the approved development branch topology**. Pending CI,
  or a red result already triaged by the manager, is not
  an automatic stop. Review, a clean committed worktree, dependency ordering, and honest reporting
  still apply. Merge completion means integrated; it does not mean verification passed.
- Only the manager pushes from the root checkout. Batch topic integration and pushes at useful
  boundaries instead of pushing every intermediate child commit. Keep the existing worktree
  push prohibition and remove clean merged worktrees promptly.
- Respect branch protection and required checks. No admin bypass, hidden skip, weaker timeout,
  assertion or gate is part of development integration. A denied protected merge stays blocked;
  continue safe independent work on the approved development branch instead.
- An explicit user instruction to stop, avoid merging, or wait for particular checks overrides
  this default.

### Child-agent checks and manager verification

- Child agents implement the topic, add appropriate regression coverage, self-review, and commit
  locally. Run inexpensive focused checks where practical: formatting, syntax, affected unit
  tests, or scoped type checks. State exactly what ran and what remains for integration.
- **Do not run heavy verification for each child topic.** No full workspace tests, b4push, full
  builds, browser/e2e suites, or repeated cold Rust compilation in child worktrees by default.
  A nominally scoped Rust test/check/clippy command that triggers a large rebuild belongs to
  the manager's verification lane too. Do not create a separate multi-gigabyte target per child
  merely to satisfy a generic completion template.
- The manager owns broad verification and batches it across integrated changes, primarily in
  CI. A dedicated confirmation topic collects/reviews evidence and can add missing coverage;
  it does not recreate a mandatory heavy local suite or CI wait at every epic boundary.
  Any necessary heavy local run still uses the machine-wide heavy guard.
- Monitor CI while development proceeds. Record the tested SHA, failed jobs, logs, and the topic
  or integration most likely responsible. Pending, cancelled, skipped, and deferred checks are
  never reported as passed. Keep unresolved verification visible in the manager ledger.

### CI alerts and fixing rounds

- Treat red CI as an alert requiring diagnosis and a fixing round, including failures found after
  a merge. The manager owns the failure until fixed or explicitly documented as unrelated or
  environmentally blocked; moving to another topic does not discard that responsibility.
- The manager decides whether to **continue development while fixing** or **finish diagnosis and
  repair first**. Continue independent work when the failure is understood and localized, the
  next work does not rely on the broken behavior, and a concrete fix is tracked.
- Pause the affected dependency chain when the cause is unclear or cross-cutting, the build or
  compiler/runtime foundation is broken, continuing would obscure attribution or compound the
  defect, or there is a known security, data-loss, contract-admission or deployment error. Diagnose, repair,
  and run the checks needed to restore confidence before resuming that chain. Unaffected work
  can continue when the manager judges it safe. Record the decision and reasoning.
- Never ignore a red run, call pass-on-retry clean, weaken a test, or suppress a failure to keep
  moving. Existing flake evidence, quarantine, and heavy-guard policies still apply.
- Before declaring the overall development complete, reconcile the CI alerts and run the
  necessary final verification/fixing round on the integrated result. Package publication is
  a separate boundary: the release procedure's verification and publication gates still apply.

### Moving development and frozen qualification

This is the normal workflow, including while slow native Mac qualification is running:

1. Review topics with focused checks, integrate compatible ready heads, then batch broad Linux
   validation once per useful integrated checkpoint. Do not rerun cold workspace builds for
   every child. An understood localized failure has a named fixup owner and affected-chain hold;
   it does not freeze unrelated development. Unknown foundation or contract errors hold their
   dependents until diagnosed and repaired.
2. Record the **development head** separately from each **qualification SHA**. Select an immutable
   commit for native qualification and keep that run on it while development advances. Record
   OS/architecture, exact Rust/Cargo and engine/ICU/esbuild versions, target/profile/features,
   commands, lock/input/binary/artifact hashes, log or run/job IDs, first results and retries.
   Unknown evidence stays unknown; old-SHA success never becomes current-head acceptance.
3. On a low-memory Mac, keep builds command-scoped and serial (`CARGO_BUILD_JOBS=1 cargo …`),
   with the existing heavy guard and reusable caches keyed by toolchain/target/profile/features
   and relevant flags. Record cache identity; rerun Cargo against the selected source, never
   infer binary identity from a cache hit. Do not change global job settings or weaken tests.
   Compilation timeout means **build incomplete / tests not run**, not behavioral failure or pass.
   Test timeouts retain their first logs and classification; skipped tests remain skipped.
4. When release preparation is authorized, freeze a candidate SHA. Take only relevant reviewed
   fixes onto that candidate, record each replacement SHA and rerun affected qualification;
   later topics continue on the development branch. Do not cancel/restart qualification merely
   to chase every development commit. Sharding can duplicate cold compilation: measure total
   cost and time, never promise a fourfold speedup from four shards.
5. Reconcile evidence before declaring qualification complete. Release/publication, production
   pushes, default-runtime switches and live contract application require separate approval and
   their existing gates. Never use the release workflow as a test shortcut: its dispatch
   `dry_run` defaults to false.

Use the [development queue and evidence record](research/development-queue.md) for the current
checkpoint, qualification owners, holds and next ready topics. The record is a ledger, not a
second scheduler; use existing workflows/scripts and preserve original receipts.

### Platform-bound verification on GitHub-hosted runners

- Prefer the existing workflows on standard GitHub-hosted runners for Linux-native builds,
  tests, and measurements (`health.yml`, `island-size-darwin.yml` with `platform=linux-x64`).
  A Mac-only local session is not a reason to ask the owner for WSL work or to add emulation.
- Run OS/CPU-specific acceptance on a matching runner. Keep a replay of saved artifact bytes
  distinct from a native measurement or native execution, and label an emulated replay as such.
- Record the exact source SHA (and merge ref for PR runs), runner image and toolchain,
  run/job/artifact IDs, artifact digest, and the first result plus every retry.
- Standard GitHub-hosted runners are free for this public repository
  ([billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions)).
  Do not use larger runners or other billed compute without owner approval. Avoid duplicate
  dispatches and purposeless reruns, and keep artifact size and retention to what the evidence needs.
- Using Actions never implies approval for deployment, release, or broader permissions.

### Codex Cloud disk capacity and broad Rust verification

The saved Codex Cloud environment used for [PR 4315](https://github.com/Takazudo/zudo-front-builder/pull/4315#issuecomment-6266482957)
exhausted its observed **32 GB filesystem** during full Rust compilation. This is an observed
host limit, not a guarantee about every Cloud environment or a universal build-size minimum.
Before a full workspace build, inspect available disk and estimate the build/target footprint
for the selected profile, features and existing caches. Serialize heavy local work through the
existing guard; memory admission alone does not establish disk capacity.

Preserve the first resource failure and its logs. A build that fails before test execution means
**zero tests executed**, not failed test assertions or passing coverage. Do not repeat the same
resource-exhausting build unchanged. Reclaim only your own disposable outputs; preserve other
owners' work, caches and evidence. Continue independent development and focused checks while
the manager tracks broad verification, subject to the affected-chain holds above.

Use existing supported standard Linux GitHub Actions for broad verification where suitable and
authorized. At the linked receipt, the separate security owner had only proposed a Linux-only
Rust verification option. It was later implemented in draft (unmerged) [PR 4317](https://github.com/Takazudo/zudo-front-builder/pull/4317)
and executed for the PR 4315 source only (see `research/development-queue.md`); that result does
not verify any other SHA. Use the option only where its review and authorized availability hold. Existing Linux size measurement
is not a Rust test lane, and a workflow that also dispatches Mac is not a Linux-only substitute.
Record source/control SHAs and first run evidence; never bypass admission gates or call skipped,
unbuilt or deferred tests passing. The [security issue record](https://github.com/Takazudo/zudo-front-builder/issues/4314#issuecomment-6266488465)
retains the capacity limitation separately from the unresolved lock-admission failure.

Keep toolchain selection process-local: `rust-toolchain.toml` selects stable in the checkout,
and a recorded qualification toolchain is named on the command (`cargo +1.99.1 ...` or
`RUSTUP_TOOLCHAIN=...`). Never run `rustup default`, `rustup override set` or other
machine-wide toolchain changes; other sessions and worktrees share them. `python3` must be
3.11+ because the `tests/unit/*.sh` suite (b4push, `health.yml`) runs
`scripts/capture-nextest-evidence.py`, which imports `tomllib`; on a Mac whose system
`python3` is older, prefix that command's `PATH` with a 3.11+ interpreter's directory. The contributor-facing
version of these rules is in [BUILDING.md](./BUILDING.md#selecting-a-toolchain-per-command).

## Worktree push policy (enforced)

This repo uses `/x-wt-teams` for multi-topic development. Child agents work in git worktrees under `worktrees/`. **Pushing from a worktree is forbidden.** Only the manager session — running from the main repo at the repo root — pushes, after merging topic branches into the base branch locally.

### Why

- CI runs on every push. Children pushing pre-empt the manager's merge + review step, multiplying CI cost across intermediate state.
- Topic branches in `worktrees/*/` are intermediate by design — they shouldn't appear as standalone PRs unless the manager creates them.

### How it's enforced

`.git/hooks/pre-push` is a direct script (not managed via `lefthook.yml`) that blocks any push from a git worktree. It is auto-installed by `pnpm install` (via the `prepare` lifecycle script) and can be re-installed manually with:

```sh
pnpm init-worktree
```

The installer source lives at `scripts/install-git-hooks.sh`; the hook itself at `scripts/hooks/pre-push`.

### Emergency bypass (human use)

```sh
ALLOW_WORKTREE_PUSH=1 git push ...
```

Use only when you genuinely need to push from a worktree (rare). Never set this in agent prompts.

### Guidance for agents

- **Child agents working in `worktrees/*/`:** commit locally only. Pushing will fail with the message above — do not retry, do not invoke the bypass. Report back to the manager with the branch name and commit SHAs; the manager merges and pushes from the main repo.
- **`/x-wt-teams` manager session:** the hook does not affect you. Your `git push` runs from the main repo (the cwd is the repo root, not `worktrees/...`). After every wave's local merges, push as usual. Do not pass `ALLOW_WORKTREE_PUSH` to children.
- **Remove each topic worktree right after its wave merges** (`git worktree remove worktrees/<topic>` once the merge is pushed and reviews are settled — re-verify the worktree is clean first). Every worktree accumulates its own `target/` (6–30G on this Rust workspace); leaving three merged worktrees around filled the disk mid-epic during #1670 (2026-07). Do not delete a worktree with uncommitted changes — surface it instead.

## npm dist-tags: `latest` always, `next` only while it is ahead

zfb publishes 11 packages in lockstep (5 platform + `@takazudo/zfb-slugify`,
`@takazudo/zfb`, `zfb-runtime`, `zfb-adapter-cloudflare`, `create-zfb`, `zfb-md-wasm`).
The rule for their dist-tags:

**A dist-tag is a promise to keep moving it. Only two states are safe — a tag that
always advances, or no tag at all.** A *frozen* tag is worse than a missing one:
it still resolves, so tooling reads it as a live channel and silently installs old
code, where a missing tag fails loudly.

- **`latest`** — maintained by npm on publish, plus `scripts/advance-latest-dist-tag.sh`
  during the prerelease phase (the `ALSO_LATEST` gate in `release.yml`, #481).
- **`next`** — NOT a standing channel. It exists only while a prerelease is
  *strictly ahead* of `latest`. `scripts/retire-next-dist-tag.mjs` runs from
  `release.yml` on every publish and removes `next` wherever it is no longer
  ahead. A genuine soak (`next=3.0.0-next.1` vs `latest=2.7.2`) survives a stable
  patch release; a graduated one is deleted. The script always exits 0 — a
  leftover dist-tag must never redden an otherwise good release, and the
  invariant self-heals on the next one.

This exists because it already went wrong once: zfb graduated at `1.0.0`, shipped
`1.1.0-next.1` on 2026-07-31, and never touched `next` again — so `@takazudo/zfb@next`
served a version a full major behind while `latest` walked to 2.7.1, confusing
consumers and dependency-bumping tooling. `release.yml` had only the forward half
of the invariant (advance `latest` during prerelease); the retirement script is the
missing reverse half.

**When adding a publishable package**, add it to BOTH `advance-latest-dist-tag.sh`
and `PUBLISHED_PACKAGES` in `retire-next-dist-tag.mjs`. The drift guard in
`scripts/__tests__/retire-next-dist-tag.test.mjs` fails the T1 gate if the two
lists disagree with each other or with the workspace's set of non-private packages.

`drift-net.yml` deliberately smokes **`latest` only**. A scheduled leg on `next`
would go red every week whenever the tag is correctly absent.

## Five-lane release changelog contract

All eleven published npm packages keep one lockstep version and the existing single GitHub Release,
tag, binary/npm publication, and Homebrew topology. Each future release nevertheless authors
exactly five default-locale-only English MDX notes:

- `docs/src/content/docs/changelog/zfb/v<version>.mdx` owns the Rust engine/CLI,
  `@takazudo/zfb`, `@takazudo/zfb-slugify`, and native carrier packaging.
- `docs/src/content/docs/changelog/zfb-runtime/v<version>.mdx` owns browser/runtime behavior and API.
- `docs/src/content/docs/changelog/zfb-adapter-cloudflare/v<version>.mdx` owns Cloudflare adapter
  behavior and API.
- `docs/src/content/docs/changelog/create-zfb/v<version>.mdx` owns the generator CLI and generated
  project behavior.
- `docs/src/content/docs/changelog/zfb-md-wasm/v<version>.mdx` owns the MD/WASM package, entries,
  API, artifacts, and package behavior.

Duplicate a cross-package change into every affected note. Omit repo-only docs/tests/CI/maintenance
with no package-facing effect. Every lockstep lane still receives a dated page; an unchanged lane
must use exactly `- No package-specific changes.` and must not borrow another package's narrative.

Shared historical lockstep notes from v0.1.0-next.5 through v2.10.0 belong to the `zfb` lane. Future
notes belong to their package lane; do not claim the initially empty runtime, adapter, create, or
MD/WASM lanes contain shared-history versions.

Compute `sidebar_position` independently for each lane as if the target page were absent. The
`scripts/next-changelog-sidebar-position.mjs` helper scans only that directory's non-index `v*.mdx`
pages, validates their positions, and adds one to its maximum; never use the retired root
`docs/src/content/docs/changelog/v*.mdx` path or a global maximum. The migrated `zfb` lane continues
after its historical maximum. An empty lane starts at position 1, and its first-ever page must set
`pagination_next: null` so pager traversal cannot cross into another package lane.

A `zfb-md-wasm` note must never call shipped artifacts "unchanged" without saying what that covers.
`ZFB_RELEASE_VERSION` is stamped into each `.wasm`, so every release moves all four SHA-256 digests
even when the compiled code and the byte sizes do not (#2885). Every file carrying a shipped-size
table must keep the digest disclaimer that `scripts/assert-md-wasm-size-docs.mjs` asserts.

Never refresh `shipped-sizes.json` (or its synced doc tables) from a Mac build — Mac and CI codegen
produce different `finalWasm`/`gzip9` bytes for the same pin, and the manifest holds CI-measured
values only. See `crates/zfb-md-wasm/LOCAL-VS-CI-SIZES.md` for the current gap and the local workflow.

The GitHub Release body has five explicit package headings and independently extracts the body of
the matching MDX source beneath each heading. Never reuse one lane's extracted notes for another.
Before the direct release push, run the package/Rust focused checks plus docs check, strict docs
build, and emitted-HTML validation. The executable release procedure lives in
`.claude/skills/l-make-release/SKILL.md`.

## Testing

zfb follows the **zudo-test-wisdom** strategy (the full guide: <https://takazudomodular.com/pj/zudo-test>). This section is the zfb-adapted, agent-facing summary — read it before writing or fixing tests. Every test sits on **two axes**:

- **Level** = *what a test can see* (logic → DOM → build output → browser → pixels).
- **Tier** = *where and when it runs* (focused inner loop → CI feedback → scheduled → manager heavy lane).

The axes are independent. "Too heavy for the PR gate" is a **tier** question, never a reason to rewrite a test at a lower **level**.

**Scoped context — read `crates/CLAUDE.md` before: adding, removing, or reclassifying ANY `#[ignore]` attribute; editing exam.yml / health.yml test wiring; or editing `.config/nextest.toml` test-groups.** That file is the authoritative home of the Rust `#[ignore]` manifest (every ignored test's identity + scheduled home), the 5-prefix taxonomy, its maintenance rules and re-measure grep pipelines, the nextest `e2e-heavy-*` test-group registration notes, and the provenance ledger. (`docs/CLAUDE.md` separately covers the docs site.)

### Test levels (escalation ladder)

| Level | What it sees | In zfb |
|---|---|---|
| 1 — Unit/logic | pure functions, transforms | `cargo test` unit tests; vitest unit tests |
| 2 — DOM component | DOM structure, no CSS engine | `zfb-runtime` happy-dom vitest (the client router) |
| 3 — Build output | emitted files **and emitted code that runs** | reading built `dist/`; the *dynamic-import-the-emitted-`_worker.js`* pattern in `zfb-adapter-cloudflare` (zfb is a code generator — string-equality proves the template was written, executing the artifact proves it threads `env`/`ctx`) |
| 4 — E2E browser | real process / browser | `crates/zfb/tests/dev_serve_e2e.rs` (dev server, Rust process-level e2e); `pnpm test:router-chromium` (`tests/router-chromium/`, real Chromium via Playwright — CI-gated by `.github/workflows/router-chromium.yml`, path-filtered to `packages/zfb-runtime/**` + `packages/zfb/src/runtime.ts` (the islands runtime — #1399) so it is NOT a required check); `pnpm test:webkit-back` (`tests/webkit-back-history/`, real WebKit back/forward-cache nav via Playwright — T4 local-heavy, Mac only, never runs in CI; see the release-day hook in RELEASE_DAY_CHECKLIST.md); `pnpm test:webkit-token` (`tests/webkit-token/`, the island build-token specs `sdk-persist` and `r-a08-mismatch` re-run unchanged in WebKit via `ZFB_BROWSER_ENGINE=webkit`; same T4 Mac-only status, exact selected counts enforced, 0 selected is a failure; #4231 item 2) |
| 5 — Visual | computed styles + pixels | the docs site / any UI — use `/verify-ui` + `/headless-browser` |
| 6 — AI-based | final resort, **not for CI** | not used; only for canvas/zoom surfaces L4+L5 cannot reach |

**Escalation rule:** when a test passes but the problem persists, **escalate to the next level — do not re-run the same test, and never tell the user to "clear cache."** If it's still broken, the code is still broken. Any CSS/layout/visibility work defaults to **Level 5**.

### Execution tiers

| Tier | What it is in zfb | Status |
|---|---|---|
| **T0 — inner loop** | Inexpensive affected unit/type/syntax/format checks. Use scoped Rust commands only when they avoid a heavy rebuild; broad workspace checks belong to manager/CI verification. Retries 0. | focused child checks, not repeated heavy suites |
| **T1 — CI feedback** | `.github/workflows/health.yml` — fmt, `clippy -D warnings`, all-target build, the workspace-resolved esbuild-only `--run-ignored ignored-only` env-gate lane, `cargo nextest run --workspace --profile ci` + a separate `cargo test --workspace --doc`, `pnpm test:workspace`, `format:check`, and build-no-v8. Utility CSS now runs through zudo-wind in-process and has no separate binary lane. The standalone `.github/workflows/actionlint.yml` check is path-filtered to workflow changes and is non-required, and a red check still requires manager triage under the development strategy above. `docs-checks.yml` also runs on every PR (#2827): its `changed-files (docs)` detector job-level-gates the strict docs build (`Build docs site (docs-checks.yml)`), and the always-reporting `Docs gate` job — required in the ruleset — is green when that build passed or was legitimately skipped and red when the detector or the build failed or was cancelled. The six-leg `wasm-md` matrix and packed-artifact browser check are optional job-level path-gated jobs; they run when the changed-files detector matches their md-wasm/browser closure. | Authoritative integration evidence and regression alerts; development merges proceed without waiting for green CI under the strategy above |
| **T3 — scheduled re-exam** | heavy/platform-bound lanes on a schedule. **Thin pre-release lane is LIVE; the full nightly upgrade is deferred** (see the T3 cutover manifest). `exam.yml` (#1344, weekly Sat 04:17 UTC) runs the `#[ignore]`d env-gate + heavy manifest allowed-to-fail, plus a full nextest + doctest re-exam on macOS (the FSEvents-vs-inotify gap ubuntu-only health.yml can't cover). `drift-net.yml` (#1342, weekly Wed 03:43 UTC) re-runs the release clean-room smoke against the live `latest` dist-tag on ubuntu + macos-15 and runs a provenance-drift leg. `security-audit.yml` (weekly Mon 07:17 UTC, #1394) covers `pnpm audit (prod)` + `cargo deny`. `yaml-candidate-watch.yml` (weekly Thu 05:29 UTC, `29 5 * * 4`) re-checks the YAML candidate drift baseline across all branches with role-aware classification: triage-severity deltas on the adopted noyalib/noyalib-serde-yaml pair (crates.io publish/yank/unyank, tag, Release, release-PR, archive) exit 10 as `CANDIDATE_DRIFT` and file the deduped tracking issue via `scripts/file-exam-issue.sh`; candidate-role deltas, branch-only movement, and crates.io record-touched tripwires are `informational-drift`, exit 0 with a `::notice::`, and close/keep closed that issue like `no-drift`. It is schedule + `workflow_dispatch` only, so it gates no PR and is deliberately not in ruleset `18452968`. `supervisor-watch.yml` (weekly Sun 05:37 UTC, `37 5 * * 0`) harvests the week's `health.yml` `[supervisor-timeline]` records — all branches for R-A/R-B, `main` only for strict identity drift — and files/closes `[exam-failure] Supervisor watch is failing` via `scripts/file-exam-issue.sh`; a week in which nothing was harvestable (no runs, or only runs red before the test step) is neutral, while a green `health` job with zero records is red — on `main` even one such job beside emitting ones, and even when the window is also incomplete — and a window it cannot prove complete (run cap, failed/malformed/non-progressing run-list page including the first, rate limit, request budget, a run still in progress, or a cancelled started job whose log answers 404) is `incomplete`, which files and alerts like red (the IFTTT payload names the verdict) and never closes the issue (#3230); its job timeout is 30 minutes, sized from a measured 7m26s full-window harvest (452 runs, 890 requests); schedule + `workflow_dispatch` only, not in ruleset `18452968`. None of the five gates a PR (schedule + `workflow_dispatch` only); all file/close one deduped tracking issue per workflow (`scripts/file-exam-issue.sh`) and IFTTT-notify on failure | live (thin) |
| **T4 — local heavy lane** | `pnpm b4push` (below) | convenience, not enforcement |

**Do not scaffold unused tiers.** zfb needs T0 + T1 now; T4 is `b4push`; T3 has the thin weekly stand-in — the full nightly upgrade stays documented-but-deferred until cutover (see the manifest below).

### Existing branch ruleset on `main`

GitHub separately enforces required checks through a **ruleset** on `main` (id `18452968`, `main-required-status-checks`; created via `scripts/apply-main-ruleset.sh`, checked in for reproducibility — rerun it to recreate/update). It requires only the checks that report on **every** PR (no path filters, no tag/workflow_run-only triggers; on a docs-only PR most of them report a job-level skip, which the ruleset reads as success — see "Docs-only pull requests" below): `health`, `build (no-v8)`, `Build binary (x86_64-unknown-linux-gnu)`, `Build binary (aarch64-unknown-linux-gnu)` (the job's `name:` is keyed on `matrix.platform.target` so each leg has a unique check context), the 4 `Smoke * (local mode)` jobs, `Scaffold E2E (packed tarballs, pre-publish)` (#1345, unconditional on every PR), `pnpm audit (prod)`, and `Docs gate` (#2827). The docs build itself — `Build docs site (docs-checks.yml)` — stays excluded: #1336 made it docs-only and #2827 moved that gate from the workflow trigger to a job-level `changed-files (docs)` detector, so it legitimately does not run on non-docs PRs and a required check that stops running would hang them forever; `Docs gate` is the unconditional job (`if: always()`, fails closed on a failed/cancelled detector or build) that turns its pass/skip/fail into a context safe to require. The two create-zfb showcase jobs (`Showcase deploy (create-zfb, production)`, `Showcase preview (create-zfb, PR)`, #2279) are excluded for the same class of reason — see "The create-zfb showcase" below. A `RepositoryRole` (`admin`, `actor_id: 5`) bypass actor with `bypass_mode: always` keeps `/l-make-release`'s direct version-bump push to `main` working (`required_status_checks` also blocks direct pushes). The bypass actor exists for separately authorized release procedures; routine development does not authorize using it. Respect protection and keep development on the approved integration branch. This document does not change the ruleset or the release procedure.

**`base/**` epic-PRs run T1 CI too (#2076).** `health.yml` and `pr-checks.yml`'s `on.pull_request.branches` include `"base/**"` alongside `main`, so an `/x-wt-teams` epic-PR (which targets a super-epic `base/**` branch) triggers `health` + the required `pnpm audit (prod)` during its own review window. Deliberately **`pull_request`-only** — `health.yml`'s `push:` trigger stays main-only (a `base/**` push trigger would double-run the suite per commit), and `pr-checks.yml` carries no `push:` trigger. These runs provide regression alerts during integration; they are not a per-epic wait-for-green barrier. The manager batches pushes and carries failures into fixing rounds under the development strategy above. The `main` ruleset is unaffected — it targets only `refs/heads/main`.

### Docs-only pull requests (#4372)

A PR into `main` or `base/**` whose every changed file is prose listed in `.github/path-filters/docs-only.yml` (`docs/src/content/**`, `docs/*.md`, root and `crates/*.md`, `crates/*/README.md`, `packages/*/README.md`, `prototypes/**/*.md`, `.claude/**/*.md`) skips the heavy PR lanes: `health`, `build (no-v8)`, the `wasm-md` jobs, the binary smokes, `Scaffold E2E` and the built-site browser smoke. `health.yml`, `node-free-smoke.yml` and `docs-checks.yml` each run the same detector: `dorny/paths-filter` with `predicate-quantifier: every` over that file, plus `.github/path-filters/docs-test-inputs.yml` for the few prose files a skipped job still reads, both checked out from the PR's base commit so a PR cannot widen its own list.

- **Required contexts still resolve.** A job skipped by its own `if:` reports success to the ruleset. The matrix `Build binary (<target>)` legs are the exception: a job-level skip happens before matrix expansion and would leave both per-target contexts pending, so those legs keep running with every step skipped.
- **Fail closed.** Only an explicit docs-only result skips anything. A failed detector, an empty listing or one at the PR-files API's 3000-file cap, a base without the filter files, any other base branch, a `push` to `main` and `workflow_dispatch` all run everything, so `main` keeps full post-merge verification (including Wind full mode).
- **The JS/format subset still runs.** Scripts and docs tests read Markdown, so a docs-only PR runs health's typecheck, `test:workspace`, `format:check`, Wind contract, md-wasm size-doc, SSE-literal and shell unit checks in `docs-checks.yml`'s `Docs-only checks (health JS subset)`, and the required `Docs gate` fails unless it passed. `scripts/__tests__/docs-only-ci-gate.test.mjs` keeps those commands in step with `health.yml` and the three detectors identical.
- **Keep test inputs out of the list.** Markdown that a test or the binary reads stays non-docs: crate fixtures, `crates/zfb/templates/**`, `research/**` (`research/3242-zudo-wind-v1-spec.md` is an `include_str!` input) and `docs/` outside `src/content`. A single prose page that a heavy job reads goes in `docs-test-inputs.yml` (today `mdx-components.mdx`, built by `tests/sdk-alias-consumer`).

### The create-zfb showcase (#2279)

`create-zfb` is otherwise invisible — the only way to see what `npm create zfb@latest my-site` produces is to run it. Two jobs in `node-free-smoke.yml` fix that by deploying the **real** scaffold output to an assets-only Cloudflare Worker (`zfb-showcase`) at **`create-zfb.takazudomodular.com`**. Config lives in `create-zfb-showcase/` (deploy config only — no `package.json`, deliberately not a pnpm workspace member).

**The site is never committed.** `scaffold-e2e` already scaffolds and builds a real site via `scripts/smoke-packed-clean-room.sh`; an opt-in `ZFB_SMOKE_DIST_OUT` makes it export that `dist/` as the `create-zfb-showcase-dist` artifact, which both showcase jobs consume. One scaffold+build, one oracle, two consumers — so the deployed site cannot drift from `crates/zfb/templates/`. This is a **share, not a refactor**: `Scaffold E2E (packed tarballs, pre-publish)` keeps its exact name, every existing step, and its required-check identity; the appended upload step carries `continue-on-error: true` so a showcase-side hiccup can never redden that required check.

| Surface | Tier | Blocking? |
|---|---|---|
| `scaffold-e2e` + its artifact export | T1 PR gate, unchanged | Required (already) |
| `html-validate` over the scaffold's emitted HTML | L3 build-output, inside the non-required showcase jobs | Blocks the deploy; requires manager triage and repair, not an automatic halt to unrelated development |
| `Showcase preview (create-zfb, PR)` | T1-adjacent, informational | **Not required** |
| `Showcase deploy (create-zfb, production)` | **Alerting guard** — runs post-merge, so it detects a bad deploy and cannot prevent one | **Not required** |

Neither showcase job is in ruleset `18452968`, and neither should be added: the preview is skipped entirely on fork, dependabot and draft PRs (no secrets, read-only token; drafts must not upload externally), so as a required check it would hang those PRs forever — the same failure mode that kept `Build docs site` out until #2827 wrapped it in the unconditional `Docs gate`.

Three constraints that are load-bearing, not stylistic:

- **`cancel-in-progress` is conditional on `pull_request`.** A job-level `concurrency` block does **not** shield a job from a workflow-level cancel — the whole run dies, and group membership in one group confers nothing in another. Without the condition, two quick `main` pushes kill a deploy mid-`wrangler deploy`, skipping the domain attach, health check, and notify.
- **`html-validate` runs *before* the banner injection**, in both jobs. Reversed, the L3 signal would judge CI's injected markup instead of zfb's emitted HTML.
- **The banner is injected at deploy time** by `scripts/showcase-inject-banner.mjs`, never committed into `crates/zfb/templates/**` — that is what keeps the "this is exactly what you get" claim honest. It is a plain string splice with no HTML parser (a parse/serialize round-trip would normalize markup and perturb `element-permitted-content`).

**The root corpus is split into two vitest projects** by the `test.projects` list in the root `vitest.config.mjs` (#3058/#3061; the Vitest 2 `vitest.workspace.mjs` file was folded in by #3557): `scripts` keeps vitest's 5000 ms `testTimeout` as a hang guardrail, and `scripts-subprocess` raises it to 90 s for the five suites that `await` real child processes — measured, not guessed. A suite belongs there only if it *awaits* a subprocess. Under Vitest 2 an `execFileSync`-blocking test could not be interrupted by `testTimeout` at any host load; Vitest 4+ also fails any test whose elapsed time reached its timeout once it returns, so the 5 s now bounds those synchronous suites too. Add such a suite to the single `SUBPROCESS_SUITES` list in that file and nowhere else — `scripts-subprocess` includes exactly that list and `scripts` excludes it, so the two projects cannot overlap and run a suite twice. The projects are inline with `extends: false`, so they do not inherit the root config and no array is concatenated into them.

Root-level vitest suites (`scripts/**/__tests__`, configured by the root `vitest.config.mjs`) only run because both `health.yml` and `run-b4push.sh` invoke the canonical `pnpm test:workspace` script, which passes **`--include-workspace-root`**. A plain recursive pnpm test invocation skips the workspace root, so without that flag a root-level suite exists and silently never runs. Keep both surfaces on the canonical script — they must stay identical or the local gate and the PR gate drift.

**Also note:** the showcase deploy jobs hardcode `wrangler@4.85.0` rather than reading it from `docs/package.json` the way the docs workflows do. That is deliberate: `npx wrangler@<string read from a file the PR can edit>` is arbitrary code execution next to the production Cloudflare token, because npm specifiers accept `file:`, `github:`, and remote-tarball forms — not just versions. Keep it in sync with `docs/package.json` by hand when bumping wrangler.

### Required behavior (agents)

1. **Declare the test plan first** — *what* you're testing, *which level*, *why* that level.
2. **Match level to goal** — don't verify a Level-5 visual bug with a Level-1 logic test.
3. **Escalate when a lower level passes but the problem persists** (never re-run, never "clear cache").
4. **Default to Level 5 for any UI/CSS/visibility work.**
5. **Report what was NOT tested** — state the blind spots.
6. **Verification specs don't self-graduate.** A one-time "it was done" proof is tagged `#[ignore = "verification: <why>"]` (Rust) / `@verification` (TS) and excluded from gates. Propose promotion to a tier in the PR description; never self-promote.
7. **Red checks require manager triage and a fixing round.** This includes non-required checks and failures after merge. Follow the development strategy above: the manager chooses whether to continue independent work or pause the affected chain. Keep failures tracked and never report unresolved verification as passed. Existing quarantine tags require their linked issue and evidence.
8. **Never game the gate.** Never weaken a gate without a linked open issue; require fresh-context review for assertion changes made to pass the gate and all edits to gate-defining files; and treat runner timeouts and liveness deadlines as guardrails, not assertions, raising them only with a linked open issue and a value derived either from a measured latency distribution (baseline versus under load) or from the child deadline the budget must contain.
9. **Manager-owned heavy verification.** When a change is covered only by a heavy/quarantined lane, the manager schedules that coverage on a capable host as part of integrated verification. Child topics do not each run the heavy lane. Required coverage must pass before the overall work is declared verified.

### Flaky tests

zfb hits flakiness often (mostly **Rust timing/ordering** in the dev-server, SSE, and plugin-runner paths). Handle it as a pipeline, not a shrug.

- **Retry budget:** local **0**; CI **1–2** with artifacts. **Pass-on-retry is a triage signal, not a success** — record it and schedule the fix. **More than 2 retries is a smell.**
- **The `flaky:` quarantine pipeline (Rust):**
  - **Step 0 — prove it ever genuinely passed** (not pass-by-skip) on some host. A test that can pass *nowhere* is **broken, not flaky** → fix or delete it now, no quarantine.
  - **Quarantine requires a paper trail** — mark it `#[ignore = "flaky: <issue-url>"]` (the inline issue URL is mandatory) and add its manifest row in `crates/CLAUDE.md`. `cargo test` skips `#[ignore]`d tests, so it no longer blocks the gate.
  - **It must still run somewhere, allowed-to-fail** — locally via `cargo test -- --ignored` (or `--include-ignored`), AND weekly in CI: `exam.yml`'s `quarantine-heavy` job runs the env-gate + heavy manifest subset (`--run-ignored ignored-only`, exact-name filterset; `pending-feature`-tagged tests are deliberately excluded — they're blocked on unimplemented features, not flakiness) allowed-to-fail, filing/closing a deduped tracking issue.
  - **Quarantine has an exit with a deadline — fix, demote, or delete.** It is not a parking lot. **Quarantine suspends *product* coverage, not just test coverage:** the behavior is unguarded until the test is fixed.
- **The 5 deflaking root causes** (fix the cause, don't add a sleep): bare timing waits, `networkidle` on no-request SPA navs, animations in flight, shared/order-coupled state, hydration races. zfb's Rust flakes are mostly timing/ordering — prefer event/condition-keyed waits and deterministic scheduling over fixed `sleep`s (as `zfb-test-utils`'s SSE helpers and the `plugin_runner` timeouts already do).
- **`cargo-nextest` (ADOPTED — #1340):** CI runs the Rust suite under `cargo nextest run --workspace --profile ci` (health.yml), configured in `.config/nextest.toml`. The `ci` profile RECORDS retries (`retries = 1`, pass-on-retry reported as `FLAKY` + the `nextest-junit-ci` artifact; #1341 consumes that telemetry) — do not raise `retries` to paper over a flake; local runs use the default profile (`retries = 0`) so flakes fail loudly. **Doctests are NOT run by nextest** — health.yml keeps a separate `cargo test --workspace --doc` step. The doctest baseline, the two `e2e-heavy-*` groups (25 flock-adopting binaries in `e2e-heavy-locked` and 18 build-only binaries in `e2e-heavy-unlocked`, each `max-threads = 1`), the required-features lane, and the inventory-parity reconciliation rule all live in `crates/CLAUDE.md`.
- **`scripts/__tests__/docs-dev-supervisor.test.mjs` phase telemetry (#2889/#2904):** opt-in `[supervisor-timeline]` lines behind `ZFB_SUPERVISOR_TIMELINE=1`, off by default (b4push does not set it — extra stderr noise the local loop didn't ask for). `health.yml` now sets it on the `pnpm test:workspace` step so every ubuntu CI run contributes a sample. This buys observability, not storage — lines land scattered across workflow logs under GitHub's retention. `scripts/supervisor-timeline-summary.mjs` reads them and applies #2887's R-A/R-B/R-C rules; its doc comment is the protocol's source of truth. `scripts/harvest-supervisor-timelines.mjs` (#2908) is the aggregation step: it re-fetches each run's `health` job log via `gh` and re-parses it with the summarizer's own `parseTimelines` on demand, so nothing is persisted between runs; its `--save-dir` keeps the raw job logs, which the weekly watch folds into a single `job-logs.tar.gz` (both passes are done reading the plain directory by then, and the source logs are only deleted once the archive is verified — a broken archive aborts the script instead of shipping an artifact that has grown, #2934) and uploads as the artifact an R-A triage reads and reuses for its `main`-only pass. Since #2931 both per-run fetches are direct REST calls — `gh api repos/{owner}/{repo}/actions/runs/<id>/jobs?per_page=100` and `gh api repos/{owner}/{repo}/actions/jobs/<id>/logs`, one metered GET each (the log endpoint's 302 to a plaintext blob is a separate, unmetered host, and gh expands `{owner}`/`{repo}` locally). The `gh run view` porcelain it replaced cost ~7 GETs per run — measured with `GH_DEBUG=api` — which put a capped 200-run harvest near `GITHUB_TOKEN`'s 1,000-per-hour primary limit, where it would 403 mid-harvest and file a tracking issue about its own rate limit; the same harvest is now ~400. The REST body is not byte-identical to the porcelain's: it has no `<job>\t<step>\t` prefix and opens with a UTF-8 BOM, so `record.raw` and the `--save-dir` bytes changed while the parsed records did not. `scripts/__tests__/fixtures/rest-job-log-capture.log` is a trimmed real capture of that body (GitHub owns the envelope, so it is captured, not hand-authored — unlike `supervisor-timeline-samples.txt`, which is zfb's own emitter contract), and both suites build their synthetic logs from its prefix. The `env=` token is a steering-only digest over `STEERING_ENV_KEYS` (`scripts/supervisor-env-identity.mjs`, #2913), carried on the wire as `env=v<N>:sha256:<16 hex>` where `<N>` is `ENV_IDENTITY_VERSION` — the version of the *contract* that produced the digest (#2933). Two digests are comparable only under the same version, so changing `STEERING_ENV_KEYS`, its order, or the serialization is a version bump rather than a new date epoch; an unversioned `env=sha256:…` record predates the prefix and reads as **v1**, so a population straddling #2933 shows no drift. The summarizer groups each case's records into one cohort per version: **R-A (failed-record detection) stays global** across versions, while identity drift, R-B, and the four timing distributions are evaluated **per cohort and never pooled**. `--strict` fails when any cohort trips R-B or carries disallowed within-version drift; a version split is reported prominently but is **not** itself a strict failure, because it heals as the old cohort rolls out of the harvest window. `--allow-drift env` therefore means "tolerate two different steering digests under the *same* contract version" and says nothing across cohorts; it is not the remedy for a contract bump, and the retired `IDENTITY_CONTRACT_EPOCH` date floor no longer exists. The harvester defaults to a rolling 8-day window (one weekly cadence plus a day of overlap, so a run in progress at one harvest is still enumerated by the next). Since #3230 that window is a closed `[since, until]` interval with `until` fixed at start — by default the start time minus a 120-minute settle margin (the `health` job's 90-minute timeout plus 30 for queueing, printed as `settle-margin=` on the `window:` line), with `since` 8 days before it, so ordinary runs in it have finished and one still not `completed` is `unavailable=in-progress` — enumerated through the REST list-workflow-runs endpoint (`created=<since>..<until>`, 100 rows per page) to proven exhaustion — a short page, or a full page reaching `total_count` — and split into non-overlapping halves wherever a query reports more than GitHub's 1,000-results-per-search ceiling. One sample per run id, from its latest attempt; a run served twice is deduplicated. `--limit` (default 1,000) is a run cap and `--max-requests` (default 950, under `GITHUB_TOKEN`'s 1,000/hour) a finite budget of metered gh calls; a cap hit, budget stop, rate limit (a secondary limit or 429 first gets one shared pause per harvest: `Retry-After` when gh shows it, else 60 s, capped at 120 s), failed/malformed/non-progressing page — the first page included, which is exit 5, not a usage error — or a `total_count` disagreement (a partition whose `total_count` moves mid-walk is re-walked from page 1 once, deduplicating by run id, before `total-count-changed`) is an `incomplete:` reason, and a started `health` job that was cancelled and whose log answers 404 (gh prints either `(HTTP 404)` or, when the log's 302 blob target fails, a bare `gh: HTTP 404` plus BlobNotFound XML — both are matched, CRLF-tolerant, and likewise for 410) is `unavailable=cancelled-job-log-404` evidence — never a never-started skip, expiry, or silent job. Any of these makes the harvester exit 5 (incomplete window), with precedence 3 > 1 > 5 > 4 > 0 so a failed run or a silent green job is never masked by missing evidence; the runs it did collect are still emitted, so a failed record among them stays visible. The repository's 7-day Actions retention expires job logs before that window closes, so the overlap day's runs are enumerated but skipped as `log-expired` (HTTP 410), never a partial harvest; raising the retention to >= 9 days in the repo settings (UI-only; there is no REST endpoint) would restore the overlap's coverage. It prints the effective window and warns when `--since` lies in the future, keeps "nothing harvestable" (exit 4, complete windows only) apart from "green `health` jobs with zero records" (exit 1) and from an incomplete window (exit 5), and its per-run manifest lines name runs carrying `failedRecords=` records — a distinct counter from the final summary line's `failed=` (runs the harvester itself could not fetch/parse), renamed apart in #2932 so no `^run=` anchor is needed to tell them apart. Its gh-retry pause (2000 ms default) is overridable via the `HARVEST_RETRY_DELAY_MS` env var (test ergonomics only; `scripts/supervisor-watch.mjs` forwards its `WATCH_RETRY_DELAY_MS` setting, #2932); `supervisor-watch.yml` automates the R-A/R-B watch weekly. The job-log fetch adds `--allow-escape-sequences` when a one-time `gh api --help` capability probe (once per `runCli` invocation) says the installed gh (>= 2.97) advertises it, since a probe failure or an older gh falls back to the pre-2.97 behavior without it (#2986).

### Supervisor watch alert triage

Open the failing [Supervisor watch](https://github.com/Takazudo/zudo-front-builder/actions/workflows/supervisor-watch.yml) run. Its job summary shows the verdict and both summaries; download its `supervisor-watch` artifact for `all/summary.txt`, `all/manifest.txt`, `main/summary.txt`, and `main/runs.txt`. Pass A (`all/`) covers every branch and allows identity differences in that mixed population. Pass B (`main/`) reuses the saved logs for `main` push runs and checks identity strictly. The artifact also has `failed-runs.txt`, `harvest-errors.txt`, `silent-runs.txt`, `expired-runs.txt`, `unavailable-runs.txt`, `harvest-notices.txt`, `enumeration.txt`, and `job-logs.tar.gz`; each per-run manifest line identifies its run, attempt, branch, SHA, and job.

- **R-A:** `outcome=failed` or a nonempty `failed-runs.txt` needs the named run's diagnostic block in the saved job log. `expected-failure` is an intentional test outcome, not R-A.
- **R-B:** read each cohort's `R-B verdict` and measured max pre-UP against the 12,000 ms boundary (75% of the 16-second budget). Diagnose the timing distribution before proposing a budget change.
- **Identity drift:** compare `main/summary.txt` identity values with `main/runs.txt` and the emitting commits. For an intentional dependency or fixture change, check individual samples immediately before and after the commit for the old-to-new transition and unexpected reversions. A contract-version split is reported separately and is not itself strict drift. A post-change-only replay can diagnose the new state but cannot prove the rolling watch is green while older samples remain in its window.
- **Silent emitter or harvest failure:** inspect `silent-runs.txt` for green `health` jobs with zero records, and `harvest-errors.txt` plus `all/manifest.txt` for fetch or parse errors. A wholly unharvestable window is neutral `no-data`; one silent `main` job is red even beside emitting jobs. Expired logs appear in `expired-runs.txt` and are skipped, not counted as silent jobs.
- **Incomplete window (`verdict=incomplete`, #3230):** read `enumeration.txt` for the `[since, until]` bounds, every run-list page and partition, the `enumeration: complete=` line, and each `incomplete: reason=` line, and `unavailable-runs.txt` for runs whose evidence is missing (`cancelled-job-log-404` with the test step's outcome, `in-progress`, `rate-limited`, `not-attempted:<reason>`, `run-cap`); a `partition-restart:` line in `enumeration.txt` is a recovered `total_count` move, not a finding. The verdict is the absence of proof, not a finding about the supervisor; red evidence among the collected runs still reports `red`. Do not relabel missing post-emitter logs as never-started or expired. Main's strict pass reads the same incomplete population, so it cannot stand in for the missing samples. Request cost is about 2 metered calls per run (the 2026-10-04..12 window spent 890 of the 950 budget on 452 runs), so a busier week truthfully reports `budget-exhausted` / incomplete; that is a capacity signal, not a supervisor finding.

A same-contract-version identity change keeps strict `main` red while both values occur in harvestable samples. Old samples eventually leave the rolling window or their logs expire; the next full-window watch can then turn green if no other finding remains. Keep its source alert open for that genuine green watch to close; a later run can also reveal an independent failure, so inspect each new alert before attributing it to the same transition. Do not suppress drift or narrow the window to manufacture recovery.

### TS-side flaky/verification idiom (vitest)

No TypeScript/vitest test in this repo has ever needed quarantine — `grep -rn '// flaky:\|// @verification:' --include='*.test.ts' packages/ crates/*/npm` returns nothing (#1349 confirmed nothing to retrofit). The convention for the day one does, mirroring the Rust taxonomy — the reason lives in a comment directly above `it.skip(...)`:

```ts
// flaky: https://github.com/Takazudo/zudo-front-builder/issues/9999
it.skip("retries the SSE reconnect within the 3-attempt budget", async () => {
  // ...
});
```

- A one-time "it was done" proof uses `// @verification: <why>` above `it.skip(...)` instead (no issue URL needed). The other 3 Rust prefixes (`env-gate:`, `heavy:`, `pending-feature: <issue-url>`) follow the same comment-above-`it.skip(...)` shape if ever needed — no precedent yet.
- **Audit**: the grep above — the TS mirror of the Rust manifest's re-measure pipeline in `crates/CLAUDE.md`.
- Same rules as Rust apply: **never game the gate** (no `it.skip` for `flaky:` without a linked open issue), **quarantine has an exit with a deadline**, and every red check still requires manager triage under Required behavior rule 7; valid quarantine needs the linked evidence and issue. The first real TS quarantine/`@verification` adds a manifest table entry (mirroring the Rust manifest in `crates/CLAUDE.md`) so this section stops being purely aspirational.

### T3 cutover manifest (post-release)

What upgrades when zfb ships a stable release, each with an explicit trigger — do not implement any row speculatively; wait for its trigger, then treat this table (not the epic that wrote it) as the up-to-date starting point. The "do not scaffold unused tiers" rule applies to every row.

| Item | Current state | Trigger to upgrade |
|---|---|---|
| Weekly → nightly cadence | `exam.yml` (Sat 04:17 UTC) + `drift-net.yml` (Wed 03:43 UTC) run weekly on GitHub-hosted runners only | A stable release ships, or adoption makes a week of undetected drift unacceptable. Bump both crons to nightly; re-check the runner budget (public repo — no self-hosted runners planned by default) |
| Windows exam leg | `@takazudo/zfb-win32-x64-msvc` ships (`release.yml` builds it natively every release) but **no CI job executes the Rust test suite on Windows** — health/exam/drift-net are ubuntu/macOS only | A deliberate commitment to Windows as a *tested* platform (e.g. a Windows bug report). Add a `windows-latest` leg to `exam.yml` mirroring `macos-exam`; expect path-separator / file-locking friction in the watcher |
| wrangler/workerd adapter heavy lane | `packages/zfb-adapter-cloudflare`'s `d1-binding.test.ts` uses a synthetic in-memory `D1Database` stub (its header documents why) — it proves env/binding passthrough, not real D1 SQL semantics or the real Workers runtime lifecycle | The Cloudflare adapter becomes a primary deploy target, or a bug the stub provably cannot catch. Add a `wrangler dev`/miniflare-backed lane — almost certainly T3/T4, not T1 |
| Homebrew automation + verify | **Tap push is automated skill-side (v1.1.0, 2026-08-01).** `/l-make-release` Step 11 runs `scripts/update-homebrew-formula.sh vX.Y.Z --push` itself after the `release.yml` watch succeeds, stable releases only; it stays manual on the `--confirm` path, which stops before publishing. Two halves remain open: the push is **not** in `release.yml` (a Release published from the web UI still needs a manual tap update), and there is still **no `brew install` smoke test** | Move the push into `release.yml` when web-UI publishes become common — this needs a **new secret**, since the built-in `GITHUB_TOKEN` is scoped to this repo and cannot write to `Takazudo/homebrew-tap` (a PAT or GitHub App token with `contents: write` on the tap, plus `ZFB_TAP_REMOTE` pointed at an HTTPS URL). Add the `brew install` + `zfb --version` smoke on a macOS runner independently — it does not need the secret |
| linux-musl decision | `packages/zfb/bin/detect-musl.mjs` (wired into `zfb.mjs`) detects Alpine/musl and fails with a friendly "not supported yet" error; no musl package or `release.yml` leg exists | Real demand (an issue asking for Alpine/musl). Then decide: add `@takazudo/zfb-linux-x64-musl` (cross-compiled leg + `optionalDependencies` entry) or keep the friendly-error posture — a product decision, not just CI |
| vitest coverage reporting | No `vitest.config.*` in the workspace configures `coverage:` — `pnpm test:workspace` runs with zero coverage instrumentation | Coverage becomes a stated release-readiness gate. Add `@vitest/coverage-v8` + `coverage:` blocks per package, and decide on purpose whether thresholds gate T1 or stay T4/local-only |

### `b4push` — local pre-push pass (T4)

`pnpm b4push` (`scripts/run-b4push.sh`) runs a **bounded** fast pass before pushing, cheap → expensive: shell-script syntax (`bash -n`), the offline `tests/unit/*.sh` unit tests (actually executed — one step per file), `cargo fmt --check`, `pnpm format:check`, `node scripts/assert-md-wasm-size-docs.mjs`, the SSE endpoint literal guard, `pnpm typecheck:workspace`, `pnpm test:workspace`, and `cargo clippy -D warnings` (warm tree). It prints a per-step duration summary. It is **not** the authoritative gate — `health.yml` is. b4push is the *fast subset*; the full Rust test suite is too heavy for a pre-push loop by default (V8 first-compile is 15–30 min).

- Escapes: `B4PUSH_SKIP_CLIPPY=1`, `B4PUSH_SKIP_JS_TEST=1`.
- Heavy steps use the machine-wide `heavy-guard` queue when available: set `HEAVY_GUARD=/path/to/heavy-guard.sh` to choose it explicitly; otherwise b4push checks `$HOME/.claude/scripts/heavy-guard.sh` and then `$HOME/.codex/scripts/heavy-guard.sh`. CI and machines without a guard run the same commands directly. Exit 75 means the guard could not acquire the queue and the command did not run; b4push reports contention and exits 75 when no other check failed. Guard execution errors and command failures remain failures. Only full Rust suites, doctests, the syntax asset regeneration, and the env-gated integration/asset suites are queued; formatting, lint, typecheck, unit tests, no-V8 cargo check, and the generated-asset diff guard remain outside the queue.
- Heavy opt-in: `B4PUSH_FULL=1 pnpm b4push` additionally runs the **required health.yml Rust parity set** (#1332): `cargo nextest run --workspace` (or `cargo test --workspace` when nextest is absent) on nextest's **default** profile (`retries = 0`, unlike CI's `--profile ci`); `cargo test --workspace --doc` (nextest branch only); the `zfb-md-extras` `test-utils`-gated suite plus its scoped `cargo clippy`; `cargo check --no-default-features -p zfb --tests`; and the three esbuild env-gate commands for islands, cross-pipeline acceptance, and command-layer coverage. Those env-gate commands run only when the staged esbuild binary is available; run `cargo build --workspace --all-targets` first for the b4push subset. Utility CSS uses the in-process zudo-wind engine and needs no external CLI binary. The optional path-gated `wasm-md` matrix and browser job are not part of b4push.
- On heavy-guard `ENV_SUSPECT`, rerun that command once. If it is still red without an assertion, type, or lint error, defer that command to CI under a `deferred-verification` issue and report it as deferred, not passed. Fix real failures; do not weaken tests or tune the suite to fit one machine.
- It is **not** wired into the git `pre-push` hook (that hook only enforces the worktree-push policy above) — the manager runs it when useful for an integration/fixing round, not before every child-topic push or merge.
