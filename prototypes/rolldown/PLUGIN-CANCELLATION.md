# Owned plugin probe cancellation

Continuation of draft #4120 from `0130f922f6ebdec324b5b9fe35bf02102623a844`.
This follows [the publication barrier](PLUGIN-PUBLICATION.md). It implements
cooperative cancellation and explicit worker/publication ownership, not a hard
native shutdown deadline or production plugin adoption.

## Implementation boundary

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

## Why timeout is not termination

[Tokio runtime shutdown documentation](https://docs.rs/tokio/latest/tokio/runtime/struct.Runtime.html#shutdown)
explicitly distinguishes stopping the wait from stopping the work. Blocking
tasks can continue after `shutdown_timeout` or `shutdown_background`. This probe
uses neither. Bounded quiescence remains an admission gate, not a claim inferred
from elapsed time or a cancellation flag.

## Regression coverage

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

## Integration and remaining gates

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
