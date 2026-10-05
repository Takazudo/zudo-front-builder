# Root scripts timing: Darwin and Linux evidence (#3640)

Five idle passes and five bounded CPU-load passes completed on each requested platform. The
results below preserve the existing timeout guards; no run approached the 5 s `scripts` timeout.

## Source and toolchain

All 20 passes ran from the same clean CI merge-ref source, `8f1a1ea46ee1868716fb0c75dbe27885d69a5ee3`
(`refs/pull/3694/merge`). The runner versions recorded in every provenance file are Node 24.14.0,
pnpm 12.8.2, Vite+ 1.0.0, Vite 8.3.1, and Vitest 5.0.1.

Both matrix jobs passed in [GitHub Actions run 37254881043](https://github.com/Takazudo/zudo-front-builder/actions/runs/37254881043),
attempt 1. The uploaded artifacts are `root-script-timing-ubuntu-24.04` and
`root-script-timing-macos-15`. Each contains five Vitest JSON files and five matching provenance
files for each profile, plus `summary.json` and `summary.md`. The batch IDs are:

| Runner | Idle batch | CPU-load batch |
| --- | --- | --- |
| `ubuntu-24.04` | `idle-2026-10-05T02-18-52-869Z` | `cpu-2026-10-05T02-19-09-231Z` |
| `macos-15` | `idle-2026-10-05T02-19-02-217Z` | `cpu-2026-10-05T02-19-19-623Z` |

I verified all 20 raw JSON SHA-256 values against their provenance records. Every run reports 442
tests passed, zero failed/pending/todo, exit code 0, and 442 numeric per-test durations. Each batch
contains 2,210 per-test observations. The raw reports and per-test distributions remain in the CI
artifacts; no generated timing data is committed to the repository.

## Per-test timing distributions

Percentiles are calculated over individual test observations pooled across the five runs in that row.
For a sorted sample of length `n`, the selected index is `min(n - 1, floor(p * (n - 1) + 0.5))`,
equivalent to `round(p * (n - 1))` capped at `n - 1`. This matches the supervisor timeline summary's
locked quantile formula; it is distinct from the standard `ceil(p * n) - 1` nearest-rank formula.

| Platform | Condition | Load workers | Passes / tests each | Samples | p50 | p90 | p99 | Max |
| --- | --- | ---: | --- | ---: | ---: | ---: | ---: | ---: |
| macOS 15 arm64 | Idle | 0 | 5 / 442 passed | 2,210 | 0.18 ms | 4.81 ms | 150.99 ms | 282.19 ms |
| macOS 15 arm64 | Bounded CPU load | 1 | 5 / 442 passed | 2,210 | 0.19 ms | 5.04 ms | 159.94 ms | 726.79 ms |
| Ubuntu 24.04 x64 | Idle | 0 | 5 / 442 passed | 2,210 | 0.53 ms | 9.82 ms | 110.83 ms | 264.04 ms |
| Ubuntu 24.04 x64 | Bounded CPU load | 2 | 5 / 442 passed | 2,210 | 0.63 ms | 15.92 ms | 182.72 ms | 458.02 ms |

The load profile used one worker on macOS's three available CPUs and two workers on Linux's four.
The slowest per-test observations across all four batches were:

| Test | Condition | Max |
| --- | --- | ---: |
| `smoke-clean-room.test.mjs`: waits for the intended release, then pins all probes and npx | macOS CPU load | 726.79 ms |
| `smoke-clean-room.test.mjs`: pins a scheduled moving channel after one resolution | macOS CPU load | 498.80 ms |
| `smoke-clean-room.test.mjs`: fails a permanently stale release channel before install | macOS CPU load | 465.30 ms |
| `changelog-layout.test.mjs`: computes the next position for every current lane | Linux CPU load | 458.02 ms |
| `smoke-clean-room.test.mjs`: rejects an unavailable or invalid registry response | macOS CPU load | 359.19 ms |

## Host details and limits

| Runner | OS image | CPU | RAM | `os.freemem()` at pass start | 1-minute load average at pass start |
| --- | --- | --- | ---: | ---: | ---: |
| `ubuntu-24.04` (`ubuntu24`, image `20260927.320.1`) | Linux 6.17.0-1022-azure | AMD EPYC 7763, 4 available CPUs | 15.6 GiB | 13.8–14.4 GiB | Idle 0.49–1.13; CPU load 1.44–2.20 |
| `macos-15` (`macos15`, image `20260907.0337.1`) | Darwin 24.6.0 | Apple M1 (Virtual), 3 available CPUs | 7.0 GiB | Idle 0.39–0.52 GiB; CPU load 0.30–0.38 GiB | Idle 5.58–6.22; CPU load 6.20–9.00 |

The timing steps ran serially after setup, with no overlapping build or test job. The macOS load
average includes earlier setup/install activity and is not a measurement of test-time contention.
On macOS, `os.freemem()` reports free pages and excludes reclaimable memory; this snapshot alone
does not establish memory starvation or pressure. All measured tests passed, and the artifacts show
no timeout failures. The CPU batches exited 0 after the harness's `finally` cleanup; it creates no
standalone burner process. The run does not provide a direct macOS memory-pressure reading.

## Timeout recommendation

Keep `scripts.testTimeout` at 5,000 ms. The slowest test observation was 726.79 ms, about 6.9 times
below that guard, and the measured p99 values were 110.83–182.72 ms. These representative CI runs
do not support shortening the guard or changing suite placement.

Keep `scripts-subprocess.testTimeout` at 90,000 ms as well. These timing runs measured only the
`scripts` project; the four awaited-subprocess suites in `scripts-subprocess` were not part of the
corpus, so this data cannot justify changing that outer guard.

The temporary evidence workflow was removed after both artifacts were captured. The four requested
Darwin/Linux idle/CPU evidence cells are complete; source issue #3630's evidence gate is satisfied.
