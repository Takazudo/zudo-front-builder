# Issue 4294: bounded timeout-test redesign

Base: `21d6807e442bbed61e578c165a91d64c4a7dc141`.
[Original first-run native report](https://github.com/Takazudo/zudo-front-builder/issues/4294#issuecomment-6251614904): nine recursive_dirs tests hit the 300-second runner limit; three watch_add_confirm liveness failures; two dev_bind_before_walk_e2e failures. The owner reports a low-spec Mac with nothing else running. Background load is not an established cause. Original raw logs remain at the report's Mac evidence location; this change neither replaces nor claims to have read those bytes.

Wisdom inspected at `08cc79d0a4d0f51448b8bf04a7b8065f733890c8` in Takazudo/zudo-test-wisdom: `test-integrity/deflaking-recipe.mdx`, `test-integrity/negative-assertion-fixtures.mdx`, `test-integrity/live-proof-evidence-contracts.mdx`, `decision-guide/aggregate-suite-budget.mdx`, `ci-operations/change-scoped-verification.mdx` (under `src/content/docs`). Apply the native event/deadline distinction, causal signals, separate cleanup verdict, retained negative assertions, and shared-helper dependency closure.

Follows `CLAUDE.md#development-strategy-integrate-first-use-ci-to-find-regressions`: standalone branch targets main; this session owns focused verification and exact-head PR CI triage. No merge/release authorized. The CLI/config/runtime/dev production lane owns dev.rs; the graph-identity lane and draft 4305 workflow are independent and untouched. The digest-barrier conversion depends on a coordinated production seam; watcher/helper/server changes do not.

## Contract map

The two dev rows describe the target barrier contract. The coordinated digest-release production seam is still outstanding; this branch currently adds HTTP phase budgets and a checked graceful-exit verdict while retaining the original sleep injection. It must not be described as completing the deterministic dev redesign.

All real filesystem tests remain at delivery/process level, not replaced by registration-only mocks. Helper controls are logic-level checks of the instrument. Existing execution tier, retry policy, 300-second runner cap and heavy groups remain unchanged.

| Failing identity (binary :: test) | Retained contract and evidence phases |
| --- | --- |
| recursive_dirs :: dependency_dir_spelled_as_retired_root_alias_does_not_resurrect_it | One nonrecursive repair, direct delivery retained, deep delivery absent; register → live → drain → retire/repair → sentinel → drain → shutdown |
| recursive_dirs :: dependency_registration_after_sync_does_not_downgrade_recursion | Dependency registration preserves recursion, retirement restores direct-only delivery; register → live → dependency registration → live → retire → delivery/absence → shutdown |
| recursive_dirs :: duplicate_spelling_boot_roots_repair_to_the_last_registered_spelling | Exactly one repair using last boot spelling, delivery survives; boot → live → retire/repair → live → shutdown |
| recursive_dirs :: reconciled_away_root_stops_delivering_and_only_new_roots_report | Only new roots reported, retired root silent, kept root live; registration → both live → drain → retire → sentinel/absence → shutdown |
| recursive_dirs :: retired_root_that_is_a_dependency_parent_keeps_direct_child_delivery | Recursive upgrade then nonrecursive downgrade, retained direct delivery and absent deep delivery |
| recursive_dirs :: retired_root_repair_rewatches_symlink_aliased_boot_root_once | One boot rewatch, original spelling where backend exposes it, retained edit delivery |
| recursive_dirs :: retired_root_repair_rewatches_symlink_aliased_dependency_parent_once | One dependency-parent rewatch, original spelling where backend exposes it, retained edit delivery |
| recursive_dirs :: retiring_outer_root_restores_nested_dependency_parent_watch | Nested direct delivery survives, deep and outer-only delivery absent |
| recursive_dirs :: shrinking_to_a_nested_root_keeps_nested_coverage_and_drops_the_rest | New nested root reported and live, outer-only subtree silent |
| watch_add_confirm :: real_watcher_add_content_file_serves_new_route_as_200 | Baseline 404 → real create → discovery → rendered route 200, existing route survives |
| watch_add_confirm :: real_watcher_edit_existing_file_still_hot_reloads | Initial 200 → real edit → fresh body 200; old static 200 alone was insufficient |
| watch_add_confirm :: real_watcher_inplace_edit_reaches_served_html_via_reload | Reload callback advances snapshot and real HTTP body contains V2; current-thread runtime retained |
| dev_bind_before_walk_e2e :: eager_request_before_render_serves_controlled_body | Controlled complete 404 while digest held, then 200 + V1 after explicit release |
| dev_bind_before_walk_e2e :: early_shutdown_before_digest_skips_graph_save | Digest hold acknowledged, listener active, absent graph before/after SIGINT, successful unforced exit within existing 12 seconds |

## Design and limits

- Existing watcher handshakes are retained. A new polling loop alone would not repair the reported failures. A current-label sentinel now excludes old queued traffic, and each predicate processes at most 256 events before returning to deadline accounting.
- Resettable 200ms tail quiet window remains the absence-observation rule. A separate 10-second total bound (the existing round-trip work budget) fails on continuous traffic or channel closure; it never returns a truncated success. This is a new hang bound, not an increased allowance. The real fixtures contain only a handful of writes.
- Begin/end phase receipts surround synchronous registration/reconciliation (including native unwatch), delivery, drain, and shutdown including backend drop. A missing end receipt localizes a stall; it does not prove which internal backend operation is defective. Tokio timers cannot preempt synchronous native calls. Keep the external runner backstop.
- Handshake success must be observed before its original deadline, even when a synchronous predicate blocks across it. Sleeps cap to remaining budget. The helper still cannot preempt blocking closures.
- Server readiness names the observed discovery/reload boundary and task state, not an unsupported FSEvents attribution. HTTP send and body read share the remaining absolute publication deadline. Cleanup runs after assertion failure, preserves the first panic, and treats unexpected task error/panic/success as a separate failure.
- The 15-second handshake reported at 222 seconds is not explained by this Linux work. The current-thread runtime is deliberately retained pending phase evidence. No concurrency setting is changed on speculation.
- Cold compilation occurs before test execution and is recorded separately. No test budget is inflated to cover cold Rust builds. The original ~20,049ms shutdown matches its 20-second SIGKILL fallback; forced cleanup is explicitly a failure, independently of elapsed time and exit status.
- Native delivery ordering is not certified by Linux. The quiet-window absence technique retains its original OS-ordering assumptions; diagnostics and helper controls do not establish a universal cross-stream fence.

## Verification ledger

Focused Linux evidence (Rust 1.99.1): 65 helper unit tests + 11 integration tests; all 114 watcher unit/integration tests, including 23 recursive_dirs cases; all three server cases (2.89s execution, 9s guarded build+run). Deliberately restoring late handshake acceptance, unbounded drain, stale sentinel acceptance, late cleanup acceptance, static served HTML, or unclassified truncated HTTP bodies makes the corresponding regression fail. The original assertions remain. The build integration_dev_loop consumer passed all 24 tests. The V8 CLI test binary built separately in 170s under the guard; eager rendering passed in 3.36s on Linux. Independent read-only review found no further blocking watcher/server/helper issues. Record final checks, exact head and PR CI in the PR. Preserve the original native report. No local suite retries for a green result. Inconclusive nonreproduction is not a Mac fix.

Shared handshake consumers requiring coverage include watcher lib/smoke/real_fs_behavior/poll_recursive_dirs, build orchestrator/integration_dev_loop, server combined_gap_integration, and CLI dev_supervision_e2e/wind_clean_warm_identity_e2e/plugin_watch_hook_confirm_2170_e2e. Shared-helper focused unit checks do not stand in for that closure; exact-head CI and the recorded focused/native follow-up own remaining evidence.

## Minimal native follow-up (not executed here)

Use a clean independent checkout at the final PR head SHA recorded in the PR, with the existing native Rust 1.99.1 and staged matching esbuild. Verify `git rev-parse HEAD`, clean status, tool versions, esbuild version, OS/backend and command selection first. Keep stdout/stderr and timing for compilation separately; build only the three selected test binaries, without `cargo clean`. Do not modify or dispatch draft 4305 and do not rerun the workspace suite.

Run once, zero retries, preserving all phase receipts:

```sh
cargo nextest run --locked --profile ci --retries 0 --no-fail-fast \
  -p zfb-watcher --test recursive_dirs
cargo nextest run --locked --profile ci --retries 0 --no-fail-fast \
  -p zfb-server --test watch_add_confirm
cargo nextest run --locked --profile ci --retries 0 --no-fail-fast \
  -p zfb --test dev_bind_before_walk_e2e \
  -E 'test(=eager_request_before_render_serves_controlled_body) | test(=early_shutdown_before_digest_skips_graph_save)'
```

Use the normal machine-wide heavy guard for process E2E/build work. Save fresh JUnit immediately after each command (the same profile path is overwritten), logs, actual child exits, SHA and selected/executed identities. A last `sync_recursive_dir_watches` begin receipt with no end differs from a sentinel timeout, perpetual drain, or backend-drop stall. Do not label any as background load without evidence. Confirm successful/unforced shutdown separately from cleanup. Keep issue 4294 open until the native outcomes and outstanding product defects are resolved.
