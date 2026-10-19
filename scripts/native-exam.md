# Native macOS exam evidence

`exam.yml` runs four isolated standard `macos-15` jobs with nextest hash partitions,
plus native doctests once in a fifth job. The existing Linux quarantine/heavy manifest
and Linux `health.yml` coverage remain separate. Linux success cannot establish Mac
recovery for [#4294](https://github.com/Takazudo/zudo-front-builder/issues/4294).

## Qualification and exact source

After this workflow revision is available on the dispatch ref, inspect current runs
and dispatch **once**, explicitly naming both the workflow ref and a full lowercase
40-character commit SHA containing these scripts:

```sh
gh run list --repo Takazudo/zudo-front-builder --workflow exam.yml --limit 20
gh workflow run exam.yml --repo Takazudo/zudo-front-builder \
  --ref ci/4294-native-macos-four-shards \
  -f source_sha=FULL_40_CHARACTER_REVIEWED_COMMIT_SHA
```

Replace the branch after integration with `main`. Every checkout uses the validated
SHA. Scheduled runs use `github.sha`. The source preflight rejects symbolic refs,
abbreviations, a checkout mismatch, or a revision predating these scripts. This first
implementation intentionally requires the harness to be committed with the tested
source: it cannot retroactively qualify the pre-implementation SHA without a separate
reviewed harness/source separation. Record both the dispatch workflow revision and
source SHA in any evidence report.

The new `exam-v2-REF-SHA` concurrency namespace never cancels an old `exam-REF` run,
and `cancel-in-progress: false` keeps an active new run intact. GitHub can still replace
a pending run in the same group, so inspect first and avoid duplicate dispatches.
No local session is operated by this workflow. Manual qualification never closes the
tracking issue; scheduled recovery still requires both the Linux actual exit status
and a successful Mac reconciliation at the scheduled SHA.

## Execution contract

Tools are Rust **1.99.1**, Node **22.22.2**, repository-pinned pnpm, and nextest
**0.9.148** in every native job (the exam Linux lane uses the same pins). Nextest's
[0.9.148 source](https://github.com/nextest-rs/nextest/blob/cargo-nextest-0.9.148/Cargo.toml)
declares Rust 1.91 minimum. Node 22.22.2 satisfies the repository engine range.

Each runner installs with `pnpm install --frozen-lockfile`, independently builds
`cargo build --locked --workspace --all-targets` using the existing Rust/staged-esbuild
cache families, then asserts executable dependencies and exact esbuild/tool versions.
There is no shared build archive and no Linux/Mac binary reuse. Before tests, metadata
records source, OS/architecture, runner image, tools, run/attempt, and SHA-256 hashes
of Cargo/package manifests, locks, nextest configuration, staging pins and harness.

For each `N=1,2,3,4`, the exact nextest commands are:

```sh
cargo nextest list --locked --workspace --profile ci --run-ignored all --message-format json
cargo nextest list --locked --workspace --profile ci --partition hash:N/4 --message-format json
cargo nextest run --locked --workspace --profile ci --partition hash:N/4 --no-fail-fast --retries 0
```

[Nextest hash sharding](https://nexte.st/docs/ci-features/partitioning/) uses binary ID
plus test name. The aggregate compares those identities, not aggregate test counts.
Each shard preserves both `max-threads=1` heavy groups, per-test timeouts, and
`target/.e2e-serialize.lock`. Matrix `fail-fast: false` allows other shards to finish
and retain evidence after a failure. No assertions or ignore attributes change.

The normal `ci` profile still has **one retry**, with its existing flaky reporter;
Linux quarantine and health retain that policy. Native recovery explicitly overrides
it to **zero**. Failed, retried, timed-out, skipped-selected and unrun cases cannot be
accepted as recovery. Doctests use `cargo test --locked --workspace --doc -- --list`
followed by `cargo test --locked --workspace --doc -- --format pretty`. The saved list
and results reconcile every listed identity, recording ignored doctests separately.

## Artifacts and verdict

Artifacts retain **3 days**: `native-exam-shard-1` through `native-exam-shard-4`,
`native-exam-doctests`, and `native-summary`. Download before expiry if investigating.
Every shard contains `full.json`, `selected.json`, `junit.xml`, `run.log`,
`exit-code.txt`, `result.json`, `metadata.json`, install/build logs, and timings.
Doctests substitute `full-list.txt` and their raw result log for nextest JSON/JUnit.
Artifacts may be incomplete on preflight failure or cancellation; that is missing
evidence, never a pass.

The wrapper captures the real child exit code and exits zero so evidence can be
uploaded. **A green wrapper or matrix job is not a passing suite.** The independent
`macos-exam` aggregate fails closed unless all five expected artifacts and successful
job results exist, every source/tool/config/run identity agrees, the selected sets
are disjoint with union equal to the full nonignored Mac inventory, JUnit records
exactly every selected result with no failures/retries/skips, and native doctests
completed successfully. The existing reporter still processes all shard reports for
accurate diagnostics. No matrix `outputs.rc` is used. `native-summary` records the
verified counts, ignored counts and per-job installation/build/inventory+test times.
Its error file explains missing or rejected evidence.

This is full **nonignored, default-feature** Mac workspace coverage, not the ignored
Linux manifest, optional-feature lanes, browser tests, or release qualification.
Inventory cannot detect a test body returning early for a newly introduced dependency;
keep preflight assertions current when such dependencies change. Required known tools
are asserted rather than letting their absence silently skip tests. Unknown output
formats fail closed and require parser review; do not relax reconciliation to get green.

## Cost and performance boundaries

GitHub documents standard hosted runners as
[free for public repositories, including macOS](https://docs.github.com/en/actions/reference/runners/github-hosted-runners#standard-github-hosted-runners-for-public-repositories).
Use `macos-15` only, never paid larger runners. Short retention limits evidence storage.
Four jobs are isolated runners, not four heavyweight processes on one runner.

No fourfold speedup is promised. Every shard pays build/list overhead, hash partitions
are not duration-balanced, and runner concurrency/queues can dominate. Record the run
and job IDs, artifact IDs/digests, runner images, exact source, first attempt, and
wall-clock duration alongside the emitted build timings after native qualification.
The prior local inventory (6,854 selected, 246 binaries, 86 ignored) was owner-reported
on `21d6807e442bbed61e578c165a91d64c4a7dc141`; it is not a result measured by this change.

## Cheap regression checks

```sh
sh tests/unit/native-exam.sh
bash tests/unit/report-flaky-tests.sh
bash tests/unit/file-exam-issue.sh
bash tests/unit/check-exam-ignore-parity.sh
actionlint .github/workflows/exam.yml
```

The native-exam shell entry is automatically included by health's `tests/unit/*.sh`
loop. Fixtures were captured from a small Linux Rust 1.99.1 / nextest 0.9.148 package;
they verify parser/protocol behavior and are explicitly **not native Mac evidence**.
Full native qualification is still required after implementation and review.
