# Execution evidence at 292dbce9

These are original local execution logs and compiler metadata from the clean
implementation revision `292dbce9b7d35ce4f4300c4125b881fac4ccd6c7`, before the
empty recovery commit or this evidence-only commit. `SHA256SUMS.json` records
copied bytes. Text extensions keep captured JSON/log output unchanged by source
formatters. No binary, tarball or generated site is committed here.

The six-case compiler metadata records exact source and binary SHA-256,
original/edited values, five export-file hashes, archive hash, candidate counts,
and compiled CSS hashes. Each case includes actual build, audit and explanation
stdout/stderr. The binary was rebuilt at this revision; it was not an old
installed scaffold binary. Browser and starter logs use that revision's built
outputs. See the parent verification README for exact command forms.

`b4push.txt` and `supervisor-retry.txt` intentionally preserve failures. Three
unchanged subprocess assertions encounter PPID1 zombies in this managed Linux
container; this is failed/deferred under #4019, not a green local full suite.
The full b4push run started before the final evidence-only commit, so its source
snapshot predates 292dbce9 only in screenshot/README/test-title evidence files;
its failing supervisor sources are unchanged from main.

`final-packed.txt` includes the expected initial install warning for local
binary version 0.0.0, followed by the smoke's supported local-tarball overrides
and successful scaffold/build assertions. It does not publish packages.

The GitHub health startup failure had two skipped WASM jobs and no executed
health/no-V8 jobs. The earlier description of zero jobs was imprecise. Required
CI gates remain mandatory; these local logs do not replace missing CI results.
