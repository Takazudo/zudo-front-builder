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
| 4 — E2E browser | real process / browser | `crates/zfb/tests/dev_serve_e2e.rs` (dev server, Rust process-level e2e); `pnpm test:router-chromium` (`tests/router-chromium/`, real Chromium via Playwright — CI-gated by `.github/workflows/router-chromium.yml`, path-filtered to `packages/zfb-runtime/**` + `packages/zfb/src/runtime.ts` (the islands runtime — #1399) so it is NOT a required check); `pnpm test:webkit-back` (`tests/webkit-back-history/`, real WebKit back/forward-cache nav via Playwright — T4 local-heavy, Mac only, never runs in CI; see the release-day hook in RELEASE_DAY_CHECKLIST.md) |
| 5 — Visual | computed styles + pixels | the docs site / any UI — use `/verify-ui` + `/headless-browser` |
| 6 — AI-based | final resort, **not for CI** | not used; only for canvas/zoom surfaces L4+L5 cannot reach |

**Escalation rule:** when a test passes but the problem persists, **escalate to the next level — do not re-run the same test, and never tell the user to "clear cache."** If it's still broken, the code is still broken. Any CSS/layout/visibility work defaults to **Level 5**.

### Execution tiers

| Tier | What it is in zfb | Status |
|---|---|---|
| **T0 — inner loop** | `cargo check`/`clippy` + scoped `cargo test -p <crate>` (affected), `pnpm -r --if-present typecheck`, `pnpm -r test`. Retries 0. | run constantly while coding |
| **T1 — PR gate** | `.github/workflows/health.yml` — fmt, `clippy -D warnings`, build, `cargo nextest run --workspace --profile ci` + a separate `cargo test --workspace --doc`, `pnpm -r test`, `format:check`, actionlint, build-no-v8. | **the authoritative gate.** A PR is mergeable when its required checks are green |
| **T3 — scheduled re-exam** | heavy/platform-bound lanes on a schedule | **Thin pre-release lane is LIVE; the full nightly upgrade is still deferred.** `.github/workflows/exam.yml` (issue #1344, weekly Saturday 04:17 UTC) runs the `#[ignore]`d env-gate + heavy manifest below allowed-to-fail, plus a full `cargo nextest run --workspace` + doctest re-exam on macOS (the one FSEvents-vs-inotify coverage gap `health.yml` can't cover, since it's ubuntu-only). `.github/workflows/drift-net.yml` (issue #1342, weekly Wednesday 03:43 UTC) re-runs the release clean-room smoke against the live `next` dist-tag on ubuntu + macOS between releases. `.github/workflows/security-audit.yml` (weekly Monday 07:17 UTC) joined the same tracked-lane pattern in issue #1394 — its `pnpm audit (prod)` and `cargo deny` jobs previously had no failure paper-trail at all. None of the three gates a PR (schedule + `workflow_dispatch` only); all file/close one deduped tracking issue per workflow (`scripts/file-exam-issue.sh`) and IFTTT-notify on failure. See "T3 cutover manifest" below for exactly what upgrades (nightly cadence, Windows leg, etc.) and its trigger |
| **T4 — local heavy lane** | `pnpm b4push` (below) | convenience, not enforcement |

**Do not scaffold unused tiers.** zfb needs T0 + T1 now; T4 is the new `b4push`; T3 has a thin weekly stand-in (`exam.yml` + `drift-net.yml`) — the full nightly upgrade stays documented-but-deferred until cutover (see the manifest below).

### Branch ruleset on `main` (T1 enforcement)

T1's "mergeable when required checks are green" is now enforced by a GitHub **ruleset** on `main` (id `18452968`, `main-required-status-checks`; created via `scripts/apply-main-ruleset.sh`, checked in for reproducibility — rerun it to recreate/update if the ruleset is ever deleted). It requires the `required_status_checks` rule with only the checks that run on **every** PR unconditionally (no path filters, no tag/workflow_run-only triggers): `health`, `build (no-v8)`, `Build binary (x86_64-unknown-linux-gnu)`, `Build binary (aarch64-unknown-linux-gnu)`, the 4 `Smoke * (local mode)` jobs, `Scaffold E2E (packed tarballs, pre-publish)`, and `pnpm audit (prod)`. (The two `Build binary` checks used to both render as the ambiguous `Build binary (ubuntu-22.04)` since both matrix legs share that runner — the job's `name:` is now keyed on `matrix.platform.target` instead so each leg has its own unique check context.) `Scaffold E2E (packed tarballs, pre-publish)` (issue #1345) runs unconditionally on every PR too (no path filter, no `if:` condition — it only depends on `build-binary`, which itself gates on `pull_request`/`push`, the same trigger scope as the rest of node-free-smoke.yml), so it belongs in this list. `Build docs site` is deliberately excluded because a sibling change (#1336) makes it path-filtered to `docs/**` — a required check that stops running on non-docs PRs would hang them forever. A `RepositoryRole` (`admin`, `actor_id: 5`) bypass actor with `bypass_mode: always` is configured so `/l-make-release`'s direct version-bump push to `main` keeps working — `required_status_checks` also blocks direct pushes (a directly-pushed commit has no passing checks recorded against it), so without this bypass entry releases would break. **Pending manual verification:** the bypass actor has not yet been proven with a live test push (see issue #1333 / the PR that introduced this ruleset for the exact verification command) — treat `/l-make-release`'s direct-push step as unverified against this ruleset until a repo owner confirms it.

### Required behavior (agents)

1. **Declare the test plan first** — *what* you're testing, *which level*, *why* that level.
2. **Match level to goal** — don't verify a Level-5 visual bug with a Level-1 logic test.
3. **Escalate when a lower level passes but the problem persists** (never re-run, never "clear cache").
4. **Default to Level 5 for any UI/CSS/visibility work.**
5. **Report what was NOT tested** — state the blind spots.
6. **Verification specs don't self-graduate.** A one-time "it was done" proof is tagged `#[ignore = "verification: <why>"]` (Rust) / `@verification` (TS) and excluded from gates. Propose promotion to a tier in the PR description; never self-promote.
7. **Red checks block the author.** Any red check on a PR you authored blocks you, *even if it is not a required check* — the only exception is a test carrying a `flaky: <issue-url>` quarantine tag (Rust `#[ignore = "flaky: <url>"]`; TS `// flaky: <url>` above `it.skip(...)`, see the TS-side idiom below) with a linked open issue.
8. **Never game the gate.** Do not add `#[ignore]` / `test.skip`, a flaky tag, a loosened tolerance, or a deleted assertion **without a linked open issue**. Making a gate pass by editing existing assertions needs a fresh-context review — not the same session that wrote the change.
9. **Scoped heavy verification.** When a change touches code covered only by a heavy/quarantined lane, run those tests on a capable host before declaring the work done.

### Flaky tests

zfb hits flakiness often (mostly **Rust timing/ordering** in the dev-server, SSE, and plugin-runner paths). Handle it as a pipeline, not a shrug.

- **Retry budget:** local **0**; CI **1–2** with artifacts. **Pass-on-retry is a triage signal, not a success** — record it and schedule the fix. **More than 2 retries is a smell.**
- **The `flaky:` quarantine pipeline (Rust):**
  - **Step 0 — prove it ever genuinely passed** (not pass-by-skip) on some host. A test that can pass *nowhere* is **broken, not flaky** → fix or delete it now, no quarantine.
  - **Quarantine requires a paper trail** — mark it `#[ignore = "flaky: <issue-url>"]` (the inline issue URL is mandatory). `cargo test` skips `#[ignore]`d tests, so it no longer blocks the gate.
  - **It must still run somewhere, allowed-to-fail** — locally via `cargo test -- --ignored` (or `--include-ignored`), AND weekly in CI: `.github/workflows/exam.yml`'s `quarantine-heavy` job runs the exact env-gate + heavy subset of the manifest below (`--run-ignored ignored-only` with an exact-name filterset; `pending-feature`-tagged tests are deliberately excluded — they're blocked on unimplemented features, not flakiness) allowed-to-fail, filing/closing a deduped tracking issue so failure data keeps flowing without gating a PR. This is the thin pre-release stand-in (issue #1344) — see "T3 cutover manifest" below for what the full nightly upgrade still needs.
  - **Quarantine has an exit with a deadline — fix, demote, or delete.** It is not a parking lot. **Quarantine suspends *product* coverage, not just test coverage:** the behavior is unguarded until the test is fixed.
- **The 5 deflaking root causes** (fix the cause, don't add a sleep): bare timing waits, `networkidle` on no-request SPA navs, animations in flight, shared/order-coupled state, hydration races. zfb's Rust flakes are mostly timing/ordering — prefer event/condition-keyed waits and deterministic scheduling over fixed `sleep`s (as `zfb-test-utils`'s SSE helpers and the `plugin_runner` timeouts already do).
- **`cargo-nextest` (ADOPTED — issue #1340):** CI runs the Rust test suite under `cargo nextest run --workspace --profile ci` (health.yml), configured in `.config/nextest.toml`. This gives the CI retry budget, per-test timeouts, and JUnit output — the mechanisms below are now real, not aspirational.
  - **Retries are RECORDED, not hidden.** The `ci` profile sets `retries = 1`; a pass-on-retry is reported as a terminal `FLAKY` line and in the JUnit report (`target/nextest/ci/junit.xml`, uploaded as the `nextest-junit-ci` artifact). This is the mechanical enforcement of "**pass-on-retry is a triage signal, not a success**" — a future sub-task (#1341) consumes that telemetry to file/triage flakes. Do not raise `retries` to paper over a flake. Local runs (b4push `B4PUSH_FULL=1`) use nextest's **default** profile (`retries = 0`) so a local flake fails loudly.
  - **Doctests are NOT run by nextest.** health.yml keeps a **separate** `cargo test --workspace --doc` step. Current doctest baseline (verify before changing; do not trust stale counts): **5 doctests execute + pass** — `zfb-css` (2) and `zfb-islands` (3) — plus **6** example blocks the harness lists but marks ignored/`no_run` (`zfb-islands` 3, `zfb-server` 2, `zfb-test-utils` 1), i.e. **11 doctest entries across 4 crates**. (The old "~15 crates" figure in this doc was wrong; #1340 measured the real numbers.) The doc-test step must not regress below this.
  - **Heavy e2e binaries are serialized by a nextest `test-group`.** nextest schedules test *binaries* concurrently by default, which would boot several real `zfb dev`/`zfb build`/`zfb preview` V8+esbuild processes at once. `.config/nextest.toml` defines a `[test-groups.e2e-heavy]` with `max-threads = 1` and an override assigning **22** heavy binaries to it: **13 flock-adopting** binaries (`build_terminates`, `client_bundling_cross_pipeline`, `dev_bind_before_walk_e2e`, `dev_build_static_parity`, `dev_content_aggregate_cold_boot_e2e`, `dev_dep_invalidation_1284_e2e`, `dev_out_of_root_collection_e2e`, `dev_public_large_tree_smoke_e2e`, `dev_serve_e2e`, `dev_serve_injected_routes_e2e`, `dev_sibling_watch_1678_e2e`, `preview_cross_mode_e2e`, `wasm_ssr_dev_smoke_e2e`) that spawn `zfb dev` and/or `zfb build` and/or `zfb preview`, plus **9 build-only** binaries (`build_cleans_outdir`, `build_package_routes`, `build_package_routes_consumer`, `client_router_autoinclude_build`, `content_snapshot_no_deferred`, `css_modules_components_build`, `end_to_end_basic_blog_build`, `html_minify_build`, `wasm_ssr_adapter_e2e`) that do not adopt the flock. For the first 13, the flock and test-group are **defense in depth** (flock = process-level guarantee; test-group = runner-level guarantee); for the build-only 9, the test-group is the **sole** serialization guard. `preview_cross_mode_e2e` (issue #1547) runs a real `zfb build` and then spawns both a real `zfb dev` and a real `zfb preview` (static) against that build output, proving the `_redirects`/plugin-middleware behavior landed by epic #1541 is identical across both. `dev_out_of_root_collection_e2e` (issue #1552, epic #1548) covers a configured collection outside the project directory (`allowOutsideRoot: true`) in a live edit→serve loop. `dev_content_aggregate_cold_boot_e2e` (issue #1598) starts a fresh content fixture and proves an entry edit reaches its entry, index, tag, and paginated aggregate routes. `wasm_ssr_dev_smoke_e2e` (issue #1511) is the newest flock-adopting member: it boots a real `zfb dev` session and serves a `prerender=false` route whose 200 response is derived from an imported Wasm module. (`check_command` and `framework_packages_no_pnpm` are deliberately excluded — the former runs only `zfb check`, the latter calls `zfb_build::bundle` in-process without spawning `zfb` or booting V8.) The group also carries a long per-test `slow-timeout` (`terminate-after` = 600s) so legitimate dev-server watchdogs are never killed as "hung". The same override also joins `render_pipeline::tests::eval_deferred_paths_via_worker_embedded_v8_non_literal_paths` and `version_stamp_from_env` by exact test name; this keeps those individual heavy tests serialized against the other 22 binaries when exam.yml runs the ignored manifest.
  - **required-features lane.** `cargo nextest run -p zfb-md-extras --features test-utils` DOES pick up the `required-features = ["test-utils"]`-gated targets (verified in #1340), so that lane runs under nextest too.
  - **Inventory parity is guaranteed by reconciliation, not assumed.** #1340 diffed `cargo test --workspace -- --list` against `cargo nextest list --workspace --run-ignored all`: byte-identical (3295 = 3295) apart from doctests. No test target uses a custom `harness = false`, so nextest can run every binary. If you add a custom-harness or benchmark test target, re-run that reconciliation — nextest silently skips binaries it cannot drive.

### `#[ignore]` manifest (Rust)

Every `#[ignore]`d Rust test carries a reason starting with one of 5 machine-greppable taxonomy prefixes — `grep -rn '#\[ignore = "' crates/` and spot-check the prefix:

- `env-gate: <binary> — <how to run>` — needs an external binary present (no issue URL needed).
- `heavy: run with --ignored — <why>` — deliberate heavy/local-only test (no issue URL needed).
- `flaky: <issue-url>` — quarantined flaky test (issue URL mandatory).
- `verification: <why>` — one-time "it was done" proof, not a regression guard (no issue URL needed).
- `pending-feature: <issue-url>` — blocked on an unimplemented product feature or unwritten test body (issue URL mandatory).

Audited and retagged in full during issue #1337 (2026-07); reconciled again during issue #1504 (2026-07); a third cross-pipeline env-gate joined during issue #1674 (2026-07); a dev workspace-sibling watch e2e joined + the pre-existing `shared_bundle_discovers_marked_glue_and_wasm_resources` env-gate row (missing since #1674) restored during issue #1678 (2026-07); a `pending-feature` row joined during issue #1698 (2026-07, sibling-mirror epic confirm pass — issue #1724). The table below lists all **35** ignored Rust tests (**27** `env-gate`, **6** `heavy`, **1** `verification`, **1** `pending-feature`) and their scheduled homes. Update this table whenever a `#[ignore]` is added, removed, or reclassified.

| Test (file:line) | Tag | Where / how it runs |
|---|---|---|
| `crates/zfb-islands/tests/integration.rs:222` `subprocess_bundler_against_real_binary` | `env-gate` (esbuild) | **CI**: `health.yml` → "Run zfb-islands esbuild-gated integration suite" step (runs right after the esbuild staging/assert step). Local: `ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-islands -- --ignored`. |
| `crates/zfb-islands/tests/integration.rs:285` `shared_bundle_keeps_islands_with_no_top_level_side_effect` | `env-gate` (esbuild) | Same as above. |
| `crates/zfb-islands/tests/integration.rs:394` `splitting_emits_chunk_for_dynamic_import` | `env-gate` (esbuild) | Same as above. |
| `crates/zfb-islands/tests/integration.rs:508` `no_dynamic_import_yields_single_file` | `env-gate` (esbuild) | Same as above. |
| `crates/zfb-islands/tests/integration.rs:556` `shared_bundle_discovers_marked_glue_and_wasm_resources` | `env-gate` (esbuild) | Same as above. |
| `crates/zfb-islands/tests/integration.rs:553` `client_script_real_esbuild_bundles_discovered_entry` | `env-gate` (esbuild) | Same as above. |
| `crates/zfb-islands/tests/integration.rs:658` `client_script_shadow_jobs_resolve_project_tsconfig_aliases` | `env-gate` (esbuild) | Same as above. |
| `crates/zfb-islands/tests/integration.rs:769` `islands_shadow_raw_import_bundles_text` | `env-gate` (esbuild) | Same as above. |
| `crates/zfb-islands/tests/integration.rs:830` `islands_shadow_alias_raw_import_bundles_text` | `env-gate` (esbuild) | Same as above. |
| `crates/zfb-islands/tests/integration.rs:913` `client_script_raw_import_bundles_text` | `env-gate` (esbuild) | Same as above. |
| `crates/zfb-islands/tests/integration.rs:978` `island_module_worker_emits_contract_companion_and_dev_layout` | `env-gate` (esbuild) | Same as above. |
| `crates/zfb-islands/tests/integration.rs:1134` `module_worker_define_only_change_updates_query_and_emitted_bytes` | `env-gate` (esbuild) | Same as above. |
| `crates/zfb-islands/tests/integration.rs:1194` `module_worker_package_config_switch_updates_query_and_emitted_bytes` | `env-gate` (esbuild) | Same as above. |
| `crates/zfb-islands/tests/integration.rs:1286` `module_worker_plugin_inputs_update_query_closure_and_emitted_bytes` | `env-gate` (esbuild) | Same as above. |
| `crates/zfb-islands/tests/integration.rs:1390` `island_css_import_bundles_without_error` | `env-gate` (esbuild) | Same as above. |
| `crates/zfb-islands/tests/integration.rs:1452` `island_module_css_import_bundles_without_error` | `env-gate` (esbuild) | Same as above. |
| `crates/zfb-islands/tests/integration.rs:1724` `islands_shadow_expands_glob_and_executes` | `env-gate` (esbuild) | Same as above. |
| `crates/zfb-islands/tests/integration.rs:1795` `islands_shadow_preserve_symlinks_is_load_bearing` | `env-gate` (esbuild) | Same as above. |
| `crates/zfb/src/commands/build.rs:8974` `preprocessing_shadows_bundle_nested_alias_raw_and_workers_with_real_esbuild` | `env-gate` (esbuild) | **CI**: `health.yml` → "Run zfb command-layer env-gate unit tests (commands::build)". Local: `ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb --lib commands::build::tests::preprocessing_shadows_bundle_nested_alias_raw_and_workers_with_real_esbuild -- --ignored`. |
| `crates/zfb/src/commands/build.rs:9358` `plugin_alias_only_client_preprocessing_triggers_shadow_with_real_esbuild` | `env-gate` (esbuild) | Same CI step as above. Local: `ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb --lib commands::build::tests::plugin_alias_only_client_preprocessing_triggers_shadow_with_real_esbuild -- --ignored`. |
| `crates/zfb/tests/client_bundling_cross_pipeline.rs:1101` `real_binary_build_and_dev_cover_client_bundling_contract` | `env-gate` (esbuild) | **CI**: `health.yml` → "Run zfb client-bundling cross-pipeline acceptance test". Local: `ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb --test client_bundling_cross_pipeline -- --ignored`. Level-4 real `zfb build` + `zfb dev --port 0`; serialized by the flock, the in-binary serial guard, and nextest `e2e-heavy` group. |
| `crates/zfb/tests/client_bundling_cross_pipeline.rs:1126` `real_binary_build_and_dev_cover_pnpm_workspace_raw_hardening_contract` | `env-gate` (esbuild) | Same CI step as above. Local: `ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb --test client_bundling_cross_pipeline -- --ignored`. Level-4 real `zfb build` + `zfb dev --port 0` confirmation for issue #1620's pnpm-workspace raw-hardening fixture; serialized by the flock, the in-binary serial guard, and nextest `e2e-heavy` group. |
| `crates/zfb/tests/client_bundling_cross_pipeline.rs:1153` `real_binary_build_and_dev_cover_workspace_reroot_host_contract` | `env-gate` (esbuild) | Same CI step as above (the step runs `-- --ignored`, which picks up every ignored test in the binary). Local: `ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb --test client_bundling_cross_pipeline -- --ignored`. Level-4 real `zfb build` + `zfb dev --port 0` for issue #1674's genuinely-claimed pnpm-workspace sub-package host reaching sibling `?raw` + worker preprocessing; serialized by the flock, the in-binary serial guard, and nextest `e2e-heavy` group. |
| `crates/zfb/tests/wasm_ssr_adapter_e2e.rs:375` `wrangler_dry_run_attaches_runtime_wasm_as_compiled_module` | `env-gate` (wrangler 4.85.0) | Local: `ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb --test wasm_ssr_adapter_e2e -- --ignored`. **CI, weekly (T3):** `exam.yml`'s `quarantine-heavy` job runs it via the exact-name filterset (issue #1344). Level-4 real `zfb build` plus pinned `wrangler deploy --dry-run --no-bundle --outdir`, using Wrangler's default `CompiledWasm` rule; it confirms the exact copied SSR-only Wasm import reaches the module table as `compiled-wasm` with matching emitted bytes, and skips when Wrangler is unavailable. Serialized by nextest `e2e-heavy`. |
| `crates/zfb-css/tests/integration.rs:48` `subprocess_engine_against_real_binary` | `env-gate` (tailwindcss v4) | Local: `cargo test -p zfb-css --test integration -- --include-ignored`. **CI, T1 (issue #1393)**: `health.yml` → "Run zfb-css tailwindcss-v4 env-gate integration test" step. **Also CI, weekly (T3)**: `exam.yml`'s `quarantine-heavy` job runs it via the exact-name filterset (issue #1344). |
| `crates/zfb-build/tests/prod_asset_graph_e2e.rs:795` `prod_asset_graph_with_real_tailwind_binary_against_fixture` | `env-gate` (tailwindcss v4) | Local: `cargo test -p zfb-build --test prod_asset_graph_e2e -- --include-ignored`. **CI, T1 (issue #1393)**: `health.yml` → "Run zfb-build tailwindcss-v4 env-gate integration test" step. **Also CI, weekly (T3)**: `exam.yml`'s `quarantine-heavy` job (issue #1344). |
| `crates/zfb/src/commands/build.rs:11176` `default_runner_emit_prod_assets_returns_non_empty_css_for_real_project` | `env-gate` (tailwindcss v4) | Local: `cargo test -p zfb --lib commands::build:: -- --include-ignored`. **CI, T1 (issue #1393)**: `health.yml` → "Run zfb command-layer env-gate unit tests (commands::build)" step. **Also CI, weekly (T3)**: `exam.yml`'s `quarantine-heavy` job (issue #1344). |
| `crates/zfb-css/tests/hi_color_extraction.rs:60` `extract_zfb_hi_colors_from_base16_ocean` | `verification` | Local only: `cargo test -p zfb-css --test hi_color_extraction -- --ignored --nocapture`. One-time color extraction helper for `assets/zfb-hi.css`, not a regression guard. |
| `crates/zfb/tests/version_stamp.rs:22` `version_stamp_from_env` | `heavy` | Local (T4): `cargo test -p zfb --test version_stamp -- --ignored`. Performs a full isolated `cargo run` recompile. **Also CI, weekly (T3)**: `exam.yml`'s `quarantine-heavy` job (issue #1344), allowed-to-fail. |
| `crates/zfb/tests/dev_dep_invalidation_1284_e2e.rs:497` `e2e_src_component_edit_rerenders_route` | `heavy` | Local (T4): `cargo test -p zfb --test dev_dep_invalidation_1284_e2e -- --ignored`. Level-4 e2e, spawns a real `zfb dev` server. **Also CI, weekly (T3)**: `exam.yml`'s `quarantine-heavy` job, allowed-to-fail. |
| `crates/zfb/tests/dev_dep_invalidation_1284_e2e.rs:706` `e2e_transitive_css_import_refreshes_stylesheet` | `heavy` | Same as above. |
| `crates/zfb/tests/dev_dep_invalidation_1284_e2e.rs:907` `e2e_new_utility_class_in_component_is_emitted` | `heavy` | Same as above. |
| `crates/zfb/src/render_pipeline.rs:2146` `eval_deferred_paths_via_worker_embedded_v8_non_literal_paths` | `heavy` | Local (T4): `cargo test -p zfb --lib render_pipeline:: -- --ignored`. Level-4 integration test: boots a real esbuild subprocess + embedded V8 isolate via `crate::v8_host_adapter::ThreadedV8Host`. **Also CI, weekly (T3)**: `exam.yml`'s `quarantine-heavy` job (issue #1344), allowed-to-fail. |
| `crates/zfb/tests/dev_sibling_watch_1678_e2e.rs:302` `e2e_dev_watches_workspace_sibling_raw_and_worker_sources` | `heavy` | Local: `ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb --test dev_sibling_watch_1678_e2e -- --ignored`. **Also CI, weekly (T3)**: `exam.yml`'s `quarantine-heavy` job runs it via the exact-name filterset (issue #1344), allowed-to-fail. Level-4 real `zfb dev --port 0` (issue #1678): proves editing a workspace-sibling `?raw` / module-worker source refreshes the served bundle restart-free, and that a newly-introduced sibling import makes its directory watched (keyed on the `watch-extra registered:` timing signal). Flock-adopting; serialized by the nextest `e2e-heavy` group. |
| `crates/zfb-build/tests/bundler_sibling_mirror_esbuild_regression.rs:172` `b_sibling_import_meta_glob_expands_under_unrelated_exclude` | `pending-feature` (issue #1724) | Local: `ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_sibling_mirror_esbuild_regression -- --ignored`. Real-esbuild proof that a workspace-sibling alias target's own `import.meta.glob` macro is staged raw (never expanded) — the build stays GREEN but the emitted bundle keeps the literal unexpanded macro text instead of static imports of the matched files. Blocked on issue #1724 (found while writing the sibling-mirror epic's #1698 confirm pass); not run in CI until fixed. |

No `flaky:` tagged tests exist as of this audit. The lone `verification:` helper above is a one-time extraction aid, not a regression guard.

### TS-side flaky/verification idiom (vitest)

No TypeScript/vitest test in this repo has ever needed quarantine — `grep -rn '// flaky:\|// @verification:' --include='*.test.ts' packages/ crates/*/npm` returns nothing as of this writing (issue #1349 confirmed there is nothing to retrofit). This section establishes the convention for the day one does, mirroring the Rust `#[ignore]` taxonomy above. vitest has no first-class "ignore with a machine-readable reason" attribute like Rust's `#[ignore = "..."]`, so the reason lives in a comment directly above `it.skip(...)`:

- **Quarantine a flaky test** — `it.skip(...)` with a `// flaky: <issue-url>` comment immediately above (issue URL mandatory, same rule as Rust's `flaky:`):

  ```ts
  // flaky: https://github.com/Takazudo/zudo-front-builder/issues/9999
  it.skip("retries the SSE reconnect within the 3-attempt budget", async () => {
    // ...
  });
  ```

- **One-time "it was done" proof, not a regression guard** — `it.skip(...)` with a `// @verification: <why>` comment immediately above, mirroring Rust's `verification:` tag (no issue URL needed):

  ```ts
  // @verification: one-time proof the v0.1 -> v0.2 content-schema migration
  // script produced the expected fixture snapshot; not a regression guard.
  it.skip("migrates the v0.1 fixture to the v0.2 schema", async () => {
    // ...
  });
  ```

- The other 3 Rust prefixes (`env-gate:`, `heavy:`, `pending-feature: <issue-url>`) follow the same comment-directly-above-`it.skip(...)` shape if a TS test ever needs them — no precedent yet, so no worked example here.
- **Audit**: `grep -rn '// flaky:\|// @verification:' --include='*.test.ts' packages/ crates/*/npm` — the TS mirror of the Rust manifest's `grep -rn '#\[ignore = "' crates/` rule above.
- Same rules as Rust apply: **never game the gate** (no `it.skip` for `flaky:` without a linked open issue), **quarantine has an exit with a deadline**, and a red check still blocks the author unless it carries a valid `// flaky: <issue-url>` tag (see "Required behavior" item 7 above).
- The first time a real TS test is quarantined or tagged `@verification`, add a manifest table entry here (mirroring the Rust `#[ignore]` manifest table above) so this section stops being purely aspirational.

### T3 cutover manifest (post-release)

What upgrades when zfb ships a stable release, each with an explicit trigger — do not implement any row speculatively; wait for its trigger, then treat this table (not the epic that wrote it) as the up-to-date starting point. The "do not scaffold unused tiers" rule above applies to every row here.

| Item | Current state | Trigger to upgrade |
|---|---|---|
| Weekly → nightly cadence | `exam.yml` (Sat 04:17 UTC) + `drift-net.yml` (Wed 03:43 UTC) run weekly on GitHub-hosted runners only | A stable (non-prerelease) release ships, or external adoption is high enough that a week of undetected drift is unacceptable. Bump both crons to nightly; re-check whether GitHub-hosted runners still fit the budget — still a public repo, so no blacksmith/self-hosted runners are planned by default |
| Windows exam leg | `@takazudo/zfb-win32-x64-msvc` already ships — `release.yml` natively builds it on `windows-latest` every release — but **no CI job ever executes the Rust test suite on Windows**. `health.yml`, `exam.yml`, and `drift-net.yml` are all ubuntu/macOS only | A deliberate commitment to Windows as a *tested* (not just built) platform — e.g. a Windows-specific bug report, or Windows graduating from "we ship a binary" to "we support it." Add a `windows-latest` leg to `exam.yml` mirroring `macos-exam` (full `cargo nextest run --workspace` + doctest re-exam); expect path-separator / file-locking friction the dev-server watcher has never been exercised against |
| wrangler/workerd adapter heavy lane | `packages/zfb-adapter-cloudflare/src/__tests__/d1-binding.test.ts`'s own header comment documents the gap: it uses a synthetic in-memory `D1Database` stub instead of real miniflare/`wrangler dev` because "a real miniflare D1 would test miniflare's SQLite, not this adapter's threading" — the test proves env/binding passthrough, not real D1 SQL semantics or the real Workers runtime lifecycle | The Cloudflare adapter becomes a primary supported deploy target (vs. today's acceptance-test-only confidence level), or a bug surfaces that the stub provably cannot catch. Add a `wrangler dev`/miniflare-backed integration lane — almost certainly T3/T4 (it boots a real Workers runtime), not T1 |
| Homebrew automation + verify | `scripts/update-homebrew-formula.sh vX.Y.Z --push` exists and works against `Takazudo/homebrew-tap`, but is a **manual** step run by hand after a stable release publishes (see `RELEASE_DAY_CHECKLIST.md` and `l-make-release` SKILL.md Step 11) — no CI automation, no post-update `brew install` verification | Homebrew installs are used often enough in practice to justify automating the tap push at the end of `release.yml` (stable releases only), plus a smoke step that actually `brew install`s and runs `zfb --version` against the freshly-pushed formula |
| linux-musl decision | `packages/zfb/bin/detect-musl.mjs` (wired into `packages/zfb/bin/zfb.mjs`) actively detects Alpine/musl and fails with a friendly "musl/Alpine is not supported yet — no `@takazudo/zfb-linux-*-musl` package exists" error rather than crashing opaquely on a glibc-only binary. No musl package exists under `packages/`, and `release.yml` has no musl build leg | Real demand surfaces (e.g. an issue asking for Alpine/musl support). Then decide whether to add `@takazudo/zfb-linux-x64-musl` (cross-compiled, a new `release.yml` matrix leg, a new `optionalDependencies` entry) or keep the friendly-error posture permanently — this is a product decision, not just a CI one |
| vitest coverage reporting | No `vitest.config.ts`/`.mjs` in the workspace (`packages/zfb`, `packages/zfb-runtime`, `packages/zfb-adapter-cloudflare`, `packages/create-zfb`, `crates/zfb-islands/npm`) configures a `coverage:` block — `pnpm -r test` runs vitest with zero coverage instrumentation today | Coverage becomes a stated release-readiness gate (e.g. a pre-1.0 checklist item). Add `@vitest/coverage-v8` per package and a `coverage: { provider: "v8", ... }` block to each `vitest.config.*`, and explicitly decide whether thresholds gate T1 (`health.yml`) or stay a T4/local-only signal — do not wire a hard coverage-% gate into `health.yml` without that decision being made on purpose |

### `b4push` — local pre-push pass (T4)

`pnpm b4push` (`scripts/run-b4push.sh`) runs a **bounded** fast pass before pushing, cheap → expensive: shell-script syntax (`bash -n`), the offline `tests/unit/*.sh` unit tests (actually executed — one step per file, not just parsed), `cargo fmt --check`, `pnpm format:check`, `pnpm -r --if-present typecheck`, `pnpm -r test`, and `cargo clippy -D warnings` (warm tree). It prints a per-step duration summary at the end. It is **not** the authoritative gate — `health.yml` is. b4push is the *fast subset*; the full Rust test suite is too heavy for a pre-push loop by default (V8 first-compile is 15–30 min).

- Escapes: `B4PUSH_SKIP_CLIPPY=1`, `B4PUSH_SKIP_JS_TEST=1`.
- Heavy opt-in: `B4PUSH_FULL=1 pnpm b4push` additionally runs the **full health.yml parity set** (issue #1332) — not just the workspace test suite: `cargo nextest run --workspace` (or `cargo test --workspace` when nextest is absent) using nextest's **default** profile (`retries = 0`, unlike CI's `--profile ci`); `cargo test --workspace --doc` (nextest branch only — nextest doesn't run doctests); the `zfb-md-extras` `test-utils`-gated suite plus its scoped `cargo clippy`; `cargo check --no-default-features -p zfb --tests`; the 16-test `zfb-islands` esbuild env-gate suite; #1504's real-zfb cross-pipeline acceptance test; and the three command/package locations carrying tailwindcss-v4 env-gates. The command-layer step pins both esbuild and Tailwind because it co-runs two esbuild tests and one Tailwind test. These **5 env-gate steps** skip (rather than fail) when their required staged binary slot is absent; run `cargo build --workspace --all-targets` first for full parity.
- It is **not** wired into the git `pre-push` hook (that hook only enforces the worktree-push policy above) — run it manually before pushing.
