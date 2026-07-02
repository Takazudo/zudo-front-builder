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

## Testing

zfb follows the **zudo-test-wisdom** strategy (the full guide: <https://takazudomodular.com/pj/zudo-test>). This section is the zfb-adapted, agent-facing summary — read it before writing or fixing tests. Every test sits on **two axes**:

- **Level** = *what a test can see* (logic → DOM → build output → browser → pixels).
- **Tier** = *where and when it runs* (inner loop → PR gate → scheduled → local heavy lane).

The axes are independent. "Too heavy for the PR gate" is a **tier** question, never a reason to rewrite a test at a lower **level**.

### Test levels (escalation ladder)

| Level | What it sees | In zfb |
|---|---|---|
| 1 — Unit/logic | pure functions, transforms | `cargo test` unit tests; vitest unit tests |
| 2 — DOM component | DOM structure, no CSS engine | `zfb-runtime` happy-dom vitest (the client router) |
| 3 — Build output | emitted files **and emitted code that runs** | reading built `dist/`; the *dynamic-import-the-emitted-`_worker.js`* pattern in `zfb-adapter-cloudflare` (zfb is a code generator — string-equality proves the template was written, executing the artifact proves it threads `env`/`ctx`) |
| 4 — E2E browser | real process / browser | `crates/zfb/tests/dev_serve_e2e.rs` (dev server). No Playwright browser E2E yet |
| 5 — Visual | computed styles + pixels | the docs site / any UI — use `/verify-ui` + `/headless-browser` |
| 6 — AI-based | final resort, **not for CI** | not used; only for canvas/zoom surfaces L4+L5 cannot reach |

**Escalation rule:** when a test passes but the problem persists, **escalate to the next level — do not re-run the same test, and never tell the user to "clear cache."** If it's still broken, the code is still broken. Any CSS/layout/visibility work defaults to **Level 5**.

### Execution tiers

| Tier | What it is in zfb | Status |
|---|---|---|
| **T0 — inner loop** | `cargo check`/`clippy` + scoped `cargo test -p <crate>` (affected), `pnpm -r --if-present typecheck`, `pnpm -r test`. Retries 0. | run constantly while coding |
| **T1 — PR gate** | `.github/workflows/health.yml` — fmt, `clippy -D warnings`, build, `cargo test --workspace`, `pnpm -r test`, `format:check`, actionlint, build-no-v8. | **the authoritative gate.** A PR is mergeable when its required checks are green |
| **T3 — scheduled re-exam** | heavy/platform-bound lanes on a schedule | **DEFERRED until release.** zfb is pre-release WIP; standing up a nightly runner is not cost-justified yet. This is a **public repo** — no blacksmith, no self-hosted runners. Adopt at cutover |
| **T4 — local heavy lane** | `pnpm b4push` (below) | convenience, not enforcement |

**Do not scaffold unused tiers.** zfb needs T0 + T1 now; T4 is the new `b4push`; T3 is documented-but-deferred.

### Branch ruleset on `main` (T1 enforcement)

T1's "mergeable when required checks are green" is now enforced by a GitHub **ruleset** on `main` (id `18452968`, `main-required-status-checks`; created via `scripts/apply-main-ruleset.sh`, checked in for reproducibility — rerun it to recreate/update if the ruleset is ever deleted). It requires the `required_status_checks` rule with only the checks that run on **every** PR unconditionally (no path filters, no tag/workflow_run-only triggers): `health`, `build (no-v8)`, `Build binary (ubuntu-22.04)`, the 4 `Smoke * (local mode)` jobs, and `pnpm audit (prod)`. `Build docs site` is deliberately excluded because a sibling change (#1336) makes it path-filtered to `docs/**` — a required check that stops running on non-docs PRs would hang them forever. A `RepositoryRole` (`admin`, `actor_id: 5`) bypass actor with `bypass_mode: always` is configured so `/l-make-release`'s direct version-bump push to `main` keeps working — `required_status_checks` also blocks direct pushes (a directly-pushed commit has no passing checks recorded against it), so without this bypass entry releases would break. **Pending manual verification:** the bypass actor has not yet been proven with a live test push (see issue #1333 / the PR that introduced this ruleset for the exact verification command) — treat `/l-make-release`'s direct-push step as unverified against this ruleset until a repo owner confirms it.

### Required behavior (agents)

1. **Declare the test plan first** — *what* you're testing, *which level*, *why* that level.
2. **Match level to goal** — don't verify a Level-5 visual bug with a Level-1 logic test.
3. **Escalate when a lower level passes but the problem persists** (never re-run, never "clear cache").
4. **Default to Level 5 for any UI/CSS/visibility work.**
5. **Report what was NOT tested** — state the blind spots.
6. **Verification specs don't self-graduate.** A one-time "it was done" proof is tagged `#[ignore = "verification: <why>"]` (Rust) / `@verification` (TS) and excluded from gates. Propose promotion to a tier in the PR description; never self-promote.
7. **Red checks block the author.** Any red check on a PR you authored blocks you, *even if it is not a required check* — the only exception is a test carrying `@flaky`/`flaky:` with a linked open issue.
8. **Never game the gate.** Do not add `#[ignore]` / `test.skip`, a flaky tag, a loosened tolerance, or a deleted assertion **without a linked open issue**. Making a gate pass by editing existing assertions needs a fresh-context review — not the same session that wrote the change.
9. **Scoped heavy verification.** When a change touches code covered only by a heavy/quarantined lane, run those tests on a capable host before declaring the work done.

### Flaky tests

zfb hits flakiness often (mostly **Rust timing/ordering** in the dev-server, SSE, and plugin-runner paths). Handle it as a pipeline, not a shrug.

- **Retry budget:** local **0**; CI **1–2** with artifacts. **Pass-on-retry is a triage signal, not a success** — record it and schedule the fix. **More than 2 retries is a smell.**
- **The `@flaky` quarantine pipeline (Rust):**
  - **Step 0 — prove it ever genuinely passed** (not pass-by-skip) on some host. A test that can pass *nowhere* is **broken, not flaky** → fix or delete it now, no quarantine.
  - **Quarantine requires a paper trail** — mark it `#[ignore = "flaky: <issue-url>"]` (the inline issue URL is mandatory). `cargo test` skips `#[ignore]`d tests, so it no longer blocks the gate.
  - **It must still run somewhere, allowed-to-fail** — today, locally via `cargo test -- --ignored` (or `--include-ignored`); post-cutover, the T3 scheduled exam runs the quarantined set allowed-to-fail so failure data keeps flowing into the tracking issue.
  - **Quarantine has an exit with a deadline — fix, demote, or delete.** It is not a parking lot. **Quarantine suspends *product* coverage, not just test coverage:** the behavior is unguarded until the test is fixed.
- **The 5 deflaking root causes** (fix the cause, don't add a sleep): bare timing waits, `networkidle` on no-request SPA navs, animations in flight, shared/order-coupled state, hydration races. zfb's Rust flakes are mostly timing/ordering — prefer event/condition-keyed waits and deterministic scheduling over fixed `sleep`s (as `zfb-test-utils`'s SSE helpers and the `plugin_runner` timeouts already do).
- **`cargo-nextest` (recommended next step, deferred):** CI uses plain `cargo test`, which has no native retry. `cargo nextest run` would give the CI retry budget (`--retries`), flaky-pass detection, per-test timeouts, and JUnit output for issue filing — **but nextest does not run doctests**, and zfb has doctests in ~15 crates. A migration must keep a separate `cargo test --doc` step or it silently drops doctest coverage; that's why it is deferred from the adoption PR rather than done blindly.

### `b4push` — local pre-push pass (T4)

`pnpm b4push` runs a **bounded** fast pass before pushing: shell-script syntax, `cargo fmt --check`, `pnpm format:check`, `pnpm -r --if-present typecheck`, `pnpm -r test`, and `cargo clippy` (warm tree). It is **not** the authoritative gate — `health.yml` is. b4push is the *fast subset*; the full `cargo test --workspace` runs in CI (V8 first-compile is 15–30 min, too heavy for a pre-push loop).

- Escapes: `B4PUSH_SKIP_CLIPPY=1`, `B4PUSH_SKIP_JS_TEST=1`.
- Heavy opt-in: `B4PUSH_FULL=1 pnpm b4push` adds `cargo test --workspace` for a thorough local run.
- It is **not** wired into the git `pre-push` hook (that hook only enforces the worktree-push policy above) — run it manually before pushing.
