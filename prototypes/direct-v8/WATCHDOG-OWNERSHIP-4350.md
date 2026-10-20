# Watchdog cleanup ownership and missing tests (#4350, for #4289 E001-a)

This is design, static review and test drafts only. It does not change #4293,
adopt a timing contract, implement a production adapter, or rerun the original
Mac race fixed by #4292. Production references are main `ea90fe12`; the
prototype is #4293 `509113ae` (`watchdog-drain-prototype.py`). Test drafts are in
`draft_watchdog_ownership_4350.py` (not discovered by `test_*.py`).

Evidence labels: **static** = read from source; **draft run** = the drafts ran
once on Linux x86_64 (Python 3.13.16) against the prototype, with no Mac run.

## 1. Callers mapped to the prototype API

The prototype replaces only the transport inside `run_receipts`. Every other
watchdog in the table keeps its own `subprocess.run` and contract.

| Layer | Spawns | Timeout today | Owner of child / streams / reap on each exit |
| --- | --- | --- | --- |
| `runner-protocol.py:56` `run_receipts` | runner (direct child) | 60 s `subprocess.run` | Normal: `run` reaps and closes. Timeout: POSIX `kill()` + unbounded `wait()`, then `Popen.__exit__` closes and waits again; unread bytes at the kill are dropped. Spawn `OSError`: recorded as a failure. Cancellation: `run` kills and waits, then propagates. |
| `check-config.py:37` `run` (called twice by `main`: baseline, candidate) | via `run_receipts` | default 60 s | Writes `json.dumps(report)`; catches only `OSError`/`ValueError`. |
| `check-outer-config.py:31` `run` / `main` | via `run_receipts` | 60 s (tests 0.3 s) | `main` has no exception handling around `run`. |
| `accept.py:289` `gate` → `Acceptance.run:218` | `bash <guard> -- python check-*.py` | 7200 s per command | On timeout kills only the direct child (the guard shell when guarded); the checker, its runner and descendants have no owner. Reads the report file right after. |
| Runner descendants (e.g. `direct_v8_outer_config` → config loader → esbuild, `loader.rs:273`) | grandchildren | loader's own `output_bounded` | Nobody above the runner. esbuild's stdio is captured by the loader (static), so it does not hold the runner's pipes. |
| `accept.py:130` `verify_runner`, `check-generated.py:142`, `check-modules.py:107`, `check-http.py:201` | runners | 60 / 45 / 60 / 10 s | Separate contracts; not covered by this design. `check-http` shares one deadline with its server drain, so an appended cleanup second would conflict. |

## 2. Proposed 60 s + shared 1 s vs the current contract

| Property | Current `run_receipts` (POSIX) | #4293 prototype |
| --- | --- | --- |
| Clock start | `communicate` entry, after `Popen` returns | `_clock()` after `Popen` returns (same point) |
| Execution allowance | 60 s | 60 s |
| After expiry | `kill()`; unbounded `wait()`; unbounded `__exit__` wait | `kill()`, bounded drain and `poll()` sharing one 1 s window (`D_total = t0 + 61 s`); no reset |
| Late bytes at the kill | dropped | recovered when the drain finishes within the window |
| Unreaped child | waited for indefinitely | handed to the caller as `pending_child` |
| Windows | `kill()` + unbounded `communicate()` | refuses (`ValueError`, POSIX only) |

Neither version promises:

- a ceiling measured from API entry: digests, spawn, logging and scheduler stalls are outside both;
- an output-size or memory bound;
- cleanup of descendants;
- lossless capture after arbitrary errors;
- child readiness.

The 61 s is a controller budget, not an end-to-end guarantee.

## 3. State and ownership table

