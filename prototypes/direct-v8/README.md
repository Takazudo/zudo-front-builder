# Direct V8 phase 1 experiment

This opt-in native experiment is tracked by #4235–#4240 and
[draft PR #4242](https://github.com/Takazudo/zudo-front-builder/pull/4242).
It does not select a new normal CLI runtime. Read [CONTRACT.md](CONTRACT.md)
for frozen semantics and [FINDINGS.md](FINDINGS.md) for actual results, failures
and remaining gates. The comparison harness is temporary feasibility evidence;
the shared service regression tests continue to protect the normal host.

## Build and config capability gate

Requirements: repository-pinned pnpm, installed Rust toolchain, Python 3, and
the normal native V8 build prerequisites. Use the repository heavy guard for
native builds. In this portable managed session its path is
`/workspace/portable-codex-home-v51/.codex/scripts/heavy-guard.sh`; on another
host use that host's published guard, without creating another queue.

From the repository root, prepare the exact baseline ICU data. This fetches a
checksum-pinned **data archive only**, extracts its known data member, and
writes it below ignored `target/`. It never builds/imports Deno code. A Cargo
cache archive can be passed using `--archive /absolute/path/to/archive.crate`.

```sh
python3 prototypes/direct-v8/prepare-icu.py
export ZFB_V8_ICU_DATA="$PWD/target/direct-v8-inputs/icu77/icudtl.dat"
```

Pass these commands to the selected heavy guard, one at a time:

```sh
cargo build --locked -p zfb-render --example direct_v8_baseline
cargo build --locked -p zfb-js-host --no-default-features --features direct-v8 --example direct_v8
cargo test --locked -p zfb-js-host --no-default-features --features direct-v8 --lib
```

The feature is non-default. Its required ICU path is a compile-time input; a
missing path fails compilation. Both runners report their engine identity.
Neither runner silently falls back to another JavaScript executor.

A later `cargo build --workspace --all-targets` can overwrite
`target/debug/examples/direct_v8` with the feature-disabled example. After any
such build, rebuild the candidate with the exact
`--no-default-features --features direct-v8 --example direct_v8` command above
before candidate checks. The recorded executable hash and engine receipt must
belong to that rebuilt binary.

Run the same frozen corpus through the independent processes:

```sh
python3 prototypes/direct-v8/check-config.py \
  --baseline target/debug/examples/direct_v8_baseline \
  --candidate target/debug/examples/direct_v8 \
  --output target/direct-v8-config-results.json
python3 prototypes/direct-v8/record-graph.py \
  --output target/direct-v8-graph.json
```

The harness checks literal expected JSON values and semantic error categories
and messages. It preserves all receipts, source-corpus hash, executable hashes
and errors. A watchdog is a failed/limited observation, never evidence of clean
shutdown. Console output is separated from the `ZFB_HOST_RECEIPT ` protocol.

The current candidate API permits one live host per OS thread, owns persistent
V8 roots until before isolate disposal, and returns only owned Rust values.
It deliberately has no unsafe Send/Sync implementation and no general op or
serialization framework. Do not load both independent V8 initializers in one
process; use the separate runners.

## Recorded phase-1 gates

Async native services/cancellation and module/Worker parity are implemented and
verified in dependency order before generated-input acceptance. The saved
receipts record 63 config cases per engine, 27 service cases per engine,
8 paired loopback HTTP cases, and 47 module/Worker cases per engine:
[config](evidence/config-gate.json), [services](evidence/services-gate.json),
[HTTP](evidence/http-gate.json), and [modules](evidence/modules-gate.json).

The real existing ZFB/esbuild/MDX/CSS/SDK producer completed at clean commit
`f2ffe216997601f5e0aa68b5625553e6700cb206`, freezing 54 files. Both independent
hosts passed all eight generated cases and all seven exact response/value
parity rows. Both hosts' negative stacks mapped to the authored `throw.ts:6:9`.
One installed Chromium 154 session rendered candidate HTML with the frozen
client assets, observed `Count: 0` → `Count: 1` and CSS `rgb(12, 34, 56)`, and
reported no page or console errors. See the
[generated receipt](evidence/generated-gate.json),
[frozen manifest](evidence/generated-manifest.json), and
[producer commands and limitations](PRODUCER.md). This bounded browser
observation does not establish a platform fleet or candidate CLI adoption.

The first producer's non-UTF-8 page failure is preserved in
[its receipt](evidence/producer-first-failure.json). The positive generated
site covers five routes; a separate prepared Worker verifies opaque binary
responses and ordered duplicate headers. The normal production asset pipeline
was not changed. [FINDINGS.md](FINDINGS.md) owns the overall acceptance ledger,
including unresolved integration checks; these passing subsets alone do not
declare the entire task complete.

No platform fleet, CLI adoption, bundler switch, release, deployment, default
switch or Deno deletion is authorized by this experiment. Keep delivery issues
open and the sole task PR draft. Generated assets and native binaries stay
local/ignored; do not use Git LFS.
