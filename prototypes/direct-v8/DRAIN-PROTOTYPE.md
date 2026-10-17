# Isolated watchdog drain proposal (#4289)

This module is **not imported by production checkers**. The existing watchdogs,
#4292 test repair and held PR heads remain unchanged. This branch is a review
proposal, not approval of a production timing or cleanup contract.

The proposed default is 60 seconds of communication after Popen returns, plus
one shared second for direct-child kill, pipe draining and reaping: nominal
post-spawn total 61 seconds. Normal execution is not shortened. Late timeout
handling consumes the remaining absolute budget; cleanup never resets it.
Early cancellation uses min(total deadline, cancellation time + one second).
These are scheduler-dependent controller budgets, not hard API-entry ceilings:
spawn, filesystem work, hashing and scheduling stalls are not bounded here.

Timeout always leaves passed=false and exit_code=null. Retry communicate uses
input=None and replaces cumulative snapshots rather than concatenating prefixes.
Only publicly exposed snapshots can be retained: a final wait failure can hide
bytes already read inside subprocess. No lossless arbitrary-error, process-tree,
or bounded-memory claim is made. A streaming collector or stronger containment
would be a separate design.

Cleanup interruptions are recorded while cleanup continues. An original
cancellation is propagated unchanged with watchdog_cleanup attached; if cleanup
interrupts a timeout, that interruption propagates with the failed timeout report
attached. No pending execution allowance is spent after early cancellation.
Unsent stdin is closed nonblocking before retrying communication. If nonblocking
setup fails, draining is refused rather than risking a buffered blocking flush.

The caller MUST retain and eventually reap pending_child, and dispose of every
handle in pending_streams. These are live objects, intentionally not JSON-ready.
A reaped child does not imply closed pipe handles. This explicit ownership API
is why the prototype cannot be substituted into production call sites without an
owner-approved supervisor/reaper policy. A failed kill cannot promise both
bounded return and guaranteed cleanup. A second signal during bookkeeping itself
is not an OS signal-masking guarantee.

## Validation and integration boundary

Run the external POSIX test supervisor (30 seconds for the test process):

```sh
python3 -I -B prototypes/direct-v8/run-drain-prototype-tests.py
```

Tests use fake clocks/processes for exact budget arithmetic, delayed expiry,
cancellation, cleanup errors, genuine open-handle transfer and cumulative output.
Real-child tests cover normal exits, pre-output expiry, acknowledged bytes left
unread until after forced expiry, and a grandchild holding both pipes. Real
fixtures also have five-second alarms. The grandchild case uses Linux subreaper
ownership and explicitly reaps the descendant after killing its test-owned group;
on Mac it is skipped, not passed. A native Mac containment fixture and all new
implementation acceptance remain outstanding. Existing #4292 Mac acceptance is
not repeated and does not qualify this module.

This follows CLAUDE.md's development integration policy. Parent is accepted test
head e6a71183; there is no dependency on merging the prototype or #4290. Codex
owns isolated checks/review and CI diagnosis. Production call-site adaptation,
whole-tree containment, other checkers' watchdogs, native builds, measurements,
release and merge are outside this draft. Owner must decide the added cleanup
budget, live-resource ownership and any output cap before production integration.