| Exit path | Bytes kept | Direct child | Streams | Accountable owner | Evidence |
| --- | --- | --- | --- | --- | --- |
| Normal exit, any code | all | reaped by `communicate` | closed | prototype | #4293 `test_normal_success_and_nonzero_exit` |
| Pre-output timeout | empty, no base64 | killed, drained, polled | closed | prototype | #4293 `test_real_pre_output_timeout`; #4292 `test_real_watchdog_before_first_output_fails_outer_rows` |
| Timeout with partial bytes | cumulative snapshot, no duplicated prefix | killed, drained | closed | prototype | #4293 `test_real_late_bytes_are_drained_without_duplicate_prefix`, `test_shared_deadline_and_cumulative_snapshots`; #4292 ready-fixture tests |
| Valid receipt after timeout | kept, still failed | as above | closed | prototype | #4293 `test_valid_receipt_after_timeout_cannot_rescue_failure`; #4292 `test_valid_ready_receipts_do_not_rescue_watchdog` |
| Kill fails | last public snapshot | `pending_child` | closed | **caller, but none exists today (U3, U4)** | #4293 `test_kill_failure_and_final_wait_without_snapshot_retains_public_bytes` |
| Drain error / close or poll error | kept | `pending_child` if not reaped | closed or `pending_streams` | **caller (U3, U4)** | #4293 `test_cleanup_io_failure…`, `test_close_and_poll_failures_are_explicit`, `test_genuine_open_stream_failure…` |
| Grandchild holds pipes, direct child killed | kept | killed, polled | closed | prototype for the child; **nobody for the grandchild (U5)** | #4293 `test_real_grandchild_held_pipes_return_without_tree_claim` (Linux; Mac skipped) |
| Grandchild holds pipes, direct child exits 0 | kept; watchdog failure; `cleanup_exit_code=0` | reaped | closed | prototype; **nobody for the grandchild (U5)** | **new draft** `test_direct_child_exits_zero_while_grandchild_holds_stdout` (draft run: pass) |
| Timeout observed after `D_total` | kept | killed, never waited, so usually `pending_child` | closed | **caller (U2)** | #4293 `test_expired_total…` (only with an already-reaped poll); **new draft** `test_u2…` |
| Cancellation during communicate | kept on `exc.watchdog_cleanup` | killed within `min(D_total, t + 1 s)` | closed | prototype, then **checker main (U7)** | #4293 `test_early_cancellation…`, `test_secondary_cleanup_interrupt…`, `test_cleanup_interrupt_after_timeout…` |
| Stdin cannot go nonblocking | kept | drain refused; usually `pending_child` | stdin in `pending_streams` | **caller (U3)** | #4293 `test_nonblocking_setup_failure…` |
| Interrupt between `Popen` and the deadline stamp | none | **not killed, not recorded (U1)** | open | **nobody** | **new draft** `test_u1…` (`expectedFailure` on 509113ae) |
| Spawn `OSError` | none | none | none | adapter must catch it (the prototype lets it propagate) | static |
| Non-POSIX host | n/a | n/a | n/a | adapter must keep the legacy path (U8) | **new draft** `test_u8…` |

## 4. Paths with no accountable owner

- **U1:** an interrupt between `_popen` and `start = _clock()` / result creation (prototype lines 25–32) escapes with no kill and no record. The window is small but unguarded.
- **U2:** a timeout observed after the shared total skips every wait. `poll()` right after `kill()` usually returns `None`, so a child that is already dying is handed over as pending.
- **U3:** `pending_child`, `pending_streams` and the raw `bytes` streams are not JSON-serializable (draft run: `TypeError`).
  - `check-config.py` `main` would crash without writing a report.
  - `accept.py` would then record `invalid gate report`.
  - No caller exists yet that accepts live handles.
- **U4:** a checker process that exits with a pending child leaves it to init or launchd, with no record. Bounded return after a failed kill cannot also guarantee reaping.
- **U5:** descendants of the runner have no owner at any layer. Nothing uses a process group, subreaper or cgroup.
- **U6:** `Acceptance.run`'s 7200 s timeout kills only the guard shell. The checker keeps running unowned and may write its report after `gate` has already read it (static).
- **U7:** cancellation propagates through both checker mains without writing a report (draft run: no file).
- **U8:** the prototype is POSIX only, while the current code also runs on Windows, where the post-kill `communicate()` is unbounded.

## 5. Recommended design

**Reaper contract (adds no new wait):** a process-lifetime `PendingRegistry` in `runner-protocol.py` owns every `pending_child` and `pending_stream`.

- `run_receipts` stores only a JSON-safe projection: pid, stream count, and `save_stream` encoding of the bytes.
- At checker exit, `release()` polls and closes once.
- A child that is still alive is recorded as `released_to_os` in the report, and the report stays failed.

