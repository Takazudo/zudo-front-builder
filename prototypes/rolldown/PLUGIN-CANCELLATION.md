# Owned plugin probe supervision and cancellation

Continuation of draft #4120 from `8cd839627b491431f8755d61235c81c3c8a75221`.
This follows [the publication barrier](PLUGIN-PUBLICATION.md). It implements
cooperative cancellation and explicit worker/publication ownership, not a hard
native shutdown deadline or production plugin adoption.

## Supervisor increment from `8cd83962`

The standalone CLI now exercises a prototype-private `Supervisor` with capacity two.
The synchronous root owns all helper handles. Requests own only a delivery
mailbox and a cancellation signal: dropping an unpolled request, a polled future,
or its joined result revokes unpublished output without joining native work,
shutting down a runtime, sleeping, or transferring a worker from a destructor.
Only the root's explicit `drain()` (and its safety-backstop Drop) can wait without
bound. The root must live outside the request executor; it is not clonable or
retained by client handles.

Admission reserves a registry entry and a fixed join lane **before** worker
spawn. Setup error/unwind rolls the reservation back. `Busy` is immediate at
capacity; `Closed` rejects admission after shutdown. A cancelled, expired,
closing, runtime-tearing-down or thread-exiting worker keeps its slot until
actual join. Completed bundles also keep their slot until the delivered result
is published or discarded. This deliberately conservative bound prevents an
unbounded completed-output queue. Discarding a receiver before delivery causes
the late result to be discarded; it never transfers publication to the worker.
No native worker, helper handle or completed bundle is leaked/detached.

There are exactly capacity fixed join lanes and one deadline coordinator,
created once at supervisor construction. Registry locks cover bookkeeping only;
spawn/setup, native execution, runtime teardown, joins, delivery wakeups and
filesystem publication occur outside them. A blocked join on one lane does not
block the other lanes or the coordinator. There is no reaper per abandoned job.

Independent review reproduced a subtle reason for fixed join lanes:
`JoinHandle::is_finished()` may be true while a thread-local destructor still
blocks `join()`. A single polling reaper that immediately joins such handles
would still head-of-line block. The real-native regression installs a gated TLS
destructor from a close hook and proves another job joins and deadline/shutdown
control continues while it is held. Publication still requires actual join.

### State and deadline contract

| Observation | Meaning |
| --- | --- |
| `cancellation_requested` | Atomic publication revocation won; native work may still run |
| `deadline_exceeded` | The cancellation deadline has elapsed, even if a prior publication commit made cancellation `TooLate` |
| `joined` | Native worker join, including private runtime and thread exit, was collected |
| `Pending(statuses)` | Admission is closed; root still owns helper handles and possibly live job identities |
| `Drained` | Explicit root drain successfully joined every owned helper and worker |
| `Closed` | New submission is rejected; not a termination claim |

The constructor/submission deadline is a **job cancellation deadline**. The
independent coordinator checks it without client polling, including after result
delivery. Native checkpoints and publication also check it. Publication samples
deadline eligibility immediately before its atomic ACTIVE-to-COMMITTING decision;
cancellation/shutdown competing before that CAS rejects output. This is a sampled
eligibility decision, not a strict wall-clock guarantee that the CAS occurs
before the Instant. After commit starts, cancellation returns `TooLate`, and an
elapsed deadline does not falsely acknowledge cancellation. Original native
errors remain primary even when cancellation also occurred.

The retained low-level `Job::wait` regression API remains **wait-only**: timeout
returns the owned Pending job and does not imply cancellation. The supervised
request is a Future; a caller can stop polling without changing the job deadline,
and dropping that future revokes publication.

`shutdown()` closes admission, requests cancellation on all retained unpublished
results, and returns without joining even helper threads. It reports Pending
until the synchronous root explicitly drains; Pending may have no job identities
when only helper joins remain. `drain()` is unbounded and must not run on a request
executor. A helper failure permanently returns an error, never a later false
Drained; root drain still collects remaining owners. A successful drain covers
owned native workers and helpers, not unrelated/global Rolldown work below.

The CLI preserves its successful output behavior. At 300 seconds it reports the
job identity, revoked publication and pending shutdown, then explicitly drains
on its synchronous root. A permanently blocked operation can still prevent CLI
process exit. Prompt reporting, bounded job count and bounded process termination
are three different properties; only the first two are implemented here.

### Native regressions and verification

