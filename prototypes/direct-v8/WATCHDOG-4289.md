# Watchdog scope and evidence (#4289)

The [Mac diagnosis](https://github.com/Takazudo/zudo-front-builder/issues/4289#issuecomment-6222264867)
separates pre-output startup expiry from bytes left unread at the kill boundary.
The observed natural failures were startup expiry; post-kill loss was reproduced
separately with an instrumented edge probe.

## Test-only repair

The partial-byte test waits for a socket acknowledgment after the fixture writes
both streams, using a test-local `Popen` subclass. Its communication wrapper
feeds the fixture's input and closes stdin before waiting for acknowledgment.
The fixture reads stdin to EOF, records it, writes both streams and signals ready.
The test verifies exact input delivery and then delegates to real `communicate`
with the unchanged 300ms timeout. Real `subprocess.run` still owns kill and wait.
The input is asserted to fit within `PIPE_BUF` before feeding the empty pipe,
so cold interpreter startup cannot block that write. Readiness failure is a test failure and reaps the
direct fixture child. Its 10-second safety ceiling matches the existing CLI
fixture ceiling; it is not a new production timeout.

A separate unsynchronized real-child fixture blocks before either stream can
write. Its test requires watchdog failure, empty stdout/stderr, no base64 fields,
zero receipts and seven failed rows. It passes through the original timeout path
whether the interpreter has started or is still starting. The partial-byte test still requires exact invalid UTF-8
bytes and stderr, failed outer rows, and null reported exit status. Additional
controls reject missing readiness and an expired readiness wait, and ensure
valid receipts cannot rescue a watchdog failure. No fixed startup sleep, retry,
production seam, deadline increase, or optional base64 field change is involved.

This proves captured-byte reporting after fixture readiness. It does not prove
complete recovery of bytes written at the kill boundary, native V8 behavior,
production readiness, or descendant cleanup. The source production files remain
unchanged. Native macOS acceptance of these Python tests is still required; no
native build is necessary.

## Relationship to the Mac diagnosis

The Mac report is correct that an ordinary fixture cannot make the production
`subprocess.run` deadline wait for interpreter readiness. This patch does not
make that claim. Only the output-dependent test uses a `Popen` subclass whose
communication wrapper sends the small fixture input, waits for acknowledgment,
and then invokes the real `communicate(timeout=0.3)`. Consequently cold interpreter
startup is outside this test's 300ms communication window, bounded instead by its
explicit 10-second fixture readiness wait. This is a test-local stdlib seam; the
production API and runtime policy are unchanged.

The test proves exact reporting of bytes already written before the communication
timeout is armed, and retains the original stdin-then-output-then-timeout sequence.
It does not prove output will appear within 300ms of an ordinary spawn. The
unsynchronized silent fixture explicitly covers timeout before first output.
No test here proves recovery of unread bytes at the kill boundary, or repairs the
native cold-start latency.

## Production choices requiring agreement

1. **Keep the current contract (recommended for this test PR).** Keep the
   spawn/communication behavior and best-effort captured output. Diagnose any
   post-kill recovery separately. Watchdog results stay failed with null reported
   exit status, even if valid receipts are captured.
2. **Add bounded diagnostic draining.** Replace `subprocess.run` with explicit
   process supervision; after kill, drain both streams for a specified cleanup
   allowance, then close pipes and report incomplete recovery if descendants
   retain them. Decide whether that allowance is additional teardown time or
   reserved inside an absolute invocation ceiling. The former increases total
   elapsed time; the latter reduces the usable execution budget. Neither is a
   behavior-neutral repair. An immediate nonblocking snapshot adds no waiting
   grace but cannot promise complete draining.
3. **Add production readiness and containment.** Specify a readiness protocol,
   startup/runtime budget partition, absolute ceiling, cleanup allowance, and
   supported process containment. Do not restart an unrestricted runtime deadline
   after readiness. POSIX process groups alone cannot guarantee cleanup of escaped
   descendants. Current documentation explicitly disclaims child-tree cleanup;
   this would be a new guarantee, not preservation of an existing one.

Any drain implementation needs regression coverage for both streams, prior and
late bytes without duplication, invalid UTF-8, missing readiness where applicable,
valid receipts remaining failed on timeout, inherited open pipes, bounded return,
start/cancellation failures, and direct-child reaping. `accept.py` has a separate
watchdog implementation and must not be described as fixed by a transport-only
change.

## Integration boundary

This topic follows `CLAUDE.md#development-strategy-integrate-first-use-ci-to-find-regressions`.
Its parent is PR #4242 at `dcf0c8a42cf177ff4d334b40f249b4d97465c2e4`;
the prototype does not exist on main. It does not depend on or absorb #4290's
separate TMPDIR fix. Codex owns this topic's Python checks, review and CI diagnosis;
its parent owner owns subsequent integration. Keep the topic draft and unmerged.
No held parent head, release, runtime default, or budget policy changes here.

Linux Cloud can validate Python transport tests only. Mac follow-up: at the exact
new topic head, run the following with default and canonical TMPDIR values:

```sh
python3 -I -B -m unittest discover -s prototypes/direct-v8 -p 'test_checker*.py'
```

Record Python/OS,
source SHA, both results and any failure without pass-on-retry relabeling. The
full suite's existing TMPDIR alias defect remains tracked separately in #4290.
