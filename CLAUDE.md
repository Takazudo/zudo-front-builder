# zfb / zudo-front-builder

Workspace for the `zfb` Rust workspace + `@takazudo/zfb-runtime` and `zfb` npm packages.

## /x-wt-teams epic workflow rule

For each `[Epic]` issue in this repo (e.g. issues #52, #53, and the rest of the super-epic's zfb-side epics), the epic PR is independent and safe to merge into `main` as soon as its workflow completes — siblings do not stack on one base. So:

- **Always invoke `/x-wt-teams` with `-a` / `--auto`** for an epic in this repo (in addition to whatever other flags the user passes — `-gcoc`, `--stay`, etc.). `-a` runs `/pr-complete -c -w` after Step 15, which merges the root PR, closes the linked issue, and watches post-merge CI on `main`.
- **After the merge succeeds**, the workflow's auto-suggest step prints the next epic's `/x-wt-teams` command for the user to copy-paste. Do not start the next epic in the same session.
- If the user explicitly says "do NOT auto-merge" or passes a flag that conflicts with `-a`, defer to the user.
- This rule is specific to `[Epic]` issues. Non-epic issues (one-off bug fixes, follow-ups like #58 inside an open epic PR) keep the default behaviour — leave the PR open for the user to review.

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

zfb publishes 10 packages in lockstep (5 platform + `@takazudo/zfb`, `zfb-runtime`,
`zfb-adapter-cloudflare`, `create-zfb`, `zfb-md-wasm`). The rule for their dist-tags:

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

All ten published npm packages keep one lockstep version and the existing single GitHub Release,
tag, binary/npm publication, and Homebrew topology. Each future release nevertheless authors
exactly five default-locale-only English MDX notes:

- `docs/src/content/docs/changelog/zfb/v<version>.mdx` owns the Rust engine/CLI,
  `@takazudo/zfb`, and native carrier packaging.
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

The GitHub Release body has five explicit package headings and independently extracts the body of
the matching MDX source beneath each heading. Never reuse one lane's extracted notes for another.
Before the direct release push, run the package/Rust focused checks plus docs check, strict docs
build, and emitted-HTML validation. The executable release procedure lives in
`.claude/skills/l-make-release/SKILL.md`.

## Testing

zfb follows the **zudo-test-wisdom** strategy (the full guide: <https://takazudomodular.com/pj/zudo-test>). This section is the zfb-adapted, agent-facing summary — read it before writing or fixing tests. Every test sits on **two axes**:

- **Level** = *what a test can see* (logic → DOM → build output → browser → pixels).
- **Tier** = *where and when it runs* (inner loop → PR gate → scheduled → local heavy lane).

The axes are independent. "Too heavy for the PR gate" is a **tier** question, never a reason to rewrite a test at a lower **level**.

**Scoped context — read `crates/CLAUDE.md` before: adding, removing, or reclassifying ANY `#[ignore]` attribute; editing exam.yml / health.yml test wiring; or editing `.config/nextest.toml` test-groups.** That file is the authoritative home of the Rust `#[ignore]` manifest (every ignored test's identity + scheduled home), the 5-prefix taxonomy, its maintenance rules and re-measure grep pipelines, the nextest `e2e-heavy-*` test-group registration notes, and the provenance ledger. (`docs/CLAUDE.md` separately covers the docs site.)

### Test levels (escalation ladder)

| Level | What it sees | In zfb |
|---|---|---|
| 1 — Unit/logic | pure functions, transforms | `cargo test` unit tests; vitest unit tests |
| 2 — DOM component | DOM structure, no CSS engine | `zfb-runtime` happy-dom vitest (the client router) |
| 3 — Build output | emitted files **and emitted code that runs** | reading built `dist/`; the *dynamic-import-the-emitted-`_worker.js`* pattern in `zfb-adapter-cloudflare` (zfb is a code generator — string-equality proves the template was written, executing the artifact proves it threads `env`/`ctx`) |
| 4 — E2E browser | real process / browser | `crates/zfb/tests/dev_serve_e2e.rs` (dev server, Rust process-level e2e); `pnpm test:router-chromium` (`tests/router-chromium/`, real Chromium via Playwright — CI-gated by `.github/workflows/router-chromium.yml`, path-filtered to `packages/zfb-runtime/**` + `packages/zfb/src/runtime.ts` (the islands runtime — #1399) so it is NOT a required check); `pnpm test:webkit-back` (`tests/webkit-back-history/`, real WebKit back/forward-cache nav via Playwright — T4 local-heavy, Mac only, never runs in CI; see the release-day hook in RELEASE_DAY_CHECKLIST.md) |
| 5 — Visual | computed styles + pixels | the docs site / any UI — use `/verify-ui` + `/headless-browser` |
| 6 — AI-based | final resort, **not for CI** | not used; only for canvas/zoom surfaces L4+L5 cannot reach |

**Escalation rule:** when a test passes but the problem persists, **escalate to the next level — do not re-run the same test, and never tell the user to "clear cache."** If it's still broken, the code is still broken. Any CSS/layout/visibility work defaults to **Level 5**.

### Execution tiers

| Tier | What it is in zfb | Status |
|---|---|---|
| **T0 — inner loop** | `cargo check`/`clippy` + scoped `cargo test -p <crate>` (affected), `pnpm typecheck:workspace`, `pnpm test:workspace`. Retries 0. | run constantly while coding |
| **T1 — PR gate** | `.github/workflows/health.yml` — fmt, `clippy -D warnings`, all-target build, three workspace-resolved `--run-ignored ignored-only` env-gate lanes (esbuild-only, Tailwind-only, and both), `cargo nextest run --workspace --profile ci` + a separate `cargo test --workspace --doc`, `pnpm test:workspace`, `format:check`, and build-no-v8. The standalone `.github/workflows/actionlint.yml` check is path-filtered to workflow changes and is non-required, but a red check still blocks its author under the flaky/red-check policy. `docs-checks.yml` also runs on every PR (#2827): its `changed-files (docs)` detector job-level-gates the strict docs build (`Build docs site (docs-checks.yml)`), and the always-reporting `Docs gate` job — required in the ruleset — is green when that build passed or was legitimately skipped and red when the detector or the build failed or was cancelled. The six-leg `wasm-md` matrix and packed-artifact browser check are optional job-level path-gated jobs; they run when the changed-files detector matches their md-wasm/browser closure. | **the authoritative gate.** A PR is mergeable when its required checks are green |
| **T3 — scheduled re-exam** | heavy/platform-bound lanes on a schedule. **Thin pre-release lane is LIVE; the full nightly upgrade is deferred** (see the T3 cutover manifest). `exam.yml` (#1344, weekly Sat 04:17 UTC) runs the `#[ignore]`d env-gate + heavy manifest allowed-to-fail, plus a full nextest + doctest re-exam on macOS (the FSEvents-vs-inotify gap ubuntu-only health.yml can't cover). `drift-net.yml` (#1342, weekly Wed 03:43 UTC) re-runs the release clean-room smoke against the live `latest` dist-tag on ubuntu + macos-15 and runs a provenance-drift leg. `security-audit.yml` (weekly Mon 07:17 UTC, #1394) covers `pnpm audit (prod)` + `cargo deny`. `yaml-candidate-watch.yml` (weekly Thu 05:29 UTC, `29 5 * * 4`) re-checks the YAML candidate drift baseline across all branches with role-aware classification: triage-severity deltas on the adopted noyalib/noyalib-serde-yaml pair (crates.io publish/yank/unyank, tag, Release, release-PR, archive) exit 10 as `CANDIDATE_DRIFT` and file the deduped tracking issue via `scripts/file-exam-issue.sh`; candidate-role deltas, branch-only movement, and crates.io record-touched tripwires are `informational-drift`, exit 0 with a `::notice::`, and close/keep closed that issue like `no-drift`. It is schedule + `workflow_dispatch` only, so it gates no PR and is deliberately not in ruleset `18452968`. None of the four gates a PR (schedule + `workflow_dispatch` only); all file/close one deduped tracking issue per workflow (`scripts/file-exam-issue.sh`) and IFTTT-notify on failure | live (thin) |
| **T4 — local heavy lane** | `pnpm b4push` (below) | convenience, not enforcement |

**Do not scaffold unused tiers.** zfb needs T0 + T1 now; T4 is `b4push`; T3 has the thin weekly stand-in — the full nightly upgrade stays documented-but-deferred until cutover (see the manifest below).

### Branch ruleset on `main` (T1 enforcement)

T1's "mergeable when required checks are green" is enforced by a GitHub **ruleset** on `main` (id `18452968`, `main-required-status-checks`; created via `scripts/apply-main-ruleset.sh`, checked in for reproducibility — rerun it to recreate/update). It requires only the checks that run on **every** PR unconditionally (no path filters, no tag/workflow_run-only triggers): `health`, `build (no-v8)`, `Build binary (x86_64-unknown-linux-gnu)`, `Build binary (aarch64-unknown-linux-gnu)` (the job's `name:` is keyed on `matrix.platform.target` so each leg has a unique check context), the 4 `Smoke * (local mode)` jobs, `Scaffold E2E (packed tarballs, pre-publish)` (#1345, unconditional on every PR), `pnpm audit (prod)`, and `Docs gate` (#2827). The docs build itself — `Build docs site (docs-checks.yml)` — stays excluded: #1336 made it docs-only and #2827 moved that gate from the workflow trigger to a job-level `changed-files (docs)` detector, so it legitimately does not run on non-docs PRs and a required check that stops running would hang them forever; `Docs gate` is the unconditional job (`if: always()`, fails closed on a failed/cancelled detector or build) that turns its pass/skip/fail into a context safe to require. The two create-zfb showcase jobs (`Showcase deploy (create-zfb, production)`, `Showcase preview (create-zfb, PR)`, #2279) are excluded for the same class of reason — see "The create-zfb showcase" below. A `RepositoryRole` (`admin`, `actor_id: 5`) bypass actor with `bypass_mode: always` keeps `/l-make-release`'s direct version-bump push to `main` working (`required_status_checks` also blocks direct pushes). **Pending manual verification:** that bypass actor has not been proven with a live test push (see issue #1333) — treat `/l-make-release`'s direct-push step as unverified until a repo owner confirms it.

**`base/**` epic-PRs run the T1 gate too (#2076).** `health.yml` and `pr-checks.yml`'s `on.pull_request.branches` include `"base/**"` alongside `main`, so an `/x-wt-teams` epic-PR (which targets a super-epic `base/**` branch) triggers `health` + the required `pnpm audit (prod)` during its own review window. Deliberately **`pull_request`-only** — `health.yml`'s `push:` trigger stays main-only (a `base/**` push trigger would double-run the suite per commit), and `pr-checks.yml` carries no `push:` trigger. The CI-cost tradeoff (N sibling epics = N extra full runs per sweep) was accepted deliberately to catch a broken epic before it merges into the shared super-epic base. The `main` ruleset is unaffected — it targets only `refs/heads/main`.

### The create-zfb showcase (#2279)

`create-zfb` is otherwise invisible — the only way to see what `npm create zfb@latest my-site` produces is to run it. Two jobs in `node-free-smoke.yml` fix that by deploying the **real** scaffold output to an assets-only Cloudflare Worker (`zfb-showcase`) at **`create-zfb.takazudomodular.com`**. Config lives in `create-zfb-showcase/` (deploy config only — no `package.json`, deliberately not a pnpm workspace member).

**The site is never committed.** `scaffold-e2e` already scaffolds and builds a real site via `scripts/smoke-packed-clean-room.sh`; an opt-in `ZFB_SMOKE_DIST_OUT` makes it export that `dist/` as the `create-zfb-showcase-dist` artifact, which both showcase jobs consume. One scaffold+build, one oracle, two consumers — so the deployed site cannot drift from `crates/zfb/templates/`. This is a **share, not a refactor**: `Scaffold E2E (packed tarballs, pre-publish)` keeps its exact name, every existing step, and its required-check identity; the appended upload step carries `continue-on-error: true` so a showcase-side hiccup can never redden that required check.

| Surface | Tier | Blocking? |
|---|---|---|
| `scaffold-e2e` + its artifact export | T1 PR gate, unchanged | Required (already) |
| `html-validate` over the scaffold's emitted HTML | L3 build-output, inside the non-required showcase jobs | Blocks the deploy and the author; never blocks **merge**, never blocks **unrelated** PRs |
| `Showcase preview (create-zfb, PR)` | T1-adjacent, informational | **Not required** |
| `Showcase deploy (create-zfb, production)` | **Alerting guard** — runs post-merge, so it detects a bad deploy and cannot prevent one | **Not required** |

Neither showcase job is in ruleset `18452968`, and neither should be added: the preview is skipped entirely on fork and dependabot PRs (no secrets, read-only token), so as a required check it would hang those PRs forever — the same failure mode that kept `Build docs site` out until #2827 wrapped it in the unconditional `Docs gate`.

Three constraints that are load-bearing, not stylistic:

- **`cancel-in-progress` is conditional on `pull_request`.** A job-level `concurrency` block does **not** shield a job from a workflow-level cancel — the whole run dies, and group membership in one group confers nothing in another. Without the condition, two quick `main` pushes kill a deploy mid-`wrangler deploy`, skipping the domain attach, health check, and notify.
- **`html-validate` runs *before* the banner injection**, in both jobs. Reversed, the L3 signal would judge CI's injected markup instead of zfb's emitted HTML.
- **The banner is injected at deploy time** by `scripts/showcase-inject-banner.mjs`, never committed into `crates/zfb/templates/**` — that is what keeps the "this is exactly what you get" claim honest. It is a plain string splice with no HTML parser (a parse/serialize round-trip would normalize markup and perturb `element-permitted-content`).

Root-level vitest suites (`scripts/**/__tests__`, configured by the root `vitest.config.mjs`) only run because both `health.yml` and `run-b4push.sh` invoke the canonical `pnpm test:workspace` script, which passes **`--include-workspace-root`**. A plain recursive pnpm test invocation skips the workspace root, so without that flag a root-level suite exists and silently never runs. Keep both surfaces on the canonical script — they must stay identical or the local gate and the PR gate drift.

**Also note:** the showcase deploy jobs hardcode `wrangler@4.85.0` rather than reading it from `docs/package.json` the way the docs workflows do. That is deliberate: `npx wrangler@<string read from a file the PR can edit>` is arbitrary code execution next to the production Cloudflare token, because npm specifiers accept `file:`, `github:`, and remote-tarball forms — not just versions. Keep it in sync with `docs/package.json` by hand when bumping wrangler.

### Required behavior (agents)

1. **Declare the test plan first** — *what* you're testing, *which level*, *why* that level.
2. **Match level to goal** — don't verify a Level-5 visual bug with a Level-1 logic test.
3. **Escalate when a lower level passes but the problem persists** (never re-run, never "clear cache").
4. **Default to Level 5 for any UI/CSS/visibility work.**
5. **Report what was NOT tested** — state the blind spots.
6. **Verification specs don't self-graduate.** A one-time "it was done" proof is tagged `#[ignore = "verification: <why>"]` (Rust) / `@verification` (TS) and excluded from gates. Propose promotion to a tier in the PR description; never self-promote.
7. **Red checks block the author.** Any red check on a PR you authored blocks you, *even if it is not a required check* — the only exception is a test carrying a `flaky: <issue-url>` quarantine tag (Rust `#[ignore = "flaky: <url>"]`; TS `// flaky: <url>` above `it.skip(...)`) with a linked open issue.
8. **Never game the gate.** Do not add `#[ignore]` / `test.skip`, a flaky tag, a loosened tolerance, or a deleted assertion **without a linked open issue**. Making a gate pass by editing existing assertions needs a fresh-context review — not the same session that wrote the change.
9. **Scoped heavy verification.** When a change touches code covered only by a heavy/quarantined lane, run those tests on a capable host before declaring the work done.

### Flaky tests

zfb hits flakiness often (mostly **Rust timing/ordering** in the dev-server, SSE, and plugin-runner paths). Handle it as a pipeline, not a shrug.

- **Retry budget:** local **0**; CI **1–2** with artifacts. **Pass-on-retry is a triage signal, not a success** — record it and schedule the fix. **More than 2 retries is a smell.**
- **The `flaky:` quarantine pipeline (Rust):**
  - **Step 0 — prove it ever genuinely passed** (not pass-by-skip) on some host. A test that can pass *nowhere* is **broken, not flaky** → fix or delete it now, no quarantine.
  - **Quarantine requires a paper trail** — mark it `#[ignore = "flaky: <issue-url>"]` (the inline issue URL is mandatory) and add its manifest row in `crates/CLAUDE.md`. `cargo test` skips `#[ignore]`d tests, so it no longer blocks the gate.
  - **It must still run somewhere, allowed-to-fail** — locally via `cargo test -- --ignored` (or `--include-ignored`), AND weekly in CI: `exam.yml`'s `quarantine-heavy` job runs the env-gate + heavy manifest subset (`--run-ignored ignored-only`, exact-name filterset; `pending-feature`-tagged tests are deliberately excluded — they're blocked on unimplemented features, not flakiness) allowed-to-fail, filing/closing a deduped tracking issue.
  - **Quarantine has an exit with a deadline — fix, demote, or delete.** It is not a parking lot. **Quarantine suspends *product* coverage, not just test coverage:** the behavior is unguarded until the test is fixed.
- **The 5 deflaking root causes** (fix the cause, don't add a sleep): bare timing waits, `networkidle` on no-request SPA navs, animations in flight, shared/order-coupled state, hydration races. zfb's Rust flakes are mostly timing/ordering — prefer event/condition-keyed waits and deterministic scheduling over fixed `sleep`s (as `zfb-test-utils`'s SSE helpers and the `plugin_runner` timeouts already do).
- **`cargo-nextest` (ADOPTED — #1340):** CI runs the Rust suite under `cargo nextest run --workspace --profile ci` (health.yml), configured in `.config/nextest.toml`. The `ci` profile RECORDS retries (`retries = 1`, pass-on-retry reported as `FLAKY` + the `nextest-junit-ci` artifact; #1341 consumes that telemetry) — do not raise `retries` to paper over a flake; local runs use the default profile (`retries = 0`) so flakes fail loudly. **Doctests are NOT run by nextest** — health.yml keeps a separate `cargo test --workspace --doc` step. The doctest baseline, the two `e2e-heavy-*` groups (24 flock-adopting binaries in `e2e-heavy-locked` and 17 build-only binaries in `e2e-heavy-unlocked`, each `max-threads = 1`), the required-features lane, and the inventory-parity reconciliation rule all live in `crates/CLAUDE.md`.
- **`scripts/__tests__/docs-dev-supervisor.test.mjs` phase telemetry (#2889/#2904):** opt-in `[supervisor-timeline]` lines behind `ZFB_SUPERVISOR_TIMELINE=1`, off by default (b4push does not set it — extra stderr noise the local loop didn't ask for). `health.yml` now sets it on the `pnpm test:workspace` step so every ubuntu CI run contributes a sample. This buys observability, not aggregation or storage — lines land scattered across workflow logs under GitHub's retention. `scripts/supervisor-timeline-summary.mjs` reads them and applies #2887's R-A/R-B/R-C rules; its doc comment is the protocol's source of truth. `scripts/harvest-supervisor-timelines.mjs` (#2908) is the aggregation step: it re-fetches each run's `health` job log via `gh` and re-parses it with the summarizer's own `parseTimelineLine` on demand, so the pipeline stays observability, not storage.

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
- Same rules as Rust apply: **never game the gate** (no `it.skip` for `flaky:` without a linked open issue), **quarantine has an exit with a deadline**, and a red check still blocks the author unless it carries a valid `// flaky: <issue-url>` tag (Required behavior rule 7). The first real TS quarantine/`@verification` adds a manifest table entry (mirroring the Rust manifest in `crates/CLAUDE.md`) so this section stops being purely aspirational.

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
- Heavy opt-in: `B4PUSH_FULL=1 pnpm b4push` additionally runs the **required health.yml Rust parity set** (#1332): `cargo nextest run --workspace` (or `cargo test --workspace` when nextest is absent) on nextest's **default** profile (`retries = 0`, unlike CI's `--profile ci`); `cargo test --workspace --doc` (nextest branch only); the `zfb-md-extras` `test-utils`-gated suite plus its scoped `cargo clippy`; `cargo check --no-default-features -p zfb --tests`; the `zfb-islands` esbuild env-gate suite; #1504's real-zfb cross-pipeline acceptance test; and the three command/package locations carrying tailwindcss-v4 env-gates. These five dedicated env-gate commands cover 36 of health.yml's 63 ignored env-gate tests (the 16 additional esbuild workspace-resolution regressions and 11 `css_command` tests remain CI-only), so b4push is not a complete health parity run. The command-layer step pins both esbuild and Tailwind because it co-runs two esbuild tests. These **5 env-gate steps** skip (rather than fail) when their required staged binary slot is absent; run `cargo build --workspace --all-targets` first for the b4push subset. The optional path-gated `wasm-md` matrix and browser job are not part of b4push.
- It is **not** wired into the git `pre-push` hook (that hook only enforces the worktree-push policy above) — run it manually before pushing.