The example now contains 42 Linux tests: all 26 previous tests plus 16 supervisor
regressions. Real-native cases cover a current-thread Tokio heartbeat after
request-future drop; capacity 1 cancellation/retries in load, close and private
runtime teardown; capacity 2 independent completion; deadline expiry without
receiver polling; late successful delivery to abandoned receivers; shutdown
admission closure and joined-only drain; delivered output cancellation; deadline
and shutdown after commit returning TooLate; and the TLS destruction boundary.
Native errors, worker panic, injected spawn/setup error and unwind, helper failure,
and both queued and transferred output abandonment retain/reconcile accounting.

Deadline races use a per-signal test-only expiry trigger after the gate/commit
handshake, plus real elapsed-Instant submission rejection; no global fake clock or
startup-speed assumption is needed. Controlled gates have Drop-based release guards so failed assertions release
native work before the root's drain. Tests have ten-second deadlock guards; local
heavy-guard and the existing CI job provide external watchdogs. Watchdog expiry
is a failure, never a product termination assertion. These finite gates do not
prove interruption of permanent blockage.

Local verification exposed a retained fixture race twice: its queued
`spawn_blocking` callback could be discarded by runtime shutdown before it ever
sent the test handshake, while generation returned Ok. Both native pause fixtures
now handshake that the blocking task has started before close returns. Existing
assertions and ten-second guards remain unchanged; failure-safe release ordering
and scoped publisher ownership were independently reviewed. The failed runs are
not relabeled passes; the repaired suite is replayed.

The unchanged read-only exact-SHA verifier discovers these example tests and
retains all 36 differential rows (including post-bundle dependency edits), four
library tests, locked build/feature-off check, formatting and cleanliness checks.
Local verification used Linux x86_64, Rust 1.99.0 and Node 24.19.0; CI retains Rust
1.99.0 and Node 24.14.0. Final commit/run evidence is recorded on PR #4120; previous
run 38681448453 belongs to `8cd83962` and is not current-head evidence.

### Exact-pin hard-shutdown investigation

At `24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9`:

