# #4019: bounded process-tree environment investigation

Scope follows the confirmed four-scope evidence sweep and
`CLAUDE.md#development-strategy-integrate-first-use-ci-to-find-regressions`.
Integration parent is captured main `cb4b6dbd3af21980b302916c5d97d4f767c51574`;
this standalone investigation has no dependency on the browser, cascade or
YAML units. The manager owns guarded execution, independent review, integration
and exact-head CI. The child performs syntax checks and foreground self-review.
No production supervisor, runtime infrastructure, security setting or test
assertion is changed.

## Existing evidence and invariant

`scripts/__tests__/docs-dev-supervisor.test.mjs` starts a detached Node
supervisor, walks descendants deepest first for SIGKILL, also signals the
private process group during fixture teardown and polls `process.kill(pid, 0)`
until ESRCH. The pre-UP regression requires an empty survivor list, including
the hidden leaf. Zombies continue to answer signal zero, so this invariant
requires actual reaping rather than merely cessation of execution. It remains
unchanged. #3991's merged exact-head CI acceptance remains satisfied; that
historical acceptance is distinct from container process-reaping acceptance.

The manager reports that current PID1 is `tail`. The installed supervisor
source path referenced by the test,
`docs/node_modules/@takazudo/zudo-doc/bin/run-parallel.mjs`, is absent in this
child worktree. Therefore the new fixture models the test's ancestry and signal
ordering; it is not provenance evidence for the installed package or a replay
of the full suite.

## Deliverable and interpretation

`tests/process-reaping-diagnostic/diagnose.py` creates a three-process tree and
uses cooperative TERM/explicit parent waits as a positive reaping control,
then deepest-first SIGKILL as an abrupt teardown observation. Python parents
deliberately stay idle until TERM so orphan behavior can be isolated from Node
child-exit scheduling. This is a stated model difference, not an inference
about production timing. It records ancestry, process states, original
PID/starttime identities, UTC/monotonic timestamps, signals, boot ID and PID1.
Cleanup uses pidfds and only this fixture's living processes. At most two
indirect children may remain as zombies adopted by PID1; those are observable
but not reapable by this observer. The cooperative case must pass before the
abrupt case starts, and runs must not be repeated to accumulate zombies.

`orphan-zombies-observed` supports the narrow claim that this container leaves
minimal abrupt orphan descendants as zombies within the measured window.
`original-identities-gone-within-window` refutes that claim for this model and
run, without proving future or full-supervisor behavior. Any live survivor or
control failure is a failed diagnostic requiring manager investigation. Exit
zero denotes a completed environment observation, never product correctness.

## Evidence ledger

- Prepared: bounded standalone fixture and usage/interpretation documentation.
- Child check: Python source compiled in memory with `compile(..., 'exec')`;
  no subprocess diagnostic or supervisor suite execution in the child lane.
- Manager execution: pending; no environment verdict claimed here.
- Supporting artifact to retain: `/workspace/scratch/zfb-sweep-261008/process-diagnostic.json`,
  plus guarded command, exit status and exact tested commit.
- Source tracker #4019 remains open. Existing supervisor tests retain their
  original assertions and require separate evidence when the manager executes
  them. #3991 historical CI acceptance is not relabelled as environment success.

Run instructions, strict bounds and prerequisites are in the fixture README.
No PID1 replacement, subreaper change, global kill or package installation is
part of this investigation. Native host discovery and personal startup context
are outside this fixture's evidentiary scope.
