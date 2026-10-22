# Direct V8 phase 1 experiment

> Phase 2 status: the actual CLI integration is on `main` (merged by
> #4368 from `feat/4301-direct-v8-cli`), and since #4390 `direct-v8` is the default
> feature. Gate B (#4395) removed the deno_core host, the `embed_v8` rollback and the
> `direct_v8_baseline`/`direct_v8_outer_config` examples. The baseline-comparison harness
> (`accept.py`, and `--baseline` in `check-config.py`/`check-generated.py`/`check-http.py`/
> `check-modules.py`/`check-outer-config.py`) is archived source-bound history: it only runs
> against a checkout before the removal (e.g. `2abb238`). See [CLI.md](CLI.md)
> for selection, ownership and current verification status. The phase-1 observations below remain historical
> and source-bound; an old hold is neither a current implementation claim nor
> proof of phase-2 acceptance. Phase 3 (#4302) platform rows and their
> recording harness are in [PLATFORMS.md](PLATFORMS.md); every native row there
> starts NOT RUN.

This opt-in native experiment is tracked by #4235–#4240 and
[draft PR #4242](https://github.com/Takazudo/zudo-front-builder/pull/4242).
It does not select a new normal CLI runtime. Read [CONTRACT.md](CONTRACT.md)
for frozen semantics and [FINDINGS.md](FINDINGS.md) for actual results, failures
and remaining gates. The comparison harness is temporary feasibility evidence;
the shared service regression tests continue to protect the normal host.

## Build and config capability gate

Requirements: repository-pinned pnpm, installed Rust toolchain, **Python >=3.11**, and
the normal native V8 build prerequisites. Use the repository heavy guard for
native builds. In this portable managed session its path is
`/workspace/codex-settings/.portable-profile-home/.codex/scripts/heavy-guard.sh`; on another
host use that host's published guard, without creating another queue.

On macOS, select a compatible interpreter explicitly; the system `python3` may
be older. For example, if Python 3.11 is already installed on your PATH:

```sh
ZFB_DEV_PYTHON="$(command -v python3.11)"
"$ZFB_DEV_PYTHON" -c 'import sys; assert sys.version_info >= (3, 11); print(sys.executable)'
"$ZFB_DEV_PYTHON" -I -B -m unittest discover -s prototypes/direct-v8 -p 'test_*.py'
```

Use that same interpreter in place of `python3` in the commands below. This is a
contributor-tooling requirement, not a CLI user or
production runtime requirement.

Direct-V8 builds embed the vendored ICU 77 data at
`crates/zfb-js-host/icu/icudtl.dat` (see its `README.md`); no environment
variable or download is needed. Verify it against the pin, offline:

```sh
python3 prototypes/direct-v8/prepare-icu.py --verify
```

Without `--verify`, `prepare-icu.py` refreshes that file from a checksum-pinned
**data archive only** (a V8/ICU bump). It extracts the known data member and never
builds/imports Deno code. A Cargo cache archive can be passed using
`--archive /absolute/path/to/archive.crate`. A refresh publishes a checksum-verified
temporary sibling with atomic replacement, so a failed write or replacement preserves
the existing file. A destination symlink is replaced rather than followed. Caught
failures clean up their temporary file; an uncatchable process kill may leave an
ignored `.icudtl-*` sibling. This is atomic visibility, not a power-loss durability
guarantee.

Run the deterministic offline publication tests without Cargo, V8, network access or
a native platform build:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 prototypes/direct-v8/test_prepare_icu.py
```

These injected failures test preparation, not native ICU initialization or platform
qualification. Both checksum pins, successful bytes and the documented output path
remain unchanged.

Pass these commands to the selected heavy guard, one at a time:

```sh
cargo build --locked -p zfb-render --no-default-features --features embed_v8,syntect-onig --example direct_v8_baseline
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

## Reproducible acceptance entry point

`accept.py` runs the whole selected Linux acceptance from one command and
refuses anything that is not the intended executable:

```sh
python3 -I -B prototypes/direct-v8/accept.py --output target/direct-v8-accept-<n> \
  --guard <this-host-published-heavy-guard> \
  --generated --esbuild <pinned esbuild 0.25.12> --chromium <installed browser>
```

It verifies the vendored, checksum-pinned ICU data, builds the baseline and candidate
runners with their exact feature commands (plus the outer-config example),
runs the native library tests, hashes both runners and sends each a probe
request whose receipt must name the right engine and return the right value,
then runs the config, services, HTTP, modules and outer-config gates and, with
`--generated`, builds the postprocessor, the normal CLI and the wrapper test
binary, prepares a fresh frozen snapshot and runs the generated-input gate
with the explicit browser. All recorded executables are checked after every
gate and before final acceptance: baseline, candidate and outer-config, plus
the postprocessor, normal CLI and wrapper test binary in generated mode.
Each generated prerequisite is pinned immediately after its build, then checked
against that same digest after later builds and fixture preparation. A changed,
missing or no-longer-executable file fails the run; final hashes are enforced,
not merely recorded. The target directory comes from
`cargo metadata`, so a machine-wide `target-dir` is honored. Every step,
command, exit code, duration, executable digest and gate count is written to
`<output>/acceptance.json`; the gates' own JSON lands beside it. A missing
executable, a runner that exits 2 because its feature is off (the all-targets
trap above), a wrong engine identity or a failed probe is refused, and there is
never a baseline/Node/Deno fallback.

Run all focused refusal suites without a Cargo build or a second target directory:

```sh
python3 -I -B -m unittest discover -s prototypes/direct-v8 -p 'test_*.py'
```

`test_accept.py` uses shell-script stand-ins and mocked builds/gates to check
executable integrity throughout the full entry point. `test_accept_failures.py`
adds real small subprocesses for launch failures, nonzero exits, watchdog output
and malformed gate reports, plus injected setup failures and interruptions.
`test_accept_reports.py` adds report mutation matrices, a small subprocess pair,
and full-entry-point cases with mocked native execution and real gate/report
aggregation. `test_checker_protocol.py` runs the config, service-mode and
outer-config checkers as real subprocesses with tiny stand-in executables,
plus a bounded real watchdog. The stand-ins use only Python's standard library
and disable site-package startup. These tests prove checker/harness behavior,
not native V8 or browser behavior.

The config/services and outer-config checkers share `runner-protocol.py` for
exact framed receipt counts and failure evidence. Missing, extra, malformed or
non-object receipts fail; malformed slots do not shift later case attribution.
Success/error flags must be JSON booleans, missing values are not null, and
errors must be strings. Duplicate JSON keys, non-finite numbers and container
nesting beyond 128 levels are refused. The nesting limit keeps rejected receipts
from preventing serialization of the failure report.
LF alone frames receipts; Unicode line separators inside JSON strings are valid.
Launch failures, watchdogs and executable changes retain failed reports, with
pre/post executable hashes and captured streams. Invalid UTF-8 fails the run and
is preserved losslessly in the corresponding `stdout_base64`/`stderr_base64`
field alongside replacement-decoded diagnostics. A failed baseline does not
prevent observing the candidate. Empty or invalid config corpora are refused
before execution; the recorded corpus hash binds the bytes actually loaded,
both engines receive those same loaded inputs, and later corpus mutation fails
the overall checker even if both individual engine results passed.

On POSIX, `run_receipts` uses the bounded drain in `watchdog-drain.py` (#4289,
option A): 60 seconds of communication after spawn, then one shared second to
kill, drain and reap the direct child. Bytes written before the kill are kept
when the drain finishes in that second; `watchdog_cleanup.drain_complete` says
whether it did. A child that cannot be reaped is recorded in the report's
`released_children` and fails the check instead of being waited for. A
cancelled check still writes a failed report before it propagates. Descendants
of the runner are not cleaned up. Windows keeps the `subprocess.run` path.
`test_watchdog_drain.py` covers the drain and these ownership paths; see
`WATCHDOG-OWNERSHIP-4350.md`.

To run only the checker protocol suite:

```sh
python3 -I -B -m unittest discover -s prototypes/direct-v8 -p 'test_checker*.py'
```

Command steps are registered before launch. Failed launches retain an error and
`exit_code: null`; watchdogs also retain a null exit code rather than inventing
an observed process exit. Both retain timing, command, guard/environment context
and `passed: false`. Command stdout/stderr logs are saved on normal and failed
exits, including the raw partial bytes captured by a watchdog; partial UTF-8 is
not decoded or discarded. This does not establish clean child-tree shutdown.

A missing, unreadable, malformed or structurally invalid gate report fails that
gate while retaining its diagnostics in `errors`. A failed command cannot be
rescued by a passing report, and both diagnostics survive when the command and
report fail independently. Valid failure reports keep their row counts.
A reported `passed: true` is not sufficient evidence. Successful rows must be
nonempty, uniquely named and explicitly boolean-true. Config/services/modules
require exactly one baseline and one candidate, observed integer-zero exits,
matching nonzero expected/receipt/row counts and identical ordered case names.
Every HTTP row requires its own passing engine pair. Outer-config requests must
match the reported rows in order, with an observed successful runner exit.
Generated reports require both correctly identified passing engines, matching
case rows including `authored-mapped-throw`, passing parity rows for every
non-throw case in order, an explicitly successful non-skipped browser check with
an integer-zero exit, and a well-formed snapshot manifest digest. Boolean values
are not accepted as integer counts or exits; truthy values are not counted as
passing rows. Sparse failure reports retain whatever counts they can represent
without satisfying these success requirements, and remain failures.

These are report-consistency checks, not receipt authentication or a replacement
for the checkers' independent expected corpus, executable integrity checks or
frozen-snapshot verification. Consistent fabricated reports do not prove native
execution. Raw gate reports and historical native evidence are not rewritten.

Once the run output is initialized, `acceptance.json` is written on operational
errors and interruptions as long as the output remains writable. Unexpected
harness bugs and interruptions are recorded and re-raised, not silently swallowed.

`acceptance.json` retains completed gate summaries even when a later integrity
check refuses the run. The additive `executable_checks` ledger names each
checkpoint, its pass/fail result and checked digests or refusal diagnostic.
A gate's local success never overrides an overall integrity failure. Historical
native receipts remain unchanged; this harness change is not itself a fresh
native replay or evidence of exact-head phase-1 integration acceptance.

## Recorded phase-1 gates

A fresh 2026-10-14 Linux replay through `accept.py` is condensed in
[evidence/acceptance-linux-2026-10-14.json](evidence/acceptance-linux-2026-10-14.json)
and described in FINDINGS.md ("Fresh Linux replay"); the original receipts
below are preserved unchanged.

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