This states honestly what can be promised. Guaranteed cleanup is not achievable here:

- A failed kill cannot be both bounded and reaped.
- macOS has no subreaper.
- A descendant can `setsid` out of any process group.

**Limited stronger option** (a separate decision, not part of the minimal patch): spawn the runner with `start_new_session=True` and `killpg` on timeout or cancellation. This covers ordinary descendants such as esbuild. It changes signal delivery, because terminal Ctrl-C no longer reaches the runner, so cancellation must `killpg` explicitly. Descendants that `setsid` are still not covered.

**Minimal adapter patch** (exact locations, not implemented):

1. **`runner-protocol.py:95–105`:** replace `subprocess.run` with the drain on POSIX and keep it on non-POSIX (U8). Then:
   - catch spawn `OSError` as today;
   - map `stdout`/`stderr` through `save_stream`;
   - map `watchdog` to the existing failure message;
   - map `cleanup_errors` and `pending_child_pid` to failures;
   - project the live handles into the registry (U3).
2. **The drain module, around the spawn:** guard the bookkeeping so an interrupt there still kills and attaches the child (U1). This is a change to the prototype's successor, not to #4293.
3. **`check-config.py:98–125` and `check-outer-config.py:61–70`:**
   - write a failed report on `BaseException` (recording `cancelled` and `watchdog_cleanup`), then re-raise (U7);
   - call `release()` in `finally` and record the result (U4).
4. **No change:**
   - `accept.py` (U6 needs the containment decision);
   - the other checkers;
   - timing values.

## 6. Missing deterministic tests (drafts)

| Draft | Asserts | Negative control | Draft run on 509113ae |
| --- | --- | --- | --- |
| `test_u3_raw_result_is_not_json_and_projection_moves_ownership` | projection is JSON; registry owns the child; release reports `released_to_os` | `json.dumps(raw result)` raises `TypeError` | pass |
| `test_u2_zero_remaining_hands_off_a_killed_child_without_any_wait` | kill sent, no wait, child pending; a later poll reports the exit (delayed reap) | — | pass (documents U2) |
| `test_u1_interrupt_between_spawn_and_bookkeeping_still_attaches_child` | an interrupted deadline stamp still kills and attaches the child | the prototype as is | expected failure (killed 0, no `watchdog_cleanup`) |
| `test_u8_non_posix_is_refused…` | the prototype refuses non-POSIX before spawning | spawn callback fails the test | pass |
| `test_direct_child_exits_zero_while_grandchild_holds_stdout` (Linux subreaper; Mac NOT RUN) | watchdog failure, `exit_code` null, `cleanup_exit_code` 0, streams closed, return < 3 s | the fixture owner kills the group and reaps the grandchild | pass |
| `test_u7_cancellation_still_writes_a_failed_report` | `check-config` main writes a failed report on cancellation | current main | expected failure (no report file) |

The existing 15 #4293 tests and the #4292 readiness tests cover the remaining guarantees listed in section 3. An approved adapter must flip both `expectedFailure` drafts to plain passes. It also needs:

- the full checker suite and independent review at its own SHA;
- a native Mac acceptance run of that new SHA only.

## 7. Proposed E001-a decision

Choose one:

1. **Recommended:** adopt option A (60 s + one shared 1 s) for `run_receipts` only, together with the reaper contract and the minimal adapter in section 5.
   - "Partial output preserved" becomes a guarantee only when the drain completes inside the window.
   - It is best effort otherwise, and the report must say which happened.
   - No production readiness signal: #4292's test-only seam is enough, and the 60 s allowance has about 250× headroom over the measured Mac fresh-file first write (median 240 ms). The loaded Mac case was only measured beyond 1 s, so this headroom is an inference, not a bound.
   - Descendant cleanup stays explicitly unclaimed.
2. **Keep production unchanged:** document the current lossy kill boundary and the unbounded post-kill wait, and leave #4293 as a reference.
3. **Containment:** require descendant cleanup, which needs the process-group option or OS-level containment, and accepts the residual `setsid` gap.

Option B (59 s + 1 s) would shorten execution and is not recommended without a measured reason.
