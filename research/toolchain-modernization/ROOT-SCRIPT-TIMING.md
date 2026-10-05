# Root scripts timing: harness and evidence gates (#3640)

This note prepares the repeated per-test timing work requested by #3640. The harness is ready for
the manager's controlled runs; no new measurement is claimed here. The historical Darwin sample
below is the issue's planning baseline, not evidence for this head or for loaded conditions.

## Harness

[`scripts/root-script-timing.mjs`](../../scripts/root-script-timing.mjs) invokes only the root
Vitest `scripts` project, sequentially, with `--reporter=json` and a distinct JSON file for every
pass. It writes a per-pass provenance JSON plus batch `summary.json` and `summary.md`. The summary
reports p50/p90/p99/max across individual test observations and per-test distributions over all
passes, with the 20 largest observed test maxima.
Percentile ranks use the repository's locked nearest-rank quantile convention from the supervisor
timeline summary. Each run also records Vitest's total, passed, failed, pending, and todo test counts
so a partial or changed corpus is visible alongside the timed samples.

The harness captures Node, pnpm, Vite+, Vite, Vitest, OS release and architecture, CPU model and
available/logical core counts, total and starting free memory, load average, source SHA/branch/dirty
state, and recognized GitHub Actions run identifiers. Each raw report is hashed and linked from its
provenance record. Passes run serially so no timing batch overlaps its own runs.

CPU mode starts a user-selected number of Node worker threads, capped at four and at the host's
available parallelism. Each worker runs a fixed-size integer loop without creating files, child
processes, or growing memory. The workers start before Vitest and are stopped in `finally`, including
when the command fails. SIGINT/SIGTERM is forwarded only to the runner's detached process group;
after five seconds the harness sends SIGKILL to that same group and still cleans up its workers.
The cleanup test uses an idle worker and a simulated runner failure; it does not conduct a loaded
measurement.

Artifacts go under the requested output directory in a unique batch subdirectory. Point it at a
temporary directory or upload the directory from CI; do not commit raw reports.

## Reproduction commands

On Darwin, run each profile as its own foreground command when the machine is idle and has no
overlapping build:

```sh
node scripts/root-script-timing.mjs --profile idle --runs 5 --output-dir "$TMPDIR/zfb-root-script-timing"
node scripts/root-script-timing.mjs --profile cpu --runs 5 --output-dir "$TMPDIR/zfb-root-script-timing"
```

On Linux CI, run the same two commands after checkout and dependency installation, in one job step
with no concurrent build/test step in that job:

```sh
node scripts/root-script-timing.mjs --profile idle --runs 5 --output-dir "$RUNNER_TEMP/zfb-root-script-timing"
node scripts/root-script-timing.mjs --profile cpu --runs 5 --output-dir "$RUNNER_TEMP/zfb-root-script-timing"
```

Upload the full `$RUNNER_TEMP/zfb-root-script-timing` directory as a CI artifact, retaining its
workflow run URL, run ID, attempt, and commit SHA. The manager owns temporary Linux workflow wiring
and its removal after preserving the artifact and run URL. If it shares a diagnostic step with the
rawHtml size check, both outputs can live under `$RUNNER_TEMP/epic-diagnostics` and be uploaded
together. Do not add a standing workflow lane for this one-off evidence.

Vite+ reports its bundled runner in `pnpm exec vp --version`; record that output with the measured
artifacts rather than relying on the older runner name in source documentation. The current
worktree reports Vite+ 1.0.0, Vite 8.3.1, and Vitest 5.0.1.

## Evidence status

| Platform / condition | Evidence required | Status |
| --- | --- | --- |
| Darwin arm64, idle | 5 sequential passes, per-test JSON and provenance | Pending manager run |
| Darwin arm64, bounded CPU load | 5 sequential passes, per-test JSON and provenance | Pending manager run |
| Linux CI x64, idle | 5 sequential passes, CI artifact and traceable run URL | Pending manager run |
| Linux CI x64, bounded CPU load | 5 sequential passes, CI artifact and traceable run URL | Pending manager run |

The issue's prior Darwin arm64 / 8 GiB sample at `70699ac` passed 438/438 with p50 0.30 ms,
p90 18.90 ms, p99 1039.03 ms, and max 2262.31 ms. It is a single idle sample on an older SHA. The
issue also says the available Linux sample was a local container, not Linux CI. Neither completes
the platform/condition matrix above.

The repository remains at `scripts.testTimeout = 5000` ms and
`scripts-subprocess.testTimeout = 90000` ms. The 90 s project remains limited to suites that await
subprocess work. No timeout or suite placement change is supported until representative healthy
Darwin and Linux CI distributions are attached with the source and environment provenance. Slow
tests and any possible memory-starvation signatures must be reviewed against their run logs before
calling them hangs. Keep source issue #3630 open until both platform/condition evidence sets exist.
