# Bounded Linux process reaping diagnostic (#4019)

This standalone Python model uses `observer -> supervisor -> wrapper -> leaf`.
It mirrors the relevant ancestry, deepest-first SIGKILL ordering and parent
reaping obligation from `scripts/__tests__/docs-dev-supervisor.test.mjs`.
It does **not** execute zfb or the installed zudo-doc supervisor. Python idle
parents deliberately do not call `waitpid` until the cooperative TERM path;
this isolates adoption/reaping behavior without Node scheduling races.

The manager runs this once through the existing machine-wide guard:

```sh
bash "$HOME/.codex/scripts/heavy-guard.sh" -- python3 tests/process-reaping-diagnostic/diagnose.py --execute > /workspace/scratch/zfb-sweep-261008/process-diagnostic.json
```

Prerequisites: Linux `/proc`, Python exposing `os.pidfd_open` and
`signal.pidfd_send_signal`, and a kernel supporting pidfds. There is no numeric
PID signalling fallback for observer cleanup. No packages need installation.

Two cases create three processes each, sequentially. The cooperative case
signals the root with TERM; every parent signals and waits for its child. Only
if all three original identities disappear does the abrupt case run. That case
signals leaf, wrapper and root with KILL. The observer waits for its direct
root, while the two indirect descendants may become PID1 zombies. **At most two
orphan zombies can remain per invocation. Do not loop or retry this probe to
grow a sample.** All workers have an eight-second lifetime backstop. Readiness
is bounded to three seconds; observation and cleanup each to two seconds.

Each observation records UTC/monotonic timestamps, PID, PPID, state and Linux
starttime ticks; the report also records boot ID, kernel, Python and PID1 command.
PID/starttime comparisons separate identity reuse from the original process.
`kill(pid, 0)` is recorded alongside `/proc` classification, preserving the
existing test's distinction: a zombie can answer signal zero even though it is
no longer executing. The observer signals only its verified fixture identities
using pidfds, skips zombies and never signals PID1 or a process group. If
readiness fails, cleanup discovers only descendants of its still-owned root;
the abrupt case is skipped. Cooperative TERM forwarding in fixture parents
addresses their unreaped direct children; the observer's cleanup remains pidfd
addressed. No supervisor implementation or assertion is changed.

Exit zero means a valid environment observation: either orphan zombies were
observed or original identities disappeared within the two-second window. It
does **not** mean the supervisor regression suite passed. Control failure,
remaining living processes, incomplete reaping or inconclusive process states
exit nonzero. Inspect the JSON `verdict`, both final snapshots, signal events
and identities; retain the exact command, exit status and tested Git SHA.

The observer cannot reap children adopted by PID1. Their presence demonstrates
this model's environment behavior, not a zfb product defect. Their absence
only describes this observation window and model. A future comparison under
a proper init belongs to another authorized environment; this probe does not
replace PID1, install a subreaper, change runtime links or signal unrelated
processes. Source issue #4019 remains open for manager evidence reconciliation.

Recorded execution: `evidence-7163b2d8.json` contains a compact extract from the
manager's one guarded run on code commit
`7163b2d8086f44c2b6783359260d18a0bb707c8c`. The cooperative control reaped all
three original processes; the abrupt case left two PPID1 zombies and no living
survivors. See `research/4019-process-reaping-diagnostic.md` for measured times
and limitations. This container has already received its bounded run; do not
execute again here to build a larger sample.