- [Module loader](https://github.com/rolldown/rolldown/blob/24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9/crates/rolldown/src/module_loader/module_loader.rs#L351)
  spawns module tasks without retained join handles; message/remaining counters
  track ordinary completion rather than acknowledged cancellation.
- [Source loading](https://github.com/rolldown/rolldown/blob/24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9/crates/rolldown/src/utils/load_source.rs#L63)
  uses runtime blocking tasks. [Build hooks](https://github.com/rolldown/rolldown/blob/24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9/crates/rolldown_plugin/src/plugin_driver/build_hooks.rs#L50)
  and [close hooks](https://github.com/rolldown/rolldown/blob/24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9/crates/rolldown_plugin/src/plugin_driver/output_hooks.rs#L293)
  directly await plugin code. Close does not interrupt a hook already running.
- [Link/generation](https://github.com/rolldown/rolldown/blob/24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9/crates/rolldown/src/bundle/bundle.rs#L279)
  includes synchronous CPU work; [finalization](https://github.com/rolldown/rolldown/blob/24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9/crates/rolldown/src/stages/generate_stage/finalize_modules.rs#L58)
  and minification use Rayon without a cancellation token.
- [Bundle close](https://github.com/rolldown/rolldown/blob/24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9/crates/rolldown/src/bundle/bundle_handle.rs#L71)
  awaits hooks and clears retained driver state, not active task joins.
- [Post-generation destruction](https://github.com/rolldown/rolldown/blob/24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9/crates/rolldown/src/bundle/bundle.rs#L297)
  moves link-stage data to [global Rayon work](https://github.com/rolldown/rolldown/blob/24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9/crates/rolldown/src/utils/defer_drop.rs#L75).
  Its private drain runs at [the next bundle construction](https://github.com/rolldown/rolldown/blob/24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9/crates/rolldown/src/bundle/bundle_factory.rs#L157),
  not at close/private Tokio shutdown. Thus even an owned worker join is not a
  proof of universal native background quiescence.

No supported pinned API gives an acknowledgement covering all these operations.
No unsafe termination, concurrent driver clearing, future abort, runtime timeout,
process backend, dependency change or speculative upstream bug report is added.
The native load/close/runtime/TLS gate tests reproduce the retained ownership
boundary; global deferred-drop timing is source evidence, not a new execution claim.

Production integration remains gated: `plugin_bundler.rs` relies on a 300-second
subprocess termination bound below its 600-second stale-file threshold, including
Windows' pre-lock write interval. It requires entry-adjacent staging.
`plugin_runner.rs` prebundles before Node startup and retains stage guards through
host lifetime; hook timeouts kill/reap that separate child. Responsive native
supervision cannot replace those termination/staging contracts. The live caller,
Node host, configuration path, shared backend, provenance admission, size budgets
and generated artifacts remain untouched. Keep #318/#4098/#4123/#4124/#4133 open,
#3328 -> #3329 -> #4099 and #3230 unchanged, and PR #4120 open/draft.

This session is the verification/CI-triage owner under the integrate-first policy.
Dependency order: owned API -> admission/shutdown -> real-native regressions ->
integrated verification -> documentation/report. No newer main was merged.

## Earlier owned-job increment at `8cd83962`

The existing native example now runs its one-shot generation on an owned thread
with a private Tokio runtime. The thread only generates bytes; it never receives
a publication capability. Its normal-return close behavior, package resolver,
compiler options and existing compatibility assertions remain unchanged.

`Cancellation` atomically revokes publication and installs native checkpoints
at build start, module load, render start and bundle generation. A reached
checkpoint returns a native plugin error through the existing lifecycle helper,
which still owns close/driver cleanup. Cancellation does not clear driver state
concurrently, drop a running generation future, abort an OS thread, or skip
close. It cannot interrupt native CPU work or a blocked hook before another
checkpoint is reached.

`Job::wait` distinguishes `Joined(Completed)` from `Pending(Job)`. The latter
retains the live worker owner and does not implicitly cancel. A completed build
or completed close hook alone is not enough: only joining the worker after its
private runtime teardown constructs the object that can publish. The worker's
panic or native error is collected at this same boundary.

Cancelling before the publication commit point prevents output even when native
work later succeeds. Publication and cancellation compete using one atomic state
transition. Cancellation after commit starts returns `TooLate`, rather than
acknowledging a cancellation while a synchronous file write is already running.
Existing exclusive-create and incomplete-write cleanup are retained.

The standalone CLI waits up to 300 seconds for the worker, requests cancellation
on expiry and then keeps ownership while joining. It prints that shutdown is
not bounded. `Job::join` and the safety-backstop `Drop` can block indefinitely;
`Drop` cancels and joins instead of silently detaching. A future async production
caller must retain Pending owners in a supervisor, not drop this blocking guard
on its executor. This increment does not expose a public production API.

## Earlier rationale: timeout is not termination

[Tokio runtime shutdown documentation](https://docs.rs/tokio/latest/tokio/runtime/struct.Runtime.html#shutdown)
explicitly distinguishes stopping the wait from stopping the work. Blocking
tasks can continue after `shutdown_timeout` or `shutdown_background`. This probe
uses neither. Bounded quiescence remains an admission gate, not a claim inferred
from elapsed time or a cancellation flag.

## Earlier regression coverage

The new tests run the real native generator and use channel handshakes to pause
it inside a load hook, inside close, and inside a blocking task spawned by close.
They require Pending ownership while work is paused, no premature output or
concurrent close, and no publication after acknowledged cancellation, including
late successful completion. The runtime-blocking-task case distinguishes actual
worker join from merely finishing generation/close. A native load cancellation
must reach a real cancellation checkpoint and return its diagnostic.

Additional cases cover successful joined publication, cancellation after join,
late cancellation after commit begins, concurrent/idempotent cancellation,
owner-drop joining, panic collection and preservation of native build errors.
The small filesystem/state-machine tests complement, not replace, real native
execution. Test waits have separate ten-second deadlock guards; they are not
production performance or shutdown guarantees.

The existing read-only `rolldown-plugin-publication.yml` verifier includes these
new example tests automatically and replays all original 36 esbuild/native
semantic rows, the library tests, locked native build, feature-off check and
Rust formatting. Run-specific results must be recorded on the PR; configured or
unexecuted tests are not verification evidence.

## Earlier integration record (superseded supervisor wording)

This increment follows
`CLAUDE.md#development-strategy-integrate-first-use-ci-to-find-regressions`.
Its integration parent is the existing draft `prototype/318-rolldown-native`
branch, not main. Dependency order is publication barrier -> owned cancellation
-> bounded shutdown/admission -> production staging -> caller/host acceptance.
This session owns exact-head verifier triage; no child performs a workspace
rebuild. The real plugin caller, shared native adapter, Node host, configuration
loader, dependency manifests/lockfiles, Wind provenance and Deno-exit work are
untouched. No subprocess backend, public selector or fallback is introduced.

A permanently blocked native operation still requires a termination/admission
strategy. The current owner is safe against abandoned writes but intentionally
not a bounded shutdown solution. General failure-tolerant multi-plugin cleanup,
production stage locks/stale sweeping, caller-to-host integration, platform and
Cargo-provenance admission remain separate. Keep #4120 open/draft; no merge,
default switch, release, package publication or deployment is included.
